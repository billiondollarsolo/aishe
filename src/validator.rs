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
            _ => {
                // For a function definition, also require a complete `{ … }`
                // body so multi-line definitions keep going until closed.
                let is_func = dispatcher::function_def_name(line).is_some();
                if shell_incomplete(line, is_func) {
                    ValidationResult::Incomplete
                } else {
                    ValidationResult::Complete
                }
            }
        }
    }
}

/// True if a shell line is unterminated: an open single/double quote, a trailing
/// unescaped backslash (line continuation), or unbalanced parentheses. When
/// `func` is set (a function definition), also require a `{ … }` body that has
/// been opened and closed.
///
/// Shell-aware enough for interactive editing: single quotes are literal (no
/// escapes inside them), backslash escapes the next char outside single quotes,
/// and brackets inside quotes are ignored. Brace balancing is applied *only* to
/// function definitions, so ordinary `${VAR}` / `{a,b}` lines aren't affected.
fn shell_incomplete(line: &str, func: bool) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    let mut paren: i32 = 0;
    let mut brace: i32 = 0;
    let mut saw_brace = false;

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
            '{' if !in_double => {
                brace += 1;
                saw_brace = true;
            }
            '}' if !in_double => brace = (brace - 1).max(0),
            _ => {}
        }
    }

    let base = in_single || in_double || escaped || paren > 0;
    // A function definition isn't done until its body has opened and closed.
    base || (func && (brace > 0 || !saw_brace))
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
        assert!(!shell_incomplete("echo hello", false));
        assert!(!shell_incomplete("echo 'hello world'", false));
        assert!(!shell_incomplete(r#"echo "a \" b""#, false));
        assert!(!shell_incomplete("echo $(date)", false));
        assert!(!shell_incomplete(r"echo a\\", false)); // escaped backslash
        assert!(!shell_incomplete("echo \"line1\nline2\"", false)); // newline in quotes
                                                                    // Braces only matter for function defs — ordinary brace use is complete.
        assert!(!shell_incomplete("echo ${VAR} {a,b}", false));
    }

    #[test]
    fn incomplete_shell_lines() {
        assert!(shell_incomplete("echo 'hello", false));
        assert!(shell_incomplete("echo \"hello", false));
        assert!(shell_incomplete(r"echo hello \", false)); // trailing continuation
        assert!(shell_incomplete("echo $(date", false));
    }

    #[test]
    fn function_definitions_need_a_closed_body() {
        // `func` flag: needs an opened+closed brace block.
        assert!(shell_incomplete("greet() {", true)); // open brace
        assert!(shell_incomplete("greet()", true)); // no body yet
        assert!(shell_incomplete("greet() {\n  echo hi", true)); // still open
        assert!(!shell_incomplete("greet() { echo hi; }", true)); // closed
        assert!(!shell_incomplete("greet() {\n  echo hi\n}", true)); // closed multi-line
    }

    #[test]
    fn function_def_continues_then_completes() {
        assert!(is_incomplete("greet() {"));
        assert!(!is_incomplete("greet() { echo hi; }"));
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
