//! Tab completion for the reedline front-end.
//!
//! Context-aware, chosen by where the cursor sits and what the segment's command
//! is:
//! - **`$VAR` / `${VAR`** anywhere — complete environment variable names.
//! - **command position** (segment start, i.e. line start or after
//!   `|`/`&&`/`;`/`(`): complete command names from the [`CommandCache`].
//! - **`cd` / `pushd` / `rmdir` arguments** — complete directories only.
//! - **`aishe` arguments** — complete meta subcommands and their fixed values.
//! - **other arguments** — complete file and directory paths.
//!
//! Tokenization is whitespace-based and quoting-naive, matching the rest of
//! aishe's parsing. It's an editor convenience: the command still runs via the
//! shell.

use std::path::PathBuf;

use reedline::{Completer, Span, Suggestion};

use crate::dispatcher::CommandCache;

/// Upper bound on suggestions returned for a single Tab.
const MAX_SUGGESTIONS: usize = 500;

/// `aishe` meta subcommands (for `aishe <Tab>`).
const META_SUBCOMMANDS: &[&str] = &[
    "mode", "model", "provider", "editor", "frontend", "stream", "theme", "config", "rehash",
    "help",
];

pub struct AisheCompleter {
    cache: CommandCache,
}

impl AisheCompleter {
    pub fn new(cache: CommandCache) -> Self {
        Self { cache }
    }
}

impl Completer for AisheCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let pos = pos.min(line.len());
        let start = word_start(line, pos);
        let word = &line[start..pos];
        let span = Span::new(start, pos);

        // Environment variables (`$VAR`, `${VAR`) take precedence anywhere.
        if word.starts_with('$') {
            return complete_env(word, span);
        }

        // The current command segment (after the last operator) and its tokens.
        let seg_tokens: Vec<&str> = segment_str(line, start).split_whitespace().collect();

        // Command position: nothing typed yet in this segment.
        if seg_tokens.is_empty() {
            return if looks_like_path(word) {
                complete_paths(word, span, false)
            } else {
                complete_commands(&self.cache, word, span)
            };
        }

        match seg_tokens[0] {
            "aishe" => complete_aishe_meta(&seg_tokens, word, span),
            "cd" | "pushd" | "rmdir" => complete_paths(word, span, true),
            _ => complete_paths(word, span, false),
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

/// The current command segment: the text before the cursor, after the last
/// pipeline/separator/subshell operator (quoting-naive).
fn segment_str(line: &str, start: usize) -> &str {
    let prefix = &line[..start];
    let mut cut = 0;
    for (i, ch) in prefix.char_indices() {
        if matches!(ch, '|' | ';' | '&' | '(') {
            cut = i + ch.len_utf8();
        }
    }
    &prefix[cut..]
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

/// Complete environment variable names for a `$VAR` / `${VAR` word.
fn complete_env(word: &str, span: Span) -> Vec<Suggestion> {
    let (brace, prefix) = match word.strip_prefix("${") {
        Some(p) => (true, p),
        None => (false, &word[1..]), // word starts with '$'
    };
    // Only an identifier prefix — bail on `$(`, `$@`, etc.
    if !prefix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return Vec::new();
    }
    let mut names: Vec<String> = std::env::vars()
        .map(|(k, _)| k)
        .filter(|k| k.starts_with(prefix))
        .collect();
    names.sort();
    names.truncate(MAX_SUGGESTIONS);
    names
        .into_iter()
        .map(|k| Suggestion {
            value: if brace {
                format!("${{{k}}}")
            } else {
                format!("${k}")
            },
            span,
            append_whitespace: !brace,
            ..Default::default()
        })
        .collect()
}

/// Complete `aishe` meta subcommands and their fixed-value arguments.
fn complete_aishe_meta(seg_tokens: &[&str], word: &str, span: Span) -> Vec<Suggestion> {
    match seg_tokens.len() {
        // `aishe <word>` — the subcommand.
        1 => str_suggestions(META_SUBCOMMANDS, word, span),
        // `aishe <sub> <word>` — fixed values for subcommands that have them.
        2 => {
            let values: &[&str] = match seg_tokens[1] {
                "mode" => &["suggest", "auto", "yolo"],
                "provider" => &["anthropic", "openai"],
                "editor" => &["emacs", "vi"],
                "frontend" => &["auto", "reedline", "zsh-pty"],
                "stream" => &["on", "off"],
                "theme" => crate::theme::PRESETS,
                _ => return Vec::new(), // model/config/etc.: free-form
            };
            str_suggestions(values, word, span)
        }
        _ => Vec::new(),
    }
}

/// Build suggestions from a fixed option list, filtered by `word` prefix.
fn str_suggestions(options: &[&str], word: &str, span: Span) -> Vec<Suggestion> {
    options
        .iter()
        .filter(|o| o.starts_with(word))
        .map(|o| Suggestion {
            value: (*o).to_string(),
            span,
            append_whitespace: true,
            ..Default::default()
        })
        .collect()
}

/// Complete file/directory paths for the current word. With `dirs_only`, only
/// directories are offered (for `cd`/`pushd`/`rmdir`).
fn complete_paths(word: &str, span: Span, dirs_only: bool) -> Vec<Suggestion> {
    // Split the word into a directory part (kept verbatim in the buffer, so a
    // leading `~/` stays literal for the shell to expand) and a file-name prefix.
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
        if dirs_only && !is_dir {
            continue;
        }
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

    fn values(s: &[Suggestion]) -> Vec<String> {
        s.iter().map(|x| x.value.clone()).collect()
    }

    #[test]
    fn word_boundary() {
        assert_eq!(word_start("git st", 6), 4);
        assert_eq!(word_start("git ", 4), 4);
        assert_eq!(word_start("git", 3), 0);
        assert_eq!(word_start("", 0), 0);
    }

    #[test]
    fn segment_after_operators() {
        assert_eq!(segment_str("ls | gr", 5).trim(), "");
        assert_eq!(segment_str("a && b ", 7).trim(), "b");
        assert_eq!(segment_str("git st", 4).trim(), "git");
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
        let mut c = AisheCompleter::new(cache);
        assert_eq!(values(&c.complete("g", 1)), vec!["git", "grep", "gzip"]);
        // command position after a pipe, too.
        let sugg = c.complete("ls | g", 6);
        assert_eq!(values(&sugg), vec!["git", "grep", "gzip"]);
    }

    #[test]
    fn completes_env_vars() {
        std::env::set_var("AISHE_COMPL_TEST_XYZ", "1");
        let mut c = AisheCompleter::new(CommandCache::new());
        let sugg = c.complete("echo $AISHE_COMPL_TEST", 22);
        assert!(values(&sugg).contains(&"$AISHE_COMPL_TEST_XYZ".to_string()));
        // brace form
        let line = "echo ${AISHE_COMPL_TEST";
        let sugg = c.complete(line, line.len());
        assert!(values(&sugg).contains(&"${AISHE_COMPL_TEST_XYZ}".to_string()));
        std::env::remove_var("AISHE_COMPL_TEST_XYZ");
    }

    #[test]
    fn aishe_meta_subcommands_and_values() {
        let mut c = AisheCompleter::new(CommandCache::new());
        // subcommands
        let sugg = c.complete("aishe mo", 8);
        let v = values(&sugg);
        assert!(v.contains(&"mode".to_string()) && v.contains(&"model".to_string()));
        // fixed values for `mode`
        let line = "aishe mode ";
        assert_eq!(
            values(&c.complete(line, line.len())),
            vec!["suggest", "auto", "yolo"]
        );
        // theme presets come from the theme module
        let line = "aishe theme ";
        assert!(values(&c.complete(line, line.len())).contains(&"nord".to_string()));
    }

    #[test]
    fn cd_completes_directories_only() {
        let dir = std::env::temp_dir().join(format!("aishe-comp-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("subdir")).unwrap();
        std::fs::write(dir.join("afile.txt"), b"x").unwrap();
        let mut c = AisheCompleter::new(CommandCache::new());

        let line = format!("cd {}/", dir.display());
        let v = values(&c.complete(&line, line.len()));
        assert!(v.iter().any(|x| x.ends_with("subdir/")));
        assert!(!v.iter().any(|x| x.ends_with("afile.txt")));

        // a plain command still completes files
        let line = format!("cat {}/", dir.display());
        let v = values(&c.complete(&line, line.len()));
        assert!(v.iter().any(|x| x.ends_with("afile.txt")));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dotfiles_hidden_unless_prefixed() {
        let dir = std::env::temp_dir().join(format!("aishe-comp-hidden-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".secret"), b"x").unwrap();
        std::fs::write(dir.join("visible"), b"x").unwrap();
        let mut c = AisheCompleter::new(CommandCache::new());

        let base = format!("cat {}/", dir.display());
        assert!(!values(&c.complete(&base, base.len()))
            .iter()
            .any(|x| x.ends_with(".secret")));
        let dotted = format!("cat {}/.", dir.display());
        assert!(values(&c.complete(&dotted, dotted.len()))
            .iter()
            .any(|x| x.ends_with(".secret")));

        std::fs::remove_dir_all(&dir).ok();
    }
}
