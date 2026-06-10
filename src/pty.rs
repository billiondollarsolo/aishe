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
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use crate::config::Config;
use crate::executor::which;
use crate::integration;

/// Run the user's real zsh inside a PTY, returning its exit code.
pub fn run_zsh(config: &Config) -> Result<u8> {
    let zsh = which("zsh").ok_or_else(|| {
        anyhow!("zsh not found on $PATH — the PTY front-end requires zsh (install it, or use the default reedline front-end)")
    })?;

    // Build an isolated ZDOTDIR whose startup files load the user's real config
    // and then append the aishe hook.
    let zdotdir = make_zdotdir().context("preparing zsh integration dir")?;
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
    cmd.env("AISHE_MODE", &config.aishe.mode);
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
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if stdout.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = stdout.flush();
            }
            Err(_) => break,
        }
    }

    done.store(true, Ordering::Relaxed);
    let status = child.wait().map_err(|e| anyhow!("waiting for zsh: {e}"))?;
    Ok((status.exit_code() & 0xff) as u8)
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
