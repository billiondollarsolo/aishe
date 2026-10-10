//! Shared user-visible terminal output sink (lean control plane).
//!
//! FIFO carries control only (`OK` / `STREAM_END` / `FILL_B64` / `CONFIRM_B64` / `RAN` / `ERROR`).
//! Multi-line answers and agent transcripts go to the original terminal output,
//! never the child PTY's master writer (which would submit them as shell input).

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Cloneable display handle shared by the PTY output relay and lean IPC thread.
#[derive(Clone, Default)]
pub struct PtyOut {
    inner: Arc<Mutex<Inner>>,
    cancelled: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
enum Inner {
    /// Discard (unit tests that only care about FIFO control, or pre-attach).
    #[default]
    Null,
    /// Original terminal output descriptor (or any output sink).
    Writer(Box<dyn Write + Send>),
    /// In-memory capture for tests.
    Capture(Vec<u8>),
}

impl PtyOut {
    pub fn null() -> Self {
        Self::default()
    }

    pub fn from_writer(writer: Box<dyn Write + Send>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::Writer(writer))),
            cancelled: None,
        }
    }

    pub fn capture() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner::Capture(Vec::new()))),
            cancelled: None,
        }
    }

    /// Guard only IPC producers. The original relay and cancellation-ACK
    /// handles remain unrestricted and share this same output mutex.
    pub(crate) fn with_cancel_flag(&self, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            cancelled: Some(cancelled),
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

    /// Raw output bytes (PTY relay). No newline translation.
    pub fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        let mut guard = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // A producer that checked before acquiring the mutex could otherwise
        // write after the unrestricted cancellation ACK has reached the user.
        if self
            .cancelled
            .as_ref()
            .is_some_and(|cancelled| cancelled.load(Ordering::SeqCst))
        {
            return Ok(());
        }
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

    /// User-visible text. Translates `\n` → `\r\n` for raw-mode terminals.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn guarded_raw_and_text_producers_stop_after_ack_while_relay_remains_live() {
        let display = PtyOut::capture();
        let cancelled = Arc::new(AtomicBool::new(false));
        let producer = display.with_cancel_flag(Arc::clone(&cancelled));
        producer.write_user_line("before");
        cancelled.store(true, Ordering::SeqCst);
        display.write_user_line("cancel ACK");
        producer.write_all(b"forbidden raw").unwrap();
        producer.clone().write_user_line("forbidden line");
        PtyWrite::new(&producer)
            .write_all(b"forbidden streamed delta")
            .unwrap();
        display.write_all(b"next shell prompt").unwrap();
        assert_eq!(
            display.take_capture(),
            "before\r\ncancel ACK\r\nnext shell prompt"
        );
    }

    #[test]
    fn ack_is_ordered_after_inflight_output_and_queued_cancelled_output_is_suppressed() {
        struct BarrierWriter {
            entered: Option<mpsc::Sender<()>>,
            resume: mpsc::Receiver<()>,
            bytes: Arc<Mutex<Vec<u8>>>,
        }

        impl Write for BarrierWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if let Some(entered) = self.entered.take() {
                    entered.send(()).unwrap();
                    self.resume
                        .recv_timeout(Duration::from_secs(5))
                        .map_err(io::Error::other)?;
                }
                self.bytes.lock().unwrap().extend_from_slice(bytes);
                Ok(bytes.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let bytes = Arc::new(Mutex::new(Vec::new()));
        let (entered, ready) = mpsc::channel();
        let (resume, resumed) = mpsc::channel();
        let display = PtyOut::from_writer(Box::new(BarrierWriter {
            entered: Some(entered),
            resume: resumed,
            bytes: Arc::clone(&bytes),
        }));
        let cancelled = Arc::new(AtomicBool::new(false));
        let producer = display.with_cancel_flag(Arc::clone(&cancelled));
        let inflight = producer.clone();
        let inflight = std::thread::spawn(move || inflight.write_all(b"inflight ").unwrap());
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        // This is the same token-before-ACK order used by RequestControl.
        // The existing write holds the output mutex until explicitly released.
        cancelled.store(true, Ordering::SeqCst);
        let ack = std::thread::spawn(move || display.write_all(b"ACK").unwrap());
        let queued = std::thread::spawn(move || producer.write_all(b"forbidden").unwrap());
        resume.send(()).unwrap();
        inflight.join().unwrap();
        ack.join().unwrap();
        queued.join().unwrap();
        assert_eq!(*bytes.lock().unwrap(), b"inflight ACK");
    }
}
