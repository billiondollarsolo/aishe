//! Temporarily splice process stdout into the lean PTY sink (agent transcript).

use std::io::Read;
use std::os::fd::{FromRawFd, RawFd};
use std::thread::JoinHandle;

use super::pty_out::PtyOut;

/// While this guard lives, `println!` / `print!` bytes are copied into `PtyOut`
/// (with `\n` → `\r\n`). Restores fd 1 on drop.
pub struct StdoutRedirect {
    saved_fd: RawFd,
    join: Option<JoinHandle<()>>,
}

impl StdoutRedirect {
    pub fn to_pty(pty: PtyOut) -> Option<Self> {
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return None;
        }
        let read_fd = fds[0];
        let write_fd = fds[1];
        let saved = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if saved < 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
            }
            return None;
        }
        if unsafe { libc::dup2(write_fd, libc::STDOUT_FILENO) } < 0 {
            unsafe {
                libc::close(read_fd);
                libc::close(write_fd);
                libc::close(saved);
            }
            return None;
        }
        unsafe {
            libc::close(write_fd);
        }

        let join = std::thread::Builder::new()
            .name("aishe-lean-stdout-pty".into())
            .spawn(move || {
                let mut file = unsafe { std::fs::File::from_raw_fd(read_fd) };
                let mut buf = [0u8; 4096];
                loop {
                    match file.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let chunk = &buf[..n];
                            let mut out = Vec::with_capacity(n + 8);
                            for &b in chunk {
                                if b == b'\n' {
                                    if out.last() != Some(&b'\r') {
                                        out.push(b'\r');
                                    }
                                    out.push(b'\n');
                                } else {
                                    out.push(b);
                                }
                            }
                            let _ = pty.write_all(&out);
                        }
                        Err(_) => break,
                    }
                }
            })
            .ok();

        let Some(join) = join else {
            unsafe {
                libc::dup2(saved, libc::STDOUT_FILENO);
                libc::close(saved);
                libc::close(read_fd);
            }
            return None;
        };

        Some(Self {
            saved_fd: saved,
            join: Some(join),
        })
    }
}

impl Drop for StdoutRedirect {
    fn drop(&mut self) {
        let _ = std::io::Write::flush(&mut std::io::stdout());
        unsafe {
            libc::dup2(self.saved_fd, libc::STDOUT_FILENO);
            libc::close(self.saved_fd);
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
