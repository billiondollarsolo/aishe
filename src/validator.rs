//! Multi-line continuation for the reedline front-end.
//!
//! When the user presses Enter on an unterminated *shell* line — an open quote,
//! a trailing line-continuation backslash, or an unbalanced `(` — reedline drops
//! to a continuation line instead of submitting (matching zsh's `quote>` /
//! `cmdsubst>` behavior).
//!
//! Crucially, natural-language input is **never** trapped: NL routinely contains
//! apostrophes (`what's eating my disk`), which a naive quote check would read as
//! an open quote. So we route the line through [`dispatcher::dispatch`] first and
//! only apply continuation logic to lines that are actually shell.

use reedline::{ValidationResult, Validator};

use crate::dispatcher::{self, CommandCache, Dispatch};

pub struct AisheValidator {
    cache: CommandCache,
}

impl AisheValidator {
    pub fn new(cache: CommandCache) -> Self {
        Self { cache }
    }
}

impl Validator for AisheValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        match dispatcher::dispatch(line, &self.cache) {
            // NL (and empty input) always submits — don't trap apostrophes.
            Dispatch::NaturalLanguage(_) => ValidationResult::Complete,
            _ if shell_incomplete(line) => ValidationResult::Incomplete,
            _ => ValidationResult::Complete,
        }
    }
}

/// True if a shell line is unterminated: an open single/double quote, a trailing
/// unescaped backslash (line continuation), or unbalanced parentheses.
///
/// Shell-aware enough for interactive editing: single quotes are literal (no
/// escapes inside them), backslash escapes the next char outside single quotes,
/// and parens inside quotes are ignored.
fn shell_incomplete(line: &str) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut paren: i32 = 0;

    for c in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if in_single {
            // Nothing is special inside single quotes except the closing quote.
            if c == '\'' {
                in_single = false;
            }
            continue;
        }
        match c {
            '\\' => escaped = true,
            '\'' if !in_double => in_single = true,
            '"' => in_double = !in_double,
            '(' if !in_double => paren += 1,
            ')' if !in_double => paren = (paren - 1).max(0),
            _ => {}
        }
    }

    in_single || in_double || escaped || paren > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache() -> CommandCache {
        let c = CommandCache::new();
        c.insert_all(&["echo", "git", "ls"]);
        c
    }

    fn is_incomplete(line: &str) -> bool {
        matches!(
            AisheValidator::new(cache()).validate(line),
            ValidationResult::Incomplete
        )
    }

    #[test]
    fn complete_shell_lines() {
        assert!(!shell_incomplete("echo hello"));
        assert!(!shell_incomplete("echo 'hello world'"));
        assert!(!shell_incomplete(r#"echo "a \" b""#));
        assert!(!shell_incomplete("echo $(date)"));
        assert!(!shell_incomplete(r"echo a\\")); // escaped backslash, not a continuation
        assert!(!shell_incomplete("echo \"line1\nline2\"")); // newline inside quotes, balanced
    }

    #[test]
    fn incomplete_shell_lines() {
        assert!(shell_incomplete("echo 'hello"));
        assert!(shell_incomplete("echo \"hello"));
        assert!(shell_incomplete(r"echo hello \")); // trailing continuation
        assert!(shell_incomplete("echo $(date"));
    }

    #[test]
    fn natural_language_never_incomplete() {
        // Apostrophes in NL must not trap the user in a continuation prompt.
        assert!(!is_incomplete("what's eating my disk"));
        assert!(!is_incomplete("?how do I count files"));
        assert!(!is_incomplete(""));
    }

    #[test]
    fn unterminated_shell_is_incomplete() {
        assert!(is_incomplete("echo 'oops"));
        assert!(is_incomplete("git commit -m \"wip"));
        // Forced-shell with a real open quote behaves like zsh (continuation).
        assert!(is_incomplete("!echo 'oops"));
    }
}
