//! Structured diagnostics, safe repairs, and redacted support bundles.
//! Text and JSON output are deliberately rendered from the same `Report`.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capabilities;
use crate::config::{self, Config};
use crate::providers::{self, Reach};

pub const REPORT_SCHEMA_VERSION: u32 = 1;

const STATE_SCAN_ENTRY_LIMIT: usize = 10_000;
const HISTORY_WARN_BYTES: u64 = 64 * 1024 * 1024;
const CONFIG_WARN_BYTES: u64 = 128 * 1024 * 1024;
const SESSION_WARN_BYTES: u64 = 512 * 1024 * 1024;
const CAPABILITY_WARN_BYTES: u64 = 32 * 1024 * 1024;
const RUNTIME_WARN_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub id: String,
    pub status: Status,
    pub severity: Severity,
    pub summary: String,
    pub detail: String,
    pub fixable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_paths: Vec<PathBuf>,
}

impl Check {
    fn new(
        id: impl Into<String>,
        status: Status,
        severity: Severity,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            status,
            severity,
            summary: summary.into(),
            detail: detail.into(),
            fixable: false,
            changed_paths: Vec::new(),
        }
    }

    fn fixable(mut self, value: bool) -> Self {
        self.fixable = value;
        self
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Paths {
    pub config: PathBuf,
    pub credentials: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub history: PathBuf,
    pub capability_dir: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub generated_at_ms: u128,
    pub version: String,
    pub platform: String,
    pub paths: Paths,
    pub checks: Vec<Check>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capability_report: Option<capabilities::Report>,
}

impl Report {
    pub fn critical_ok(&self) -> bool {
        !self
            .checks
            .iter()
            .any(|check| check.severity == Severity::Critical && check.status == Status::Fail)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub probe: bool,
    pub live: bool,
    pub fix: bool,
}

pub fn inspect(version: &str, options: &Options) -> Report {
    let paths = resolved_paths();
    let mut checks = Vec::new();
    checks.push(Check::new(
        "version",
        Status::Pass,
        Severity::Info,
        format!("version: aishe {version}"),
        "running binary metadata",
    ));

    let zsh = crate::executor::which("zsh");
    let bash = crate::executor::which("bash");
    match (&zsh, &bash) {
        (Some(path), _) => checks.push(Check::new(
            "shell.backing",
            Status::Pass,
            Severity::Critical,
            format!("backing shell: zsh ({})", path.display()),
            "the interactive PTY uses the installed zsh",
        )),
        (None, Some(path)) => checks.push(Check::new(
            "shell.backing",
            Status::Warn,
            Severity::Critical,
            format!("backing shell: bash ({}) — zsh not found", path.display()),
            "non-interactive use and the bash hook work; install zsh for `aishe`",
        )),
        (None, None) => checks.push(Check::new(
            "shell.backing",
            Status::Fail,
            Severity::Critical,
            "backing shell: none found",
            "install zsh or bash",
        )),
    }
    checks.push(if zsh.is_some() {
        Check::new(
            "shell.frontend",
            Status::Pass,
            Severity::Critical,
            "front-end: zsh-pty (wraps your real zsh)",
            "native zsh line editing, plugins, job control, and history remain active",
        )
    } else {
        Check::new(
            "shell.frontend",
            Status::Fail,
            Severity::Critical,
            "front-end: zsh-pty needs zsh",
            "install zsh; `aishe -c` and `aishe init bash` remain available",
        )
    });
    checks.push(shell_compatibility_check());
    checks.push(shell_keybinding_check());

    let (mut config, config_valid) = match Config::load_quiet() {
        Ok(Some(config)) => {
            let schema = Config::schema_version_on_disk()
                .ok()
                .flatten()
                .unwrap_or(config.version);
            let current_schema = config::CONFIG_SCHEMA_VERSION;
            let (status, detail) = if schema < current_schema {
                (
                    Status::Warn,
                    format!(
                        "schema {schema} on disk; the next normal command migrates it atomically \
                         to schema {current_schema} after creating a timestamped backup"
                    ),
                )
            } else {
                (Status::Pass, format!("schema {schema}"))
            };
            checks.push(
                Check::new(
                    "config.file",
                    status,
                    Severity::Critical,
                    format!("config: {}", paths.config.display()),
                    detail,
                )
                .fixable(true),
            );
            (config, true)
        }
        Ok(None) => {
            checks.push(Check::new(
                "config.file",
                Status::Warn,
                Severity::Critical,
                format!("config: not created at {}", paths.config.display()),
                "run `aishe setup`; Doctor never invents an unverified provider config",
            ));
            (Config::default(), false)
        }
        Err(error) => {
            checks.push(
                Check::new(
                    "config.file",
                    Status::Fail,
                    Severity::Critical,
                    format!("config: malformed at {}", paths.config.display()),
                    crate::redact::redact(&error.to_string()),
                )
                .fixable(false),
            );
            (Config::default(), false)
        }
    };

    checks.push(match crate::credentials::Store::load() {
        Ok(Some(_)) => Check::new(
            "credentials.file",
            Status::Pass,
            Severity::Warning,
            format!("credentials: {}", paths.credentials.display()),
            "private shared credentials file is valid; values were not displayed",
        ),
        Ok(None) => Check::new(
            "credentials.file",
            Status::Pass,
            Severity::Info,
            format!(
                "credentials: not created at {}",
                paths.credentials.display()
            ),
            "run `aishe auth set PROFILE` or let interactive setup save a key",
        ),
        Err(error) => Check::new(
            "credentials.file",
            Status::Fail,
            Severity::Warning,
            format!(
                "credentials: unsafe or unreadable at {}",
                paths.credentials.display()
            ),
            crate::redact::redact(&error.to_string()),
        )
        .fixable(true),
    });

    match std::env::current_dir()
        .ok()
        .and_then(|cwd| config.apply_project_overlay(&cwd))
    {
        Some(outcome) if outcome.error.is_some() => checks.push(Check::new(
            "config.project",
            Status::Warn,
            Severity::Warning,
            format!("project config: malformed at {}", outcome.path.display()),
            crate::redact::redact(outcome.error.as_deref().unwrap_or("unknown error")),
        )),
        Some(outcome) => checks.push(Check::new(
            "config.project",
            if outcome.deferred.is_empty() {
                Status::Pass
            } else {
                Status::Warn
            },
            Severity::Warning,
            format!(
                "project config: {} ({})",
                outcome.path.display(),
                if outcome.trusted {
                    "trusted"
                } else {
                    "untrusted"
                }
            ),
            format!(
                "{} applied; {} deferred{}",
                outcome.applied.len(),
                outcome.deferred.len(),
                if outcome.deferred.is_empty() {
                    String::new()
                } else {
                    format!(": {}", outcome.deferred.join(", "))
                }
            ),
        )),
        None => checks.push(Check::new(
            "config.project",
            Status::Pass,
            Severity::Info,
            "project config: none",
            "using user configuration and command-line overrides",
        )),
    }

    append_policy_checks(&mut checks, &mut config);
    crate::ui::configure(&config.ui);
    let terminal = crate::ui::TerminalCapabilities::detect_stdout();
    checks.push(Check::new(
        "ui.terminal_policy",
        Status::Pass,
        Severity::Info,
        format!(
            "terminal UI: {:?} theme · {:?} color · {:?} glyphs · {:?} motion",
            terminal.theme, terminal.color_depth, terminal.unicode, terminal.motion
        )
        .to_ascii_lowercase(),
        format!(
            "{}x{} cells; NO_COLOR, TERM=dumb, redirection, and JSON can reduce the configured presentation policy",
            terminal.columns, terminal.rows
        ),
    ));
    append_backend_checks(&mut checks, &config, config_valid, options);

    let provider = config.active_provider_name().to_string();
    let provider_config = active_provider(&config);
    let connection_id = config.active_connection_id().to_string();
    let connection_label = config
        .active_connection()
        .map(|connection| connection.label.as_str())
        .unwrap_or(&connection_id);
    checks.push(Check::new(
        "provider.active",
        Status::Pass,
        Severity::Info,
        format!(
            "connection: {connection_label} ({connection_id}) · provider: {provider} · model {}",
            provider_config.model
        ),
        format!(
            "{} · transport {}",
            provider_config.base_url, provider_config.transport
        ),
    ));
    if !config.aishe.provider_fallback.is_empty() {
        checks.push(Check::new(
            "provider.fallback",
            Status::Pass,
            Severity::Info,
            format!(
                "fallback chain: {} → {}",
                provider,
                config.aishe.provider_fallback.join(" → ")
            ),
            "fallbacks are tried only after the active provider fails",
        ));
    }

    let auth = config
        .active_connection()
        .map(crate::connection::auth_status)
        .unwrap_or(crate::connection::ConnectionAuthStatus {
            kind: "legacy".into(),
            profile: None,
            available: false,
            detail: "connection metadata unavailable".into(),
        });
    checks.push(Check::new(
        "provider.credential",
        if auth.available { Status::Pass } else { Status::Warn },
        Severity::Warning,
        format!(
            "authentication: {}{} · {}",
            auth.kind,
            auth.profile
                .as_deref()
                .map(|profile| format!(" profile '{profile}'"))
                .unwrap_or_default(),
            auth.detail
        ),
        if auth.available {
            "secret values stay in private stores and are never written to diagnostics".into()
        } else {
            format!(
                "authentication for connection '{connection_id}' is unavailable; use `aishe connection show {connection_id}`"
            )
        },
    ));

    if options.probe && !options.live {
        let probe = providers::probe(&config, &provider);
        let (status, detail) = match probe.reach {
            Reach::Up(status) => (
                Status::Pass,
                format!("{provider}: reachable [HTTP {status}] ({})", probe.endpoint),
            ),
            Reach::Unauthorized(status) => (
                Status::Warn,
                format!(
                    "{provider}: reachable but key rejected [HTTP {status}] ({})",
                    probe.endpoint
                ),
            ),
            Reach::ManagedOAuth(oauth_provider) => (
                Status::Pass,
                format!(
                    "{provider}: {oauth_provider} OAuth ready; endpoint validation runs through managed OpenCode ({})",
                    probe.endpoint
                ),
            ),
            Reach::Down(error) => (
                Status::Warn,
                format!(
                    "{provider}: unreachable ({}) — {}",
                    probe.endpoint,
                    crate::redact::redact(&error)
                ),
            ),
        };
        checks.push(Check::new(
            "provider.reachability",
            status,
            Severity::Warning,
            "reachability probe:",
            detail,
        ));
    }

    let capability_report = if options.live && config_valid {
        let report = capabilities::validate(&config, true);
        checks.extend(capability_checks(&report));
        Some(report)
    } else {
        capabilities::load(&config)
    };

    append_sandbox_checks(&mut checks, &config);

    checks.push(Check::new(
        "privacy.redaction",
        if config.aishe.redact_secrets {
            Status::Pass
        } else {
            Status::Warn
        },
        Severity::Warning,
        format!(
            "secret redaction: {}",
            if config.aishe.redact_secrets {
                "on"
            } else {
                "off"
            }
        ),
        "redaction applies to context, diagnostics, and configured audit fields",
    ));
    let audit_on = config.logging.enabled
        || matches!(
            std::env::var("AISHE_LOG").ok().as_deref(),
            Some("1") | Some("true") | Some("yes")
        );
    checks.push(Check::new(
        "logging.audit",
        if audit_on { Status::Pass } else { Status::Warn },
        Severity::Info,
        format!("audit log: {}", if audit_on { "on" } else { "off" }),
        if audit_on {
            format!(
                "{}; redact {}",
                config
                    .logging
                    .file
                    .clone()
                    .unwrap_or_else(|| crate::audit::default_path().display().to_string()),
                if config.logging.redact { "on" } else { "off" }
            )
        } else {
            "enable with [logging] enabled=true or AISHE_LOG=1".into()
        },
    ));

    let enabled_mcp = config
        .mcp_servers
        .values()
        .filter(|server| server.enabled)
        .count();
    checks.push(Check::new(
        "mcp.servers",
        Status::Pass,
        Severity::Info,
        if config.mcp_servers.is_empty() {
            "MCP servers: none configured".into()
        } else {
            format!(
                "MCP servers: {} configured ({enabled_mcp} enabled)",
                config.mcp_servers.len()
            )
        },
        "run `aishe mcp` for the configured server list",
    ));
    checks.push(
        Check::new(
            "history.persistence",
            if paths.history.exists() || !config_valid {
                Status::Pass
            } else {
                Status::Warn
            },
            Severity::Warning,
            format!(
                "history: {} ({})",
                paths.history.display(),
                if config.aishe.share_history {
                    "shared; persistent across upgrades"
                } else {
                    "per-session"
                }
            ),
            if paths.history.exists() {
                "history file exists and is never removed by setup or update"
            } else {
                "the file will be created on first command; `doctor --fix` can create it now"
            },
        )
        .fixable(config_valid),
    );
    append_retention_checks(&mut checks, &paths, &config);
    checks.push(Check::new(
        "statusline",
        Status::Pass,
        Severity::Info,
        format!(
            "statusline: {}",
            if config.aishe.status_line {
                config.aishe.status_line_position.as_str()
            } else {
                "off"
            }
        ),
        format!("items: {}", config.aishe.status_line_items.join(", ")),
    ));
    checks.push(Check::new(
        "pricing.active_model",
        if crate::usage::price_for(&provider_config.model, &config.pricing).is_some() {
            Status::Pass
        } else {
            Status::Warn
        },
        Severity::Info,
        if crate::usage::price_for(&provider_config.model, &config.pricing).is_some() {
            format!("pricing: configured for '{}'", provider_config.model)
        } else {
            format!("pricing: no price for '{}'", provider_config.model)
        },
        if crate::usage::price_for(&provider_config.model, &config.pricing).is_some() {
            "cost and budget estimates are available".into()
        } else {
            format!(
                "set exact rates with `aishe price set {} --input USD --output USD`",
                provider_config.model
            )
        },
    ));

    if options.fix {
        checks.push(apply_safe_fixes(&paths, config_valid));
    }

    Report {
        schema_version: REPORT_SCHEMA_VERSION,
        generated_at_ms: now_ms(),
        version: version.to_string(),
        platform: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        paths,
        checks,
        capability_report,
    }
}

fn shell_compatibility_check() -> Check {
    let shell = std::env::var_os("SHELL")
        .and_then(|value| PathBuf::from(value).file_name().map(|name| name.to_owned()))
        .and_then(|name| name.to_str().map(str::to_ascii_lowercase))
        .unwrap_or_else(|| "unknown".into());
    let pty = std::env::var_os("AISHE_OUR_ZDOTDIR").is_some()
        || std::env::var_os("AISHE_PTY_PROMPT").is_some();
    let hook = std::env::var_os("AISHE_PENDING_FILE").is_some();
    let bash_version = std::env::var("BASH_VERSION").ok().or_else(|| {
        std::process::Command::new("bash")
            .arg("--version")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|output| output.lines().next().map(str::to_string))
    });
    let (tier, surface, caveat) = compatibility_tier(&shell, pty, hook, bash_version.as_deref());
    Check::new(
        "shell.compatibility",
        Status::Pass,
        Severity::Info,
        format!("interactive shell: {tier} · {surface}"),
        caveat,
    )
}

fn shell_keybinding_check() -> Check {
    let bindings = [
        (
            "force-agent",
            std::env::var("AISHE_NL_KEY").unwrap_or_else(|_| "^[^M".into()),
        ),
        (
            "mode-cycle",
            std::env::var("AISHE_MODE_KEY").unwrap_or_else(|_| "^[[Z".into()),
        ),
        (
            "fix-last",
            std::env::var("AISHE_FIX_KEY").unwrap_or_else(|_| "^X^F".into()),
        ),
        (
            "edit-buffer",
            std::env::var("AISHE_EDIT_KEY").unwrap_or_else(|_| "^X^A".into()),
        ),
        (
            "semantic-recall",
            std::env::var("AISHE_RECALL_KEY").unwrap_or_else(|_| "^X^R".into()),
        ),
    ];
    let collisions = keybinding_collisions(&bindings);
    if collisions.is_empty() {
        Check::new(
            "shell.keybindings",
            Status::Pass,
            Severity::Info,
            "AIShe keybindings: distinct",
            "force-agent, mode-cycle, fix-last, edit-buffer, and semantic-recall use distinct configured sequences",
        )
    } else {
        Check::new(
            "shell.keybindings",
            Status::Warn,
            Severity::Warning,
            "AIShe keybindings: configured conflict",
            format!(
                "{}; rebind AISHE_NL_KEY, AISHE_MODE_KEY, AISHE_FIX_KEY, AISHE_EDIT_KEY, or AISHE_RECALL_KEY",
                collisions.join("; ")
            ),
        )
    }
}

fn keybinding_collisions(bindings: &[(&str, String)]) -> Vec<String> {
    let mut collisions = Vec::new();
    for (index, (left_name, left_value)) in bindings.iter().enumerate() {
        for (right_name, right_value) in bindings.iter().skip(index + 1) {
            if !left_value.is_empty() && left_value == right_value {
                collisions.push(format!(
                    "{left_name} and {right_name} both use {left_value:?}"
                ));
            }
        }
    }
    collisions
}

fn compatibility_tier(
    shell: &str,
    pty: bool,
    hook: bool,
    bash_version: Option<&str>,
) -> (&'static str, String, &'static str) {
    if pty {
        return (
            "Tier A",
            "zsh PTY".into(),
            "flagship routing, native zsh editing/plugins, prompt, status, approvals, and agent rendering",
        );
    }
    if hook && shell.contains("zsh") {
        return (
            "Tier A-",
            "native zsh hook".into(),
            "full-buffer route/staging contract; the user's prompt and plugin stack remain authoritative",
        );
    }
    if hook && shell.contains("bash") {
        let version = bash_version.unwrap_or("unknown version");
        let major = version
            .split(|character: char| !character.is_ascii_digit())
            .find(|part| !part.is_empty())
            .and_then(|part| part.parse::<u32>().ok());
        if major == Some(3) {
            return (
                "Tier B-",
                format!("native Bash {version}"),
                "core hook routing/state is qualified; Ctrl-G, recall, Shift-Tab, Ctrl-O, and Ctrl-X Ctrl-F use documented alternatives",
            );
        }
        return (
            "Tier B",
            format!("native Bash {version}"),
            "declared Bash hook matrix; reduced full-buffer classification and Bash-native # comments remain",
        );
    }
    (
        "Tier C",
        format!("non-interactive/current {shell}"),
        "stable -c, pipe, route, and JSON contracts; launch `aishe` for Tier A or install a documented native hook",
    )
}

fn append_policy_checks(checks: &mut Vec<Check>, config: &mut Config) {
    match crate::policy::load() {
        Ok(None) => checks.push(Check::new(
            "policy.organization",
            Status::Pass,
            Severity::Info,
            "organization policy: none",
            format!("no policy file at {}", crate::policy::path().display()),
        )),
        Ok(Some(loaded)) => match loaded.policy.constrain(config) {
            Ok(()) => checks.push(Check::new(
                "policy.organization",
                Status::Pass,
                Severity::Critical,
                format!("organization policy: {}", loaded.path.display()),
                "validated and applied as the highest-precedence constraint layer",
            )),
            Err(error) => checks.push(Check::new(
                "policy.organization",
                Status::Fail,
                Severity::Critical,
                "organization policy denied the active configuration",
                crate::redact::redact(&error.to_string()),
            )),
        },
        Err(error) => checks.push(Check::new(
            "policy.organization",
            Status::Fail,
            Severity::Critical,
            format!(
                "organization policy: invalid at {}",
                crate::policy::path().display()
            ),
            crate::redact::redact(&error.to_string()),
        )),
    }
}

fn append_backend_checks(
    checks: &mut Vec<Check>,
    config: &Config,
    config_valid: bool,
    options: &Options,
) {
    let runtime_required = config.backend.engine == "opencode";
    checks.push(Check::new(
        "backend.engine",
        if config.backend.engine == "opencode" {
            Status::Pass
        } else {
            Status::Warn
        },
        Severity::Critical,
        format!("agent engine: {}", config.backend.engine),
        if config.backend.engine == "opencode" {
            "all new AI turns use the managed OpenCode adapter"
        } else {
            "native is a time-limited compatibility/repair backend"
        },
    ));

    let manager = match crate::backend::RuntimeManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            checks.push(Check::new(
                "backend.runtime.present",
                if runtime_required {
                    Status::Fail
                } else {
                    Status::Warn
                },
                if runtime_required {
                    Severity::Critical
                } else {
                    Severity::Warning
                },
                "managed runtime: path unavailable",
                crate::redact::redact(&error.to_string()),
            ));
            return;
        }
    };
    let manifest = manager.manifest();
    let status = manager.status();
    let (runtime_ready, runtime_version, runtime_hash) = match &status {
        crate::backend::RuntimeStatus::Ready {
            version, sha256, ..
        } => (true, version.clone(), Some(sha256.clone())),
        crate::backend::RuntimeStatus::Missing { expected_version } => {
            (false, expected_version.clone(), None)
        }
        crate::backend::RuntimeStatus::Invalid {
            expected_version, ..
        } => (false, expected_version.clone(), None),
    };
    checks.push(
        Check::new(
            "backend.runtime.present",
            if runtime_ready {
                Status::Pass
            } else if runtime_required {
                Status::Fail
            } else {
                Status::Warn
            },
            if runtime_required {
                Severity::Critical
            } else {
                Severity::Warning
            },
            if runtime_ready {
                "managed runtime: installed"
            } else {
                "managed runtime: unavailable"
            },
            match &status {
                crate::backend::RuntimeStatus::Ready { binary, .. } => binary.display().to_string(),
                crate::backend::RuntimeStatus::Missing { .. } => {
                    "run `aishe backend install` or `aishe setup`".into()
                }
                crate::backend::RuntimeStatus::Invalid { reason, .. } => {
                    format!("{reason}; run `aishe backend repair`")
                }
            },
        )
        .fixable(true),
    );
    checks.push(Check::new(
        "backend.runtime.version",
        if runtime_ready && runtime_version == manifest.version {
            Status::Pass
        } else if runtime_required {
            Status::Fail
        } else {
            Status::Skipped
        },
        if runtime_required {
            Severity::Critical
        } else {
            Severity::Warning
        },
        format!("managed runtime version: {runtime_version}"),
        format!("this AIShe build requires exactly {}", manifest.version),
    ));
    checks.push(Check::new(
        "backend.runtime.hash",
        if runtime_hash.is_some() {
            Status::Pass
        } else if runtime_required {
            Status::Fail
        } else {
            Status::Skipped
        },
        if runtime_required {
            Severity::Critical
        } else {
            Severity::Warning
        },
        "managed runtime hash",
        runtime_hash.unwrap_or_else(|| "not available because verification failed".into()),
    ));
    let license = manager.version_dir().join("LICENSE");
    let notices = manager.version_dir().join("THIRD_PARTY_NOTICES.md");
    checks.push(Check::new(
        "backend.runtime.license",
        if license.is_file() && notices.is_file() {
            Status::Pass
        } else {
            Status::Fail
        },
        Severity::Warning,
        "managed runtime license notices",
        if license.is_file() && notices.is_file() {
            format!("{}; {}", license.display(), notices.display())
        } else {
            "LICENSE or THIRD_PARTY_NOTICES.md is missing; repair the runtime".into()
        },
    ));

    let instance_keys = crate::backend::supervisor::instance_keys().unwrap_or_default();
    let mut loaded_count = 0usize;
    let mut verified_states = Vec::new();
    for key in &instance_keys {
        if crate::backend::control::load_state_for(key)
            .ok()
            .flatten()
            .is_some()
        {
            loaded_count += 1;
        }
        if let Some(state) = crate::backend::control::verified_state_for(key)
            .ok()
            .flatten()
        {
            verified_states.push(state);
        }
    }
    if instance_keys.is_empty() {
        if crate::backend::control::load_state()
            .ok()
            .flatten()
            .is_some()
        {
            loaded_count += 1;
        }
        if let Some(state) = crate::backend::control::verified_state().ok().flatten() {
            verified_states.push(state);
        }
    }
    checks.push(Check::new(
        "backend.supervisor",
        if !verified_states.is_empty() {
            Status::Pass
        } else if loaded_count > 0 {
            Status::Warn
        } else {
            Status::Skipped
        },
        Severity::Warning,
        if !verified_states.is_empty() {
            "backend supervisor pool: running"
        } else if loaded_count > 0 {
            "backend supervisor pool: stale or unauthenticated state"
        } else {
            "backend supervisor: stopped (idle)"
        },
        format!(
            "{} running of {} recorded instance(s); limit {}",
            verified_states.len(),
            loaded_count,
            config.backend.max_instances
        ),
    ));
    let loopback = !verified_states.is_empty()
        && verified_states.iter().all(|state| {
            state.control_url.starts_with("http://127.0.0.1:")
                && state.opencode_url.starts_with("http://127.0.0.1:")
        });
    for (id, summary, pass_detail) in [
        (
            "backend.server.loopback",
            "backend listeners",
            "both control and OpenCode listeners are IPv4 loopback-only",
        ),
        (
            "backend.server.auth",
            "backend authentication",
            "private state passed authenticated health with bounded identities",
        ),
        (
            "backend.server.health",
            "backend health",
            "supervisor and OpenCode process identities are live",
        ),
    ] {
        checks.push(Check::new(
            id,
            if !verified_states.is_empty() && (id != "backend.server.loopback" || loopback) {
                Status::Pass
            } else {
                Status::Skipped
            },
            Severity::Critical,
            summary,
            if !verified_states.is_empty() {
                pass_detail
            } else {
                "not running; use `aishe doctor --live` for an active smoke test"
            },
        ));
    }

    let smoke = if options.live && runtime_ready {
        Some(crate::backend::supervisor::smoke_test(&manager))
    } else {
        None
    };
    let smoke_status = match &smoke {
        Some(Ok(())) => Status::Pass,
        Some(Err(_)) => Status::Fail,
        None => Status::Skipped,
    };
    let smoke_detail = match &smoke {
        Some(Ok(())) => {
            "pinned server started with isolated HOME/XDG, authenticated health, and trusted proxy-tool registration"
                .into()
        }
        Some(Err(error)) => crate::redact::redact(&error.to_string()),
        None => "run `aishe doctor --live` to start the isolated smoke server".into(),
    };
    for (id, summary) in [
        ("backend.config.isolated", "backend config isolation"),
        ("backend.plugin.hash", "trusted plugin hash"),
        ("backend.tools.restricted", "model tool restriction"),
        ("backend.tool_bridge", "AIShe tool bridge"),
    ] {
        checks.push(Check::new(
            id,
            smoke_status,
            Severity::Critical,
            summary,
            smoke_detail.clone(),
        ));
    }
    checks.push(Check::new(
        "backend.events",
        Status::Skipped,
        Severity::Warning,
        "backend event stream",
        "full SSE ordering/cancellation is exercised by adapter contract tests and a real turn",
    ));
    checks.push(Check::new(
        "backend.provider",
        if config_valid {
            Status::Pass
        } else {
            Status::Skipped
        },
        Severity::Warning,
        format!("backend provider: {}", config.aishe.provider),
        active_provider(config).base_url.clone(),
    ));
    checks.push(Check::new(
        "backend.model",
        if config_valid {
            Status::Pass
        } else {
            Status::Skipped
        },
        Severity::Warning,
        format!("backend model: {}", config.active_model()),
        "exact model availability is checked by provider validation",
    ));
    checks.push(Check::new(
        "backend.credential_isolation",
        Status::Pass,
        Severity::Critical,
        "credential isolation",
        "credentials enter the supervisor through a bounded private pipe; tool subprocess environments drop provider secrets",
    ));

    let backend_root = crate::backend::supervisor::backend_root().ok();
    let sessions = backend_root
        .as_ref()
        .map(|root| root.join("sessions").join("mappings.json"));
    checks.push(Check::new(
        "sessions.storage",
        Status::Pass,
        Severity::Warning,
        "managed session storage",
        sessions
            .map(|path| {
                if path.exists() {
                    format!("mapping index present at {}", path.display())
                } else {
                    format!("created on first managed turn at {}", path.display())
                }
            })
            .unwrap_or_else(|| "data directory unavailable".into()),
    ));
    checks.push(Check::new(
        "sessions.migration",
        Status::Pass,
        Severity::Info,
        "legacy session retention",
        "legacy task/session files remain readable and are never deleted by runtime operations",
    ));
    let usage_journal = backend_root.map(|root| root.join("journal.json"));
    checks.push(Check::new(
        "usage.mapping",
        Status::Pass,
        Severity::Warning,
        "managed usage mapping",
        usage_journal
            .map(|path| {
                if path.exists() {
                    format!("durable de-duplication journal at {}", path.display())
                } else {
                    format!(
                        "journal will be created on first managed turn at {}",
                        path.display()
                    )
                }
            })
            .unwrap_or_else(|| "data directory unavailable".into()),
    ));
}

fn append_sandbox_checks(checks: &mut Vec<Check>, config: &Config) {
    let state = crate::dependencies::bubblewrap_probe();
    let present = !matches!(&state, crate::dependencies::BubblewrapState::Missing);
    let usable = matches!(&state, crate::dependencies::BubblewrapState::Usable { .. });
    checks.push(Check::new(
        "sandbox.bubblewrap.present",
        if present {
            Status::Pass
        } else if cfg!(target_os = "linux") {
            Status::Warn
        } else {
            Status::Skipped
        },
        Severity::Warning,
        "bubblewrap binary",
        format!("{state:?}"),
    ));
    checks.push(Check::new(
        "sandbox.bubblewrap.functional",
        if usable {
            Status::Pass
        } else if config.sandbox.require_functional {
            Status::Fail
        } else {
            Status::Warn
        },
        if config.sandbox.require_functional {
            Severity::Critical
        } else {
            Severity::Warning
        },
        "bubblewrap functional isolation",
        if usable {
            "real self-test proved writable workspace, read-only host root, private /tmp, and network namespace"
        } else {
            "run `aishe setup` for a consent-gated package install and functional retry"
        },
    ));
    checks.push(Check::new(
        "sandbox.workspace.escape",
        if usable {
            Status::Pass
        } else {
            Status::Skipped
        },
        Severity::Critical,
        "workspace escape protection",
        if usable {
            "functional probe refused a write through the read-only host root"
        } else {
            "no kernel sandbox was available to exercise; policy-only checks are not an OS boundary"
        },
    ));
}

fn capability_checks(report: &capabilities::Report) -> Vec<Check> {
    [
        ("provider.live.credential", &report.credential),
        ("provider.live.reachability", &report.reachability),
        ("provider.live.model_list", &report.model_list),
        ("provider.live.model", &report.model_available),
        ("provider.live.text", &report.text),
        ("provider.live.structured", &report.structured),
        ("provider.live.tools", &report.tools),
        ("provider.live.streaming", &report.streaming),
    ]
    .into_iter()
    .map(|(id, check)| {
        let status = match check.state {
            capabilities::State::Pass => Status::Pass,
            capabilities::State::Warn => Status::Warn,
            capabilities::State::Fail => Status::Warn,
            capabilities::State::Skipped => Status::Skipped,
        };
        Check::new(
            id,
            status,
            Severity::Warning,
            id.trim_start_matches("provider.live.").replace('_', " "),
            check.detail.clone(),
        )
    })
    .collect()
}

fn active_provider(config: &Config) -> &config::ProviderConfig {
    config.active_provider_config()
}

pub fn resolved_paths() -> Paths {
    let config_path = Config::path();
    let config_dir = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let data_dir = config::data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aishe");
    Paths {
        config: config_path,
        credentials: crate::credentials::path(),
        config_dir,
        history: data_dir.join("history.ext"),
        capability_dir: data_dir.join("capabilities"),
        data_dir,
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct StateSize {
    bytes: u64,
    entries: usize,
    present: bool,
    complete: bool,
}

fn append_retention_checks(checks: &mut Vec<Check>, paths: &Paths, config: &Config) {
    let audit_path = std::env::var_os("AISHE_LOG_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| config.logging.file.as_ref().map(PathBuf::from))
        .unwrap_or_else(crate::audit::default_path);
    let undo_path = std::env::var_os("AISHE_UNDO_JOURNAL")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.data_dir.join("undo.jsonl"));
    let open_code_credentials = paths.data_dir.join("backend").join("opencode").join("xdg");
    let runtime_path = std::env::var_os("AISHE_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.data_dir.join("runtime"));

    let audit_rotated = with_suffix(&audit_path, ".1");
    for specification in [
        RetentionSpec {
            id: "state.size.history",
            label: "shell history",
            paths: vec![paths.history.clone(), paths.data_dir.join("history.vec")],
            warn_bytes: HISTORY_WARN_BYTES,
            per_file_warn_bytes: None,
            cleanup: "aishe uninstall --history --dry-run",
            policy: "retained until explicit deletion; no automatic rotation",
        },
        RetentionSpec {
            id: "state.size.audit",
            label: "audit log",
            paths: vec![audit_path, audit_rotated],
            warn_bytes: crate::audit::AUDIT_ROTATE_BYTES * 2,
            per_file_warn_bytes: Some(crate::audit::AUDIT_ROTATE_BYTES),
            cleanup: "aishe uninstall --audit-undo --dry-run",
            policy: "active file rotates at 256 MiB; one .1 generation is retained",
        },
        RetentionSpec {
            id: "state.size.undo",
            label: "undo journal",
            paths: vec![with_suffix(&undo_path, ".1"), undo_path],
            warn_bytes: crate::undo::UNDO_ROTATE_BYTES * 2,
            per_file_warn_bytes: Some(crate::undo::UNDO_ROTATE_BYTES),
            cleanup: "aishe uninstall --audit-undo --dry-run",
            policy: "active file rotates at 128 MiB; one .1 generation is retained",
        },
        RetentionSpec {
            id: "state.size.sessions",
            label: "sessions, tasks, and usage journals",
            paths: vec![
                paths.data_dir.join("tasks"),
                paths.data_dir.join("backend").join("sessions"),
                paths.data_dir.join("backend").join("journal"),
                paths.data_dir.join("backend").join("journal.json"),
            ],
            warn_bytes: SESSION_WARN_BYTES,
            per_file_warn_bytes: None,
            cleanup: "aishe uninstall --sessions --dry-run",
            policy: "retained until explicit deletion; OAuth credentials are excluded",
        },
        RetentionSpec {
            id: "state.size.config",
            label: "configuration and credentials",
            paths: vec![paths.config_dir.clone(), open_code_credentials],
            warn_bytes: CONFIG_WARN_BYTES,
            per_file_warn_bytes: None,
            cleanup: "aishe uninstall --config --dry-run",
            policy: "retained until explicit credential/config deletion",
        },
        RetentionSpec {
            id: "state.size.capabilities",
            label: "capability cache",
            paths: vec![paths.capability_dir.clone()],
            warn_bytes: CAPABILITY_WARN_BYTES,
            per_file_warn_bytes: None,
            cleanup: "aishe doctor --fix",
            policy: "records expire after seven days; Doctor removes stale entries",
        },
        RetentionSpec {
            id: "state.size.runtime",
            label: "managed runtime/cache",
            paths: vec![runtime_path],
            warn_bytes: RUNTIME_WARN_BYTES,
            per_file_warn_bytes: None,
            cleanup: "aishe uninstall --runtime --dry-run",
            policy: "managed artifacts persist across upgrades until explicit cleanup",
        },
    ] {
        checks.push(retention_check(&specification));
    }
}

struct RetentionSpec<'a> {
    id: &'a str,
    label: &'a str,
    paths: Vec<PathBuf>,
    warn_bytes: u64,
    per_file_warn_bytes: Option<u64>,
    cleanup: &'a str,
    policy: &'a str,
}

fn retention_check(specification: &RetentionSpec<'_>) -> Check {
    let size = state_size(&specification.paths, STATE_SCAN_ENTRY_LIMIT);
    let oversized_file = specification.per_file_warn_bytes.is_some_and(|limit| {
        specification.paths.iter().any(|path| {
            std::fs::symlink_metadata(path)
                .is_ok_and(|metadata| metadata.is_file() && metadata.len() >= limit)
        })
    });
    let status = if !size.present {
        Status::Skipped
    } else if !size.complete || oversized_file || size.bytes >= specification.warn_bytes {
        Status::Warn
    } else {
        Status::Pass
    };
    let scan = if size.complete {
        format!(
            "{} across {} filesystem entries",
            human_bytes(size.bytes),
            size.entries
        )
    } else {
        format!(
            "at least {} across {} filesystem entries; bounded scan stopped at {} entries",
            human_bytes(size.bytes),
            size.entries,
            STATE_SCAN_ENTRY_LIMIT
        )
    };
    Check::new(
        specification.id,
        status,
        Severity::Warning,
        format!("{}: {scan}", specification.label),
        format!(
            "{}; warning threshold {}; preview cleanup with `{}`",
            specification.policy,
            human_bytes(specification.warn_bytes),
            specification.cleanup
        ),
    )
}

fn state_size(paths: &[PathBuf], entry_limit: usize) -> StateSize {
    let mut result = StateSize {
        complete: true,
        ..StateSize::default()
    };
    let mut pending = paths.to_vec();
    while let Some(path) = pending.pop() {
        if result.entries >= entry_limit {
            result.complete = false;
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                result.present = true;
                result.complete = false;
                continue;
            }
        };
        result.present = true;
        result.entries += 1;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_file() {
            result.bytes = result.bytes.saturating_add(metadata.len());
            continue;
        }
        if metadata.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    pending.extend(
                        entries
                            .filter_map(|entry| entry.ok())
                            .map(|entry| entry.path()),
                    );
                }
                Err(_) => result.complete = false,
            }
        }
    }
    result
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: &[(&str, u64)] = &[
        ("GiB", 1024 * 1024 * 1024),
        ("MiB", 1024 * 1024),
        ("KiB", 1024),
    ];
    for (unit, divisor) in UNITS {
        if bytes >= *divisor {
            return format!("{:.1} {unit}", bytes as f64 / *divisor as f64);
        }
    }
    format!("{bytes} B")
}

fn apply_safe_fixes(paths: &Paths, config_valid: bool) -> Check {
    let mut changed = Vec::new();
    let mut notes = Vec::new();
    for dir in [&paths.config_dir, &paths.data_dir] {
        if !dir.exists() && std::fs::create_dir_all(dir).is_ok() {
            changed.push(dir.clone());
        }
    }
    if config_valid && !paths.history.exists() {
        if let Some(parent) = paths.history.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.history)
            .is_ok()
        {
            changed.push(paths.history.clone());
        }
    }
    for root in [&paths.config_dir, &paths.data_dir] {
        repair_private_tree(root, &mut changed);
    }
    // AISHE_CREDENTIALS_FILE may live outside the normal config directory.
    repair_private_tree(&paths.credentials, &mut changed);
    let removed = capabilities::clear_stale().unwrap_or(0);
    if let Ok(manager) = crate::backend::RuntimeManager::new() {
        match manager.status() {
            crate::backend::RuntimeStatus::Invalid { .. } => {
                match manager.install(crate::backend::InstallSource::Default, true) {
                    Ok(_) => {
                        changed.push(manager.version_dir());
                        notes.push("reinstalled corrupt managed runtime".to_string());
                    }
                    Err(error) => notes.push(format!(
                        "runtime repair failed: {}",
                        crate::redact::redact(&error.to_string())
                    )),
                }
            }
            crate::backend::RuntimeStatus::Ready { .. } => {
                match crate::backend::supervisor::prepare_layout() {
                    Ok(prepared) => {
                        if prepared.changed_paths.is_empty() {
                            notes.push("verified managed plugin and isolated layout".to_string());
                        } else {
                            changed.extend(prepared.changed_paths);
                            notes.push("repaired managed plugin and isolated layout".to_string());
                        }
                    }
                    Err(error) => notes.push(format!(
                        "plugin repair failed: {}",
                        crate::redact::redact(&error.to_string())
                    )),
                }
            }
            crate::backend::RuntimeStatus::Missing { .. } => {
                notes.push("runtime missing; run `aishe backend install`".to_string());
            }
        }
    }
    if let Ok(Some(state)) = crate::backend::control::load_state() {
        if !crate::backend::control::state_processes_exist(&state)
            && crate::backend::control::remove_state_if_nonce(&state.startup_nonce).is_ok()
        {
            notes.push("removed verified-stale supervisor state".to_string());
        }
    }
    if let Ok(keys) = crate::backend::supervisor::instance_keys() {
        for key in keys {
            if let Ok(Some(state)) = crate::backend::control::load_state_for(&key) {
                if !crate::backend::control::state_processes_exist(&state)
                    && crate::backend::control::remove_state_for_if_nonce(
                        &key,
                        &state.startup_nonce,
                    )
                    .is_ok()
                {
                    notes.push(format!(
                        "removed stale supervisor state for {}",
                        if state.connection_id.is_empty() {
                            "legacy connection"
                        } else {
                            &state.connection_id
                        }
                    ));
                }
            }
        }
    }
    changed.sort();
    changed.dedup();
    let mut detail = if changed.is_empty() && removed == 0 {
        "no changes were necessary".into()
    } else {
        format!(
            "{} path(s) created/repaired; {removed} stale capability record(s) removed",
            changed.len()
        )
    };
    if !notes.is_empty() {
        detail.push_str("; ");
        detail.push_str(&notes.join("; "));
    }
    Check {
        id: "repair.safe".into(),
        status: Status::Pass,
        severity: Severity::Info,
        summary: "safe repairs".into(),
        detail,
        fixable: true,
        changed_paths: changed,
    }
}

fn repair_private_tree(path: &Path, changed: &mut Vec<PathBuf>) {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    let desired = if metadata.is_dir() { 0o700 } else { 0o600 };
    if set_private_mode(path, desired) {
        changed.push(path.to_path_buf());
    }
    if metadata.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            repair_private_tree(&entry.path(), changed);
        }
    }
}

#[cfg(unix)]
fn set_private_mode(path: &Path, desired: u32) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.permissions().mode() & 0o777 == desired {
        return false;
    }
    let mut permissions = metadata.permissions();
    permissions.set_mode(desired);
    std::fs::set_permissions(path, permissions).is_ok()
}

#[cfg(not(unix))]
fn set_private_mode(_path: &Path, _desired: u32) -> bool {
    false
}

pub fn render_text(report: &Report) -> String {
    let mut output = String::from("aishe doctor\n────────────\n");
    for check in &report.checks {
        let icon = match check.status {
            Status::Pass => "✓",
            Status::Warn => "!",
            Status::Fail => "✗",
            Status::Skipped => "·",
        };
        output.push_str(&format!(
            "{icon} {}: {}\n",
            crate::commands::display_safe(&check.id),
            crate::commands::display_safe(&check.summary)
        ));
        if !check.detail.is_empty() {
            output.push_str(&format!(
                "    {}\n",
                crate::commands::display_safe(&check.detail)
            ));
        }
        for path in &check.changed_paths {
            output.push_str(&format!(
                "    changed: {}\n",
                crate::commands::display_safe(&path.display().to_string())
            ));
        }
    }
    output.push('\n');
    output.push_str(if report.critical_ok() {
        "all critical checks passed\n"
    } else {
        "some critical checks failed\n"
    });
    output
}

#[derive(Serialize)]
struct SupportBundle<'a> {
    schema_version: u32,
    report: &'a Report,
    config: Value,
    exclusions: Vec<String>,
}

pub fn write_bundle(path: &Path, report: &Report, config: Option<&Config>) -> Result<()> {
    let config = config
        .and_then(|config| serde_json::to_value(config).ok())
        .map(redact_config)
        .unwrap_or(Value::Null);
    let bundle = SupportBundle {
        schema_version: 1,
        report,
        config,
        exclusions: {
            let mut values = [
                "credential values",
                "environment values",
                "prompts",
                "command history",
                "file contents",
                "audit contents",
            ]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
            if let Ok(Some(loaded)) = crate::policy::load() {
                values.extend(loaded.policy.support_bundle_exclusions);
            }
            values.sort();
            values.dedup();
            values
        },
    };
    let bytes = serde_json::to_vec_pretty(&bundle)?;
    config::write_atomic(path, &bytes)
        .with_context(|| format!("writing support bundle {}", path.display()))?;
    let _ = set_private_mode(path, 0o600);
    Ok(())
}

fn redact_config(mut value: Value) -> Value {
    redact_value(&mut value, None);
    value
}

fn redact_value(value: &mut Value, parent: Option<&str>) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                let lower = key.to_ascii_lowercase();
                let secret_field = lower != "api_key_env"
                    && (lower.contains("password")
                        || lower.contains("secret")
                        || lower == "token"
                        || lower == "authorization"
                        || lower == "api_key");
                let secret_container = matches!(lower.as_str(), "env" | "headers")
                    || matches!(parent, Some("env" | "headers"));
                if secret_field || secret_container {
                    *child = Value::String("<redacted>".into());
                } else {
                    redact_value(child, Some(&lower));
                }
            }
        }
        Value::Array(values) => {
            for child in values {
                redact_value(child, parent);
            }
        }
        Value::String(text) => *text = crate::redact::redact(text),
        _ => {}
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacted_config_keeps_key_name_but_not_secret_values() {
        let mut config = Config::default();
        config.providers.openai.api_key_env = "OPENAI_API_KEY".into();
        config.mcp_servers.insert(
            "secret-test".into(),
            config::McpServerConfig {
                command: Some("tool".into()),
                args: Vec::new(),
                env: [("TOKEN".into(), "sk-proj-super-secret".into())]
                    .into_iter()
                    .collect(),
                url: None,
                headers: [("Authorization".into(), "Bearer abc123".into())]
                    .into_iter()
                    .collect(),
                enabled: true,
            },
        );
        let value = redact_config(serde_json::to_value(config).unwrap());
        let text = serde_json::to_string(&value).unwrap();
        assert!(text.contains("OPENAI_API_KEY"));
        assert!(!text.contains("sk-proj-super-secret"));
        assert!(!text.contains("Bearer abc123"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn text_uses_the_same_check_ids_as_json() {
        let report = Report {
            schema_version: 1,
            generated_at_ms: 0,
            version: "test".into(),
            platform: "test".into(),
            paths: resolved_paths(),
            checks: vec![Check::new(
                "example.check",
                Status::Pass,
                Severity::Info,
                "example",
                "detail",
            )],
            capability_report: None,
        };
        let text = render_text(&report);
        let json = serde_json::to_string(&report).unwrap();
        assert!(text.contains("example.check"));
        assert!(json.contains("example.check"));
    }

    #[test]
    fn compatibility_tiers_name_surfaces_and_bash_caveats() {
        let cases = [
            ("zsh", true, true, None, "Tier A"),
            ("zsh", false, true, None, "Tier A-"),
            ("bash", false, true, Some("3.2.57"), "Tier B-"),
            ("bash", false, true, Some("5.3.9"), "Tier B"),
            ("fish", false, false, None, "Tier C"),
        ];
        for (shell, pty, hook, version, expected) in cases {
            let (tier, surface, caveat) = compatibility_tier(shell, pty, hook, version);
            assert_eq!(tier, expected);
            assert!(!surface.is_empty());
            assert!(!caveat.is_empty());
        }
        let (_, _, bash3) = compatibility_tier("bash", false, true, Some("3.2.57"));
        assert!(bash3.contains("Ctrl-G"));
    }

    #[test]
    fn keybinding_conflicts_are_reported_without_dropping_an_action() {
        let distinct = [
            ("force-agent", "^[^M".into()),
            ("mode-cycle", "^[[Z".into()),
            ("fix-last", "^X^F".into()),
            ("semantic-recall", "^X^R".into()),
        ];
        assert!(keybinding_collisions(&distinct).is_empty());

        let colliding = [
            ("force-agent", "^G".into()),
            ("mode-cycle", "^[[Z".into()),
            ("fix-last", "^G".into()),
            ("semantic-recall", "^X^R".into()),
        ];
        let collisions = keybinding_collisions(&colliding);
        assert_eq!(collisions.len(), 1);
        assert!(collisions[0].contains("force-agent and fix-last"));
        assert!(collisions[0].contains("^G"));
    }

    #[test]
    fn retention_check_warns_at_the_documented_bound() {
        let path = std::env::temp_dir().join(format!(
            "aishe-retention-bound-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::remove_file(&path).ok();
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(4096).unwrap();
        let check = retention_check(&RetentionSpec {
            id: "state.size.test",
            label: "test state",
            paths: vec![path.clone()],
            warn_bytes: 4096,
            per_file_warn_bytes: None,
            cleanup: "aishe cleanup --dry-run",
            policy: "test policy",
        });
        assert_eq!(check.status, Status::Warn);
        assert!(check.summary.contains("4.0 KiB"));
        assert!(check.detail.contains("aishe cleanup --dry-run"));
        std::fs::remove_file(path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn state_size_is_bounded_and_never_follows_symlinks() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "aishe-state-size-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let outside = root.with_extension("outside");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_file(&outside).ok();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("small"), [0_u8; 17]).unwrap();
        let outside_file = std::fs::File::create(&outside).unwrap();
        outside_file.set_len(1024 * 1024).unwrap();
        symlink(&outside, root.join("outside-link")).unwrap();

        let complete = state_size(std::slice::from_ref(&root), 100);
        assert!(complete.complete);
        assert_eq!(complete.bytes, 17);
        assert_eq!(complete.entries, 3);

        let bounded = state_size(std::slice::from_ref(&root), 1);
        assert!(!bounded.complete);
        assert_eq!(bounded.entries, 1);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_file(&outside).ok();
    }

    #[cfg(unix)]
    #[test]
    fn private_tree_repair_covers_nested_state_without_following_symlinks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root =
            std::env::temp_dir().join(format!("aishe-private-repair-{}", std::process::id()));
        let nested = root.join("nested");
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("state.json"), "{}").unwrap();
        std::fs::write(&outside, "outside").unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(
            nested.join("state.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
        symlink(&outside, nested.join("outside-link")).unwrap();

        let mut changed = Vec::new();
        repair_private_tree(&root, &mut changed);
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(nested.join("state.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
            0o644
        );

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_file(&outside).ok();
    }
}
