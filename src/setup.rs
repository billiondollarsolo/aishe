//! Resumable setup state machine and terminal driver.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::capabilities::{self, Report, State};
use crate::config::Config;
use crate::profiles::{self, Profile};
use crate::promptui::{self, MenuResult};
use crate::provider_catalog::{self, Family};
use crate::usage::{self, Price};

const DRAFT_SCHEMA_VERSION: u32 = 3;

pub const EXIT_OK: u8 = 0;
pub const EXIT_PAUSED: u8 = 2;
pub const EXIT_INPUT: u8 = 3;
pub const EXIT_RUNTIME: u8 = 4;
pub const EXIT_PROVIDER: u8 = 5;
pub const EXIT_SANDBOX: u8 = 6;
pub const EXIT_POLICY: u8 = 7;

#[derive(Debug)]
struct ClassifiedError {
    code: u8,
    message: String,
}

impl std::fmt::Display for ClassifiedError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ClassifiedError {}

pub fn exit_code(error: &anyhow::Error) -> u8 {
    error
        .downcast_ref::<ClassifiedError>()
        .map(|error| error.code)
        .unwrap_or(EXIT_INPUT)
}

fn classified(code: u8, error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::Error::new(ClassifiedError {
        code,
        message: crate::redact::redact(&error.to_string()),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Discovery,
    Platform,
    Runtime,
    Sandbox,
    Service,
    Endpoint,
    Credential,
    Model,
    Profile,
    Pricing,
    Status,
    Validation,
    Review,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptResult<T> {
    Value(T),
    Back,
    Cancel,
}

impl Step {
    fn next(self) -> Self {
        match self {
            Self::Discovery => Self::Platform,
            Self::Platform => Self::Runtime,
            Self::Runtime => Self::Sandbox,
            Self::Sandbox => Self::Service,
            Self::Service => Self::Endpoint,
            Self::Endpoint => Self::Credential,
            Self::Credential => Self::Model,
            Self::Model => Self::Profile,
            Self::Profile => Self::Pricing,
            Self::Pricing => Self::Status,
            Self::Status => Self::Validation,
            Self::Validation => Self::Review,
            Self::Review => Self::Review,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Discovery => Self::Discovery,
            Self::Platform => Self::Discovery,
            Self::Runtime => Self::Platform,
            Self::Sandbox => Self::Runtime,
            Self::Service => Self::Sandbox,
            Self::Endpoint => Self::Service,
            Self::Credential => Self::Endpoint,
            Self::Model => Self::Credential,
            Self::Profile => Self::Model,
            Self::Pricing => Self::Profile,
            Self::Status => Self::Pricing,
            Self::Validation => Self::Status,
            Self::Review => Self::Validation,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Draft {
    schema_version: u32,
    step: Step,
    service: String,
    /// When true, setup preselects subscription OAuth after an explicit
    /// ChatGPT/Codex or Grok OAuth service choice.
    #[serde(default)]
    prefer_oauth: bool,
    config: Config,
}

/// Setup-only service picker entries. Catalog services stay shared with
/// Settings; explicit OAuth rows are discoverability wrappers around the
/// official OpenAI and xAI endpoints.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ServiceMenuEntry {
    ChatGptCodexOAuth,
    GrokOAuth,
    Catalog(usize),
}

fn service_menu_entries() -> Vec<ServiceMenuEntry> {
    let mut entries = vec![
        ServiceMenuEntry::ChatGptCodexOAuth,
        ServiceMenuEntry::GrokOAuth,
    ];
    entries.extend((0..provider_catalog::SERVICES.len()).map(ServiceMenuEntry::Catalog));
    entries
}

fn service_menu_label(entry: ServiceMenuEntry) -> String {
    match entry {
        ServiceMenuEntry::ChatGptCodexOAuth => {
            "ChatGPT / Codex OAuth — Sign in with ChatGPT Plus/Pro (no API key)".into()
        }
        ServiceMenuEntry::GrokOAuth => {
            "Grok OAuth — Sign in with SuperGrok subscription (no API key)".into()
        }
        ServiceMenuEntry::Catalog(index) => {
            let service = &provider_catalog::SERVICES[index];
            format!("{} — {}", service.label, service.help)
        }
    }
}

fn service_menu_default(draft: &Draft) -> usize {
    let entries = service_menu_entries();
    if draft.prefer_oauth {
        let oauth_entry = match draft.service.as_str() {
            "openai" => Some(ServiceMenuEntry::ChatGptCodexOAuth),
            "xai" => Some(ServiceMenuEntry::GrokOAuth),
            _ => None,
        };
        if let Some(wanted) = oauth_entry {
            if let Some(index) = entries.iter().position(|entry| *entry == wanted) {
                return index;
            }
        }
    }
    entries
        .iter()
        .position(|entry| match entry {
            ServiceMenuEntry::Catalog(index) => {
                provider_catalog::SERVICES[*index].key == draft.service
            }
            ServiceMenuEntry::ChatGptCodexOAuth | ServiceMenuEntry::GrokOAuth => false,
        })
        .unwrap_or(0)
}

fn oauth_subscription_choice(provider: crate::oauth::OAuthProvider) -> &'static str {
    match provider {
        crate::oauth::OAuthProvider::Openai => {
            "Sign in with ChatGPT / Codex OAuth (Plus/Pro subscription)"
        }
        crate::oauth::OAuthProvider::Xai => "Sign in with Grok OAuth (SuperGrok subscription)",
    }
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub resume: bool,
    pub restart: bool,
    pub verify_only: bool,
    pub non_interactive: bool,
    pub service: Option<String>,
    pub base_url: Option<String>,
    pub key_env: Option<String>,
    pub credential_profile: Option<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub profile: Option<Profile>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub live: bool,
    pub backend: Option<String>,
    pub install_backend: bool,
    pub runtime_file: Option<PathBuf>,
    pub runtime_base_url: Option<String>,
    pub sandbox: Option<String>,
    pub install_system_deps: bool,
    pub default_scope: Option<String>,
    pub network: Option<String>,
    pub output: Option<String>,
    pub json: bool,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub exit_code: u8,
    pub applied: bool,
    pub config_path: PathBuf,
    pub backup: Option<PathBuf>,
    pub report: Option<Report>,
}

pub fn run(options: Options) -> Result<Outcome> {
    if options.verify_only {
        let mut config = Config::load_quiet()?
            .context("no config exists; run `aishe setup` first")
            .map_err(|error| classified(EXIT_INPUT, error))?;
        crate::policy::constrain(&mut config).map_err(|error| classified(EXIT_POLICY, error))?;
        verify_runtime_and_sandbox(&config, false, None, None, false)?;
        let report = capabilities::validate(&config, options.live);
        let verified = report.credential.state != State::Fail
            && report.model_available.state != State::Fail
            && (!options.live || report.verified());
        if options.json {
            print_setup_json(
                false,
                &config,
                None,
                &report,
                if verified { EXIT_OK } else { EXIT_PROVIDER },
            )?;
        } else {
            print_report(&report);
        }
        return Ok(Outcome {
            exit_code: if verified { EXIT_OK } else { EXIT_PROVIDER },
            applied: false,
            config_path: Config::path(),
            backup: None,
            report: Some(report),
        });
    }
    if options.non_interactive {
        return run_non_interactive(options);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("setup needs an interactive terminal; use `aishe setup --non-interactive`");
    }
    run_interactive(options)
}

fn run_non_interactive(options: Options) -> Result<Outcome> {
    validate_noninteractive_options(&options).map_err(|error| classified(EXIT_INPUT, error))?;
    let existing = Config::load_quiet().map_err(|error| classified(EXIT_INPUT, error))?;
    let mut config = existing.clone().unwrap_or_default();
    if let Some(service_key) = options.service.as_deref() {
        let service = provider_catalog::find(service_key)
            .with_context(|| format!("unknown service '{service_key}'"))
            .map_err(|error| classified(EXIT_INPUT, error))?;
        apply_service(&mut config, service);
    } else if existing.is_none() {
        return Err(classified(
            EXIT_INPUT,
            "fresh non-interactive setup requires --service",
        ));
    }
    apply_overrides(&mut config, &options).map_err(|error| classified(EXIT_INPUT, error))?;
    validate_config(&config).map_err(|error| classified(EXIT_INPUT, error))?;
    let loaded_policy = crate::policy::load().map_err(|error| classified(EXIT_POLICY, error))?;
    if let Some(loaded) = &loaded_policy {
        loaded
            .policy
            .validate_request(&config)
            .map_err(|error| classified(EXIT_POLICY, error))?;
    }
    let runtime = verify_runtime_and_sandbox(
        &config,
        options.install_backend,
        options.runtime_file.as_deref(),
        options.runtime_base_url.as_deref(),
        options.install_system_deps,
    )?;
    let report = capabilities::validate(&config, options.live);
    if report.credential.state == State::Fail
        || report.model_available.state == State::Fail
        || (options.live && !report.live_verified())
    {
        if options.json {
            print_setup_json(false, &config, runtime.as_ref(), &report, EXIT_PROVIDER)?;
        } else {
            print_report(&report);
        }
        return Err(classified(
            EXIT_PROVIDER,
            "provider credential, model, or requested live capability validation failed",
        ));
    }
    let backup =
        save_transactional(&config, None).map_err(|error| classified(EXIT_RUNTIME, error))?;
    discard_draft().ok();
    if options.json {
        print_setup_json(true, &config, runtime.as_ref(), &report, EXIT_OK)?;
    } else {
        print_report(&report);
        println!("Saved config to {}", Config::path().display());
    }
    Ok(Outcome {
        exit_code: EXIT_OK,
        applied: true,
        config_path: Config::path(),
        backup,
        report: Some(report),
    })
}

fn validate_noninteractive_options(options: &Options) -> Result<()> {
    if options
        .backend
        .as_deref()
        .is_some_and(|value| value != "opencode")
    {
        anyhow::bail!("--backend must be opencode");
    }
    if options.runtime_file.is_some() && options.runtime_base_url.is_some() {
        anyhow::bail!("--runtime-file and --runtime-base-url are mutually exclusive");
    }
    if (options.runtime_file.is_some() || options.runtime_base_url.is_some())
        && !options.install_backend
    {
        anyhow::bail!("--runtime-file/--runtime-base-url require --install-backend");
    }
    if let Some(url) = &options.runtime_base_url {
        validate_runtime_base_url(url)?;
    }
    if options.install_system_deps && options.sandbox.as_deref() != Some("bwrap") {
        anyhow::bail!("--install-system-deps requires --sandbox bwrap");
    }
    Ok(())
}

fn verify_runtime_and_sandbox(
    config: &Config,
    install_backend: bool,
    runtime_file: Option<&Path>,
    runtime_base_url: Option<&str>,
    install_system_deps: bool,
) -> Result<Option<crate::backend::RuntimeStatus>> {
    let loaded_policy = crate::policy::load().map_err(|error| classified(EXIT_POLICY, error))?;
    if let (Some(requested), Some(managed)) = (
        runtime_base_url,
        loaded_policy
            .as_ref()
            .and_then(|loaded| loaded.policy.runtime_base_url()),
    ) {
        if requested.trim_end_matches('/') != managed.trim_end_matches('/') {
            return Err(classified(
                EXIT_POLICY,
                "runtime mirror differs from the organization-managed mirror",
            ));
        }
    }

    if cfg!(target_os = "linux") && config.sandbox.linux_backend == "bwrap" {
        let mut state = crate::dependencies::bubblewrap_probe();
        if !matches!(state, crate::dependencies::BubblewrapState::Usable { .. })
            && install_system_deps
        {
            let plan = crate::dependencies::bubblewrap_install_plan()
                .map_err(|error| classified(EXIT_SANDBOX, error))?;
            state = crate::dependencies::install_bubblewrap(&plan, true)
                .map_err(|error| classified(EXIT_SANDBOX, error))?;
        }
        if !matches!(state, crate::dependencies::BubblewrapState::Usable { .. }) {
            return Err(classified(
                EXIT_SANDBOX,
                format!(
                    "functional bubblewrap is required but unavailable ({state:?}); \
                     pass --install-system-deps to authorize installation or explicitly \
                     choose --sandbox policy when organization policy permits"
                ),
            ));
        }
    } else if config.sandbox.require_functional {
        return Err(classified(
            EXIT_SANDBOX,
            "functional bubblewrap is required but this platform/sandbox selection cannot provide it",
        ));
    }

    if config.backend.engine != "opencode" {
        return Ok(None);
    }
    let manager =
        crate::backend::RuntimeManager::new().map_err(|error| classified(EXIT_RUNTIME, error))?;
    let asset = manager
        .manifest()
        .asset_for_current_platform()
        .map_err(|error| classified(EXIT_RUNTIME, error))?;
    if let Some(loaded) = &loaded_policy {
        loaded
            .policy
            .validate_runtime_hash(&asset.sha256)
            .map_err(|error| classified(EXIT_POLICY, error))?;
    }
    let status = if install_backend {
        let source = if let Some(path) = runtime_file {
            crate::backend::InstallSource::Local(path.to_path_buf())
        } else {
            let base = loaded_policy
                .as_ref()
                .and_then(|loaded| loaded.policy.runtime_base_url())
                .or(runtime_base_url);
            if let Some(base) = base {
                runtime_source_from_base(&manager, base)
                    .map_err(|error| classified(EXIT_RUNTIME, error))?
            } else {
                crate::backend::InstallSource::Default
            }
        };
        manager
            .install(source, false)
            .map_err(|error| classified(EXIT_RUNTIME, error))?
    } else {
        manager
            .verify()
            .map_err(|error| classified(EXIT_RUNTIME, error))?
    };
    crate::backend::supervisor::smoke_test(&manager)
        .map_err(|error| classified(EXIT_RUNTIME, error))?;
    Ok(Some(status))
}

fn print_setup_json(
    applied: bool,
    config: &Config,
    runtime: Option<&crate::backend::RuntimeStatus>,
    report: &Report,
    exit_code: u8,
) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "schema_version": 1,
            "applied": applied,
            "exit_code": exit_code,
            "config_path": Config::path(),
            "credentials_path": crate::credentials::path(),
            "backend": config.backend.engine,
            "runtime": runtime,
            "sandbox": {
                "backend": config.sandbox.linux_backend,
                "functional": matches!(
                    crate::dependencies::bubblewrap_probe(),
                    crate::dependencies::BubblewrapState::Usable { .. }
                ),
            },
            "scope": config.backend.default_scope,
            "network": config.backend.workspace_network,
            "provider": report,
        }))?
    );
    Ok(())
}

fn run_interactive(options: Options) -> Result<Outcome> {
    promptui::brand();
    promptui::header(
        "aishe setup",
        "Configure, verify, and safely apply your AIShe environment.",
        "Active config is not changed until the final Apply step.",
    );

    if options.restart {
        discard_draft()?;
    }
    let baseline = Config::load_quiet()?.unwrap_or_default();
    let mut draft = if options.resume {
        load_draft()?.context("no resumable setup draft exists")?
    } else if let Some(saved) = load_draft()? {
        let choices = vec![
            "Resume saved setup".to_string(),
            "Start over (discard draft only)".to_string(),
            "Exit without changes".to_string(),
        ];
        match promptui::menu(
            "An unfinished setup was found",
            &choices,
            0,
            false,
            "The draft contains configuration choices but never credentials.",
        )? {
            MenuResult::Selected(0) => saved,
            MenuResult::Selected(1) => {
                discard_draft()?;
                fresh_draft(baseline.clone())
            }
            _ => {
                return Ok(Outcome {
                    exit_code: EXIT_PAUSED,
                    applied: false,
                    config_path: Config::path(),
                    backup: None,
                    report: None,
                })
            }
        }
    } else {
        fresh_draft(baseline.clone())
    };

    // Secret material is intentionally process-local. A resumed draft that had
    // advanced past Credential must return there when no saved/environment
    // source now exists.
    let mut pending_credential: Option<(String, String)> = None;
    if matches!(
        draft.step,
        Step::Model
            | Step::Profile
            | Step::Pricing
            | Step::Status
            | Step::Validation
            | Step::Review
    ) && active_provider(&draft.config).requires_auth()
        && crate::credentials::resolve(active_provider(&draft.config))
            .map(|resolved| resolved.secret().is_none())
            .unwrap_or(true)
        && crate::oauth::active_provider(&draft.config)
            .map(|provider| provider.is_none())
            .unwrap_or(true)
    {
        println!("  The resumed draft contains no credential; returning to the Credential step.");
        draft.step = Step::Credential;
        save_draft(&draft)?;
    }

    let mut report = None;
    loop {
        match draft.step {
            Step::Discovery => {
                step_header(1, "Welcome and existing state");
                print_existing_state(&baseline)?;
                let choices = vec![
                    "Continue setup".to_string(),
                    "Pause and resume later".to_string(),
                ];
                match promptui::menu(
                    "AIShe is ready to verify this environment",
                    &choices,
                    0,
                    true,
                    "Setup preserves config, credentials, history, tasks, and sessions. Only the final Apply changes active configuration.",
                )? {
                    MenuResult::Selected(0) => advance(&mut draft)?,
                    MenuResult::Selected(1) | MenuResult::Cancel => return cancel(draft),
                    MenuResult::Back => {}
                    MenuResult::Selected(_) => unreachable!(),
                }
            }
            Step::Platform => {
                step_header(2, "Shell and platform");
                print_platform_state();
                if crate::executor::which("zsh").is_some() {
                    promptui::success("zsh is installed and the PTY front-end is available");
                    advance(&mut draft)?;
                    continue;
                }
                promptui::warning(
                    "zsh is missing. AIShe can still provide non-interactive commands and a bash hook, but `aishe` cannot launch its native interactive front-end.",
                );
                let mut choices = vec!["Continue in shell-only mode".to_string()];
                if cfg!(target_os = "linux") {
                    if let Ok(plan) = crate::dependencies::zsh_install_plan() {
                        choices.insert(0, format!("Install zsh now — {}", plan.display));
                    }
                }
                match promptui::menu(
                    "Interactive shell dependency",
                    &choices,
                    0,
                    true,
                    "System package installation is an immediate side effect. AIShe shows and executes an argv plan without `sh -c`.",
                )? {
                    MenuResult::Selected(0) if choices.len() == 2 => {
                        let plan = crate::dependencies::zsh_install_plan()?;
                        let Some(consent) =
                            promptui::confirm(&format!("Run `{}` now", plan.display), false)?
                        else {
                            return cancel(draft);
                        };
                        if consent {
                            crate::dependencies::install_zsh(&plan, true)?;
                            promptui::success("zsh installed and verified");
                            advance(&mut draft)?;
                        }
                    }
                    MenuResult::Selected(_) => advance(&mut draft)?,
                    MenuResult::Back => draft.step = draft.step.previous(),
                    MenuResult::Cancel => return cancel(draft),
                }
            }
            Step::Runtime => {
                step_header(3, "Agent runtime");
                let manager = crate::backend::RuntimeManager::new()?;
                print_runtime_state(&manager)?;
                let loaded_policy = crate::policy::load()?;
                let approved_base = loaded_policy
                    .as_ref()
                    .and_then(|loaded| loaded.policy.runtime_base_url())
                    .map(str::to_string)
                    .or_else(|| options.runtime_base_url.clone());
                match manager.status() {
                    crate::backend::RuntimeStatus::Ready { .. } => {
                        if let Some(loaded) = &loaded_policy {
                            let asset = manager.manifest().asset_for_current_platform()?;
                            loaded.policy.validate_runtime_hash(&asset.sha256)?;
                        }
                        crate::backend::supervisor::smoke_test(&manager)?;
                        promptui::success(
                            "runtime hash, version, authenticated server, and trusted plugin verified",
                        );
                        advance(&mut draft)?;
                    }
                    crate::backend::RuntimeStatus::Missing { .. }
                    | crate::backend::RuntimeStatus::Invalid { .. } => {
                        let choices = vec![
                            "Install and verify the pinned runtime".into(),
                            "Use an approved local archive…".into(),
                            "Use a runtime mirror…".into(),
                            "Continue shell-only and resume setup later".into(),
                        ];
                        match promptui::menu(
                            "OpenCode Agent Runtime",
                            &choices,
                            0,
                            true,
                            "Runtime installation writes only AIShe's private versioned runtime directory. It never changes config, credentials, or history.",
                        )? {
                            MenuResult::Selected(index @ 0..=2) => {
                                let source = match index {
                                    0 => {
                                        if let Some(path) = &options.runtime_file {
                                            crate::backend::InstallSource::Local(path.clone())
                                        } else if let Some(base) = approved_base {
                                            runtime_source_from_base(&manager, &base)?
                                        } else {
                                            crate::backend::InstallSource::Default
                                        }
                                    }
                                    1 => {
                                        let Some(value) = promptui::text(
                                            "Local runtime archive",
                                            "",
                                            |value| {
                                                let path = Path::new(value);
                                                if !path.is_file() {
                                                    anyhow::bail!("archive file does not exist");
                                                }
                                                Ok(())
                                            },
                                        )?
                                        else {
                                            return cancel(draft);
                                        };
                                        if value == ":back" {
                                            continue;
                                        }
                                        crate::backend::InstallSource::Local(PathBuf::from(value))
                                    }
                                    2 => {
                                        let default = approved_base.as_deref().unwrap_or("");
                                        let Some(value) = promptui::text(
                                            "Runtime mirror base URL",
                                            default,
                                            validate_runtime_base_url,
                                        )?
                                        else {
                                            return cancel(draft);
                                        };
                                        if value == ":back" {
                                            continue;
                                        }
                                        runtime_source_from_base(&manager, &value)?
                                    }
                                    _ => unreachable!(),
                                };
                                let status = manager.install(source, true)?;
                                if let crate::backend::RuntimeStatus::Ready { version, .. } = status
                                {
                                    if let Some(loaded) = &loaded_policy {
                                        let asset =
                                            manager.manifest().asset_for_current_platform()?;
                                        loaded.policy.validate_runtime_hash(&asset.sha256)?;
                                    }
                                    crate::backend::supervisor::smoke_test(&manager)?;
                                    promptui::success(&format!(
                                        "OpenCode {version} installed, checksum-verified, and started successfully"
                                    ));
                                    advance(&mut draft)?;
                                }
                            }
                            MenuResult::Selected(3) => {
                                promptui::warning(
                                    "Agent turns remain unavailable until `aishe backend install` succeeds; normal zsh commands continue to work.",
                                );
                                advance(&mut draft)?;
                            }
                            MenuResult::Back => draft.step = draft.step.previous(),
                            MenuResult::Cancel => return cancel(draft),
                            MenuResult::Selected(_) => unreachable!(),
                        }
                    }
                }
            }
            Step::Sandbox => {
                step_header(4, "Execution sandbox");
                if !cfg!(target_os = "linux") {
                    promptui::warning(
                        "OS sandbox: unavailable in this macOS release. Workspace paths are policy-checked, and every yolo shell session shows an explicit no-OS-sandbox warning before acceptance.",
                    );
                    draft.config.sandbox.linux_backend = "policy".into();
                    advance(&mut draft)?;
                    continue;
                }
                let policy_requires = crate::policy::load()?
                    .as_ref()
                    .and_then(|loaded| loaded.policy.require_bubblewrap)
                    == Some(true);
                match crate::dependencies::bubblewrap_probe() {
                    crate::dependencies::BubblewrapState::Usable { path } => {
                        promptui::success(&format!(
                            "bubblewrap passed writable-workspace, read-only-root, and network-isolation tests ({})",
                            path.display()
                        ));
                        draft.config.sandbox.linux_backend = "bwrap".into();
                        draft.config.sandbox.require_functional = policy_requires;
                        advance(&mut draft)?;
                    }
                    state => {
                        let detail = match state {
                            crate::dependencies::BubblewrapState::Missing => {
                                "bubblewrap is not installed".to_string()
                            }
                            crate::dependencies::BubblewrapState::InstalledButUnusable {
                                reason,
                            } => format!("bubblewrap is installed but unusable: {reason}"),
                            crate::dependencies::BubblewrapState::Unsupported => {
                                "bubblewrap is unsupported in this environment".to_string()
                            }
                            crate::dependencies::BubblewrapState::Usable { .. } => unreachable!(),
                        };
                        promptui::warning(&detail);
                        let plan = crate::dependencies::bubblewrap_install_plan().ok();
                        if let Some(plan) = &plan {
                            println!("  exact install command: {}", plan.display);
                        }
                        let mut choices = Vec::new();
                        if plan.is_some() {
                            choices.push("Install bubblewrap now".into());
                        }
                        if !policy_requires {
                            choices.push("Continue with policy-only degradation".into());
                        }
                        choices.push("Pause and install it manually".into());
                        match promptui::menu(
                            "Linux workspace isolation",
                            &choices,
                            0,
                            true,
                            "bubblewrap confines workspace agent commands to a read-only host, a writable project, private /tmp, and the selected network policy.",
                        )? {
                            MenuResult::Selected(index)
                                if plan.is_some() && index == 0 =>
                            {
                                let plan = plan.as_ref().unwrap();
                                let Some(consent) = promptui::confirm(
                                    &format!("Run `{}` now", plan.display),
                                    false,
                                )?
                                else {
                                    return cancel(draft);
                                };
                                if consent {
                                    crate::dependencies::install_bubblewrap(plan, true)?;
                                    draft.config.sandbox.linux_backend = "bwrap".into();
                                    draft.config.sandbox.require_functional = policy_requires;
                                    promptui::success(
                                        "bubblewrap installed and passed its functional self-test",
                                    );
                                    advance(&mut draft)?;
                                }
                            }
                            MenuResult::Selected(index)
                                if !policy_requires
                                    && index == usize::from(plan.is_some()) =>
                            {
                                draft.config.sandbox.linux_backend = "policy".into();
                                draft.config.sandbox.require_functional = false;
                                promptui::warning(
                                    "Policy-only mode checks paths but is not a kernel isolation boundary.",
                                );
                                advance(&mut draft)?;
                            }
                            MenuResult::Selected(_) | MenuResult::Cancel => {
                                return cancel(draft)
                            }
                            MenuResult::Back => draft.step = draft.step.previous(),
                        }
                    }
                }
            }
            Step::Service => {
                step_header(5, "Provider and credential");
                let entries = service_menu_entries();
                let labels: Vec<String> = entries.iter().copied().map(service_menu_label).collect();
                let default = service_menu_default(&draft);
                match promptui::menu(
                    "Provider service",
                    &labels,
                    default,
                    false,
                    "ChatGPT/Codex and Grok OAuth use a subscription login. Other rows use API keys, local models, or custom endpoints.",
                )? {
                    MenuResult::Selected(index) => {
                        pending_credential = None;
                        match entries[index] {
                            ServiceMenuEntry::ChatGptCodexOAuth => {
                                let service = provider_catalog::find("openai")
                                    .expect("openai catalog service");
                                apply_service(&mut draft.config, service);
                                draft.service = service.key.to_string();
                                draft.prefer_oauth = true;
                                // Official OAuth is endpoint-bound; skip the
                                // editable URL step and go straight to login.
                                draft.step = Step::Credential;
                                save_draft(&draft)?;
                            }
                            ServiceMenuEntry::GrokOAuth => {
                                let service = provider_catalog::find("xai")
                                    .expect("xai catalog service");
                                apply_service(&mut draft.config, service);
                                draft.service = service.key.to_string();
                                draft.prefer_oauth = true;
                                draft.step = Step::Credential;
                                save_draft(&draft)?;
                            }
                            ServiceMenuEntry::Catalog(service_index) => {
                                let service = &provider_catalog::SERVICES[service_index];
                                apply_service(&mut draft.config, service);
                                draft.service = service.key.to_string();
                                draft.prefer_oauth = false;
                                advance(&mut draft)?;
                            }
                        }
                    }
                    _ => return cancel(draft),
                }
            }
            Step::Endpoint => {
                let provider = active_provider_mut(&mut draft.config);
                match promptui::text("API endpoint", &provider.base_url, validate_url)? {
                    Some(value) if value == ":back" => {
                        draft.prefer_oauth = false;
                        draft.step = draft.step.previous();
                    }
                    Some(value) => {
                        provider.base_url = provider_catalog::normalize_base_url(&value);
                        if provider.auth_required.is_none() {
                            provider.auth_required =
                                Some(!crate::config::is_loopback_url(&provider.base_url));
                        }
                        // Editing the endpoint leaves the generic credential path.
                        draft.prefer_oauth = false;
                        advance(&mut draft)?;
                    }
                    None => return cancel(draft),
                }
            }
            Step::Credential => {
                let provider = active_provider(&draft.config).clone();
                if !provider.requires_auth() {
                    promptui::success(&format!(
                        "Credential not required for local endpoint {}",
                        provider.base_url
                    ));
                    pending_credential = None;
                    set_active_auth(&mut draft.config, crate::config::ConnectionAuth::None);
                    advance(&mut draft)?;
                    continue;
                }
                let profile = provider.credential_profile();
                let existing = crate::credentials::resolve(&provider);
                if existing
                    .as_ref()
                    .is_ok_and(|resolved| resolved.secret().is_none())
                {
                    if let Some(oauth_provider) = crate::oauth::active_provider(&draft.config)? {
                        promptui::success(&format!(
                            "Using existing {oauth_provider} OAuth credential from AIShe's private runtime store"
                        ));
                        pending_credential = None;
                        advance(&mut draft)?;
                        continue;
                    }
                }
                if let Err(error) = &existing {
                    println!(
                        "  ! Credential store unavailable: {}",
                        crate::commands::display_safe(&crate::redact::redact(&error.to_string()))
                    );
                    println!(
                        "    Repair it with `aishe doctor --fix`, then retry saved-key entry."
                    );
                }
                let mut choices = Vec::new();
                let existing_index = if let Ok(resolved) = &existing {
                    if resolved.secret().is_some() {
                        choices.push(format!(
                            "Use existing credential ({})",
                            resolved.source.label()
                        ));
                        Some(0)
                    } else {
                        None
                    }
                } else {
                    None
                };
                let oauth_provider = crate::oauth::OAuthProvider::from_base_url(&provider.base_url);
                let oauth_index = oauth_provider.map(|_| choices.len());
                if let Some(oauth_provider) = oauth_provider {
                    choices.push(oauth_subscription_choice(oauth_provider).into());
                }
                let save_key_index = choices.len();
                choices.push("Enter and save an API key locally (recommended)".into());
                let env_index = choices.len();
                choices.push(format!(
                    "Use environment variable only (${env})",
                    env = provider.api_key_env
                ));
                let default_credential = if draft.prefer_oauth {
                    oauth_index.or(existing_index).unwrap_or(0)
                } else {
                    existing_index.unwrap_or(0)
                };
                match promptui::menu(
                    &format!("Credential profile '{profile}'"),
                    &choices,
                    default_credential,
                    true,
                    "Saved keys live in a private credentials file; subscription OAuth uses AIShe's private runtime store; environment variables remain available for automation and overrides.",
                )? {
                    MenuResult::Selected(index) if Some(index) == existing_index => {
                        pending_credential = None;
                        draft.prefer_oauth = false;
                        set_active_auth(
                            &mut draft.config,
                            crate::config::ConnectionAuth::ApiKey {
                                credential: Some(profile.clone()),
                                api_key_env: Some(provider.api_key_env.clone()),
                            },
                        );
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(index) if Some(index) == oauth_index => {
                        let oauth_provider = oauth_provider.expect("index only exists with provider");
                        let Some(label) = promptui::text("OAuth profile label", "work", |value| {
                            crate::oauth::normalize_profile(value)?;
                            Ok(())
                        })? else {
                            return cancel(draft);
                        };
                        if label == ":back" {
                            continue;
                        }
                        let code = crate::oauth::login_profile(oauth_provider, &label, false, false)?;
                        if code != 0 {
                            promptui::error("OAuth login did not complete; choose a credential method again");
                            continue;
                        }
                        set_active_auth(
                            &mut draft.config,
                            crate::config::ConnectionAuth::OAuth { profile: label },
                        );
                        draft.prefer_oauth = true;
                        pending_credential = None;
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(index) if index == save_key_index => {
                        let Some(secret) = promptui::secret(
                            &format!("API key for '{profile}'"),
                            crate::credentials::MAX_SECRET_BYTES,
                        )?
                        else {
                            return cancel(draft);
                        };
                        crate::credentials::validate_secret(&secret)?;
                        draft.prefer_oauth = false;
                        set_active_auth(
                            &mut draft.config,
                            crate::config::ConnectionAuth::ApiKey {
                                credential: Some(profile.clone()),
                                api_key_env: Some(provider.api_key_env.clone()),
                            },
                        );
                        pending_credential = Some((profile, secret));
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(index) if index == env_index => {
                        let provider = active_provider_mut(&mut draft.config);
                        match promptui::text(
                            "Environment override variable",
                            &provider.api_key_env,
                            validate_env_name,
                        )? {
                            Some(value) if value == ":back" => {}
                            Some(value) => {
                                provider.api_key_env = value;
                                let credential = provider.credential_profile();
                                let key_env = provider.api_key_env.clone();
                                draft.prefer_oauth = false;
                                set_active_auth(
                                    &mut draft.config,
                                    crate::config::ConnectionAuth::ApiKey {
                                        credential: Some(credential),
                                        api_key_env: Some(key_env),
                                    },
                                );
                                pending_credential = None;
                                advance(&mut draft)?;
                            }
                            None => return cancel(draft),
                        }
                    }
                    MenuResult::Back => {
                        // Explicit OAuth service rows skip Endpoint; send the
                        // user back to the provider list instead of a URL they
                        // never saw.
                        draft.step = if draft.prefer_oauth {
                            Step::Service
                        } else {
                            draft.step.previous()
                        };
                    }
                    MenuResult::Cancel => return cancel(draft),
                    MenuResult::Selected(_) => unreachable!(),
                }
            }
            Step::Model => {
                step_header(6, "Model and pricing");
                let provider_name = draft.config.active_connection_id().to_string();
                let current = active_provider(&draft.config).model.clone();
                let using_oauth = pending_credential.is_none()
                    && crate::credentials::resolve(active_provider(&draft.config))
                        .is_ok_and(|resolved| resolved.secret().is_none())
                    && crate::oauth::active_provider(&draft.config)?.is_some();
                if using_oauth {
                    promptui::success(
                        "OAuth is ready; model availability is validated through the managed runtime",
                    );
                    match promptui::text("Model ID", &current, |value| {
                        if value.trim().is_empty() {
                            anyhow::bail!("model cannot be empty");
                        }
                        if value.len() > 512 || value.chars().any(char::is_control) {
                            anyhow::bail!("model must be 1–512 printable characters");
                        }
                        Ok(())
                    })? {
                        Some(value) if value == ":back" => draft.step = draft.step.previous(),
                        Some(value) => {
                            active_provider_mut(&mut draft.config).model = value;
                            advance(&mut draft)?;
                        }
                        None => return cancel(draft),
                    }
                    continue;
                }
                let catalog = with_pending(&pending_credential, || {
                    capabilities::list_models(&draft.config, &provider_name)
                })?;
                if let Ok(models) = catalog {
                    let auth_required = active_provider(&draft.config).requires_auth();
                    promptui::success(&format!(
                        "{}; /v1/models returned {} model(s)",
                        if auth_required {
                            "Credential accepted"
                        } else {
                            "Endpoint reached"
                        },
                        models.len()
                    ));
                    const VISIBLE_MODEL_LIMIT: usize = 24;
                    let mut visible = Vec::new();
                    if models.iter().any(|model| model == &current) {
                        visible.push(current.clone());
                    }
                    visible.extend(
                        models
                            .iter()
                            .filter(|model| *model != &current)
                            .take(VISIBLE_MODEL_LIMIT.saturating_sub(visible.len()))
                            .cloned(),
                    );
                    let mut options = vec!["Type a model ID…".to_string()];
                    options.extend(visible.iter().cloned());
                    let default = visible
                        .iter()
                        .position(|model| model == &current)
                        .map(|position| position + 1)
                        .unwrap_or(0);
                    if models.len() > visible.len() {
                        promptui::warning(&format!(
                            "Showing {} of {} models; type any returned model ID to select it",
                            visible.len(),
                            models.len()
                        ));
                    }
                    match promptui::menu(
                        "Available models (refreshed from /v1/models)",
                        &options,
                        default,
                        true,
                        "Listed models are validated by exact catalog membership. A typed ID is checked against the full catalog, then with one minimal request only if it was not listed.",
                    )? {
                        MenuResult::Selected(0) => {
                            match prompt_manual_model(
                                &mut draft,
                                Some(&models),
                                &pending_credential,
                            )? {
                                PromptResult::Value(()) => advance(&mut draft)?,
                                PromptResult::Back => {}
                                PromptResult::Cancel => return cancel(draft),
                            }
                        }
                        MenuResult::Selected(index) => {
                            let model = visible[index - 1].clone();
                            active_provider_mut(&mut draft.config).model = model.clone();
                            promptui::success(&format!(
                                "Model '{model}' is present in the current endpoint catalog"
                            ));
                            advance(&mut draft)?;
                        }
                        MenuResult::Back => draft.step = draft.step.previous(),
                        MenuResult::Cancel => return cancel(draft),
                    }
                } else {
                    let error = catalog.unwrap_err();
                    let detail = crate::redact::redact(&error.to_string());
                    promptui::error(&format!(
                        "Could not load /v1/models ({:?}): {detail}",
                        error.kind()
                    ));
                    let credential_rejected = matches!(
                        error.kind(),
                        crate::providers::ErrorKind::MissingCredential
                            | crate::providers::ErrorKind::InvalidCredential
                    );
                    let choices = if credential_rejected {
                        vec![
                            "Back to credential".into(),
                            "Retry /v1/models".into(),
                            "Cancel setup".into(),
                        ]
                    } else {
                        vec![
                            "Retry /v1/models".into(),
                            "Type a model ID and validate with one minimal request".into(),
                            "Back to credential or endpoint".into(),
                            "Cancel setup".into(),
                        ]
                    };
                    match promptui::menu(
                        "Model discovery needs attention",
                        &choices,
                        0,
                        false,
                        "A successful catalog request validates the endpoint and credential without spending tokens. Manual fallback makes one small generation request.",
                    )? {
                        MenuResult::Selected(0) if credential_rejected => {
                            draft.step = draft.step.previous()
                        }
                        MenuResult::Selected(1) if credential_rejected => {}
                        MenuResult::Selected(0) => {}
                        MenuResult::Selected(1) => {
                            match prompt_manual_model(&mut draft, None, &pending_credential)? {
                                PromptResult::Value(()) => advance(&mut draft)?,
                                PromptResult::Back => {}
                                PromptResult::Cancel => return cancel(draft),
                            }
                        }
                        MenuResult::Selected(2) if !credential_rejected => {
                            draft.step = draft.step.previous()
                        }
                        MenuResult::Selected(_) | MenuResult::Cancel => return cancel(draft),
                        MenuResult::Back => unreachable!(),
                    }
                }
            }
            Step::Profile => {
                step_header(7, "Behavior and scope");
                let choices = vec![
                    "Conservative — suggest, confirm all tool commands".into(),
                    "Balanced — auto safe commands, confirm writes".into(),
                    "Autonomous — yolo with readiness checks".into(),
                    "Custom — preserve individual controls".into(),
                ];
                match promptui::menu(
                    "Safety profile",
                    &choices,
                    profile_index(&draft.config.aishe.safety_profile),
                    true,
                    "Profiles are transparent setting bundles; Settings shows every value.",
                )? {
                    MenuResult::Selected(index) => {
                        let profile = [
                            Profile::Conservative,
                            Profile::Balanced,
                            Profile::Autonomous,
                            Profile::Custom,
                        ][index];
                        let changes = profiles::apply(&mut draft.config, profile);
                        println!("  {} safety setting(s) changed", changes.len());
                        if configure_scope_and_network(&mut draft)? {
                            advance(&mut draft)?;
                        }
                    }
                    MenuResult::Back => draft.step = draft.step.previous(),
                    MenuResult::Cancel => return cancel(draft),
                }
            }
            Step::Pricing => {
                let model = draft.config.active_model().to_string();
                let using_oauth = crate::credentials::resolve(active_provider(&draft.config))
                    .is_ok_and(|resolved| resolved.secret().is_none())
                    && crate::oauth::active_provider(&draft.config)?.is_some();
                if using_oauth {
                    println!(
                        "\n  Pricing: provider subscription OAuth; token usage is tracked but API cost is not estimated"
                    );
                    advance(&mut draft)?;
                    continue;
                }
                if usage::price_for(&model, &draft.config.pricing).is_some() {
                    println!(
                        "\n  Pricing: configured for {}",
                        crate::commands::display_safe(&model)
                    );
                    advance(&mut draft)?;
                    continue;
                }
                let choices = vec![
                    "Enter input/output rates (USD per 1M tokens)".into(),
                    "Leave price unknown".into(),
                    "Return to model selection".into(),
                ];
                match promptui::menu(
                    &format!("No price is known for {model}"),
                    &choices,
                    0,
                    true,
                    "AIShe will never invent a rate; unknown pricing disables cost budgets.",
                )? {
                    MenuResult::Selected(0) => {
                        let input = match prompt_rate("Input price")? {
                            PromptResult::Value(value) => value,
                            PromptResult::Back => continue,
                            PromptResult::Cancel => return cancel(draft),
                        };
                        let output = match prompt_rate("Output price")? {
                            PromptResult::Value(value) => value,
                            PromptResult::Back => continue,
                            PromptResult::Cancel => return cancel(draft),
                        };
                        draft.config.pricing.insert(model, Price { input, output });
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(1) => advance(&mut draft)?,
                    MenuResult::Selected(2) | MenuResult::Back => draft.step = Step::Model,
                    MenuResult::Cancel => return cancel(draft),
                    MenuResult::Selected(_) => unreachable!(),
                }
            }
            Step::Status => {
                step_header(8, "Interface");
                let output_choices = vec![
                    "Focus — final responses; live activity stays off scrollback".into(),
                    "Compact — persistent one-line tool activity".into(),
                    "Detailed — expanded tool metadata and timing".into(),
                ];
                let output_default = match draft.config.backend.output.as_str() {
                    "compact" => 1,
                    "detailed" => 2,
                    _ => 0,
                };
                match promptui::menu(
                    "Agent transcript density",
                    &output_choices,
                    output_default,
                    true,
                    "Focus is the clean default. Press Ctrl-O in AIShe to toggle full tool details for the current shell; raw chain-of-thought is never shown.",
                )? {
                    MenuResult::Selected(0) => draft.config.backend.output = "focus".into(),
                    MenuResult::Selected(1) => draft.config.backend.output = "compact".into(),
                    MenuResult::Selected(2) => draft.config.backend.output = "detailed".into(),
                    MenuResult::Back => {
                        draft.step = draft.step.previous();
                        continue;
                    }
                    MenuResult::Cancel => return cancel(draft),
                    MenuResult::Selected(_) => unreachable!(),
                }
                let positions = vec![
                    "Right prompt — compact and persistent".into(),
                    "Below — Codex-style secondary prompt line".into(),
                    "Off — keep only per-call/exit summaries".into(),
                ];
                let position_default = match draft.config.aishe.status_line_position.as_str() {
                    "below" => 1,
                    "off" => 2,
                    _ => 0,
                };
                match promptui::menu(
                    "Live status-line placement",
                    &positions,
                    position_default,
                    true,
                    "Right is best for wide terminals; Below has room for more metrics.",
                )? {
                    MenuResult::Selected(2) => {
                        draft.config.aishe.status_line = false;
                        draft.config.aishe.status_line_position = "off".into();
                        println!("  preview: (status line off)");
                    }
                    MenuResult::Selected(position) => {
                        draft.config.aishe.status_line = true;
                        draft.config.aishe.status_line_position =
                            if position == 1 { "below" } else { "right" }.into();
                        if !choose_status_items(&mut draft)? {
                            continue;
                        }
                        print_status_preview(&draft.config);
                    }
                    MenuResult::Back => {
                        draft.step = draft.step.previous();
                        continue;
                    }
                    MenuResult::Cancel => return cancel(draft),
                }
                let audit_required = crate::policy::load()?
                    .as_ref()
                    .and_then(|loaded| loaded.policy.require_audit_logging)
                    == Some(true);
                if audit_required {
                    draft.config.logging.enabled = true;
                    draft.config.logging.redact = true;
                    promptui::warning("Audit logging: on · Managed by organization");
                } else {
                    let Some(audit) = promptui::confirm(
                        "Enable the private redacted audit log? (records prompts, answers, and actions)",
                        draft.config.logging.enabled,
                    )?
                    else {
                        return cancel(draft);
                    };
                    draft.config.logging.enabled = audit;
                }
                advance(&mut draft)?;
            }
            Step::Validation => {
                step_header(9, "End-to-end validation");
                if let Some(loaded) = crate::policy::load()? {
                    loaded.policy.constrain(&mut draft.config)?;
                    if let Err(error) = loaded.policy.validate_request(&draft.config) {
                        promptui::error(&format!("Managed by organization: {error}"));
                        draft.step = Step::Service;
                        save_draft(&draft)?;
                        continue;
                    }
                }
                let backend_check = with_pending(&pending_credential, || {
                    validate_managed_backend(&draft.config)
                })?;
                if let Err(error) = backend_check {
                    promptui::error(&format!(
                        "Managed backend validation failed: {}",
                        crate::redact::redact(&error.to_string())
                    ));
                    let choices = vec![
                        "Retry managed backend validation".into(),
                        "Back to agent runtime".into(),
                        "Pause setup".into(),
                    ];
                    match promptui::menu(
                        "Backend needs attention",
                        &choices,
                        0,
                        true,
                        "No active config or credential was changed. Runtime files already installed remain checksum-verifiable and reusable.",
                    )? {
                        MenuResult::Selected(0) => {}
                        MenuResult::Selected(1) | MenuResult::Back => {
                            draft.step = Step::Runtime
                        }
                        MenuResult::Selected(2) | MenuResult::Cancel => return cancel(draft),
                        MenuResult::Selected(_) => unreachable!(),
                    }
                    continue;
                }
                promptui::success(
                    "runtime, authenticated loopback server, isolated config, and trusted plugin passed",
                );
                let Some(live) = promptui::confirm(
                    "Run live text/structured/tool/streaming checks? (may use tokens)",
                    true,
                )?
                else {
                    return cancel(draft);
                };
                let checked = with_pending(&pending_credential, || {
                    capabilities::validate(&draft.config, live)
                })?;
                print_report(&checked);
                report = Some(checked);
                advance(&mut draft)?;
            }
            Step::Review => {
                step_header(10, "Review and Apply");
                print_review(
                    &baseline,
                    &draft.config,
                    report.as_ref(),
                    pending_credential.as_ref(),
                )?;
                let Some(apply) = promptui::confirm("Apply this configuration", true)? else {
                    return cancel(draft);
                };
                if !apply {
                    draft.step = Step::Service;
                    save_draft(&draft)?;
                    continue;
                }
                validate_config(&draft.config)?;
                if let Some(loaded) = crate::policy::load()? {
                    loaded.policy.constrain(&mut draft.config)?;
                    loaded.policy.validate_request(&draft.config)?;
                }
                let backup =
                    save_transactional(&draft.config, pending_credential.take()).map_err(|error| {
                        classified(
                            EXIT_RUNTIME,
                            format!(
                                "final backend health failed; prior config and credentials were restored: {error}"
                            ),
                        )
                    })?;
                discard_draft()?;
                promptui::success("Setup complete");
                println!();
                println!(
                    "  ✓ zsh             {}",
                    crate::executor::which("zsh")
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "shell-only mode".into())
                );
                println!(
                    "  ✓ agent engine    OpenCode {}",
                    crate::backend::RuntimeManifest::embedded()?.version
                );
                println!(
                    "  ✓ provider        {} · {}",
                    draft.config.aishe.provider,
                    draft.config.active_model()
                );
                println!(
                    "  ✓ sandbox         {} · {}",
                    draft.config.sandbox.linux_backend, draft.config.backend.default_scope
                );
                println!("  ✓ history         preserved");
                println!();
                println!("  config: {}", Config::path().display());
                println!("  credentials: {}", crate::credentials::path().display());
                if let Some(path) = &backup {
                    println!("  backup: {}", path.display());
                }
                if let Some(report) = &report {
                    println!(
                        "  provider: {}",
                        if report.verified() {
                            "verified"
                        } else {
                            "saved with warnings; run `aishe setup --verify --live`"
                        }
                    );
                }
                if promptui::confirm("Run the guided first-session tour now", true)?
                    .unwrap_or(false)
                {
                    crate::tour::run(crate::tour::Options::default())?;
                } else {
                    println!();
                    println!("  Run: aishe");
                    println!("  Inside AIShe:");
                    println!("    git status                 runs in zsh");
                    println!("    explain this repository    asks the agent");
                    println!();
                    println!("  Run `aishe tour` when you are ready.");
                }
                return Ok(Outcome {
                    exit_code: EXIT_OK,
                    applied: true,
                    config_path: Config::path(),
                    backup,
                    report,
                });
            }
        }
    }
}

fn step_header(number: usize, title: &str) {
    promptui::section(&format!("Step {number} of 10 · {title}"));
}

fn print_existing_state(config: &Config) -> Result<()> {
    let config_path = Config::path();
    let schema = Config::schema_version_on_disk()?;
    let credential_profiles = crate::credentials::Store::load()?
        .map(|store| store.profile_names())
        .unwrap_or_default();
    let runtime = crate::backend::RuntimeManager::new()?.status();
    let runtime_text = match runtime {
        crate::backend::RuntimeStatus::Ready { version, .. } => {
            format!("OpenCode {version} · verified on disk")
        }
        crate::backend::RuntimeStatus::Missing { expected_version } => {
            format!("OpenCode {expected_version} · not installed")
        }
        crate::backend::RuntimeStatus::Invalid {
            expected_version,
            reason,
        } => format!("OpenCode {expected_version} · invalid ({reason})"),
    };
    let policy = crate::policy::load()?;
    let history = crate::config::data_root()
        .map(|root| root.join("aishe").join("history.ext"))
        .filter(|path| path.exists());
    let sessions = crate::backend::opencode::session::SessionStore::from_default_root()
        .and_then(|store| store.records(None))
        .map(|records| records.len())
        .unwrap_or(0);
    println!(
        "  install: {}",
        if config_path.exists() {
            "upgrade / reconfiguration"
        } else {
            "fresh"
        }
    );
    println!(
        "  config: {}{}",
        config_path.display(),
        schema
            .map(|value| format!(" · schema {value}"))
            .unwrap_or_else(|| " · not created".into())
    );
    println!(
        "  credentials: {} profile(s){}",
        credential_profiles.len(),
        if credential_profiles.is_empty() {
            String::new()
        } else {
            format!(" ({})", credential_profiles.join(", "))
        }
    );
    println!(
        "  retained state: history {} · {} task(s) · {sessions} managed session(s)",
        if history.is_some() {
            "present"
        } else {
            "not created"
        },
        crate::tasks::list().len()
    );
    println!("  runtime: {runtime_text}");
    println!(
        "  organization policy: {}",
        policy
            .as_ref()
            .map(|loaded| loaded.path.display().to_string())
            .unwrap_or_else(|| "none".into())
    );
    println!(
        "  active preference: {} · {} · {}",
        config.aishe.provider,
        config.active_model(),
        config.aishe.mode
    );
    Ok(())
}

fn print_platform_state() {
    println!(
        "  platform: {}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Some(zsh) = crate::executor::which("zsh") {
        let version = std::process::Command::new(&zsh)
            .arg("--version")
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .unwrap_or_else(|| "version unavailable".into());
        println!("  zsh: {} · {}", zsh.display(), version);
    } else {
        println!("  zsh: not installed");
    }
    println!(
        "  terminal: PTY {} · color {} · width {}",
        if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
            "available"
        } else {
            "unavailable"
        },
        if std::env::var_os("NO_COLOR").is_some() {
            "disabled by NO_COLOR"
        } else {
            "enabled when supported"
        },
        crossterm::terminal::size()
            .map(|(columns, _)| columns)
            .unwrap_or(80)
    );
    println!("  config: {}", Config::path().display());
    println!(
        "  data: {}",
        crate::config::data_root()
            .map(|path| path.join("aishe").display().to_string())
            .unwrap_or_else(|| "unavailable".into())
    );
    let proxy = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY"]
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .copied()
        .collect::<Vec<_>>();
    let ca = ["SSL_CERT_FILE", "SSL_CERT_DIR"]
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .copied()
        .collect::<Vec<_>>();
    println!(
        "  network environment: proxy {} · custom CA {}",
        if proxy.is_empty() {
            "none".into()
        } else {
            proxy.join(",")
        },
        if ca.is_empty() {
            "none".into()
        } else {
            ca.join(",")
        }
    );
}

fn print_runtime_state(manager: &crate::backend::RuntimeManager) -> Result<()> {
    let manifest = manager.manifest();
    let asset = manifest.asset_for_current_platform()?;
    let source = manifest.source_url(
        asset,
        crate::policy::load()?
            .as_ref()
            .and_then(|loaded| loaded.policy.runtime_base_url()),
    );
    let hostname = url::Url::parse(&source)
        .ok()
        .and_then(|url| url.host_str().map(str::to_string))
        .unwrap_or_else(|| "approved local source".into());
    println!("  engine: OpenCode Agent Runtime {}", manifest.version);
    println!(
        "  asset: {} · {:.1} MiB",
        asset.name,
        asset.size as f64 / 1_048_576.0
    );
    println!("  source: {hostname}");
    println!("  install path: {}", manager.version_dir().display());
    println!("  verification: exact size + SHA-256 {}", asset.sha256);
    println!("  license: MIT · bundled LICENSE and THIRD_PARTY_NOTICES.md");
    Ok(())
}

fn runtime_source_from_base(
    manager: &crate::backend::RuntimeManager,
    base: &str,
) -> Result<crate::backend::InstallSource> {
    validate_runtime_base_url(base)?;
    let asset = manager.manifest().asset_for_current_platform()?;
    Ok(crate::backend::InstallSource::Url(
        manager.manifest().source_url(asset, Some(base)),
    ))
}

fn validate_runtime_base_url(value: &str) -> Result<()> {
    let parsed = url::Url::parse(value).context("enter a valid runtime mirror URL")?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
        || parsed.host_str() == Some("localhost");
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        anyhow::bail!("runtime mirror must use HTTPS (HTTP is allowed only on loopback)");
    }
    Ok(())
}

fn configure_scope_and_network(draft: &mut Draft) -> Result<bool> {
    if draft.config.aishe.safety_profile == "autonomous"
        && cfg!(target_os = "linux")
        && !matches!(
            crate::dependencies::bubblewrap_probe(),
            crate::dependencies::BubblewrapState::Usable { .. }
        )
    {
        promptui::error(
            "Autonomous workspace mode requires functional bubblewrap on Linux. Return to the Sandbox step or choose another behavior profile.",
        );
        return Ok(false);
    }
    let policy = crate::policy::load()?;
    let host_allowed = policy
        .as_ref()
        .and_then(|loaded| loaded.policy.allow_host_yolo)
        != Some(false);
    let mut scopes = vec!["Workspace — project writes only; safest default".to_string()];
    if host_allowed {
        scopes.push("Host — full user/system access after per-shell yolo acceptance".into());
    }
    let default = usize::from(draft.config.backend.default_scope == "host" && host_allowed);
    match promptui::menu(
        "Default execution scope",
        &scopes,
        default,
        true,
        "This is an authority boundary, not yolo acceptance. Yolo always requires a new explicit acceptance in each live shell.",
    )? {
        MenuResult::Selected(0) => draft.config.backend.default_scope = "workspace".into(),
        MenuResult::Selected(1) if host_allowed => draft.config.backend.default_scope = "host".into(),
        MenuResult::Back | MenuResult::Cancel => return Ok(false),
        MenuResult::Selected(_) => unreachable!(),
    }
    if draft.config.backend.default_scope == "workspace" {
        let network_forbidden = policy
            .as_ref()
            .and_then(|loaded| loaded.policy.allow_network)
            == Some(false);
        if network_forbidden {
            draft.config.backend.workspace_network = "deny".into();
            promptui::warning("Network: denied · Managed by organization");
        } else {
            let choices = vec![
                "Deny — no network from workspace agent commands".into(),
                "Allow — network tools remain subject to mode approvals".into(),
            ];
            match promptui::menu(
                "Workspace network",
                &choices,
                usize::from(draft.config.backend.workspace_network == "allow"),
                true,
                "Provider API traffic is separate. This controls network access by model-selected tools and commands.",
            )? {
                MenuResult::Selected(0) => draft.config.backend.workspace_network = "deny".into(),
                MenuResult::Selected(1) => draft.config.backend.workspace_network = "allow".into(),
                MenuResult::Back | MenuResult::Cancel => return Ok(false),
                MenuResult::Selected(_) => unreachable!(),
            }
        }
    }
    Ok(true)
}

fn validate_managed_backend(config: &Config) -> Result<()> {
    if config.backend.engine != "opencode" {
        anyhow::bail!("the v0.5 agent implementation requires backend.engine=opencode");
    }
    let manager = crate::backend::RuntimeManager::new()?;
    manager.verify()?;
    crate::backend::supervisor::smoke_test(&manager)?;
    let state = crate::backend::supervisor::ensure_running(config)?;
    let expected = manager.manifest().version.as_str();
    let result = if state.runtime_version == expected
        && state.plugin_sha256 == env!("AISHE_OPENCODE_PLUGIN_SHA256")
        && state.opencode_url.starts_with("http://127.0.0.1:")
        && state.control_url.starts_with("http://127.0.0.1:")
    {
        Ok(())
    } else {
        anyhow::bail!("managed backend identity or loopback isolation is inconsistent");
    };
    let _ = crate::backend::supervisor::request_stop();
    result
}

fn apply_service(config: &mut Config, service: &provider_catalog::Service) {
    let id = match service.family {
        Family::Anthropic => "anthropic",
        Family::OpenAiCompatible => "openai",
    };
    if !config.connections.contains_key(id) {
        let settings = if id == "anthropic" {
            config.providers.anthropic.clone()
        } else {
            config.providers.openai.clone()
        };
        let provider_name = match service.family {
            Family::Anthropic => "anthropic",
            Family::OpenAiCompatible => service.key,
        };
        let auth = crate::config::ConnectionAuth::Auto;
        let label = crate::config::branded_connection_label(provider_name, service.base_url, &auth)
            .unwrap_or_else(|| service.label.into());
        config.connections.insert(
            id.into(),
            crate::config::ConnectionConfig {
                provider: provider_name.into(),
                label,
                settings,
                auth,
                reasoning_effort: None,
            },
        );
    }
    let _ = config.select_connection(id);
    if id == "anthropic" {
        provider_catalog::apply(service, &mut config.providers.anthropic);
    } else {
        provider_catalog::apply(service, &mut config.providers.openai);
    }
    provider_catalog::apply(service, active_provider_mut(config));
    let provider_name = match service.family {
        Family::Anthropic => "anthropic",
        Family::OpenAiCompatible => service.key,
    };
    if let Some(connection) = config.active_connection_mut() {
        connection.provider = provider_name.into();
        // Keep brand labels current when switching openai/xai presets before auth.
        if !connection.uses_oauth() {
            if let Some(label) = crate::config::branded_connection_label(
                provider_name,
                &connection.settings.base_url,
                &connection.auth,
            ) {
                connection.label = label;
            }
        }
    }
    config.aishe.provider = provider_name.into();
}

fn apply_overrides(config: &mut Config, options: &Options) -> Result<()> {
    if let Some(backend) = &options.backend {
        config.backend.engine = backend.clone();
    }
    if let Some(scope) = &options.default_scope {
        config.backend.default_scope = scope.clone();
    }
    if let Some(network) = &options.network {
        config.backend.workspace_network = network.clone();
    }
    if let Some(output) = &options.output {
        config.backend.output = output.clone();
    }
    if let Some(sandbox) = &options.sandbox {
        config.sandbox.linux_backend = sandbox.clone();
        config.sandbox.require_functional = sandbox == "bwrap";
    }
    let provider = active_provider_mut(config);
    if let Some(base_url) = &options.base_url {
        provider.base_url = provider_catalog::normalize_base_url(base_url);
    }
    if let Some(key_env) = &options.key_env {
        validate_env_name(key_env)?;
        provider.api_key_env = key_env.clone();
    }
    if let Some(profile) = &options.credential_profile {
        provider.credential = crate::credentials::normalize_profile(profile)?;
    }
    if let Some(model) = &options.model {
        if model.trim().is_empty() {
            anyhow::bail!("--model cannot be empty");
        }
        provider.model = model.trim().to_string();
    }
    if let Some(transport) = &options.transport {
        validate_transport(transport)?;
        provider.transport = transport.clone();
    }
    let configure_key = options.key_env.is_some() || options.credential_profile.is_some();
    let settings = active_provider(config).clone();
    if !settings.requires_auth() {
        set_active_auth(config, crate::config::ConnectionAuth::None);
    } else if configure_key {
        set_active_auth(
            config,
            crate::config::ConnectionAuth::ApiKey {
                credential: Some(settings.credential_profile()),
                api_key_env: Some(settings.api_key_env),
            },
        );
    }
    if let Some(profile) = options.profile {
        profiles::apply(config, profile);
    } else if config.aishe.safety_profile == "custom" && !Config::path().exists() {
        profiles::apply(config, Profile::Conservative);
    }
    match (options.input_price, options.output_price) {
        (Some(input), Some(output)) => {
            validate_rate(input)?;
            validate_rate(output)?;
            config
                .pricing
                .insert(config.active_model().to_string(), Price { input, output });
        }
        (None, None) => {}
        _ => anyhow::bail!("--input-price and --output-price must be provided together"),
    }
    Ok(())
}

fn active_provider(config: &Config) -> &crate::config::ProviderConfig {
    config.active_provider_config()
}

fn active_provider_mut(config: &mut Config) -> &mut crate::config::ProviderConfig {
    let id = config.active_connection_id().to_string();
    if config.connections.contains_key(&id) {
        &mut config.connections.get_mut(&id).expect("checked").settings
    } else if config.aishe.provider == "anthropic" {
        &mut config.providers.anthropic
    } else {
        &mut config.providers.openai
    }
}

fn set_active_auth(config: &mut Config, auth: crate::config::ConnectionAuth) {
    if let Some(connection) = config.active_connection_mut() {
        if let Some(label) = crate::config::branded_connection_label(
            &connection.provider,
            &connection.settings.base_url,
            &auth,
        ) {
            connection.label = label;
        }
        connection.auth = auth;
    }
}

fn fresh_draft(config: Config) -> Draft {
    let provider = config.active_provider_name();
    let base_url = config
        .active_provider_config()
        .base_url
        .trim_end_matches('/');
    let service = provider_catalog::SERVICES
        .iter()
        .find(|service| {
            service.key == provider
                && !service.base_url.is_empty()
                && service.base_url.trim_end_matches('/') == base_url
        })
        .or_else(|| {
            provider_catalog::SERVICES.iter().find(|service| {
                !service.base_url.is_empty() && service.base_url.trim_end_matches('/') == base_url
            })
        })
        .map_or("custom", |service| service.key);
    let prefer_oauth = matches!(
        config
            .active_connection()
            .map(|connection| &connection.auth),
        Some(crate::config::ConnectionAuth::OAuth { .. })
    ) && matches!(service, "openai" | "xai");
    Draft {
        schema_version: DRAFT_SCHEMA_VERSION,
        step: Step::Discovery,
        service: service.into(),
        prefer_oauth,
        config,
    }
}

fn advance(draft: &mut Draft) -> Result<()> {
    draft.step = draft.step.next();
    save_draft(draft)
}

fn cancel(draft: Draft) -> Result<Outcome> {
    save_draft(&draft)?;
    println!(
        "\n  Setup paused. Resume with `aishe setup --resume`; active config was not changed."
    );
    Ok(Outcome {
        exit_code: EXIT_PAUSED,
        applied: false,
        config_path: Config::path(),
        backup: None,
        report: None,
    })
}

fn prompt_manual_model(
    draft: &mut Draft,
    catalog: Option<&[String]>,
    pending_credential: &Option<(String, String)>,
) -> Result<PromptResult<()>> {
    let mut current = active_provider(&draft.config).model.clone();
    loop {
        match promptui::text("Model ID", &current, |value| {
            if value.trim().is_empty() {
                anyhow::bail!("model cannot be empty");
            }
            if value.len() > 256 || value.chars().any(char::is_control) {
                anyhow::bail!("model must be at most 256 characters with no control characters");
            }
            Ok(())
        })? {
            Some(value) if value == ":back" => return Ok(PromptResult::Back),
            Some(value) => {
                let model = value.trim().to_string();
                if catalog.is_some_and(|models| models.iter().any(|item| item == &model)) {
                    active_provider_mut(&mut draft.config).model = model.clone();
                    promptui::success(&format!(
                        "Model '{model}' is present in the current endpoint catalog"
                    ));
                    return Ok(PromptResult::Value(()));
                }

                if catalog.is_some() {
                    let Some(validate) = promptui::confirm(
                        "Model was not listed. Validate it with one minimal request? (may use tokens)",
                        true,
                    )?
                    else {
                        return Ok(PromptResult::Cancel);
                    };
                    if !validate {
                        current = model;
                        continue;
                    }
                }
                promptui::warning(&format!(
                    "Model '{model}' was not returned by /v1/models; making one minimal request to validate the exact ID"
                ));
                let mut candidate = draft.config.clone();
                active_provider_mut(&mut candidate).model = model.clone();
                let checked = with_pending(pending_credential, || {
                    capabilities::validate_model_request(&candidate)
                })?;
                match checked {
                    Ok(()) => {
                        active_provider_mut(&mut draft.config).model = model.clone();
                        promptui::success(&format!(
                            "Model '{model}' accepted a generation request"
                        ));
                        return Ok(PromptResult::Value(()));
                    }
                    Err(error) => {
                        promptui::error(&format!(
                            "Model validation failed ({:?}): {}",
                            error.kind(),
                            crate::redact::redact(&error.to_string())
                        ));
                        current = model;
                    }
                }
            }
            None => return Ok(PromptResult::Cancel),
        }
    }
}

fn prompt_rate(label: &str) -> Result<PromptResult<f64>> {
    let Some(value) = promptui::text(label, "0", |value| {
        let rate: f64 = value.parse().context("enter a number such as 0.25")?;
        validate_rate(rate)
    })?
    else {
        return Ok(PromptResult::Cancel);
    };
    if value == ":back" {
        return Ok(PromptResult::Back);
    }
    Ok(PromptResult::Value(value.parse()?))
}

fn choose_status_items(draft: &mut Draft) -> Result<bool> {
    let choices = vec![
        "Compact — identity, mode, scope, session cost, requests".into(),
        "Detailed — identity, mode, scope, last/session tokens and costs, requests".into(),
        "Identity — connection, auth, model, reasoning, and selection only".into(),
        "Custom ordered fields…".into(),
    ];
    match promptui::menu(
        "Status-line contents",
        &choices,
        0,
        true,
        "Fields: identity, connection, provider, endpoint, auth, selection, model, reasoning, mode, backend, scope, task, elapsed, context, last_tokens, last_cost, session_tokens, session_cost, requests, plan.",
    )? {
        MenuResult::Selected(0) => {
            draft.config.aishe.status_line_items =
                ["identity", "mode", "scope", "session_cost", "requests"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }
        MenuResult::Selected(1) => {
            draft.config.aishe.status_line_items = [
                "identity",
                "mode",
                "backend",
                "scope",
                "last_tokens",
                "last_cost",
                "session_tokens",
                "session_cost",
                "requests",
            ]
            .into_iter()
            .map(str::to_string)
            .collect();
        }
        MenuResult::Selected(2) => {
            draft.config.aishe.status_line_items =
                ["identity", "mode", "scope"]
                    .into_iter()
                    .map(str::to_string)
                    .collect();
        }
        MenuResult::Selected(3) => {
            let default = draft.config.aishe.status_line_items.join(",");
            let Some(value) = promptui::text("Comma-separated fields", &default, |value| {
                validate_status_items(
                    &value
                        .split(',')
                        .map(|item| item.trim().to_string())
                        .collect::<Vec<_>>(),
                )
            })?
            else {
                return Ok(false);
            };
            if value == ":back" {
                return Ok(false);
            }
            draft.config.aishe.status_line_items = value
                .split(',')
                .map(|item| item.trim().to_string())
                .collect();
        }
        MenuResult::Back | MenuResult::Cancel => return Ok(false),
        MenuResult::Selected(_) => unreachable!(),
    }
    Ok(true)
}

fn validate_status_items(items: &[String]) -> Result<()> {
    const ALLOWED: &[&str] = &[
        "identity",
        "connection",
        "provider",
        "endpoint",
        "auth",
        "selection",
        "model",
        "reasoning",
        "mode",
        "backend",
        "scope",
        "task",
        "elapsed",
        "context",
        "last_tokens",
        "last_cost",
        "session_tokens",
        "session_cost",
        "requests",
        "plan",
    ];
    if items.is_empty() || items.iter().all(|item| item.is_empty()) {
        anyhow::bail!("choose at least one status field");
    }
    if let Some(item) = items.iter().find(|item| !ALLOWED.contains(&item.as_str())) {
        anyhow::bail!("unknown status field '{item}'");
    }
    Ok(())
}

fn print_status_preview(config: &Config) {
    let endpoint = url::Url::parse(&config.active_provider_config().base_url)
        .ok()
        .and_then(|url| url.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let sample = config
        .aishe
        .status_line_items
        .iter()
        .map(|item| match item.as_str() {
            "identity" => format!(
                "{} ({}) · {}@{} · {} · {}/{} · default",
                config
                    .active_connection()
                    .map(|value| value.label.as_str())
                    .unwrap_or(config.active_connection_id()),
                config.active_connection_id(),
                config.active_provider_name(),
                endpoint,
                config
                    .active_connection()
                    .map(crate::config::ConnectionConfig::auth_label)
                    .unwrap_or_else(|| "Auto (legacy)".into()),
                config.active_model(),
                config.active_reasoning_effort(),
            ),
            "connection" => config
                .active_connection()
                .map(|connection| connection.label.clone())
                .unwrap_or_else(|| config.active_connection_id().to_string()),
            "provider" => config.active_provider_name().to_string(),
            "endpoint" => endpoint.clone(),
            "auth" => config
                .active_connection()
                .map(crate::config::ConnectionConfig::auth_label)
                .unwrap_or_else(|| "Auto (legacy)".into()),
            "selection" => "default".into(),
            "model" => config.active_model().to_string(),
            "reasoning" => config.active_reasoning_effort().to_string(),
            "mode" => config.aishe.mode.clone(),
            "backend" => config.backend.engine.clone(),
            "scope" => config.backend.default_scope.clone(),
            "task" => "task repo-audit".into(),
            "elapsed" => "last 4.2s".into(),
            "context" => "context 18%".into(),
            "last_tokens" => "last 1,697/374 tok".into(),
            "last_cost" => "last ~$0.0012".into(),
            "session_tokens" => "session 8,421/1,904 tok".into(),
            "session_cost" => "session ~$0.0174".into(),
            "requests" => "6 reqs".into(),
            "plan" => "plan".into(),
            _ => String::new(),
        })
        .map(|value| crate::commands::display_safe(&value))
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  preview ({}): {sample}",
        config.aishe.status_line_position
    );
}

fn validate_rate(value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("price must be a finite non-negative number");
    }
    Ok(())
}

fn validate_url(value: &str) -> Result<()> {
    let normalized = provider_catalog::normalize_base_url(value);
    if !(normalized.starts_with("http://") || normalized.starts_with("https://")) {
        anyhow::bail!("endpoint must use http:// or https://");
    }
    Ok(())
}

fn validate_env_name(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("environment variable name cannot be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || chars.any(|character| !(character == '_' || character.is_ascii_alphanumeric()))
    {
        anyhow::bail!("use a shell variable name such as OPENAI_API_KEY");
    }
    Ok(())
}

fn validate_transport(value: &str) -> Result<()> {
    if matches!(value, "auto" | "responses" | "chat") {
        Ok(())
    } else {
        anyhow::bail!("transport must be auto, responses, or chat")
    }
}

pub(crate) fn validate_config(config: &Config) -> Result<()> {
    if !matches!(config.backend.engine.as_str(), "opencode" | "native") {
        anyhow::bail!("backend.engine must be opencode or native");
    }
    if !matches!(config.backend.default_scope.as_str(), "workspace" | "host") {
        anyhow::bail!("backend.default_scope must be workspace or host");
    }
    if !matches!(config.backend.workspace_network.as_str(), "allow" | "deny") {
        anyhow::bail!("backend.workspace_network must be allow or deny");
    }
    if !matches!(
        config.backend.output.as_str(),
        "focus" | "compact" | "detailed"
    ) {
        anyhow::bail!("backend.output must be focus, compact, or detailed");
    }
    if !matches!(config.sandbox.linux_backend.as_str(), "bwrap" | "policy") {
        anyhow::bail!("sandbox.linux_backend must be bwrap or policy");
    }
    if config.active_provider_name().trim().is_empty() {
        anyhow::bail!("provider cannot be empty");
    }
    let provider = active_provider(config);
    validate_url(&provider.base_url)?;
    crate::credentials::normalize_profile(&provider.credential_profile())?;
    validate_env_name(&provider.api_key_env)?;
    if provider.model.trim().is_empty() {
        anyhow::bail!("model cannot be empty");
    }
    validate_transport(&provider.transport)?;
    if config.aishe.status_line
        && !matches!(
            config.aishe.status_line_position.as_str(),
            "right" | "below"
        )
    {
        anyhow::bail!("status_line_position must be right, below, or off");
    }
    validate_status_items(&config.aishe.status_line_items)?;
    if !(1..=600).contains(&config.aishe.hook_timeout_secs) {
        anyhow::bail!("hook_timeout_secs must be between 1 and 600");
    }
    Ok(())
}

fn profile_index(value: &str) -> usize {
    match Profile::parse(value) {
        Some(Profile::Conservative) => 0,
        Some(Profile::Balanced) => 1,
        Some(Profile::Autonomous) => 2,
        _ => 3,
    }
}

fn print_report(report: &Report) {
    promptui::section(&format!(
        "Provider check: {} · {} · {}",
        report.provider, report.model, report.transport
    ));
    for (label, check) in [
        ("credential", &report.credential),
        ("reachability", &report.reachability),
        ("model list", &report.model_list),
        ("model", &report.model_available),
        ("text", &report.text),
        ("structured", &report.structured),
        ("tools", &report.tools),
        ("streaming", &report.streaming),
    ] {
        let detail = format!("{label}: {}", crate::commands::display_safe(&check.detail));
        match check.state {
            State::Pass => promptui::success(&detail),
            State::Warn => promptui::warning(&detail),
            State::Fail => promptui::error(&detail),
            State::Skipped => println!("  · {detail}"),
        }
    }
}

fn print_review(
    baseline: &Config,
    configured: &Config,
    report: Option<&Report>,
    pending_credential: Option<&(String, String)>,
) -> Result<()> {
    let before = toml::to_string_pretty(baseline)?;
    let after = toml::to_string_pretty(configured)?;
    promptui::section("Review");
    println!(
        "    provider: {}",
        crate::commands::display_safe(&configured.aishe.provider)
    );
    println!(
        "    endpoint: {}",
        crate::commands::display_safe(&active_provider(configured).base_url)
    );
    println!(
        "    model: {}",
        crate::commands::display_safe(configured.active_model())
    );
    println!(
        "    credential: {} ({})",
        crate::commands::display_safe(&active_provider(configured).credential_profile()),
        if pending_credential.is_some() {
            "will save locally on Apply".to_string()
        } else if active_provider(configured).requires_auth() {
            match crate::credentials::resolve(active_provider(configured)) {
                Ok(resolved) if resolved.secret().is_some() => resolved.source.label(),
                Ok(_) => crate::oauth::active_provider(configured)
                    .ok()
                    .flatten()
                    .map(|provider| format!("{provider} OAuth"))
                    .unwrap_or_else(|| "unavailable".into()),
                Err(_) => "unavailable".into(),
            }
        } else {
            "not required".to_string()
        }
    );
    println!(
        "    environment override: ${}",
        crate::commands::display_safe(&active_provider(configured).api_key_env)
    );
    println!(
        "    profile: {}",
        crate::commands::display_safe(&configured.aishe.safety_profile)
    );
    println!(
        "    backend: {} · OpenCode {}",
        crate::commands::display_safe(&configured.backend.engine),
        crate::backend::RuntimeManifest::embedded()?.version
    );
    println!(
        "    scope/network: {} · {}",
        crate::commands::display_safe(&configured.backend.default_scope),
        crate::commands::display_safe(&configured.backend.workspace_network)
    );
    println!(
        "    sandbox: {}{}",
        crate::commands::display_safe(&configured.sandbox.linux_backend),
        if configured.sandbox.require_functional {
            " · required"
        } else {
            ""
        }
    );
    println!(
        "    interface: {} output · statusline {}",
        crate::commands::display_safe(&configured.backend.output),
        if configured.aishe.status_line {
            configured.aishe.status_line_position.as_str()
        } else {
            "off"
        }
    );
    println!(
        "    pricing/budget: {} · {}",
        if crate::credentials::resolve(active_provider(configured))
            .is_ok_and(|resolved| resolved.secret().is_none())
            && crate::oauth::active_provider(configured)
                .ok()
                .flatten()
                .is_some()
        {
            "provider subscription; API cost not estimated"
        } else if usage::price_for(configured.active_model(), &configured.pricing).is_some() {
            "exact price available"
        } else {
            "price unknown; cost budgets disabled"
        },
        if configured.aishe.budget_usd > 0.0 {
            format!("${:.4} session cap", configured.aishe.budget_usd)
        } else {
            "no user session cap".into()
        }
    );
    println!(
        "    audit/redaction: {} · {}",
        if configured.logging.enabled {
            "audit on"
        } else {
            "audit off"
        },
        if configured.aishe.redact_secrets {
            "redaction on"
        } else {
            "redaction off"
        }
    );
    if let Some(loaded) = crate::policy::load()? {
        println!("    organization: Managed by {}", loaded.path.display());
    }
    println!(
        "    config: {}",
        crate::commands::display_safe(&Config::path().display().to_string())
    );
    if let Some(report) = report {
        println!(
            "    validation: {}",
            if report.verified() {
                "verified"
            } else {
                "warnings remain"
            }
        );
    }
    let diff = crate::undo::unified_diff(&before, &after);
    if diff.is_empty() {
        println!("\n  No configuration changes.");
    } else {
        println!("\n{diff}");
    }
    Ok(())
}

fn with_pending<T>(pending: &Option<(String, String)>, operation: impl FnOnce() -> T) -> Result<T> {
    match pending {
        Some((profile, secret)) => {
            crate::credentials::with_staged(profile, secret.clone(), operation)
        }
        None => Ok(operation()),
    }
}

fn save_with_pending(
    config: &Config,
    pending: Option<(String, String)>,
) -> Result<Option<PathBuf>> {
    let Some((profile, secret)) = pending else {
        return save_applied(config);
    };
    let previous = crate::credentials::Store::load()?;
    let mut updated = previous.clone().unwrap_or_default();
    updated.set(&profile, secret)?;
    updated.save()?;
    match save_applied(config) {
        Ok(backup) => Ok(backup),
        Err(error) => {
            let rollback = match previous {
                Some(store) => store.save(),
                None => match std::fs::remove_file(crate::credentials::path()) {
                    Ok(()) => Ok(()),
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(())
                    }
                    Err(remove_error) => Err(remove_error.into()),
                },
            };
            if let Err(rollback_error) = rollback {
                anyhow::bail!(
                    "{error}; additionally failed to roll back the credential write: \
                     {rollback_error}"
                );
            }
            Err(error)
        }
    }
}

/// Persist credentials/config, then prove the persisted configuration can start
/// the authenticated managed backend. Any failure restores the exact prior
/// bytes (including the absence of a file) and deliberately leaves the setup
/// draft intact for repair/resume.
fn save_transactional(
    config: &Config,
    pending: Option<(String, String)>,
) -> Result<Option<PathBuf>> {
    let config_path = Config::path();
    let credentials_path = crate::credentials::path();
    let result = transaction_with_rollback(
        &config_path,
        &credentials_path,
        || save_with_pending(config, pending),
        || {
            if config.backend.engine == "opencode" {
                crate::backend::supervisor::ensure_running(config).map(|_| ())
            } else {
                Ok(())
            }
        },
    );
    if result.is_err() {
        let _ = crate::backend::control::request_stop();
    }
    result
}

fn transaction_with_rollback<T>(
    config_path: &Path,
    credentials_path: &Path,
    persist: impl FnOnce() -> Result<T>,
    verify: impl FnOnce() -> Result<()>,
) -> Result<T> {
    let prior_config = snapshot_file(config_path)?;
    let prior_credentials = snapshot_file(credentials_path)?;
    let persisted = persist()?;
    if let Err(error) = verify() {
        let config_rollback = restore_file(config_path, prior_config);
        let credential_rollback = restore_file(credentials_path, prior_credentials);
        match (config_rollback, credential_rollback) {
            (Ok(()), Ok(())) => return Err(error).context("persisted backend health check"),
            (config_result, credential_result) => {
                anyhow::bail!(
                    "persisted backend health check failed: {error}; config rollback: {}; \
                     credential rollback: {}",
                    config_result
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "ok".into()),
                    credential_result
                        .err()
                        .map(|error| error.to_string())
                        .unwrap_or_else(|| "ok".into())
                );
            }
        }
    }
    Ok(persisted)
}

fn snapshot_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("snapshotting {}", path.display())),
    }
}

fn restore_file(path: &Path, snapshot: Option<Vec<u8>>) -> Result<()> {
    match snapshot {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
                crate::config::set_private_dir(parent);
            }
            crate::config::write_atomic(path, &bytes)?;
            crate::config::set_private_file(path);
            Ok(())
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        },
    }
}

pub(crate) fn save_applied(config: &Config) -> Result<Option<PathBuf>> {
    let path = Config::path();
    let backup = if path.exists() {
        Some(create_setup_backup(&path)?)
    } else {
        None
    };
    config.save()?;
    let reloaded = Config::load_quiet()?.context("saved config could not be reloaded")?;
    validate_config(&reloaded)?;
    Ok(backup)
}

fn create_setup_backup(source: &Path) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for suffix in 0u32.. {
        let extension = if suffix == 0 {
            format!("toml.setup.{stamp}.bak")
        } else {
            format!("toml.setup.{stamp}.{suffix}.bak")
        };
        let backup = source.with_extension(extension);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut destination = match options.open(&backup) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("creating config backup {}", backup.display()))
            }
        };
        let result = (|| -> Result<()> {
            let mut original = std::fs::File::open(source)
                .with_context(|| format!("opening config {}", source.display()))?;
            std::io::copy(&mut original, &mut destination)
                .with_context(|| format!("backing up config to {}", backup.display()))?;
            destination
                .sync_all()
                .with_context(|| format!("syncing config backup {}", backup.display()))?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&backup);
            return Err(error);
        }
        set_private_permissions(&backup);
        return Ok(backup);
    }
    unreachable!("u32 backup suffix space exhausted")
}

fn draft_path() -> Option<PathBuf> {
    crate::config::data_root().map(|root| root.join("aishe").join("setup-draft.json"))
}

fn save_draft(draft: &Draft) -> Result<()> {
    let path = draft_path().context("data directory is unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_private_directory(parent);
    }
    let bytes = serde_json::to_vec_pretty(draft)?;
    crate::config::write_atomic(&path, &bytes)?;
    set_private_permissions(&path);
    Ok(())
}

fn load_draft() -> Result<Option<Draft>> {
    let Some(path) = draft_path() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let draft: Draft = serde_json::from_slice(&bytes)
        .with_context(|| format!("reading setup draft {}", path.display()))?;
    if draft.schema_version != DRAFT_SCHEMA_VERSION {
        anyhow::bail!(
            "setup draft schema {} is unsupported; use `aishe setup --restart`",
            draft.schema_version
        );
    }
    Ok(Some(draft))
}

fn discard_draft() -> Result<()> {
    let Some(path) = draft_path() else {
        return Ok(());
    };
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

#[cfg(unix)]
fn set_private_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_state_machine_moves_forward_and_back() {
        assert_eq!(Step::Discovery.next(), Step::Platform);
        assert_eq!(Step::Sandbox.next(), Step::Service);
        assert_eq!(Step::Service.next(), Step::Endpoint);
        assert_eq!(Step::Review.next(), Step::Review);
        assert_eq!(Step::Model.previous(), Step::Credential);
        assert_eq!(Step::Service.previous(), Step::Sandbox);
        assert_eq!(Step::Discovery.previous(), Step::Discovery);
    }

    #[test]
    fn noninteractive_options_apply_service_profile_and_price() {
        let mut config = Config::default();
        let options = Options {
            service: Some("ollama".into()),
            model: Some("qwen-test".into()),
            profile: Some(Profile::Balanced),
            input_price: Some(0.0),
            output_price: Some(0.0),
            ..Options::default()
        };
        let service = provider_catalog::find("ollama").unwrap();
        apply_service(&mut config, service);
        apply_overrides(&mut config, &options).unwrap();
        assert_eq!(config.aishe.provider, "ollama");
        assert_eq!(config.active_connection_id(), "openai");
        assert_eq!(config.active_provider_name(), "ollama");
        assert_eq!(config.active_model(), "qwen-test");
        assert!(!config.providers.openai.requires_auth());
        assert_eq!(config.aishe.safety_profile, "balanced");
        assert_eq!(config.pricing["qwen-test"].input, 0.0);
    }

    #[test]
    fn xai_preset_selects_responses_and_oauth_eligible_endpoint() {
        let service = provider_catalog::find("xai").unwrap();
        let mut config = Config::default();
        apply_service(&mut config, service);
        assert_eq!(config.aishe.provider, "xai");
        assert_eq!(config.active_connection_id(), "openai");
        assert_eq!(config.active_provider_name(), "xai");
        assert_eq!(config.providers.openai.base_url, "https://api.x.ai");
        assert_eq!(config.providers.openai.credential, "xai");
        assert_eq!(config.providers.openai.api_key_env, "XAI_API_KEY");
        assert_eq!(config.providers.openai.transport, "responses");
        assert_eq!(config.active_connection().unwrap().provider, "xai");
        assert_eq!(
            crate::oauth::OAuthProvider::from_base_url(&config.providers.openai.base_url),
            Some(crate::oauth::OAuthProvider::Xai)
        );
    }

    #[test]
    fn noninteractive_setup_can_preserve_legacy_auto_auth_explicitly() {
        let service = provider_catalog::find("openai").unwrap();
        let mut config = Config::default();
        apply_service(&mut config, service);
        let options = Options {
            service: Some("openai".into()),
            ..Options::default()
        };
        apply_overrides(&mut config, &options).unwrap();
        assert!(matches!(
            config.active_connection().unwrap().auth,
            crate::config::ConnectionAuth::Auto
        ));
    }

    #[test]
    fn validation_rejects_bad_env_and_rates() {
        assert!(validate_env_name("OPENAI_API_KEY").is_ok());
        assert!(validate_env_name("bad-name").is_err());
        assert!(validate_rate(f64::NAN).is_err());
        assert!(validate_rate(-1.0).is_err());
    }

    #[test]
    fn noninteractive_runtime_inputs_are_explicit_and_non_conflicting() {
        assert!(validate_noninteractive_options(&Options {
            runtime_file: Some(PathBuf::from("runtime.tar.gz")),
            ..Options::default()
        })
        .is_err());
        assert!(validate_noninteractive_options(&Options {
            install_backend: true,
            runtime_file: Some(PathBuf::from("runtime.tar.gz")),
            runtime_base_url: Some("https://mirror.example/runtime".into()),
            ..Options::default()
        })
        .is_err());
        assert!(validate_noninteractive_options(&Options {
            install_backend: true,
            runtime_base_url: Some("https://mirror.example/runtime".into()),
            ..Options::default()
        })
        .is_ok());
    }

    #[test]
    fn draft_contains_no_environment_values() {
        let draft = fresh_draft(Config::default());
        let encoded = serde_json::to_string(&draft).unwrap();
        assert!(encoded.contains("api_key_env"));
        assert!(!encoded.contains("sk-"));
        assert_eq!(draft.config.version, crate::config::CONFIG_SCHEMA_VERSION);
    }

    #[test]
    fn fresh_draft_recognizes_the_active_catalog_service() {
        for key in ["anthropic", "openai", "xai", "ollama"] {
            let mut config = Config::default();
            apply_service(&mut config, provider_catalog::find(key).unwrap());
            assert_eq!(fresh_draft(config).service, key);
        }
    }

    #[test]
    fn setup_service_menu_surfaces_explicit_oauth_options_first() {
        let entries = service_menu_entries();
        assert_eq!(entries[0], ServiceMenuEntry::ChatGptCodexOAuth);
        assert_eq!(entries[1], ServiceMenuEntry::GrokOAuth);
        assert_eq!(
            service_menu_label(ServiceMenuEntry::ChatGptCodexOAuth),
            "ChatGPT / Codex OAuth — Sign in with ChatGPT Plus/Pro (no API key)"
        );
        assert_eq!(
            service_menu_label(ServiceMenuEntry::GrokOAuth),
            "Grok OAuth — Sign in with SuperGrok subscription (no API key)"
        );
        assert!(entries.iter().any(|entry| {
            matches!(entry, ServiceMenuEntry::Catalog(index)
                if provider_catalog::SERVICES[*index].key == "openai")
        }));
        assert!(entries.iter().any(|entry| {
            matches!(entry, ServiceMenuEntry::Catalog(index)
                if provider_catalog::SERVICES[*index].key == "xai")
        }));

        let mut draft = fresh_draft(Config::default());
        draft.service = "openai".into();
        draft.prefer_oauth = true;
        assert_eq!(service_menu_default(&draft), 0);
        draft.service = "xai".into();
        assert_eq!(service_menu_default(&draft), 1);
        draft.prefer_oauth = false;
        draft.service = "ollama".into();
        let ollama = service_menu_default(&draft);
        assert!(matches!(
            service_menu_entries()[ollama],
            ServiceMenuEntry::Catalog(index)
                if provider_catalog::SERVICES[index].key == "ollama"
        ));

        assert_eq!(
            oauth_subscription_choice(crate::oauth::OAuthProvider::Openai),
            "Sign in with ChatGPT / Codex OAuth (Plus/Pro subscription)"
        );
        assert_eq!(
            oauth_subscription_choice(crate::oauth::OAuthProvider::Xai),
            "Sign in with Grok OAuth (SuperGrok subscription)"
        );
    }

    #[test]
    fn explicit_oauth_service_choices_bind_official_endpoints() {
        for (entry, key, oauth) in [
            (
                ServiceMenuEntry::ChatGptCodexOAuth,
                "openai",
                crate::oauth::OAuthProvider::Openai,
            ),
            (
                ServiceMenuEntry::GrokOAuth,
                "xai",
                crate::oauth::OAuthProvider::Xai,
            ),
        ] {
            let service = match entry {
                ServiceMenuEntry::ChatGptCodexOAuth => provider_catalog::find("openai").unwrap(),
                ServiceMenuEntry::GrokOAuth => provider_catalog::find("xai").unwrap(),
                ServiceMenuEntry::Catalog(_) => unreachable!(),
            };
            let mut config = Config::default();
            apply_service(&mut config, service);
            assert_eq!(config.active_provider_name(), key);
            assert_eq!(
                crate::oauth::OAuthProvider::from_base_url(
                    &config.active_provider_config().base_url
                ),
                Some(oauth)
            );
        }
    }

    #[test]
    fn failed_post_apply_verification_restores_exact_prior_files() {
        let root = std::env::temp_dir().join(format!(
            "aishe-setup-rollback-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        let credentials = root.join("credentials.toml");
        std::fs::write(&config, b"old-config\n").unwrap();
        std::fs::write(&credentials, b"old-secret-store\n").unwrap();

        let result = transaction_with_rollback(
            &config,
            &credentials,
            || {
                crate::config::write_atomic(&config, b"new-config\n")?;
                crate::config::write_atomic(&credentials, b"new-secret-store\n")?;
                Ok(())
            },
            || anyhow::bail!("injected backend health failure"),
        );
        assert!(result.unwrap_err().to_string().contains("health check"));
        assert_eq!(std::fs::read(&config).unwrap(), b"old-config\n");
        assert_eq!(std::fs::read(&credentials).unwrap(), b"old-secret-store\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_first_apply_removes_new_config_and_credentials() {
        let root = std::env::temp_dir().join(format!(
            "aishe-setup-first-rollback-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = root.join("config.toml");
        let credentials = root.join("credentials.toml");

        let result = transaction_with_rollback(
            &config,
            &credentials,
            || {
                crate::config::write_atomic(&config, b"new-config\n")?;
                crate::config::write_atomic(&credentials, b"new-secret-store\n")?;
                Ok(())
            },
            || anyhow::bail!("injected backend health failure"),
        );
        assert!(result.is_err());
        assert!(!config.exists());
        assert!(!credentials.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
