//! aishe — a natural-language-aware shell.
//!
//! Behaves like zsh for recognizable commands; anything else is treated as a
//! natural-language request handled by an LLM (suggest or yolo mode).

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::style::Stylize;

use aishe::agent::controller::{TurnFailure, TurnOptions, TurnOutcome, INTERRUPTED};
use aishe::agent::Mode as AgentMode;
use aishe::commands::CommandRegistry;
use aishe::config::Config;
use aishe::dispatcher::{self, CommandCache, Dispatch};
use aishe::executor::Executor;
use aishe::providers::{self, Provider};
use aishe::safety::{self, Risk};
use aishe::session::Session;
use aishe::skills::SkillRegistry;
use aishe::{context, integration, modes};

/// Exit code from `--auto-line` when the suggested command is dangerous: the
/// shell hook treats any non-zero code as "pre-fill for review" instead of
/// running. (See `integration::ZSH_HOOK`.)
const EXIT_AUTO_DANGEROUS: u8 = 20;

/// Version string shown by `aishe --version`: crate version plus the build's git
/// SHA and date (captured by `build.rs`).
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AISHE_GIT_SHA"),
    ", ",
    env!("AISHE_BUILD_DATE"),
    ")"
);

extern "C" fn handle_sigint(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Clamp the configurable prompt-blocking shell-hook budget. Explicit commands
/// (`aishe suggest`) deliberately do not use this budget: a script must receive
/// a complete JSON result or a normal provider error, never a signal-truncated
/// success response.
fn hook_budget_secs(config: &Config) -> u32 {
    config.aishe.hook_timeout_secs.clamp(1, 600)
}

/// SIGALRM handler for the hook budget. The whole point is to leave stdout EMPTY
/// so the shell hook sees no suggestion and the prompt simply returns. This runs
/// in signal context, so it must be async-signal-safe: only a single raw
/// `libc::write` to fd 2 (stderr) and `libc::_exit` — no Rust stdlib I/O and no
/// allocation.
extern "C" fn handle_hook_alarm(_sig: libc::c_int) {
    const MSG: &[u8] = b"aishe: suggestion timed out\n";
    unsafe {
        libc::write(2, MSG.as_ptr() as *const libc::c_void, MSG.len());
        libc::_exit(0);
    }
}

/// Install the SIGALRM handler and arm the configured hook alarm. The caller is
/// responsible for cancelling it (`libc::alarm(0)`) before returning normally.
fn arm_hook_budget(config: &Config) {
    unsafe {
        libc::signal(
            libc::SIGALRM,
            handle_hook_alarm as *const () as libc::sighandler_t,
        );
        libc::alarm(hook_budget_secs(config));
    }
}

/// Cancel a previously armed hook budget alarm.
fn cancel_hook_budget() {
    unsafe {
        libc::alarm(0);
    }
}

#[derive(Parser, Debug)]
#[command(name = "aishe", version = VERSION, about = "A natural-language-aware shell")]
struct Args {
    /// Override the interaction mode for this session.
    #[arg(long, value_parser = ["suggest", "auto", "yolo"])]
    mode: Option<String>,
    /// Override the model for this session.
    #[arg(long)]
    model: Option<String>,
    /// Override the provider for this session.
    #[arg(long, value_parser = ["anthropic", "openai"])]
    provider: Option<String>,
    /// Run a single input non-interactively and exit.
    #[arg(short = 'c')]
    command: Option<String>,
    /// (shell hook) Suggest a command for a natural-language line: prints the
    /// command to stdout and the explanation/answer to stderr.
    #[arg(long, hide = true)]
    suggest_line: Option<String>,
    /// (shell hook) Run the yolo loop for a natural-language line.
    #[arg(long, hide = true)]
    yolo_line: Option<String>,
    /// (shell hook) Auto mode: print a suggested command and exit 0 if the
    /// safety gate deems it safe (caller runs it), or a non-zero code if
    /// dangerous (caller pre-fills it for review).
    #[arg(long, hide = true)]
    auto_line: Option<String>,
    /// (shell hook) Fix-the-last-command: given the failed command, print a
    /// corrected one. Reads the exit status from `$AISHE_LAST_EXIT`.
    #[arg(long, hide = true)]
    fix_line: Option<String>,
    /// (shell hook) Accept the configured yolo scope for this live shell.
    #[arg(long, hide = true)]
    accept_yolo: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Args, Debug)]
struct SetupArgs {
    /// Resume the last interrupted setup draft.
    #[arg(long)]
    resume: bool,
    /// Discard only the setup draft and start from the active config.
    #[arg(long)]
    restart: bool,
    /// Verify the active config without changing it.
    #[arg(long)]
    verify: bool,
    /// Configure from flags without opening a terminal UI.
    #[arg(long)]
    non_interactive: bool,
    /// Service preset: anthropic, openai, groq, openrouter, together, ollama, custom.
    #[arg(long)]
    service: Option<String>,
    /// Provider base URL (host root, without /v1).
    #[arg(long)]
    base_url: Option<String>,
    /// Name of the environment variable containing the API key.
    #[arg(long)]
    key_env: Option<String>,
    /// Saved credential profile name.
    #[arg(long)]
    credential_profile: Option<String>,
    /// Model identifier.
    #[arg(long)]
    model: Option<String>,
    /// API transport: auto, responses, or chat.
    #[arg(long, value_parser = ["auto", "responses", "chat"])]
    transport: Option<String>,
    /// Safety profile.
    #[arg(long, value_parser = ["conservative", "balanced", "autonomous", "custom"])]
    profile: Option<String>,
    /// Input price in USD per million tokens.
    #[arg(long)]
    input_price: Option<f64>,
    /// Output price in USD per million tokens.
    #[arg(long)]
    output_price: Option<f64>,
    /// Make minimal live generation requests while validating.
    #[arg(long)]
    live: bool,
    /// Agent backend (enterprise setup currently supports opencode).
    #[arg(long, value_parser = ["opencode"])]
    backend: Option<String>,
    /// Install or repair the pinned managed OpenCode runtime.
    #[arg(long)]
    install_backend: bool,
    /// Install the runtime from an approved local archive.
    #[arg(long, value_name = "PATH", conflicts_with = "runtime_base_url")]
    runtime_file: Option<std::path::PathBuf>,
    /// Download the pinned runtime asset from this mirror base URL.
    #[arg(long, value_name = "URL", conflicts_with = "runtime_file")]
    runtime_base_url: Option<String>,
    /// Linux sandbox requirement.
    #[arg(long, value_parser = ["bwrap", "policy"])]
    sandbox: Option<String>,
    /// Explicitly authorize supported system package installation.
    #[arg(long)]
    install_system_deps: bool,
    /// Default agent execution scope.
    #[arg(long, value_parser = ["workspace", "host"])]
    default_scope: Option<String>,
    /// Workspace-agent network policy.
    #[arg(long, value_parser = ["allow", "deny"])]
    network: Option<String>,
    /// Native agent transcript density.
    #[arg(long, value_parser = ["compact", "detailed"])]
    output: Option<String>,
    /// Emit a stable machine-readable setup result.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Internal managed backend supervisor.
    #[command(name = "__backend-supervisor", hide = true)]
    BackendSupervisor,
    /// Configure and verify Aishe interactively, or provision it with flags.
    Setup(Box<SetupArgs>),
    /// Edit the current configuration through an interactive section hub.
    Settings {
        /// Print effective fields and their provenance as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage provider API keys in Aishe's private shared credentials file.
    Auth {
        #[command(subcommand)]
        cmd: aishe::auth::AuthCommand,
    },
    /// Run the resumable guided first-session tour.
    Tour {
        /// Discard only tour progress/workspace and begin at lesson one.
        #[arg(long)]
        restart: bool,
        /// Run every lesson without terminal prompts.
        #[arg(long)]
        non_interactive: bool,
    },
    /// Print a shell integration snippet: `eval "$(aishe init zsh)"`.
    Init {
        /// Shell to emit integration for (zsh or bash).
        shell: String,
    },
    /// Launch your real interactive zsh (with all native plugins) under aishe.
    Zsh,
    /// Check your environment: shell, config, front-end, provider, API key.
    Doctor {
        /// Also probe each provider in the chain for reachability (a short
        /// network request per endpoint; costs no tokens).
        #[arg(long)]
        probe: bool,
        /// Run minimal live text, structured-output, tools, and streaming checks.
        #[arg(long)]
        live: bool,
        /// Emit the structured report as JSON.
        #[arg(long)]
        json: bool,
        /// Apply safe, local, idempotent repairs (never installs packages).
        #[arg(long)]
        fix: bool,
        /// Write a redacted JSON support bundle to this path.
        #[arg(long, value_name = "PATH")]
        bundle: Option<std::path::PathBuf>,
    },
    /// Manage Aishe's private, compatibility-pinned agent runtime.
    Backend {
        #[command(subcommand)]
        cmd: BackendCmd,
    },
    /// Print a shell completion script for `aishe` itself (bash/zsh/fish/...).
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
    /// Print a roff man page for `aishe` (e.g. `aishe man > /usr/share/man/man1/aishe.1`).
    Man,
    /// Remove Aishe components by category; user state is preserved by default.
    Uninstall {
        /// Remove the running Aishe binary plus known completion/man artifacts.
        #[arg(long)]
        binary: bool,
        /// Remove managed OpenCode runtimes and disposable runtime caches.
        #[arg(long)]
        runtime: bool,
        /// Permanently remove AI sessions and tool journals.
        #[arg(long)]
        sessions: bool,
        /// Permanently remove config, credentials, commands, and skills.
        #[arg(long)]
        config: bool,
        /// Permanently remove shell and semantic history.
        #[arg(long)]
        history: bool,
        /// Permanently remove audit and undo journals.
        #[arg(long)]
        audit_undo: bool,
        /// Select every category, including all user state.
        #[arg(long)]
        all: bool,
        /// Show the exact removal plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Confirm the displayed plan non-interactively.
        #[arg(long)]
        yes: bool,
    },
    /// Trust the current project's `.aishe/config.toml` so its sensitive keys
    /// (provider/endpoint, MCP servers, audit logging, safety toggles, `yolo`)
    /// apply. Safe cosmetic keys apply without trust.
    Trust {
        /// List every trusted file instead of trusting one.
        #[arg(long)]
        list: bool,
        /// A specific project file to trust — a skill
        /// (`.aishe/skills/<name>/SKILL.md`) or a command
        /// (`.aishe/commands/<name>.md`). Defaults to this project's
        /// `.aishe/config.toml`.
        path: Option<std::path::PathBuf>,
    },
    /// Drop trust for the current project's `.aishe/config.toml`, or for a
    /// specific project file.
    Untrust {
        /// Drop trust for every trusted file, not just this one.
        #[arg(long)]
        all: bool,
        /// A specific project file to untrust. Defaults to this project's
        /// `.aishe/config.toml`.
        path: Option<std::path::PathBuf>,
    },
    /// Show or set the interaction mode (with a value, saves it to your config).
    Mode {
        #[arg(value_parser = ["suggest", "auto", "yolo"])]
        value: Option<String>,
    },
    /// Show or set the agent execution scope for future turns.
    Scope {
        #[arg(value_parser = ["workspace", "host"])]
        value: Option<String>,
    },
    /// Show or set network access for workspace-scoped agent turns.
    Network {
        #[arg(value_parser = ["allow", "deny"])]
        value: Option<String>,
    },
    /// Show or set the model for the active provider (saves it to your config).
    Model { value: Option<String> },
    /// Show or set the provider (with a value, saves it to your config).
    Provider {
        /// Set anthropic/openai, or use `test` to validate the active provider.
        #[arg(value_parser = ["anthropic", "openai", "test"])]
        value: Option<String>,
        /// With `provider test`, make minimal text/structured/tool/stream requests.
        #[arg(long)]
        live: bool,
        /// With `provider test`, emit the capability report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List models returned by the configured endpoint.
    Models {
        /// Provider block to query (defaults to the active provider).
        #[arg(long, value_parser = ["anthropic", "openai"])]
        provider: Option<String>,
        /// Ignore a cached capability record and request the endpoint again.
        #[arg(long)]
        refresh: bool,
        /// Emit a JSON array.
        #[arg(long)]
        json: bool,
    },
    /// Show or apply a transparent safety profile.
    Profile {
        #[arg(value_parser = ["conservative", "balanced", "autonomous", "custom"])]
        value: Option<String>,
    },
    /// Check whether autonomous mode is ready for real work.
    Readiness {
        #[arg(long)]
        json: bool,
    },
    /// Manage per-model token prices used for estimates and budgets.
    Price {
        #[command(subcommand)]
        cmd: PriceCmd,
    },
    /// Print the active configuration.
    Config {
        /// Include effective-value provenance after project overlays.
        #[arg(long)]
        effective: bool,
        /// Emit JSON instead of TOML/text.
        #[arg(long)]
        json: bool,
    },
    /// List the MCP tools offered to yolo.
    Mcp,
    /// List your custom slash-commands.
    Commands,
    /// List model-invoked skills.
    Skills,
    /// Undo the most recent AI file change (from the built-in file tools).
    Undo {
        /// List recorded change sets instead of reverting.
        #[arg(long)]
        list: bool,
    },
    /// Show the audit log of AI calls and actions (needs logging enabled).
    Log {
        /// Only this session id.
        #[arg(long)]
        session: Option<String>,
        /// Only this event kind (ai_request, ai_response, ai_error, action).
        #[arg(long)]
        action: Option<String>,
        /// Only entries whose model name contains this substring.
        #[arg(long)]
        model: Option<String>,
        /// Only entries newer than this, e.g. 30m, 2h, 3d, 1w.
        #[arg(long)]
        since: Option<String>,
        /// Show at most the last N entries.
        #[arg(short = 'n', long)]
        limit: Option<usize>,
        /// Emit raw JSONL instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Summarize token usage and estimated cost from the audit log.
    Usage {
        /// Group by: model (default), day, or session.
        #[arg(long, value_parser = ["model", "day", "session"])]
        by: Option<String>,
        /// Only entries newer than this, e.g. 30m, 2h, 3d, 1w.
        #[arg(long)]
        since: Option<String>,
    },
    /// Turn a natural-language request into a shell command (for scripting).
    /// Prints the command to stdout; exit 0 = safe/answer, 20 = flagged (either
    /// `dangerous`, or `unknown` when the gate cannot tell what the command runs
    /// — the command is still printed for review), 1 = no provider or no query.
    /// Use `--json` for structured output.
    Suggest {
        /// The natural-language request (any number of words).
        query: Vec<String>,
        /// Emit `{"kind","command","explanation","risk","reason"}` instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Semantic search over your shell history (opt-in; needs an embedder).
    History {
        #[command(subcommand)]
        cmd: HistoryCmd,
    },
    /// List durable AI task sessions.
    Sessions {
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect or manage one durable AI task session.
    Session {
        #[command(subcommand)]
        cmd: TaskSessionCmd,
    },
    /// Resume the most recent interrupted task, or a specific task ID.
    Resume {
        id: Option<String>,
        /// Replacement working directory when the original no longer exists.
        #[arg(long, value_name = "PATH")]
        cwd: Option<std::path::PathBuf>,
    },
    /// Preview a command's file changes against a throwaway copy of the working
    /// tree (read-only system, no network via bubblewrap), then keep or discard.
    DryRun {
        /// The shell command to preview.
        command: String,
        /// Apply the previewed changes to the real working tree (default: discard).
        #[arg(long)]
        apply: bool,
    },
    /// Inspect or configure the environment context sent to the model.
    Context {
        /// Explain included/excluded sections, sources, and token estimates.
        #[arg(long)]
        explain: bool,
        /// Include a proposed request in the token/cost estimate (text is not echoed).
        #[arg(long, value_name = "TEXT")]
        preview: Option<String>,
        /// Emit stable metadata JSON; section contents are intentionally omitted.
        #[arg(long)]
        json: bool,
        /// Persistently exclude an optional section (repeatable).
        #[arg(long, value_name = "SECTION")]
        exclude: Vec<String>,
        /// Persistently include an optional section (repeatable).
        #[arg(long, value_name = "SECTION")]
        include: Vec<String>,
    },
    /// Generate a runnable script + markdown runbook from a recorded session.
    Runbook {
        /// The audit session id to export (default: the most recent session).
        #[arg(long)]
        session: Option<String>,
        /// Output directory for the runbook files (default: current directory).
        #[arg(short = 'o', long)]
        out: Option<String>,
        /// Re-run the recorded commands through the safety gate (not the model).
        #[arg(long)]
        replay: bool,
    },
}

#[derive(Subcommand, Debug)]
enum HistoryCmd {
    /// Find past commands by meaning, e.g. "the docker run with the volume mount".
    Search {
        /// The natural-language query (any number of words).
        query: Vec<String>,
        /// How many results to show.
        #[arg(short = 'n', long, default_value_t = 5)]
        limit: usize,
        /// Print only the matching command(s), no score column — for the recall
        /// key binding to pre-fill the line. Notices go to stderr; stdout stays
        /// empty when there's no match (or the feature is off).
        #[arg(long)]
        bare: bool,
    },
    /// (Re)build the semantic index from your shell-history log.
    Index {
        /// Re-embed everything from scratch instead of only new commands.
        #[arg(long)]
        rebuild: bool,
    },
}

#[derive(Subcommand, Debug)]
enum BackendCmd {
    /// Show the managed runtime and supervisor state.
    Status {
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install the exact OpenCode runtime supported by this Aishe build.
    Install {
        /// Use a previously downloaded archive instead of the configured mirror.
        #[arg(long, value_name = "PATH")]
        from: Option<std::path::PathBuf>,
        /// Replace an already verified runtime.
        #[arg(long)]
        force: bool,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify runtime identity, checksum metadata, and executable version.
    Verify {
        /// Also start the authenticated server and run a health probe.
        #[arg(long)]
        live: bool,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reinstall a missing or invalid managed runtime.
    Repair {
        /// Use a previously downloaded archive.
        #[arg(long, value_name = "PATH")]
        from: Option<std::path::PathBuf>,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Select the immediately previous verified runtime when compatible.
    Rollback,
    /// Gracefully stop the private backend supervisor.
    Stop,
    /// Print the private backend log tail.
    Logs {
        #[arg(long, default_value_t = 100)]
        tail: usize,
    },
    /// Remove inactive runtime staging/cache entries.
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
enum TaskSessionCmd {
    /// Show one task record.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Set a human-readable task name.
    Rename { id: String, name: String },
    /// Delete exactly one task record.
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
enum PriceCmd {
    /// List built-in matches and exact user price overrides.
    List,
    /// Set input/output USD per million tokens for an exact model ID.
    Set {
        model: String,
        #[arg(long)]
        input: f64,
        #[arg(long)]
        output: f64,
    },
    /// Remove an exact user price override.
    Remove { model: String },
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").red());
            ExitCode::from(1)
        }
    }
}

fn backend_command(command: &BackendCmd) -> Result<u8> {
    use aishe::backend::{InstallSource, RuntimeManager, RuntimeStatus};

    let manager = RuntimeManager::new()?;
    match command {
        BackendCmd::Status { json } => {
            let status = manager.status();
            let loaded = aishe::backend::control::load_state();
            let verified = aishe::backend::control::verified_state();
            let (supervisor, supervisor_text) = match verified {
                Ok(Some(state)) => {
                    let loopback = state.control_url.starts_with("http://127.0.0.1:")
                        && state.opencode_url.starts_with("http://127.0.0.1:");
                    (
                        serde_json::json!({
                            "state": "running",
                            "supervisor_pid": state.supervisor_pid,
                            "opencode_pid": state.opencode_pid,
                            "runtime_version": state.runtime_version,
                            "plugin_sha256": state.plugin_sha256,
                            "provider_id": state.provider_id,
                            "model_id": state.model_id,
                            "started_at_ms": state.started_at_ms,
                            "loopback": loopback
                        }),
                        format!(
                            "supervisor: running · pid {} · OpenCode pid {} · {}/{}",
                            state.supervisor_pid,
                            state.opencode_pid,
                            aishe::commands::display_safe(&state.provider_id),
                            aishe::commands::display_safe(&state.model_id)
                        ),
                    )
                }
                Ok(None) => match loaded {
                    Ok(Some(_)) => (
                        serde_json::json!({"state":"stale"}),
                        "supervisor: stale state (Doctor can repair it)".into(),
                    ),
                    Ok(None) => (
                        serde_json::json!({"state":"stopped"}),
                        "supervisor: stopped (starts on the next AI turn)".into(),
                    ),
                    Err(error) => {
                        let detail = aishe::redact::redact(&error.to_string());
                        (
                            serde_json::json!({"state":"invalid","detail":detail}),
                            format!("supervisor: invalid state · {detail}"),
                        )
                    }
                },
                Err(error) => {
                    let detail = aishe::redact::redact(&error.to_string());
                    (
                        serde_json::json!({"state":"invalid","detail":detail}),
                        format!("supervisor: invalid state · {detail}"),
                    )
                }
            };
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": 1,
                        "runtime": &status,
                        "supervisor": supervisor
                    }))?
                );
            } else {
                match &status {
                    RuntimeStatus::Ready {
                        version,
                        binary,
                        sha256,
                    } => {
                        println!("agent runtime: OpenCode {version} · ready");
                        println!("binary: {}", binary.display());
                        println!("sha256: {sha256}");
                    }
                    RuntimeStatus::Missing { expected_version } => {
                        println!("agent runtime: OpenCode {expected_version} · not installed");
                        println!("next: aishe backend install");
                    }
                    RuntimeStatus::Invalid {
                        expected_version,
                        reason,
                    } => {
                        println!("agent runtime: OpenCode {expected_version} · invalid");
                        println!("reason: {reason}");
                        println!("next: aishe backend repair");
                    }
                }
                println!("{supervisor_text}");
            }
            Ok(if matches!(status, RuntimeStatus::Ready { .. }) {
                0
            } else {
                1
            })
        }
        BackendCmd::Install { from, force, json } => {
            let source = from
                .clone()
                .map(InstallSource::Local)
                .unwrap_or(InstallSource::Default);
            let status = manager.install(source, *force)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else if let RuntimeStatus::Ready {
                version, binary, ..
            } = status
            {
                println!("✓ installed OpenCode {version}");
                println!("  {}", binary.display());
            }
            Ok(0)
        }
        BackendCmd::Verify { live, json } => {
            let status = manager.verify()?;
            if *live {
                // The supervisor health probe is intentionally distinct from
                // `--version`; until a provider is needed it starts with no key.
                aishe::backend::supervisor::smoke_test(&manager)?;
            }
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "runtime": status,
                        "live": live,
                    }))?
                );
            } else {
                println!(
                    "✓ managed runtime{} verified",
                    if *live { " and server" } else { "" }
                );
            }
            Ok(0)
        }
        BackendCmd::Repair { from, json } => {
            let source = from
                .clone()
                .map(InstallSource::Local)
                .unwrap_or(InstallSource::Default);
            let status = manager.install(source, true)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&status)?);
            } else {
                println!("✓ managed OpenCode runtime repaired");
            }
            Ok(0)
        }
        BackendCmd::Rollback => {
            let _ = aishe::backend::supervisor::request_stop();
            let status = manager.rollback()?;
            if let RuntimeStatus::Ready {
                version, sha256, ..
            } = status
            {
                println!("✓ rolled back to the prior verified OpenCode {version} install");
                println!("  sha256: {sha256}");
            }
            Ok(0)
        }
        BackendCmd::Stop => aishe::backend::supervisor::request_stop(),
        BackendCmd::Logs { tail } => {
            aishe::backend::supervisor::print_logs(*tail)?;
            Ok(0)
        }
        BackendCmd::Gc { dry_run } => {
            let removed = manager.garbage_collect(*dry_run)?;
            for path in &removed {
                println!(
                    "{} {}",
                    if *dry_run { "would remove" } else { "removed" },
                    path.display()
                );
            }
            if removed.is_empty() {
                println!("runtime cache is clean");
            }
            Ok(0)
        }
    }
}

fn uninstall_command(
    selection: aishe::uninstall::Selection,
    dry_run: bool,
    yes: bool,
) -> Result<u8> {
    let plan = aishe::uninstall::Plan::discover(selection)?;
    let existing = plan.existing_targets();
    println!(
        "{}",
        if dry_run {
            "Aishe uninstall preview"
        } else {
            "Aishe uninstall plan"
        }
    );
    println!("User state is preserved unless its category was explicitly selected.");
    let selected_categories = [
        (plan.selection.binary, "binary/completions/man"),
        (plan.selection.runtime, "managed runtime/cache"),
        (plan.selection.sessions, "AI sessions/tool journals"),
        (plan.selection.config, "config/credentials"),
        (plan.selection.history, "shell history"),
        (plan.selection.audit_undo, "audit/undo data"),
    ]
    .into_iter()
    .filter_map(|(selected, name)| selected.then_some(name))
    .collect::<Vec<_>>();
    println!("Selected: {}", selected_categories.join(", "));
    if existing.is_empty() {
        println!("Nothing selected is currently installed.");
        return Ok(0);
    }
    let mut previous = "";
    for target in &existing {
        if target.category != previous {
            println!("\n{}:", target.category);
            previous = target.category;
        }
        println!(
            "  {} {}",
            if target.recoverable {
                "replaceable"
            } else {
                "permanent"
            },
            target.path.display()
        );
    }
    if dry_run {
        println!("\nNo files were changed.");
        return Ok(0);
    }

    if !yes {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "confirmation required; review `aishe uninstall --dry-run` and rerun with --yes"
            );
        }
        use std::io::Write;
        if plan.selection.includes_user_state() {
            print!(
                "\nSelected user state cannot be recovered by Aishe. Type `delete` to continue: "
            );
        } else {
            print!("\nRemove the selected replaceable components? [y/N] ");
        }
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let confirmed = if plan.selection.includes_user_state() {
            answer.trim() == "delete"
        } else {
            matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        };
        if !confirmed {
            println!("Cancelled; no files were changed.");
            return Ok(2);
        }
    }

    let removed = plan.apply()?;
    println!("\nRemoved {} selected path(s).", removed.len());
    if plan.selection.includes_user_state() {
        println!("Selected user state was permanently removed and is not recoverable by Aishe.");
    } else {
        println!("Config, credentials, history, sessions, audit, and undo data were preserved.");
    }
    Ok(0)
}

fn run() -> Result<u8> {
    let args = Args::parse();

    if matches!(args.cmd, Some(Cmd::BackendSupervisor)) {
        return aishe::backend::supervisor::run_supervisor();
    }

    // Ordinary one-shot shell commands must remain ordinary shell commands:
    // prove the route before loading config/policy/providers/plugins or touching
    // the managed backend. Ambiguous input falls through to the full classifier.
    if let Some(command) = args
        .command
        .as_deref()
        .and_then(dispatcher::fast_shell_line)
    {
        let mut executor = Executor::new()?;
        executor.set_history_log(fast_history_log()?);
        return Ok(executor.run(&command) as u8);
    }

    // Setup is deliberately handled before ordinary config loading: its job is
    // to create, repair, or verify the config without invoking a legacy wizard.
    if let Some(Cmd::Setup(setup)) = &args.cmd {
        let outcome = match aishe::setup::run(aishe::setup::Options {
            resume: setup.resume,
            restart: setup.restart,
            verify_only: setup.verify,
            non_interactive: setup.non_interactive,
            service: setup.service.clone(),
            base_url: setup.base_url.clone(),
            key_env: setup.key_env.clone(),
            credential_profile: setup.credential_profile.clone(),
            model: setup.model.clone(),
            transport: setup.transport.clone(),
            profile: setup
                .profile
                .as_deref()
                .and_then(aishe::profiles::Profile::parse),
            input_price: setup.input_price,
            output_price: setup.output_price,
            live: setup.live,
            backend: setup.backend.clone(),
            install_backend: setup.install_backend,
            runtime_file: setup.runtime_file.clone(),
            runtime_base_url: setup.runtime_base_url.clone(),
            sandbox: setup.sandbox.clone(),
            install_system_deps: setup.install_system_deps,
            default_scope: setup.default_scope.clone(),
            network: setup.network.clone(),
            output: setup.output.clone(),
            json: setup.json,
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                let code = aishe::setup::exit_code(&error);
                let message = aishe::redact::redact(&error.to_string());
                if setup.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "schema_version": 1,
                            "applied": false,
                            "exit_code": code,
                            "error": message,
                        }))?
                    );
                } else {
                    eprintln!("aishe setup: {message}");
                }
                return Ok(code);
            }
        };
        return Ok(outcome.exit_code);
    }

    if let Some(Cmd::Settings { json }) = &args.cmd {
        if *json {
            let (_, provenance) = aishe::settings::provenance()?;
            println!("{}", serde_json::to_string_pretty(&provenance)?);
        } else {
            aishe::settings::run()?;
        }
        return Ok(0);
    }

    // Credential management deliberately uses only user config. A project
    // overlay can never redirect a write to a different saved profile.
    if let Some(Cmd::Auth { cmd }) = &args.cmd {
        return aishe::auth::run(cmd);
    }

    if let Some(Cmd::Tour {
        restart,
        non_interactive,
    }) = &args.cmd
    {
        aishe::tour::run(aishe::tour::Options {
            restart: *restart,
            non_interactive: *non_interactive,
        })?;
        return Ok(0);
    }

    // `doctor` inspects the environment without loading/initializing config.
    if let Some(Cmd::Doctor {
        probe,
        live,
        json,
        fix,
        bundle,
    }) = &args.cmd
    {
        let report = aishe::diagnostics::inspect(
            VERSION,
            &aishe::diagnostics::Options {
                probe: *probe || *live,
                live: *live,
                fix: *fix,
            },
        );
        if let Some(path) = bundle {
            let config = Config::load_quiet().ok().flatten();
            aishe::diagnostics::write_bundle(path, &report, config.as_ref())?;
            if !*json {
                eprintln!("aishe: wrote redacted support bundle to {}", path.display());
            }
        }
        if *json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", aishe::diagnostics::render_text(&report));
        }
        return Ok(if report.critical_ok() { 0 } else { 1 });
    }

    if let Some(Cmd::Backend { cmd }) = &args.cmd {
        return backend_command(cmd);
    }

    // `completions <shell>` prints a completion script and exits.
    if let Some(Cmd::Completions { shell }) = args.cmd {
        use clap::CommandFactory;
        clap_complete::generate(shell, &mut Args::command(), "aishe", &mut std::io::stdout());
        return Ok(0);
    }

    // `man` prints a roff man page generated from the clap command tree.
    if matches!(args.cmd, Some(Cmd::Man)) {
        use clap::CommandFactory;
        let man = clap_mangen::Man::new(Args::command());
        let mut out = Vec::new();
        man.render(&mut out).ok();
        use std::io::Write;
        let _ = std::io::stdout().write_all(&out);
        return Ok(0);
    }

    if let Some(Cmd::Uninstall {
        binary,
        runtime,
        sessions,
        config,
        history,
        audit_undo,
        all,
        dry_run,
        yes,
    }) = &args.cmd
    {
        return uninstall_command(
            aishe::uninstall::Selection {
                binary: *binary || *all,
                runtime: *runtime || *all,
                sessions: *sessions || *all,
                config: *config || *all,
                history: *history || *all,
                audit_undo: *audit_undo || *all,
            },
            *dry_run,
            *yes,
        );
    }

    // `trust` / `untrust` manage the project-config trust store; no config load.
    if let Some(Cmd::Trust { list, path }) = &args.cmd {
        return Ok(trust_command(*list, path.as_deref()));
    }
    if let Some(Cmd::Untrust { all, path }) = &args.cmd {
        return Ok(untrust_command(*all, path.as_deref()));
    }

    if let Some(Cmd::Sessions { json }) = &args.cmd {
        return Ok(tasks_list_command(*json));
    }
    if let Some(Cmd::Session { cmd }) = &args.cmd {
        return task_session_command(cmd);
    }

    // `undo` reverts AI file changes from the journal; no config or provider.
    if let Some(Cmd::Undo { list }) = &args.cmd {
        return Ok(undo_command(*list));
    }

    // `init <shell>` needs no config or provider.
    if let Some(Cmd::Init { shell }) = &args.cmd {
        return match integration::script(shell) {
            Some(s) => {
                print!("{s}");
                Ok(0)
            }
            None => {
                eprintln!(
                    "aishe: no integration for '{shell}' (supported: {})",
                    integration::SUPPORTED.join(", ")
                );
                Ok(1)
            }
        };
    }

    // Audit inspection/export is useful for recovery and support even before
    // provider setup. Load existing pricing/log preferences when available,
    // but never launch setup or materialize a default config as a side effect.
    if matches!(
        args.cmd,
        Some(Cmd::Log { .. } | Cmd::Usage { .. } | Cmd::Runbook { .. })
    ) {
        let mut config = Config::load_quiet()?.unwrap_or_default();
        let _project_overlay = std::env::current_dir()
            .ok()
            .and_then(|cwd| config.apply_project_overlay(&cwd));
        config.apply_overrides(
            args.mode.as_deref(),
            args.provider.as_deref(),
            args.model.as_deref(),
        );
        return match &args.cmd {
            Some(Cmd::Log {
                session,
                action,
                model,
                since,
                limit,
                json,
            }) => Ok(log_command(
                &config,
                session.as_deref(),
                action.as_deref(),
                model.as_deref(),
                since.as_deref(),
                *limit,
                *json,
            )),
            Some(Cmd::Usage { by, since }) => Ok(usage_history_command(
                &config,
                by.as_deref(),
                since.as_deref(),
            )),
            Some(Cmd::Runbook {
                session,
                out,
                replay,
            }) => runbook_command(&config, session.as_deref(), out.as_deref(), *replay),
            _ => unreachable!("matched audit command"),
        };
    }

    let mut config = Config::load_or_init()?;
    // A project-local `.aishe/config.toml` overrides the user config (safe keys
    // always; sensitive keys only when the file is trusted). Applied before flags
    // so precedence is: CLI flags > project overlay > user config > defaults.
    let project_overlay = std::env::current_dir()
        .ok()
        .and_then(|cwd| config.apply_project_overlay(&cwd));
    // CLI flags win over the config file (which wins over compiled defaults).
    config.apply_overrides(
        args.mode.as_deref(),
        args.provider.as_deref(),
        args.model.as_deref(),
    );
    // Administrator policy is the final, read-only constraint layer. It can
    // reduce authority or reject a provider/model, never inject credentials.
    aishe::policy::constrain(&mut config)?;
    if args.accept_yolo {
        init_audit(&config);
        return match ensure_yolo_acceptance(&config) {
            Ok(()) => Ok(0),
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                Ok(1)
            }
        };
    }

    // First-class inspection / settings subcommands. They print (or persist a
    // setting) and exit, so they work the same in the zsh-PTY, a bare shell, or a
    // script. The inspectors show the *effective* config (project overlay + flags
    // applied); the setters write to the user config file (see `set_or_show`).
    match &args.cmd {
        Some(Cmd::Config { effective, json }) => {
            if *effective {
                let (effective_config, provenance) = aishe::settings::provenance()?;
                if *json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "config": effective_config,
                            "provenance": provenance,
                        }))?
                    );
                } else {
                    aishe::settings::print_provenance(&provenance);
                }
            } else if *json {
                println!("{}", serde_json::to_string_pretty(&config)?);
            } else {
                println!("config file: {}", Config::path().display());
                match toml::to_string_pretty(&config) {
                    Ok(t) => println!("\n{t}"),
                    Err(e) => eprintln!("aishe: {e}"),
                }
            }
            return Ok(0);
        }
        Some(Cmd::Mcp) => {
            print_mcp_listing(&aishe::mcp::McpRegistry::connect(&config.mcp_servers));
            return Ok(0);
        }
        Some(Cmd::Commands) => {
            let commands = CommandRegistry::load();
            if commands.is_empty() {
                println!(
                    "no custom commands (add *.md files to {})",
                    aishe::commands::user_dir().unwrap_or_default().display()
                );
            } else {
                println!("custom slash-commands:");
                for (name, desc) in commands.list() {
                    println!("\x20 /{name}  —  {desc}");
                }
            }
            return Ok(0);
        }
        Some(Cmd::Skills) => {
            let skills = SkillRegistry::load();
            if skills.is_empty() {
                println!(
                    "no skills (add <name>/SKILL.md files to {})",
                    aishe::skills::user_dir().unwrap_or_default().display()
                );
            } else {
                println!("model-invoked skills (yolo mode):");
                for (name, desc) in skills.list() {
                    println!("\x20 {name}  —  {desc}");
                }
            }
            warn_untrusted_skills(&skills);
            return Ok(0);
        }
        Some(Cmd::Mode { value }) => return Ok(set_or_show("mode", value.as_deref(), &config)),
        Some(Cmd::Scope { value }) => return Ok(set_or_show("scope", value.as_deref(), &config)),
        Some(Cmd::Network { value }) => {
            return Ok(set_or_show("network", value.as_deref(), &config))
        }
        Some(Cmd::Model { value }) => return Ok(set_or_show("model", value.as_deref(), &config)),
        Some(Cmd::Provider { value, live, json }) => {
            if value.as_deref() == Some("test") {
                let report = aishe::capabilities::validate(&config, *live);
                if *json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    print_capability_report(&report);
                }
                return Ok(
                    if report.credential.state == aishe::capabilities::State::Fail {
                        1
                    } else {
                        0
                    },
                );
            }
            if *live || *json {
                eprintln!("aishe: --live/--json require `aishe provider test`");
                return Ok(1);
            }
            return Ok(set_or_show("provider", value.as_deref(), &config));
        }
        Some(Cmd::Models {
            provider,
            refresh,
            json,
        }) => {
            if *refresh {
                let _ = aishe::capabilities::clear();
            }
            return Ok(models_command(
                &config,
                provider.as_deref().unwrap_or(&config.aishe.provider),
                *json,
            ));
        }
        Some(Cmd::Profile { value }) => {
            return Ok(profile_command(&config, value.as_deref()));
        }
        Some(Cmd::Readiness { json }) => {
            let report = aishe::profiles::readiness(&config);
            if *json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "autonomous readiness: {}",
                    if report.ready { "ready" } else { "not ready" }
                );
                for check in report.checks {
                    println!(
                        "  {} {}: {}",
                        if check.ready {
                            "✓"
                        } else if check.required {
                            "✗"
                        } else {
                            "!"
                        },
                        check.id,
                        check.detail
                    );
                }
            }
            return Ok(if report.ready { 0 } else { 1 });
        }
        Some(Cmd::Price { cmd }) => return Ok(price_command(&config, cmd)),
        Some(Cmd::Resume { id, cwd }) => {
            return resume_task_command(&config, id.as_deref(), cwd.as_deref())
        }
        Some(Cmd::Context {
            explain,
            preview,
            json,
            exclude,
            include,
        }) => {
            return context_command(
                config,
                *explain,
                preview.as_deref(),
                *json,
                exclude,
                include,
            );
        }
        Some(Cmd::History { cmd }) => {
            return history_command(&config, cmd);
        }
        Some(Cmd::DryRun { command, apply }) => {
            return dry_run_command(command, *apply);
        }
        Some(
            Cmd::Setup(_)
            | Cmd::Settings { .. }
            | Cmd::Tour { .. }
            | Cmd::Sessions { .. }
            | Cmd::Session { .. }
            | Cmd::Log { .. }
            | Cmd::Usage { .. }
            | Cmd::Runbook { .. }
            | Cmd::Backend { .. }
            | Cmd::BackendSupervisor,
        ) => {
            unreachable!("handled before config load")
        }
        _ => {}
    }

    // Tell an interactive user what a project config did (and how to trust it).
    let interactive_entry = args.command.is_none()
        && args.suggest_line.is_none()
        && args.yolo_line.is_none()
        && args.auto_line.is_none()
        && args.fix_line.is_none()
        && !args.accept_yolo
        && std::io::stdin().is_terminal();
    if interactive_entry {
        notify_project_overlay(&project_overlay);
    }

    // Initialize the audit log (off unless enabled in config or via $AISHE_LOG).
    init_audit(&config);

    // Non-interactive invocations (`-c` and the shell-hook helpers) never use
    // the PTY front-end — they need the in-process executor/provider, not a
    // wrapped interactive zsh.
    let non_interactive = args.command.is_some()
        || args.suggest_line.is_some()
        || args.yolo_line.is_some()
        || args.auto_line.is_some()
        || args.fix_line.is_some()
        || args.accept_yolo
        || matches!(args.cmd, Some(Cmd::Suggest { .. }));

    // The interactive shell is the zsh-PTY front-end: it drives the user's real
    // zsh, with the AI injected via a command_not_found hook, so zsh is required.
    // Piped (non-tty) stdin with no `-c`: read commands from stdin instead of
    // launching the interactive shell. An explicit `aishe zsh` always launches it.
    let explicit_zsh = matches!(args.cmd, Some(Cmd::Zsh));
    let piped_stdin = !non_interactive && !explicit_zsh && !std::io::stdin().is_terminal();
    let want_pty = !non_interactive && !piped_stdin;

    if want_pty {
        if aishe::executor::which("zsh").is_none() {
            eprintln!(
                "{}",
                "aishe: the interactive shell needs zsh, which isn't on your PATH.".red()
            );
            eprintln!(
                "  Install zsh, then run aishe again:\n  \
                   apt install zsh  |  dnf install zsh  |  brew install zsh  |  apk add zsh\n  \
                 (the install.sh script also installs zsh for you.)\n  \
                 Without zsh you can still use aishe non-interactively (`aishe -c …`)\n  \
                 or as a hook in your shell: add  eval \"$(aishe init bash)\"  to ~/.bashrc."
            );
            return Ok(1);
        }
        return aishe::pty::run_zsh(&config, &history_paths(&config).1);
    }

    let mut executor = Executor::new()?;
    context::init(executor.shell());
    // The `history` builtin reads the timestamped log (also available in `-c`).
    executor.set_history_log(history_paths(&config).1);

    let cache = CommandCache::new();
    cache.build(executor.shell());

    // Build the provider, but keep the shell fully usable without it. We do NOT
    // warn here: local commands shouldn't print LLM noise, and the NL paths
    // (REPL, -c, hooks) each report a missing provider at the point of use.
    let mut provider: Option<Arc<dyn Provider>> = providers::make(&config).ok();

    // Install a non-fatal SIGINT handler (see INTERRUPTED docs).
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_sigint as *const () as libc::sighandler_t,
        );
    }

    // User-defined slash-commands and model-invoked skills (plugins).
    let commands = CommandRegistry::load();
    let skills = aishe::skills::SkillRegistry::load();
    // Deliberately NOT warning about untrusted project skills here: this runs
    // for every invocation, so it printed on plain shell pass-through
    // (`aishe -c 'free -m'`) in any repo carrying a skill file, polluting
    // stderr for commands that never consult a skill. The warning belongs where
    // skills are actually relevant — `aishe skills`, `aishe doctor`, and the
    // yolo loop that can invoke them.
    // MCP servers (extra yolo tools). Empty/instant unless `[mcp_servers]` is set.
    let mcp = aishe::mcp::McpRegistry::connect(&config.mcp_servers);

    // Public scripting interface: `aishe suggest "<nl>" [--json]`.
    if let Some(Cmd::Suggest { query, json }) = &args.cmd {
        let q = query.join(" ");
        let code = suggest_command(&q, *json, &mut executor, provider.as_deref(), &config)?;
        record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }

    // Shell-hook helpers (called by `aishe init` integration). Each is its own
    // process under the interactive PTY, so after it runs we append its metered
    // usage to the shared session tally (a no-op outside a PTY session) for the
    // one-line summary the PTY prints on exit.
    if let Some(line) = args.suggest_line {
        let code = suggest_line(&line, &mut executor, provider.as_deref(), &config)?;
        record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(line) = args.yolo_line {
        let code = yolo_line(
            &line,
            &mut executor,
            provider.as_deref(),
            &config,
            &skills,
            &mcp,
        )?;
        record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(line) = args.auto_line {
        let code = auto_line(&line, &mut executor, provider.as_deref(), &config)?;
        record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(cmd) = args.fix_line {
        let code = fix_line(&cmd, &mut executor, provider.as_deref(), &config)?;
        record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }

    // Non-interactive single-shot mode (-c).
    if let Some(input) = args.command {
        return one_shot(
            &input,
            &mut executor,
            &mut provider,
            &config,
            &cache,
            &commands,
            &skills,
            &mcp,
        );
    }

    // Pipe/script mode: run each line of piped stdin like a `-c` invocation.
    if piped_stdin {
        let mut last = 0u8;
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    last = one_shot(
                        trimmed,
                        &mut executor,
                        &mut provider,
                        &config,
                        &cache,
                        &commands,
                        &skills,
                        &mcp,
                    )?;
                }
                Err(_) => break,
            }
        }
        return Ok(last);
    }

    // Every interactive session is handled by the zsh-PTY branch above, and every
    // non-interactive path (hooks, `-c`, piped stdin) returns before here.
    Ok(0)
}

/// Shell-hook helper: print a suggested command to stdout (for `print -z` /
/// readline pre-fill) and any explanation/answer to stderr.
/// Persistent conversation-memory file for the shell-hook front-ends. Each NL
/// call is a separate process, so memory is shared via this file (set and
/// exported by the injected hook as `AISHE_SESSION_FILE`, keyed by the shell's
/// PID). `None` when memory is off or the hook did not set it.
fn hook_session_path(config: &Config) -> Option<std::path::PathBuf> {
    if !config.aishe.memory {
        return None;
    }
    std::env::var("AISHE_SESSION_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(std::path::PathBuf::from)
}

/// Hand a native pre-admission fallback command back to the live shell without
/// mixing the command protocol into the managed agent's terminal output.
///
/// The integration passes `AISHE_PENDING_FILE` only to hidden hook invocations.
/// Direct `aishe --auto-line` callers keep the historical stdout/exit-code
/// contract when the variable is absent.
fn stage_hook_command(action: &str, command: &str) -> Result<bool> {
    let Some(path) = std::env::var_os("AISHE_PENDING_FILE")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return Ok(false);
    };
    write_hook_command(&path, action, command)?;
    Ok(true)
}

fn write_hook_command(path: &std::path::Path, action: &str, command: &str) -> Result<()> {
    use std::io::Write;

    if path.exists() {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("inspecting shell handoff {}", path.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1_048_576 {
            anyhow::bail!("AISHE_PENDING_FILE is not a bounded regular file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } {
                anyhow::bail!("AISHE_PENDING_FILE is not owned by the current user");
            }
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("opening shell handoff {}", path.display()))?;
    writeln!(file, "{action}")?;
    writeln!(file, "{command}")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Whether `cmd` parses as valid shell syntax. Used so the hook front-ends can
/// silently tell a runnable command from a question answered as prose: a model
/// "command" that does not parse (e.g. a sentence, or a malformed redirect) is
/// treated as an answer instead of being printed for the shell to eval/pre-fill.
/// Permissive on spawn failure (returns true) so a missing shell never blocks.
fn shell_syntax_ok(executor: &Executor, cmd: &str) -> bool {
    std::process::Command::new(executor.shell())
        .arg("-nc")
        .arg(cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(true)
}

/// Explain why model-backed features are unavailable without hiding the useful
/// provider detail. Provider construction is intentionally non-fatal so local
/// shell commands still work; when the active provider's key is absent, name
/// the exact environment variable at the point an LLM feature is requested.
fn print_llm_unavailable(config: &Config) {
    let provider = match config.aishe.provider.as_str() {
        "anthropic" => Some(&config.providers.anthropic),
        "openai" => Some(&config.providers.openai),
        _ => None,
    };
    if let Some(provider) = provider {
        let profile = provider.credential_profile();
        match aishe::credentials::resolve(provider) {
            Ok(resolved) if !resolved.source.is_available() => {
                eprintln!(
                    "aishe: API key missing for credential profile '{profile}' — \
                     run `aishe auth set {profile}` (or set ${} for an override)",
                    provider.api_key_env
                );
                return;
            }
            Err(error) => {
                eprintln!(
                    "aishe: credential store unavailable — {}",
                    aishe::redact::redact(&error.to_string())
                );
                return;
            }
            _ => {}
        }
    }
    eprintln!("aishe: LLM not configured — run `aishe doctor` for details");
}

static YOLO_WORKSPACE_ACCEPTED: AtomicBool = AtomicBool::new(false);
static YOLO_HOST_ACCEPTED: AtomicBool = AtomicBool::new(false);

fn ensure_yolo_acceptance(config: &Config) -> Result<()> {
    use aishe::agent::ExecutionScope;
    use std::io::Write;

    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("backend.default_scope must be workspace or host")?;
    let in_process = match scope {
        ExecutionScope::Workspace => &YOLO_WORKSPACE_ACCEPTED,
        ExecutionScope::Host => &YOLO_HOST_ACCEPTED,
    };
    if in_process.load(Ordering::SeqCst) || acceptance_file_contains(scope) {
        in_process.store(true, Ordering::SeqCst);
        return Ok(());
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "yolo {:?} requires one interactive acceptance in each Aishe shell",
            scope
        );
    }

    let workspace = std::env::current_dir()
        .context("resolving yolo workspace")
        .and_then(|path| {
            aishe::backend::opencode::session::SessionStore::resolve_workspace(&path)
        })?;
    println!();
    match scope {
        ExecutionScope::Workspace => {
            #[cfg(target_os = "linux")]
            let sandbox = match aishe::dependencies::bubblewrap_probe() {
                aishe::dependencies::BubblewrapState::Usable { .. } => "bubblewrap verified",
                state => {
                    anyhow::bail!(
                        "yolo workspace requires functional bubblewrap; current state: {state:?}. \
                         Install it with your system package manager or rerun `aishe setup`"
                    )
                }
            };
            #[cfg(not(target_os = "linux"))]
            let sandbox = "policy checks only — no supported OS sandbox";
            println!("{}", "Enter yolo · workspace?".yellow().bold());
            println!();
            println!(
                "The agent may run commands and change files without asking again in this shell session."
            );
            println!("Actions are confined to:");
            println!("  {}", workspace.display());
            println!("Network: {}", config.backend.workspace_network);
            println!("Sandbox: {sandbox}");
            #[cfg(target_os = "macos")]
            println!(
                "{}",
                "Warning: macOS workspace mode is not kernel-isolated.".yellow()
            );
            print!("\nType yolo to continue: ");
        }
        ExecutionScope::Host => {
            if !config.sandbox.allow_host_yolo {
                anyhow::bail!("yolo host scope is disabled by policy");
            }
            println!("{}", "Enter yolo · host?".red().bold());
            println!();
            println!(
                "The agent may execute any command available to your user, use sudo, modify system files, access the network, and make irreversible changes without asking again in this shell session."
            );
            #[cfg(target_os = "macos")]
            println!(
                "{}",
                "Warning: actions execute without a supported OS sandbox.".yellow()
            );
            print!("\nType yolo-host to continue: ");
        }
    }
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading yolo acceptance")?;
    let expected = match scope {
        ExecutionScope::Workspace => "yolo",
        ExecutionScope::Host => "yolo-host",
    };
    if answer.trim() != expected {
        anyhow::bail!("yolo scope was not accepted");
    }
    persist_acceptance(scope)?;
    in_process.store(true, Ordering::SeqCst);
    aishe::audit::action(
        "agent:yolo_accept",
        &format!("scope={scope:?} workspace={}", workspace.display()),
        Some(0),
    );
    Ok(())
}

fn acceptance_file_contains(scope: aishe::agent::ExecutionScope) -> bool {
    let Ok(Some(path)) = acceptance_path() else {
        return false;
    };
    let Ok(metadata) = std::fs::symlink_metadata(&path) else {
        return false;
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1024 {
        return false;
    }
    let marker = match scope {
        aishe::agent::ExecutionScope::Workspace => "workspace",
        aishe::agent::ExecutionScope::Host => "host",
    };
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| text.lines().any(|line| line == marker))
}

fn persist_acceptance(scope: aishe::agent::ExecutionScope) -> Result<()> {
    use std::io::Write;

    let Some(path) = acceptance_path()? else {
        // Direct single-process invocations need no durable marker.
        return Ok(());
    };
    if path.exists() {
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1024 {
            anyhow::bail!("AISHE_ACCEPTANCE_FILE is not a bounded regular file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.uid() != unsafe { libc::geteuid() }
                || metadata.permissions().mode() & 0o077 != 0
            {
                anyhow::bail!("AISHE_ACCEPTANCE_FILE has unsafe ownership or permissions");
            }
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(&path)
        .with_context(|| format!("opening per-shell acceptance {}", path.display()))?;
    let marker = match scope {
        aishe::agent::ExecutionScope::Workspace => "workspace",
        aishe::agent::ExecutionScope::Host => "host",
    };
    writeln!(file, "{marker}")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn acceptance_path() -> Result<Option<std::path::PathBuf>> {
    let Some(path) = std::env::var_os("AISHE_ACCEPTANCE_FILE")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return Ok(None);
    };
    let shell_id =
        std::env::var("AISHE_SHELL_ID").context("AISHE_ACCEPTANCE_FILE requires AISHE_SHELL_ID")?;
    if !(16..=128).contains(&shell_id.len())
        || !shell_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("AISHE_SHELL_ID is invalid");
    }
    let expected = format!("aishe-yolo-accept-{shell_id}");
    if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
        anyhow::bail!("AISHE_ACCEPTANCE_FILE does not match this shell identity");
    }
    let expected_parent = std::env::temp_dir()
        .canonicalize()
        .context("resolving the temporary directory")?;
    let parent = path
        .parent()
        .context("AISHE_ACCEPTANCE_FILE has no parent")?
        .canonicalize()
        .context("resolving AISHE_ACCEPTANCE_FILE parent")?;
    if parent != expected_parent {
        anyhow::bail!("AISHE_ACCEPTANCE_FILE must be in the private shell temporary directory");
    }
    Ok(Some(path))
}

/// Try the managed agent engine. `Ok(None)` is the only compatibility-fallback
/// signal and is returned solely before prompt admission.
fn managed_turn(
    config: &Config,
    prompt: &str,
    mode: AgentMode,
    render: bool,
) -> Result<Option<TurnOutcome>> {
    if config.backend.engine != "opencode" {
        return Ok(None);
    }
    if mode == AgentMode::Yolo {
        ensure_yolo_acceptance(config)?;
    }
    let options = TurnOptions::from_config(config, mode, render)?;
    match aishe::agent::controller::run_turn(config, prompt, options) {
        Ok(outcome) => {
            record_managed_usage(&outcome, config);
            Ok(Some(outcome))
        }
        Err(TurnFailure::PreAdmission(error))
            if config.backend.fallback == "native" && mode != AgentMode::Yolo =>
        {
            eprintln!(
                "{}",
                "aishe: agent engine unavailable; using native fallback".yellow()
            );
            aishe::audit::action(
                "backend:fallback",
                &aishe::redact::redact(&error.to_string()),
                None,
            );
            Ok(None)
        }
        Err(error) => {
            let admitted = error.admitted();
            let detail = aishe::redact::redact(&error.into_error().to_string());
            if admitted {
                anyhow::bail!(
                    "managed agent turn failed after admission; it was not retried: {detail}"
                );
            }
            anyhow::bail!("managed agent engine unavailable: {detail} (run `aishe backend status`)")
        }
    }
}

fn record_managed_usage(outcome: &TurnOutcome, config: &Config) {
    let usage = aishe::usage::Usage {
        input: outcome.usage.input_tokens,
        output: outcome.usage.output_tokens,
        requests: outcome
            .events
            .iter()
            .filter(|event| matches!(event, aishe::agent::AgentEvent::Usage { .. }))
            .count() as u64,
    };
    if usage.is_empty() {
        return;
    }
    let Ok(path) = std::env::var("AISHE_USAGE_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    aishe::usagelog::append(std::path::Path::new(&path), usage, config.active_model());
    if let Ok(status_path) = std::env::var("AISHE_STATUS_FILE") {
        if !status_path.is_empty() {
            aishe::usagelog::write_status(
                std::path::Path::new(&status_path),
                std::path::Path::new(&path),
                &config.pricing,
                Some((usage, config.active_model())),
                &config.aishe.status_line_items,
            );
            let task = outcome
                .session_id
                .strip_prefix("ses_")
                .unwrap_or(&outcome.session_id);
            let task = task.chars().take(12).collect::<String>();
            let context_tokens = outcome.events.iter().rev().find_map(|event| match event {
                aishe::agent::AgentEvent::Usage { usage } => Some(usage.input_tokens),
                _ => None,
            });
            let mut metadata = vec![
                ("task", format!("task {task}")),
                (
                    "elapsed",
                    format!("last {:.1}s", outcome.elapsed_ms as f64 / 1000.0),
                ),
            ];
            if let Some(tokens) = context_tokens {
                metadata.push((
                    "context",
                    format!("context {} tok", aishe::usage::group(tokens)),
                ));
            }
            aishe::usagelog::merge_status(std::path::Path::new(&status_path), &metadata);
        }
    }
}

fn suggest_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if config.backend.engine == "opencode" {
        arm_hook_budget(config);
        let managed = managed_turn(config, line, AgentMode::Suggest, false);
        cancel_hook_budget();
        match managed {
            Ok(Some(outcome)) => {
                let suggestion = modes::suggest::parse_suggestion(&outcome.text);
                emit_suggest_hook(&suggestion, executor);
                return Ok(0);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                return Ok(1);
            }
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(1);
    };
    // Bound the blocking LLM call so a dead/slow network can't freeze the prompt.
    arm_hook_budget(config);
    // Hook calls are one per process; share memory across them via a file so
    // follow-ups ("is it enabled?") keep the prior turns' context.
    let mem = hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    let suggestion = modes::suggest::request(line, p, executor, config, session.history())?;
    // The blocking network work is done; let the rest run unbounded.
    cancel_hook_budget();
    let reply = emit_suggest_hook(&suggestion, executor);
    if let Some(path) = &mem {
        session.record_user(line);
        session.record_assistant(&reply);
        session.save_persisted(path);
    }
    Ok(0)
}

fn emit_suggest_hook(suggestion: &modes::suggest::Suggestion, executor: &Executor) -> String {
    match suggestion {
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } if shell_syntax_ok(executor, command) => {
            if !explanation.is_empty() {
                eprintln!("{}", explanation.as_str().dim());
            }
            println!("{command}");
            if explanation.is_empty() {
                command.clone()
            } else {
                format!("{command}\n{explanation}")
            }
        }
        // A "command" that is not valid shell is really an answer (prose). Show it
        // on stderr; print nothing to stdout so the hook neither evals nor
        // pre-fills it.
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            let answer = if explanation.is_empty() {
                command.clone()
            } else {
                explanation.clone()
            };
            if !answer.is_empty() {
                eprintln!("{answer}");
            }
            answer
        }
        modes::suggest::Suggestion::Answer { explanation } => {
            // No command to run; render the answer to stderr so the shell hook's
            // stdout capture stays empty.
            if !explanation.is_empty() {
                eprintln!("{explanation}");
            }
            explanation.clone()
        }
    }
}

/// Shell-hook helper for the fix-the-last-command key: given the failed command
/// (and `$AISHE_LAST_EXIT`), ask the model for a corrected command and print it
/// for the widget to pre-fill. With `fix_capture_stderr`, a read-only safe
/// command is re-run once to capture its error output for a better fix.
fn fix_line(
    cmd: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let exit = std::env::var("AISHE_LAST_EXIT").unwrap_or_else(|_| "unknown".to_string());
    let ctx = aishe::fix::error_context(cmd, config.aishe.fix_capture_stderr);
    let prompt = aishe::fix::build_prompt(cmd, &exit, ctx.as_deref());

    if config.backend.engine == "opencode" {
        arm_hook_budget(config);
        let managed = managed_turn(config, &prompt, AgentMode::Suggest, false);
        cancel_hook_budget();
        match managed {
            Ok(Some(outcome)) => {
                emit_suggest_hook(&modes::suggest::parse_suggestion(&outcome.text), executor);
                return Ok(0);
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                return Ok(1);
            }
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(1);
    };
    arm_hook_budget(config);
    let suggestion = modes::suggest::request(&prompt, p, executor, config, Vec::new())?;
    cancel_hook_budget();
    match suggestion {
        // A runnable corrected command: print to stdout for the widget to pre-fill.
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } if shell_syntax_ok(executor, &command) => {
            if !explanation.is_empty() {
                eprintln!("{}", explanation.as_str().dim());
            }
            println!("{command}");
        }
        // Prose (not valid shell) — show it on stderr, print nothing to stdout so
        // the widget doesn't pre-fill a non-command.
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            let answer = if explanation.is_empty() {
                command
            } else {
                explanation
            };
            if !answer.is_empty() {
                eprintln!("{answer}");
            }
        }
        modes::suggest::Suggestion::Answer { explanation } => {
            if !explanation.is_empty() {
                eprintln!("{explanation}");
            }
        }
    }
    Ok(0)
}

/// Shell-hook helper: run the yolo loop directly for a natural-language line.
fn yolo_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &aishe::mcp::McpRegistry,
) -> Result<u8> {
    if config.backend.engine == "opencode" {
        match managed_turn(config, line, AgentMode::Yolo, true) {
            Ok(Some(_)) => return Ok(0),
            Ok(None) => {}
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                return Ok(1);
            }
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(1);
    };
    let mem = hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    modes::yolo::run(
        line,
        p,
        executor,
        config,
        &INTERRUPTED,
        skills,
        mcp,
        &mut session,
    )?;
    if let Some(path) = &mem {
        session.save_persisted(path);
    }
    Ok(0)
}

/// Shell-hook helper for `auto` mode: get a suggestion and either stage it for
/// the live shell (hook invocation) or print it with the legacy exit-code
/// contract (direct invocation).
///
/// - Answer (no command): nothing on stdout, exit 0.
/// - Safe command: command on stdout, exit 0 (hook runs it).
/// - Dangerous command: command on stdout + reason on stderr, exit
///   `EXIT_AUTO_DANGEROUS` (hook pre-fills it instead).
/// - Command whose head the gate could not resolve ([`Risk::Unknown`]): same as
///   dangerous — command on stdout + reason on stderr, exit
///   `EXIT_AUTO_DANGEROUS`. The code is deliberately not new, so hooks that
///   switch on `0` vs `20` keep working and fail closed by default.
fn auto_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if config.backend.engine == "opencode" {
        match managed_turn(config, line, AgentMode::Auto, true) {
            Ok(Some(_)) => return Ok(0),
            Ok(None) => {}
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                return Ok(1);
            }
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(1);
    };
    // Bound the blocking LLM call so a dead/slow network can't freeze the prompt.
    arm_hook_budget(config);
    let mem = hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    let suggestion = modes::suggest::request(line, p, executor, config, session.history())?;
    // The blocking network work is done; let the rest run unbounded.
    cancel_hook_budget();
    let (reply, code) = match &suggestion {
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } if shell_syntax_ok(executor, command) => {
            if !explanation.is_empty() {
                eprintln!("{}", explanation.as_str().dim());
            }
            let code = match safety::assess(command) {
                Risk::Safe => 0,
                Risk::Dangerous(reason) => {
                    eprintln!("{}", format!("⚠ {reason} — pre-filled for review").yellow());
                    EXIT_AUTO_DANGEROUS
                }
                // Same exit code on purpose: the contract's `20` means "do not
                // auto-run this, pre-fill it for review", which is exactly the
                // right handling for a command the gate could not resolve. A new
                // code would break every hook that switches on 0 vs 20.
                Risk::Unknown(reason) => {
                    eprintln!(
                        "{}",
                        format!("⚠ could not verify ({reason}) — pre-filled for review").yellow()
                    );
                    EXIT_AUTO_DANGEROUS
                }
            };
            let action = if code == 0 { "run" } else { "fill" };
            if !stage_hook_command(action, command)? {
                println!("{command}");
            }
            let reply = if explanation.is_empty() {
                command.clone()
            } else {
                format!("{command}\n{explanation}")
            };
            (reply, code)
        }
        // A "command" that is not valid shell is really an answer: surface it and
        // emit no command, so the hook never evals a non-command.
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            let answer = if explanation.is_empty() {
                command.clone()
            } else {
                explanation.clone()
            };
            if !answer.is_empty() {
                eprintln!("{answer}");
            }
            (answer, 0)
        }
        modes::suggest::Suggestion::Answer { explanation } => {
            if !explanation.is_empty() {
                eprintln!("{explanation}");
            }
            (explanation.clone(), 0)
        }
    };
    if let Some(path) = &mem {
        session.record_user(line);
        session.record_assistant(&reply);
        session.save_persisted(path);
    }
    Ok(code)
}

/// Public scripting interface (`aishe suggest`). Turns a natural-language request
/// into a shell command and prints it (text or `--json`). Exit-code contract,
/// stable across minor versions:
///
/// - `0` — a safe command (printed to stdout) or a prose answer (to stderr),
/// - `20` — a command the safety gate flags (still printed, for review): either
///   dangerous, or one whose head the gate could not resolve,
/// - `1` — no provider, no query, or provider request failure.
///
/// In JSON mode a single object is printed to stdout with fields
/// `{kind, command, explanation, risk, reason}`. `risk` is `"safe"`,
/// `"dangerous"`, `"unknown"` (the gate could not tell what the command runs —
/// treat like `"dangerous"`), or `"n/a"` for an answer. The existing values and
/// exit codes are unchanged; `"unknown"` is additive, so a consumer that only
/// tests `risk == "safe"` still fails closed.
fn suggest_command(
    query: &str,
    json: bool,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if query.trim().is_empty() {
        eprintln!("aishe: suggest needs a request, e.g. aishe suggest \"list files by size\"");
        return Ok(1);
    }
    let suggestion = if config.backend.engine == "opencode" {
        match managed_turn(config, query, AgentMode::Suggest, false) {
            Ok(Some(outcome)) => modes::suggest::parse_suggestion(&outcome.text),
            Ok(None) => {
                let Some(provider) = provider else {
                    print_llm_unavailable(config);
                    return Ok(1);
                };
                match modes::suggest::request_strict(query, provider, executor, config, Vec::new())
                {
                    Ok(suggestion) => suggestion,
                    Err(error) => {
                        eprintln!(
                            "{}",
                            format!("aishe: {}", crate::providers::actionable_error(&error)).red()
                        );
                        return Ok(1);
                    }
                }
            }
            Err(error) => {
                eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                return Ok(1);
            }
        }
    } else {
        let Some(provider) = provider else {
            print_llm_unavailable(config);
            return Ok(1);
        };
        match modes::suggest::request_strict(query, provider, executor, config, Vec::new()) {
            Ok(suggestion) => suggestion,
            Err(error) => {
                eprintln!(
                    "{}",
                    format!("aishe: {}", crate::providers::actionable_error(&error)).red()
                );
                return Ok(1);
            }
        }
    };

    // Classify into (kind, command, explanation, risk, reason, exit).
    let (kind, command, explanation, risk, reason, code) = match suggestion {
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } if shell_syntax_ok(executor, &command) => match safety::assess(&command) {
            Risk::Safe => ("command", command, explanation, "safe", String::new(), 0),
            Risk::Dangerous(r) => (
                "command",
                command,
                explanation,
                "dangerous",
                r.to_string(),
                EXIT_AUTO_DANGEROUS,
            ),
            Risk::Unknown(r) => (
                "command",
                command,
                explanation,
                "unknown",
                r.to_string(),
                EXIT_AUTO_DANGEROUS,
            ),
        },
        // A "command" that isn't valid shell is really a prose answer.
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            let answer = if explanation.is_empty() {
                command
            } else {
                explanation
            };
            ("answer", String::new(), answer, "n/a", String::new(), 0)
        }
        modes::suggest::Suggestion::Answer { explanation } => (
            "answer",
            String::new(),
            explanation,
            "n/a",
            String::new(),
            0,
        ),
    };

    if json {
        let obj = serde_json::json!({
            "kind": kind,
            "command": command,
            "explanation": explanation,
            "risk": risk,
            "reason": reason,
        });
        println!("{obj}");
    } else if kind == "command" {
        if !explanation.is_empty() {
            eprintln!("{}", explanation.as_str().dim());
        }
        if risk == "dangerous" {
            eprintln!("{}", format!("⚠ {reason}").yellow());
        } else if risk == "unknown" {
            eprintln!("{}", format!("⚠ could not verify ({reason})").yellow());
        }
        println!("{command}");
    } else if !explanation.is_empty() {
        // An answer: to stderr so stdout stays empty (no command to run).
        eprintln!("{explanation}");
    }
    Ok(code)
}

/// Run one dispatch cycle non-interactively for the `-c` flag.
#[allow(clippy::too_many_arguments)]
fn one_shot(
    input: &str,
    executor: &mut Executor,
    provider: &mut Option<Arc<dyn Provider>>,
    config: &Config,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
    mcp: &aishe::mcp::McpRegistry,
) -> Result<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    // One-shot runs are a single process: no cross-line memory.
    let mut session = Session::new(false);
    // User-defined /slash-commands work in -c too.
    if try_custom_command(
        trimmed,
        commands,
        executor,
        provider.as_deref(),
        config,
        skills,
        mcp,
        &mut session,
    )? {
        return Ok(executor.last_exit as u8);
    }
    match dispatcher::dispatch(trimmed, cache) {
        Dispatch::Shell(line) => Ok(executor.run(&line) as u8),
        Dispatch::Builtin(tokens) => {
            if matches!(tokens[0].as_str(), "exit" | "quit") {
                // `exit N` propagates N; bare `exit` uses the last command's code.
                let code = tokens
                    .get(1)
                    .and_then(|s| s.parse::<u8>().ok())
                    .unwrap_or(executor.last_exit as u8);
                return Ok(code);
            }
            if tokens[0] == "aishe" {
                // Read-only listings are useful in -c; state-changing meta
                // commands need the interactive session (persistence/restart).
                match tokens.get(1).map(|s| s.as_str()) {
                    Some("commands") => {
                        if commands.is_empty() {
                            println!(
                                "no custom commands (add *.md files to {})",
                                aishe::commands::user_dir().unwrap_or_default().display()
                            );
                        } else {
                            println!("custom slash-commands:");
                            for (name, desc) in commands.list() {
                                println!("\x20 /{name}  —  {desc}");
                            }
                        }
                    }
                    Some("skills") => {
                        if skills.is_empty() {
                            println!(
                                "no skills (add <name>/SKILL.md files to {})",
                                aishe::skills::user_dir().unwrap_or_default().display()
                            );
                        } else {
                            println!("model-invoked skills (yolo mode):");
                            for (name, desc) in skills.list() {
                                println!("\x20 {name}  —  {desc}");
                            }
                        }
                        warn_untrusted_skills(skills);
                    }
                    Some("mcp") => print_mcp_listing(mcp),
                    Some("config") => {
                        println!("config file: {}", Config::path().display());
                        match toml::to_string_pretty(config) {
                            Ok(t) => println!("\n{t}"),
                            Err(e) => eprintln!("aishe: {e}"),
                        }
                    }
                    Some("usage") => print_usage_summary(provider.as_deref(), config),
                    _ => println!("aishe meta commands are interactive-only"),
                }
                return Ok(0);
            }
            Ok(executor.run_builtin(&tokens) as u8)
        }
        Dispatch::NaturalLanguage(nl) => {
            let agent_mode = AgentMode::parse(&config.aishe.mode).unwrap_or(AgentMode::Suggest);
            if config.backend.engine == "opencode" {
                let render = agent_mode != AgentMode::Suggest;
                match managed_turn(config, &nl, agent_mode, render) {
                    Ok(Some(outcome)) => {
                        if agent_mode == AgentMode::Suggest {
                            emit_suggest_hook(
                                &modes::suggest::parse_suggestion(&outcome.text),
                                executor,
                            );
                        }
                        return Ok(0);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        eprintln!("aishe: {}", aishe::redact::redact(&error.to_string()));
                        return Ok(1);
                    }
                }
            }
            match provider {
                Some(p) => {
                    if config.aishe.mode == "yolo" {
                        modes::yolo::run(
                            &nl,
                            p.as_ref(),
                            executor,
                            config,
                            &INTERRUPTED,
                            skills,
                            mcp,
                            &mut session,
                        )?;
                    } else {
                        // -c + NL in suggest/auto mode: print suggested command, don't run.
                        modes::suggest::run(
                            &nl,
                            p.as_ref(),
                            executor,
                            config,
                            true,
                            false,
                            &mut session,
                        )?;
                    }
                    Ok(0)
                }
                None => {
                    print_llm_unavailable(config);
                    Ok(1)
                }
            }
        }
    }
}

/// Parse a `/name arg…` slash-command line into (name, args).
fn parse_slash(line: &str) -> Option<(&str, Vec<&str>)> {
    let rest = line.trim().strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?;
    Some((name, parts.collect()))
}

/// Run a user-defined `/slash-command` if `line` names one. Returns whether it
/// was handled. Built-in meta subcommands are left for normal dispatch.
#[allow(clippy::too_many_arguments)]
fn try_custom_command(
    line: &str,
    commands: &CommandRegistry,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &aishe::mcp::McpRegistry,
    session: &mut Session,
) -> Result<bool> {
    let Some((name, args)) = parse_slash(line) else {
        return Ok(false);
    };
    if dispatcher::is_meta_subcommand(name) {
        return Ok(false);
    }
    // MCP prompts (`/<server>:<prompt> args`): fetch the prompt and run it.
    if mcp.is_prompt(name) {
        match mcp.prompt_text(name, &args) {
            Some(Ok(text)) if !text.trim().is_empty() => {
                let mode = config.aishe.mode.as_str();
                run_nl(
                    &text, mode, provider, executor, config, skills, mcp, session,
                )?;
            }
            Some(Ok(_)) => println!("  {}", "(empty prompt)".dim()),
            Some(Err(e)) => eprintln!("{}", format!("aishe: mcp prompt: {e}").red()),
            None => {}
        }
        return Ok(true);
    }
    let Some(cmd) = commands.get(name) else {
        return Ok(false);
    };
    let ex = cmd.expand(&args);
    if ex.text.is_empty() {
        return Ok(true);
    }
    if ex.shell {
        if !gate_custom_shell(cmd, &ex.text) {
            return Ok(true);
        }
        executor.run(&ex.text);
    } else {
        // A repo-supplied `mode:` may not escalate past the user's configured
        // mode unless the command file is trusted (audit finding #3): otherwise
        // `mode: yolo` in a cloned repo silently dispatches the agentic loop.
        let configured = config.aishe.mode.as_str();
        let mode = cmd.effective_mode(configured, custom_cmd_trusted(cmd));
        if let Some(want) = ex.mode.as_deref().filter(|w| *w != mode) {
            eprintln!(
                "{}",
                format!(
                    "aishe: ignoring untrusted project command mode '{want}' — running in '{mode}' (aishe trust {} to allow)",
                    cmd.source.as_deref().unwrap_or(std::path::Path::new("")).display()
                )
                .yellow()
            );
        }
        run_nl(
            &ex.text, mode, provider, executor, config, skills, mcp, session,
        )?;
    }
    Ok(true)
}

/// Tell the user about *project* skills that `SkillRegistry::load` dropped as
/// untrusted, and how to enable them.
///
/// A project skill's body is repo-supplied text handed to the model as
/// instructions, and the model pulls it in mid-loop with no user in the loop —
/// so unlike a `shell:true` project command there is no moment to confirm it at.
/// It is excluded until `aishe trust <file>`; this is the only signal that it
/// exists at all, so it must not be silent.
fn warn_untrusted_skills(skills: &SkillRegistry) {
    for path in skills.untrusted() {
        // The path is repo-supplied: escape control characters so a crafted
        // filename cannot repaint the line (same reason as `gate_custom_shell`).
        let shown = aishe::commands::display_safe(&path.display().to_string());
        eprintln!(
            "{}",
            format!("aishe: ignoring untrusted project skill — aishe trust {shown}").yellow()
        );
    }
}

/// Whether a custom command's source file is currently trusted. A user-origin
/// command (`source == None`) is trusted by construction.
fn custom_cmd_trusted(cmd: &aishe::commands::CustomCommand) -> bool {
    match cmd.source.as_deref() {
        None => true,
        Some(src) => {
            let contents = std::fs::read_to_string(src).unwrap_or_default();
            aishe::trust::is_trusted(src, &contents)
        }
    }
}

/// Gate execution of a `shell:true` custom command. Returns whether to run it.
///
/// A **project**-origin command (from a cloned repo's `<cwd>/.aishe/commands`) must
/// be trusted (`aishe trust <file>`) or explicitly confirmed — the resolved shell
/// command is shown first — before it can run. **Both** origins additionally pass
/// through the standard safety gate (`assess` + `confirm_dangerous`).
fn gate_custom_shell(cmd: &aishe::commands::CustomCommand, resolved: &str) -> bool {
    use std::io::Write;
    // ponytail: prompt-per-run is the floor for an untrusted project command;
    // `aishe trust <file>` upgrades it to trust-once. No sandbox beyond that.
    if let Some(src) = cmd.source.as_deref() {
        if cmd.needs_trust_confirm(custom_cmd_trusted(cmd)) {
            println!();
            println!("  {}", "untrusted project command".yellow().bold());
            println!("  {} {}", "file:".dim(), src.display().to_string().dim());
            // The body is repo-supplied: escape control characters so it cannot
            // repaint this line and preview a command other than the one that runs.
            println!(
                "  {} {}",
                "will run:".dim(),
                aishe::commands::display_safe(resolved).white().bold()
            );
            print!("  Run this shell command? [y/N] ");
            std::io::stdout().flush().ok();
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_err() {
                return false;
            }
            if !matches!(line.trim(), "y" | "Y" | "yes" | "Yes") {
                println!("  {}", "cancelled".dim());
                return false;
            }
        }
    }
    // Both origins: assess + confirm the resolved body like any other command.
    matches!(modes::safety_gate(resolved), modes::GateOutcome::Proceed)
}

/// Run a natural-language request in the given mode.
#[allow(clippy::too_many_arguments)]
fn run_nl(
    nl: &str,
    mode: &str,
    provider: Option<&dyn Provider>,
    executor: &mut Executor,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &aishe::mcp::McpRegistry,
    session: &mut Session,
) -> Result<()> {
    let agent_mode = AgentMode::parse(mode).unwrap_or(AgentMode::Suggest);
    if config.backend.engine == "opencode" {
        let render = agent_mode != AgentMode::Suggest;
        if let Some(outcome) = managed_turn(config, nl, agent_mode, render)? {
            if agent_mode == AgentMode::Suggest {
                emit_suggest_hook(&modes::suggest::parse_suggestion(&outcome.text), executor);
            }
            return Ok(());
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(());
    };
    match mode {
        "yolo" => modes::yolo::run(nl, p, executor, config, &INTERRUPTED, skills, mcp, session)?,
        "auto" => modes::suggest::run(nl, p, executor, config, false, true, session)?,
        _ => modes::suggest::run(nl, p, executor, config, false, false, session)?,
    }
    Ok(())
}

/// Print the `aishe mcp` listing: connected tools (yolo), plus any prompts
/// (invocable as `/<server>:<prompt>`).
fn print_mcp_listing(mcp: &aishe::mcp::McpRegistry) {
    if mcp.is_fully_empty() {
        println!("no MCP servers (configure them under [mcp_servers] in config)");
        return;
    }
    if !mcp.is_empty() {
        println!("MCP tools (yolo mode):");
        for (name, desc) in mcp.list() {
            let desc = desc.lines().next().unwrap_or("");
            println!("\x20 {name}  -  {desc}");
        }
    }
    let prompts = mcp.list_prompts();
    if !prompts.is_empty() {
        println!("MCP prompts (run as /<server>:<prompt>):");
        for (name, desc) in prompts {
            let desc = desc.lines().next().unwrap_or("");
            println!("\x20 /{name}  -  {desc}");
        }
    }
}

/// Print the session token/cost summary (`aishe usage` / `/usage`).
/// Append this process's metered usage to the shared per-session tally named by
/// `AISHE_USAGE_FILE`, so the interactive PTY can print a one-line session-cost
/// summary on exit. No-op when the env var is unset (i.e. not under a PTY
/// session) or no model calls were made.
fn record_session_usage(provider: Option<&dyn Provider>, config: &Config) {
    let Ok(path) = std::env::var("AISHE_USAGE_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Some(p) = provider else { return };
    let snap = p.meter().snapshot();
    if snap.is_empty() {
        return;
    }
    aishe::usagelog::append(std::path::Path::new(&path), snap, config.active_model());
    if let Ok(status_path) = std::env::var("AISHE_STATUS_FILE") {
        if !status_path.is_empty() {
            aishe::usagelog::write_status(
                std::path::Path::new(&status_path),
                std::path::Path::new(&path),
                &config.pricing,
                Some((snap, config.active_model())),
                &config.aishe.status_line_items,
            );
        }
    }
}

fn print_usage_summary(provider: Option<&dyn Provider>, config: &Config) {
    match provider {
        Some(p) => {
            let snap = p.meter().snapshot();
            if snap.is_empty() {
                println!("usage: no model calls yet this session");
            } else {
                println!(
                    "usage: {}",
                    aishe::usage::summary(snap, config.active_model(), &config.pricing)
                );
            }
            if config.aishe.budget_usd > 0.0 {
                println!(
                    "budget: ${:.2} (set budget_usd=0 for unlimited)",
                    config.aishe.budget_usd
                );
            }
        }
        None => println!("usage: provider not configured"),
    }
}

/// Back the `mode` / `model` / `provider` subcommands. With no `value`, print the
/// effective current value. With a `value`, persist it to the *user* config file:
/// we reload a fresh `Config` (no project overlay, no this-invocation flags) so a
/// project overlay or a `--mode`/`--provider` flag can't get baked into the saved
/// file. Clap already validated `mode`/`provider` against their allowed sets.
fn set_or_show(field: &str, value: Option<&str>, effective: &Config) -> u8 {
    let Some(value) = value else {
        let current = match field {
            "mode" => effective.aishe.mode.clone(),
            "provider" => effective.aishe.provider.clone(),
            "scope" => effective.backend.default_scope.clone(),
            "network" => effective.backend.workspace_network.clone(),
            _ => effective.active_model().to_string(),
        };
        println!("{field}: {current}");
        return 0;
    };
    let mut cfg = match Config::load_or_init() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("aishe: {e}");
            return 1;
        }
    };
    match field {
        "mode" => {
            cfg.aishe.mode = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        "provider" => cfg.aishe.provider = value.to_string(),
        "scope" => {
            cfg.backend.default_scope = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        "network" => {
            cfg.backend.workspace_network = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        _ => cfg.set_active_model(value.to_string()),
    }
    if let Err(e) = cfg.save() {
        eprintln!("aishe: {e}");
        return 1;
    }
    if field == "model" {
        // Under the PTY front-end, let the parent zsh prompt pick up the saved
        // model on its very next precmd. This is best-effort: config persistence
        // is the source of truth, and non-PTY invocations have no state file.
        if let Some(path) = std::env::var_os("AISHE_MODEL_FILE").filter(|p| !p.is_empty()) {
            let _ = std::fs::write(path, aishe::commands::display_safe(cfg.active_model()));
        }
    }
    if field == "scope" {
        if let Some(path) = std::env::var_os("AISHE_SCOPE_FILE").filter(|p| !p.is_empty()) {
            let _ = std::fs::write(
                path,
                aishe::commands::display_safe(&cfg.backend.default_scope),
            );
        }
    }
    println!(
        "{} = {}  (saved to {})",
        aishe::commands::display_safe(field),
        aishe::commands::display_safe(value),
        aishe::commands::display_safe(&Config::path().display().to_string())
    );
    0
}

fn models_command(config: &Config, provider: &str, json: bool) -> u8 {
    match aishe::capabilities::list_models(config, provider) {
        Ok(models) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&models).unwrap_or_else(|_| "[]".into())
                );
            } else {
                println!("{provider}: {} model(s):", models.len());
                for model in models {
                    let active =
                        if provider == config.aishe.provider && model == config.active_model() {
                            " (active)"
                        } else {
                            ""
                        };
                    println!("  {}{active}", aishe::commands::display_safe(&model));
                }
            }
            0
        }
        Err(error) => {
            eprintln!(
                "aishe: models: {:?}: {}",
                error.kind(),
                aishe::redact::redact(&error.to_string())
            );
            1
        }
    }
}

fn print_capability_report(report: &aishe::capabilities::Report) {
    println!(
        "provider validation: {} · {} · {}",
        aishe::commands::display_safe(&report.provider),
        aishe::commands::display_safe(&report.model),
        aishe::commands::display_safe(&report.transport)
    );
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
        let marker = match check.state {
            aishe::capabilities::State::Pass => "✓",
            aishe::capabilities::State::Warn => "!",
            aishe::capabilities::State::Fail => "✗",
            aishe::capabilities::State::Skipped => "·",
        };
        println!(
            "  {marker} {label}: {}",
            aishe::commands::display_safe(&check.detail)
        );
    }
}

fn context_command(
    mut effective: Config,
    explain: bool,
    request: Option<&str>,
    json_output: bool,
    excludes: &[String],
    includes: &[String],
) -> Result<u8> {
    const OPTIONAL: &[&str] = &[
        "history",
        "project_context",
        "project_tasks",
        "host_profile",
    ];
    for section in excludes.iter().chain(includes.iter()) {
        if !OPTIONAL.contains(&section.as_str()) {
            eprintln!(
                "aishe: unknown context section '{section}' (expected {})",
                OPTIONAL.join(", ")
            );
            return Ok(1);
        }
    }
    if let Some(section) = excludes
        .iter()
        .find(|section| includes.iter().any(|included| included == *section))
    {
        eprintln!("aishe: context section '{section}' cannot be included and excluded together");
        return Ok(1);
    }

    if !excludes.is_empty() || !includes.is_empty() {
        let mut persisted =
            Config::load_quiet()?.context("no config exists; run `aishe setup` first")?;
        for section in excludes {
            if !persisted
                .aishe
                .context_exclude
                .iter()
                .any(|item| item == section)
            {
                persisted.aishe.context_exclude.push(section.clone());
            }
        }
        for section in includes {
            persisted
                .aishe
                .context_exclude
                .retain(|item| item != section);
            match section.as_str() {
                "project_context" => persisted.aishe.project_context = true,
                "project_tasks" => persisted.aishe.project_tasks = true,
                "host_profile" => persisted.aishe.host_profile = true,
                _ => {}
            }
        }
        persisted.save()?;
        effective.aishe.context_exclude = persisted.aishe.context_exclude.clone();
        effective.aishe.project_context = persisted.aishe.project_context;
        effective.aishe.project_tasks = persisted.aishe.project_tasks;
        effective.aishe.host_profile = persisted.aishe.host_profile;
        if !json_output {
            for section in excludes {
                println!("context.{section} = excluded");
            }
            for section in includes {
                println!("context.{section} = included");
            }
        }
    }

    let mut executor = Executor::new()?;
    executor.set_history_log(history_paths(&effective).1);
    context::init(executor.shell());
    if !explain && request.is_none() && !json_output && excludes.is_empty() && includes.is_empty() {
        print!("{}", context::build(&executor, &effective));
        return Ok(0);
    }
    let report = context::preview(&executor, &effective, request);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(0);
    }
    println!(
        "context preview: {} · {} · ~{} tokens{}",
        aishe::commands::display_safe(&report.provider),
        aishe::commands::display_safe(&report.model),
        report.total_estimated_tokens,
        report
            .estimated_input_cost_usd
            .map(|cost| format!(" · ~${cost:.6} input"))
            .unwrap_or_else(|| " · cost n/a".into())
    );
    for section in &report.sections {
        println!(
            "  {} {:16} ~{:5} tok · {} · {}{}",
            if section.included { "✓" } else { "–" },
            aishe::commands::display_safe(&section.id),
            section.estimated_tokens,
            if section.required {
                "required"
            } else if section.included {
                "included"
            } else {
                "excluded"
            },
            aishe::commands::display_safe(&section.source),
            if section.redactions > 0 {
                format!(" · {} redacted", section.redactions)
            } else {
                String::new()
            }
        );
    }
    if let Some(text) = request {
        println!(
            "  request: {} chars · ~{} tokens (text intentionally not echoed)",
            text.chars().count(),
            report.request_estimated_tokens
        );
    }
    Ok(0)
}

fn tasks_list_command(json_output: bool) -> u8 {
    let records = aishe::tasks::list();
    let managed = aishe::backend::opencode::session::SessionStore::from_default_root()
        .and_then(|store| store.records(None))
        .unwrap_or_default();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "managed": managed,
                "legacy": records,
            }))
            .unwrap_or_else(|_| "{}".into())
        );
        return 0;
    }
    if records.is_empty() && managed.is_empty() {
        println!("no AI sessions");
        return 0;
    }
    if !managed.is_empty() {
        println!("managed OpenCode sessions (newest mapping last):");
        for record in managed {
            println!(
                "  {}  {:?} · {:?}  {}",
                aishe::commands::display_safe(&record.backend_session_id),
                record.mode,
                record.scope,
                aishe::commands::display_safe(&record.workspace.display().to_string())
            );
        }
    }
    if records.is_empty() {
        return 0;
    }
    println!("legacy durable task sessions (oldest first, retained):");
    for record in records {
        println!(
            "  {}  {:?}  {} / {}  {}",
            aishe::commands::display_safe(&record.id),
            record.status,
            aishe::commands::display_safe(&record.provider),
            aishe::commands::display_safe(&record.model),
            aishe::commands::display_safe(
                &record
                    .name
                    .as_deref()
                    .unwrap_or(record.objective.as_str())
                    .chars()
                    .take(72)
                    .collect::<String>()
            )
        );
    }
    0
}

fn task_session_command(command: &TaskSessionCmd) -> Result<u8> {
    match command {
        TaskSessionCmd::Show { id, json } => {
            let record = aishe::tasks::load(id)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!("task: {}", record.id);
                println!("status: {:?}", record.status);
                println!(
                    "name: {}",
                    aishe::commands::display_safe(record.name.as_deref().unwrap_or("(none)"))
                );
                println!(
                    "objective: {}",
                    aishe::commands::display_safe(&record.objective)
                );
                println!(
                    "provider: {} · {}",
                    aishe::commands::display_safe(&record.provider),
                    aishe::commands::display_safe(&record.model)
                );
                println!(
                    "cwd: {}",
                    aishe::commands::display_safe(&record.cwd.display().to_string())
                );
                println!(
                    "usage: {} in · {} out · {} reqs",
                    record.usage.input, record.usage.output, record.usage.requests
                );
                println!("messages: {}", record.messages.len());
                println!("completed tools: {}", record.completed_tools.len());
                if let Some(pending) = record.pending_tool {
                    println!(
                        "pending tool: {} ({}, may_have_started={})",
                        aishe::commands::display_safe(&pending.call.name),
                        aishe::commands::display_safe(&pending.call.id),
                        pending.may_have_started
                    );
                }
                if let Some(error) = record.last_error {
                    println!("last error: {}", aishe::commands::display_safe(&error));
                }
            }
            Ok(0)
        }
        TaskSessionCmd::Rename { id, name } => {
            aishe::tasks::rename(id, name)?;
            println!("renamed task {id} to {name}");
            Ok(0)
        }
        TaskSessionCmd::Delete { id } => {
            aishe::tasks::delete(id)?;
            println!("deleted task {id} (the task record cannot be recovered)");
            Ok(0)
        }
    }
}

fn resume_task_command(
    config: &Config,
    id: Option<&str>,
    replacement_cwd: Option<&std::path::Path>,
) -> Result<u8> {
    if let Some(id) = id.filter(|id| id.starts_with("ses_")) {
        return resume_managed_session(config, id, replacement_cwd);
    }
    let record = match id {
        Some(id) => aishe::tasks::load(id)?,
        None => aishe::tasks::most_recent_resumable()
            .context("no interrupted, failed, or active task is available to resume")?,
    };
    let cwd = if record.cwd.is_dir() {
        record.cwd.clone()
    } else if let Some(path) = replacement_cwd {
        if !path.is_dir() {
            anyhow::bail!("replacement cwd {} is not a directory", path.display());
        }
        path.to_path_buf()
    } else if std::io::stdin().is_terminal() {
        let current = std::env::current_dir()?;
        let Some(value) = aishe::promptui::text(
            &format!(
                "Original cwd {} is missing; replacement",
                record.cwd.display()
            ),
            &current.display().to_string(),
            |value| {
                if std::path::Path::new(value).is_dir() {
                    Ok(())
                } else {
                    anyhow::bail!("path must be an existing directory")
                }
            },
        )?
        else {
            anyhow::bail!("resume cancelled");
        };
        if value == ":back" {
            anyhow::bail!("resume cancelled");
        }
        std::path::PathBuf::from(value)
    } else {
        anyhow::bail!(
            "original cwd {} no longer exists; pass `aishe resume {} --cwd PATH`",
            record.cwd.display(),
            record.id
        );
    };
    let provider = providers::make(config).map_err(|error| {
        anyhow::anyhow!("cannot resume without an LLM provider: {error}; run `aishe doctor --live`")
    })?;
    let mut executor = Executor::new()?;
    executor.redirect_cwd(cwd);
    executor.set_history_log(history_paths(config).1);
    context::init(executor.shell());
    init_audit(config);
    let skills = SkillRegistry::load();
    let mcp = aishe::mcp::McpRegistry::connect(&config.mcp_servers);
    modes::yolo::resume(
        record,
        provider.as_ref(),
        &mut executor,
        config,
        &INTERRUPTED,
        &skills,
        &mcp,
    )?;
    record_session_usage(Some(provider.as_ref()), config);
    Ok(0)
}

fn resume_managed_session(
    config: &Config,
    id: &str,
    replacement_cwd: Option<&std::path::Path>,
) -> Result<u8> {
    use aishe::agent::{BackendSession, ExecutionScope, Mode};

    let already_in_aishe_shell = std::env::var_os("AISHE_SHELL_ID").is_some();
    if !already_in_aishe_shell
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
    {
        anyhow::bail!(
            "`aishe resume {id}` needs an interactive terminal when run outside an active Aishe shell"
        );
    }
    let state = aishe::backend::supervisor::ensure_running(config)?;
    let control = aishe::backend::control::SupervisorClient::new(state)?;
    let client = aishe::backend::opencode::OpenCodeClient::new(
        control.opencode_connection(),
        control.provider_id(),
        control.model_id(),
    )?;
    let sessions = client.list_sessions(None)?;
    let summary = sessions
        .into_iter()
        .find(|session| session.id == id)
        .with_context(|| format!("managed session '{id}' was not found"))?;
    let workspace = match replacement_cwd {
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) => anyhow::bail!("replacement cwd {} is not a directory", path.display()),
        None if summary.workspace.is_dir() => summary.workspace,
        None => anyhow::bail!(
            "managed session workspace {} is missing; pass --cwd PATH",
            summary.workspace.display()
        ),
    };
    let session = BackendSession {
        id: id.to_string(),
        workspace,
        backend: "opencode".into(),
    };
    let mode = Mode::parse(&config.aishe.mode).context("active mode is invalid")?;
    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("active backend scope is invalid")?;
    let shell_id = aishe::agent::controller::current_shell_id()?;
    aishe::backend::opencode::session::SessionStore::from_default_root()?
        .bind(&shell_id, &session, mode, scope)?;
    println!("resumed managed session {id}");
    println!("workspace: {}", session.workspace.display());
    if already_in_aishe_shell {
        println!("The next natural-language turn in this shell continues that conversation.");
        return Ok(0);
    }
    println!("Opening Aishe; natural-language turns continue that conversation.");
    std::env::set_current_dir(&session.workspace).with_context(|| {
        format!(
            "entering managed session workspace {}",
            session.workspace.display()
        )
    })?;
    aishe::pty::run_zsh_with_shell_id(config, &history_paths(config).1, &shell_id)
}

fn profile_command(effective: &Config, value: Option<&str>) -> u8 {
    let Some(value) = value else {
        println!("profile: {}", effective.aishe.safety_profile);
        return 0;
    };
    let Some(profile) = aishe::profiles::Profile::parse(value) else {
        eprintln!("aishe: unknown profile '{value}'");
        return 1;
    };
    let mut config = match Config::load_quiet() {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!("aishe: no config; run `aishe setup`");
            return 1;
        }
        Err(error) => {
            eprintln!("aishe: {error}");
            return 1;
        }
    };
    let changes = aishe::profiles::apply(&mut config, profile);
    if let Err(error) = config.save() {
        eprintln!("aishe: {error}");
        return 1;
    }
    println!("profile = {}", profile.key());
    if changes.is_empty() {
        println!("  no setting changes");
    } else {
        for change in changes {
            println!("  {}: {} → {}", change.field, change.before, change.after);
        }
    }
    0
}

fn price_command(_effective: &Config, command: &PriceCmd) -> u8 {
    let mut config = match Config::load_quiet() {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!("aishe: no config; run `aishe setup`");
            return 1;
        }
        Err(error) => {
            eprintln!("aishe: {error}");
            return 1;
        }
    };
    match command {
        PriceCmd::List => {
            if config.pricing.is_empty() {
                println!("no user price overrides");
            } else {
                println!("user prices (USD per 1M tokens):");
                for (model, price) in &config.pricing {
                    println!(
                        "  {}: input ${:.6} · output ${:.6}",
                        aishe::commands::display_safe(model),
                        price.input,
                        price.output
                    );
                }
            }
            let model = config.active_model();
            match aishe::usage::price_for(model, &config.pricing) {
                Some(price) => println!(
                    "active {}: input ${:.6} · output ${:.6}",
                    aishe::commands::display_safe(model),
                    price.input,
                    price.output
                ),
                None => {
                    let model = aishe::commands::display_safe(model);
                    println!(
                        "active {model}: unknown; run `aishe price set {model} --input USD --output USD`"
                    )
                }
            }
            return 0;
        }
        PriceCmd::Set {
            model,
            input,
            output,
        } => {
            if !input.is_finite() || *input < 0.0 || !output.is_finite() || *output < 0.0 {
                eprintln!("aishe: prices must be finite non-negative numbers");
                return 1;
            }
            config.pricing.insert(
                model.clone(),
                aishe::usage::Price {
                    input: *input,
                    output: *output,
                },
            );
            if let Err(error) = config.save() {
                eprintln!("aishe: {error}");
                return 1;
            }
            println!(
                "price {} = input ${input:.6} · output ${output:.6} per 1M tokens",
                aishe::commands::display_safe(model)
            );
        }
        PriceCmd::Remove { model } => {
            if config.pricing.remove(model).is_none() {
                eprintln!(
                    "aishe: no exact user price override for '{}'",
                    aishe::commands::display_safe(model)
                );
                return 1;
            }
            if let Err(error) = config.save() {
                eprintln!("aishe: {error}");
                return 1;
            }
            println!(
                "removed price override for {}",
                aishe::commands::display_safe(model)
            );
        }
    }
    0
}

fn data_dir() -> std::path::PathBuf {
    aishe::config::data_root()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("aishe")
}

/// The reedline history file and the timestamped sidecar log paths. Shared across
/// sessions by default (zsh `SHARE_HISTORY`), or pid-suffixed per-session when
/// `share_history` is off.
fn history_paths(config: &Config) -> (std::path::PathBuf, std::path::PathBuf) {
    if config.aishe.share_history {
        (data_dir().join("history"), data_dir().join("history.ext"))
    } else {
        let pid = std::process::id();
        (
            data_dir().join(format!("history.{pid}")),
            data_dir().join(format!("history.{pid}.ext")),
        )
    }
}

/// History destination for the direct `-c` fast path. A parent Aishe PTY
/// exports the exact active file; standalone invocations preserve the existing
/// first-run, migration, malformed-config, and `share_history=false` contracts.
/// No provider, plugin, MCP registry, or managed backend is constructed.
fn fast_history_log() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("AISHE_HISTFILE").filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    let mut config = Config::load_or_init()?;
    let _project_overlay = std::env::current_dir()
        .ok()
        .and_then(|cwd| config.apply_project_overlay(&cwd));
    Ok(history_paths(&config).1)
}

/// The on-disk semantic-history vector store.
fn semhist_path() -> std::path::PathBuf {
    data_dir().join("history.vec")
}

/// `aishe dry-run "<cmd>"`: run the command against a throwaway copy of the
/// working tree under bubblewrap (read-only root, no network), show the file
/// changes it would make, then keep them (`--apply`) or discard them.
fn dry_run_command(command: &str, apply: bool) -> Result<u8> {
    if !aishe::overlay::available() {
        eprintln!(
            "aishe: dry-run needs Linux bubblewrap (bwrap) for safe isolation — install it \
             (apt install bubblewrap | dnf install bubblewrap | pacman -S bubblewrap)."
        );
        return Ok(1);
    }
    let cwd = std::env::current_dir()?;
    let staging = std::env::temp_dir().join(format!("aishe-dryrun-{}", std::process::id()));
    std::fs::remove_dir_all(&staging).ok();
    let _guard = TempDirGuard(staging.clone());

    if let Err(e) = aishe::overlay::copy_tree(&cwd, &staging) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }

    // Run the command in the sandbox: <bwrap-argv…> -- <shell> -c <command>.
    let shell = Executor::new()
        .ok()
        .map(|e| e.shell().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
    let argv = aishe::overlay::dry_run_argv(&cwd, &staging);
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg(&shell)
        .arg("-c")
        .arg(command)
        .status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("aishe: failed to launch sandbox: {e}");
            return Ok(1);
        }
    };

    let changes = aishe::overlay::changes(&cwd, &staging);
    println!();
    if changes.is_empty() {
        println!(
            "{} no file changes (command exit {code}).",
            "dry-run:".bold()
        );
        return Ok(code as u8);
    }
    println!(
        "{} {} file change(s) (command exit {code}):",
        "dry-run:".bold(),
        changes.len()
    );
    aishe::overlay::print_changes(&changes);
    if apply {
        let failed = aishe::overlay::apply_journaled(&cwd, &staging, &changes, "dry_run");
        if failed.is_empty() {
            println!(
                "\n{} applied {} change(s) to the working tree ({} to revert).",
                "✓".green(),
                changes.len(),
                "aishe undo".bold()
            );
        } else {
            println!(
                "\n{} applied with {} failure(s): {}",
                "!".yellow(),
                failed.len(),
                failed.join(", ")
            );
        }
    } else {
        println!(
            "\n{} re-run with {} to keep these changes.",
            "discarded —".dim(),
            "--apply".bold()
        );
    }
    Ok(code as u8)
}

/// Removes a directory tree on drop (best-effort), for the dry-run staging copy.
struct TempDirGuard(std::path::PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Dispatch `aishe history <search|index>`.
fn history_command(config: &Config, cmd: &HistoryCmd) -> Result<u8> {
    match cmd {
        HistoryCmd::Index { rebuild } => history_index(config, *rebuild),
        HistoryCmd::Search { query, limit, bare } => {
            history_search(config, &query.join(" "), *limit, *bare)
        }
    }
}

/// Notice + early return when the feature is off, with how to turn it on. In
/// `bare` mode the notice goes to stderr so stdout stays clean for the widget.
fn semantic_history_off_notice_bare(bare: bool) -> u8 {
    if bare {
        eprintln!("aishe: semantic history is off (set semantic_history = true).");
        return 0;
    }
    semantic_history_off_notice()
}

/// Notice + early return when the feature is off, with how to turn it on.
fn semantic_history_off_notice() -> u8 {
    println!(
        "semantic history is off. Enable it in {}:\n  \
         [aishe]\n  semantic_history = true\n  \
         embedding_provider = \"openai\"   # anthropic has no embeddings endpoint\n  \
         embedding_model = \"text-embedding-3-small\"\n\
         then run `aishe history index`.",
        Config::path().display()
    );
    0
}

/// Embed any not-yet-indexed history commands (or all, with `--rebuild`) into the
/// vector store. Reports how many were added.
fn history_index(config: &Config, rebuild: bool) -> Result<u8> {
    if !config.aishe.semantic_history {
        return Ok(semantic_history_off_notice());
    }
    let store = semhist_path();
    let hist = history_paths(config).1;
    match aishe::index::reindex(config, &store, &hist, rebuild) {
        Ok(Ok(ix)) => {
            println!(
                "indexed {} command(s) ({} in the store).",
                ix.added, ix.total
            );
            Ok(0)
        }
        Ok(Err(aishe::index::Skip::NoHistory)) => {
            println!("no history to index yet (run some commands first).");
            Ok(0)
        }
        Ok(Err(aishe::index::Skip::UpToDate(n))) => {
            println!("semantic index is up to date ({n} commands).");
            Ok(0)
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            Ok(1)
        }
    }
}

/// Embed the query and print the closest past commands by meaning. In `bare`
/// mode only the command text is printed (no score column) and every notice goes
/// to stderr, so the recall key binding can assign stdout straight to the line.
fn history_search(config: &Config, query: &str, limit: usize, bare: bool) -> Result<u8> {
    if !config.aishe.semantic_history {
        return Ok(semantic_history_off_notice_bare(bare));
    }
    if query.trim().is_empty() {
        eprintln!(
            "aishe: history search needs a query, e.g. aishe history search \"docker volume\""
        );
        return Ok(1);
    }
    let store = semhist_path();
    let entries = aishe::semhist::load(&store);
    if entries.is_empty() {
        let msg = "the semantic index is empty — run `aishe history index` first.";
        if bare {
            eprintln!("aishe: {msg}");
        } else {
            println!("{msg}");
        }
        return Ok(0);
    }
    let provider = providers::embedder(config)?;
    let qv = provider.embed(&[query.to_string()], &config.aishe.embedding_model)?;
    let Some(qvec) = qv.into_iter().next() else {
        eprintln!("aishe: the embedder returned no vector for the query.");
        return Ok(1);
    };
    let hits = aishe::semhist::top_k(&entries, &qvec, limit.max(1));
    if hits.is_empty() {
        if bare {
            eprintln!("aishe: no match.");
        } else {
            println!("no matches.");
        }
        return Ok(0);
    }
    for (score, cmd) in hits {
        if bare {
            println!("{cmd}");
        } else {
            println!("{}  {cmd}", format!("{score:.2}").dim());
        }
    }
    Ok(0)
}

/// `AISHE_LOG=1` forces it on, `AISHE_LOG_FILE` overrides the path.
/// Resolve the effective audit-log settings from the config file and the
/// environment. Precedence: `AISHE_LOG` *enables* logging on top of the config
/// flag (either turns it on; neither leaves it off), and `AISHE_LOG_FILE`
/// *overrides* the configured path. Pure so the precedence is unit-testable
/// without touching the process environment or the global audit state.
fn resolve_audit(
    config: &Config,
    env_log: Option<&str>,
    env_file: Option<&str>,
) -> (bool, Option<std::path::PathBuf>) {
    let env_on = matches!(env_log, Some("1") | Some("true") | Some("yes"));
    let enabled = config.logging.enabled || env_on;
    let path = env_file
        .map(std::path::PathBuf::from)
        .or_else(|| config.logging.file.clone().map(std::path::PathBuf::from));
    (enabled, path)
}

/// Print a dim/yellow note about what a project `.aishe/config.toml` did, and
/// how to unlock its deferred sensitive keys.
fn notify_project_overlay(outcome: &Option<aishe::config::OverlayOutcome>) {
    let Some(o) = outcome else { return };
    if let Some(err) = &o.error {
        eprintln!(
            "{}",
            format!(
                "aishe: ignoring malformed project config {}: {err}",
                o.path.display()
            )
            .yellow()
        );
        return;
    }
    if !o.applied.is_empty() {
        let how = if o.trusted { ", trusted" } else { "" };
        eprintln!(
            "{}",
            format!(
                "aishe: applied project config {} ({} key(s){how})",
                o.path.display(),
                o.applied.len()
            )
            .dim()
        );
    }
    if !o.deferred.is_empty() {
        eprintln!(
            "{}",
            format!(
                "aishe: {} sensitive key(s) in {} need trust to apply ({}). Run `aishe trust`.",
                o.deferred.len(),
                o.path.display(),
                o.deferred.join(", ")
            )
            .yellow()
        );
    }
}

/// Resolve the nearest project config from cwd, or print why there is none.
fn current_project_config() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    match Config::find_project_config(&cwd) {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "aishe: no .aishe/config.toml found at or above {}",
                cwd.display()
            );
            None
        }
    }
}

/// `aishe trust [--list]`: trust the current project's config, or list trusted.
fn trust_command(list: bool, explicit: Option<&std::path::Path>) -> u8 {
    if list {
        let items = aishe::trust::list();
        if items.is_empty() {
            println!("No trusted project files.");
        } else {
            println!("Trusted project files:");
            for (path, _) in items {
                println!("  {path}");
            }
        }
        return 0;
    }
    // With no argument this trusts the project config; with one it trusts that
    // exact file, which is how a project skill or command is enabled (the gate
    // that rejects them prints the very command to run).
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => match current_project_config() {
            Some(p) => p,
            None => return 1,
        },
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("aishe: {}: {e}", path.display());
            return 1;
        }
    };
    // Only a `config.toml` has sensitive keys to report; a skill or command file
    // is markdown, so parsing it as TOML would be meaningless.
    let is_config = path.file_name().and_then(|n| n.to_str()) == Some("config.toml");
    let deferred = if is_config {
        match toml::from_str::<toml::Table>(&text) {
            Ok(table) => Config::default().merge_project_table(&table, false).1,
            Err(e) => {
                eprintln!("aishe: malformed project config {}: {e}", path.display());
                return 1;
            }
        }
    } else {
        Vec::new()
    };
    match aishe::trust::trust(&path, &text) {
        Ok(_) => {
            println!("Trusted {}", path.display());
            if !deferred.is_empty() {
                println!("  now applies: {}", deferred.join(", "));
            }
            0
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            1
        }
    }
}

/// `aishe untrust [--all]`: drop trust for the current project, or all of them.
fn untrust_command(all: bool, explicit: Option<&std::path::Path>) -> u8 {
    if all {
        return match aishe::trust::untrust_all() {
            Ok(n) => {
                println!("Dropped trust for {n} project file(s).");
                0
            }
            Err(e) => {
                eprintln!("aishe: {e}");
                1
            }
        };
    }
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => match current_project_config() {
            Some(p) => p,
            None => return 1,
        },
    };
    match aishe::trust::untrust(&path) {
        Ok(true) => {
            println!("Dropped trust for {}", path.display());
            0
        }
        Ok(false) => {
            println!("{} was not trusted", path.display());
            0
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            1
        }
    }
}

/// `aishe undo` / `aishe undo --list`: revert the most recent AI file change (or
/// list recorded change sets). Reads the reversible-edits journal written by the
/// built-in file tools.
fn undo_command(list: bool) -> u8 {
    if list {
        let batches = aishe::undo::list();
        if batches.is_empty() {
            println!("no recorded AI file changes");
            return 0;
        }
        println!("recorded AI file changes (most recent last):");
        for b in &batches {
            let state = if b.reverted {
                "reverted".dim().to_string()
            } else {
                "active".green().to_string()
            };
            println!(
                "  {}  {} file(s)  [{}]  {}",
                b.id,
                b.files.len(),
                state,
                b.summary.as_str().dim()
            );
        }
        return 0;
    }
    match aishe::undo::undo_last() {
        Ok(Some(u)) => {
            for f in &u.restored {
                println!("{} {}", "restored".green(), f);
            }
            for e in &u.errors {
                eprintln!("{} {}", "aishe undo:".red(), e);
            }
            if u.restored.is_empty() && u.errors.is_empty() {
                println!("nothing to restore in the last change set");
            }
            if u.errors.is_empty() {
                0
            } else {
                1
            }
        }
        Ok(None) => {
            println!("nothing to undo");
            0
        }
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").red());
            1
        }
    }
}

/// Resolve the audit log path for the read-only `log`/`usage` commands, without
/// initializing the writer: `$AISHE_LOG_FILE`, else `[logging] file`, else the
/// default `$XDG_DATA_HOME/aishe/audit.jsonl`.
fn audit_log_path(config: &Config) -> std::path::PathBuf {
    if let Ok(p) = std::env::var("AISHE_LOG_FILE") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    if let Some(p) = &config.logging.file {
        return std::path::PathBuf::from(p);
    }
    aishe::audit::default_path()
}

/// Parse a relative `--since` like `30m`, `2h`, `3d`, `1w` into a cutoff epoch-ms.
/// A bare number means minutes. Returns `None` if unparseable.
fn parse_since(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().ok()?;
    let secs = match &s[split..] {
        "" | "m" => 60,
        "s" => 1,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    Some(aishe::audit::now_ms_u64().saturating_sub(n * secs * 1000))
}

/// `aishe log`: print (filtered) audit entries as a table, or raw JSONL.
#[allow(clippy::too_many_arguments)]
fn log_command(
    config: &Config,
    session: Option<&str>,
    action: Option<&str>,
    model: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> u8 {
    let path = audit_log_path(config);
    let mut entries = aishe::audit::read_entries(&path);
    if entries.is_empty() && !path.exists() {
        eprintln!(
            "aishe: no audit log at {} (enable it in [logging] or with AISHE_LOG=1)",
            path.display()
        );
        return 0;
    }
    let cutoff = since.and_then(parse_since);
    entries.retain(|e| {
        if let Some(c) = cutoff {
            if e.ts_ms < c {
                return false;
            }
        }
        if let Some(s) = session {
            if e.session != s {
                return false;
            }
        }
        if let Some(a) = action {
            if e.kind != a {
                return false;
            }
        }
        if let Some(m) = model {
            if !e.model.as_deref().is_some_and(|em| em.contains(m)) {
                return false;
            }
        }
        true
    });
    if let Some(n) = limit {
        let len = entries.len();
        if len > n {
            entries.drain(0..len - n);
        }
    }
    if entries.is_empty() {
        println!("no matching audit entries");
        return 0;
    }
    if json {
        for e in &entries {
            println!("{}", e.raw);
        }
        return 0;
    }
    for e in &entries {
        let detail = match e.kind.as_str() {
            "session_start" => format!("── session {} ──", e.session),
            "ai_request" => format!(
                "→ ask {} ({})",
                e.model.as_deref().unwrap_or("?"),
                e.mode.as_deref().unwrap_or("?")
            ),
            "ai_response" => format!(
                "← {} · {} in / {} out",
                e.model.as_deref().unwrap_or("?"),
                e.tokens_in.unwrap_or(0),
                e.tokens_out.unwrap_or(0)
            ),
            "ai_error" => format!(
                "✗ {} {}",
                e.model.as_deref().unwrap_or("?"),
                e.text.as_deref().unwrap_or("")
            ),
            "action" => {
                let exit = e.exit.map(|c| format!(" [exit {c}]")).unwrap_or_default();
                format!(
                    "$ {}{}  ({})",
                    e.command.as_deref().unwrap_or(""),
                    exit,
                    e.source.as_deref().unwrap_or("")
                )
            }
            other => other.to_string(),
        };
        let colored = if e.kind == "ai_error" {
            detail.red().to_string()
        } else if e.kind == "session_start" {
            detail.dim().to_string()
        } else {
            detail
        };
        println!("{}  {}", aishe::audit::fmt_utc(e.ts_ms).dim(), colored);
    }
    0
}

/// `aishe usage`: aggregate token counts and estimated cost from the audit log.
fn usage_history_command(config: &Config, by: Option<&str>, since: Option<&str>) -> u8 {
    use aishe::usage::{self, Usage};
    let path = audit_log_path(config);
    let entries = aishe::audit::read_entries(&path);
    if entries.is_empty() && !path.exists() {
        eprintln!(
            "aishe: no audit log at {} (enable it in [logging] or with AISHE_LOG=1)",
            path.display()
        );
        return 0;
    }
    let cutoff = since.and_then(parse_since);
    let by = by.unwrap_or("model");

    #[derive(Default)]
    struct Agg {
        tin: u64,
        tout: u64,
        reqs: u64,
        cost: f64,
        unknown: u64,
    }
    let mut groups: std::collections::BTreeMap<String, Agg> = std::collections::BTreeMap::new();
    let mut total = Agg::default();
    for e in &entries {
        if e.kind != "ai_response" {
            continue;
        }
        if let Some(c) = cutoff {
            if e.ts_ms < c {
                continue;
            }
        }
        let tin = e.tokens_in.unwrap_or(0);
        let tout = e.tokens_out.unwrap_or(0);
        let model = e.model.as_deref().unwrap_or("?");
        let (cost, known) = match usage::price_for(model, &config.pricing) {
            Some(p) => (
                usage::cost(
                    Usage {
                        input: tin,
                        output: tout,
                        requests: 1,
                    },
                    p,
                ),
                true,
            ),
            None => (0.0, false),
        };
        let key = match by {
            "day" => aishe::audit::fmt_date(e.ts_ms),
            "session" => e.session.clone(),
            _ => model.to_string(),
        };
        let g = groups.entry(key).or_default();
        for agg in [g, &mut total] {
            agg.tin += tin;
            agg.tout += tout;
            agg.reqs += 1;
            agg.cost += cost;
            if !known {
                agg.unknown += 1;
            }
        }
    }
    if total.reqs == 0 {
        println!("no model calls recorded in the audit log");
        return 0;
    }
    println!("usage by {by}:");
    let fmt_cost = |a: &Agg| {
        if a.unknown == 0 {
            format!("~${:.4}", a.cost)
        } else if a.cost > 0.0 {
            format!("~${:.4} (+{} unpriced)", a.cost, a.unknown)
        } else {
            "cost n/a".to_string()
        }
    };
    for (k, a) in &groups {
        println!(
            "  {:<28} {:>10} in  {:>9} out  {:>4} req  {}",
            k,
            a.tin,
            a.tout,
            a.reqs,
            fmt_cost(a)
        );
    }
    println!(
        "  {:<28} {:>10} in  {:>9} out  {:>4} req  {}",
        "TOTAL".to_string(),
        total.tin,
        total.tout,
        total.reqs,
        fmt_cost(&total)
    );
    0
}

/// `aishe runbook`: turn a recorded session (from the audit log) into a runnable
/// `.sh` script and a human `.md` runbook — or, with `--replay`, re-run the
/// recorded commands through the safety gate (never the model).
fn runbook_command(
    config: &Config,
    session: Option<&str>,
    out: Option<&str>,
    replay: bool,
) -> Result<u8> {
    let path = audit_log_path(config);
    let entries = aishe::audit::read_entries(&path);
    if entries.is_empty() {
        eprintln!(
            "aishe: no audit log at {} (enable it in [logging] or with AISHE_LOG=1)",
            path.display()
        );
        return Ok(0);
    }
    // Target session: the requested one, else the most recent recorded session.
    let session_id = match session {
        Some(s) => s.to_string(),
        None => entries
            .iter()
            .rev()
            .map(|e| e.session.clone())
            .find(|s| !s.is_empty())
            .unwrap_or_default(),
    };
    let rows: Vec<&aishe::audit::Entry> =
        entries.iter().filter(|e| e.session == session_id).collect();
    if rows.is_empty() {
        eprintln!("aishe: no entries for session '{session_id}'");
        return Ok(1);
    }
    // The request that started it (first ai_request prompt), and the commands run.
    let request = rows
        .iter()
        .find(|e| e.kind == "ai_request")
        .and_then(|e| e.text.clone());
    let commands: Vec<(String, Option<i64>)> = rows
        .iter()
        .filter(|e| e.kind == "action")
        .filter_map(|e| e.command.clone().map(|c| (c, e.exit)))
        .collect();

    if replay {
        return Ok(replay_commands(&commands));
    }

    if commands.is_empty() {
        eprintln!("aishe: session '{session_id}' ran no commands to export");
        return Ok(1);
    }
    let when = rows
        .first()
        .map(|e| aishe::audit::fmt_utc(e.ts_ms))
        .unwrap_or_default();
    let sh = render_runbook_sh(&session_id, &when, request.as_deref(), &commands);
    let md = render_runbook_md(&session_id, &when, request.as_deref(), &rows);

    let dir = std::path::PathBuf::from(out.unwrap_or("."));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }
    let base = format!("runbook-{}", session_id.replace(['/', ' '], "_"));
    let sh_path = dir.join(format!("{base}.sh"));
    let md_path = dir.join(format!("{base}.md"));
    if let Err(e) = std::fs::write(&sh_path, sh).and_then(|_| std::fs::write(&md_path, md)) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }
    println!("{} {}", "wrote".green(), sh_path.display());
    println!("{} {}", "wrote".green(), md_path.display());
    Ok(0)
}

/// Render the runnable script for a session's commands.
fn render_runbook_sh(
    session: &str,
    when: &str,
    request: Option<&str>,
    commands: &[(String, Option<i64>)],
) -> String {
    let mut s = String::from("#!/usr/bin/env bash\n");
    s.push_str(&format!(
        "# Runbook generated by aishe from audit session {session} ({when} UTC).\n"
    ));
    if let Some(r) = request {
        s.push_str(&format!("# Request: {}\n", r.lines().next().unwrap_or(r)));
    }
    s.push_str("# Review before running — these are the commands the AI ran, in order.\n");
    s.push_str("# Secrets are already redacted in the audit log they came from.\n");
    s.push_str("set -uo pipefail\n\n");
    for (cmd, exit) in commands {
        if matches!(exit, Some(c) if *c != 0) {
            s.push_str(&format!("# (exited {} when recorded)\n", exit.unwrap()));
        }
        s.push_str(cmd);
        s.push('\n');
    }
    s
}

/// Render the human-readable markdown runbook.
fn render_runbook_md(
    session: &str,
    when: &str,
    request: Option<&str>,
    rows: &[&aishe::audit::Entry],
) -> String {
    let title = request
        .and_then(|r| r.lines().next())
        .map(|l| l.to_string())
        .unwrap_or_else(|| format!("aishe session {session}"));
    let mut m = format!("# Runbook: {title}\n\n");
    m.push_str(&format!(
        "Generated by aishe from audit session `{session}` ({when} UTC).\n\n## Steps\n\n"
    ));
    let mut n = 0;
    for e in rows {
        match e.kind.as_str() {
            "action" => {
                if let Some(cmd) = &e.command {
                    n += 1;
                    let exit = e.exit.map(|c| format!(" → exit {c}")).unwrap_or_default();
                    let src = e.source.as_deref().unwrap_or("");
                    m.push_str(&format!("{n}. `{cmd}`{exit}  _({src})_\n"));
                }
            }
            "ai_response" => {
                if let Some(t) = &e.text {
                    if !t.is_empty() {
                        m.push_str(&format!("> {t}\n\n"));
                    }
                }
            }
            _ => {}
        }
    }
    m.push_str(&format!(
        "\n## Reproduce\n\n```sh\nbash runbook-{}.sh\n```\n",
        session.replace(['/', ' '], "_")
    ));
    m
}

/// `aishe runbook --replay`: re-run recorded commands through the safety gate.
/// Safe commands run; dangerous ones are skipped with a warning (the gate, not the
/// model, decides — so reproduction is deterministic and never re-prompts an LLM).
fn replay_commands(commands: &[(String, Option<i64>)]) -> u8 {
    if commands.is_empty() {
        println!("nothing to replay");
        return 0;
    }
    let mut executor = match Executor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aishe: {e}");
            return 1;
        }
    };
    let mut last = 0u8;
    for (cmd, _) in commands {
        match safety::assess(cmd) {
            Risk::Safe => {
                println!("{} {cmd}", "›".green());
                last = executor.run(cmd) as u8;
            }
            Risk::Dangerous(reason) => {
                eprintln!("{} skipped (dangerous: {reason}): {cmd}", "!".yellow());
            }
            // Replay is non-interactive by design, so an unresolvable command is
            // skipped rather than guessed at.
            Risk::Unknown(reason) => {
                eprintln!("{} skipped (unverifiable: {reason}): {cmd}", "!".yellow());
            }
        }
    }
    last
}

fn init_audit(config: &Config) {
    let env_log = std::env::var("AISHE_LOG").ok();
    let env_file = std::env::var("AISHE_LOG_FILE").ok();
    let (enabled, path) = resolve_audit(config, env_log.as_deref(), env_file.as_deref());
    aishe::audit::init(enabled, path, config.logging.redact);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_precedence_env_over_file() {
        let mut cfg = Config::default();

        // Off in the file, no env: stays off.
        cfg.logging.enabled = false;
        cfg.logging.file = None;
        let (on, path) = resolve_audit(&cfg, None, None);
        assert!(!on);
        assert!(path.is_none());

        // AISHE_LOG=1 turns it on even though the file says off.
        let (on, _) = resolve_audit(&cfg, Some("1"), None);
        assert!(on);
        // Non-truthy values do not enable it.
        let (on, _) = resolve_audit(&cfg, Some("0"), None);
        assert!(!on);

        // File enables it; env unset keeps it on.
        cfg.logging.enabled = true;
        let (on, _) = resolve_audit(&cfg, None, None);
        assert!(on);

        // AISHE_LOG_FILE overrides the configured path.
        cfg.logging.file = Some("/from/config.jsonl".into());
        let (_, path) = resolve_audit(&cfg, None, Some("/from/env.jsonl"));
        assert_eq!(path.unwrap(), std::path::PathBuf::from("/from/env.jsonl"));
        // Without the env var, the configured path wins.
        let (_, path) = resolve_audit(&cfg, None, None);
        assert_eq!(
            path.unwrap(),
            std::path::PathBuf::from("/from/config.jsonl")
        );
    }

    #[test]
    fn hook_command_handoff_is_exact_and_private() {
        let path = std::env::temp_dir().join(format!(
            "aishe-hook-handoff-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_file(&path);
        write_hook_command(&path, "run", "cd '/tmp/a b'\nprintf ok").unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "run\ncd '/tmp/a b'\nprintf ok\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(path);
    }
}
