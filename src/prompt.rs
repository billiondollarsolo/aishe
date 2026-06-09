//! Custom reedline prompt: `{cwd} {glyph} ` with exit-code-colored glyph and an
//! optional right prompt of `model · mode`.

use std::borrow::Cow;
use std::path::PathBuf;

use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};

pub struct LlmshPrompt {
    cwd_display: String,
    glyph: char,
    last_exit: i32,
    right: String,
    show_right: bool,
}

impl LlmshPrompt {
    pub fn new(cwd: PathBuf, mode: &str, last_exit: i32, model: String, show_right: bool) -> Self {
        let glyph = match mode {
            "yolo" => '⚡',
            "auto" => '»',
            _ => '❯',
        };
        let cwd_display = abbreviate_home(&cwd);
        let right = format!("{model} · {mode}");
        Self {
            cwd_display,
            glyph,
            last_exit,
            right,
            show_right,
        }
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

impl Prompt for LlmshPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.cwd_display.clone())
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        if self.show_right {
            Cow::Owned(self.right.clone())
        } else {
            Cow::Borrowed("")
        }
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Owned(format!(" {} ", self.glyph))
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
        reedline::Color::Cyan
    }

    fn get_indicator_color(&self) -> reedline::Color {
        if self.last_exit == 0 {
            reedline::Color::Green
        } else {
            reedline::Color::Red
        }
    }

    fn get_prompt_right_color(&self) -> reedline::Color {
        reedline::Color::DarkGrey
    }
}
