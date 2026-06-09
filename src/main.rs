//! llmsh — a natural-language-aware shell.
//!
//! Behaves like zsh for recognizable commands; anything else is treated as a
//! natural-language request handled by an LLM (suggest or yolo mode).

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::style::Stylize;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, DefaultHinter, Emacs, FileBackedHistory, KeyCode,
    KeyModifiers, MenuBuilder, Reedline, ReedlineEvent, ReedlineMenu, Signal,
};

use llmsh::completer::LlmshCompleter;
use llmsh::config::Config;
use llmsh::dispatcher::{self, CommandCache, Dispatch};
use llmsh::executor::Executor;
use llmsh::highlight::CmdHighlighter;
use llmsh::prompt::LlmshPrompt;
use llmsh::providers::{self, Provider};
use llmsh::safety::{self, Risk};
use llmsh::theme::Theme;
use llmsh::{context, integration, modes};

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
#[command(name = "llmsh", version, about = "A natural-language-aware shell")]
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
    /// Print a shell integration snippet: `eval "$(llmsh init zsh)"`.
    Init {
        /// Shell to emit integration for (zsh or bash).
        shell: String,
    },
    /// Launch your real interactive zsh (with all native plugins) under llmsh.
    Zsh,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("{}", format!("llmsh: {e}").red());
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<u8> {
    let args = Args::parse();

    // `init <shell>` needs no config or provider.
    if let Some(Cmd::Init { shell }) = &args.cmd {
        return match integration::script(shell) {
            Some(s) => {
                print!("{s}");
                Ok(0)
            }
            None => {
                eprintln!(
                    "llmsh: no integration for '{shell}' (supported: {})",
                    integration::SUPPORTED.join(", ")
                );
                Ok(1)
            }
        };
    }

    let mut config = Config::load_or_init()?;
    if let Some(m) = &args.mode {
        config.llmsh.mode = m.clone();
    }
    if let Some(p) = &args.provider {
        config.llmsh.provider = p.clone();
    }
    if let Some(m) = &args.model {
        config.set_active_model(m.clone());
    }

    // zsh-PTY front-end: drive the user's real zsh. Smarts live in the injected
    // command_not_found hook, so we don't need an in-process executor/provider.
    let want_pty =
        args.pty || matches!(args.cmd, Some(Cmd::Zsh)) || config.llmsh.front_end == "zsh-pty";
    if want_pty {
        return llmsh::pty::run_zsh(&config);
    }

    let mut executor = Executor::new()?;
    context::init(executor.shell());

    let cache = CommandCache::new();
    cache.build(executor.shell());

    // Build the provider lazily-ish: report errors but keep the shell usable.
    let mut provider: Option<Box<dyn Provider>> = match providers::make(&config) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("{}", format!("llmsh: LLM disabled — {e}").dim());
            None
        }
    };

    // Install a non-fatal SIGINT handler (see INTERRUPTED docs).
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_sigint as *const () as libc::sighandler_t,
        );
    }

    // Shell-hook helpers (called by `llmsh init` integration).
    if let Some(line) = args.suggest_line {
        return suggest_line(&line, &mut executor, provider.as_deref(), &config);
    }
    if let Some(line) = args.yolo_line {
        return yolo_line(&line, &mut executor, provider.as_deref(), &config);
    }
    if let Some(line) = args.auto_line {
        return auto_line(&line, &mut executor, provider.as_deref(), &config);
    }

    // Non-interactive single-shot mode (-c).
    if let Some(input) = args.command {
        return one_shot(&input, &mut executor, &mut provider, &config, &cache);
    }

    repl(&mut executor, &mut provider, &mut config, &cache)
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
        eprintln!("llmsh: LLM not configured");
        return Ok(1);
    };
    match modes::suggest::request(line, p, executor, config)? {
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
) -> Result<u8> {
    let Some(p) = provider else {
        eprintln!("llmsh: LLM not configured");
        return Ok(1);
    };
    modes::yolo::run(line, p, executor, config, &INTERRUPTED)?;
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
        eprintln!("llmsh: LLM not configured");
        return Ok(1);
    };
    match modes::suggest::request(line, p, executor, config)? {
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
    provider: &mut Option<Box<dyn Provider>>,
    config: &Config,
    cache: &CommandCache,
) -> Result<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    match dispatcher::dispatch(trimmed, cache) {
        Dispatch::Shell(line) => Ok(executor.run(&line) as u8),
        Dispatch::Builtin(tokens) => {
            if matches!(tokens[0].as_str(), "exit" | "quit") {
                return Ok(executor.last_exit as u8);
            }
            if tokens[0] == "llmsh" {
                // Meta commands are no-ops worth nothing in -c; print help-ish.
                println!("llmsh meta commands are interactive-only");
                return Ok(0);
            }
            Ok(executor.run_builtin(&tokens) as u8)
        }
        Dispatch::NaturalLanguage(nl) => match provider {
            Some(p) => {
                if config.llmsh.mode == "yolo" {
                    modes::yolo::run(&nl, p.as_ref(), executor, config, &INTERRUPTED)?;
                } else {
                    // -c + NL in suggest/auto mode: print suggested command, don't run.
                    modes::suggest::run(&nl, p.as_ref(), executor, config, true, false)?;
                }
                Ok(0)
            }
            None => {
                eprintln!("llmsh: LLM not configured");
                Ok(1)
            }
        },
    }
}

fn repl(
    executor: &mut Executor,
    provider: &mut Option<Box<dyn Provider>>,
    config: &mut Config,
    cache: &CommandCache,
) -> Result<u8> {
    let history_path = data_dir().join("history");
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let history = Box::new(
        FileBackedHistory::with_file(10_000, history_path)
            .unwrap_or_else(|_| FileBackedHistory::new(10_000).expect("in-memory history")),
    );

    // Tab → completion menu (command names / file paths), Shift-Tab → previous.
    let mut keybindings = default_emacs_keybindings();
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::BackTab,
        ReedlineEvent::MenuPrevious,
    );
    let edit_mode = Box::new(Emacs::new(keybindings));
    let theme = Theme::from_config(&config.theme);

    let completion_menu = Box::new(ColumnarMenu::default().with_name("completion_menu"));

    let mut line_editor = Reedline::create()
        .with_history(history)
        .with_completer(Box::new(LlmshCompleter::new(cache.clone())))
        .with_menu(ReedlineMenu::EngineCompleter(completion_menu))
        .with_hinter(Box::new(DefaultHinter::default()))
        .with_highlighter(Box::new(CmdHighlighter::new(cache.clone(), theme)))
        .with_edit_mode(edit_mode);

    loop {
        let prompt = LlmshPrompt::new(
            executor.cwd().clone(),
            &config.llmsh.mode,
            executor.last_exit,
            config.active_model().to_string(),
            config.llmsh.show_right_prompt,
            theme,
        );

        INTERRUPTED.store(false, Ordering::SeqCst);
        let sig = line_editor.read_line(&prompt);
        match sig {
            Ok(Signal::Success(buffer)) => {
                let line = buffer.trim();
                if line.is_empty() {
                    continue;
                }
                if handle_line(line, executor, provider, config, cache)? {
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
                eprintln!("llmsh: input error: {e}");
                return Ok(1);
            }
        }
    }
}

/// Handle one input line. Returns Ok(true) if the shell should exit.
fn handle_line(
    line: &str,
    executor: &mut Executor,
    provider: &mut Option<Box<dyn Provider>>,
    config: &mut Config,
    cache: &CommandCache,
) -> Result<bool> {
    match dispatcher::dispatch(line, cache) {
        Dispatch::Shell(cmd) => {
            executor.run(&cmd);
        }
        Dispatch::Builtin(tokens) => match tokens[0].as_str() {
            "exit" | "quit" => return Ok(true),
            "llmsh" => handle_meta(&tokens, config, provider, executor, cache),
            _ => {
                executor.run_builtin(&tokens);
            }
        },
        Dispatch::NaturalLanguage(nl) => {
            let Some(p) = provider.as_deref() else {
                eprintln!(
                    "{}",
                    "llmsh: LLM not configured — set your API key env var".dim()
                );
                return Ok(false);
            };
            match config.llmsh.mode.as_str() {
                "yolo" => modes::yolo::run(&nl, p, executor, config, &INTERRUPTED)?,
                "auto" => modes::suggest::run(&nl, p, executor, config, false, true)?,
                _ => modes::suggest::run(&nl, p, executor, config, false, false)?,
            }
        }
    }
    Ok(false)
}

/// Handle `llmsh ...` meta commands.
fn handle_meta(
    tokens: &[String],
    config: &mut Config,
    provider: &mut Option<Box<dyn Provider>>,
    executor: &Executor,
    cache: &CommandCache,
) {
    let sub = tokens.get(1).map(|s| s.as_str()).unwrap_or("help");
    match sub {
        "mode" => {
            if let Some(m) = tokens.get(2) {
                if matches!(m.as_str(), "suggest" | "auto" | "yolo") {
                    config.llmsh.mode = m.clone();
                    persist(config);
                    println!("mode → {m}");
                } else {
                    eprintln!("llmsh: mode must be 'suggest', 'auto', or 'yolo'");
                }
            } else {
                println!("mode: {}", config.llmsh.mode);
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
                    config.llmsh.provider = p.clone();
                    persist(config);
                    rebuild_provider(config, provider);
                    println!("provider → {p}");
                } else {
                    eprintln!("llmsh: provider must be 'anthropic' or 'openai'");
                }
            } else {
                println!("provider: {}", config.llmsh.provider);
            }
        }
        "config" => {
            println!("config file: {}", Config::path().display());
            match toml::to_string_pretty(config) {
                Ok(t) => println!("\n{t}"),
                Err(e) => eprintln!("llmsh: {e}"),
            }
        }
        "rehash" => {
            cache.rehash(executor.shell());
            println!("rehashed ({} commands cached)", cache.len());
        }
        _ => print_meta_help(),
    }
}

fn print_meta_help() {
    println!(
        "llmsh meta commands:\n\
\x20 llmsh mode [suggest|auto|yolo]  show or set interaction mode\n\
\x20 llmsh model [NAME]          show or set the model\n\
\x20 llmsh provider [a|o]        show or set the provider\n\
\x20 llmsh config                print active config\n\
\x20 llmsh rehash                rebuild the command cache\n\
\x20 llmsh help                  show this help\n\
\n\
input prefixes:\n\
\x20 ?<text>   force natural-language\n\
\x20 !<cmd>    force shell (safety-exempt)\n\
\n\
exit with `exit`, `quit`, or Ctrl-D."
    );
}

fn rebuild_provider(config: &Config, provider: &mut Option<Box<dyn Provider>>) {
    match providers::make(config) {
        Ok(p) => *provider = Some(p),
        Err(e) => {
            eprintln!("{}", format!("llmsh: {e}").dim());
            *provider = None;
        }
    }
}

fn persist(config: &Config) {
    if let Err(e) = config.save() {
        eprintln!("{}", format!("llmsh: could not save config: {e}").dim());
    }
}

fn data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("llmsh")
}
