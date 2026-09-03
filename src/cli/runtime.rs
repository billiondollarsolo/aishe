//! Non-interactive command execution and shell-hook orchestration.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::agent::controller::{TurnFailure, TurnOptions, TurnOutcome, INTERRUPTED};
use crate::agent::Mode as AgentMode;
use crate::command_surface::{self, ArgumentPolicy, Lifecycle, Surface, SurfaceSupport};
use crate::commands::CommandRegistry;
use crate::config::Config;
use crate::dispatcher::{self, CommandCache, Dispatch};
use crate::executor::Executor;
use crate::modes;
use crate::providers::{Msg, Provider, ResponseFormat};
use crate::safety::{self, Risk};
use crate::session::Session;
use crate::skills::SkillRegistry;
use crate::ui::SemanticStylize;

const EXIT_AUTO_DANGEROUS: u8 = 20;
const EXIT_COMMAND_UNAVAILABLE: u8 = 2;

extern "C" fn handle_sigint(_sig: libc::c_int) {
    INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Install the process-wide non-fatal interrupt handler used by native turns.
pub fn install_sigint_handler() {
    unsafe {
        libc::signal(
            libc::SIGINT,
            handle_sigint as *const () as libc::sighandler_t,
        );
    }
}

fn hook_budget_secs(config: &Config) -> u32 {
    config.aishe.hook_timeout_secs.clamp(1, 600)
}

extern "C" fn handle_hook_alarm(_sig: libc::c_int) {
    const MSG: &[u8] = b"aishe: suggestion timed out\n";
    unsafe {
        libc::write(2, MSG.as_ptr() as *const libc::c_void, MSG.len());
        libc::_exit(0);
    }
}

fn arm_hook_budget(config: &Config) {
    unsafe {
        libc::signal(
            libc::SIGALRM,
            handle_hook_alarm as *const () as libc::sighandler_t,
        );
        libc::alarm(hook_budget_secs(config));
    }
}

fn cancel_hook_budget() {
    unsafe {
        libc::alarm(0);
    }
}

/// Intercept a conservative local command-name correction before a hidden
/// shell-hook request can construct a provider, backend, or MCP client.
///
/// A matching typo never executes and is never transmitted. The cue is written
/// to stderr at most once for each typo head in a live shell; repeated matches
/// are silently intercepted. Explicit `?` input is ineligible in the dispatcher
/// itself and therefore always reaches the requested agent route.
pub fn intercept_hook_typo(line: &str, cache: &CommandCache) -> Result<bool> {
    let Some(assistance) = dispatcher::typo_assistance(line, cache) else {
        return Ok(false);
    };

    let should_emit = match acceptance_path()? {
        Some(path) => record_typo_once(&path, &assistance.original)?,
        None => true,
    };
    if should_emit {
        eprintln!(
            "aishe: '{}' was not found; did you mean '{}'? Nothing ran. Prefix ? to ask the agent.",
            crate::commands::display_safe(&assistance.original),
            crate::commands::display_safe(&assistance.candidate),
        );
    }
    Ok(true)
}

/// Store typo rate-limit markers alongside the other private, shell-scoped
/// hook state. Shell integrations already remove this file on exit, so the
/// markers have exactly the live-shell lifetime and need no global cache.
fn record_typo_once(path: &std::path::Path, head: &str) -> Result<bool> {
    use std::io::{Read, Seek, Write};

    if path.exists() {
        validate_hook_state_file(path)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("opening private shell state {}", path.display()))?;
    #[cfg(unix)]
    unsafe {
        if libc::flock(std::os::fd::AsRawFd::as_raw_fd(&file), libc::LOCK_EX) != 0 {
            return Err(std::io::Error::last_os_error()).context("locking private shell state");
        }
    }
    validate_open_hook_state(&file)?;
    file.rewind()?;
    let mut existing = String::new();
    (&mut file).take(1025).read_to_string(&mut existing)?;
    let marker = format!("typo:{head}");
    if existing.lines().any(|line| line == marker) {
        return Ok(false);
    }
    if existing.len().saturating_add(marker.len() + 1) > 1024 {
        // Fail closed: the typo is still intercepted, but an overfull state
        // file cannot cause unbounded growth or repeated user-facing noise.
        return Ok(false);
    }
    writeln!(file, "{marker}")?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = file.metadata()?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            anyhow::bail!("private shell state is not owned by the current user");
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(true)
}

fn validate_hook_state_file(path: &std::path::Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 1024 {
        anyhow::bail!("private shell state is not a bounded regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!("private shell state has unsafe ownership or permissions");
        }
    }
    Ok(())
}

fn validate_open_hook_state(file: &std::fs::File) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > 1024 {
        anyhow::bail!("private shell state is not a bounded regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            anyhow::bail!("private shell state has unsafe ownership or permissions");
        }
    }
    Ok(())
}

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
    if let Err(error) = crate::connection::resolve(config) {
        crate::cli::error_contract::emit_from(error.as_ref());
        return;
    }
    crate::cli::error_contract::emit_classified(
        crate::user_error::ErrorNamespace::Provider,
        "connection_unavailable",
        format!(
            "Connection '{}' is unavailable through the selected backend.",
            crate::commands::display_safe(config.active_connection_id())
        ),
        "Run `aishe doctor`, repair the active connection, then retry.",
        None,
    );
}

fn print_managed_hook_failure(code: &'static str, error: &anyhow::Error) {
    crate::cli::error_contract::emit_classified(
        crate::user_error::ErrorNamespace::Backend,
        code,
        "The managed agent could not complete the shell-hook request.",
        "Run `aishe backend status`, then retry or select the native fallback.",
        Some(&crate::redact::redact(&error.to_string())),
    );
}

static YOLO_WORKSPACE_ACCEPTED: AtomicBool = AtomicBool::new(false);
static YOLO_HOST_ACCEPTED: AtomicBool = AtomicBool::new(false);

/// ZLE invokes the hidden acceptance helper while it owns the terminal and has
/// echo disabled. The acceptance phrase is not a secret, so make it visible for
/// this one read and restore the exact inherited terminal flags immediately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YoloAcceptance {
    Accepted,
    Declined,
}

pub fn yolo_answer_accepts(answer: Option<&str>, expected: &str) -> bool {
    answer.map(str::trim).is_some_and(|value| value == expected)
}

pub fn ensure_yolo_acceptance(config: &Config) -> Result<YoloAcceptance> {
    use crate::agent::ExecutionScope;
    use std::io::Write;

    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("backend.default_scope must be workspace or host")?;
    let in_process = match scope {
        ExecutionScope::Workspace => &YOLO_WORKSPACE_ACCEPTED,
        ExecutionScope::Host => &YOLO_HOST_ACCEPTED,
    };
    if in_process.load(Ordering::SeqCst) || acceptance_file_contains(scope) {
        in_process.store(true, Ordering::SeqCst);
        return Ok(YoloAcceptance::Accepted);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "yolo {:?} requires one interactive acceptance in each AIShe shell",
            scope
        );
    }

    let workspace = std::env::current_dir()
        .context("resolving yolo workspace")
        .and_then(|path| {
            crate::backend::opencode::session::SessionStore::resolve_workspace(&path)
        })?;
    println!();
    match scope {
        ExecutionScope::Workspace => {
            #[cfg(target_os = "linux")]
            let sandbox = match crate::dependencies::bubblewrap_probe() {
                crate::dependencies::BubblewrapState::Usable { .. } => "bubblewrap verified",
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
    // Raw-mode read: a cooked read_line echoed Shift-Tab as ^[[Z and ignored Esc.
    let answer = crate::promptui::read_terminal_line(true).context("reading yolo acceptance")?;
    let expected = match scope {
        ExecutionScope::Workspace => "yolo",
        ExecutionScope::Host => "yolo-host",
    };
    if !yolo_answer_accepts(answer.as_deref(), expected) {
        let current = std::env::var("AISHE_MODE")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| config.aishe.mode.clone());
        println!(
            "yolo not enabled · mode stays {}",
            crate::commands::display_safe(&current)
        );
        return Ok(YoloAcceptance::Declined);
    }
    persist_acceptance(scope)?;
    in_process.store(true, Ordering::SeqCst);
    crate::audit::action(
        "agent:yolo_accept",
        &format!("scope={scope:?} workspace={}", workspace.display()),
        Some(0),
    );
    Ok(YoloAcceptance::Accepted)
}

fn acceptance_file_contains(scope: crate::agent::ExecutionScope) -> bool {
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
        crate::agent::ExecutionScope::Workspace => "workspace",
        crate::agent::ExecutionScope::Host => "host",
    };
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| text.lines().any(|line| line == marker))
}

fn persist_acceptance(scope: crate::agent::ExecutionScope) -> Result<()> {
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
        crate::agent::ExecutionScope::Workspace => "workspace",
        crate::agent::ExecutionScope::Host => "host",
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
    if mode == AgentMode::Yolo && ensure_yolo_acceptance(config)? == YoloAcceptance::Declined {
        return Ok(None);
    }
    let options = TurnOptions::from_config(config, mode, render)?;
    match crate::agent::controller::run_turn(config, prompt, options) {
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
            crate::audit::action(
                "backend:fallback",
                &crate::redact::redact(&error.to_string()),
                None,
            );
            Ok(None)
        }
        Err(error) => {
            let admitted = error.admitted();
            let detail = crate::redact::redact(&error.into_error().to_string());
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
    let usage = crate::usage::Usage {
        input: outcome.usage.input_tokens,
        output: outcome.usage.output_tokens,
        requests: outcome
            .events
            .iter()
            .filter(|event| matches!(event, crate::agent::AgentEvent::Usage { .. }))
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
    crate::usagelog::append_attributed(
        std::path::Path::new(&path),
        usage,
        config.active_model(),
        Some(config.active_connection_id()),
    );
    if let Ok(status_path) = std::env::var("AISHE_STATUS_FILE") {
        if !status_path.is_empty() {
            crate::usagelog::write_status_for_connection(
                std::path::Path::new(&status_path),
                std::path::Path::new(&path),
                &config.pricing,
                Some((usage, config.active_model())),
                &config.effective_status_line_items(),
                config.active_connection_id(),
            );
            let task = outcome
                .session_id
                .strip_prefix("ses_")
                .unwrap_or(&outcome.session_id);
            let task = task.chars().take(12).collect::<String>();
            let context_tokens = outcome.events.iter().rev().find_map(|event| match event {
                crate::agent::AgentEvent::Usage { usage } => Some(usage.input_tokens),
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
                    format!("context {} tok", crate::usage::group(tokens)),
                ));
            }
            if let Some(connection) = config.active_connection() {
                if let crate::config::ConnectionAuth::OAuth { profile } = &connection.auth {
                    if let Some(provider) =
                        crate::oauth::OAuthProvider::from_base_url(&connection.settings.base_url)
                    {
                        if let Some(usage) = crate::oauth::plan_usage(provider, profile) {
                            metadata.push(("plan", usage.summary));
                        }
                    }
                }
            }
            crate::usagelog::merge_status(std::path::Path::new(&status_path), &metadata);
        }
    }
}

pub fn suggest_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let line = crate::attachments::expand(line, executor.cwd(), config)?.prompt;
    suggest_line_raw(&line, executor, provider, config)
}

fn suggest_line_raw(
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
                let answer = suggestion_is_answer(&suggestion, executor);
                emit_suggest_hook(&suggestion, executor);
                if answer {
                    print_first_answer_hint_stderr(config);
                }
                return Ok(0);
            }
            Ok(None) => {}
            Err(error) => {
                print_managed_hook_failure("suggest_line_managed", &error);
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
    let mem = crate::cli::session::hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    let suggestion = modes::suggest::request(line, p, executor, config, session.history())?;
    // The blocking network work is done; let the rest run unbounded.
    cancel_hook_budget();
    let answer = suggestion_is_answer(&suggestion, executor);
    let reply = emit_suggest_hook(&suggestion, executor);
    if answer {
        print_first_answer_hint_stderr(config);
    }
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

fn suggestion_is_answer(suggestion: &modes::suggest::Suggestion, executor: &Executor) -> bool {
    match suggestion {
        modes::suggest::Suggestion::Answer { .. } => true,
        modes::suggest::Suggestion::Command { command, .. } => !shell_syntax_ok(executor, command),
    }
}

fn print_first_answer_hint_stderr(config: &Config) {
    if !crate::ui::TerminalCapabilities::detect_stderr().is_tty {
        return;
    }
    if let Some(hint) = crate::hints::take_first_answer_next_action(config) {
        eprintln!("{hint}");
    }
}

/// Shell-hook helper for the fix-the-last-command key: given the failed command
/// (and `$AISHE_LAST_EXIT`), ask the model for a corrected command and print it
/// for the widget to pre-fill. With `fix_capture_stderr`, a read-only safe
/// command is re-run once to capture its error output for a better fix.
pub fn fix_line(
    cmd: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let exit = std::env::var("AISHE_LAST_EXIT").unwrap_or_else(|_| "unknown".to_string());
    fix_command(cmd, &exit, executor, provider, config)
}

pub fn fix_command(
    cmd: &str,
    exit: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let ctx = crate::fix::error_context(cmd, config.aishe.fix_capture_stderr);
    let prompt = crate::fix::build_prompt(cmd, exit, ctx.as_deref());

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
                print_managed_hook_failure("fix_line_managed", &error);
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

/// Rewrite the current ZLE buffer through the strict suggestion contract. The
/// caller owns buffer replacement; this helper never executes the result.
pub fn edit_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if line.trim().is_empty() {
        return Ok(0);
    }
    let prompt = format!(
        "Rewrite the shell command below to be correct, clear, and idiomatic. \
Return exactly one runnable shell command using the command response shape. \
Preserve the user's intent and do not execute it.\n\nCurrent command:\n{line}"
    );
    suggest_line_raw(&prompt, executor, provider, config)
}

/// Generate a command and hand it to the active shell's existing private
/// staging channel. The parent ZLE hook owns insertion; this never executes.
pub fn ask_insert(
    request: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if request.trim().is_empty() {
        return Ok(1);
    }
    let prompt = format!(
        "Create exactly one runnable shell command for this request. Do not execute it.\nRequest: {request}"
    );
    let suggestion = if config.backend.engine == "opencode" {
        match managed_turn(config, &prompt, AgentMode::Suggest, false) {
            Ok(Some(outcome)) => modes::suggest::parse_suggestion(&outcome.text),
            Ok(None) => {
                let Some(provider) = provider else {
                    print_llm_unavailable(config);
                    return Ok(1);
                };
                modes::suggest::request(&prompt, provider, executor, config, Vec::new())?
            }
            Err(error) => {
                print_managed_hook_failure("ask_insert_managed", &error);
                return Ok(1);
            }
        }
    } else {
        let Some(provider) = provider else {
            print_llm_unavailable(config);
            return Ok(1);
        };
        modes::suggest::request(&prompt, provider, executor, config, Vec::new())?
    };
    let modes::suggest::Suggestion::Command { command, .. } = suggestion else {
        anyhow::bail!("the model returned an answer instead of a shell command");
    };
    if !shell_syntax_ok(executor, &command) {
        anyhow::bail!("the generated command is not valid shell syntax");
    }
    if !stage_hook_command("fill", &command)? {
        anyhow::bail!("--insert requires an active AIShe shell");
    }
    eprintln!("aishe: command staged for review; press Enter to run or edit it first");
    Ok(0)
}

/// Shell-hook helper: run the yolo loop directly for a natural-language line.
pub fn yolo_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &crate::mcp::McpRegistry,
) -> Result<u8> {
    let line = crate::attachments::expand(line, executor.cwd(), config)?.prompt;
    if config.backend.engine == "opencode" {
        match managed_turn(config, &line, AgentMode::Yolo, true) {
            Ok(Some(_)) => return Ok(0),
            Ok(None) => {}
            Err(error) => {
                print_managed_hook_failure("yolo_line_managed", &error);
                return Ok(1);
            }
        }
    }
    let Some(p) = provider else {
        print_llm_unavailable(config);
        return Ok(1);
    };
    let mem = crate::cli::session::hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    modes::yolo::run(
        &line,
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
pub fn auto_line(
    line: &str,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    let line = crate::attachments::expand(line, executor.cwd(), config)?.prompt;
    if config.backend.engine == "opencode" {
        match managed_turn(config, &line, AgentMode::Auto, true) {
            Ok(Some(_)) => return Ok(0),
            Ok(None) => {}
            Err(error) => {
                print_managed_hook_failure("auto_line_managed", &error);
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
    let mem = crate::cli::session::hook_session_path(config);
    let mut session = match &mem {
        Some(path) => Session::load_persisted(path),
        None => Session::new(false),
    };
    let suggestion = modes::suggest::request(&line, p, executor, config, session.history())?;
    // The blocking network work is done; let the rest run unbounded.
    cancel_hook_budget();
    let answer = suggestion_is_answer(&suggestion, executor);
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
                    eprintln!("{}", format!("! {reason} — pre-filled for review").yellow());
                    EXIT_AUTO_DANGEROUS
                }
                // Same exit code on purpose: the contract's `20` means "do not
                // auto-run this, pre-fill it for review", which is exactly the
                // right handling for a command the gate could not resolve. A new
                // code would break every hook that switches on 0 vs 20.
                Risk::Unknown(reason) => {
                    eprintln!(
                        "{}",
                        format!("! could not verify ({reason}) — pre-filled for review").yellow()
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
        session.record_user(&line);
        session.record_assistant(&reply);
        session.save_persisted(path);
    }
    if answer {
        print_first_answer_hint_stderr(config);
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
/// `{schema_version, kind, command, explanation, risk, reason}`. `risk` is `"safe"`,
/// `"dangerous"`, `"unknown"` (the gate could not tell what the command runs —
/// treat like `"dangerous"`), or `"n/a"` for an answer. The existing values and
/// exit codes are unchanged; `"unknown"` is additive, so a consumer that only
/// tests `risk == "safe"` still fails closed.
pub fn suggest_command(
    query: &str,
    json: bool,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if query.trim().is_empty() {
        let error = crate::user_error::UserError::classified(
            crate::user_error::ErrorNamespace::Cli,
            "missing_request",
            "The suggest command needs a natural-language request.",
            "Run `aishe suggest \"list files by size\"`.",
        )
        .expect("static user-error code is valid");
        print_suggest_error(&error, json);
        return Ok(1);
    }
    let suggestion = if config.backend.engine == "opencode" {
        match managed_turn(config, query, AgentMode::Suggest, false) {
            Ok(Some(outcome)) => modes::suggest::parse_suggestion(&outcome.text),
            Ok(None) => {
                let Some(provider) = provider else {
                    print_suggest_provider_unavailable(config, json);
                    return Ok(1);
                };
                match modes::suggest::request_strict(query, provider, executor, config, Vec::new())
                {
                    Ok(suggestion) => suggestion,
                    Err(error) => {
                        print_suggest_error(&crate::providers::user_error(&error), json);
                        return Ok(1);
                    }
                }
            }
            Err(error) => {
                let public = crate::user_error::UserError::classified(
                    crate::user_error::ErrorNamespace::Backend,
                    "suggest_failed",
                    "The managed backend could not complete the suggestion.",
                    "Run `aishe backend status`, then retry the request.",
                )
                .expect("static user-error code is valid")
                .with_retryable(true)
                .with_detail(crate::redact::redact(&error.to_string()));
                print_suggest_error(&public, json);
                return Ok(1);
            }
        }
    } else {
        let Some(provider) = provider else {
            print_suggest_provider_unavailable(config, json);
            return Ok(1);
        };
        match modes::suggest::request_strict(query, provider, executor, config, Vec::new()) {
            Ok(suggestion) => suggestion,
            Err(error) => {
                print_suggest_error(&crate::providers::user_error(&error), json);
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
            "schema_version": 1,
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
            eprintln!("{}", format!("! {reason}").yellow());
        } else if risk == "unknown" {
            eprintln!("{}", format!("! could not verify ({reason})").yellow());
        }
        println!("{command}");
    } else if !explanation.is_empty() {
        // An answer: to stderr so stdout stays empty (no command to run).
        eprintln!("{explanation}");
    }
    Ok(code)
}

/// Non-executing answer command with a stable stdout contract. `--schema`
/// accepts the deliberately small, provider-portable JSON Schema subset used by
/// AIShe itself: type, properties, required, items, enum, and
/// additionalProperties=false.
pub fn ask_command(
    query: &str,
    json_output: bool,
    schema_path: Option<&std::path::Path>,
    executor: &Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<u8> {
    if query.trim().is_empty() {
        anyhow::bail!("`aishe ask` needs a request");
    }
    let query = crate::attachments::expand(query, executor.cwd(), config)?.prompt;
    let schema = schema_path.map(read_answer_schema).transpose()?;
    let schema_instruction = schema.as_ref().map_or_else(String::new, |value| {
        format!(
            " Return only JSON matching this schema, without markdown fences: {}",
            value
        )
    });
    let prompt = format!(
        "Answer the request directly. Do not run commands or request tools. Be concise and factual.{schema_instruction}\n\n{query}"
    );
    let answer = if config.backend.engine == "opencode" {
        match managed_turn(config, &prompt, AgentMode::Suggest, false)? {
            Some(outcome) => answer_from_suggestion(&outcome.text),
            None => native_answer(&prompt, schema.as_ref(), executor, provider, config)?,
        }
    } else {
        native_answer(&prompt, schema.as_ref(), executor, provider, config)?
    };
    let answer = crate::commands::display_safe(answer.trim());

    if let Some(schema) = &schema {
        let value = parse_answer_json(&answer)?;
        validate_answer_schema(schema, &value, "$")?;
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "result": value,
            }))?
        );
    } else if json_output {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "answer": answer,
            }))?
        );
    } else {
        println!("{answer}");
    }
    Ok(0)
}

fn native_answer(
    prompt: &str,
    schema: Option<&serde_json::Value>,
    executor: &Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Result<String> {
    let provider = provider.context("no model provider is configured; run `aishe setup`")?;
    let format = schema.map_or(ResponseFormat::Text, |schema| ResponseFormat::JsonSchema {
        name: "aishe_answer".into(),
        schema: schema.clone(),
    });
    let message = format!(
        "{}\nUser request: {prompt}",
        crate::context::build(executor, config)
    );
    provider
        .complete(
            "You are AIShe, a non-executing command-line assistant. Answer directly and never claim to have run anything.",
            &[Msg::User(message)],
            &format,
        )
        .map_err(|error| anyhow::anyhow!(crate::providers::actionable_error(&error)))
}

fn answer_from_suggestion(raw: &str) -> String {
    match modes::suggest::parse_suggestion(raw) {
        modes::suggest::Suggestion::Answer { explanation } => explanation,
        modes::suggest::Suggestion::Command {
            command,
            explanation,
        } => {
            if explanation.is_empty() {
                command
            } else {
                explanation
            }
        }
    }
}

fn read_answer_schema(path: &std::path::Path) -> Result<serde_json::Value> {
    const MAX_SCHEMA_BYTES: u64 = 1024 * 1024;
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("reading schema metadata {}", path.display()))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_SCHEMA_BYTES {
        anyhow::bail!("schema must be a regular file no larger than 1 MiB");
    }
    let schema: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
    if !schema.is_object() {
        anyhow::bail!("schema root must be an object");
    }
    Ok(schema)
}

fn parse_answer_json(text: &str) -> Result<serde_json::Value> {
    let trimmed = text.trim();
    let trimmed = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .unwrap_or(trimmed)
        .strip_suffix("```")
        .unwrap_or(trimmed)
        .trim();
    serde_json::from_str(trimmed).context("provider answer was not valid JSON")
}

fn validate_answer_schema(
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
) -> Result<()> {
    if let Some(expected) = schema.get("type").and_then(serde_json::Value::as_str) {
        let matches = match expected {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "number" => value.is_number(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => anyhow::bail!("unsupported schema type {other:?} at {path}"),
        };
        if !matches {
            anyhow::bail!("answer does not match schema type {expected:?} at {path}");
        }
    }
    if let Some(options) = schema.get("enum").and_then(serde_json::Value::as_array) {
        if !options.contains(value) {
            anyhow::bail!("answer is not one of the allowed enum values at {path}");
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object);
        if let Some(required) = schema.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(key) {
                    anyhow::bail!("answer is missing required property {path}.{key}");
                }
            }
        }
        if schema
            .get("additionalProperties")
            .and_then(serde_json::Value::as_bool)
            == Some(false)
        {
            for key in object.keys() {
                if !properties.is_some_and(|values| values.contains_key(key)) {
                    anyhow::bail!("answer contains unexpected property {path}.{key}");
                }
            }
        }
        if let Some(properties) = properties {
            for (key, child_schema) in properties {
                if let Some(child) = object.get(key) {
                    validate_answer_schema(child_schema, child, &format!("{path}.{key}"))?;
                }
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for (index, child) in values.iter().enumerate() {
            validate_answer_schema(items, child, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
}

/// `suggest` keeps its pre-1.0 aggregate failure status of 1. The structured
/// error retains its domain-specific status so consumers can migrate without
/// losing the stable command-level contract.
fn print_suggest_error(error: &crate::user_error::UserError, json: bool) {
    if json {
        match error.render_json() {
            Ok(document) => eprintln!("{document}"),
            Err(_) => {
                let fallback = serde_json::json!({
                    "schema_version": 1,
                    "code": "internal.serialization_failed",
                    "message": "AIShe could not serialize the error.",
                    "retryable": false,
                    "exit_code": 1,
                    "next_action": "Run `aishe doctor` and retry.",
                    "detail": null,
                });
                eprintln!("{fallback}");
            }
        }
    } else {
        eprintln!("{}", error.render_text());
    }
}

fn print_suggest_provider_unavailable(config: &Config, json: bool) {
    if !json {
        print_llm_unavailable(config);
        return;
    }
    let error = crate::user_error::UserError::classified(
        crate::user_error::ErrorNamespace::Config,
        "provider_unavailable",
        "No usable model provider is configured.",
        "Run `aishe setup`, then verify the provider with `aishe doctor --live`.",
    )
    .expect("static user-error code is valid");
    print_suggest_error(&error, true);
}

fn command_cli_hint(spec: &command_surface::CommandSpec) -> Option<String> {
    let invocation = spec.cli?;
    let mut parts = vec!["aishe", invocation.command];
    parts.extend(invocation.prefix_args.iter().copied());
    let mut hint = parts.join(" ");
    match spec.arguments {
        ArgumentPolicy::None => {}
        ArgumentPolicy::OptionalValue(value) => {
            hint.push_str(&format!(" [{value}]"));
        }
        ArgumentPolicy::PassThrough(value) => {
            hint.push_str(&format!(" [{value}…]"));
        }
    }
    Some(hint)
}

/// Reject registered names which are diagnostic-only in `aishe -c`. Returning
/// before custom/MCP lookup is intentional: active built-ins and tombstones own
/// their aliases and cannot be shadowed by local files or remote prompts.
fn reject_unavailable_one_shot_slash(input: &str) -> Option<u8> {
    let parsed = command_surface::parse_slash(input)?;
    let spec = parsed.spec?;
    match spec.lifecycle {
        Lifecycle::Tombstone { guidance, .. } => {
            eprintln!("aishe: /{} is no longer available; {guidance}", parsed.name);
            Some(EXIT_COMMAND_UNAVAILABLE)
        }
        Lifecycle::Active => match spec.support(Surface::OneShot) {
            SurfaceSupport::Supported => None,
            SurfaceSupport::Recognized(reason) | SurfaceSupport::Unavailable(reason) => {
                eprintln!(
                    "aishe: /{} is unavailable in one-shot mode: {reason}",
                    parsed.name
                );
                if let Some(hint) = command_cli_hint(spec) {
                    eprintln!("next: {hint}");
                }
                Some(EXIT_COMMAND_UNAVAILABLE)
            }
        },
    }
}

/// Run one dispatch cycle non-interactively for the `-c` flag.
#[allow(clippy::too_many_arguments)]
pub fn one_shot(
    input: &str,
    executor: &mut Executor,
    provider: &mut Option<Arc<dyn Provider>>,
    config: &Config,
    cache: &CommandCache,
    commands: &CommandRegistry,
    skills: &SkillRegistry,
    mcp: &crate::mcp::McpRegistry,
) -> Result<u8> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Ok(0);
    }
    if let Some(code) = reject_unavailable_one_shot_slash(trimmed) {
        return Ok(code);
    }
    if trimmed.starts_with('!') {
        print_forced_shell_cue();
    } else if trimmed.starts_with('#') {
        print_legacy_hash_cue();
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
                // Commands declared as one-shot-capable are handled locally.
                // Surface validation above already rejected shell-only entries
                // and tombstones with an actionable diagnostic.
                match tokens.get(1).map(|s| s.as_str()) {
                    Some("help") => {
                        return Ok(print_help_command(tokens.get(2).map(String::as_str)));
                    }
                    Some("status") => {
                        crate::cli::status::command(config, false);
                    }
                    Some("reasoning") => {
                        crate::cli::connection::reasoning(
                            config,
                            tokens.get(2).map(String::as_str),
                            false,
                        );
                    }
                    Some("log") => {
                        crate::cli::history::log(config, None, None, None, None, Some(20), false);
                    }
                    Some("skills") => {
                        if skills.is_empty() {
                            println!(
                                "no skills (add <name>/SKILL.md files to {})",
                                crate::skills::user_dir().unwrap_or_default().display()
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
                    Some("usage") => {
                        crate::cli::status::print_usage_summary(provider.as_deref(), config)
                    }
                    Some("reset") => return crate::cli::session::reset(config),
                    Some(id) => {
                        // This branch is an invariant guard, not ordinary user
                        // routing: registry tests require every one-shot-active
                        // stable id to have an implementation above.
                        eprintln!("aishe: command registry has no one-shot handler for '{id}'");
                        return Ok(1);
                    }
                    None => return Ok(1),
                }
                return Ok(0);
            }
            Ok(executor.run_builtin(&tokens) as u8)
        }
        Dispatch::NaturalLanguage(nl) => {
            let nl = crate::attachments::expand(&nl, executor.cwd(), config)?.prompt;
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
                        print_managed_hook_failure("one_shot_managed", &error);
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

pub fn print_forced_shell_cue() {
    eprintln!("AIShe · shell override — AI safety gate bypassed for this line only");
}

fn print_legacy_hash_cue() {
    eprintln!("AIShe · # agent prefix is deprecated; use ? (removal planned for 0.9)");
}

/// Run a user-defined `/slash-command` if `line` names one. Returns whether it
/// was handled. Active built-ins and compatibility tombstones are reserved by
/// the command-surface registry and left for normal dispatch.
#[allow(clippy::too_many_arguments)]
fn try_custom_command(
    line: &str,
    commands: &CommandRegistry,
    executor: &mut Executor,
    provider: Option<&dyn Provider>,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &crate::mcp::McpRegistry,
    session: &mut Session,
) -> Result<bool> {
    let Some(parsed) = command_surface::parse_slash(line) else {
        return Ok(false);
    };
    if parsed.spec.is_some() {
        return Ok(false);
    }
    let name = parsed.name;
    let args = parsed.args;
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
pub fn warn_untrusted_skills(skills: &SkillRegistry) {
    for path in skills.untrusted() {
        // The path is repo-supplied: escape control characters so a crafted
        // filename cannot repaint the line (same reason as `gate_custom_shell`).
        let shown = crate::commands::display_safe(&path.display().to_string());
        eprintln!(
            "{}",
            format!("aishe: ignoring untrusted project skill — aishe trust {shown}").yellow()
        );
    }
}

/// Whether a custom command's source file is currently trusted. A user-origin
/// command (`source == None`) is trusted by construction.
fn custom_cmd_trusted(cmd: &crate::commands::CustomCommand) -> bool {
    match cmd.source.as_deref() {
        None => true,
        Some(src) => {
            let contents = std::fs::read_to_string(src).unwrap_or_default();
            crate::trust::is_trusted(src, &contents)
        }
    }
}

/// Gate execution of a `shell:true` custom command. Returns whether to run it.
///
/// A **project**-origin command (from a cloned repo's `<cwd>/.aishe/commands`) must
/// be trusted (`aishe trust <file>`) or explicitly confirmed — the resolved shell
/// command is shown first — before it can run. **Both** origins additionally pass
/// through the standard safety gate (`assess` + `confirm_dangerous`).
fn gate_custom_shell(cmd: &crate::commands::CustomCommand, resolved: &str) -> bool {
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
                crate::commands::display_safe(resolved).white().bold()
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
    mcp: &crate::mcp::McpRegistry,
    session: &mut Session,
) -> Result<()> {
    let nl = crate::attachments::expand(nl, executor.cwd(), config)?.prompt;
    let agent_mode = AgentMode::parse(mode).unwrap_or(AgentMode::Suggest);
    if config.backend.engine == "opencode" {
        let render = agent_mode != AgentMode::Suggest;
        if let Some(outcome) = managed_turn(config, &nl, agent_mode, render)? {
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
        "yolo" => modes::yolo::run(&nl, p, executor, config, &INTERRUPTED, skills, mcp, session)?,
        "auto" => modes::suggest::run(&nl, p, executor, config, false, true, session)?,
        _ => modes::suggest::run(&nl, p, executor, config, false, false, session)?,
    }
    Ok(())
}

/// Print the `aishe mcp` listing: connected tools (yolo), plus any prompts
/// (invocable as `/<server>:<prompt>`).
pub fn print_mcp_listing(mcp: &crate::mcp::McpRegistry) {
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

pub fn print_help_command(topic: Option<&str>) -> u8 {
    // Task-first product help is the single index; `/help all` expands to the
    // full table. Only append custom commands once.
    let code = crate::product_help::print_help(topic);
    if topic.is_none() {
        let commands = CommandRegistry::load();
        for name in commands.shadowed() {
            eprintln!("aishe: custom command /{name} is shadowed by a built-in (rename the file)");
        }
        if commands.is_empty() {
            println!(
                "custom slash-commands: none (add *.md under {})",
                crate::commands::user_dir().unwrap_or_default().display()
            );
        } else {
            println!("custom slash-commands:");
            for (name, description) in commands.list() {
                println!("  /{name:<14} {description}");
            }
        }
    }
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_temp_file(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "aishe-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }

    #[test]
    fn hook_command_handoff_is_exact_and_private() {
        let path = private_temp_file("hook-handoff");
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

    #[test]
    fn typo_markers_are_unique_bounded_private_and_cleanup_is_recoverable() {
        let path = private_temp_file("typo-state");
        let _ = std::fs::remove_file(&path);
        assert!(record_typo_once(&path, "gti").unwrap());
        assert!(!record_typo_once(&path, "gti").unwrap());
        assert!(record_typo_once(&path, "sl").unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "typo:gti\ntypo:sl\n"
        );
        assert!(std::fs::metadata(&path).unwrap().len() <= 1024);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_file(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn forced_agent_input_bypasses_typo_assistance() {
        let cache = CommandCache::new();
        cache.insert_all(&["git"]);
        assert!(dispatcher::typo_assistance("? gti status", &cache).is_none());
        assert!(dispatcher::typo_assistance("gti status", &cache).is_some());
    }
}

#[cfg(test)]
mod yolo_tests {
    use super::*;

    #[test]
    fn yolo_answer_requires_the_exact_word() {
        assert!(yolo_answer_accepts(Some("yolo"), "yolo"));
        assert!(yolo_answer_accepts(Some("  yolo \n"), "yolo"));
        assert!(!yolo_answer_accepts(Some("n"), "yolo"));
        assert!(!yolo_answer_accepts(Some("yolo"), "yolo-host"));
        assert!(!yolo_answer_accepts(None, "yolo"));
    }
}
