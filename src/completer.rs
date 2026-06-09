//! Tab completion for the reedline front-end.
//!
//! Two kinds, chosen by where the cursor sits:
//! - **command position** (first word of the line, or after `|`/`&&`/`;`/`(`):
//!   complete command names from the [`CommandCache`] — `$PATH` executables,
//!   zsh builtins, and the user's aliases/functions.
//! - **argument position** (anything else): complete file and directory paths
//!   relative to the current word, expanding a leading `~/`.
//!
//! Tokenization is whitespace-based and quoting-naive, matching the rest of
//! llmsh's v0.1 parsing (see `dispatcher`). It's an editor convenience, not a
//! parser: the actual command still runs through `zsh -c`.

use std::path::PathBuf;

use reedline::{Completer, Span, Suggestion};

use crate::dispatcher::CommandCache;

/// Upper bound on suggestions returned for a single Tab, so an empty prefix at
/// the command position doesn't allocate the entire `$PATH`.
const MAX_SUGGESTIONS: usize = 500;

pub struct LlmshCompleter {
    cache: CommandCache,
}

impl LlmshCompleter {
    pub fn new(cache: CommandCache) -> Self {
        Self { cache }
    }
}

impl Completer for LlmshCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let pos = pos.min(line.len());
        let start = word_start(line, pos);
        let word = &line[start..pos];
        let span = Span::new(start, pos);

        // A bare word at the command position completes command names; anything
        // that looks like a path (or sits in argument position) completes paths.
        if is_command_position(line, start) && !looks_like_path(word) {
            complete_commands(&self.cache, word, span)
        } else {
            complete_paths(word, span)
        }
    }
}

/// Byte index where the word under the cursor begins (after the last
/// space/tab). Whitespace is ASCII, so this lands on a char boundary.
fn word_start(line: &str, pos: usize) -> usize {
    let bytes = line.as_bytes();
    let mut i = pos;
    while i > 0 {
        match bytes[i - 1] {
            b' ' | b'\t' => break,
            _ => i -= 1,
        }
    }
    i
}

/// True if the word being completed is the command head: nothing precedes it on
/// the line, or it follows a pipeline/separator/subshell opener.
fn is_command_position(line: &str, word_start: usize) -> bool {
    let prefix = line[..word_start].trim_end();
    prefix.is_empty()
        || prefix.ends_with('|')
        || prefix.ends_with('&')
        || prefix.ends_with(';')
        || prefix.ends_with('(')
}

/// A word that should be path-completed even at the command position
/// (`./script`, `/usr/bin/x`, `~/bin/tool`, `bin/tool`).
fn looks_like_path(word: &str) -> bool {
    word.starts_with('.') || word.starts_with('/') || word.starts_with('~') || word.contains('/')
}

fn complete_commands(cache: &CommandCache, word: &str, span: Span) -> Vec<Suggestion> {
    let mut names = cache.matching(word);
    names.sort();
    names.dedup();
    names.truncate(MAX_SUGGESTIONS);
    names
        .into_iter()
        .map(|name| Suggestion {
            value: name,
            span,
            append_whitespace: true,
            ..Default::default()
        })
        .collect()
}

fn complete_paths(word: &str, span: Span) -> Vec<Suggestion> {
    // Split the word into a directory part (kept verbatim in the buffer, so a
    // leading `~/` stays literal for zsh to expand) and a file-name prefix.
    let (dir_part, file_prefix) = match word.rfind('/') {
        Some(idx) => (&word[..=idx], &word[idx + 1..]),
        None => ("", word),
    };

    let read_path: PathBuf = if dir_part.is_empty() {
        PathBuf::from(".")
    } else if let Some(rest) = dir_part.strip_prefix("~/") {
        home().join(rest)
    } else {
        PathBuf::from(dir_part)
    };

    let entries = match std::fs::read_dir(&read_path) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let include_hidden = file_prefix.starts_with('.');
    let mut out: Vec<Suggestion> = Vec::new();
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if !name.starts_with(file_prefix) {
            continue;
        }
        if name.starts_with('.') && !include_hidden {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let value = if is_dir {
            format!("{dir_part}{name}/")
        } else {
            format!("{dir_part}{name}")
        };
        out.push(Suggestion {
            value,
            span,
            // Don't append a space after a directory, so the user can keep
            // descending; do after a file.
            append_whitespace: !is_dir,
            ..Default::default()
        });
        if out.len() >= MAX_SUGGESTIONS {
            break;
        }
    }
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundary() {
        assert_eq!(word_start("git st", 6), 4);
        assert_eq!(word_start("git ", 4), 4); // empty word after a space
        assert_eq!(word_start("git", 3), 0);
        assert_eq!(word_start("", 0), 0);
    }

    #[test]
    fn command_position_detection() {
        assert!(is_command_position("gi", 0));
        assert!(is_command_position("  gi", 2));
        assert!(is_command_position("ls | gr", 5)); // after a pipe
        assert!(is_command_position("a && b", 5));
        assert!(!is_command_position("git st", 4)); // argument position
    }

    #[test]
    fn path_detection() {
        assert!(looks_like_path("./run"));
        assert!(looks_like_path("/usr/bin"));
        assert!(looks_like_path("~/bin"));
        assert!(looks_like_path("src/ma"));
        assert!(!looks_like_path("git"));
    }

    #[test]
    fn completes_command_names() {
        let cache = CommandCache::new();
        cache.insert_all(&["git", "grep", "gzip", "ls"]);
        let mut c = LlmshCompleter::new(cache);
        let sugg = c.complete("g", 1);
        let values: Vec<_> = sugg.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(values, vec!["git", "grep", "gzip"]);
        assert!(sugg.iter().all(|s| s.span == Span::new(0, 1)));
        assert!(sugg.iter().all(|s| s.append_whitespace));
    }

    #[test]
    fn completes_paths_in_argument_position() {
        let dir = std::env::temp_dir().join(format!("llmsh-comp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("alpha.txt"), b"x").unwrap();
        std::fs::write(dir.join("alpine.txt"), b"x").unwrap();

        let mut c = LlmshCompleter::new(CommandCache::new());
        let prefix = format!("cat {}/al", dir.display());
        let pos = prefix.len();
        let sugg = c.complete(&prefix, pos);
        let values: Vec<_> = sugg.iter().map(|s| s.value.clone()).collect();
        assert!(values.iter().any(|v| v.ends_with("alpha.txt")));
        assert!(values.iter().any(|v| v.ends_with("alpine.txt")));
        // Files get a trailing space; none here is a directory.
        assert!(sugg.iter().all(|s| s.append_whitespace));

        // Directory entries get a trailing slash and no appended space.
        let prefix2 = format!("cat {}/sub", dir.display());
        let sugg2 = c.complete(&prefix2, prefix2.len());
        assert_eq!(sugg2.len(), 1);
        assert!(sugg2[0].value.ends_with("subdir/"));
        assert!(!sugg2[0].append_whitespace);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dotfiles_hidden_unless_prefixed() {
        let dir = std::env::temp_dir().join(format!("llmsh-comp-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".secret"), b"x").unwrap();
        std::fs::write(dir.join("visible"), b"x").unwrap();

        let mut c = LlmshCompleter::new(CommandCache::new());
        let base = format!("cat {}/", dir.display());
        let all = c.complete(&base, base.len());
        assert!(all.iter().all(|s| !s.value.ends_with(".secret")));

        let dotted = format!("cat {}/.", dir.display());
        let hidden = c.complete(&dotted, dotted.len());
        assert!(hidden.iter().any(|s| s.value.ends_with(".secret")));

        std::fs::remove_dir_all(&dir).ok();
    }
}
