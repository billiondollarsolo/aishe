//! Actual command outcomes, kept separately from model-written plans and prose.
//!
//! A check is explicit: ordinary commands never become checks because their
//! output resembles a test runner. Freshness covers recorded task effects;
//! it is not a claim that unrelated processes have left the workspace alone.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub(super) const MAX_EVIDENCE: usize = 128;
const COMMAND_BYTE_LIMIT: usize = 8 * 1024;
const OUTPUT_BYTE_LIMIT: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionKind {
    Check,
    Activity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOutcome {
    Running,
    Passed,
    Failed,
    Cancelled,
    NotRun,
    Uncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CheckEvidence {
    pub call_id: String,
    pub kind: ExecutionKind,
    pub command: String,
    pub cwd: PathBuf,
    pub started_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub outcome: ExecutionOutcome,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub workspace_revision: u64,
}

impl CheckEvidence {
    pub(super) fn started(
        call_id: &str,
        command: &str,
        cwd: &Path,
        kind: ExecutionKind,
        workspace_revision: u64,
    ) -> Self {
        Self {
            // Provider call identifiers are continuation metadata. Keep them
            // intact, as the rest of the durable tool journal does.
            call_id: call_id.into(),
            kind,
            command: bounded_redacted(command, COMMAND_BYTE_LIMIT),
            cwd: PathBuf::from(bounded_redacted(&cwd.to_string_lossy(), COMMAND_BYTE_LIMIT)),
            started_at_ms: super::now_ms(),
            finished_at_ms: None,
            duration_ms: None,
            exit_code: None,
            outcome: ExecutionOutcome::Running,
            output: String::new(),
            workspace_revision,
        }
    }

    pub fn stale(&self, workspace_revision: u64) -> bool {
        self.kind == ExecutionKind::Check
            && matches!(
                self.outcome,
                ExecutionOutcome::Passed | ExecutionOutcome::Failed
            )
            && self.workspace_revision != workspace_revision
    }

    pub(super) fn finish(
        &mut self,
        exit_code: i32,
        output: &str,
        duration_ms: u64,
        cancelled: bool,
    ) {
        self.finished_at_ms = Some(super::now_ms());
        self.duration_ms = Some(duration_ms);
        self.exit_code = Some(exit_code);
        self.outcome = if cancelled || exit_code == 130 {
            ExecutionOutcome::Cancelled
        } else if exit_code == 0 {
            ExecutionOutcome::Passed
        } else {
            ExecutionOutcome::Failed
        };
        self.output = bounded_redacted(output, OUTPUT_BYTE_LIMIT);
    }

    pub(super) fn not_run(&mut self, reason: &str) {
        self.finished_at_ms = Some(super::now_ms());
        self.duration_ms = Some(0);
        self.outcome = ExecutionOutcome::NotRun;
        self.output = bounded_redacted(reason, OUTPUT_BYTE_LIMIT);
    }

    pub(super) fn interrupted(&mut self, cancelled: bool) {
        if self.outcome != ExecutionOutcome::Running {
            return;
        }
        self.finished_at_ms = Some(super::now_ms());
        // No exit status was observed. Even cancellation cannot manufacture an
        // exit code for a process whose result never reached the checkpoint.
        self.outcome = if cancelled {
            ExecutionOutcome::Cancelled
        } else {
            ExecutionOutcome::Uncertain
        };
        self.output = if cancelled {
            "Cancelled before the command result was recorded; effects may have occurred."
        } else {
            "Interrupted before the command result was recorded; effects may have occurred."
        }
        .into();
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CheckSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub not_run: usize,
    pub uncertain: usize,
    pub stale: usize,
    pub running: usize,
    pub omitted: usize,
    pub unresolved: Vec<String>,
}

impl CheckSummary {
    /// A concise description of observed checks, never task acceptance.
    pub fn label(&self) -> String {
        if self.total == 0 {
            return "No recorded checks".into();
        }
        let mut parts = Vec::new();
        for (count, label) in [
            (self.passed, "passed"),
            (self.failed, "failed"),
            (self.cancelled, "cancelled"),
            (self.not_run, "not run"),
            (self.uncertain, "uncertain"),
            (self.stale, "stale"),
            (self.running, "running"),
        ] {
            if count > 0 {
                parts.push(format!("{count} {label}"));
            }
        }
        format!("Recorded checks: {}", parts.join(" · "))
    }
}

pub(super) fn summary(record: &super::Record) -> CheckSummary {
    let mut summary = CheckSummary {
        omitted: record.evidence_dropped,
        ..CheckSummary::default()
    };
    for check in record
        .evidence
        .iter()
        .filter(|entry| entry.kind == ExecutionKind::Check)
    {
        summary.total += 1;
        if check.stale(record.workspace_revision) {
            summary.stale += 1;
            continue;
        }
        match check.outcome {
            ExecutionOutcome::Passed => summary.passed += 1,
            ExecutionOutcome::Failed => summary.failed += 1,
            ExecutionOutcome::Cancelled => summary.cancelled += 1,
            ExecutionOutcome::NotRun => summary.not_run += 1,
            ExecutionOutcome::Uncertain => summary.uncertain += 1,
            ExecutionOutcome::Running => summary.running += 1,
        }
    }
    if summary.total == 0 {
        summary
            .unresolved
            .push("No explicit checks have been recorded.".into());
    }
    for (count, issue) in [
        (summary.failed, "Recorded checks failed."),
        (summary.cancelled, "Checks were cancelled before passing."),
        (summary.not_run, "Requested checks were not run."),
        (
            summary.uncertain,
            "Check results were not recorded before interruption.",
        ),
        (
            summary.stale,
            "Checks predate a later command or file changes.",
        ),
        (summary.running, "Checks are still running."),
        (
            summary.omitted,
            "Older execution records were omitted to keep the journal bounded.",
        ),
    ] {
        if count > 0 {
            summary.unresolved.push(issue.into());
        }
    }
    if let Some(error) = &record.last_error {
        let error = bounded_redacted(error, 1024);
        if !error.is_empty() && !summary.unresolved.contains(&error) {
            summary.unresolved.push(error);
        }
    }
    summary
}

/// Redact the complete input before cutting it. Truncating first could expose
/// the beginning of a credential whose full shape is needed by the redactor.
fn bounded_redacted(value: &str, byte_limit: usize) -> String {
    let mut redacted = crate::redact::redact(value);
    if redacted.len() <= byte_limit {
        return redacted;
    }
    const MARKER: &str = "\n[truncated]";
    let mut end = byte_limit.saturating_sub(MARKER.len());
    while !redacted.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    redacted.truncate(end);
    redacted.push_str(MARKER);
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_observed_exit_codes_and_never_treats_timeouts_as_passing() {
        for (code, cancelled, outcome) in [
            (0, false, ExecutionOutcome::Passed),
            (1, false, ExecutionOutcome::Failed),
            (124, false, ExecutionOutcome::Failed),
            (130, false, ExecutionOutcome::Cancelled),
            (0, true, ExecutionOutcome::Cancelled),
        ] {
            let mut check =
                CheckEvidence::started("call", "test", Path::new("/tmp"), ExecutionKind::Check, 0);
            check.finish(code, "actual output", 42, cancelled);
            assert_eq!(check.outcome, outcome);
            assert_eq!(check.exit_code, Some(code));
            assert_eq!(check.duration_ms, Some(42));
            assert!(check.finished_at_ms.is_some());
        }
    }

    #[test]
    fn redacts_before_truncation_and_preserves_unicode_boundaries() {
        let secret = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789";
        let input = format!("{}{}", "é".repeat(OUTPUT_BYTE_LIMIT / 2 - 2), secret);
        let redacted = bounded_redacted(&input, OUTPUT_BYTE_LIMIT);
        assert!(redacted.len() <= OUTPUT_BYTE_LIMIT);
        assert!(!redacted.contains("sk-proj"));
        assert!(!redacted.contains(secret));
        assert!(redacted.ends_with("[truncated]"));
    }

    #[test]
    fn crash_recovery_leaves_unknown_result_and_no_invented_exit() {
        let mut check =
            CheckEvidence::started("call", "test", Path::new("/tmp"), ExecutionKind::Check, 0);
        check.interrupted(false);
        assert_eq!(check.outcome, ExecutionOutcome::Uncertain);
        assert_eq!(check.exit_code, None);
        assert_eq!(check.duration_ms, None);
        check.interrupted(true);
        assert_eq!(check.outcome, ExecutionOutcome::Uncertain);
    }
}
