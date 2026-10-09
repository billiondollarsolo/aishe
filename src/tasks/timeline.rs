//! Typed observations of task execution. This journal never reads provider
//! continuation items or infers successful work from assistant prose.

use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

pub const MAX_EVENTS: usize = 256;
const MAX_SUBJECT_CHARS: usize = 160;
const MAX_DETAIL_CHARS: usize = 2048;
// Character bounds include full-width UTF-8 and JSON escaping. Keep enough
// bytes for every accepted event while still bounding every read.
const MAX_JOURNAL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    TaskStarted,
    Admission,
    ProviderTurn,
    ToolPlanned,
    ToolStarted,
    ToolResult,
    Question,
    Approval,
    FollowupQueued,
    FollowupReceived,
    CheckResult,
    HandoffRequested,
    HandoffCompleted,
    Resumed,
    Finished,
    WorkflowQueued,
    WorkflowReleased,
    /// A human/model-written plan, not an observed effect or check result.
    PlanNote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventOutcome {
    Completed,
    Failed,
    Cancelled,
    Declined,
    NotExecuted,
    Uncertain,
    Waiting,
    HandedOff,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    #[default]
    Native,
    Background,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskTimelineEvent {
    /// Monotonic within its source. Combining sources preserves both counters.
    pub sequence: u64,
    pub at_ms: u128,
    #[serde(default)]
    pub source: EventSource,
    #[serde(default)]
    pub source_id: String,
    pub kind: EventKind,
    pub subject: String,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<EventOutcome>,
}

impl TaskTimelineEvent {
    pub(super) fn new(
        sequence: u64,
        source: EventSource,
        source_id: &str,
        kind: EventKind,
        subject: &str,
        detail: &str,
        outcome: Option<EventOutcome>,
    ) -> Self {
        Self {
            sequence,
            at_ms: super::now_ms(),
            source,
            source_id: safe_text(source_id, MAX_SUBJECT_CHARS, false),
            kind,
            subject: safe_text(subject, MAX_SUBJECT_CHARS, false),
            detail: safe_text(detail, MAX_DETAIL_CHARS, true),
            outcome,
        }
    }

    /// Reapply bounds when reading old or imported private journals.
    pub fn sanitized(mut self) -> Self {
        self.source_id = safe_text(&self.source_id, MAX_SUBJECT_CHARS, false);
        self.subject = safe_text(&self.subject, MAX_SUBJECT_CHARS, false);
        self.detail = safe_text(&self.detail, MAX_DETAIL_CHARS, true);
        self
    }
}

pub(super) fn push(
    record: &mut super::Record,
    kind: EventKind,
    subject: &str,
    detail: &str,
    outcome: Option<EventOutcome>,
) {
    record.timeline_sequence = record
        .timeline_sequence
        .max(record.timeline.last().map_or(0, |event| event.sequence));
    record.timeline_sequence = record.timeline_sequence.saturating_add(1);
    if record.timeline.len() >= MAX_EVENTS {
        let dropped = record.timeline.len() + 1 - MAX_EVENTS;
        record.timeline.drain(..dropped);
        record.timeline_dropped = record.timeline_dropped.saturating_add(dropped);
    }
    let mut event = TaskTimelineEvent::new(
        record.timeline_sequence,
        EventSource::Native,
        &record.id,
        kind,
        subject,
        detail,
        outcome,
    );
    event.at_ms = event
        .at_ms
        .max(record.timeline.last().map_or(0, |previous| previous.at_ms));
    record.timeline.push(event);
}

/// Render only public tool arguments. Credential-valued JSON keys are masked
/// even when a short value does not match the general text redactor.
pub(super) fn tool_arguments(value: &serde_json::Value) -> String {
    fn scrub(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(fields) => serde_json::Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| {
                        let lower = key.to_ascii_lowercase().replace('-', "_");
                        let private = [
                            "password",
                            "passwd",
                            "secret",
                            "token",
                            "credential",
                            "authorization",
                            "encrypted",
                        ]
                        .iter()
                        .any(|part| lower.contains(part))
                            || lower == "auth"
                            || lower == "key"
                            || lower == "apikey"
                            || lower == "api_key"
                            || lower == "private_key"
                            || lower == "access_key";
                        (
                            safe_text(key, 160, false),
                            if private {
                                serde_json::Value::String("<redacted>".into())
                            } else {
                                scrub(value)
                            },
                        )
                    })
                    .collect(),
            ),
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(scrub).collect())
            }
            serde_json::Value::String(text) => {
                serde_json::Value::String(crate::redact::redact(text))
            }
            value => value.clone(),
        }
    }
    serde_json::to_string_pretty(&scrub(value)).unwrap_or_default()
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TimelineJournal {
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub dropped: usize,
    #[serde(default)]
    pub events: Vec<TaskTimelineEvent>,
}

/// Append an external transition without modifying the worker's checkpoint.
/// The separate private lock serializes human actions from multiple shells.
pub fn append_background(
    id: &str,
    kind: EventKind,
    subject: &str,
    detail: &str,
    outcome: Option<EventOutcome>,
) -> Result<()> {
    let path = background_path(id)?;
    append_at(&path, id, kind, subject, detail, outcome)
}

pub fn read_background(id: &str) -> Result<TimelineJournal> {
    read_at(&background_path(id)?)
}

fn background_path(id: &str) -> Result<PathBuf> {
    if !super::valid_id(id) || !(8..=80).contains(&id.len()) {
        anyhow::bail!("invalid background task ID");
    }
    let root = crate::config::data_root().context("task data directory is unavailable")?;
    // Keep the authoritative task record untouched. AISHE_TASKS_DIR applies to
    // native checkpoints; the background ledger lives beside record.json.
    Ok(root
        .join("aishe")
        .join("background-tasks")
        .join(id)
        .join("timeline.json"))
}

fn append_at(
    path: &Path,
    id: &str,
    kind: EventKind,
    subject: &str,
    detail: &str,
    outcome: Option<EventOutcome>,
) -> Result<()> {
    let parent = path.parent().context("timeline has no parent directory")?;
    std::fs::create_dir_all(parent)?;
    super::set_private(parent, 0o700);
    let lock_path = path.with_extension("lock");
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    no_follow(&mut options);
    let lock = options.open(&lock_path)?;
    super::set_private(&lock_path, 0o600);
    lock.lock_exclusive()?;
    let mut journal = read_at(path)?;
    journal.sequence = journal
        .sequence
        .max(journal.events.last().map_or(0, |event| event.sequence))
        .saturating_add(1);
    if journal.events.len() >= MAX_EVENTS {
        let dropped = journal.events.len() + 1 - MAX_EVENTS;
        journal.events.drain(..dropped);
        journal.dropped = journal.dropped.saturating_add(dropped);
    }
    let mut event = TaskTimelineEvent::new(
        journal.sequence,
        EventSource::Background,
        id,
        kind,
        subject,
        detail,
        outcome,
    );
    event.at_ms = event
        .at_ms
        .max(journal.events.last().map_or(0, |previous| previous.at_ms));
    journal.events.push(event);
    crate::config::write_atomic(path, &serde_json::to_vec(&journal)?)?;
    super::set_private(path, 0o600);
    Ok(())
}

fn read_at(path: &Path) -> Result<TimelineJournal> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TimelineJournal::default())
        }
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        anyhow::bail!("task timeline exceeds its private preview limit or is not a regular file");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let mut bytes = Vec::new();
    options
        .open(path)?
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        anyhow::bail!("task timeline exceeds its private preview limit");
    }
    let mut journal: TimelineJournal = serde_json::from_slice(&bytes)?;
    if journal.events.len() > MAX_EVENTS {
        anyhow::bail!("task timeline exceeds its event limit");
    }
    journal.events = journal
        .events
        .into_iter()
        .map(TaskTimelineEvent::sanitized)
        .collect();
    Ok(journal)
}

#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW);
}

#[cfg(not(unix))]
fn no_follow(_options: &mut OpenOptions) {}

fn safe_text(value: &str, limit: usize, multiline: bool) -> String {
    let redacted = crate::redact::redact(value);
    let escaped = redacted
        .chars()
        .flat_map(|character| {
            if character.is_control() && !(multiline && (character == '\n' || character == '\t')) {
                format!("\\u{{{:x}}}", character as u32)
                    .chars()
                    .collect::<Vec<_>>()
            } else {
                vec![character]
            }
        })
        .collect::<String>();
    let mut bounded = escaped.chars().take(limit).collect::<String>();
    if escaped.chars().nth(limit).is_some() {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_path(label: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir()
            .join(format!(
                "aishe-timeline-{}-{label}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
            .join("timeline.json")
    }

    #[test]
    fn concurrent_appends_are_durable_ordered_and_bounded() {
        let path = test_path("concurrent");
        let threads = (0..4)
            .map(|worker| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for item in 0..80 {
                        append_at(
                            &path,
                            "task",
                            EventKind::FollowupQueued,
                            &format!("{worker}/{item}"),
                            "real queue transition",
                            None,
                        )
                        .unwrap();
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        let journal = read_at(&path).unwrap();
        assert_eq!(journal.sequence, 320);
        assert_eq!(journal.events.len(), MAX_EVENTS);
        assert_eq!(journal.dropped, 64);
        assert_eq!(journal.events.first().unwrap().sequence, 65);
        assert_eq!(journal.events.last().unwrap().sequence, 320);
        assert!(journal
            .events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn tool_previews_mask_short_key_values_and_controls_before_truncation() {
        let arguments = serde_json::json!({"nested":{"password":"short", "api_key":"secret-value"}, "command":"printf '\u{1b}[2J'; export TOKEN=private", "encrypted_content":"opaque-raw"});
        let preview = tool_arguments(&arguments);
        let event = TaskTimelineEvent::new(
            1,
            EventSource::Native,
            "task",
            EventKind::ToolPlanned,
            "tool",
            &preview,
            None,
        );
        assert!(!event.detail.contains("short"));
        assert!(!event.detail.contains("secret-value"));
        assert!(!event.detail.contains("opaque-raw"));
        assert!(!event.detail.contains("TOKEN=private"));
        assert!(!event.detail.contains('\u{1b}'));
        assert!(event.detail.contains("<redacted>"));
        let large = format!("{} TOKEN=private", "x".repeat(MAX_DETAIL_CHARS * 2));
        let event = TaskTimelineEvent::new(
            1,
            EventSource::Native,
            "task",
            EventKind::ToolResult,
            "tool",
            &large,
            None,
        );
        assert!(event.detail.chars().count() <= MAX_DETAIL_CHARS + 1);
        assert!(!event.detail.contains("private"));
    }

    #[cfg(unix)]
    #[test]
    fn journal_symlinks_are_rejected_without_reading_the_target() {
        let path = test_path("symlink");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", &path).unwrap();
        assert!(read_at(&path).is_err());
        assert!(append_at(&path, "task", EventKind::Finished, "done", "", None).is_err());
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
