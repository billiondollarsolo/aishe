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
    "cd", "export", "unset", "source", ".", "exit", "quit", "aishe", "pushd", "popd", "dirs",
];

/// Hardcoded fallback list of zsh builtins, used if querying zsh fails.
const FALLBACK_BUILTINS: &[&str] = &[
    "alias", "autoload", "bindkey", "command", "compgen", "declare", "echo", "eval", "exec", "fc",
    "getopts", "hash", "jobs", "kill", "let", "local", "print", "printf", "pushd", "popd", "read",
    "readonly", "set", "setopt", "shift", "test", "trap", "type", "typeset", "ulimit", "umask",
    "wait", "which", "zmodload", "cd", "export", "unset", "source", "true", "false", "bg", "fg",
    "disown", "enable", "disable", "where", "whence", ":", "repeat", "noglob",
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
        // Synchronous PATH scan + a fallback builtin set, so pure shell builtins
        // (`print`, `let`, `typeset`, `jobs`, `:`, …) are recognized immediately
        // — before the background fetch lands. This matters for `-c`/one-shot and
        // the very first interactive prompt, which otherwise race the thread and
        // misroute builtins to the LLM.
        let path_cmds = scan_path();
        {
            let mut w = self.inner.write().unwrap();
            w.extend(path_cmds);
            w.extend(INTERCEPTED.iter().map(|s| s.to_string()));
            w.extend(FALLBACK_BUILTINS.iter().map(|s| s.to_string()));
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

    /// Command names beginning with `prefix`, case-insensitively (for tab
    /// completion). Unsorted.
    pub fn matching(&self, prefix: &str) -> Vec<String> {
        let lp = prefix.to_lowercase();
        self.inner
            .read()
            .unwrap()
            .iter()
            .filter(|n| n.to_lowercase().starts_with(&lp))
            .cloned()
            .collect()
    }

    /// Command names fuzzily matching `query` (subsequence), ranked best-first.
    /// Used as a fallback when there are no prefix matches.
    pub fn fuzzy(&self, query: &str) -> Vec<String> {
        let all: Vec<String> = self.inner.read().unwrap().iter().cloned().collect();
        crate::fuzzy::rank(all, query)
    }

    /// The closest known command to `token` within `max_dist` edits, if any (for
    /// "did you mean" spelling correction). `None` when `token` is already a known
    /// command or nothing is close enough.
    pub fn correction(&self, token: &str, max_dist: usize) -> Option<String> {
        let guard = self.inner.read().unwrap();
        crate::fuzzy::correction(token, guard.iter().map(String::as_str), max_dist)
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

    // 2b. Slash-commands: `/<meta> …` is an alias for `aishe <meta> …`. Only a
    //     known meta subcommand intercepts, so `/usr/bin/x` stays a shell path.
    if let Some(rest) = trimmed.strip_prefix('/') {
        let sub = rest.split_whitespace().next().unwrap_or("");
        if is_meta_subcommand(sub) {
            let mut toks = vec!["aishe".to_string()];
            toks.extend(rest.split_whitespace().map(str::to_string));
            return Dispatch::Builtin(toks);
        }
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

    // 4b. Function definitions (`name() { … }`, `function name { … }`) — route
    //     to shell before the operator/cache checks (the body may contain `;`).
    if function_def_name(trimmed).is_some() {
        return Dispatch::Shell(trimmed.to_string());
    }

    // 4c. Shell control structures (`for`/`while`/`if`/`case`/…, `[[`, `((`,
    //     `{`) — route to shell so loops/conditionals can be typed and run.
    if is_shell_construct_head(trimmed) {
        return Dispatch::Shell(trimmed.to_string());
    }

    // Env assignments: `FOO=bar cmd`. A pure assignment line is shell.
    let effective_first = effective_command_token(&tokens);
    if let EffectiveHead::Assignment = effective_first {
        return Dispatch::Shell(trimmed.to_string());
    }

    // Array assignment at the head of the line (`arr=(a b c)`, `path+=(/x)`),
    // possibly followed by `; cmd …` — route the whole line to shell.
    if is_array_assignment(trimmed) {
        return Dispatch::Shell(trimmed.to_string());
    }

    // 5. Pipelines / compound lines, split quote-aware on `|`/`;`/`&&`/`||`. It's
    //    shell if every segment's head is a known command or a shell reserved
    //    word (so `grep -E 'a|b'` stays one segment, and `x=1; while …; done`
    //    routes to shell). Otherwise → natural language.
    let segments = split_top_level(trimmed);
    if segments.len() > 1 {
        let all_shell = segments.iter().all(|seg| {
            if is_array_assignment(seg) {
                return true; // `arr=(a b c)` segment is shell
            }
            match effective_command_token(&tokenize(seg)) {
                EffectiveHead::Token(t) => cache.contains(&t) || is_reserved_word(&t),
                _ => true, // assignment-only or empty segment is fine
            }
        });
        return if all_shell {
            Dispatch::Shell(trimmed.to_string())
        } else {
            Dispatch::NaturalLanguage(trimmed.to_string())
        };
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

/// If `line` begins a shell function definition (`name() …`, `name () …`, or
/// `function name …`), return the function's name. Used to route definitions to
/// the shell, balance their braces in the validator, and persist them.
pub fn function_def_name(line: &str) -> Option<String> {
    let t = line.trim();
    // `function name [()] [{ … }]`
    if let Some(rest) = t.strip_prefix("function ") {
        let name = rest
            .trim_start()
            .split(|c: char| c.is_whitespace() || c == '(' || c == '{')
            .next()
            .unwrap_or("");
        return is_valid_func_name(name).then(|| name.to_string());
    }
    // `name() …` / `name () …`  (the `()` is required to disambiguate)
    if let Some(paren) = t.find('(') {
        let name = t[..paren].trim();
        if t[paren + 1..].trim_start().starts_with(')') && is_valid_func_name(name) {
            return Some(name.to_string());
        }
    }
    None
}

/// A POSIX-ish function name: starts with a letter/underscore, then word chars
/// or `-`.
fn is_valid_func_name(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// True if the line begins a shell control structure / compound command, so it
/// should run as shell (and the validator can continue it across lines).
pub fn is_shell_construct_head(line: &str) -> bool {
    let t = line.trim_start();
    if t.starts_with("[[") || t.starts_with("((") || t.starts_with('{') {
        return true;
    }
    let first = t.split(|c: char| c.is_whitespace()).next().unwrap_or("");
    matches!(
        first,
        "if" | "for" | "while" | "until" | "case" | "select" | "function" | "time" | "repeat"
    )
}

/// A zsh array assignment at the start of `seg`, e.g. `arr=(a b c)` or
/// `path+=(/x)`. The whitespace tokenizer splits the parenthesized values, so
/// without this the head resolves to a value word and the line misroutes to the
/// LLM. Recognized as shell.
fn is_array_assignment(seg: &str) -> bool {
    let s = seg.trim_start();
    let eq = match s.find('=') {
        Some(i) => i,
        None => return false,
    };
    let name = s[..eq].strip_suffix('+').unwrap_or(&s[..eq]);
    !name.is_empty()
        && name
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && s[eq + 1..].trim_start().starts_with('(')
}

fn starts_with_shell_syntax(line: &str) -> bool {
    line.starts_with("./")
        || line.starts_with('/')
        || line.starts_with("~/")
        || line.starts_with("$(")
        || line.starts_with('(')
}

/// Split a line into top-level segments on **unquoted** `|`, `||`, `&&`, `;`.
/// Quote/escape-aware, so operators inside `'…'`/`"…"` (e.g. `grep -E 'a|b'`)
/// don't split. Empty segments are dropped.
fn split_top_level(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut segs = Vec::new();
    let mut cur = String::new();
    let (mut in_s, mut in_d, mut esc) = (false, false, false);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if esc {
            cur.push(c);
            esc = false;
            i += 1;
            continue;
        }
        if in_s {
            cur.push(c);
            if c == '\'' {
                in_s = false;
            }
            i += 1;
            continue;
        }
        if in_d {
            if c == '\\' {
                cur.push(c);
                esc = true;
            } else {
                cur.push(c);
                if c == '"' {
                    in_d = false;
                }
            }
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                cur.push(c);
                esc = true;
                i += 1;
            }
            '\'' => {
                in_s = true;
                cur.push(c);
                i += 1;
            }
            '"' => {
                in_d = true;
                cur.push(c);
                i += 1;
            }
            ';' => {
                segs.push(std::mem::take(&mut cur));
                i += 1;
            }
            '|' => {
                segs.push(std::mem::take(&mut cur));
                i += if chars.get(i + 1) == Some(&'|') { 2 } else { 1 };
            }
            '&' if chars.get(i + 1) == Some(&'&') => {
                segs.push(std::mem::take(&mut cur));
                i += 2;
            }
            _ => {
                cur.push(c);
                i += 1;
            }
        }
    }
    segs.push(cur);
    segs.into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// `aishe` meta subcommands (also reachable as `/<name>` slash-commands). Keep
/// in sync with `handle_meta` in main.rs and the completer's list.
pub fn is_meta_subcommand(w: &str) -> bool {
    matches!(
        w,
        "mode"
            | "model"
            | "provider"
            | "editor"
            | "frontend"
            | "stream"
            | "structured"
            | "theme"
            | "config"
            | "rehash"
            | "commands"
            | "skills"
            | "usage"
            | "reset"
            | "ghost"
            | "help"
    )
}

/// Shell reserved words / compound-command heads that are valid segment heads
/// even when not in the command cache (so control structures route to shell).
fn is_reserved_word(w: &str) -> bool {
    matches!(
        w,
        "if" | "then"
            | "elif"
            | "else"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "time"
            | "in"
            | "{"
            | "}"
            | "!"
            | "[["
            | "(("
            | "["
            | "repeat"
    )
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
/// Also sources the user's `.aishrc` files so aliases/functions defined there
/// are recognized as commands (the executor sources the same files at run time).
fn fetch_aliases_and_functions(shell: &Path) -> HashSet<String> {
    let script = "[ -f \"$HOME/.aishrc\" ] && source \"$HOME/.aishrc\" 2>/dev/null; \
         [ -f \"${XDG_CONFIG_HOME:-$HOME/.config}/aishe/aishrc\" ] && \
         source \"${XDG_CONFIG_HOME:-$HOME/.config}/aishe/aishrc\" 2>/dev/null; \
         alias +; print -l ${(k)functions}";
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
    fn array_assignment_is_shell() {
        let c = cache_with(&["echo"]);
        // `arr=(a b c)` has spaces inside the parens — must not misroute.
        assert!(matches!(dispatch("arr=(a b c)", &c), Dispatch::Shell(_)));
        assert!(matches!(
            dispatch("arr=(a b c); echo ${#arr}", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(dispatch("path+=(/x /y)", &c), Dispatch::Shell(_)));
        // not an array assignment: `echo a=(b)` is a command, routes via head.
        assert!(matches!(dispatch("echo a=(b)", &c), Dispatch::Shell(_)));
    }

    #[test]
    fn repeat_keyword_is_shell() {
        let c = cache_with(&["echo"]);
        assert!(matches!(
            dispatch("repeat 3 echo hi", &c),
            Dispatch::Shell(_)
        ));
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
    fn slash_commands_route_to_meta() {
        let c = cache_with(&[]);
        assert_eq!(
            dispatch("/mode auto", &c),
            Dispatch::Builtin(vec!["aishe".into(), "mode".into(), "auto".into()])
        );
        assert_eq!(
            dispatch("/help", &c),
            Dispatch::Builtin(vec!["aishe".into(), "help".into()])
        );
        // an absolute path is NOT a slash-command
        assert!(matches!(dispatch("/usr/bin/env", &c), Dispatch::Shell(_)));
        assert!(matches!(
            dispatch("/notacmd", &c),
            Dispatch::NaturalLanguage(_) | Dispatch::Shell(_)
        ));
    }

    #[test]
    fn quoted_operators_are_not_pipelines() {
        let c = cache_with(&["grep", "sed", "awk"]);
        // `|` / `;` inside quotes must NOT be treated as a pipeline → stays shell.
        assert!(matches!(
            dispatch("grep -E 'foo|bar' a.txt", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(
            dispatch("sed 's/a/b/;s/c/d/' f", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(
            dispatch(r#"awk -F'|' '{print $1}' f"#, &c),
            Dispatch::Shell(_)
        ));
    }

    #[test]
    fn compound_with_reserved_words_is_shell() {
        let c = cache_with(&["echo"]);
        assert!(matches!(
            dispatch("i=0; while [ $i -lt 3 ]; do echo $i; i=$((i+1)); done", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(
            dispatch("echo a; if true; then echo b; fi", &c),
            Dispatch::Shell(_)
        ));
    }

    #[test]
    fn function_definitions_detected() {
        assert_eq!(
            function_def_name("greet() { echo hi; }").as_deref(),
            Some("greet")
        );
        assert_eq!(function_def_name("greet () {").as_deref(), Some("greet"));
        assert_eq!(
            function_def_name("function greet {").as_deref(),
            Some("greet")
        );
        assert_eq!(
            function_def_name("function greet() {").as_deref(),
            Some("greet")
        );
        // not function definitions
        assert_eq!(function_def_name("echo (hi)"), None);
        assert_eq!(function_def_name("a=$(date)"), None);
        assert_eq!(function_def_name("git status"), None);
    }

    #[test]
    fn control_structures_route_to_shell() {
        let c = cache_with(&[]);
        assert!(matches!(
            dispatch("for i in 1 2 3; do", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(dispatch("if true; then", &c), Dispatch::Shell(_)));
        assert!(matches!(
            dispatch("while read l; do", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(dispatch("case $x in", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("[[ -f x ]]", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("{ echo hi; }", &c), Dispatch::Shell(_)));
        // a normal command that merely mentions a keyword is not a construct
        assert!(matches!(
            dispatch("show me the iframe", &c),
            Dispatch::NaturalLanguage(_)
        ));
    }

    #[test]
    fn function_def_routes_to_shell() {
        let c = cache_with(&[]);
        assert!(matches!(
            dispatch("greet() { echo hi; }", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(dispatch("greet() {", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("function g {", &c), Dispatch::Shell(_)));
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
