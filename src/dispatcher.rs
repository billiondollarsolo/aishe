//! Decide whether an input line is a shell command or a natural-language
//! request, and maintain the command cache that backs that decision.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// The outcome of dispatching one input line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// Run as a shell line (the string may have a leading sigil stripped).
    Shell(String),
    /// Treat as natural language for the LLM.
    NaturalLanguage(String),
    /// An intercepted builtin, pre-tokenized.
    Builtin(Vec<String>),
}

/// Builtins we handle in-process to persist shell state.
const INTERCEPTED: &[&str] = &[
    "cd", "export", "unset", "source", ".", "exit", "quit", "aishe",
];

/// Hardcoded fallback list of zsh builtins, used if querying zsh fails.
const FALLBACK_BUILTINS: &[&str] = &[
    "alias", "autoload", "bindkey", "command", "compgen", "declare", "echo", "eval", "exec", "fc",
    "getopts", "hash", "jobs", "kill", "let", "local", "print", "printf", "pushd", "popd", "read",
    "readonly", "set", "setopt", "shift", "test", "trap", "type", "typeset", "ulimit", "umask",
    "wait", "which", "zmodload", "cd", "export", "unset", "source", "true", "false", "bg", "fg",
    "disown", "enable", "disable", "where", "whence",
];

/// Shared command cache, swappable from a background thread.
#[derive(Clone)]
pub struct CommandCache {
    inner: Arc<RwLock<HashSet<String>>>,
}

impl Default for CommandCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandCache {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Build the cache: scan `$PATH` synchronously (fast), then fetch zsh
    /// builtins/aliases/functions on a background thread so the first prompt
    /// is not blocked.
    pub fn build(&self, shell: &Path) {
        // Synchronous PATH scan.
        let path_cmds = scan_path();
        {
            let mut w = self.inner.write().unwrap();
            w.extend(path_cmds);
            w.extend(INTERCEPTED.iter().map(|s| s.to_string()));
        }

        // Background fetch of shell builtins + user aliases/functions.
        let inner = Arc::clone(&self.inner);
        let shell = shell.to_path_buf();
        std::thread::spawn(move || {
            let mut extra = fetch_builtins(&shell);
            extra.extend(fetch_aliases_and_functions(&shell));
            if !extra.is_empty() {
                let mut w = inner.write().unwrap();
                w.extend(extra);
            }
        });
    }

    /// Rebuild synchronously (used by `aishe rehash`).
    pub fn rehash(&self, shell: &Path) {
        let mut fresh: HashSet<String> = scan_path();
        fresh.extend(INTERCEPTED.iter().map(|s| s.to_string()));
        fresh.extend(fetch_builtins(shell));
        fresh.extend(fetch_aliases_and_functions(shell));
        let mut w = self.inner.write().unwrap();
        *w = fresh;
    }

    pub fn contains(&self, token: &str) -> bool {
        self.inner.read().unwrap().contains(token)
    }

    /// Command names beginning with `prefix` (for tab completion). Unsorted.
    pub fn matching(&self, prefix: &str) -> Vec<String> {
        self.inner
            .read()
            .unwrap()
            .iter()
            .filter(|n| n.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Insert a set of command names (used by tests and seeding).
    pub fn insert_all(&self, items: &[&str]) {
        let mut w = self.inner.write().unwrap();
        for i in items {
            w.insert((*i).to_string());
        }
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().len()
    }
}

/// Dispatch one input line against the cache (PRD §4.2 decision order).
pub fn dispatch(line: &str, cache: &CommandCache) -> Dispatch {
    let trimmed = line.trim();

    // 1. Forced LLM.
    if let Some(rest) = trimmed.strip_prefix('?') {
        return Dispatch::NaturalLanguage(rest.trim().to_string());
    }
    // 2. Forced shell (safety-exempt).
    if let Some(rest) = trimmed.strip_prefix('!') {
        return Dispatch::Shell(rest.trim().to_string());
    }

    let tokens = tokenize(trimmed);
    let first = tokens.first().map(|s| s.as_str()).unwrap_or("");

    // 3. Intercepted builtins.
    if INTERCEPTED.contains(&first) {
        return Dispatch::Builtin(tokens);
    }

    // 4. Shell-syntax signals.
    if starts_with_shell_syntax(trimmed) {
        return Dispatch::Shell(trimmed.to_string());
    }

    // Env assignments: `FOO=bar cmd`. A pure assignment line is shell.
    let effective_first = effective_command_token(&tokens);
    if let EffectiveHead::Assignment = effective_first {
        return Dispatch::Shell(trimmed.to_string());
    }

    // 5. Pipeline check (before the single-token cache hit, so a cached head
    //    followed by an uncached pipeline segment still routes to NL).
    if contains_operator(trimmed) {
        let heads = pipeline_heads(trimmed);
        if !heads.is_empty() && heads.iter().all(|h| cache.contains(h)) {
            return Dispatch::Shell(trimmed.to_string());
        }
        return Dispatch::NaturalLanguage(trimmed.to_string());
    }

    // 6. Cache hit on the effective head.
    if let EffectiveHead::Token(tok) = effective_first {
        if cache.contains(&tok) {
            return Dispatch::Shell(trimmed.to_string());
        }
    }

    // 7. Else → natural language.
    Dispatch::NaturalLanguage(trimmed.to_string())
}

enum EffectiveHead {
    /// The line is (only) env assignments, e.g. `FOO=bar` — treat as shell.
    Assignment,
    /// The effective command token after skipping leading assignments.
    Token(String),
    None,
}

/// Skip leading `K=V` assignments and return the first real command token.
fn effective_command_token(tokens: &[String]) -> EffectiveHead {
    let mut saw_assignment = false;
    for tok in tokens {
        if is_assignment(tok) {
            saw_assignment = true;
            continue;
        }
        return EffectiveHead::Token(tok.clone());
    }
    if saw_assignment {
        EffectiveHead::Assignment
    } else {
        EffectiveHead::None
    }
}

/// An env assignment token: contains `=` before any space, and the part before
/// `=` is a valid identifier.
fn is_assignment(tok: &str) -> bool {
    if let Some((name, _)) = tok.split_once('=') {
        !name.is_empty()
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && name
                .chars()
                .next()
                .map(|c| !c.is_ascii_digit())
                .unwrap_or(false)
    } else {
        false
    }
}

fn starts_with_shell_syntax(line: &str) -> bool {
    line.starts_with("./")
        || line.starts_with('/')
        || line.starts_with("~/")
        || line.starts_with("$(")
        || line.starts_with('(')
}

fn contains_operator(line: &str) -> bool {
    line.contains('|') || line.contains("&&") || line.contains("||") || line.contains(';')
}

/// Split a line on `|`, `&&`, `||`, `;` and return the head token of each
/// non-empty segment. Naive (ignores quoting), per PRD v0.1.
fn pipeline_heads(line: &str) -> Vec<String> {
    let normalized = line.replace("&&", "|").replace("||", "|").replace(';', "|");
    let mut heads = Vec::new();
    for segment in normalized.split('|') {
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        let toks = tokenize(seg);
        match effective_command_token(&toks) {
            EffectiveHead::Token(t) => heads.push(t),
            EffectiveHead::Assignment => {} // pure assignment segment; ignore
            EffectiveHead::None => return Vec::new(),
        }
    }
    heads
}

/// Very small whitespace tokenizer (quoting-naive; sufficient for head checks).
fn tokenize(line: &str) -> Vec<String> {
    line.split_whitespace().map(|s| s.to_string()).collect()
}

/// Scan every `$PATH` directory for entries with any execute bit set.
fn scan_path() -> HashSet<String> {
    let mut set = HashSet::new();
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return set,
    };
    for dir in std::env::split_paths(&path) {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                continue;
            }
            if meta.permissions().mode() & 0o111 != 0 {
                if let Some(name) = entry.file_name().to_str() {
                    set.insert(name.to_string());
                }
            }
        }
    }
    set
}

/// Query the shell's builtins, with a 500ms timeout and a hardcoded fallback.
fn fetch_builtins(shell: &Path) -> HashSet<String> {
    let script = "print -l ${(k)builtins}";
    match run_with_timeout(shell, &["-c", script], Duration::from_millis(500)) {
        Some(out) if !out.trim().is_empty() => out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => FALLBACK_BUILTINS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Query user aliases and function names via an interactive shell, with a 2s
/// timeout (`.zshrc` may be slow). On timeout, returns empty with a warning.
fn fetch_aliases_and_functions(shell: &Path) -> HashSet<String> {
    let script = "alias +; print -l ${(k)functions}";
    match run_with_timeout(shell, &["-ic", script], Duration::from_secs(2)) {
        Some(out) => out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        None => {
            eprintln!("\x1b[2maishe: aliases/functions query timed out; continuing\x1b[0m");
            HashSet::new()
        }
    }
}

/// Run a shell command with a timeout; kill on expiry. Returns captured
/// stdout, or None on timeout/failure. stderr is always discarded.
fn run_with_timeout(shell: &Path, args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new(shell)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                let out = child.wait_with_output().ok()?;
                return Some(String::from_utf8_lossy(&out.stdout).to_string());
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with(items: &[&str]) -> CommandCache {
        let cache = CommandCache::new();
        {
            let mut w = cache.inner.write().unwrap();
            for i in items {
                w.insert(i.to_string());
            }
        }
        cache
    }

    #[test]
    fn forced_llm_and_shell() {
        let c = cache_with(&[]);
        assert_eq!(
            dispatch("?ls", &c),
            Dispatch::NaturalLanguage("ls".to_string())
        );
        assert_eq!(
            dispatch("!frobnicate", &c),
            Dispatch::Shell("frobnicate".to_string())
        );
    }

    #[test]
    fn intercepted_builtins() {
        let c = cache_with(&[]);
        assert_eq!(
            dispatch("cd /tmp", &c),
            Dispatch::Builtin(vec!["cd".into(), "/tmp".into()])
        );
        assert!(matches!(dispatch("export FOO=1", &c), Dispatch::Builtin(_)));
        assert!(matches!(dispatch("aishe help", &c), Dispatch::Builtin(_)));
    }

    #[test]
    fn shell_syntax_signals() {
        let c = cache_with(&[]);
        assert!(matches!(dispatch("./run.sh", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("/usr/bin/env", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("~/bin/tool", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("$(date)", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("(echo hi)", &c), Dispatch::Shell(_)));
    }

    #[test]
    fn env_assignment_is_shell() {
        let c = cache_with(&["env"]);
        assert!(matches!(dispatch("FOO=1 env", &c), Dispatch::Shell(_)));
        // pure assignment.
        assert!(matches!(dispatch("FOO=bar", &c), Dispatch::Shell(_)));
    }

    #[test]
    fn cache_hit_and_miss() {
        let c = cache_with(&["git", "ls", "grep"]);
        assert!(matches!(dispatch("git status", &c), Dispatch::Shell(_)));
        // single unknown token → NL.
        assert!(matches!(
            dispatch("frobnicate", &c),
            Dispatch::NaturalLanguage(_)
        ));
        // NL sentence.
        assert!(matches!(
            dispatch("show me big files", &c),
            Dispatch::NaturalLanguage(_)
        ));
    }

    #[test]
    fn pipeline_all_cached_is_shell() {
        let c = cache_with(&["ls", "grep"]);
        assert!(matches!(dispatch("ls | grep x", &c), Dispatch::Shell(_)));
    }

    #[test]
    fn pipeline_with_miss_is_nl() {
        let c = cache_with(&["find"]);
        assert!(matches!(
            dispatch("find big files | wat", &c),
            Dispatch::NaturalLanguage(_)
        ));
    }

    #[test]
    fn assignment_detection() {
        assert!(is_assignment("FOO=bar"));
        assert!(is_assignment("_X=1"));
        assert!(!is_assignment("1FOO=bar"));
        assert!(!is_assignment("notassignment"));
        assert!(!is_assignment("--flag=value")); // leading dash not identifier
    }
}
