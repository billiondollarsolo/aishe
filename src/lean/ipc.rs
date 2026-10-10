//! FIFO IPC between the lean zsh child and the long-lived parent.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Context, Result};

use crate::config::Config;
use crate::executor::Executor;
use crate::session::Session;

use super::nl::LeanWarm;
use super::pty_out::PtyOut;
use super::sessions::LeanSessionStore;

/// Per-shell handoff files. The parent must use explicit paths: CommandBuilder
/// env values are inherited by zsh, not by the already-running IPC thread.
#[derive(Clone, Debug, Default)]
pub struct LeanShellFiles {
    pub model: Option<PathBuf>,
    pub selection: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub scope: Option<PathBuf>,
    pub usage: Option<PathBuf>,
    pub status: Option<PathBuf>,
    pub commands: Option<PathBuf>,
    pub execution_state: Option<PathBuf>,
    pub background_status: Option<PathBuf>,
    pub background_events: Option<PathBuf>,
    pub background_seen: Option<PathBuf>,
    pub handoff_control: Option<PathBuf>,
}

impl LeanShellFiles {
    fn apply_selection(&self, config: &mut Config) -> Result<bool> {
        for (path, allowed, field) in [
            (
                &self.scope,
                &["workspace", "host"][..],
                &mut config.backend.default_scope,
            ),
            (
                &self.output,
                &["focus", "compact", "detailed"][..],
                &mut config.backend.output,
            ),
        ] {
            if let Some(path) = path {
                if let Ok(value) = std::fs::read_to_string(path) {
                    let value = value.trim();
                    if !allowed.contains(&value) {
                        anyhow::bail!("invalid shell handoff value");
                    }
                    *field = value.into();
                }
            }
        }
        let Some(path) = &self.selection else {
            return Ok(false);
        };
        let selection = crate::connection::read_selection(path)?;
        let changed = selection.connection_id != config.active_connection_id()
            || selection.model_id != config.active_model()
            || (!selection.reasoning_effort.is_empty()
                && selection.reasoning_effort != config.active_reasoning_effort());
        if changed {
            config.select_connection(&selection.connection_id)?;
            config.set_active_model(selection.model_id);
            if !selection.reasoning_effort.is_empty() {
                config.set_active_reasoning_effort(selection.reasoning_effort);
            }
        }
        Ok(changed)
    }

    fn sync(&self, config: &Config, warm: &LeanWarm, last: Option<(crate::usage::Usage, &str)>) {
        for (path, text) in [
            (
                &self.model,
                crate::commands::display_safe(config.active_model()),
            ),
            (&self.output, config.backend.output.clone()),
            (&self.scope, config.backend.default_scope.clone()),
        ] {
            if let Some(path) = path {
                let _ = crate::config::write_atomic(path, text.as_bytes());
            }
        }
        if let Some(path) = &self.selection {
            let previous = crate::connection::read_selection(path).ok();
            let scope = previous
                .as_ref()
                .filter(|selection| {
                    selection.connection_id == config.active_connection_id()
                        && selection.model_id == config.active_model()
                        && selection.reasoning_effort == config.active_reasoning_effort()
                })
                .map(|selection| selection.selection_scope.as_str())
                .unwrap_or("shell");
            let _ = crate::connection::write_selection(
                path,
                &crate::connection::ShellSelection {
                    connection_id: config.active_connection_id().into(),
                    connection_label: config
                        .active_connection()
                        .map(|connection| connection.label.clone())
                        .unwrap_or_else(|| config.active_connection_id().into()),
                    provider: config.active_provider_name().into(),
                    endpoint_host: url::Url::parse(&config.active_provider_config().base_url)
                        .ok()
                        .and_then(|url| url.host_str().map(str::to_owned))
                        .unwrap_or_else(|| "unknown".into()),
                    auth_label: config
                        .active_connection()
                        .map(crate::config::ConnectionConfig::auth_label)
                        .unwrap_or_else(|| "Auto (legacy)".into()),
                    model_id: config.active_model().into(),
                    reasoning_effort: config.active_reasoning_effort().into(),
                    selection_scope: scope.into(),
                },
            );
        }
        if warm.commands.is_some() {
            if let Some(path) = &self.commands {
                let body = super::nl::command_completion_text(warm.commands.as_ref());
                let _ = crate::config::write_atomic(path, body.as_bytes());
            }
        }
        if let (Some(status), Some(usage)) = (&self.status, &self.usage) {
            crate::usagelog::write_status(
                status,
                usage,
                &config.pricing,
                last,
                &config.effective_status_line_items(),
            );
            crate::usagelog::merge_status(
                status,
                &warm.recent_status(config.active_connection_id()),
            );
        }
    }
}

pub struct IpcGuard {
    pub req_path: PathBuf,
    pub rep_path: PathBuf,
    pub busy: Arc<AtomicBool>,
    pub cancelled: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    background_thread: Option<JoinHandle<()>>,
    background_events: Option<PathBuf>,
}

/// Poll the shared task index independently of the foreground agent. Only a
/// changed summary wakes ZLE; the shell never spawns a process or parses JSON
/// while editing a command. The FIFO writer is nonblocking so a busy or closed
/// shell cannot delay a task or the parent shutdown.
fn spawn_background_watcher(
    files: &LeanShellFiles,
    stop: &Arc<AtomicBool>,
) -> Result<Option<JoinHandle<()>>> {
    let (Some(status), Some(events)) = (&files.background_status, &files.background_events) else {
        return Ok(None);
    };
    mkfifo(events)?;
    let initial = crate::background::shell_status_text(None, files.background_seen.as_deref())
        .unwrap_or_else(|_| "running\t0\nready\t0\nattention\t0\nneeds_you\t0\nqueued\t0\n".into());
    crate::config::write_atomic(status, initial.as_bytes())?;
    let status = status.clone();
    let events = events.clone();
    let seen = files.background_seen.clone();
    let stop = Arc::clone(stop);
    Ok(Some(
        std::thread::Builder::new()
            .name("aishe-task-status".into())
            .spawn(move || {
                let mut previous = initial;
                let mut notification_pending = true;
                while !stop.load(Ordering::Relaxed) {
                    crate::background::scheduler::wake_pending_workflows();
                    let _ = crate::background::refresh_task_cache();
                    if let Ok(summary) = crate::background::shell_status_text(None, seen.as_deref())
                    {
                        if summary != previous
                            && crate::config::write_atomic(&status, summary.as_bytes()).is_ok()
                        {
                            previous = summary;
                            notification_pending = true;
                        }
                    }
                    if notification_pending {
                        if let Ok(mut signal) = OpenOptions::new()
                            .write(true)
                            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
                            .open(&events)
                        {
                            if signal.metadata().is_ok_and(|metadata| {
                                metadata.file_type().is_fifo()
                                    && metadata.uid() == unsafe { libc::geteuid() }
                            }) && signal.write_all(b"\n").is_ok()
                            {
                                notification_pending = false;
                            }
                        }
                    }
                    std::thread::park_timeout(std::time::Duration::from_secs(2));
                }
            })?,
    ))
}

pub fn spawn_ipc(config: Config, pty: PtyOut) -> Result<IpcGuard> {
    spawn_ipc_with_files(config, pty, LeanShellFiles::default())
}

pub fn spawn_ipc_with_files(
    config: Config,
    pty: PtyOut,
    files: LeanShellFiles,
) -> Result<IpcGuard> {
    let dir = std::env::temp_dir();
    let id = std::process::id();
    let req_path = dir.join(format!("aishe-lean-{id}.req"));
    let rep_path = dir.join(format!("aishe-lean-{id}.rep"));
    let _ = std::fs::remove_file(&req_path);
    let _ = std::fs::remove_file(&rep_path);
    mkfifo(&req_path)?;
    mkfifo(&rep_path)?;

    let req = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&req_path)
        .with_context(|| format!("opening {}", req_path.display()))?;
    let mut rep = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&rep_path)
        .with_context(|| format!("opening {}", rep_path.display()))?;

    let stop = Arc::new(AtomicBool::new(false));
    let busy = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::new(AtomicBool::new(false));
    let background_thread = spawn_background_watcher(&files, &stop)?;
    let background_events = files.background_events.clone();
    let busy_thread = Arc::clone(&busy);
    let cancelled_thread = Arc::clone(&cancelled);
    let stop_thread = Arc::clone(&stop);
    let req_path_thread = req_path.clone();
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "/".into());
    let model = config.active_model().to_string();
    let thread = std::thread::Builder::new()
        .name("aishe-lean-ipc".into())
        .spawn(move || {
            let mut executor = match Executor::new() {
                Ok(executor) => executor,
                Err(_) => return,
            };
            crate::context::init(executor.shell());
            executor.set_cancel_flag(Arc::clone(&cancelled_thread));
            executor.set_terminal_input_owned_elsewhere(true);
            let mut session = Session::new(true);
            let mut store = Some(LeanSessionStore::create(&cwd, &model));
            let mut provider = None;
            let mut warm = LeanWarm::default();
            let mut config = config;
            let mut last_usage = std::collections::BTreeMap::new();
            files.sync(&config, &warm, None);
            let mut reader = BufReader::new(req);
            let mut line = String::new();
            while !stop_thread.load(Ordering::Relaxed) {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        if stop_thread.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
                let raw = line.trim_end_matches(['\n', '\r']);
                if raw == "STOP" {
                    break;
                }
                let operation = raw.split('\t').next().unwrap_or("");
                if matches!(operation, "NL" | "FIX" | "CONFIRM_YES" | "SLASH") {
                    if let Some(path) = &files.execution_state {
                        let denied = crate::executor::sensitive_environment_names(&config);
                        match super::state::consume_environment(path, &denied) {
                            Ok(environment) => {
                                executor.replace_agent_environment(environment, &denied);
                            }
                            Err(error) => {
                                let _ = writeln!(rep, "ERROR\t{error}");
                                let _ = rep.flush();
                                continue;
                            }
                        }
                    }
                }
                match files.apply_selection(&mut config) {
                    Ok(true) => provider = None,
                    Ok(false) => {}
                    Err(error) => {
                        let _ = writeln!(
                            rep,
                            "ERROR\t{}",
                            crate::commands::display_safe(&error.to_string())
                        );
                        let _ = rep.flush();
                        continue;
                    }
                }
                if raw.starts_with("SLASH\t") {
                    let command = raw
                        .rsplit('\t')
                        .next()
                        .unwrap_or("")
                        .split_whitespace()
                        .next();
                    if config.aishe.budget_usd > 0.0
                        || matches!(command, Some("/usage" | "/status"))
                    {
                        if let Some(path) = &files.usage {
                            warm.replace_usage_from_log(path);
                        }
                    }
                }
                if config.aishe.budget_usd > 0.0
                    && (raw.starts_with("NL\t") || raw.starts_with("FIX\t"))
                {
                    if let Some(path) = &files.usage {
                        warm.replace_usage_from_log(path);
                    }
                }
                let before_provider = provider.clone();
                let before = provider
                    .as_ref()
                    .map(|p: &Arc<dyn crate::providers::Provider>| p.meter().snapshot())
                    .unwrap_or_default();
                let call_model = config.active_model().to_string();
                let call_connection = config.active_connection_id().to_string();
                cancelled_thread.store(false, Ordering::SeqCst);
                crate::agent::controller::INTERRUPTED.store(false, Ordering::SeqCst);
                busy_thread.store(true, Ordering::SeqCst);
                let _handoff_control = files
                    .handoff_control
                    .as_deref()
                    .map(crate::agent::native::handoff::ControlGuard::set);
                let mut reply = crate::lean::handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    &mut store,
                    &mut warm,
                    &pty,
                    raw,
                );
                if cancelled_thread.load(Ordering::SeqCst) {
                    reply = "CANCELLED".into();
                }
                let after = provider
                    .as_ref()
                    .map(|p| p.meter().snapshot())
                    .unwrap_or_default();
                let same_provider = match (&before_provider, &provider) {
                    (Some(before), Some(after)) => Arc::ptr_eq(before, after),
                    (None, Some(_)) => true,
                    _ => false,
                };
                if same_provider {
                    let mut delta = after.delta_since(before);
                    if !config.aishe.provider_fallback.is_empty() {
                        // The unified meter cannot attribute a fallback turn
                        // to the configured model's price.
                        delta = delta.without_attribution();
                    }
                    if !delta.is_empty() {
                        warm.record_usage(delta, &call_model, &call_connection);
                        if let Some(path) = &files.usage {
                            crate::usagelog::append_attributed(
                                path,
                                delta,
                                &call_model,
                                Some(&call_connection),
                            );
                        }
                        last_usage.insert(call_connection, (delta, call_model));
                    }
                }
                files.sync(
                    &config,
                    &warm,
                    last_usage
                        .get(config.active_connection_id())
                        .map(|(usage, model)| (*usage, model.as_str())),
                );
                let _ = writeln!(rep, "{reply}");
                let _ = rep.flush();
                busy_thread.store(false, Ordering::SeqCst);
            }
            let _ = std::fs::remove_file(req_path_thread);
        })?;

    Ok(IpcGuard {
        req_path,
        rep_path,
        busy,
        cancelled,
        stop,
        thread: Some(thread),
        background_thread,
        background_events,
    })
}

impl Drop for IpcGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        crate::agent::controller::INTERRUPTED.store(true, Ordering::SeqCst);
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.background_thread.take() {
            thread.thread().unpark();
            let _ = thread.join();
        }
        if let Ok(mut wake) = OpenOptions::new().write(true).open(&self.req_path) {
            let _ = wake.write_all(b"STOP\n");
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.req_path);
        let _ = std::fs::remove_file(&self.rep_path);
        if let Some(events) = &self.background_events {
            let _ = std::fs::remove_file(events);
        }
    }
}

fn mkfifo(path: &Path) -> Result<()> {
    let cstr = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| anyhow!("fifo path contains NUL"))?;
    let rc = unsafe { libc::mkfifo(cstr.as_ptr(), 0o600) };
    if rc != 0 {
        return Err(anyhow!(
            "mkfifo {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_shell_files_sync_selection_and_apply_cli_changes() {
        let root = std::env::temp_dir().join(format!("aishe-ui-files-{}", rand::random::<u64>()));
        std::fs::create_dir_all(&root).unwrap();
        let files = LeanShellFiles {
            model: Some(root.join("model")),
            selection: Some(root.join("selection")),
            output: Some(root.join("output")),
            scope: Some(root.join("scope")),
            commands: Some(root.join("commands")),
            ..Default::default()
        };
        let mut config = Config::default();
        let warm = LeanWarm::default();
        config.set_active_model("first-model".into());
        files.sync(&config, &warm, None);
        let selection_path = files.selection.as_ref().unwrap();
        let mut selection = crate::connection::read_selection(selection_path).unwrap();
        assert_eq!(selection.model_id, "first-model");
        assert_eq!(
            std::fs::read_to_string(files.model.as_ref().unwrap()).unwrap(),
            "first-model"
        );
        selection.connection_id = "openai".into();
        selection.model_id = "cli-model".into();
        crate::connection::write_selection(selection_path, &selection).unwrap();
        assert!(files.apply_selection(&mut config).unwrap());
        assert_eq!(config.active_connection_id(), "openai");
        assert_eq!(config.active_model(), "cli-model");
        assert!(!files.apply_selection(&mut config).unwrap());
        config.backend.output = "detailed".into();
        config.backend.default_scope = "host".into();
        files.sync(&config, &warm, None);
        assert_eq!(
            std::fs::read_to_string(files.output.as_ref().unwrap()).unwrap(),
            "detailed"
        );
        assert_eq!(
            std::fs::read_to_string(files.scope.as_ref().unwrap()).unwrap(),
            "host"
        );
        assert_eq!(
            std::fs::read_to_string(files.model.as_ref().unwrap()).unwrap(),
            "cli-model"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
