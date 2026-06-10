//! aishe — a natural-language-aware shell.
//!
//! Behaves like zsh for recognizable commands; anything else is treated as a
//! natural-language request handled by an LLM (suggest or yolo mode).

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::style::Stylize;
use reedline::{
    default_emacs_keybindings, default_vi_insert_keybindings, default_vi_normal_keybindings,
    ColumnarMenu, EditMode, Emacs, FileBackedHistory, KeyCode, KeyModifiers, Keybindings, ListMenu,
    MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu, Signal, Vi,
};

use aishe::commands::CommandRegistry;
use aishe::completer::AisheCompleter;
use aishe::config::Config;
use aishe::dispatcher::{self, CommandCache, Dispatch};
use aishe::executor::Executor;
use aishe::highlight::CmdHighlighter;
use aishe::prompt::AishePrompt;
use aishe::providers::{self, Provider};
use aishe::safety::{self, Risk};
use aishe::session::Session;
use aishe::skills::SkillRegistry;
use aishe::theme::Theme;
use aishe::validator::AisheValidator;
use aishe::{context, history_expand, integration, modes};

/// Exit code from `--auto-line` when the suggested command is dangerous: the
/// shell hook treats any non-zero code as "pre-fill for review" instead of
/// running. (See `integration::ZSH_HOOK`.)
const EXIT_AUTO_DANGEROUS: u8 = 20;

/// Set by the SIGINT handler; checked by the yolo loop and reset around runs.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_sigint(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

#[derive(Parser, Debug)]
#[command(name = "aishe", version, about = "A natural-language-aware shell")]
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
    /// Use the zsh-PTY front-end: drive your real interactive zsh (with all
    /// native plugins) instead of the built-in reedline editor.
    #[arg(long)]
    pty: bool,
    /// Force the built-in reedline editor for this session, overriding the
    /// "auto" front-end (which otherwise prefers zsh-pty when zsh is present).
    #[arg(long = "no-pty")]
    no_pty: bool,
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
    Doctor,
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
    if matches!(args.cmd, Some(Cmd::Doctor)) {
        return Ok(doctor());
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
    if let Some(m) = &args.mode {
        config.aishe.mode = m.clone();
    }
    if let Some(p) = &args.provider {
        config.aishe.provider = p.clone();
    }
    if let Some(m) = &args.model {
        config.set_active_model(m.clone());
    }

    // Initialize the audit log (off unless enabled in config or via $AISHE_LOG).
    init_audit(&config);

    // Non-interactive invocations (`-c` and the shell-hook helpers) never use
    // the PTY front-end — they need the in-process executor/provider, not a
    // wrapped interactive zsh.
    let non_interactive = args.command.is_some()
        || args.suggest_line.is_some()
        || args.yolo_line.is_some()
        || args.auto_line.is_some();

    // zsh-PTY front-end: drive the user's real zsh. Smarts live in the injected
    // command_not_found hook, so we don't need an in-process executor/provider.
    // Resolution: an explicit `--pty`/`zsh`/`zsh-pty` wins; `--no-pty`/`reedline`
    // forces the built-in editor; the default "auto" picks zsh-pty whenever zsh
    // is on $PATH and falls back to reedline otherwise.
    let want_pty = !non_interactive
        && if args.pty || matches!(args.cmd, Some(Cmd::Zsh)) {
            true
        } else if args.no_pty {
            false
        } else {
            match config.aishe.front_end.as_str() {
                "zsh-pty" => true,
                "reedline" => false,
                _ => aishe::executor::which("zsh").is_some(),
            }
        };
    if want_pty {
        return aishe::pty::run_zsh(&config);
    }

    let mut executor = Executor::new()?;
    context::init(executor.shell());

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

    // Shell-hook helpers (called by `aishe init` integration).
    if let Some(line) = args.suggest_line {
        return suggest_line(&line, &mut executor, provider.as_deref(), &config);
    }
    if let Some(line) = args.yolo_line {
        return yolo_line(&line, &mut executor, provider.as_deref(), &config, &skills);
    }
    if let Some(line) = args.auto_line {
        return auto_line(&line, &mut executor, provider.as_deref(), &config);
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
        );
    }

    repl(
        &mut executor,
        &mut provider,
        &mut config,
        &cache,
        &commands,
        &skills,
    )
}

/// Environment check (`aishe doctor`): report shell, config, front-end,
/// provider, and API-key status. Returns 1 if a *critical* check fails (no
/// backing shell, or a malformed config); a missing API key is a warning only.
fn doctor() -> u8 {
    let ok = "✓".green();
    let bad = "✗".red();
    let warn = "!".yellow();
    let mut critical_ok = true;

    println!("{}", "aishe doctor".bold());
    println!("────────────");

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
    let cfg = match Config::load_quiet() {
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

    // Front-end resolution.
    let resolved = match cfg.aishe.front_end.as_str() {
        "zsh-pty" => "zsh-pty",
        "reedline" => "reedline",
        _ if zsh.is_some() => "zsh-pty (auto)",
        _ => "reedline (auto — zsh not found)",
    };
    println!(
        "{ok} front-end: {resolved}  [config: {}]",
        cfg.aishe.front_end
    );

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
    // Shell-hook calls are one per process, so there is no in-session memory.
    match modes::suggest::request(line, p, executor, config, Vec::new())? {
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            if !explanation.is_empty() {
                eprintln!("{}", explanation.as_str().dim());
            }
            println!("{command}");
            Ok(0)
        }
        modes::suggest::Suggestion::Answer { explanation } => {
            // No command to run; render the answer to stderr so the shell hook's
            // stdout capture stays empty.
            if !explanation.is_empty() {
                eprintln!("{explanation}");
            }
            Ok(0)
        }
    }
}

/// Shell-hook helper: run the yolo loop directly for a natural-language line.
fn yolo_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
) -> Result<u8> {
    let Some(p) = provider else {
        eprintln!("aishe: LLM not configured");
        return Ok(1);
    };
    let mut session = Session::new(false);
    modes::yolo::run(
        line,
        p,
        executor,
        config,
        &INTERRUPTED,
        skills,
        &mut session,
    )?;
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
    match modes::suggest::request(line, p, executor, config, Vec::new())? {
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            if !explanation.is_empty() {
                eprintln!("{}", explanation.as_str().dim());
            }
            println!("{command}");
            match safety::assess(&command) {
                Risk::Safe => Ok(0),
                Risk::Dangerous(reason) => {
                    eprintln!("{}", format!("⚠ {reason} — pre-filled for review").yellow());
                    Ok(EXIT_AUTO_DANGEROUS)
                }
            }
        }
        modes::suggest::Suggestion::Answer { explanation } => {
            if !explanation.is_empty() {
                eprintln!("{explanation}");
            }
            Ok(0)
        }
    }
}

/// Run one dispatch cycle non-interactively for the `-c` flag.
fn one_shot(
    input: &str,
    executor: &mut Executor,
    provider: &mut Option<Arc<dyn Provider>>,
    config: &Config,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
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
        &mut session,
    )? {
        return Ok(executor.last_exit as u8);
    }
    match dispatcher::dispatch(trimmed, cache) {
        Dispatch::Shell(line) => Ok(executor.run(&line) as u8),
        Dispatch::Builtin(tokens) => {
            if matches!(tokens[0].as_str(), "exit" | "quit") {
                return Ok(executor.last_exit as u8);
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

/// Add aishe's menu keybindings — Tab/Shift-Tab completion menu and Ctrl-R
/// history menu — to a keymap. Shared by the emacs and vi keymaps.
fn add_aishe_bindings(kb: &mut Keybindings) {
    kb.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    kb.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    // Ctrl-R → browsable, filterable history menu (upgrade over the default
    // single-line incremental search): type to filter, arrows to pick.
    kb.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('r'),
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("history_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
}

/// Build the reedline edit mode for the configured keymap. "vi" gives modal
/// editing (Esc for normal mode); anything else is emacs. aishe's menu bindings
/// are added to both vi sub-keymaps so completion/history work in either mode.
fn build_edit_mode(edit_mode: &str) -> Box<dyn EditMode> {
    if edit_mode == "vi" {
        let mut insert = default_vi_insert_keybindings();
        let mut normal = default_vi_normal_keybindings();
        add_aishe_bindings(&mut insert);
        add_aishe_bindings(&mut normal);
        Box::new(Vi::new(insert, normal))
    } else {
        let mut kb = default_emacs_keybindings();
        add_aishe_bindings(&mut kb);
        Box::new(Emacs::new(kb))
    }
}

fn repl(
    executor: &mut Executor,
    provider: &mut Option<Arc<dyn Provider>>,
    config: &mut Config,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
) -> Result<u8> {
    // One-time hint in the interactive shell if LLM features are unavailable.
    if provider.is_none() {
        let key_env = match config.aishe.provider.as_str() {
            "openai" => &config.providers.openai.api_key_env,
            _ => &config.providers.anthropic.api_key_env,
        };
        eprintln!(
            "{}",
            format!("aishe: LLM features disabled — set ${key_env} to enable").dim()
        );
    }

    let history_path = data_dir().join("history");
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let history = Box::new(
        FileBackedHistory::with_file(10_000, history_path)
            .unwrap_or_else(|_| FileBackedHistory::new(10_000).expect("in-memory history")),
    );

    // Tab → completion menu (command names / file paths), Shift-Tab → previous,
    // Ctrl-R → browsable history menu. Applied to whichever keymap is active.
    let edit_mode = build_edit_mode(&config.aishe.edit_mode);
    let theme = Theme::from_config(&config.theme);

    let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));
    let history_menu = Box::new(ListMenu::default().with_name("history_menu"));

    // Inline AI ghost-text autosuggestion (shares the provider, so its tokens
    // count in the session usage and budget). The worker only runs when a
    // provider exists; the hinter falls back to history hints otherwise.
    let ghost = aishe::ghost::Ghost::new(config.aishe.ghost_text, provider.clone(), config.clone());

    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(Box::new(
            AisheCompleter::new(cache.clone()).with_slash_commands(commands.list()),
        ))
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_menu(ReedlineMenu::HistoryMenu(history_menu))
        .with_validator(Box::new(AisheValidator::new(cache.clone())))
        .with_hinter(ghost.hinter(aishe::ghost::default_style()))
        .with_highlighter(Box::new(CmdHighlighter::new(cache.clone(), theme)))
        .with_edit_mode(edit_mode);

    // Conversation memory for natural-language turns this session.
    let mut session = Session::new(config.aishe.memory);

    loop {
        // Computed once per prompt (not per keystroke), reading .git/HEAD.
        let git = if config.aishe.git_prompt {
            aishe::prompt::git_segment(executor.cwd())
        } else {
            None
        };
        let prompt = AishePrompt::new(
            executor.cwd().clone(),
            &config.aishe.mode,
            executor.last_exit,
            config.active_model().to_string(),
            config.aishe.show_right_prompt,
            theme,
            config.aishe.prompt_format.as_deref(),
            git,
        );

        INTERRUPTED.store(false, Ordering::SeqCst);
        let sig = line_editor.read_line(&prompt);
        match sig {
            Ok(Signal::Success(buffer)) => {
                // The line is submitted; don't let the ghost worker predict for it.
                ghost.reset();
                let line = buffer.trim();
                if line.is_empty() {
                    continue;
                }
                // zsh-style history expansion (!!, !$, ^a^b, …) before dispatch.
                let hist: Vec<String> = executor.history.iter().map(|(c, _)| c.clone()).collect();
                let line: String = match history_expand::expand(line, &hist) {
                    Ok(Some(expanded)) => {
                        // Echo the expanded line, as zsh does.
                        println!("{}", expanded.as_str().dim());
                        expanded
                    }
                    Ok(None) => line.to_string(),
                    Err(e) => {
                        eprintln!("{}", format!("aishe: {e}").red());
                        continue;
                    }
                };
                if handle_line(
                    &line,
                    executor,
                    provider,
                    config,
                    cache,
                    commands,
                    skills,
                    &mut session,
                    &ghost,
                )? {
                    return Ok(executor.last_exit as u8);
                }
            }
            Ok(Signal::CtrlC) => {
                // Clear the line and re-prompt.
                continue;
            }
            Ok(Signal::CtrlD) => {
                println!("exit");
                return Ok(executor.last_exit as u8);
            }
            Err(e) => {
                eprintln!("aishe: input error: {e}");
                return Ok(1);
            }
        }
    }
}

/// Handle one input line. Returns Ok(true) if the shell should exit.
#[allow(clippy::too_many_arguments)]
fn handle_line(
    line: &str,
    executor: &mut Executor,
    provider: &mut Option<Arc<dyn Provider>>,
    config: &mut Config,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
    session: &mut Session,
    ghost: &aishe::ghost::Ghost,
) -> Result<bool> {
    // User-defined /slash-commands (plugins/skills) run before everything else.
    if try_custom_command(
        line,
        commands,
        executor,
        provider.as_deref(),
        config,
        skills,
        session,
    )? {
        return Ok(false);
    }

    // autocd: a bare directory name (that isn't a known command) means `cd`
    // there, like zsh's AUTO_CD.
    if let Some(dir) = autocd_target(line, executor.cwd(), cache) {
        executor.run_builtin(&["cd".to_string(), dir]);
        return Ok(false);
    }

    match dispatcher::dispatch(line, cache) {
        Dispatch::Shell(cmd) => {
            executor.run(&cmd);
            // A newly-defined alias/function must be recognized as a command on
            // later lines (the executor persists the definition via the rc).
            if let Some(name) = alias_name(&cmd) {
                cache.insert_all(&[name]);
            }
            if let Some(name) = dispatcher::function_def_name(&cmd) {
                cache.insert_all(&[&name]);
            }
        }
        Dispatch::Builtin(tokens) => match tokens[0].as_str() {
            "exit" | "quit" => return Ok(true),
            "aishe" => handle_meta(
                &tokens, config, provider, executor, cache, commands, skills, session, ghost,
            ),
            _ => {
                executor.run_builtin(&tokens);
            }
        },
        Dispatch::NaturalLanguage(nl) => {
            run_nl(
                &nl,
                &config.aishe.mode,
                provider.as_deref(),
                executor,
                config,
                skills,
                session,
            )?;
        }
    }
    Ok(false)
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
fn try_custom_command(
    line: &str,
    commands: &CommandRegistry,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
    session: &mut Session,
) -> Result<bool> {
    let Some((name, args)) = parse_slash(line) else {
        return Ok(false);
    };
    if dispatcher::is_meta_subcommand(name) {
        return Ok(false);
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
        run_nl(&ex.text, mode, provider, executor, config, skills, session)?;
    }
    Ok(true)
}

/// Run a natural-language request in the given mode.
fn run_nl(
    nl: &str,
    mode: &str,
    provider: Option<&dyn Provider>,
    executor: &mut Executor,
    config: &Config,
    skills: &SkillRegistry,
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
        "yolo" => modes::yolo::run(nl, p, executor, config, &INTERRUPTED, skills, session)?,
        "auto" => modes::suggest::run(nl, p, executor, config, false, true, session)?,
        _ => modes::suggest::run(nl, p, executor, config, false, false, session)?,
    }
    Ok(())
}

/// zsh `AUTO_CD`: if `line` is a bare token (no whitespace/sigil) that names an
/// existing directory and is *not* a known command, return it as a `cd` target.
fn autocd_target(line: &str, cwd: &std::path::Path, cache: &CommandCache) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || t.contains(char::is_whitespace) {
        return None;
    }
    if t.starts_with('?') || t.starts_with('!') || t.contains('=') {
        return None;
    }
    if cache.contains(t) {
        return None; // it's a command, not a directory to enter
    }
    let resolved: std::path::PathBuf = if t == "~" {
        dirs::home_dir()?
    } else if let Some(rest) = t.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else if t.starts_with('/') {
        std::path::PathBuf::from(t)
    } else {
        cwd.join(t)
    };
    resolved.is_dir().then(|| t.to_string())
}

/// Extract the alias name from an `alias NAME=...` line (single definition only).
fn alias_name(cmd: &str) -> Option<&str> {
    let t = cmd.trim();
    if t.contains(';') || t.contains('|') || t.contains('&') {
        return None;
    }
    let rest = t.strip_prefix("alias ")?.trim_start();
    let (name, _) = rest.split_once('=')?;
    let name = name.trim();
    (!name.is_empty() && !name.starts_with('-') && !name.contains(char::is_whitespace))
        .then_some(name)
}

/// Handle `aishe ...` meta commands.
#[allow(clippy::too_many_arguments)]
fn handle_meta(
    tokens: &[String],
    config: &mut Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &Executor,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
    session: &mut Session,
    ghost: &aishe::ghost::Ghost,
) {
    let sub = tokens.get(1).map(|s| s.as_str()).unwrap_or("help");
    match sub {
        "ghost" => match tokens.get(2).map(|s| s.as_str()) {
            Some("on") | Some("true") => {
                config.aishe.ghost_text = true;
                persist(config);
                ghost.set_enabled(true);
                if provider.is_none() {
                    println!("ghost-text → on (no provider configured; set your API key)");
                } else {
                    println!("ghost-text → on");
                }
            }
            Some("off") | Some("false") => {
                config.aishe.ghost_text = false;
                persist(config);
                ghost.set_enabled(false);
                println!("ghost-text → off");
            }
            Some(_) => eprintln!("aishe: ghost must be 'on' or 'off'"),
            None => println!(
                "ghost-text: {}",
                if ghost.is_enabled() { "on" } else { "off" }
            ),
        },
        "reset" => {
            let n = session.turns();
            session.clear();
            if session.enabled() {
                println!(
                    "conversation memory cleared ({n} turn{} forgotten)",
                    if n == 1 { "" } else { "s" }
                );
            } else {
                println!("conversation memory is off (set memory = true to enable)");
            }
        }
        "commands" => {
            if commands.is_empty() {
                println!("no custom commands (add *.md files to ~/.config/aishe/commands/)");
            } else {
                println!("custom slash-commands:");
                for (name, desc) in commands.list() {
                    println!("\x20 /{name}  —  {desc}");
                }
            }
        }
        "skills" => {
            if skills.is_empty() {
                println!("no skills (add <name>/SKILL.md files to ~/.config/aishe/skills/)");
            } else {
                println!("model-invoked skills (yolo mode):");
                for (name, desc) in skills.list() {
                    println!("\x20 {name}  —  {desc}");
                }
            }
        }
        "mode" => {
            if let Some(m) = tokens.get(2) {
                if matches!(m.as_str(), "suggest" | "auto" | "yolo") {
                    config.aishe.mode = m.clone();
                    persist(config);
                    println!("mode → {m}");
                } else {
                    eprintln!("aishe: mode must be 'suggest', 'auto', or 'yolo'");
                }
            } else {
                println!("mode: {}", config.aishe.mode);
            }
        }
        "model" => {
            if let Some(m) = tokens.get(2) {
                config.set_active_model(m.clone());
                persist(config);
                rebuild_provider(config, provider);
                println!("model → {m}");
            } else {
                println!("model: {}", config.active_model());
            }
        }
        "provider" => {
            if let Some(p) = tokens.get(2) {
                if p == "anthropic" || p == "openai" {
                    config.aishe.provider = p.clone();
                    persist(config);
                    rebuild_provider(config, provider);
                    println!("provider → {p}");
                } else {
                    eprintln!("aishe: provider must be 'anthropic' or 'openai'");
                }
            } else {
                println!("provider: {}", config.aishe.provider);
            }
        }
        "config" => {
            println!("config file: {}", Config::path().display());
            match toml::to_string_pretty(config) {
                Ok(t) => println!("\n{t}"),
                Err(e) => eprintln!("aishe: {e}"),
            }
        }
        "editor" => {
            if let Some(m) = tokens.get(2) {
                if matches!(m.as_str(), "emacs" | "vi") {
                    config.aishe.edit_mode = m.clone();
                    persist(config);
                    println!("editor → {m} (restart aishe to apply)");
                } else {
                    eprintln!("aishe: editor must be 'emacs' or 'vi'");
                }
            } else {
                println!("editor: {}", config.aishe.edit_mode);
            }
        }
        "stream" => match tokens.get(2).map(|s| s.as_str()) {
            Some("on") | Some("true") => {
                config.aishe.stream = true;
                persist(config);
                println!("stream → on");
            }
            Some("off") | Some("false") => {
                config.aishe.stream = false;
                persist(config);
                println!("stream → off");
            }
            Some(_) => eprintln!("aishe: stream must be 'on' or 'off'"),
            None => println!("stream: {}", if config.aishe.stream { "on" } else { "off" }),
        },
        "structured" => match tokens.get(2).map(|s| s.as_str()) {
            Some(v @ ("schema" | "json" | "prompt")) => {
                config.aishe.structured = v.to_string();
                persist(config);
                println!("structured → {v}");
            }
            Some(_) => eprintln!("aishe: structured must be 'schema', 'json', or 'prompt'"),
            None => println!("structured: {}", config.aishe.structured),
        },
        "frontend" => {
            if let Some(f) = tokens.get(2) {
                if matches!(f.as_str(), "auto" | "reedline" | "zsh-pty") {
                    config.aishe.front_end = f.clone();
                    persist(config);
                    println!("front-end → {f} (restart aishe to apply)");
                } else {
                    eprintln!("aishe: front-end must be 'auto', 'reedline', or 'zsh-pty'");
                }
            } else {
                println!("front-end: {}", config.aishe.front_end);
            }
        }
        "theme" => {
            if let Some(t) = tokens.get(2) {
                if aishe::theme::PRESETS.contains(&t.as_str()) {
                    config.theme.preset = Some(t.clone());
                    persist(config);
                    println!("theme → {t} (restart aishe to apply)");
                } else {
                    eprintln!(
                        "aishe: unknown theme '{t}' (presets: {})",
                        aishe::theme::PRESETS.join(", ")
                    );
                }
            } else {
                let cur = config.theme.preset.as_deref().unwrap_or("default");
                println!(
                    "theme: {cur}  (presets: {})",
                    aishe::theme::PRESETS.join(", ")
                );
            }
        }
        "rehash" => {
            cache.rehash(executor.shell());
            println!("rehashed ({} commands cached)", cache.len());
        }
        "usage" => print_usage_summary(provider.as_deref(), config),
        _ => print_meta_help(),
    }
}

/// Print the session token/cost summary (`aishe usage` / `/usage`).
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

fn print_meta_help() {
    println!(
        "aishe meta commands:\n\
\x20 aishe mode [suggest|auto|yolo]  show or set interaction mode\n\
\x20 aishe model [NAME]          show or set the model\n\
\x20 aishe provider [a|o]        show or set the provider\n\
\x20 aishe editor [emacs|vi]     show or set the line-editor keymap\n\
\x20 aishe frontend [auto|reedline|zsh-pty]  show or set the front-end\n\
\x20 aishe stream [on|off]       show or toggle token streaming\n\
\x20 aishe structured [schema|json|prompt]  output-format strategy\n\
\x20 aishe theme [PRESET]        show or set the color preset\n\
\x20 aishe commands              list custom /slash-commands\n\
\x20 aishe skills                list model-invoked skills (yolo)\n\
\x20 aishe usage                 session token & cost usage\n\
\x20 aishe reset                 clear conversation memory\n\
\x20 aishe ghost [on|off]        inline AI ghost-text autosuggestion\n\
\x20 aishe config                print active config\n\
\x20 aishe rehash                rebuild the command cache\n\
\x20 aishe help                  show this help\n\
\n\
(each also works as a slash-command, e.g. /mode auto, /config, /help)\n\
\n\
input prefixes:\n\
\x20 ?<text>   force natural-language\n\
\x20 !<cmd>    force shell (safety-exempt)\n\
\n\
exit with `exit`, `quit`, or Ctrl-D."
    );
}

fn rebuild_provider(config: &Config, provider: &mut Option<Arc<dyn Provider>>) {
    match providers::make(config) {
        Ok(p) => *provider = Some(p),
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").dim());
            *provider = None;
        }
    }
}

fn persist(config: &Config) {
    if let Err(e) = config.save() {
        eprintln!("{}", format!("aishe: could not save config: {e}").dim());
    }
}

fn data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("aishe")
}

/// Initialize the audit logger from config, with environment overrides:
/// `AISHE_LOG=1` forces it on, `AISHE_LOG_FILE` overrides the path.
fn init_audit(config: &Config) {
    let env_on = matches!(
        std::env::var("AISHE_LOG").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    );
    let enabled = config.logging.enabled || env_on;
    let path = std::env::var("AISHE_LOG_FILE")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| config.logging.file.clone().map(std::path::PathBuf::from));
    aishe::audit::init(enabled, path, config.logging.redact);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_name_extracts_single_definition() {
        assert_eq!(alias_name("alias g=git"), Some("g"));
        assert_eq!(alias_name("alias ll='ls -la'"), Some("ll"));
        assert_eq!(alias_name("alias"), None); // listing
        assert_eq!(alias_name("alias g=git; rm x"), None); // operators
        assert_eq!(alias_name("git status"), None);
    }

    #[test]
    fn autocd_only_for_bare_existing_dirs() {
        let base = std::env::temp_dir().join(format!("aishe-autocd-{}", std::process::id()));
        std::fs::create_dir_all(base.join("sub")).unwrap();
        std::fs::write(base.join("file.txt"), b"x").unwrap();
        let cache = CommandCache::new();
        cache.insert_all(&["ls", "sub_cmd"]);

        // a bare existing directory → cd target
        assert_eq!(autocd_target("sub", &base, &cache).as_deref(), Some("sub"));
        // a file is not a directory
        assert_eq!(autocd_target("file.txt", &base, &cache), None);
        // a known command is never treated as autocd, even if a dir exists
        std::fs::create_dir_all(base.join("ls")).unwrap();
        assert_eq!(autocd_target("ls", &base, &cache), None);
        // multi-token, sigils, and assignments are excluded
        assert_eq!(autocd_target("sub other", &base, &cache), None);
        assert_eq!(autocd_target("?sub", &base, &cache), None);
        assert_eq!(autocd_target("FOO=sub", &base, &cache), None);

        std::fs::remove_dir_all(&base).ok();
    }
}
