//! Shared sink for parent → PTY master writes (lean control plane).
//!
//! FIFO carries control only (`OK` / `STREAM_END` / `FILL_B64` / `CONFIRM_B64` / `RAN` / `ERROR`).
//! Multi-line answers and agent transcripts are written here so the interactive
//! child TTY sees them without flattening through a one-line FIFO reply.

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

/// Cloneable handle shared by the stdin→PTY pump and the lean IPC thread.
#[derive(Clone, Default)]
pub struct PtyOut {
    inner: Arc<Mutex<Inner>>,
}

enum Inner {
    /// Discard (unit tests that only care about FIFO control, or pre-attach).
    Null,
    /// Live PTY master writer (or any sink).
    Writer(Box<dyn Write + Send>),
    /// In-memory capture for tests.
    Capture(Vec<u8>),
}

impl Default for Inner {
    fn default() -> Self {
        Inner::Null
    }
}

impl PtyOut {
    pub fn null() -> Self {
        Self::default()
    }

    pub fn from_writer(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::Writer(writer))),
        }
    }

    pub fn capture() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::Capture(Vec::new()))),
        }
    }

    pub fn is_capturing(&self) -> bool {
        matches!(
            *self.inner.lock().unwrap_or_else(|e| e.into_inner()),
            Inner::Capture(_)
        )
    }

    pub fn is_live(&self) -> bool {
        !matches!(
            *self.inner.lock().unwrap_or_else(|e| e.into_inner()),
            Inner::Null
        )
    }

    /// Raw byte write (stdin pump). No newline translation.
    pub fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *guard {
            Inner::Null => Ok(()),
            Inner::Writer(w) => {
                w.write_all(buf)?;
                w.flush()
            }
            Inner::Capture(v) => {
                v.extend_from_slice(buf);
                Ok(())
            }
        }
    }

    /// User-visible text. Translates `\n` → `\r\n` for raw-mode PTY masters.
    pub fn write_user_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut out = String::with_capacity(text.len() + 8);
        for ch in text.chars() {
            if ch == '\n' {
                out.push('\r');
                out.push('\n');
            } else if ch != '\r' {
                out.push(ch);
            }
        }
        let _ = self.write_all(out.as_bytes());
    }

    pub fn write_user_line(&self, text: &str) {
        let mut line = text.to_string();
        if !line.ends_with('\n') {
            line.push('\n');
        }
        self.write_user_text(&line);
    }

    pub fn take_capture(&self) -> String {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        match &mut *guard {
            Inner::Capture(v) => {
                let s = String::from_utf8_lossy(v).into_owned();
                v.clear();
                s
            }
            _ => String::new(),
        }
    }
}

/// [`std::io::Write`] adapter so modes streamers can push deltas into the PTY.
pub struct PtyWrite<'a> {
    pty: &'a PtyOut,
}

impl<'a> PtyWrite<'a> {
    pub fn new(pty: &'a PtyOut) -> Self {
        Self { pty }
    }
}

impl Write for PtyWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.pty.write_user_text(&text);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
