//! Reversible command preview (proposal R2 — the overlay/dry-run backend).
//!
//! `aishe dry-run "<cmd>"` runs a command against a *throwaway copy* of the
//! working tree, under bubblewrap with a read-only root and the network disabled,
//! so the command really executes but its writes are confined to the copy and it
//! has no external side effects. aishe then diffs the copy against the real tree
//! and shows exactly what would change — apply it with `--apply`, or discard it
//! (the default) and nothing was ever touched.
//!
//! This is the portable building block N2 (plan → preview → apply/undo) builds on.
//! Because there's no kernel overlay, the working tree is copied; a size cap keeps
//! that bounded. Out-of-tree paths are read-only (bubblewrap), so a command can't
//! escape the preview.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Cap on the working-tree copy: a dry-run of a huge tree (e.g. a multi-GB `.git`)
/// isn't practical, so we refuse rather than copy unboundedly.
const MAX_FILES: usize = 20_000;
const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// Whether the dry-run backend can run here (needs bubblewrap for the read-only
/// root + network isolation that make the preview safe).
pub fn available() -> bool {
    crate::sandbox::bwrap_available()
}

/// The bubblewrap argv (without the trailing shell) for a dry-run: a read-only
/// root with the *staging copy* bind-mounted at `cwd` (so cwd-relative writes land
/// in the copy), a writable `/tmp`, private `/dev` and `/proc`, **no network**
/// (`--unshare-net`), started in `cwd`, dying with the parent. Ends with `--`, so
/// the caller appends the shell + `-c <command>`.
pub fn dry_run_argv(cwd: &Path, staging: &Path) -> Vec<String> {
    let cwd = cwd.display().to_string();
    let staging = staging.display().to_string();
    [
        "bwrap",
        "--ro-bind",
        "/",
        "/", // read-only system …
        "--bind",
        "/tmp",
        "/tmp", // … writable /tmp …
        "--bind",
        &staging,
        &cwd, // … and the throwaway copy mounted where cwd is.
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--unshare-net", // no network during a preview
        "--chdir",
        &cwd,
        "--die-with-parent",
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Recursively copy `src` into `dst` (regular files and directories only;
/// symlinks and special files are skipped). Enforces the size cap, returning the
/// number of files copied or an error describing why the tree is too big.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<usize, String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("creating staging dir: {e}"))?;
    let mut files = 0usize;
    let mut bytes = 0u64;
    copy_into(src, dst, &mut files, &mut bytes)?;
    Ok(files)
}

fn copy_into(src: &Path, dst: &Path, files: &mut usize, bytes: &mut u64) -> Result<(), String> {
    let entries = std::fs::read_dir(src).map_err(|e| format!("reading {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_symlink() {
            continue; // skip symlinks: copying their target could escape the tree
        }
        if ft.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| format!("mkdir {}: {e}", to.display()))?;
            copy_into(&from, &to, files, bytes)?;
        } else if ft.is_file() {
            let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
            *files += 1;
            *bytes += len;
            if *files > MAX_FILES || *bytes > MAX_BYTES {
                return Err(format!(
                    "working tree too large to dry-run (> {} files or > {} MB) — \
                     try a smaller subdirectory",
                    MAX_FILES,
                    MAX_BYTES / (1024 * 1024)
                ));
            }
            std::fs::copy(&from, &to).map_err(|e| format!("copy {}: {e}", from.display()))?;
        }
    }
    Ok(())
}

/// What happened to a file in the dry-run, relative to the real tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
}

/// One file's change in the preview.
#[derive(Debug, Clone)]
pub struct Change {
    /// Path relative to the working tree.
    pub rel: String,
    pub kind: ChangeKind,
    /// A unified diff for text files (`None` for binary content).
    pub diff: Option<String>,
}

/// Compare the post-run `staging` copy against the `original` tree and return the
/// changes (sorted by path). Files present only in staging are Added, only in
/// original are Deleted, and differing content is Modified.
pub fn changes(original: &Path, staging: &Path) -> Vec<Change> {
    let orig_files = list_files(original);
    let stage_files = list_files(staging);
    let mut out = Vec::new();
    for rel in orig_files.union(&stage_files) {
        let in_orig = orig_files.contains(rel);
        let in_stage = stage_files.contains(rel);
        let rel_str = rel.display().to_string();
        match (in_orig, in_stage) {
            (true, false) => out.push(Change {
                rel: rel_str,
                kind: ChangeKind::Deleted,
                diff: text_diff(&original.join(rel), None),
            }),
            (false, true) => out.push(Change {
                rel: rel_str,
                kind: ChangeKind::Added,
                diff: text_diff(None, &staging.join(rel)),
            }),
            (true, true) => {
                let o = std::fs::read(original.join(rel)).unwrap_or_default();
                let s = std::fs::read(staging.join(rel)).unwrap_or_default();
                if o != s {
                    out.push(Change {
                        rel: rel_str,
                        kind: ChangeKind::Modified,
                        diff: diff_bytes(&o, &s),
                    });
                }
            }
            (false, false) => {}
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Apply the previewed changes and journal each file's pre-image first, so the
/// whole batch is reversible with `aishe undo`. Binary content can't be stored in
/// the text journal, so those changes apply but aren't undoable (mirrors the file
/// tools). `tool` labels the journal entries. Returns the paths it failed to apply.
pub fn apply_journaled(
    original: &Path,
    staging: &Path,
    changes: &[Change],
    tool: &str,
) -> Vec<String> {
    for c in changes {
        let dest = original.join(&c.rel);
        let summary = format!("dry-run apply: {}", c.rel);
        match c.kind {
            ChangeKind::Added => crate::undo::record(&dest, false, None, tool, &summary),
            ChangeKind::Modified | ChangeKind::Deleted => {
                if let Ok(before) = std::fs::read_to_string(&dest) {
                    crate::undo::record(&dest, true, Some(before), tool, &summary);
                }
            }
        }
    }
    apply(original, staging, changes)
}

/// Print a previewed change set: a header per file plus colorized unified diffs.
pub fn print_changes(changes: &[Change]) {
    use crossterm::style::Stylize;
    for c in changes {
        let (tag, painted) = match c.kind {
            ChangeKind::Added => ("added", c.rel.as_str().green().to_string()),
            ChangeKind::Modified => ("modified", c.rel.as_str().yellow().to_string()),
            ChangeKind::Deleted => ("deleted", c.rel.as_str().red().to_string()),
        };
        println!("  {} {painted}", format!("{tag:>8}").dim());
        if let Some(diff) = &c.diff {
            for line in diff.lines() {
                let colored = if line.starts_with('-') {
                    line.red().to_string()
                } else if line.starts_with('+') {
                    line.green().to_string()
                } else {
                    line.dim().to_string()
                };
                println!("    {colored}");
            }
        } else if c.kind != ChangeKind::Deleted {
            println!("    {}", "(binary)".dim());
        }
    }
}

/// Apply the previewed changes from `staging` onto the real `original` tree:
/// added/modified files are copied over, deleted files removed. Best-effort per
/// file; returns the list of paths it failed to apply.
pub fn apply(original: &Path, staging: &Path, changes: &[Change]) -> Vec<String> {
    let mut failed = Vec::new();
    for c in changes {
        let dest = original.join(&c.rel);
        let result = match c.kind {
            ChangeKind::Added | ChangeKind::Modified => (|| {
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(staging.join(&c.rel), &dest).map(|_| ())
            })(),
            ChangeKind::Deleted => std::fs::remove_file(&dest),
        };
        if result.is_err() {
            failed.push(c.rel.clone());
        }
    }
    failed
}

/// All regular files under `root`, as paths relative to `root` (symlinks skipped,
/// matching [`copy_tree`]).
fn list_files(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    walk(root, root, &mut out);
    out
}

fn walk(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            walk(root, &path, out);
        } else if ft.is_file() {
            if let Ok(rel) = path.strip_prefix(root) {
                out.insert(rel.to_path_buf());
            }
        }
    }
}

/// A unified diff for an add (`old=None`) or delete (`new=None`), reading the one
/// side that exists. `None` when that side is binary.
fn text_diff(old: impl AsForDiff, new: impl AsForDiff) -> Option<String> {
    let o = old.read_text();
    let n = new.read_text();
    match (o, n) {
        (Some(o), Some(n)) => Some(crate::undo::unified_diff(&o, &n)),
        _ => None,
    }
}

/// Unified diff between two byte buffers, or `None` if either side is non-UTF-8.
fn diff_bytes(old: &[u8], new: &[u8]) -> Option<String> {
    match (std::str::from_utf8(old), std::str::from_utf8(new)) {
        (Ok(o), Ok(n)) => Some(crate::undo::unified_diff(o, n)),
        _ => None,
    }
}

/// Helper so `text_diff` can take either a `&Path` (read it) or `None` (empty).
trait AsForDiff {
    fn read_text(self) -> Option<String>;
}
impl AsForDiff for &PathBuf {
    fn read_text(self) -> Option<String> {
        std::fs::read(self)
            .ok()
            .and_then(|b| String::from_utf8(b).ok())
    }
}
impl AsForDiff for Option<()> {
    fn read_text(self) -> Option<String> {
        Some(String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn changes_classifies_add_modify_delete() {
        let base = std::env::temp_dir().join(format!("aishe-ov-{}", std::process::id()));
        let orig = base.join("orig");
        let stage = base.join("stage");
        std::fs::create_dir_all(&orig).unwrap();
        write(&orig.join("keep.txt"), "same\n");
        write(&orig.join("edit.txt"), "before\n");
        write(&orig.join("gone.txt"), "delete me\n");
        // Staging starts as a copy, then a "command" edited it.
        copy_tree(&orig, &stage).unwrap();
        write(&stage.join("edit.txt"), "after\n"); // modified
        write(&stage.join("new.txt"), "created\n"); // added
        std::fs::remove_file(stage.join("gone.txt")).unwrap(); // deleted

        let ch = changes(&orig, &stage);
        let by = |name: &str| ch.iter().find(|c| c.rel == name).map(|c| c.kind.clone());
        assert_eq!(by("new.txt"), Some(ChangeKind::Added));
        assert_eq!(by("edit.txt"), Some(ChangeKind::Modified));
        assert_eq!(by("gone.txt"), Some(ChangeKind::Deleted));
        assert_eq!(by("keep.txt"), None, "unchanged file isn't a change");
        // The modified file carries a unified diff.
        let edit = ch.iter().find(|c| c.rel == "edit.txt").unwrap();
        let d = edit.diff.as_ref().unwrap();
        assert!(d.contains("before") && d.contains("after"), "{d}");
        assert!(d.contains('-') && d.contains('+'), "has +/- markers: {d}");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn apply_makes_the_changes_real() {
        let base = std::env::temp_dir().join(format!("aishe-ov-ap-{}", std::process::id()));
        let orig = base.join("orig");
        let stage = base.join("stage");
        std::fs::create_dir_all(&orig).unwrap();
        write(&orig.join("a.txt"), "old\n");
        write(&orig.join("del.txt"), "x\n");
        copy_tree(&orig, &stage).unwrap();
        write(&stage.join("a.txt"), "new\n");
        write(&stage.join("sub/b.txt"), "created\n");
        std::fs::remove_file(stage.join("del.txt")).unwrap();

        let ch = changes(&orig, &stage);
        let failed = apply(&orig, &stage, &ch);
        assert!(failed.is_empty(), "apply failures: {failed:?}");
        assert_eq!(
            std::fs::read_to_string(orig.join("a.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(
            std::fs::read_to_string(orig.join("sub/b.txt")).unwrap(),
            "created\n"
        );
        assert!(
            !orig.join("del.txt").exists(),
            "deleted file removed on apply"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn dry_run_argv_isolates_root_and_network() {
        let argv = dry_run_argv(Path::new("/work/proj"), Path::new("/tmp/stage"));
        assert_eq!(argv[0], "bwrap");
        assert!(argv.iter().any(|a| a == "--unshare-net"), "blocks network");
        // The staging copy is bound at the cwd path.
        let i = argv.iter().position(|a| a == "/tmp/stage").unwrap();
        assert_eq!(argv[i + 1], "/work/proj");
        assert_eq!(argv.last().unwrap(), "--");
    }

    #[test]
    fn copy_tree_skips_symlinks() {
        let base = std::env::temp_dir().join(format!("aishe-ov-sl-{}", std::process::id()));
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        write(&src.join("real.txt"), "hi\n");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/passwd", src.join("link")).unwrap();
        let dst = base.join("dst");
        copy_tree(&src, &dst).unwrap();
        assert!(dst.join("real.txt").exists());
        assert!(!dst.join("link").exists(), "symlinks are not copied");
        std::fs::remove_dir_all(&base).ok();
    }
}
