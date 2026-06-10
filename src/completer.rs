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

/// `aishe` meta subcommands, with a short description shown in the menu.
const META_SUBCOMMANDS: &[(&str, &str)] = &[
    ("mode", "interaction mode: suggest|auto|yolo"),
    ("model", "show or set the model"),
    ("provider", "anthropic|openai"),
    ("editor", "line-editor keymap: emacs|vi"),
    ("frontend", "auto|reedline|zsh-pty"),
    ("stream", "toggle token streaming"),
    ("theme", "color preset"),
    ("config", "print active config"),
    ("rehash", "rebuild the command cache"),
    ("help", "show help"),
];

/// First-argument subcommands for a few common tools.
const GIT_SUBCOMMANDS: &[&str] = &[
    "add",
    "branch",
    "checkout",
    "cherry-pick",
    "clean",
    "clone",
    "commit",
    "config",
    "diff",
    "fetch",
    "init",
    "log",
    "merge",
    "mv",
    "pull",
    "push",
    "rebase",
    "remote",
    "reset",
    "restore",
    "revert",
    "rm",
    "show",
    "stash",
    "status",
    "switch",
    "tag",
];
const CARGO_SUBCOMMANDS: &[&str] = &[
    "add", "bench", "build", "check", "clean", "clippy", "doc", "fmt", "init", "install", "new",
    "publish", "remove", "run", "test", "update",
];
const DOCKER_SUBCOMMANDS: &[&str] = &[
    "build", "compose", "exec", "images", "inspect", "kill", "logs", "ps", "pull", "push", "rm",
    "rmi", "run", "start", "stop", "tag",
];
const NPM_SUBCOMMANDS: &[&str] = &[
    "install",
    "ci",
    "run",
    "start",
    "test",
    "build",
    "init",
    "publish",
    "update",
    "uninstall",
    "exec",
    "ls",
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
            _ => command_arg_suggestions(&seg_tokens, word, span)
                .unwrap_or_else(|| complete_paths(word, span, false)),
        }
    }
}

/// Per-command argument completion for a few common tools. Returns `None` to
/// fall back to path completion (e.g. `git checkout <file>`).
fn command_arg_suggestions(seg: &[&str], word: &str, span: Span) -> Option<Vec<Suggestion>> {
    let arg_index = seg.len(); // the current word is argument #arg_index (1 = first)
    if arg_index == 1 {
        let subs: &[&str] = match seg[0] {
            "git" => GIT_SUBCOMMANDS,
            "cargo" => CARGO_SUBCOMMANDS,
            "docker" => DOCKER_SUBCOMMANDS,
            "npm" | "pnpm" | "yarn" => NPM_SUBCOMMANDS,
            _ => return None,
        };
        return Some(str_suggestions(subs, word, span));
    }
    // Dynamic git branch completion for branch-oriented subcommands.
    if seg[0] == "git"
        && matches!(
            seg.get(1),
            Some(&("checkout" | "switch" | "merge" | "rebase"))
        )
    {
        let matches: Vec<Suggestion> = git_branches()
            .into_iter()
            .filter(|b| b.starts_with(word))
            .map(|b| Suggestion {
                value: b,
                span,
                append_whitespace: true,
                ..Default::default()
            })
            .collect();
        if !matches.is_empty() {
            return Some(matches);
        }
    }
    None
}

/// Local git branch names (best-effort; empty if not a repo or git is missing).
fn git_branches() -> Vec<String> {
    std::process::Command::new("git")
        .args(["for-each-ref", "--format=%(refname:short)", "refs/heads"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect()
        })
        .unwrap_or_default()
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
    // Case-insensitive prefix first; fall back to fuzzy subsequence (e.g. `gco`).
    let mut names = cache.matching(word);
    if names.is_empty() && !word.is_empty() {
        names = cache.fuzzy(word); // already ranked best-first
    } else {
        names.sort();
        names.dedup();
    }
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
    let lp = prefix.to_lowercase();
    let mut vars: Vec<(String, String)> = std::env::vars()
        .filter(|(k, _)| k.to_lowercase().starts_with(&lp))
        .collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    vars.truncate(MAX_SUGGESTIONS);
    vars.into_iter()
        .map(|(k, v)| Suggestion {
            value: if brace {
                format!("${{{k}}}")
            } else {
                format!("${k}")
            },
            // Show the (truncated) current value in the menu.
            description: Some(truncate(&v, 40)),
            span,
            append_whitespace: !brace,
            ..Default::default()
        })
        .collect()
}

/// Complete `aishe` meta subcommands and their fixed-value arguments.
fn complete_aishe_meta(seg_tokens: &[&str], word: &str, span: Span) -> Vec<Suggestion> {
    match seg_tokens.len() {
        // `aishe <word>` — the subcommand (with a description in the menu).
        1 => META_SUBCOMMANDS
            .iter()
            .filter(|(name, _)| name.to_lowercase().starts_with(&word.to_lowercase()))
            .map(|(name, desc)| Suggestion {
                value: (*name).to_string(),
                description: Some((*desc).to_string()),
                span,
                append_whitespace: true,
                ..Default::default()
            })
            .collect(),
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

/// Build suggestions from a fixed option list, filtered by `word` prefix
/// (case-insensitive).
fn str_suggestions(options: &[&str], word: &str, span: Span) -> Vec<Suggestion> {
    let lw = word.to_lowercase();
    options
        .iter()
        .filter(|o| o.to_lowercase().starts_with(&lw))
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
    let mut all: Vec<(String, bool)> = Vec::new();
    for entry in entries.flatten() {
        let name = match entry.file_name().into_string() {
            Ok(n) => n,
            Err(_) => continue,
        };
        if name.starts_with('.') && !include_hidden {
            continue;
        }
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if dirs_only && !is_dir {
            continue;
        }
        all.push((name, is_dir));
    }

    // Case-insensitive prefix first; fall back to fuzzy subsequence.
    let lp = file_prefix.to_lowercase();
    let mut chosen: Vec<(String, bool)> = all
        .iter()
        .filter(|(n, _)| n.to_lowercase().starts_with(&lp))
        .cloned()
        .collect();
    if chosen.is_empty() && !file_prefix.is_empty() {
        let names = crate::fuzzy::rank(all.iter().map(|(n, _)| n.clone()).collect(), file_prefix);
        chosen = names
            .into_iter()
            .filter_map(|n| {
                all.iter()
                    .find(|(en, _)| *en == n)
                    .map(|(en, d)| (en.clone(), *d))
            })
            .collect();
    } else {
        chosen.sort_by(|a, b| a.0.cmp(&b.0));
    }
    chosen.truncate(MAX_SUGGESTIONS);

    chosen
        .into_iter()
        .map(|(name, is_dir)| Suggestion {
            value: if is_dir {
                format!("{dir_part}{name}/")
            } else {
                format!("{dir_part}{name}")
            },
            span,
            // Don't append a space after a directory, so the user can keep
            // descending; do after a file.
            append_whitespace: !is_dir,
            ..Default::default()
        })
        .collect()
}

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// Truncate a string to `max` chars, appending `…` if it was cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
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
    fn fuzzy_fallback_when_no_prefix_match() {
        let cache = CommandCache::new();
        cache.insert_all(&["git-checkout", "grep", "ls"]);
        let mut c = AisheCompleter::new(cache);
        // no command starts with "gco", so fuzzy finds git-checkout
        assert!(values(&c.complete("gco", 3)).contains(&"git-checkout".to_string()));
        // case-insensitive prefix
        assert!(values(&c.complete("GRE", 3)).contains(&"grep".to_string()));
    }

    #[test]
    fn per_command_subcommands() {
        let mut c = AisheCompleter::new(CommandCache::new());
        let v = values(&c.complete("git c", 5));
        assert!(v.contains(&"commit".to_string()) && v.contains(&"checkout".to_string()));
        assert_eq!(values(&c.complete("cargo b", 7)), vec!["bench", "build"]);
        // unknown tool falls back to path completion (no subcommand list)
        let v = values(&c.complete("frobnicate sub", 14));
        assert!(!v.iter().any(|x| x == "status"));
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
