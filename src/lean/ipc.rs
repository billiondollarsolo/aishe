//! FIFO IPC between the lean zsh child and the long-lived parent.

use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::{anyhow, Context, Result};

use crate::config::Config;
use crate::executor::Executor;
use crate::session::Session;

pub struct IpcGuard {
    pub req_path: PathBuf,
    pub rep_path: PathBuf,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub fn spawn_ipc(config: Config) -> Result<IpcGuard> {
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
    let stop_thread = Arc::clone(&stop);
    let req_path_thread = req_path.clone();
    let thread = std::thread::Builder::new()
        .name("aishe-lean-ipc".into())
        .spawn(move || {
            let mut executor = match Executor::new() {
                Ok(executor) => executor,
                Err(_) => return,
            };
            crate::context::init(executor.shell());
            let mut session = Session::new(true);
            let mut provider = None;
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
                let reply = crate::lean::handle_ipc_line(
                    &config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    raw,
                );
                let _ = writeln!(rep, "{reply}");
                let _ = rep.flush();
            }
            let _ = std::fs::remove_file(req_path_thread);
        })?;

    Ok(IpcGuard {
        req_path,
        rep_path,
        stop,
        thread: Some(thread),
    })
}

impl Drop for IpcGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mut wake) = OpenOptions::new().write(true).open(&self.req_path) {
            let _ = wake.write_all(b"STOP\n");
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.req_path);
        let _ = std::fs::remove_file(&self.rep_path);
    }
}

fn mkfifo(path: &PathBuf) -> Result<()> {
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
