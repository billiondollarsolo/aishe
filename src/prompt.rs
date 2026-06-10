//! Custom reedline prompt: `{cwd} {glyph} ` with exit-code-colored glyph and an
//! optional right prompt of `model · mode`.

use std::borrow::Cow;
use std::path::{Path, PathBuf};

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
    ) -> Self {
        let glyph = match mode {
            "yolo" => '⚡',
            "auto" => '»',
            _ => '❯',
        };
        let cwd_display = abbreviate_home(&cwd);
        let right = match git {
            Some(branch) => format!("⎇ {branch}  {model} · {mode}"),
            None => format!("{model} · {mode}"),
        };
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
    // Walk up to find a `.git` directory or file.
    let mut dir = Some(cwd);
    let git_dir = loop {
        let d = dir?;
        let candidate = d.join(".git");
        if candidate.is_dir() {
            break candidate;
        }
        if candidate.is_file() {
            // Worktree/submodule: `.git` is `gitdir: <path>`.
            let text = std::fs::read_to_string(&candidate).ok()?;
            let p = text.strip_prefix("gitdir:")?.trim();
            let pb = PathBuf::from(p);
            break if pb.is_absolute() { pb } else { d.join(pb) };
        }
        dir = d.parent();
    };

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
    fn git_segment_none_outside_repo() {
        let dir = std::env::temp_dir().join(format!("aishe-nogit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(git_segment(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
