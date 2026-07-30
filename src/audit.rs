//! Structured audit logging for AI calls, responses, and AI-initiated actions.
//!
//! When enabled, aishe appends one JSON object per line (JSONL) to a log file,
//! recording each model request, its visible response (with token usage), and
//! every tool the model causes to run. Managed-agent records also carry durable
//! session/message/call identities, structured arguments, bounded results,
//! approvals, file changes, and lifecycle events. Secrets in every logged
//! string are redacted unless turned off.
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

/// Cap on a prompt, visible response, reasoning summary, argument string, or
/// diff. This is intentionally much larger than the old summary-only limit so
/// the audit trail normally retains complete conversational records while one
/// pathological value still cannot grow without bound.
const MAX_FIELD_CHARS: usize = 64 * 1024;

/// Command output is useful evidence but is frequently much larger than the
/// request that caused it. Keep a meaningful tail-sized record without turning
/// the audit log into an unbounded transcript of build/download output.
const MAX_OUTPUT_CHARS: usize = 16 * 1024;

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
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&resolved).ok().map(|file| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&resolved, std::fs::Permissions::from_mode(0o600));
            }
            Mutex::new(file)
        })
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
    crate::config::data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aishe")
        .join("audit.jsonl")
}

/// Write one event. No-op when logging is inactive.
pub fn event(kind: &str, fields: Value) {
    let Some(audit) = AUDIT.get() else { return };
    let Some(sink) = &audit.sink else { return };
    // `event` is the final write boundary. Sanitize recursively here so a new
    // structured caller cannot accidentally bypass redaction by placing a
    // secret inside an array/object rather than a top-level text field.
    let fields = sanitize_value(&fields, audit.redact, MAX_FIELD_CHARS);
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

/// Redact and bound captured command/tool output before it is embedded in a
/// structured event. `event` sanitizes it again at the write boundary; this
/// smaller limit is what keeps noisy tool results practical.
pub fn bounded_output(output: &str) -> String {
    let redact_values = AUDIT.get().is_some_and(|audit| audit.redact);
    sanitize_text(output, redact_values, MAX_OUTPUT_CHARS)
}

/// Redact (per config) and truncate a text field for logging.
fn field(s: &str) -> String {
    let redact_values = AUDIT.get().is_some_and(|audit| audit.redact);
    sanitize_text(s, redact_values, MAX_FIELD_CHARS)
}

fn sanitize_text(s: &str, redact_values: bool, max: usize) -> String {
    let scrubbed = if redact_values {
        redact::redact(s)
    } else {
        s.to_string()
    };
    truncate(&scrubbed, max)
}

fn sanitize_value(value: &Value, redact_values: bool, max: usize) -> Value {
    match value {
        Value::String(value) => Value::String(sanitize_text(value, redact_values, max)),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| sanitize_value(value, redact_values, max))
                .collect(),
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_value(value, redact_values, max)))
                .collect(),
        ),
        _ => value.clone(),
    }
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

// --- Reading the log back (for `aishe log` / `aishe usage`) -----------------

/// One parsed audit line. Fields absent for a given `kind` are `None`. The raw
/// JSON is kept so `--json` can re-emit it verbatim.
#[derive(Debug, Clone)]
pub struct Entry {
    pub ts_ms: u64,
    pub session: String,
    pub kind: String,
    pub model: Option<String>,
    pub mode: Option<String>,
    pub tokens_in: Option<u64>,
    pub tokens_out: Option<u64>,
    pub source: Option<String>,
    pub command: Option<String>,
    pub exit: Option<i64>,
    pub backend_session: Option<String>,
    pub message_id: Option<String>,
    pub call_id: Option<String>,
    pub tool: Option<String>,
    pub event: Option<String>,
    pub success: Option<bool>,
    pub duration_ms: Option<u64>,
    /// A short human label: the response summary, request prompt, or error text.
    pub text: Option<String>,
    pub raw: Value,
}

/// Read and parse every line of an audit log. Missing file or malformed lines
/// yield an empty list / are skipped (best-effort, never panics).
pub fn read_entries(path: &std::path::Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .map(entry_from)
        .collect()
}

fn entry_from(v: Value) -> Entry {
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    let u = |k: &str| v.get(k).and_then(|x| x.as_u64());
    Entry {
        ts_ms: u("ts_ms").unwrap_or(0),
        session: s("session").unwrap_or_default(),
        kind: s("kind").unwrap_or_default(),
        model: s("model"),
        mode: s("mode"),
        tokens_in: u("tokens_in"),
        tokens_out: u("tokens_out"),
        source: s("source"),
        command: s("command"),
        exit: v.get("exit").and_then(|x| x.as_i64()),
        backend_session: s("backend_session"),
        message_id: s("message_id"),
        call_id: s("call_id"),
        tool: s("tool"),
        event: s("event"),
        success: v.get("success").and_then(Value::as_bool),
        duration_ms: u("duration_ms"),
        text: s("response")
            .or_else(|| s("summary"))
            .or_else(|| s("error"))
            .or_else(|| s("prompt"))
            .or_else(|| s("output")),
        raw: v,
    }
}

/// Format epoch milliseconds as `YYYY-MM-DD HH:MM` UTC.
pub fn fmt_utc(ts_ms: u64) -> String {
    let secs = (ts_ms / 1000) as i64;
    let (y, m, d) = civil_from_days(secs.div_euclid(86400));
    let sod = secs.rem_euclid(86400);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        sod / 3600,
        (sod % 3600) / 60
    )
}

/// Format epoch milliseconds as `YYYY-MM-DD` UTC (for daily aggregation).
pub fn fmt_date(ts_ms: u64) -> String {
    let (y, m, d) = civil_from_days(((ts_ms / 1000) as i64).div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Civil date from a day count since the Unix epoch (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Current epoch milliseconds (for `--since` cutoffs).
pub fn now_ms_u64() -> u64 {
    now_ms() as u64
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
    fn structured_values_are_recursively_redacted_and_bounded() {
        let value = json!({
            "args": {
                "token": "API_TOKEN=secret-value",
                "nested": ["safe", "x".repeat(10)]
            }
        });
        let clean = sanitize_value(&value, true, 4);
        assert_ne!(clean["args"]["token"], "API_TOKEN=secret-value");
        assert_eq!(clean["args"]["nested"][0], "safe");
        assert_eq!(clean["args"]["nested"][1], "xxxx…[+6 chars]");
    }

    #[test]
    fn default_path_ends_in_audit_jsonl() {
        assert!(default_path().ends_with("aishe/audit.jsonl"));
    }

    #[test]
    fn utc_formatting_matches_known_epochs() {
        assert_eq!(fmt_date(0), "1970-01-01");
        assert_eq!(fmt_utc(0), "1970-01-01 00:00");
        // 2026-06-12 22:45:40 UTC = 1781304340 s.
        assert_eq!(fmt_utc(1_781_304_340_000), "2026-06-12 22:45");
        assert_eq!(fmt_date(1_781_304_340_000), "2026-06-12");
    }

    #[test]
    fn read_entries_parses_kinds() {
        let dir = std::env::temp_dir().join(format!("aishe-audit-rd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.jsonl");
        std::fs::write(
            &p,
            "{\"ts_ms\":1,\"session\":\"s1\",\"kind\":\"ai_response\",\"model\":\"gpt-4o\",\"tokens_in\":10,\"tokens_out\":5,\"summary\":\"ok\"}\n\
             {\"ts_ms\":2,\"session\":\"s1\",\"kind\":\"action\",\"source\":\"yolo\",\"command\":\"ls\",\"exit\":0}\n\
             not json — skipped\n",
        )
        .unwrap();
        let es = read_entries(&p);
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].kind, "ai_response");
        assert_eq!(es[0].tokens_in, Some(10));
        assert_eq!(es[0].model.as_deref(), Some("gpt-4o"));
        assert_eq!(es[1].command.as_deref(), Some("ls"));
        assert_eq!(es[1].exit, Some(0));
        // Missing file → empty, no panic.
        assert!(read_entries(&dir.join("nope.jsonl")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_entries_parses_managed_tool_identity() {
        let value = json!({
            "ts_ms": 3,
            "session": "process-1",
            "kind": "tool_result",
            "backend_session": "ses_1",
            "message_id": "msg_1",
            "call_id": "call_1",
            "tool": "run_command",
            "success": true,
            "exit": 0,
            "duration_ms": 42,
            "output": "ok"
        });
        let entry = entry_from(value);
        assert_eq!(entry.backend_session.as_deref(), Some("ses_1"));
        assert_eq!(entry.message_id.as_deref(), Some("msg_1"));
        assert_eq!(entry.call_id.as_deref(), Some("call_1"));
        assert_eq!(entry.tool.as_deref(), Some("run_command"));
        assert_eq!(entry.success, Some(true));
        assert_eq!(entry.duration_ms, Some(42));
        assert_eq!(entry.text.as_deref(), Some("ok"));
    }

    #[test]
    fn inactive_logger_is_silent() {
        // Without init(), every helper is a no-op and must not panic.
        ai_request("suggest", "m", "hello");
        ai_response("suggest", "m", "ok", 1, 2);
        action("yolo", "ls", Some(0));
        assert_eq!(bounded_output("ok"), "ok");
        assert!(!is_active());
    }
}
