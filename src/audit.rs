//! Structured audit logging for AI calls, responses, and AI-initiated actions.
//!
//! When enabled, aishe appends one JSON object per line (JSONL) to a log file,
//! recording each model request, its response (with token usage), and every
//! command the model causes to run (yolo tool calls, auto/confirmed commands),
//! with exit codes. Secrets in logged text are redacted unless turned off.
//!
//! Logging is **off by default** (it writes prompts and outputs to disk). Enable
//! it in `[logging]` or with `AISHE_LOG=1`. The logger is a process-global
//! initialized once at startup, so call sites do not thread it around.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::redact;

/// Cap on any single logged text field, so one giant prompt/response cannot
/// produce an unbounded log line.
const MAX_FIELD_CHARS: usize = 4000;

static AUDIT: OnceLock<Audit> = OnceLock::new();

struct Audit {
    sink: Option<Mutex<std::fs::File>>,
    redact: bool,
    session: String,
    path: Option<PathBuf>,
}

/// Initialize the global audit logger. Safe to call once; later calls are
/// ignored. `enabled` opens the file for appending; `redact` scrubs secrets from
/// logged text.
pub fn init(enabled: bool, path: Option<PathBuf>, redact: bool) {
    let resolved = path.unwrap_or_else(default_path);
    let sink = if enabled {
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&resolved)
            .ok()
            .map(Mutex::new)
    } else {
        None
    };
    let audit = Audit {
        sink,
        redact,
        session: format!("{}-{}", std::process::id(), now_ms()),
        path: Some(resolved),
    };
    let _ = AUDIT.set(audit);
    // Mark the start of a session so log files are easy to segment.
    event(
        "session_start",
        json!({ "version": env!("CARGO_PKG_VERSION") }),
    );
}

/// Whether logging is active (enabled and the file opened).
pub fn is_active() -> bool {
    AUDIT.get().map(|a| a.sink.is_some()).unwrap_or(false)
}

/// The resolved log file path, if logging was initialized.
pub fn log_path() -> Option<PathBuf> {
    AUDIT.get().and_then(|a| a.path.clone())
}

/// Default log location: `$XDG_DATA_HOME/aishe/audit.jsonl`.
pub fn default_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aishe")
        .join("audit.jsonl")
}

/// Write one event. No-op when logging is inactive.
pub fn event(kind: &str, fields: Value) {
    let Some(audit) = AUDIT.get() else { return };
    let Some(sink) = &audit.sink else { return };
    let mut obj = json!({
        "ts_ms": now_ms(),
        "session": audit.session,
        "kind": kind,
    });
    if let (Some(map), Some(extra)) = (obj.as_object_mut(), fields.as_object()) {
        for (k, v) in extra {
            map.insert(k.clone(), v.clone());
        }
    }
    if let Ok(mut f) = sink.lock() {
        let _ = writeln!(f, "{obj}");
    }
}

/// Log an outgoing model request.
pub fn ai_request(mode: &str, model: &str, prompt: &str) {
    if !is_active() {
        return;
    }
    event(
        "ai_request",
        json!({
            "mode": mode,
            "model": model,
            "prompt": field(prompt),
        }),
    );
}

/// Log a model response with token usage (`input`/`output`) and a short summary.
pub fn ai_response(mode: &str, model: &str, summary: &str, input: u64, output: u64) {
    if !is_active() {
        return;
    }
    event(
        "ai_response",
        json!({
            "mode": mode,
            "model": model,
            "summary": field(summary),
            "tokens_in": input,
            "tokens_out": output,
        }),
    );
}

/// Log a failed model call.
pub fn ai_error(mode: &str, model: &str, error: &str) {
    if !is_active() {
        return;
    }
    event(
        "ai_error",
        json!({ "mode": mode, "model": model, "error": field(error) }),
    );
}

/// Log a command the AI caused to run, with its exit code (when known).
pub fn action(source: &str, command: &str, exit: Option<i32>) {
    if !is_active() {
        return;
    }
    event(
        "action",
        json!({ "source": source, "command": field(command), "exit": exit }),
    );
}

/// Redact (per config) and truncate a text field for logging.
fn field(s: &str) -> String {
    let scrubbed = match AUDIT.get() {
        Some(a) if a.redact => redact::redact(s),
        _ => s.to_string(),
    };
    truncate(&scrubbed, MAX_FIELD_CHARS)
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max).collect();
    format!("{kept}…[+{} chars]", s.chars().count() - max)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_caps_long_fields() {
        let s = "x".repeat(10);
        assert_eq!(truncate(&s, 4), "xxxx…[+6 chars]");
        assert_eq!(truncate("short", 10), "short");
    }

    #[test]
    fn default_path_ends_in_audit_jsonl() {
        assert!(default_path().ends_with("aishe/audit.jsonl"));
    }

    #[test]
    fn inactive_logger_is_silent() {
        // Without init(), every helper is a no-op and must not panic.
        ai_request("suggest", "m", "hello");
        ai_response("suggest", "m", "ok", 1, 2);
        action("yolo", "ls", Some(0));
        assert!(!is_active());
    }
}
