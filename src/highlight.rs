//! A syntax highlighter in the spirit of zsh-syntax-highlighting. It tokenizes
//! the line and colors the command head (known/unknown), flags, quoted strings,
//! operators, paths, env assignments, and the `?`/`!` sigils, using the active
//! theme.

use nu_ansi_term::Style;
use reedline::{Highlighter, StyledText};

use crate::dispatcher::CommandCache;
use crate::theme::Theme;

pub struct CmdHighlighter {
    cache: CommandCache,
    theme: Theme,
}

impl CmdHighlighter {
    pub fn new(cache: CommandCache, theme: Theme) -> Self {
        Self { cache, theme }
    }
}

impl Highlighter for CmdHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut out = StyledText::new();
        if line.is_empty() {
            return out;
        }

        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;

        // Leading forced-mode sigil.
        match chars[0] {
            '?' => {
                push(&mut out, self.theme.sigil_nl.to_nu_style(), "?");
                // The rest is natural language: render plainly.
                push(
                    &mut out,
                    Style::new(),
                    &chars[1..].iter().collect::<String>(),
                );
                return out;
            }
            '!' => {
                push(&mut out, self.theme.sigil_shell.to_nu_style(), "!");
                i = 1; // continue highlighting the rest as a shell line
            }
            _ => {}
        }

        // `expect_command` is true at the start of the line and after each
        // command separator, so we know which token is a command head.
        let mut expect_command = true;

        while i < chars.len() {
            let c = chars[i];

            // Whitespace runs.
            if c.is_whitespace() {
                let start = i;
                while i < chars.len() && chars[i].is_whitespace() {
                    i += 1;
                }
                push(&mut out, Style::new(), &collect(&chars, start, i));
                continue;
            }

            // Quoted strings.
            if c == '\'' || c == '"' {
                let quote = c;
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    i += 1;
                }
                if i < chars.len() {
                    i += 1; // closing quote
                }
                push(
                    &mut out,
                    self.theme.string.to_nu_style(),
                    &collect(&chars, start, i),
                );
                continue;
            }

            // Operators (two-char first).
            if let Some(len) = operator_len(&chars, i) {
                let start = i;
                i += len;
                push(
                    &mut out,
                    self.theme.operator.to_nu_style(),
                    &collect(&chars, start, i),
                );
                expect_command = true;
                continue;
            }

            // A bare word: read until whitespace / operator / quote.
            let start = i;
            while i < chars.len()
                && !chars[i].is_whitespace()
                && operator_len(&chars, i).is_none()
                && chars[i] != '\''
                && chars[i] != '"'
            {
                i += 1;
            }
            let word = collect(&chars, start, i);
            let style = self.classify_word(&word, expect_command);
            push(&mut out, style, &word);

            // A leading `VAR=value` keeps us in command position; anything else
            // consumes the command head.
            if !(expect_command && is_assignment(&word)) {
                expect_command = false;
            }
        }

        out
    }
}

impl CmdHighlighter {
    fn classify_word(&self, word: &str, expect_command: bool) -> Style {
        if expect_command {
            if is_assignment(word) {
                return self.theme.assignment.to_nu_style();
            }
            if is_path_like(word) {
                return self.theme.path.to_nu_style();
            }
            if self.cache.contains(word) {
                return self.theme.known_cmd.to_nu_style();
            }
            return self.theme.unknown_cmd.to_nu_style();
        }
        if word.starts_with('-') {
            return self.theme.flag.to_nu_style();
        }
        if is_path_like(word) {
            return self.theme.path.to_nu_style();
        }
        Style::new()
    }
}

fn push(out: &mut StyledText, style: Style, text: &str) {
    if !text.is_empty() {
        out.push((style, text.to_string()));
    }
}

fn collect(chars: &[char], start: usize, end: usize) -> String {
    chars[start..end].iter().collect()
}

/// Length of a shell operator starting at `i`, or None.
fn operator_len(chars: &[char], i: usize) -> Option<usize> {
    let c = chars[i];
    let next = chars.get(i + 1).copied();
    match (c, next) {
        ('&', Some('&')) | ('|', Some('|')) | ('>', Some('>')) => Some(2),
        ('|', _) | (';', _) | ('>', _) | ('<', _) | ('&', _) => Some(1),
        _ => None,
    }
}

fn is_assignment(word: &str) -> bool {
    if let Some((name, _)) = word.split_once('=') {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !name.chars().next().unwrap().is_ascii_digit()
    } else {
        false
    }
}

fn is_path_like(word: &str) -> bool {
    word.starts_with("./")
        || word.starts_with("../")
        || word.starts_with('/')
        || word.starts_with("~/")
        || (word.contains('/') && !word.starts_with('-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with(items: &[&str]) -> CommandCache {
        let cache = CommandCache::new();
        cache.insert_all(items);
        cache
    }

    /// Reconstruct the plain text from the styled spans — highlighting must be
    /// lossless.
    fn rendered(h: &CmdHighlighter, line: &str) -> String {
        h.highlight(line, line.len())
            .buffer
            .into_iter()
            .map(|(_, s)| s)
            .collect()
    }

    #[test]
    fn highlighting_is_lossless() {
        let h = CmdHighlighter::new(cache_with(&["git", "ls"]), Theme::default());
        for line in [
            "git status",
            "ls -la | grep foo",
            "FOO=bar ./run.sh --flag 'a string'",
            "echo \"hi\" && cd /tmp",
            "?what is up",
            "!rm -rf build",
        ] {
            assert_eq!(rendered(&h, line), line, "lossless failed for: {line}");
        }
    }

    #[test]
    fn produces_multiple_spans() {
        let h = CmdHighlighter::new(cache_with(&["git"]), Theme::default());
        let st = h.highlight("git -v | wc", 0);
        // command, space, flag, space, operator, space, command → several spans
        assert!(st.buffer.len() >= 5, "got {} spans", st.buffer.len());
    }

    #[test]
    fn assignment_detection() {
        assert!(is_assignment("FOO=bar"));
        assert!(!is_assignment("--flag=x"));
        assert!(!is_assignment("1A=x"));
    }

    #[test]
    fn path_detection() {
        assert!(is_path_like("./run.sh"));
        assert!(is_path_like("/usr/bin"));
        assert!(is_path_like("~/notes"));
        assert!(is_path_like("src/main.rs"));
        assert!(!is_path_like("plainword"));
        assert!(!is_path_like("-rf"));
    }
}
