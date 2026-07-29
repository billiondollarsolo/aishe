//! Foreground executor for supervisor-routed proxy tools.

use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use crate::agent::{ExecutionScope, Mode, NetworkPolicy};
use crate::backend::bridge::{
    LeaseIdentity, LeaseRegistration, ToolCompletion, ToolStarted, ToolWork,
};
use crate::backend::control::SupervisorClient;
use crate::config::Config;
use crate::executor::Executor;

const WORKER_SHUTDOWN_GRACE: Duration = Duration::from_millis(100);

pub struct ToolWorker {
    stop: Arc<AtomicBool>,
    client: SupervisorClient,
    identity: LeaseIdentity,
    thread: Option<JoinHandle<()>>,
}

impl ToolWorker {
    pub fn start(
        client: SupervisorClient,
        registration: LeaseRegistration,
        config: Config,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        Self::start_with_output(client, registration, config, cancel, true)
    }

    /// Start a validation worker without echoing model-requested commands or
    /// their output to stdout. This keeps `setup --json` machine-readable while
    /// retaining the same policy, sandbox, audit, and credential isolation.
    pub fn start_silent(
        client: SupervisorClient,
        registration: LeaseRegistration,
        config: Config,
        cancel: Arc<AtomicBool>,
    ) -> Result<Self> {
        Self::start_with_output(client, registration, config, cancel, false)
    }

    fn start_with_output(
        client: SupervisorClient,
        registration: LeaseRegistration,
        config: Config,
        cancel: Arc<AtomicBool>,
        stream_output: bool,
    ) -> Result<Self> {
        let denied_environment = sensitive_environment_names(&config);
        let identity = client.register(&registration)?;
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_client = client.clone();
        let worker_identity = identity.clone();
        let thread = std::thread::Builder::new()
            .name("aishe-tool-worker".into())
            .spawn(move || {
                let skills = crate::skills::SkillRegistry::load();
                let mcp = crate::mcp::McpRegistry::connect(&config.mcp_servers);
                let mut approvals = HashSet::new();
                while !worker_stop.load(Ordering::SeqCst) {
                    match worker_client.next(&worker_identity) {
                        Ok(Some(work)) => {
                            let started = ToolStarted {
                                lease_id: worker_identity.lease_id.clone(),
                                session_id: work.session_id.clone(),
                                message_id: work.message_id.clone(),
                                call_id: work.call_id.clone(),
                            };
                            if worker_client.started(&started).is_err() {
                                break;
                            }
                            let result = execute(
                                &work,
                                &skills,
                                &mcp,
                                &mut approvals,
                                &cancel,
                                &denied_environment,
                                stream_output,
                            );
                            let completion = ToolCompletion {
                                lease_id: worker_identity.lease_id.clone(),
                                session_id: work.session_id,
                                message_id: work.message_id,
                                call_id: work.call_id,
                                success: result.success,
                                output: Value::String(result.output),
                                exit_code: result.exit_code,
                            };
                            if worker_client.complete(&completion).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            let _ = worker_client.heartbeat(&worker_identity);
                        }
                        Err(_) if worker_stop.load(Ordering::SeqCst) => break,
                        Err(error) => {
                            eprintln!(
                                "aishe: foreground tool bridge stopped: {}",
                                crate::redact::redact(&error.to_string())
                            );
                            break;
                        }
                    }
                }
            })
            .context("starting foreground tool worker")?;
        Ok(Self {
            stop,
            client,
            identity,
            thread: Some(thread),
        })
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.client.unregister(&self.identity);
        if let Some(thread) = self.thread.take() {
            // Unregister wakes an idle bridge poll immediately. A command also
            // observes the shared cancellation flag and tears down its process
            // group. External HTTP/MCP implementations are only timeout-bounded,
            // however, and Rust cannot safely kill their worker thread. Never
            // hold Ctrl-C or foreground teardown hostage to that timeout: after
            // a short grace, detach. The revoked lease makes any late completion
            // fail and the started durable call remains outcome-unknown.
            finish_worker_thread(thread);
        }
    }
}

impl Drop for ToolWorker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn finish_worker_thread(thread: JoinHandle<()>) -> bool {
    let deadline = std::time::Instant::now() + WORKER_SHUTDOWN_GRACE;
    while !thread.is_finished() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    if thread.is_finished() {
        let _ = thread.join();
        true
    } else {
        // Dropping a JoinHandle detaches it. Its bridge lease has already been
        // revoked, so a late completion cannot be accepted.
        false
    }
}

struct ExecutionResult {
    success: bool,
    output: String,
    exit_code: Option<i32>,
}

fn execute(
    work: &ToolWork,
    skills: &crate::skills::SkillRegistry,
    mcp: &crate::mcp::McpRegistry,
    approvals: &mut HashSet<String>,
    cancel: &Arc<AtomicBool>,
    denied_environment: &HashSet<String>,
    stream_output: bool,
) -> ExecutionResult {
    if cancel.load(Ordering::SeqCst) {
        return failure("Agent tool execution was cancelled before it started.");
    }
    let mut work = work.clone();
    if let Err(message) = approve(&mut work, approvals) {
        return failure(&message);
    }
    if cancel.load(Ordering::SeqCst) {
        return failure("Agent tool execution was cancelled before it started.");
    }
    let result = match work.tool.as_str() {
        "run_command" => run_command(&work, cancel, denied_environment, stream_output),
        "read_file" | "write_file" | "edit_file" | "list_dir" => run_file_tool(&work),
        "search_files" => search_files(&work, denied_environment, cancel),
        "fetch_url" => fetch_url(&work, cancel),
        "use_skill" => use_skill(&work, skills),
        "mcp_call" => mcp_call(&work, mcp, cancel),
        "ask_user" => ask_user(&work),
        "apply_patch" => apply_patch(&work, denied_environment, cancel),
        _ => failure("unknown foreground tool"),
    };
    crate::audit::action(
        &format!("agent:{}", work.tool),
        &crate::redact::redact(&format!(
            "{}:{}:{}",
            work.session_id, work.message_id, work.call_id
        )),
        result.exit_code,
    );
    result
}

fn approve(
    work: &mut ToolWork,
    approvals: &mut HashSet<String>,
) -> std::result::Result<(), String> {
    match work.mode {
        Mode::Suggest => {
            return Err("Suggest mode does not execute model-requested host tools.".into())
        }
        Mode::Yolo => return Ok(()),
        Mode::Auto => {}
    }
    if !work.interactive || !std::io::stdin().is_terminal() {
        if auto_approval_reason(work).is_some() {
            return Err("Auto mode requires an interactive approval for this tool.".into());
        }
        return Ok(());
    }
    loop {
        let Some(reason) = auto_approval_reason(work) else {
            return Ok(());
        };
        if approvals.contains(&approval_key(work)) {
            return Ok(());
        }
        let detail = approval_detail(work);
        println!();
        println!("  ┌─ agent action ─────────────────────────────────");
        println!(
            "  │ {}",
            crate::commands::display_safe(&format!("{}  {detail}", work.tool))
        );
        println!("  │ target   {}", crate::commands::display_safe(&detail));
        println!("  │ reason   {}", crate::commands::display_safe(&reason));
        println!(
            "  │ policy   {:?} scope · {:?} network · {}",
            work.scope,
            work.network,
            approval_capabilities(work)
        );
        println!("  └────────────────────────────────────────────────");
        if work.tool == "run_command" {
            print!("  [o] allow once  [s] allow matching this session  [e] edit  [d] deny: ");
        } else {
            print!("  [o] allow once  [s] allow matching this session  [d] deny: ");
        }
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return Err("Could not read the approval decision.".into());
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "o" | "once" | "y" | "yes" => return Ok(()),
            "s" | "session" | "always" => {
                approvals.insert(approval_key(work));
                return Ok(());
            }
            "e" | "edit" if work.tool == "run_command" => {
                print!("  replacement command: ");
                let _ = std::io::stdout().flush();
                let mut command = String::new();
                if std::io::stdin().read_line(&mut command).is_err()
                    || command.trim().is_empty()
                    || command.len() > 65_536
                {
                    return Err("Edited command is empty or invalid.".into());
                }
                work.args["command"] = Value::String(command.trim_end().to_string());
            }
            _ => return Err("User declined the agent action.".into()),
        }
    }
}

fn approval_detail(work: &ToolWork) -> String {
    match work.tool.as_str() {
        "run_command" => work
            .args
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("run command")
            .to_string(),
        "fetch_url" => work
            .args
            .get("url")
            .and_then(Value::as_str)
            .unwrap_or("network request")
            .to_string(),
        "mcp_call" => format!(
            "{} / {}",
            work.args
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or("server"),
            work.args
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("tool")
        ),
        _ => work
            .args
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or(work.tool.as_str())
            .to_string(),
    }
}

fn approval_key(work: &ToolWork) -> String {
    use sha2::{Digest, Sha256};

    let identity = match work.tool.as_str() {
        "run_command" => work
            .args
            .get("command")
            .and_then(Value::as_str)
            .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default(),
        "mcp_call" => format!(
            "{}:{}",
            work.args
                .get("server")
                .and_then(Value::as_str)
                .unwrap_or(""),
            work.args.get("tool").and_then(Value::as_str).unwrap_or("")
        ),
        "apply_patch" => {
            let value = work.args.get("patch").and_then(Value::as_str).unwrap_or("");
            format!("{:x}", Sha256::digest(value.as_bytes()))
        }
        _ => approval_detail(work),
    };
    format!(
        "{:?}:{:?}:{}:{}",
        work.scope, work.network, work.tool, identity
    )
}

fn approval_capabilities(work: &ToolWork) -> String {
    let network = work.tool == "fetch_url"
        || work
            .args
            .get("command")
            .and_then(Value::as_str)
            .is_some_and(crate::sandbox::is_network_command);
    let sudo = work
        .args
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.split_whitespace().any(|part| part == "sudo"));
    let writes = matches!(
        work.tool.as_str(),
        "write_file" | "edit_file" | "apply_patch" | "mcp_call"
    ) || work
        .args
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(crate::sandbox::is_write_command);
    format!(
        "{}{}{}",
        if writes { "writes" } else { "read-only" },
        if network { " · network" } else { "" },
        if sudo { " · sudo" } else { "" }
    )
}

fn auto_approval_reason(work: &ToolWork) -> Option<String> {
    match work.tool.as_str() {
        "read_file" | "list_dir" | "search_files" | "use_skill" | "ask_user" => None,
        "run_command" => {
            let command = work
                .args
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("");
            match crate::safety::assess(command) {
                crate::safety::Risk::Safe => None,
                crate::safety::Risk::Dangerous(reason) => {
                    Some(format!("dangerous command: {reason}"))
                }
                crate::safety::Risk::Unknown(reason) => {
                    Some(format!("command safety is unknown: {reason}"))
                }
            }
        }
        "write_file" | "edit_file" | "apply_patch" => Some("this action changes files".into()),
        "fetch_url" => Some("this action accesses the network".into()),
        "mcp_call" => Some("this action calls an external MCP tool".into()),
        _ => Some("this action is not classified as read-only".into()),
    }
}

fn run_command(
    work: &ToolWork,
    cancel: &Arc<AtomicBool>,
    denied_environment: &HashSet<String>,
    stream_output: bool,
) -> ExecutionResult {
    let command = work
        .args
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("");
    if command.trim().is_empty() {
        return failure("No command was provided.");
    }
    if work.network == NetworkPolicy::Deny && crate::sandbox::is_network_command(command) {
        return failure("Network access is denied for this workspace tool lease.");
    }
    #[cfg(not(target_os = "linux"))]
    if work.scope == ExecutionScope::Workspace {
        if let Some(reason) = crate::sandbox::sandbox_refusal(command) {
            return failure(&format!(
                "Workspace policy refused this command on macOS: {reason}"
            ));
        }
    }
    let cwd = match command_cwd(work) {
        Ok(path) => path,
        Err(error) => return failure(&error.to_string()),
    };
    let mut executor = match agent_executor(work, &cwd, denied_environment) {
        Ok(executor) => executor,
        Err(error) => return failure(&error.to_string()),
    };
    executor.set_cancel_flag(Arc::clone(cancel));
    let timeout = work
        .args
        .get("timeout_secs")
        .and_then(Value::as_u64)
        .unwrap_or(120)
        .clamp(1, 3600);
    let interactive = work
        .args
        .get("interactive")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || command_requires_interactive_terminal(command);
    if stream_output {
        println!("  → {}", crate::commands::display_safe(command));
    }
    let (code, output) = if interactive {
        if !work.interactive || !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()
        {
            return failure(
                "This command requires a foreground terminal. Resume the session from an interactive Aishe shell and try again.",
            );
        }
        if stream_output {
            println!("  ↳ attached interactive terminal (Ctrl-C interrupts the child)");
        }
        executor.run_interactive_captured(command, Duration::from_secs(timeout), stream_output)
    } else {
        executor.run_captured(command, Duration::from_secs(timeout), stream_output)
    };
    ExecutionResult {
        success: code == 0,
        output,
        exit_code: Some(code),
    }
}

fn command_requires_interactive_terminal(command: &str) -> bool {
    let mut command_position = true;
    let mut wrapper = false;
    for token in shell_words_and_operators(command) {
        if matches!(token.as_str(), ";" | "|" | "||" | "&&" | "&" | "(" | ")") {
            command_position = true;
            wrapper = false;
            continue;
        }
        if !command_position {
            continue;
        }
        let token = token.to_ascii_lowercase();
        if token.contains('=') && !token.starts_with('=') {
            continue;
        }
        if wrapper && token.starts_with('-') {
            continue;
        }
        let executable = token.rsplit('/').next().unwrap_or(&token);
        if matches!(
            executable,
            "sudo"
                | "doas"
                | "su"
                | "login"
                | "passwd"
                | "ssh"
                | "sftp"
                | "ftp"
                | "telnet"
                | "gpg"
                | "gpg2"
                | "pinentry"
                | "cryptsetup"
                | "mysql"
                | "psql"
                | "sqlite3"
                | "vim"
                | "vi"
                | "nvim"
                | "nano"
                | "emacs"
                | "less"
                | "more"
                | "top"
                | "htop"
                | "btop"
                | "watch"
        ) {
            return true;
        }
        wrapper = matches!(
            executable,
            "env" | "command" | "exec" | "nohup" | "nice" | "timeout" | "stdbuf"
        );
        command_position = wrapper;
    }
    false
}

/// Minimal quote-aware tokenization for command-head detection. It deliberately
/// does not attempt to evaluate shell expansions; it only prevents words inside
/// quoted arguments from being mistaken for executables and identifies control
/// operators that begin a new pipeline command.
fn shell_words_and_operators(command: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut characters = command.chars().peekable();
    while let Some(character) = characters.next() {
        if escaped {
            current.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            } else {
                current.push(character);
            }
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            quote = Some(character);
            continue;
        }
        if character.is_whitespace() {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            continue;
        }
        if matches!(character, ';' | '|' | '&' | '(' | ')') {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
            let mut operator = character.to_string();
            if matches!(character, '|' | '&') && characters.peek() == Some(&character) {
                operator.push(characters.next().expect("peeked operator"));
            }
            result.push(operator);
            continue;
        }
        current.push(character);
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn command_cwd(work: &ToolWork) -> Result<PathBuf> {
    let requested = work
        .args
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let path = match requested {
        Some(value) if Path::new(value).is_absolute() => PathBuf::from(value),
        Some(value) => work.workspace.join(value),
        None => work.workspace.clone(),
    };
    let canonical = path
        .canonicalize()
        .with_context(|| format!("command cwd {} is invalid", path.display()))?;
    if work.scope == ExecutionScope::Workspace && !canonical.starts_with(&work.workspace) {
        anyhow::bail!("command cwd escapes the workspace scope");
    }
    Ok(canonical)
}

fn run_file_tool(work: &ToolWork) -> ExecutionResult {
    let path = work.args.get("path").and_then(Value::as_str).unwrap_or("");
    let write = matches!(work.tool.as_str(), "write_file" | "edit_file");
    if let Err(error) = validate_tool_path(work, path, write) {
        return failure(&error.to_string());
    }
    let (name, args) = if work.tool == "edit_file" {
        (
            "edit_file",
            serde_json::json!({
                "path":path,
                "find":work.args.get("old").and_then(Value::as_str).unwrap_or(""),
                "replace":work.args.get("new").and_then(Value::as_str).unwrap_or(""),
                "all":false
            }),
        )
    } else {
        (work.tool.as_str(), work.args.clone())
    };
    let (_, output) = crate::tools::execute(name, &args, &work.workspace, false, false);
    ExecutionResult {
        success: !output.starts_with("Error") && !output.starts_with("User declined"),
        output,
        exit_code: None,
    }
}

fn validate_tool_path(work: &ToolWork, value: &str, write: bool) -> Result<PathBuf> {
    if value.is_empty() || value.contains('\0') {
        anyhow::bail!("file path is invalid");
    }
    let candidate = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        work.workspace.join(value)
    };
    let canonical = if write && !candidate.exists() {
        let parent = candidate
            .parent()
            .context("file target has no parent directory")?
            .canonicalize()?;
        parent.join(
            candidate
                .file_name()
                .context("file target has no file name")?,
        )
    } else {
        candidate.canonicalize()?
    };
    if work.scope == ExecutionScope::Workspace && !canonical.starts_with(&work.workspace) {
        anyhow::bail!("file path escapes the workspace scope");
    }
    Ok(canonical)
}

fn search_files(
    work: &ToolWork,
    denied_environment: &HashSet<String>,
    cancel: &Arc<AtomicBool>,
) -> ExecutionResult {
    let query = work.args.get("query").and_then(Value::as_str).unwrap_or("");
    let path = work.args.get("path").and_then(Value::as_str).unwrap_or(".");
    if let Err(error) = validate_tool_path(work, path, false) {
        return failure(&error.to_string());
    }
    let Some(rg) = crate::executor::which("rg") else {
        return failure("search_files requires ripgrep (rg).");
    };
    let mut executor = match agent_executor(work, &work.workspace, denied_environment) {
        Ok(executor) => executor,
        Err(error) => return failure(&error.to_string()),
    };
    executor.set_cancel_flag(Arc::clone(cancel));
    let command = format!(
        "{} --no-heading --line-number --color never --max-columns 2048 --max-count 1000 -- {} {}",
        shell_quote(&rg.to_string_lossy()),
        shell_quote(query),
        shell_quote(path)
    );
    let (code, output) = executor.run_captured(&command, Duration::from_secs(30), false);
    if matches!(code, 0 | 1) {
        ExecutionResult {
            success: true,
            output,
            exit_code: Some(code),
        }
    } else {
        ExecutionResult {
            success: false,
            output,
            exit_code: Some(code),
        }
    }
}

fn apply_patch(
    work: &ToolWork,
    denied_environment: &HashSet<String>,
    cancel: &Arc<AtomicBool>,
) -> ExecutionResult {
    use rand::RngCore;
    use std::fs::OpenOptions;

    let patch = work.args.get("patch").and_then(Value::as_str).unwrap_or("");
    if patch.is_empty()
        || patch.contains("GIT binary patch")
        || patch.lines().any(|line| line.starts_with("Binary files "))
    {
        return failure("apply_patch requires a non-binary unified diff.");
    }
    let targets = match patch_targets(work, patch) {
        Ok(targets) if !targets.is_empty() => targets,
        Ok(_) => return failure("apply_patch did not contain any file targets."),
        Err(error) => return failure(&error.to_string()),
    };
    let preimages = targets
        .iter()
        .map(|path| {
            let existed = path.exists();
            let before = if existed {
                std::fs::read_to_string(path).ok()
            } else {
                None
            };
            (path.clone(), existed, before)
        })
        .collect::<Vec<_>>();

    let mut random = [0u8; 8];
    rand::rng().fill_bytes(&mut random);
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let patch_path = work.workspace.join(format!(".aishe-patch-{suffix}.diff"));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let write_result = options.open(&patch_path).and_then(|mut file| {
        file.write_all(patch.as_bytes())?;
        file.sync_all()
    });
    if let Err(error) = write_result {
        return failure(&format!("Could not stage the patch: {error}"));
    }

    let mut executor = match agent_executor(work, &work.workspace, denied_environment) {
        Ok(executor) => executor,
        Err(error) => {
            let _ = std::fs::remove_file(&patch_path);
            return failure(&error.to_string());
        }
    };
    executor.set_cancel_flag(Arc::clone(cancel));
    let argument = shell_quote(&patch_path.to_string_lossy());
    let check = format!("git apply --check --whitespace=nowarn -- {argument}");
    let (check_code, check_output) = executor.run_captured(&check, Duration::from_secs(30), false);
    if check_code != 0 {
        let _ = std::fs::remove_file(&patch_path);
        return ExecutionResult {
            success: false,
            output: format!("Patch validation failed:\n{check_output}"),
            exit_code: Some(check_code),
        };
    }
    if cancel.load(Ordering::SeqCst) {
        let _ = std::fs::remove_file(&patch_path);
        return failure("Patch application was cancelled after validation.");
    }
    let apply = format!("git apply --whitespace=nowarn -- {argument}");
    let (code, output) = executor.run_captured(&apply, Duration::from_secs(60), false);
    let _ = std::fs::remove_file(&patch_path);
    if code != 0 {
        return ExecutionResult {
            success: false,
            output: format!("Patch application failed:\n{output}"),
            exit_code: Some(code),
        };
    }
    for (path, existed, before) in preimages {
        if !(existed && before.is_none()) {
            crate::undo::record(
                &path,
                existed,
                before,
                "apply_patch",
                &format!("patch {}", path.display()),
            );
        }
    }
    ExecutionResult {
        success: true,
        output: format!("Applied patch to {} file(s).", targets.len()),
        exit_code: Some(0),
    }
}

fn patch_targets(work: &ToolWork, patch: &str) -> Result<Vec<PathBuf>> {
    let mut targets = Vec::new();
    for line in patch.lines().filter(|line| line.starts_with("+++ ")) {
        let raw = line
            .trim_start_matches("+++ ")
            .split('\t')
            .next()
            .unwrap_or("");
        if raw == "/dev/null" {
            continue;
        }
        if raw.starts_with('"') || raw.contains('\0') {
            anyhow::bail!("quoted or invalid patch paths are not supported");
        }
        let relative = raw.strip_prefix("b/").unwrap_or(raw);
        if relative.is_empty() || Path::new(relative).is_absolute() {
            anyhow::bail!("patch path must be workspace-relative");
        }
        let target = validate_tool_path(work, relative, true)?;
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    Ok(targets)
}

fn agent_executor(
    work: &ToolWork,
    cwd: &Path,
    denied_environment: &HashSet<String>,
) -> Result<Executor> {
    #[allow(unused_mut)]
    let mut executor = Executor::new_agent(cwd, denied_environment)?;
    if work.scope == ExecutionScope::Workspace {
        #[cfg(target_os = "linux")]
        match crate::dependencies::bubblewrap_probe() {
            crate::dependencies::BubblewrapState::Usable { .. } => {
                executor.set_sandbox_wrap(crate::sandbox::agent_bwrap_argv(
                    &work.workspace,
                    cwd,
                    work.network,
                )?);
            }
            state => {
                anyhow::bail!(
                    "Workspace execution requires functional bubblewrap; current state: {state:?}"
                )
            }
        }
    }
    Ok(executor)
}

fn sensitive_environment_names(config: &Config) -> HashSet<String> {
    [
        config.providers.openai.api_key_env.as_str(),
        config.providers.anthropic.api_key_env.as_str(),
        "AISHE_PROVIDER_API_KEY",
        "AISHE_BRIDGE_TOKEN",
        "OPENCODE_SERVER_PASSWORD",
    ]
    .into_iter()
    .filter(|name| !name.is_empty())
    .map(|name| name.to_ascii_uppercase())
    .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn fetch_url(work: &ToolWork, cancel: &Arc<AtomicBool>) -> ExecutionResult {
    if cancel.load(Ordering::SeqCst) {
        return failure("Network request was cancelled before it started.");
    }
    if work.network == NetworkPolicy::Deny {
        return failure("Network access is denied for this workspace tool lease.");
    }
    let (_, output) = crate::tools::execute("fetch_url", &work.args, &work.workspace, false, false);
    ExecutionResult {
        success: !output.starts_with("Error"),
        output,
        exit_code: None,
    }
}

fn use_skill(work: &ToolWork, skills: &crate::skills::SkillRegistry) -> ExecutionResult {
    let name = work.args.get("name").and_then(Value::as_str).unwrap_or("");
    match skills.get(name) {
        Some(skill) => success(skill.body.clone()),
        None => failure(&format!("No approved skill named '{name}'.")),
    }
}

fn mcp_call(
    work: &ToolWork,
    mcp: &crate::mcp::McpRegistry,
    cancel: &Arc<AtomicBool>,
) -> ExecutionResult {
    if cancel.load(Ordering::SeqCst) {
        return failure("MCP request was cancelled before it started.");
    }
    let server = work
        .args
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool = work.args.get("tool").and_then(Value::as_str).unwrap_or("");
    if server.is_empty()
        || tool.is_empty()
        || !server
            .bytes()
            .chain(tool.bytes())
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return failure("MCP server/tool identity is invalid.");
    }
    let exposed = format!("mcp__{server}__{tool}");
    let args = work
        .args
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let (_, output) = mcp.call(&exposed, &args);
    ExecutionResult {
        success: !output.starts_with("Error"),
        output,
        exit_code: None,
    }
}

fn ask_user(work: &ToolWork) -> ExecutionResult {
    if !work.interactive || !std::io::stdin().is_terminal() {
        return failure("The agent asked a question, but no interactive terminal is attached.");
    }
    let prompt = work
        .args
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("Agent question");
    print!("  {} ", crate::commands::display_safe(prompt));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    match std::io::stdin().read_line(&mut answer) {
        Ok(_) => success(answer.trim_end().to_string()),
        Err(error) => failure(&error.to_string()),
    }
}

fn success(output: String) -> ExecutionResult {
    ExecutionResult {
        success: true,
        output,
        exit_code: None,
    }
}

fn failure(message: &str) -> ExecutionResult {
    ExecutionResult {
        success: false,
        output: message.to_string(),
        exit_code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(workspace: &Path, tool: &str, args: Value) -> ToolWork {
        ToolWork {
            session_id: "ses_test".into(),
            message_id: "msg_test".into(),
            call_id: "call_test".into(),
            tool: tool.into(),
            args,
            workspace: workspace.canonicalize().unwrap(),
            mode: Mode::Yolo,
            scope: ExecutionScope::Workspace,
            network: NetworkPolicy::Deny,
            interactive: false,
        }
    }

    #[test]
    fn workspace_file_paths_reject_parent_and_symlink_escapes() {
        let root =
            std::env::temp_dir().join(format!("aishe-tool-worker-path-{}", std::process::id()));
        let workspace = root.join("workspace");
        let outside = root.join("outside");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let task = work(
            &workspace,
            "write_file",
            serde_json::json!({"path":"../outside/x"}),
        );
        assert!(validate_tool_path(&task, "../outside/x", true).is_err());
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, workspace.join("link")).unwrap();
            assert!(validate_tool_path(&task, "link/x", true).is_err());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn suggest_never_executes_and_auto_gates_only_risky_actions() {
        let mut item = ToolWork {
            session_id: "ses_test".into(),
            message_id: "msg_test".into(),
            call_id: "call_test".into(),
            tool: "run_command".into(),
            args: serde_json::json!({"command":"true"}),
            workspace: std::env::temp_dir(),
            mode: Mode::Suggest,
            scope: ExecutionScope::Host,
            network: NetworkPolicy::Allow,
            interactive: false,
        };
        let mut approvals = HashSet::new();
        assert!(approve(&mut item, &mut approvals).is_err());
        item.mode = Mode::Auto;
        assert!(approve(&mut item, &mut approvals).is_ok());
        item.args = serde_json::json!({"command":"rm -rf /tmp/aishe-do-not-run"});
        assert!(approve(&mut item, &mut approvals).is_err());
        item.mode = Mode::Yolo;
        assert!(approve(&mut item, &mut approvals).is_ok());
    }

    #[test]
    fn cancellation_prevents_every_tool_before_dispatch() {
        let root = std::env::temp_dir().join(format!("aishe-tool-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let item = work(
            &root,
            "write_file",
            serde_json::json!({"path":"must-not-exist","content":"no"}),
        );
        let cancel = Arc::new(AtomicBool::new(true));
        let mut approvals = HashSet::new();
        let result = execute(
            &item,
            &crate::skills::SkillRegistry::default(),
            &crate::mcp::McpRegistry::default(),
            &mut approvals,
            &cancel,
            &HashSet::new(),
            false,
        );
        assert!(!result.success);
        assert!(result.output.contains("cancelled"));
        assert!(!root.join("must-not-exist").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn worker_join_is_bounded_for_a_stuck_external_call() {
        let handle = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(250));
        });
        assert!(!finish_worker_thread(handle));
    }

    #[test]
    fn interactive_terminal_detection_is_conservative_and_path_aware() {
        for command in [
            "sudo apt update",
            "/usr/bin/ssh user@example.test",
            "git commit && gpg --sign file",
            "env FOO=1 nvim README.md",
            "printf x | less",
        ] {
            assert!(
                command_requires_interactive_terminal(command),
                "expected interactive detection for {command}"
            );
        }
        for command in [
            "printf 'sudo is only text'",
            "echo ssh-compatible",
            "psql-dump database",
            "cargo test",
        ] {
            assert!(
                !command_requires_interactive_terminal(command),
                "unexpected interactive detection for {command}"
            );
        }
    }
}
