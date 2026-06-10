//! Custom reedline prompt: `{cwd} {glyph} ` with exit-code-colored glyph and an
//! optional right prompt of `model · mode`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
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
#[derive(Debug, Default, PartialEq, Eq)]
struct GitStatus {
    staged: bool,
    dirty: bool,
    ahead: u32,
    behind: u32,
}

/// The full git prompt segment: branch (from `.git/HEAD`, cheap) plus, when
/// `show_status` is set, `+` (staged), `*` (unstaged changes), `⇡N`/`⇣N`
/// ahead/behind the upstream (one short, time-limited `git status` call), and
/// `⚑N` for stashes. `None` outside a repo.
pub fn git_segment_full(cwd: &Path, show_status: bool) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let branch = branch_from_head(&git_dir)?;
    let mut out = format!("⎇ {branch}");
    if show_status {
        if let Some(st) = git_status(cwd) {
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
        let stashes = stash_count(&git_dir);
        if stashes > 0 {
            out.push_str(&format!("⚑{stashes}"));
        }
    }
    Some(out)
}

/// Count stash entries by counting lines in `<git_dir>/logs/refs/stash` (cheap,
/// no `git` process). `0` when there are none.
fn stash_count(git_dir: &Path) -> usize {
    std::fs::read_to_string(git_dir.join("logs/refs/stash"))
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

/// Run `git status --porcelain=v2 --branch` with a short timeout and parse the
/// dirty flag and ahead/behind counts. Best-effort: `None` on error/timeout, so a
/// slow or huge repo never freezes the prompt.
fn git_status(cwd: &Path) -> Option<GitStatus> {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    let dir = cwd.to_path_buf();
    std::thread::spawn(move || {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&dir)
            .args([
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=no",
            ])
            .output();
        let _ = tx.send(out);
    });
    let out = match rx.recv_timeout(Duration::from_millis(250)) {
        Ok(Ok(o)) if o.status.success() => o.stdout,
        _ => return None,
    };
    let text = String::from_utf8_lossy(&out);
    Some(parse_git_status(&text))
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
