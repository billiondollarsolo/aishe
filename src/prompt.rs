//! Custom reedline prompt: `{cwd} {glyph} ` with exit-code-colored glyph and an
//! optional right prompt of `model · mode`.

use std::borrow::Cow;
use std::path::PathBuf;

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
    pub fn new(
        cwd: PathBuf,
        mode: &str,
        last_exit: i32,
        model: String,
        show_right: bool,
        theme: Theme,
        prompt_format: Option<&str>,
    ) -> Self {
        let glyph = match mode {
            "yolo" => '⚡',
            "auto" => '»',
            _ => '❯',
        };
        let cwd_display = abbreviate_home(&cwd);
        let right = format!("{model} · {mode}");
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
