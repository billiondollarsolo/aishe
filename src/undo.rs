//! Reversible file edits.
//!
//! Every built-in file-tool write (`write_file` / `edit_file`) records the file's
//! *pre-image* to an append-only journal, so `aishe undo` can restore it. All
//! changes made within one aishe process share a **batch** id, so a single
//! `aishe undo` reverts a whole yolo run as a unit (in reverse order), not just
//! the last file touched.
//!
//! The journal is JSONL at `$XDG_DATA_HOME/aishe/undo.jsonl` (override with
//! `AISHE_UNDO_JOURNAL`). Journaling is **best-effort**: if it can't be written,
//! the file write itself still proceeds — undo is a safety net, never a gate.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// How many context lines a diff shows around the changed region.
const DIFF_CONTEXT: usize = 2;
/// Cap on `-`/`+` lines shown per side before truncating the diff.
const DIFF_MAX_LINES: usize = 80;

/// One line in the journal: a recorded change, or a marker that a batch has been
/// reverted (so a second `aishe undo` moves on to the prior batch).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Record {
    Change {
        batch: String,
        ts: u64,
        /// Absolute path of the changed file.
        path: String,
        /// Whether the file existed before the change.
        existed: bool,
        /// Prior contents (`None` when the file was newly created).
        before: Option<String>,
        tool: String,
        summary: String,
    },
    Reverted {
        batch: String,
        ts: u64,
    },
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Process-global batch id: every file change in one aishe invocation shares it,
/// so `aishe undo` reverts the run as a unit.
fn batch_id() -> &'static str {
    static BATCH: OnceLock<String> = OnceLock::new();
    BATCH.get_or_init(|| format!("{}-{}", now(), std::process::id()))
}

#[cfg(test)]
thread_local! {
    /// Per-thread journal override for tests, so parallel unit tests never touch
    /// the real journal or race over a process-global env var.
    static TEST_JOURNAL: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_test_journal(p: Option<PathBuf>) {
    TEST_JOURNAL.with(|t| *t.borrow_mut() = p);
}

/// The journal path: a test override, else `$AISHE_UNDO_JOURNAL`, else
/// `$XDG_DATA_HOME/aishe/undo.jsonl`.
pub fn journal_path() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(p) = TEST_JOURNAL.with(|t| t.borrow().clone()) {
        return Some(p);
    }
    if let Ok(p) = std::env::var("AISHE_UNDO_JOURNAL") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    dirs::data_dir().map(|d| d.join("aishe").join("undo.jsonl"))
}

/// Record a file change made by a built-in tool. `before` is the prior contents
/// (`None` when the file was newly created, i.e. `existed == false`). Best-effort:
/// a journaling error is swallowed so it can never fail the actual write.
pub fn record(resolved: &Path, existed: bool, before: Option<String>, tool: &str, summary: &str) {
    if let Some(j) = journal_path() {
        let _ = record_to(&j, batch_id(), resolved, existed, before, tool, summary);
    }
}

#[allow(clippy::too_many_arguments)]
fn record_to(
    journal: &Path,
    batch: &str,
    resolved: &Path,
    existed: bool,
    before: Option<String>,
    tool: &str,
    summary: &str,
) -> Result<()> {
    append(
        journal,
        &Record::Change {
            batch: batch.to_string(),
            ts: now(),
            path: resolved.display().to_string(),
            existed,
            before,
            tool: tool.to_string(),
            summary: summary.to_string(),
        },
    )
}

fn append(journal: &Path, rec: &Record) -> Result<()> {
    if let Some(parent) = journal.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let line = serde_json::to_string(rec).context("serializing undo record")?;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal)
        .with_context(|| format!("opening undo journal {}", journal.display()))?;
    writeln!(f, "{line}").context("writing undo record")?;
    Ok(())
}

fn read_records(journal: &Path) -> Vec<Record> {
    let Ok(text) = std::fs::read_to_string(journal) else {
        return Vec::new();
    };
    // Tolerate the odd malformed line rather than failing the whole undo.
    text.lines()
        .filter_map(|l| serde_json::from_str::<Record>(l).ok())
        .collect()
}

/// A recorded batch of changes (one aishe run), for `aishe undo --list`.
pub struct Batch {
    pub id: String,
    pub ts: u64,
    /// Unique file paths touched, in first-seen order.
    pub files: Vec<String>,
    pub reverted: bool,
    /// The first change's summary, as a human label.
    pub summary: String,
}

/// List recorded batches in chronological order (most recent last).
pub fn list() -> Vec<Batch> {
    match journal_path() {
        Some(j) => list_in(&j),
        None => Vec::new(),
    }
}

fn reverted_set(records: &[Record]) -> HashSet<String> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::Reverted { batch, .. } => Some(batch.clone()),
            _ => None,
        })
        .collect()
}

fn list_in(journal: &Path) -> Vec<Batch> {
    let records = read_records(journal);
    let reverted = reverted_set(&records);
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, Batch> = HashMap::new();
    for r in &records {
        if let Record::Change {
            batch,
            ts,
            path,
            summary,
            ..
        } = r
        {
            let b = map.entry(batch.clone()).or_insert_with(|| {
                order.push(batch.clone());
                Batch {
                    id: batch.clone(),
                    ts: *ts,
                    files: Vec::new(),
                    reverted: reverted.contains(batch),
                    summary: summary.clone(),
                }
            });
            if !b.files.contains(path) {
                b.files.push(path.clone());
            }
        }
    }
    order.into_iter().filter_map(|id| map.remove(&id)).collect()
}

/// The result of an undo: the batch reverted, the files restored, and any errors.
pub struct Undone {
    pub batch: String,
    pub restored: Vec<String>,
    pub errors: Vec<String>,
}

/// Revert the most recent not-yet-reverted batch. Returns `Ok(None)` when there is
/// nothing left to undo.
pub fn undo_last() -> Result<Option<Undone>> {
    match journal_path() {
        Some(j) => undo_last_in(&j),
        None => Ok(None),
    }
}

fn undo_last_in(journal: &Path) -> Result<Option<Undone>> {
    let records = read_records(journal);
    let reverted = reverted_set(&records);

    // Batches in first-seen order; pick the most recent one not already reverted.
    let mut order: Vec<String> = Vec::new();
    for r in &records {
        if let Record::Change { batch, .. } = r {
            if !order.contains(batch) {
                order.push(batch.clone());
            }
        }
    }
    let Some(target) = order.into_iter().rev().find(|b| !reverted.contains(b)) else {
        return Ok(None);
    };

    // Restore the batch's changes in reverse order, so a file that was created and
    // then edited within the batch ends up removed (back to its original absence).
    let changes: Vec<(&str, bool, &Option<String>)> = records
        .iter()
        .filter_map(|r| match r {
            Record::Change {
                batch,
                path,
                existed,
                before,
                ..
            } if *batch == target => Some((path.as_str(), *existed, before)),
            _ => None,
        })
        .collect();

    let mut restored: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for (path, existed, before) in changes.into_iter().rev() {
        let p = Path::new(path);
        let outcome = if existed {
            match before {
                Some(content) => std::fs::write(p, content),
                None => {
                    errors.push(format!("{path}: no stored contents to restore"));
                    continue;
                }
            }
        } else {
            // The tool created this file; undo removes it (already-gone is fine).
            match std::fs::remove_file(p) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        };
        match outcome {
            Ok(()) => {
                if !restored.contains(&path.to_string()) {
                    restored.push(path.to_string());
                }
            }
            Err(e) => errors.push(format!("{path}: {e}")),
        }
    }

    // Mark the batch reverted so the next undo moves to the prior batch.
    let _ = append(
        journal,
        &Record::Reverted {
            batch: target.clone(),
            ts: now(),
        },
    );

    Ok(Some(Undone {
        batch: target,
        restored,
        errors,
    }))
}

/// A compact unified-style diff of two text blobs: trims the common head/tail and
/// shows the differing middle with a little surrounding context. Returns an empty
/// string when the contents are identical. Lines are prefixed `- ` / `+ ` / `  `
/// (the caller adds color).
pub fn unified_diff(before: &str, after: &str) -> String {
    if before == after {
        return String::new();
    }
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();

    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len().saturating_sub(pre)
        && suf < b.len().saturating_sub(pre)
        && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let a_mid = &a[pre..a.len() - suf];
    let b_mid = &b[pre..b.len() - suf];

    let mut out: Vec<String> = Vec::new();
    // Leading context.
    for line in &a[pre.saturating_sub(DIFF_CONTEXT)..pre] {
        out.push(format!("  {line}"));
    }
    push_capped(&mut out, a_mid, '-');
    push_capped(&mut out, b_mid, '+');
    // Trailing context.
    let tail_start = a.len() - suf;
    let tail_end = (tail_start + DIFF_CONTEXT).min(a.len());
    for line in &a[tail_start..tail_end] {
        out.push(format!("  {line}"));
    }
    out.join("\n")
}

fn push_capped(out: &mut Vec<String>, lines: &[&str], sign: char) {
    if lines.len() > DIFF_MAX_LINES {
        for line in &lines[..DIFF_MAX_LINES] {
            out.push(format!("{sign} {line}"));
        }
        out.push(format!(
            "{sign} … ({} more line(s))",
            lines.len() - DIFF_MAX_LINES
        ));
    } else {
        for line in lines {
            out.push(format!("{sign} {line}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "aishe-undo-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn diff_localized_edit() {
        let d = unified_diff("a\nb\nc\n", "a\nB\nc\n");
        assert!(d.contains("- b"), "{d}");
        assert!(d.contains("+ B"), "{d}");
        assert!(d.contains("  a"), "{d}"); // context retained
    }

    #[test]
    fn diff_identical_is_empty() {
        assert_eq!(unified_diff("x\ny", "x\ny"), "");
    }

    #[test]
    fn modify_then_undo_restores_prior_contents() {
        let dir = tmpdir("mod");
        let journal = dir.join("undo.jsonl");
        let file = dir.join("f.txt");
        std::fs::write(&file, "v1").unwrap();
        std::fs::write(&file, "v2").unwrap(); // tool modifies it
        record_to(
            &journal,
            "b1",
            &file,
            true,
            Some("v1".into()),
            "edit_file",
            "edit f.txt",
        )
        .unwrap();

        let undone = undo_last_in(&journal).unwrap().unwrap();
        assert_eq!(undone.restored, vec![file.display().to_string()]);
        assert!(undone.errors.is_empty());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "v1");
        // A second undo has nothing left.
        assert!(undo_last_in(&journal).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_then_undo_removes_file() {
        let dir = tmpdir("create");
        let journal = dir.join("undo.jsonl");
        let file = dir.join("new.txt");
        std::fs::write(&file, "fresh").unwrap(); // tool created it
        record_to(
            &journal,
            "b1",
            &file,
            false,
            None,
            "write_file",
            "write new.txt",
        )
        .unwrap();

        undo_last_in(&journal).unwrap().unwrap();
        assert!(!file.exists(), "created file should be removed on undo");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn batches_revert_most_recent_first() {
        let dir = tmpdir("batches");
        let journal = dir.join("undo.jsonl");
        let f1 = dir.join("a.txt");
        let f2 = dir.join("b.txt");
        std::fs::write(&f1, "A1").unwrap();
        std::fs::write(&f2, "B1").unwrap();
        std::fs::write(&f1, "A2").unwrap();
        std::fs::write(&f2, "B2").unwrap();
        record_to(
            &journal,
            "b1",
            &f1,
            true,
            Some("A1".into()),
            "edit_file",
            "edit a.txt",
        )
        .unwrap();
        record_to(
            &journal,
            "b2",
            &f2,
            true,
            Some("B1".into()),
            "edit_file",
            "edit b.txt",
        )
        .unwrap();

        // list shows both, newest last, both active.
        let batches = list_in(&journal);
        assert_eq!(
            batches.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["b1", "b2"]
        );
        assert!(batches.iter().all(|b| !b.reverted));

        // First undo reverts b2 (most recent).
        let u = undo_last_in(&journal).unwrap().unwrap();
        assert_eq!(u.batch, "b2");
        assert_eq!(std::fs::read_to_string(&f2).unwrap(), "B1");
        assert_eq!(std::fs::read_to_string(&f1).unwrap(), "A2"); // b1 untouched
                                                                 // Now b2 is marked reverted.
        assert!(
            list_in(&journal)
                .iter()
                .find(|b| b.id == "b2")
                .unwrap()
                .reverted
        );

        // Second undo reverts b1.
        let u2 = undo_last_in(&journal).unwrap().unwrap();
        assert_eq!(u2.batch, "b1");
        assert_eq!(std::fs::read_to_string(&f1).unwrap(), "A1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nothing_to_undo_on_empty_journal() {
        let dir = tmpdir("empty");
        let journal = dir.join("undo.jsonl");
        assert!(undo_last_in(&journal).unwrap().is_none());
        assert!(list_in(&journal).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
