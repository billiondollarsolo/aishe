//! aishe — a natural-language-aware shell.
//!
//! Behaves like zsh for recognizable commands; anything else is treated as a
//! natural-language request handled by an LLM (suggest or yolo mode).

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::style::Stylize;

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

/// Set by the SIGINT handler; checked by the yolo loop and reset around runs.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigint(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Hard wall-clock budget for the prompt-blocking shell hooks (`--suggest-line`,
/// `--auto-line`). On a dead/slow network these would otherwise hang the user's
/// prompt for the provider read timeout plus retries; instead we arm a SIGALRM
/// and bail out cleanly when it fires. See [`arm_hook_budget`].
const HOOK_BUDGET_SECS: u32 = 15;

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

/// Install the SIGALRM handler and arm a `HOOK_BUDGET_SECS` alarm. The caller is
/// responsible for cancelling it (`libc::alarm(0)`) before returning normally.
fn arm_hook_budget() {
    unsafe {
        libc::signal(
            libc::SIGALRM,
            handle_hook_alarm as *const () as libc::sighandler_t,
        );
        libc::alarm(HOOK_BUDGET_SECS);
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
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
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
    },
    /// Print a shell completion script for `aishe` itself (bash/zsh/fish/...).
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
    /// Trust the current project's `.aishe/config.toml` so its sensitive keys
    /// (provider/endpoint, MCP servers, audit logging, safety toggles, `yolo`)
    /// apply. Safe cosmetic keys apply without trust.
    Trust {
        /// List all trusted project configs instead of trusting this one.
        #[arg(long)]
        list: bool,
    },
    /// Drop trust for the current project's `.aishe/config.toml`.
    Untrust {
        /// Drop trust for every project config, not just this one.
        #[arg(long)]
        all: bool,
    },
    /// Show or set the interaction mode (with a value, saves it to your config).
    Mode {
        #[arg(value_parser = ["suggest", "auto", "yolo"])]
        value: Option<String>,
    },
    /// Show or set the model for the active provider (saves it to your config).
    Model { value: Option<String> },
    /// Show or set the provider (with a value, saves it to your config).
    Provider {
        #[arg(value_parser = ["anthropic", "openai"])]
        value: Option<String>,
    },
    /// Print the active configuration.
    Config,
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
    /// Semantic search over your shell history (opt-in; needs an embedder).
    History {
        #[command(subcommand)]
        cmd: HistoryCmd,
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
    /// Print the environment context block aishe sends to the model (redacted).
    Context,
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

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").red());
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8> {
    let args = Args::parse();

    // `doctor` inspects the environment without loading/initializing config.
    if let Some(Cmd::Doctor { probe }) = args.cmd {
        return Ok(doctor(probe));
    }

    // `completions <shell>` prints a completion script and exits.
    if let Some(Cmd::Completions { shell }) = args.cmd {
        use clap::CommandFactory;
        clap_complete::generate(shell, &mut Args::command(), "aishe", &mut std::io::stdout());
        return Ok(0);
    }

    // `trust` / `untrust` manage the project-config trust store; no config load.
    if let Some(Cmd::Trust { list }) = &args.cmd {
        return Ok(trust_command(*list));
    }
    if let Some(Cmd::Untrust { all }) = &args.cmd {
        return Ok(untrust_command(*all));
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

    // First-class inspection / settings subcommands. They print (or persist a
    // setting) and exit, so they work the same in the zsh-PTY, a bare shell, or a
    // script. The inspectors show the *effective* config (project overlay + flags
    // applied); the setters write to the user config file (see `set_or_show`).
    match &args.cmd {
        Some(Cmd::Config) => {
            println!("config file: {}", Config::path().display());
            match toml::to_string_pretty(&config) {
                Ok(t) => println!("\n{t}"),
                Err(e) => eprintln!("aishe: {e}"),
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
                println!("no custom commands (add *.md files to ~/.config/aishe/commands/)");
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
                println!("no skills (add <name>/SKILL.md files to ~/.config/aishe/skills/)");
            } else {
                println!("model-invoked skills (yolo mode):");
                for (name, desc) in skills.list() {
                    println!("\x20 {name}  —  {desc}");
                }
            }
            return Ok(0);
        }
        Some(Cmd::Mode { value }) => return Ok(set_or_show("mode", value.as_deref(), &config)),
        Some(Cmd::Model { value }) => return Ok(set_or_show("model", value.as_deref(), &config)),
        Some(Cmd::Provider { value }) => {
            return Ok(set_or_show("provider", value.as_deref(), &config))
        }
        Some(Cmd::Log {
            session,
            action,
            model,
            since,
            limit,
            json,
        }) => {
            return Ok(log_command(
                &config,
                session.as_deref(),
                action.as_deref(),
                model.as_deref(),
                since.as_deref(),
                *limit,
                *json,
            ));
        }
        Some(Cmd::Usage { by, since }) => {
            return Ok(usage_history_command(
                &config,
                by.as_deref(),
                since.as_deref(),
            ));
        }
        Some(Cmd::Context) => {
            let executor = Executor::new()?;
            context::init(executor.shell());
            print!("{}", context::build(&executor, &config));
            return Ok(0);
        }
        Some(Cmd::Runbook {
            session,
            out,
            replay,
        }) => {
            return runbook_command(&config, session.as_deref(), out.as_deref(), *replay);
        }
        Some(Cmd::History { cmd }) => {
            return history_command(&config, cmd);
        }
        Some(Cmd::DryRun { command, apply }) => {
            return dry_run_command(command, *apply);
        }
        _ => {}
    }

    // Tell an interactive user what a project config did (and how to trust it).
    let interactive_entry = args.command.is_none()
        && args.suggest_line.is_none()
        && args.yolo_line.is_none()
        && args.auto_line.is_none()
        && args.fix_line.is_none()
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
        || args.fix_line.is_some();

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
    // MCP servers (extra yolo tools). Empty/instant unless `[mcp_servers]` is set.
    let mcp = aishe::mcp::McpRegistry::connect(&config.mcp_servers);

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

/// Environment check (`aishe doctor`): report shell, config, front-end,
/// provider, and API-key status. Returns 1 if a *critical* check fails (no
/// backing shell, or a malformed config); a missing API key is a warning only.
fn doctor(probe: bool) -> u8 {
    let ok = "✓".green();
    let bad = "✗".red();
    let warn = "!".yellow();
    let mut critical_ok = true;

    println!("{}", "aishe doctor".bold());
    println!("────────────");
    println!("{ok} version: aishe {VERSION}");

    // Backing shell.
    let zsh = aishe::executor::which("zsh");
    let bash = aishe::executor::which("bash");
    match (&zsh, &bash) {
        (Some(z), _) => println!("{ok} backing shell: zsh ({})", z.display()),
        (None, Some(b)) => println!("{ok} backing shell: bash ({}) — zsh not found", b.display()),
        (None, None) => {
            println!("{bad} backing shell: none found (install zsh or bash)");
            critical_ok = false;
        }
    }

    // Config.
    let mut cfg = match Config::load_quiet() {
        Ok(Some(c)) => {
            println!("{ok} config: {}", Config::path().display());
            c
        }
        Ok(None) => {
            println!("{warn} config: not created yet (run `aishe` once to set up)");
            Config::default()
        }
        Err(e) => {
            println!(
                "{bad} config: malformed at {} ({e})",
                Config::path().display()
            );
            critical_ok = false;
            Config::default()
        }
    };

    // Project-local config overlay (`.aishe/config.toml`), if any. Apply it to
    // `cfg` so the provider/model/front-end lines below reflect what will
    // actually be used in this directory.
    if let Ok(cwd) = std::env::current_dir() {
        match cfg.apply_project_overlay(&cwd) {
            Some(o) if o.error.is_some() => {
                println!(
                    "{warn} project config: malformed at {} ({})",
                    o.path.display(),
                    o.error.as_deref().unwrap_or("")
                );
            }
            Some(o) => {
                let trust = if o.trusted { "trusted" } else { "untrusted" };
                println!(
                    "{ok} project config: {} ({trust}; {} applied, {} deferred)",
                    o.path.display(),
                    o.applied.len(),
                    o.deferred.len()
                );
                if !o.deferred.is_empty() {
                    println!(
                        "{warn} deferred until trusted (`aishe trust`): {}",
                        o.deferred.join(", ")
                    );
                }
            }
            None => println!("{ok} project config: none"),
        }
    }

    // Front-end: the interactive shell is the zsh-PTY wrapper, which requires zsh.
    if zsh.is_some() {
        println!("{ok} front-end: zsh-pty (wraps your real zsh)");
    } else {
        println!(
            "{bad} front-end: zsh-pty needs zsh, which is not installed. \
             Install it for the interactive shell; `aishe -c …` and the bash hook \
             still work without it."
        );
        critical_ok = false;
    }

    // Provider, model, and API key.
    let (provider, model, key_env) = match cfg.aishe.provider.as_str() {
        "openai" => (
            "openai",
            &cfg.providers.openai.model,
            &cfg.providers.openai.api_key_env,
        ),
        _ => (
            "anthropic",
            &cfg.providers.anthropic.model,
            &cfg.providers.anthropic.api_key_env,
        ),
    };
    println!("{ok} provider: {provider} · model {model}");
    if !cfg.aishe.provider_fallback.is_empty() {
        println!(
            "{ok} fallback chain: {} → {}",
            provider,
            cfg.aishe.provider_fallback.join(" → ")
        );
    }
    // Reachability probe (opt-in `--probe`): one short, read-only request per chain
    // member, so "offline-capable" / fallback claims are verifiable. Unreachable or
    // key-rejected members are warnings, not critical: a fallback may legitimately
    // be down, and `doctor` should still pass offline.
    if probe {
        println!("  {}", "reachability probe:".bold());
        for name in providers::chain_names(&cfg) {
            let pr = providers::probe(&cfg, &name);
            match pr.reach {
                providers::Reach::Up(s) => {
                    println!("  {ok} {name}: reachable [HTTP {s}] ({})", pr.endpoint)
                }
                providers::Reach::Unauthorized(s) => println!(
                    "  {warn} {name}: reachable but key rejected [HTTP {s}] ({})",
                    pr.endpoint
                ),
                providers::Reach::Down(e) => {
                    println!("  {warn} {name}: unreachable ({}) — {e}", pr.endpoint)
                }
            }
        }
    }
    // Yolo sandbox backend.
    if cfg.aishe.yolo_sandbox {
        if cfg.aishe.sandbox_backend == "bwrap" {
            if aishe::sandbox::bwrap_available() {
                println!("{ok} yolo sandbox: bwrap (OS isolation: read-only root)");
            } else {
                println!(
                    "{warn} yolo sandbox: bwrap requested but bubblewrap not installed — using policy"
                );
            }
        } else {
            println!("{ok} yolo sandbox: policy (best-effort gate)");
        }
    }
    // Reversible command preview (`aishe dry-run`), which needs bubblewrap.
    if aishe::overlay::available() {
        println!("{ok} dry-run: available (bubblewrap)");
    } else {
        println!("{warn} dry-run: needs bubblewrap (install it to use `aishe dry-run`)");
    }
    let key_set = std::env::var(key_env)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if key_set {
        println!("{ok} API key: ${key_env} is set");
    } else {
        println!("{warn} API key: ${key_env} not set — LLM features disabled (export it)");
    }

    // Privacy: secret redaction and audit logging.
    println!(
        "{ok} secret redaction: {}",
        if cfg.aishe.redact_secrets {
            "on"
        } else {
            "off"
        }
    );
    let env_log = matches!(
        std::env::var("AISHE_LOG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    );
    if cfg.logging.enabled || env_log {
        let path = std::env::var("AISHE_LOG_FILE").ok().unwrap_or_else(|| {
            cfg.logging
                .file
                .clone()
                .unwrap_or_else(|| aishe::audit::default_path().display().to_string())
        });
        println!(
            "{ok} audit log: on ({path}; redact {})",
            if cfg.logging.redact { "on" } else { "off" }
        );
    } else {
        println!("{warn} audit log: off (enable with [logging] enabled=true or AISHE_LOG=1)");
    }

    // MCP servers and history.
    let enabled_mcp = cfg.mcp_servers.values().filter(|s| s.enabled).count();
    if cfg.mcp_servers.is_empty() {
        println!("{ok} MCP servers: none configured");
    } else {
        println!(
            "{ok} MCP servers: {} configured ({enabled_mcp} enabled) — `aishe mcp` to list",
            cfg.mcp_servers.len()
        );
    }
    let (_, hist_log) = history_paths(&cfg);
    println!(
        "{ok} history: {} ({})",
        hist_log.display(),
        if cfg.aishe.share_history {
            "shared"
        } else {
            "per-session"
        }
    );

    println!();
    if critical_ok {
        println!("{}", "all critical checks passed".green());
        0
    } else {
        println!("{}", "some checks failed".red());
        1
    }
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

fn suggest_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let Some(p) = provider else {
        eprintln!("aishe: LLM not configured");
        return Ok(1);
    };
    // Bound the blocking LLM call so a dead/slow network can't freeze the prompt.
    arm_hook_budget();
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
    let reply = match &suggestion {
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
    };
    if let Some(path) = &mem {
        session.record_user(line);
        session.record_assistant(&reply);
        session.save_persisted(path);
    }
    Ok(0)
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
    let Some(p) = provider else {
        eprintln!("aishe: LLM not configured");
        return Ok(1);
    };
    let exit = std::env::var("AISHE_LAST_EXIT").unwrap_or_else(|_| "unknown".to_string());
    let ctx = aishe::fix::error_context(cmd, config.aishe.fix_capture_stderr);
    let prompt = aishe::fix::build_prompt(cmd, &exit, ctx.as_deref());

    arm_hook_budget();
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
    let Some(p) = provider else {
        eprintln!("aishe: LLM not configured");
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

/// Shell-hook helper for `auto` mode: get a suggestion, print the command to
/// stdout, and signal safety via the exit code so the hook can decide whether to
/// run it directly (`eval`) or pre-fill it for review.
///
/// - Answer (no command): nothing on stdout, exit 0.
/// - Safe command: command on stdout, exit 0 (hook runs it).
/// - Dangerous command: command on stdout + reason on stderr, exit
///   `EXIT_AUTO_DANGEROUS` (hook pre-fills it instead).
fn auto_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let Some(p) = provider else {
        eprintln!("aishe: LLM not configured");
        return Ok(1);
    };
    // Bound the blocking LLM call so a dead/slow network can't freeze the prompt.
    arm_hook_budget();
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
            println!("{command}");
            let code = match safety::assess(command) {
                Risk::Safe => 0,
                Risk::Dangerous(reason) => {
                    eprintln!("{}", format!("⚠ {reason} — pre-filled for review").yellow());
                    EXIT_AUTO_DANGEROUS
                }
            };
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
                                "no custom commands (add *.md files to ~/.config/aishe/commands/)"
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
                                "no skills (add <name>/SKILL.md files to ~/.config/aishe/skills/)"
                            );
                        } else {
                            println!("model-invoked skills (yolo mode):");
                            for (name, desc) in skills.list() {
                                println!("\x20 {name}  —  {desc}");
                            }
                        }
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
        Dispatch::NaturalLanguage(nl) => match provider {
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
                eprintln!("aishe: LLM not configured");
                Ok(1)
            }
        },
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
        executor.run(&ex.text);
    } else {
        let mode = ex.mode.as_deref().unwrap_or(config.aishe.mode.as_str());
        run_nl(
            &ex.text, mode, provider, executor, config, skills, mcp, session,
        )?;
    }
    Ok(true)
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
    let Some(p) = provider else {
        eprintln!(
            "{}",
            "aishe: LLM not configured — set your API key env var".dim()
        );
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
        "mode" => cfg.aishe.mode = value.to_string(),
        "provider" => cfg.aishe.provider = value.to_string(),
        _ => cfg.set_active_model(value.to_string()),
    }
    if let Err(e) = cfg.save() {
        eprintln!("aishe: {e}");
        return 1;
    }
    println!("{field} = {value}  (saved to {})", Config::path().display());
    0
}

fn data_dir() -> std::path::PathBuf {
    dirs::data_dir()
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
            "aishe: dry-run needs bubblewrap (bwrap) for safe isolation — install it \
             (apt install bubblewrap | dnf install bubblewrap | brew install bubblewrap)."
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
fn trust_command(list: bool) -> u8 {
    if list {
        let items = aishe::trust::list();
        if items.is_empty() {
            println!("No trusted project configs.");
        } else {
            println!("Trusted project configs:");
            for (path, _) in items {
                println!("  {path}");
            }
        }
        return 0;
    }
    let Some(path) = current_project_config() else {
        return 1;
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("aishe: {e}");
            return 1;
        }
    };
    // Report which sensitive keys trusting will unlock.
    let deferred = match toml::from_str::<toml::Table>(&text) {
        Ok(table) => Config::default().merge_project_table(&table, false).1,
        Err(e) => {
            eprintln!("aishe: malformed project config {}: {e}", path.display());
            return 1;
        }
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
fn untrust_command(all: bool) -> u8 {
    if all {
        return match aishe::trust::untrust_all() {
            Ok(n) => {
                println!("Dropped trust for {n} project config(s).");
                0
            }
            Err(e) => {
                eprintln!("aishe: {e}");
                1
            }
        };
    }
    let Some(path) = current_project_config() else {
        return 1;
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
}
