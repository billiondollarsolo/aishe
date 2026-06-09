//! A lightweight syntax highlighter in the spirit of zsh-syntax-highlighting:
//! the command head is colored green when it is a known command (in the cache),
//! red when unknown, with shell sigils (`?`, `!`) tinted.

use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

use crate::dispatcher::CommandCache;

pub struct CmdHighlighter {
    cache: CommandCache,
}

impl CmdHighlighter {
    pub fn new(cache: CommandCache) -> Self {
        Self { cache }
    }
}

impl Highlighter for CmdHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();

        if line.is_empty() {
            return styled;
        }

        // Forced-NL / forced-shell sigils.
        if let Some(rest) = line.strip_prefix('?') {
            styled.push((Style::new().fg(Color::Magenta), "?".to_string()));
            styled.push((Style::new(), rest.to_string()));
            return styled;
        }
        if let Some(rest) = line.strip_prefix('!') {
            styled.push((Style::new().fg(Color::Yellow), "!".to_string()));
            styled.push((Style::new(), rest.to_string()));
            return styled;
        }

        // Split into leading whitespace, first token, and the rest.
        let trimmed_start = line.len() - line.trim_start().len();
        let (leading, remainder) = line.split_at(trimmed_start);
        let head_len = remainder
            .find(char::is_whitespace)
            .unwrap_or(remainder.len());
        let head = &remainder[..head_len];
        let tail = &remainder[head_len..];

        if !leading.is_empty() {
            styled.push((Style::new(), leading.to_string()));
        }

        let head_style = if head.is_empty() {
            Style::new()
        } else if is_known(head, &self.cache) {
            Style::new().fg(Color::Green)
        } else {
            Style::new().fg(Color::Red)
        };
        styled.push((head_style, head.to_string()));

        if !tail.is_empty() {
            styled.push((Style::new(), tail.to_string()));
        }

        styled
    }
}

/// A head is "known" if it's cached, looks like a path, or is an env assignment.
fn is_known(head: &str, cache: &CommandCache) -> bool {
    head.starts_with("./")
        || head.starts_with('/')
        || head.starts_with("~/")
        || head.contains('=')
        || cache.contains(head)
}
