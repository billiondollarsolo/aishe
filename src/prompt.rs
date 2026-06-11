//! Custom reedline prompt: `{cwd} {glyph} ` with exit-code-colored glyph and an
//! optional right prompt of `model · mode`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reedline::{
    Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, PromptViMode,
};

use crate::theme::Theme;

pub struct AishePrompt {
    glyph: char,
    last_exit: i32,
    right: String,
    show_right: bool,
    theme: Theme,
    /// Rendered left prompt: either the cwd or a custom format applied.
    left: String,
}

impl AishePrompt {
    // A prompt legitimately has many independent display inputs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cwd: PathBuf,
        mode: &str,
        last_exit: i32,
        model: String,
        show_right: bool,
        theme: Theme,
        prompt_format: Option<&str>,
        git: Option<String>,
        duration: Option<String>,
    ) -> Self {
        let glyph = match mode {
            "yolo" => '⚡',
            "auto" => '»',
            _ => '❯',
        };
        let cwd_display = abbreviate_home(&cwd);
        // Right prompt: git segment, command duration, then "model · mode".
        let mut parts: Vec<String> = Vec::new();
        if let Some(g) = git {
            parts.push(g);
        }
        if let Some(d) = duration {
            parts.push(d);
        }
        parts.push(format!("{model} · {mode}"));
        let right = parts.join("  ");
        let left = match prompt_format {
            Some(fmt) => apply_format(fmt, &cwd_display, mode, &model, last_exit),
            None => cwd_display,
        };
        Self {
            glyph,
            last_exit,
            right,
            show_right,
            theme,
            left,
        }
    }
}

/// Git status flags for the prompt segment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct GitStatus {
    staged: bool,
    dirty: bool,
    ahead: u32,
    behind: u32,
}

/// Count stash entries by counting lines in `<git_dir>/logs/refs/stash` (cheap,
/// no `git` process). `0` when there are none.
fn stash_count(git_dir: &Path) -> usize {
    std::fs::read_to_string(git_dir.join("logs/refs/stash"))
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Run `git status` directly (no internal timeout). Used on a background thread by
/// [`VcsCache`], where blocking is fine because the prompt does not wait on it.
fn git_status_run(cwd: &Path) -> Option<GitStatus> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args([
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_git_status(&String::from_utf8_lossy(&out.stdout)))
}

/// Background-refreshed git status/stash counts, so the prompt never blocks on a
/// slow repo. The branch is read cheaply from `.git/HEAD` on every prompt; the
/// `git status` and stash counts are computed on a background thread and cached.
/// Status markers therefore lag by at most one prompt (zsh `vcs_info` async
/// style). Held by the REPL and cloned into the worker.
#[derive(Clone, Default)]
pub struct VcsCache {
    inner: Arc<Mutex<VcsState>>,
}

#[derive(Default)]
struct VcsState {
    /// The directory the cached status belongs to (cleared on `cd`).
    cwd: Option<PathBuf>,
    status: Option<GitStatus>,
    stashes: usize,
    /// A refresh is running; don't spawn another.
    inflight: bool,
}

impl VcsCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The git segment for `cwd`: the branch computed now, plus status markers
    /// from the cache (and a background refresh kicked off for next time). Returns
    /// `None` outside a repo. When `show_status` is false, just the branch.
    pub fn segment(&self, cwd: &Path, show_status: bool) -> Option<String> {
        let git_dir = find_git_dir(cwd)?;
        let branch = branch_from_head(&git_dir)?;
        let mut out = format!("⎇ {branch}");
        if !show_status {
            return Some(out);
        }

        let (status, stashes) = {
            let mut st = self.inner.lock().ok()?;
            if st.cwd.as_deref() != Some(cwd) {
                // Directory changed: drop stale markers until the refresh lands.
                st.cwd = Some(cwd.to_path_buf());
                st.status = None;
                st.stashes = 0;
            }
            if !st.inflight {
                st.inflight = true;
                let inner = Arc::clone(&self.inner);
                let dir = cwd.to_path_buf();
                let gdir = git_dir.clone();
                std::thread::spawn(move || {
                    let status = git_status_run(&dir);
                    let stashes = stash_count(&gdir);
                    if let Ok(mut st) = inner.lock() {
                        // Only store if we're still in the same directory.
                        if st.cwd.as_deref() == Some(dir.as_path()) {
                            st.status = status;
                            st.stashes = stashes;
                        }
                        st.inflight = false;
                    }
                });
            }
            (st.status.clone(), st.stashes)
        };

        if let Some(st) = status {
            if st.staged {
                out.push('+');
            }
            if st.dirty {
                out.push('*');
            }
            if st.ahead > 0 {
                out.push_str(&format!("⇡{}", st.ahead));
            }
            if st.behind > 0 {
                out.push_str(&format!("⇣{}", st.behind));
            }
        }
        if stashes > 0 {
            out.push_str(&format!("⚑{stashes}"));
        }
        Some(out)
    }
}

/// Parse `git status --porcelain=v2 --branch` output. Changed/renamed entries
/// carry a two-char `XY` field: `X` is the staged status, `Y` the worktree one
/// (`.` means unchanged).
fn parse_git_status(text: &str) -> GitStatus {
    let mut st = GitStatus::default();
    for line in text.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for tok in ab.split_whitespace() {
                if let Some(n) = tok.strip_prefix('+') {
                    st.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = tok.strip_prefix('-') {
                    st.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let xy: Vec<char> = line.chars().skip(2).take(2).collect();
            if matches!(xy.first(), Some(c) if *c != '.') {
                st.staged = true;
            }
            if matches!(xy.get(1), Some(c) if *c != '.') {
                st.dirty = true;
            }
        } else if line.starts_with("u ") {
            // Unmerged paths count as a dirty tree.
            st.dirty = true;
        }
    }
    st
}

/// Format a command duration compactly: `850ms`, `3.2s`, `1m05s`, `1h02m`.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let secs = d.as_secs();
    if secs < 60 {
        let tenths = (ms % 1000) / 100;
        return format!("{secs}.{tenths}s");
    }
    if secs < 3600 {
        return format!("{}m{:02}s", secs / 60, secs % 60);
    }
    format!("{}h{:02}m", secs / 3600, (secs % 3600) / 60)
}

/// Substitute `{cwd}` / `{mode}` / `{model}` / `{exit}` placeholders in a custom
/// prompt format string.
fn apply_format(fmt: &str, cwd: &str, mode: &str, model: &str, last_exit: i32) -> String {
    fmt.replace("{cwd}", cwd)
        .replace("{mode}", mode)
        .replace("{model}", model)
        .replace("{exit}", &last_exit.to_string())
}

/// The current git branch (or short detached SHA) for `cwd`, read directly from
/// `.git/HEAD` — no `git` process, so it's cheap enough to compute per prompt.
/// Returns `None` when not inside a work tree.
pub fn git_segment(cwd: &Path) -> Option<String> {
    branch_from_head(&find_git_dir(cwd)?)
}

/// Walk up from `cwd` to the repository's git directory (handling a `.git` file
/// for worktrees/submodules). `None` when not inside a work tree.
fn find_git_dir(cwd: &Path) -> Option<PathBuf> {
    let mut dir = Some(cwd);
    loop {
        let d = dir?;
        let candidate = d.join(".git");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if candidate.is_file() {
            // Worktree/submodule: `.git` is `gitdir: <path>`.
            let text = std::fs::read_to_string(&candidate).ok()?;
            let p = text.strip_prefix("gitdir:")?.trim();
            let pb = PathBuf::from(p);
            return Some(if pb.is_absolute() { pb } else { d.join(pb) });
        }
        dir = d.parent();
    }
}

/// The branch name (or short detached SHA) from `<git_dir>/HEAD`.
fn branch_from_head(git_dir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(rf) = head.strip_prefix("ref: refs/heads/") {
        Some(rf.to_string())
    } else if head.len() >= 7 {
        // Detached HEAD: show a short SHA.
        Some(head[..7].to_string())
    } else {
        None
    }
}

/// Replace a leading $HOME with `~`.
fn abbreviate_home(cwd: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = cwd.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    cwd.display().to_string()
}

impl Prompt for AishePrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        if self.show_right {
            Cow::Owned(self.right.clone())
        } else {
            Cow::Borrowed("")
        }
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        // In vi mode, tag the indicator so the active sub-mode is visible.
        match edit_mode {
            PromptEditMode::Vi(PromptViMode::Normal) => Cow::Owned(format!(" [N]{} ", self.glyph)),
            PromptEditMode::Vi(PromptViMode::Insert) => Cow::Owned(format!(" [I]{} ", self.glyph)),
            _ => Cow::Owned(format!(" {} ", self.glyph)),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("· ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }

    fn get_prompt_color(&self) -> reedline::Color {
        self.theme.cwd.to_crossterm()
    }

    fn get_indicator_color(&self) -> reedline::Color {
        if self.last_exit == 0 {
            self.theme.glyph_ok.to_crossterm()
        } else {
            self.theme.glyph_err.to_crossterm()
        }
    }

    fn get_prompt_right_color(&self) -> reedline::Color {
        self.theme.right_prompt.to_crossterm()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcs_cache_returns_branch_without_blocking() {
        let dir = std::env::temp_dir().join(format!("aishe-vcs-{}", std::process::id()));
        let gitdir = dir.join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let vcs = VcsCache::new();
        // Branch only.
        assert_eq!(vcs.segment(&dir, false).as_deref(), Some("⎇ main"));
        // With status: the branch is returned immediately; markers come from the
        // background refresh, which fails on this fake repo, so just the branch.
        let started = std::time::Instant::now();
        assert_eq!(vcs.segment(&dir, true).as_deref(), Some("⎇ main"));
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "segment must not block on git status"
        );
        // Outside a repo: None.
        let outside = std::env::temp_dir().join(format!("aishe-vcs-none-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        assert!(vcs.segment(&outside, true).is_none());

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&outside).ok();
    }

    #[test]
    fn git_segment_reads_branch_from_head() {
        let dir = std::env::temp_dir().join(format!("aishe-git-{}", std::process::id()));
        let gitdir = dir.join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feature/x\n").unwrap();
        assert_eq!(git_segment(&dir).as_deref(), Some("feature/x"));

        // detached HEAD → short sha
        std::fs::write(gitdir.join("HEAD"), "0123456789abcdef\n").unwrap();
        assert_eq!(git_segment(&dir).as_deref(), Some("0123456"));

        // a subdirectory still finds the repo by walking up
        let sub = dir.join("a/b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(git_segment(&sub).as_deref(), Some("0123456"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_duration_scales() {
        assert_eq!(format_duration(Duration::from_millis(850)), "850ms");
        assert_eq!(format_duration(Duration::from_millis(3200)), "3.2s");
        assert_eq!(format_duration(Duration::from_secs(65)), "1m05s");
        assert_eq!(format_duration(Duration::from_secs(3720)), "1h02m");
    }

    #[test]
    fn parse_git_status_reads_dirty_staged_and_ab() {
        // unstaged change (Y=M) → dirty, not staged.
        let out = "# branch.oid abc\n# branch.head main\n# branch.ab +2 -1\n1 .M N... file\n";
        let st = parse_git_status(out);
        assert!(st.dirty);
        assert!(!st.staged);
        assert_eq!(st.ahead, 2);
        assert_eq!(st.behind, 1);

        // staged change (X=M) → staged, not dirty.
        let staged = "# branch.head main\n1 M. N... file\n";
        let st = parse_git_status(staged);
        assert!(st.staged);
        assert!(!st.dirty);

        let clean = "# branch.head main\n# branch.ab +0 -0\n";
        let st = parse_git_status(clean);
        assert!(!st.dirty);
        assert!(!st.staged);
        assert_eq!(st.ahead, 0);
        assert_eq!(st.behind, 0);
    }

    #[test]
    fn stash_count_counts_reflog_lines() {
        let git_dir = std::env::temp_dir().join(format!("aishe-stash-{}", std::process::id()));
        std::fs::create_dir_all(git_dir.join("logs/refs")).unwrap();
        assert_eq!(stash_count(&git_dir), 0); // no stash file yet
        std::fs::write(
            git_dir.join("logs/refs/stash"),
            "0000 1111 t <t> 0 +0 WIP one\n0000 2222 t <t> 0 +0 WIP two\n",
        )
        .unwrap();
        assert_eq!(stash_count(&git_dir), 2);
        std::fs::remove_dir_all(&git_dir).ok();
    }

    #[test]
    fn git_segment_none_outside_repo() {
        let dir = std::env::temp_dir().join(format!("aishe-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_segment(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
