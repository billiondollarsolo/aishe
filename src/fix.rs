//! Error-context capture for the fix-the-last-command key (proposal N1).
//!
//! When the fix key is pressed after a command fails, aishe can re-run the failed
//! command to capture its actual error output and feed that into the correction
//! prompt — so the model fixes the *real* error ("unknown option", "no such
//! file", "not a git repository"), not just a guess from the command text.
//!
//! Re-running is gated hard: only commands the sandbox classifier deems read-only
//! *and* the safety gate deems safe are re-run, so a destructive or network
//! command is never executed a second time. It is also opt-in
//! (`fix_capture_stderr`) and bounded by a timeout. The diagnostic run uses a
//! fresh executor with no history log, so it doesn't pollute history.

use std::time::Duration;

use crate::executor::Executor;
use crate::safety::{assess, Risk};
use crate::sandbox::is_write_command;

/// How long a diagnostic re-run may take before it's abandoned.
const RERUN_TIMEOUT: Duration = Duration::from_secs(8);
/// Trailing lines of captured output kept for the prompt.
const MAX_CTX_LINES: usize = 20;
/// Hard char cap on the captured context (keeps the prompt small).
const MAX_CTX_CHARS: usize = 2000;

/// Safe to re-run for diagnosis: a read-only command (no writes/network per the
/// sandbox classifier) that the safety gate also deems safe. Conservative —
/// anything not clearly read-only is excluded, so a destructive or network
/// command is never re-run.
pub fn safe_to_rerun(cmd: &str) -> bool {
    !is_write_command(cmd) && matches!(assess(cmd), Risk::Safe)
}

/// Re-run a failed command (when `enabled` and it's [`safe_to_rerun`]) to capture
/// its error output for the fix prompt. Bounded by a timeout; returns the
/// trailing lines, or `None` when disabled, unsafe to re-run, or the re-run
/// produced no output. The diagnostic run uses a fresh executor with no history
/// log, so it never pollutes the recorded history.
pub fn error_context(cmd: &str, enabled: bool) -> Option<String> {
    if !enabled || !safe_to_rerun(cmd) {
        return None;
    }
    let mut diag = Executor::new().ok()?;
    let (_, out) = diag.run_captured(cmd, RERUN_TIMEOUT, false);
    let tail = tail(&out);
    (!tail.trim().is_empty()).then_some(tail)
}

/// The fix prompt: the failed command, its exit status, and (when available) its
/// captured error output, asking for a corrected command.
pub fn build_prompt(cmd: &str, exit: &str, ctx: Option<&str>) -> String {
    let mut p =
        format!("The previous shell command failed with exit status {exit}. Command: {cmd}.");
    if let Some(c) = ctx {
        if !c.trim().is_empty() {
            p.push_str("\nIts error output was:\n");
            p.push_str(c.trim_end());
            p.push('\n');
        }
    }
    p.push_str(" Reply with a corrected shell command.");
    p
}

/// Last [`MAX_CTX_LINES`] lines of `s`, capped to [`MAX_CTX_CHARS`] chars (keeping
/// the most recent chars, which carry the actual error).
fn tail(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let start = lines.len().saturating_sub(MAX_CTX_LINES);
    let joined = lines[start..].join("\n");
    let chars: Vec<char> = joined.chars().collect();
    if chars.len() > MAX_CTX_CHARS {
        chars[chars.len() - MAX_CTX_CHARS..].iter().collect()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_to_rerun_only_read_only_safe_commands() {
        // Read-only and safe → re-runnable.
        assert!(safe_to_rerun("ls /etc"));
        assert!(safe_to_rerun("git status"));
        assert!(safe_to_rerun("grep -r foo ."));
        // Writes / state-changing / network → never re-run.
        assert!(!safe_to_rerun("npm install"));
        assert!(!safe_to_rerun("git push"));
        assert!(!safe_to_rerun("rm foo"));
        assert!(!safe_to_rerun("curl http://example.com"));
        // Dangerous → never re-run even though the head looks read-only.
        assert!(!safe_to_rerun("cat /etc/passwd > /dev/sda"));
    }

    #[test]
    fn build_prompt_includes_context_when_present() {
        let without = build_prompt("ls /x", "2", None);
        assert!(without.contains("exit status 2"));
        assert!(without.contains("Command: ls /x"));
        assert!(!without.contains("error output"));
        let with = build_prompt("ls /x", "2", Some("ls: cannot access '/x': No such file"));
        assert!(with.contains("Its error output was:"));
        assert!(with.contains("No such file"));
    }

    #[test]
    fn error_context_disabled_or_unsafe_returns_none() {
        // Disabled.
        assert!(error_context("ls /nonexistent", false).is_none());
        // Unsafe to re-run: must NOT execute, returns None.
        let marker = std::env::temp_dir().join(format!("aishe-fix-norerun-{}", std::process::id()));
        std::fs::remove_file(&marker).ok();
        let cmd = format!("touch {}", marker.display());
        assert!(error_context(&cmd, true).is_none());
        assert!(
            !marker.exists(),
            "a write command must never be re-run for diagnosis"
        );
    }

    #[test]
    fn error_context_captures_a_safe_failures_output() {
        let path = std::env::temp_dir().join(format!("aishe-fix-missing-{}", std::process::id()));
        std::fs::remove_file(&path).ok();
        let cmd = format!("ls {}", path.display());
        let ctx = error_context(&cmd, true).expect("safe failing command yields context");
        assert!(
            ctx.contains("No such file") || ctx.to_lowercase().contains("cannot access"),
            "expected the ls error in the context, got: {ctx:?}"
        );
    }

    #[test]
    fn tail_caps_lines_and_chars() {
        let many = (0..100)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let t = tail(&many);
        assert!(t.lines().count() <= MAX_CTX_LINES);
        assert!(t.contains("line99"), "keeps the most recent lines");
        assert!(!t.contains("line0\n"), "drops the oldest lines");
    }
}
