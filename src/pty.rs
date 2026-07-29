//! PTY front-end: launch the user's *real* interactive zsh inside a
//! pseudo-terminal, with their full configuration and plugins loaded, plus the
//! aishe AI hook injected.
//!
//! This is how `aishe` supports *every* zsh extension — zsh-autosuggestions,
//! zsh-syntax-highlighting, fzf-tab, powerlevel10k, oh-my-zsh — without forking
//! or reimplementing any of them: it runs the genuine zsh ZLE and merely proxies
//! the terminal. Natural-language interception happens inside that zsh via the
//! injected `command_not_found_handler`, which calls back into `aishe`.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use crossterm::style::Stylize;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::config::Config;
use crate::executor::which;
use crate::integration;

/// Set by the SIGTERM/SIGHUP handlers so the main PTY loop breaks and the normal
/// RAII cleanup (RawGuard + ZdotdirGuard) runs. A `kill`/SIGHUP would otherwise
/// bypass those Drops and could leave the terminal in raw mode and the temp
/// ZDOTDIR on disk. The handler does nothing unsafe — just flips this flag; the
/// blocked `reader.read()` returns with EINTR, so the loop sees the flag and
/// exits cleanly, running Drops on the way out.
static TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_term(_sig: libc::c_int) {
    TERMINATED.store(true, Ordering::SeqCst);
}

fn random_shell_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Run the user's real zsh inside a PTY, returning its exit code.
pub fn run_zsh(config: &Config, history_log: &std::path::Path) -> Result<u8> {
    let zsh = which("zsh").ok_or_else(|| {
        anyhow!("zsh not found on $PATH — the interactive front-end requires zsh (install it, or use `aishe -c …` / the bash hook)")
    })?;

    // Build an isolated ZDOTDIR whose startup files load the user's real config
    // and then append the aishe hook. The guard removes the temp dir on every
    // return path (normal exit, `?` error, or panic-unwind); it must outlive zsh,
    // so it's bound for the whole function.
    let zdotdir = make_zdotdir().context("preparing zsh integration dir")?;
    let _zdotdir_guard = ZdotdirGuard(zdotdir.clone());
    let real_zdotdir = std::env::var("ZDOTDIR").unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.display().to_string())
            .unwrap_or_else(|| "/".to_string())
    });

    let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| anyhow!("openpty failed: {e}"))?;

    let mut cmd = CommandBuilder::new(&zsh);
    cmd.arg("-i");
    cmd.env("ZDOTDIR", &zdotdir);
    cmd.env("AISHE_OUR_ZDOTDIR", &zdotdir);
    cmd.env("AISHE_REAL_ZDOTDIR", &real_zdotdir);
    let shell_id = random_shell_id();
    cmd.env("AISHE_SHELL_ID", &shell_id);
    cmd.env("AISHE_MODE", &config.aishe.mode);
    cmd.env("AISHE_SCOPE", &config.backend.default_scope);
    let acceptance_file = std::env::temp_dir().join(format!("aishe-yolo-accept-{shell_id}"));
    std::fs::remove_file(&acceptance_file).ok();
    cmd.env("AISHE_ACCEPTANCE_FILE", &acceptance_file);
    let _acceptance_guard = FileGuard(acceptance_file);
    let display_model = crate::commands::display_safe(config.active_model());
    cmd.env("AISHE_MODEL", &display_model);
    cmd.env(
        "AISHE_FAILURE_HINTS",
        if config.aishe.failure_hints { "1" } else { "0" },
    );
    // `aishe model <name>` runs as a child of zsh, so it cannot directly update
    // the parent shell's AISHE_MODEL. Share the current value through a tiny
    // per-session file that the prompt hook reads before every prompt.
    let model_file = std::env::temp_dir().join(format!("aishe-model-{}", std::process::id()));
    let _model_guard = if std::fs::write(&model_file, &display_model).is_ok() {
        cmd.env("AISHE_MODEL_FILE", &model_file);
        Some(FileGuard(model_file))
    } else {
        None
    };
    cmd.env(
        "AISHE_PTY_PROMPT",
        if config.aishe.pty_prompt { "1" } else { "0" },
    );
    // Shared per-session usage tally: each NL child process appends its metered
    // usage here so we can print a one-line session summary on exit. Removed on
    // every return path by the guard below.
    let usage_file = std::env::temp_dir().join(format!("aishe-usage-{}", std::process::id()));
    std::fs::remove_file(&usage_file).ok();
    cmd.env("AISHE_USAGE_FILE", &usage_file);
    let _usage_guard = FileGuard(usage_file.clone());
    // A separately rendered status file lets the next prompt show last-call and
    // session totals without spawning a helper process from every `precmd`.
    let status_file = std::env::temp_dir().join(format!("aishe-status-{}", std::process::id()));
    crate::usagelog::write_status(
        &status_file,
        &usage_file,
        &config.pricing,
        None,
        &config.aishe.status_line_items,
    );
    cmd.env("AISHE_STATUS_FILE", &status_file);
    cmd.env(
        "AISHE_STATUS_POSITION",
        if config.aishe.status_line {
            config.aishe.status_line_position.as_str()
        } else {
            "off"
        },
    );
    cmd.env(
        "AISHE_STATUS_ITEMS",
        config.aishe.status_line_items.join(","),
    );
    let _status_guard = FileGuard(status_file);
    // Persist interactive commands to aishe's timestamped history log (via a zsh
    // preexec hook), so `aishe history` and semantic search have data — the PTY's
    // commands run in real zsh, not through aishe's executor. When the user's
    // zsh config has no HISTFILE, the wrapper also adopts this file as zsh's
    // native history so Up-arrow/Ctrl-R survive sessions and binary upgrades.
    if let Some(parent) = history_log.parent() {
        let _ = std::fs::create_dir_all(parent);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
        }
    }
    // zsh's SHARE_HISTORY appender expects the history file to exist. Create it
    // privately on first use; the mode only applies to a new file and never
    // changes permissions on an existing user's log.
    {
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(history_log);
    }
    cmd.env("AISHE_HISTFILE", history_log);
    cmd.env(
        "AISHE_SHARE_HISTORY",
        if config.aishe.share_history { "1" } else { "0" },
    );
    if let Ok(cwd) = std::env::current_dir() {
        cmd.cwd(cwd);
    }

    let mut child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| anyhow!("failed to spawn zsh: {e}"))?;
    // The parent does not use the slave end.
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| anyhow!("pty reader: {e}"))?;
    let mut writer = pair
        .master
        .take_writer()
        .map_err(|e| anyhow!("pty writer: {e}"))?;
    let master = pair.master;

    // Raw mode so keystrokes pass straight through to zsh's ZLE.
    crossterm::terminal::enable_raw_mode().context("entering raw mode")?;
    let _guard = RawGuard;

    // Catch SIGTERM/SIGHUP (e.g. `kill`, terminal close) so we break the loop and
    // run the RAII Drops (cooked-mode restore + temp ZDOTDIR removal) instead of
    // dying with the terminal left in raw mode. The handler only sets a flag; the
    // blocked read below returns EINTR and the loop observes it. Reset the flag
    // first so a stale value from a prior call can't short-circuit this session.
    TERMINATED.store(false, Ordering::SeqCst);
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_term as *const () as libc::sighandler_t,
        );
        libc::signal(libc::SIGHUP, handle_term as *const () as libc::sighandler_t);
    }

    let done = Arc::new(AtomicBool::new(false));

    // stdin -> pty
    {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin();
            let mut buf = [0u8; 4096];
            while !done.load(Ordering::Relaxed) {
                match stdin.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).is_err() || writer.flush().is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }

    // Window-resize forwarding (poll; SIGWINCH without async is fiddly).
    {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut last = (cols, rows);
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                if let Ok(size) = crossterm::terminal::size() {
                    if size != last {
                        last = size;
                        let _ = master.resize(PtySize {
                            rows: size.1,
                            cols: size.0,
                            pixel_width: 0,
                            pixel_height: 0,
                        });
                    }
                }
            }
        });
    }

    // pty -> stdout (main thread; ends at EOF when zsh exits).
    let mut stdout = std::io::stdout();
    let mut buf = [0u8; 4096];
    loop {
        if TERMINATED.load(Ordering::SeqCst) {
            break;
        }
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
            // A signal (EINTR) or a real read error both land here; in either case
            // we stop and let the Drops run. Re-check the flag is implicit: we break.
            Err(_) => break,
        }
    }

    done.store(true, Ordering::Relaxed);
    // If we broke out because of SIGTERM/SIGHUP, zsh is probably still running and
    // `wait()` would block; ask it to exit so we can reap it and let the Drops run.
    if TERMINATED.load(Ordering::SeqCst) {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| anyhow!("waiting for zsh: {e}"))?;

    // Restore cooked mode before printing so the summary's newline isn't
    // staircased (the RawGuard would also do this on drop; doing it twice is
    // harmless). zsh has fully exited by now.
    let _ = crossterm::terminal::disable_raw_mode();

    // One-line "what did this session cost" summary, if any AI calls were made
    // and usage display is on. To stderr so it never pollutes piped stdout.
    if config.aishe.show_usage {
        if let Some(line) = crate::usagelog::summarize(&usage_file, &config.pricing) {
            eprintln!("{}", line.dim());
        }
    }

    // Opt-in: keep the semantic index fresh by incrementally embedding the
    // session's new commands on exit. Best-effort and quiet — a missing key or
    // offline embedder just leaves the index as-is.
    if config.aishe.semantic_history && config.aishe.semantic_history_autoindex {
        let store = history_log.with_file_name("history.vec");
        if let Ok(Ok(ix)) = crate::index::reindex(config, &store, history_log, false) {
            if ix.added > 0 {
                eprintln!(
                    "{}",
                    format!(
                        "aishe: indexed {} new command(s) for semantic search",
                        ix.added
                    )
                    .dim()
                );
            }
        }
    }

    Ok((status.exit_code() & 0xff) as u8)
}

/// Removes a file on drop (best-effort), so the per-session usage tally is
/// cleaned up on every return path including panic-unwind.
struct FileGuard(std::path::PathBuf);
impl Drop for FileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Create a temp ZDOTDIR containing `.zshenv` and `.zshrc` that load the user's
/// real config and then the aishe hook.
fn make_zdotdir() -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("aishe-zdotdir-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(".zshenv"), integration::WRAPPER_ZSHENV)?;
    std::fs::write(dir.join(".zshrc"), integration::wrapper_zshrc())?;
    Ok(dir)
}

/// Restores cooked-mode terminal on drop.
struct RawGuard;
impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

/// Removes the temp ZDOTDIR on drop (best-effort, ignoring errors) so each PTY
/// session cleans up its `${TMPDIR}/aishe-zdotdir-<pid>` instead of leaking it.
/// Covers normal exit, error returns, and panic-unwind.
struct ZdotdirGuard(std::path::PathBuf);
impl Drop for ZdotdirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
