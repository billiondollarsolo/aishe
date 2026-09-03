//! Decide whether an input line is a shell command or a natural-language
//! request, and maintain the command cache that backs that decision.

use std::collections::HashSet;
use std::fmt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde::Serialize;

use crate::command_surface;

/// Schema version for the public `aishe route --json` contract.
pub const ROUTE_SCHEMA_VERSION: u32 = 1;

/// Maximum user-input characters retained by diagnostic and debug views.
const ROUTE_DIAGNOSTIC_INPUT_CHARS: usize = 512;
/// Maximum effective-head characters retained by diagnostic and debug views.
const ROUTE_DIAGNOSTIC_HEAD_CHARS: usize = 128;

/// Schema version for the deliberately separate typo-assistance research
/// contract. A cue is advisory only: it never changes [`RouteDecision`],
/// executes a command, or authorizes an agent request.
pub const TYPO_ASSISTANCE_SCHEMA_VERSION: u32 = 1;

/// One conservative two-word question form shared with generated zsh routing.
/// Keep the table declarative: changing a pair updates Rust classification,
/// zsh highlighting, and zsh Enter submission together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuestionPairRule {
    pub first: &'static str,
    pub seconds: &'static [&'static str],
}

const QUESTION_VERBS: &[&str] = &[
    "is", "are", "was", "were", "do", "does", "did", "can", "could", "would", "should", "will",
];

/// Canonical two-word natural-language grammar for command-name collisions.
pub const QUESTION_PAIR_RULES: &[QuestionPairRule] = &[
    QuestionPairRule {
        first: "what",
        seconds: QUESTION_VERBS,
    },
    QuestionPairRule {
        first: "where",
        seconds: QUESTION_VERBS,
    },
    QuestionPairRule {
        first: "when",
        seconds: QUESTION_VERBS,
    },
    QuestionPairRule {
        first: "why",
        seconds: QUESTION_VERBS,
    },
    QuestionPairRule {
        first: "who",
        seconds: &[
            "is", "are", "was", "were", "am", "do", "does", "did", "can", "could", "would",
            "should", "will",
        ],
    },
    QuestionPairRule {
        first: "how",
        seconds: &[
            "is", "are", "was", "were", "do", "does", "did", "can", "could", "would", "should",
            "will", "many", "much", "long", "far", "old", "often",
        ],
    },
    QuestionPairRule {
        first: "can",
        seconds: &["you"],
    },
    QuestionPairRule {
        first: "could",
        seconds: &["you"],
    },
    QuestionPairRule {
        first: "would",
        seconds: &["you"],
    },
    QuestionPairRule {
        first: "will",
        seconds: &["you"],
    },
    QuestionPairRule {
        first: "should",
        seconds: &["i", "we"],
    },
    QuestionPairRule {
        first: "is",
        seconds: &["there"],
    },
    QuestionPairRule {
        first: "are",
        seconds: &["there"],
    },
    QuestionPairRule {
        first: "do",
        seconds: &["you"],
    },
    QuestionPairRule {
        first: "does",
        seconds: &["the"],
    },
    QuestionPairRule {
        first: "did",
        seconds: &["the"],
    },
];

/// A trailing question mark is agent evidence only for these first words.
pub const TRAILING_QUESTION_HEADS: &[&str] = &[
    "what", "where", "who", "when", "why", "how", "which", "whose", "whom", "can", "could",
    "would", "will", "should", "is", "are", "do", "does", "did",
];

/// Characters which make a line shell-shaped before question-pair grammar.
pub const QUESTION_SHELL_EVIDENCE: &[char] =
    &['|', ';', '&', '<', '>', '$', '`', '(', ')', '{', '}'];

/// The destination selected for one submitted line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteKind {
    Shell,
    NaturalLanguage,
    Builtin,
}

impl fmt::Display for RouteKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Shell => "shell",
            Self::NaturalLanguage => "natural_language",
            Self::Builtin => "builtin",
        };
        formatter.write_str(name)
    }
}

/// Stable, deterministic evidence for a route decision.
///
/// Variant names are part of the public JSON contract. Add variants rather
/// than repurposing an existing reason when the classifier gains a new rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    EmptyInput,
    ForcedAgent,
    ForcedShell,
    SlashCommand,
    InterceptedBuiltin,
    ShellSyntax,
    FunctionDefinition,
    ControlStructure,
    Assignment,
    QuestionGrammar,
    CompoundShell,
    CompoundUnknown,
    KnownCommand,
    UnknownInput,
}

impl fmt::Display for RouteReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::EmptyInput => "empty_input",
            Self::ForcedAgent => "forced_agent",
            Self::ForcedShell => "forced_shell",
            Self::SlashCommand => "slash_command",
            Self::InterceptedBuiltin => "intercepted_builtin",
            Self::ShellSyntax => "shell_syntax",
            Self::FunctionDefinition => "function_definition",
            Self::ControlStructure => "control_structure",
            Self::Assignment => "assignment",
            Self::QuestionGrammar => "question_grammar",
            Self::CompoundShell => "compound_shell",
            Self::CompoundUnknown => "compound_unknown",
            Self::KnownCommand => "known_command",
            Self::UnknownInput => "unknown_input",
        };
        formatter.write_str(name)
    }
}

/// Classifier surface responsible for the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Rust,
    GeneratedZsh,
    GeneratedBash,
    Explicit,
}

impl fmt::Display for RouteSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Rust => "rust",
            Self::GeneratedZsh => "generated_zsh",
            Self::GeneratedBash => "generated_bash",
            Self::Explicit => "explicit",
        };
        formatter.write_str(name)
    }
}

/// A complete, inspectable route decision.
///
/// `normalized` is the exact executable/prompt payload after an explicit sigil
/// is stripped. Debug and CLI diagnostic representations are separately
/// bounded so a pathological input cannot flood logs or inject control bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct RouteDecision {
    pub kind: RouteKind,
    pub normalized: String,
    pub reason: RouteReason,
    pub head: Option<String>,
    pub known_command: bool,
    pub ambiguous: bool,
    pub source: RouteSource,
    dispatch: Dispatch,
}

impl fmt::Debug for RouteDecision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RouteDecision")
            .field("kind", &self.kind)
            .field(
                "normalized",
                &safe_bounded(&self.normalized, ROUTE_DIAGNOSTIC_INPUT_CHARS),
            )
            .field("reason", &self.reason)
            .field(
                "head",
                &self
                    .head
                    .as_deref()
                    .map(|head| safe_bounded(head, ROUTE_DIAGNOSTIC_HEAD_CHARS)),
            )
            .field("known_command", &self.known_command)
            .field("ambiguous", &self.ambiguous)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// Machine-readable instruction for selecting the opposite route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteOverride {
    pub kind: RouteKind,
    pub prefix: &'static str,
    pub guidance: &'static str,
    pub safety_bypass: bool,
}

/// Bounded public representation used by `aishe route --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouteDiagnostic {
    pub schema_version: u32,
    pub kind: RouteKind,
    pub reason: RouteReason,
    pub normalized: String,
    pub head: Option<String>,
    pub known_command: bool,
    pub ambiguous: bool,
    pub source: RouteSource,
    /// Whether the selected route bypasses AIShe's AI command safety gate.
    pub safety_bypass: bool,
    pub opposite_route_override: RouteOverride,
}

/// A conservative, process-local spelling cue for an otherwise unknown head.
///
/// This is intentionally not embedded in [`RouteDecision`] or
/// [`RouteDiagnostic`]. Routing remains a stable, deterministic decision; a
/// suggestion depends on the commands visible on this machine's `PATH` and is
/// therefore optional evidence presented after classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypoAssistance {
    pub schema_version: u32,
    pub original: String,
    pub candidate: String,
    pub edit_distance: usize,
    pub executes_automatically: bool,
}

impl RouteDecision {
    pub fn into_dispatch(self) -> Dispatch {
        self.dispatch
    }

    pub fn opposite_route_override(&self) -> RouteOverride {
        match self.kind {
            RouteKind::Shell => RouteOverride {
                kind: RouteKind::NaturalLanguage,
                prefix: "?",
                guidance: "prefix ? to force the agent route",
                safety_bypass: false,
            },
            RouteKind::NaturalLanguage | RouteKind::Builtin => RouteOverride {
                kind: RouteKind::Shell,
                prefix: "!",
                guidance: "prefix ! to force the shell route; this bypasses the AI safety gate",
                safety_bypass: true,
            },
        }
    }

    /// Produce a bounded, schema-versioned diagnostic object. Control bytes are
    /// retained as data here and escaped by JSON serialization; text rendering
    /// should call [`safe_diagnostic_text`] before writing a field to a TTY.
    pub fn diagnostic(&self) -> RouteDiagnostic {
        RouteDiagnostic {
            schema_version: ROUTE_SCHEMA_VERSION,
            kind: self.kind,
            reason: self.reason,
            normalized: bounded(&self.normalized, ROUTE_DIAGNOSTIC_INPUT_CHARS),
            head: self
                .head
                .as_deref()
                .map(|head| bounded(head, ROUTE_DIAGNOSTIC_HEAD_CHARS)),
            known_command: self.known_command,
            ambiguous: self.ambiguous,
            source: self.source,
            safety_bypass: self.reason == RouteReason::ForcedShell,
            opposite_route_override: self.opposite_route_override(),
        }
    }
}

/// Escape control characters and bound a user-controlled value for terminal
/// diagnostics. The classifier's execution payload is never altered.
pub fn safe_diagnostic_text(value: &str) -> String {
    safe_bounded(value, ROUTE_DIAGNOSTIC_INPUT_CHARS)
}

fn bounded(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut result: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        result.push('…');
    }
    result
}

fn safe_bounded(value: &str, max_chars: usize) -> String {
    bounded(value, max_chars)
        .chars()
        .flat_map(char::escape_default)
        .collect()
}

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
    "cd", "export", "unset", "source", ".", "exit", "quit", "pushd", "popd", "dirs",
    // Background-job builtins (reedline front-end manages the job table).
    "jobs", "fg", "bg", "wait", "disown", // History listing from the timestamped log.
    "history",
];

/// Hardcoded fallback list of zsh builtins, used if querying zsh fails.
const FALLBACK_BUILTINS: &[&str] = &[
    "alias",
    "autoload",
    "bindkey",
    "command",
    "compgen",
    "declare",
    "echo",
    "eval",
    "exec",
    "fc",
    "getopts",
    "hash",
    "jobs",
    "kill",
    "let",
    "local",
    "print",
    "printf",
    "pushd",
    "popd",
    "read",
    "readonly",
    "set",
    "setopt",
    "shift",
    "test",
    "trap",
    "type",
    "typeset",
    "ulimit",
    "umask",
    "wait",
    "which",
    "zmodload",
    "cd",
    "export",
    "unset",
    "source",
    "true",
    "false",
    "bg",
    "fg",
    "disown",
    "enable",
    "disable",
    "where",
    "whence",
    ":",
    "repeat",
    "noglob",
    "unsetopt",
    "integer",
    "float",
    "unhash",
    "unfunction",
    "zcompile",
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

    /// Populate only process-local command evidence: executable names from the
    /// current `PATH` plus AIShe's intercepted and fallback shell builtins.
    ///
    /// Unlike [`Self::build`], this does not spawn a shell or a discovery
    /// thread. It is used by `aishe route`, which must remain a backend-free,
    /// configuration-free diagnostic fast path.
    pub fn discover_local(&self) {
        let mut commands = self.write();
        commands.extend(scan_path());
        commands.extend(INTERCEPTED.iter().map(|name| (*name).to_owned()));
        commands.extend(FALLBACK_BUILTINS.iter().map(|name| (*name).to_owned()));
    }

    /// Read/write the command set, recovering from a poisoned lock rather than
    /// panicking. The set is shared with a background fetch thread; if that thread
    /// ever panicked while holding the lock, a plain `.unwrap()` here would cascade
    /// into a shell crash — recovering the guard keeps the shell alive (worst case
    /// the set is momentarily incomplete, which only affects command routing).
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashSet<String>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashSet<String>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
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
            let mut w = self.write();
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
                let mut w = inner.write().unwrap_or_else(|e| e.into_inner());
                w.extend(extra);
            }
        });
    }

    /// Seed only the command names needed to classify one input line.
    ///
    /// This is the conservative `aishe -c` fast path: it avoids walking every
    /// directory in `$PATH`, starting shell-discovery threads, or constructing
    /// any AI/backend state. Builtins are known locally; every other candidate
    /// must resolve to an executable regular file on the current `$PATH`.
    ///
    /// The caller may use the result only when [`dispatch`] returns
    /// [`Dispatch::Shell`]. A builtin or natural-language result is deliberately
    /// inconclusive because an alias/function discovered from `.aishrc` may
    /// still change full dispatch.
    fn seed_for_line(&self, line: &str) {
        {
            let mut commands = self.write();
            commands.extend(FALLBACK_BUILTINS.iter().map(|name| name.to_string()));
        }
        for segment in split_top_level(line) {
            let EffectiveHead::Token(head) = effective_command_token(&tokenize(&segment)) else {
                continue;
            };
            if path_executable_exists(&head) {
                self.write().insert(head);
            }
        }
    }

    /// Rebuild synchronously (used by `aishe rehash`).
    pub fn rehash(&self, shell: &Path) {
        let mut fresh: HashSet<String> = scan_path();
        fresh.extend(INTERCEPTED.iter().map(|s| s.to_string()));
        fresh.extend(fetch_builtins(shell));
        fresh.extend(fetch_aliases_and_functions(shell));
        let mut w = self.write();
        *w = fresh;
    }

    pub fn contains(&self, token: &str) -> bool {
        self.read().contains(token)
    }

    /// Command names beginning with `prefix`, case-insensitively (for tab
    /// completion). Unsorted.
    pub fn matching(&self, prefix: &str) -> Vec<String> {
        let lp = prefix.to_lowercase();
        self.read()
            .iter()
            .filter(|n| n.to_lowercase().starts_with(&lp))
            .cloned()
            .collect()
    }

    /// Command names fuzzily matching `query` (subsequence), ranked best-first.
    /// Used as a fallback when there are no prefix matches.
    pub fn fuzzy(&self, query: &str) -> Vec<String> {
        let all: Vec<String> = self.read().iter().cloned().collect();
        crate::fuzzy::rank(all, query)
    }

    /// The closest known command to `token` within `max_dist` edits, if any (for
    /// "did you mean" spelling correction). `None` when `token` is already a known
    /// command or nothing is close enough.
    pub fn correction(&self, token: &str, max_dist: usize) -> Option<String> {
        let guard = self.read();
        crate::fuzzy::correction(token, guard.iter().map(String::as_str), max_dist)
    }

    /// Insert a set of command names (used by tests and seeding).
    pub fn insert_all(&self, items: &[&str]) {
        let mut w = self.write();
        for i in items {
            w.insert((*i).to_string());
        }
    }

    #[allow(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.read().len()
    }
}

/// Return a conservative local typo cue without changing the route.
///
/// The cue is eligible only for an unforced, unknown natural-language route,
/// uses edit distance one, and rejects common prose verbs. Longer phrases need
/// command-shaped argument evidence so ordinary requests are not peppered with
/// executable-name suggestions. Callers are responsible for rate limiting the
/// presentation; this pure function performs no I/O and no network activity.
pub fn typo_assistance(line: &str, cache: &CommandCache) -> Option<TypoAssistance> {
    let route = route(line, cache);
    if route.kind != RouteKind::NaturalLanguage || route.reason != RouteReason::UnknownInput {
        return None;
    }

    let head = route.head.as_deref()?;
    if !(3..=32).contains(&head.chars().count())
        || !head
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "._+-".contains(character))
        || common_natural_language_head(head)
    {
        return None;
    }

    let words: Vec<&str> = route.normalized.split_whitespace().collect();
    if words.len() > 2
        && !words
            .iter()
            .skip(1)
            .any(|word| command_argument_evidence(word))
    {
        return None;
    }

    let candidate = cache.correction(head, 1)?;
    let edit_distance = crate::fuzzy::edit_distance(head, &candidate);
    Some(TypoAssistance {
        schema_version: TYPO_ASSISTANCE_SCHEMA_VERSION,
        original: bounded(head, ROUTE_DIAGNOSTIC_HEAD_CHARS),
        candidate: bounded(&candidate, ROUTE_DIAGNOSTIC_HEAD_CHARS),
        edit_distance,
        executes_automatically: false,
    })
}

fn command_argument_evidence(word: &str) -> bool {
    word.starts_with('-')
        || word.starts_with('/')
        || word.starts_with("./")
        || word.starts_with("../")
        || word.starts_with("~/")
        || word.contains('=')
}

fn common_natural_language_head(head: &str) -> bool {
    matches!(
        head.to_ascii_lowercase().as_str(),
        "add"
            | "analyze"
            | "answer"
            | "are"
            | "build"
            | "can"
            | "change"
            | "check"
            | "compare"
            | "could"
            | "create"
            | "debug"
            | "describe"
            | "design"
            | "did"
            | "do"
            | "does"
            | "edit"
            | "explain"
            | "fix"
            | "generate"
            | "give"
            | "help"
            | "hello"
            | "hi"
            | "how"
            | "implement"
            | "is"
            | "list"
            | "make"
            | "plan"
            | "please"
            | "read"
            | "review"
            | "should"
            | "show"
            | "summarize"
            | "tell"
            | "thank"
            | "thanks"
            | "update"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "whom"
            | "whose"
            | "why"
            | "will"
            | "would"
            | "write"
    )
}

/// Return a delegated shell line only when it can be proven without loading
/// user configuration, providers, plugins, MCP servers, or the managed backend.
///
/// `None` means "use the normal path", not "this is natural language".
pub fn fast_shell_line(line: &str) -> Option<String> {
    let trimmed = line.trim();
    // `/name` is also AIShe's custom-command namespace. Loading the command
    // registry is intentionally outside this admission path, so only bypass
    // configuration for a leading slash when it names a real executable
    // absolute path. Unknown `/name` input must reach `one_shot`, which resolves
    // user/project commands before ordinary shell dispatch.
    if trimmed.starts_with('/') {
        let tokens = tokenize(trimmed);
        let EffectiveHead::Token(head) = effective_command_token(&tokens) else {
            return None;
        };
        let executable = std::fs::metadata(&head)
            .is_ok_and(|metadata| !metadata.is_dir() && metadata.permissions().mode() & 0o111 != 0);
        if !executable {
            return None;
        }
    }
    let cache = CommandCache::new();
    cache.seed_for_line(trimmed);
    match route(trimmed, &cache).into_dispatch() {
        Dispatch::Shell(command) => Some(command),
        Dispatch::NaturalLanguage(_) | Dispatch::Builtin(_) => None,
    }
}

/// Classify one input line against the cache and preserve the evidence used.
///
/// This is the canonical Rust routing contract. It performs no configuration,
/// provider, network, or managed-backend work.
pub fn route(line: &str, cache: &CommandCache) -> RouteDecision {
    let trimmed = line.trim();

    if trimmed.is_empty() {
        return decision(
            RouteKind::NaturalLanguage,
            String::new(),
            RouteReason::EmptyInput,
            RouteSource::Rust,
            cache,
            Dispatch::NaturalLanguage(String::new()),
        );
    }

    // 1. Forced LLM: a leading `?` or `#` sends the line to the AI even if it
    //    starts with a real command (e.g. `? who was the first man on the moon`).
    if let Some(rest) = trimmed
        .strip_prefix('?')
        .or_else(|| trimmed.strip_prefix('#'))
    {
        let normalized = rest.trim().to_string();
        return decision(
            RouteKind::NaturalLanguage,
            normalized.clone(),
            RouteReason::ForcedAgent,
            RouteSource::Explicit,
            cache,
            Dispatch::NaturalLanguage(normalized),
        );
    }
    // 2. Forced shell (safety-exempt).
    if let Some(rest) = trimmed.strip_prefix('!') {
        let normalized = rest.trim().to_string();
        return decision(
            RouteKind::Shell,
            normalized.clone(),
            RouteReason::ForcedShell,
            RouteSource::Explicit,
            cache,
            Dispatch::Shell(normalized),
        );
    }

    // 2b. Built-in slash commands are reserved by the command-surface
    //     registry. Tombstones intercept too, so removed commands produce local
    //     migration guidance rather than becoming model input. Unknown `/name`
    //     remains available to custom commands/MCP prompts, and an executable
    //     absolute path such as `/usr/bin/env` remains a shell line.
    if let Some(parsed) = command_surface::parse_slash(trimmed) {
        if let Some(spec) = parsed.spec {
            let mut toks = vec!["aishe".to_string(), spec.id.to_string()];
            toks.extend(parsed.args.into_iter().map(str::to_string));
            return decision(
                RouteKind::Builtin,
                trimmed.to_string(),
                RouteReason::SlashCommand,
                RouteSource::Rust,
                cache,
                Dispatch::Builtin(toks),
            );
        }
    }

    let tokens = tokenize(trimmed);
    let first = tokens.first().map(|s| s.as_str()).unwrap_or("");

    // 3. Intercepted builtins.
    if INTERCEPTED.contains(&first) {
        return decision(
            RouteKind::Builtin,
            trimmed.to_string(),
            RouteReason::InterceptedBuiltin,
            RouteSource::Rust,
            cache,
            Dispatch::Builtin(tokens),
        );
    }

    // 4. Shell-syntax signals.
    if starts_with_shell_syntax(trimmed) {
        return shell_decision(trimmed, RouteReason::ShellSyntax, cache);
    }

    // 4b. Function definitions (`name() { … }`, `function name { … }`) — route
    //     to shell before the operator/cache checks (the body may contain `;`).
    if function_def_name(trimmed).is_some() {
        return shell_decision(trimmed, RouteReason::FunctionDefinition, cache);
    }

    // 4c. Shell control structures (`for`/`while`/`if`/`case`/…, `[[`, `((`,
    //     `{`) — route to shell so loops/conditionals can be typed and run.
    if is_shell_construct_head(trimmed) {
        return shell_decision(trimmed, RouteReason::ControlStructure, cache);
    }

    // Env assignments: `FOO=bar cmd`. A pure assignment line is shell.
    let effective_first = effective_command_token(&tokens);
    if let EffectiveHead::Assignment = effective_first {
        return shell_decision(trimmed, RouteReason::Assignment, cache);
    }

    // Assignment at the head of the line (`v='a b'`, `x=$(cmd)`, `arr=(a b c)`,
    // `m[k]=v`), possibly followed by `; cmd …` — route the whole line to shell.
    if is_assignment_head(trimmed) {
        return shell_decision(trimmed, RouteReason::Assignment, cache);
    }

    // A small, conservative full-buffer grammar resolves command-name
    // collisions such as macOS `/usr/bin/what` and the ubiquitous `who`.
    // Shell operators/expansions and forced-shell input won above, so a phrase
    // like `what is the capital of France` reaches the AI while `what app.o`,
    // `who -u`, and `find . -name foo?` remain real commands.
    if looks_like_question(trimmed) {
        let normalized = trimmed.to_string();
        return decision(
            RouteKind::NaturalLanguage,
            normalized.clone(),
            RouteReason::QuestionGrammar,
            RouteSource::Rust,
            cache,
            Dispatch::NaturalLanguage(normalized),
        );
    }

    // 5. Pipelines / compound lines, split quote-aware on `|`/`;`/`&&`/`||`. It's
    //    shell if every segment's head is a known command or a shell reserved
    //    word (so `grep -E 'a|b'` stays one segment, and `x=1; while …; done`
    //    routes to shell). Otherwise → natural language.
    let segments = split_top_level(trimmed);
    if segments.len() > 1 {
        let all_shell = segments.iter().all(|seg| {
            if is_assignment_head(seg) {
                return true; // `v='a b'` / `arr=(…)` / `m[k]=v` segment is shell
            }
            match effective_command_token(&tokenize(seg)) {
                EffectiveHead::Token(t) => cache.contains(&t) || is_reserved_word(&t),
                _ => true, // assignment-only or empty segment is fine
            }
        });
        return if all_shell {
            shell_decision(trimmed, RouteReason::CompoundShell, cache)
        } else {
            let normalized = trimmed.to_string();
            decision(
                RouteKind::NaturalLanguage,
                normalized.clone(),
                RouteReason::CompoundUnknown,
                RouteSource::Rust,
                cache,
                Dispatch::NaturalLanguage(normalized),
            )
        };
    }

    // 6. Cache hit on the effective head.
    if let EffectiveHead::Token(tok) = effective_first {
        if cache.contains(&tok) {
            return shell_decision(trimmed, RouteReason::KnownCommand, cache);
        }
    }

    // 7. Else → natural language.
    let normalized = trimmed.to_string();
    decision(
        RouteKind::NaturalLanguage,
        normalized.clone(),
        RouteReason::UnknownInput,
        RouteSource::Rust,
        cache,
        Dispatch::NaturalLanguage(normalized),
    )
}

/// Compatibility adapter for existing execution call sites.
pub fn dispatch(line: &str, cache: &CommandCache) -> Dispatch {
    route(line, cache).into_dispatch()
}

fn shell_decision(normalized: &str, reason: RouteReason, cache: &CommandCache) -> RouteDecision {
    let normalized = normalized.to_string();
    decision(
        RouteKind::Shell,
        normalized.clone(),
        reason,
        RouteSource::Rust,
        cache,
        Dispatch::Shell(normalized),
    )
}

fn decision(
    kind: RouteKind,
    normalized: String,
    reason: RouteReason,
    source: RouteSource,
    cache: &CommandCache,
    dispatch: Dispatch,
) -> RouteDecision {
    let head = match effective_command_token(&tokenize(&normalized)) {
        EffectiveHead::Token(head) => Some(head),
        EffectiveHead::Assignment | EffectiveHead::None => None,
    };
    let known_command = head.as_deref().is_some_and(|head| cache.contains(head));
    let ambiguous = known_command
        && (reason == RouteReason::QuestionGrammar
            || looks_like_known_command_collision(&normalized));
    RouteDecision {
        kind,
        normalized,
        reason,
        head,
        known_command,
        ambiguous,
        source,
        dispatch,
    }
}

/// Mark only known command-name phrases with local natural-language evidence.
/// This hint never changes the route or grants authority.
fn looks_like_known_command_collision(line: &str) -> bool {
    if line.chars().any(|character| {
        matches!(
            character,
            '|' | ';' | '&' | '<' | '>' | '$' | '`' | '(' | ')' | '{' | '}'
        )
    }) {
        return false;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some(first) = words.first() else {
        return false;
    };
    let collision_head = matches!(
        first.to_ascii_lowercase().as_str(),
        "what"
            | "where"
            | "who"
            | "time"
            | "test"
            | "find"
            | "install"
            | "open"
            | "say"
            | "yes"
            | "false"
            | "true"
            | "read"
            | "type"
            | "source"
            | "history"
    );
    collision_head && words.len() >= 3 && !words.iter().skip(1).any(|word| word.starts_with('-'))
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

/// True if `seg` starts with a shell variable assignment, e.g. `v=1`,
/// `v='a b'`, `x=$(cmd args)`, `arr=(a b c)`, `path+=(/x)`, or `m[k]=v`. The
/// whitespace-naive tokenizer splits a quoted/substituted/parenthesized value, so
/// without this the head resolves to a value word and the line misroutes to the
/// LLM. The value (after `=`) may be anything; only the name is validated.
fn is_assignment_head(seg: &str) -> bool {
    let s = seg.trim_start();
    let eq = match s.find('=') {
        Some(i) => i,
        None => return false,
    };
    let name = &s[..eq];
    // No whitespace before `=` (otherwise it's `cmd arg=...`, not an assignment).
    if name.is_empty() || name.contains(char::is_whitespace) {
        return false;
    }
    // Allow `name[key]` (array element) and `name+` (append); validate the base.
    let base = name.split('[').next().unwrap_or(name);
    let base = base.strip_suffix('+').unwrap_or(base);
    !base.is_empty()
        && base
            .chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && base.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn starts_with_shell_syntax(line: &str) -> bool {
    line.starts_with("./")
        || line.starts_with('/')
        || line.starts_with("~/")
        || line.starts_with("$(")
        || line.starts_with('(')
}

/// Conservative question grammar shared by one-shot routing and the zsh
/// integration's route-aware highlighting.
fn looks_like_question(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.starts_with(['?', '#']) {
        return true;
    }
    if trimmed.starts_with('!') {
        return false;
    }
    // These characters are stronger evidence of shell grammar than English.
    if trimmed
        .chars()
        .any(|character| QUESTION_SHELL_EVIDENCE.contains(&character))
    {
        return false;
    }
    let mut words = trimmed.split_whitespace();
    let first = words.next().unwrap_or("").to_ascii_lowercase();
    let second = words
        .next()
        .unwrap_or("")
        .trim_end_matches(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .to_ascii_lowercase();
    if second.is_empty() {
        return false;
    }
    let question_pair = QUESTION_PAIR_RULES
        .iter()
        .any(|rule| rule.first == first && rule.seconds.contains(&second.as_str()));
    if question_pair {
        return true;
    }
    trimmed.ends_with('?') && TRAILING_QUESTION_HEADS.contains(&first.as_str())
}

/// Split a line into top-level segments on **unquoted, unparenthesized** `|`,
/// `||`, `&&`, `;`. Quote/escape-aware (operators inside `'…'`/`"…"` like
/// `grep -E 'a|b'` don't split), paren-depth-aware (so `|` inside `$((7 | 8))`,
/// `$(a; b)`, or `( a | b )` doesn't split), and aware of the `>|` clobber
/// redirect. Empty segments are dropped.
fn split_top_level(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut segs = Vec::new();
    let mut cur = String::new();
    let (mut in_s, mut in_d, mut esc) = (false, false, false);
    let mut depth: i32 = 0; // unquoted parenthesis nesting
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
        // Inside parentheses (command/arith/process substitution), operators are
        // part of the sub-expression: track depth and don't split.
        if c == '(' {
            depth += 1;
            cur.push(c);
            i += 1;
            continue;
        }
        if c == ')' {
            depth -= 1;
            cur.push(c);
            i += 1;
            continue;
        }
        if depth > 0 {
            cur.push(c);
            i += 1;
            continue;
        }
        match c {
            '\\' => {
                cur.push(c);
                esc = true;
                i += 1;
            }
            // `>|` (clobber redirect): the `|` is part of the redirect, not a pipe.
            '|' if cur.trim_end().ends_with('>') => {
                cur.push(c);
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

/// Resolve a bare command without a directory scan. Unlike
/// [`crate::executor::which`], this admission check requires an executable
/// non-directory entry; merely sharing a filename with a non-executable file
/// must not make natural language run as shell.
fn path_executable_exists(name: &str) -> bool {
    if name.is_empty() || name.contains('/') {
        return false;
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        std::fs::metadata(dir.join(name))
            .is_ok_and(|metadata| !metadata.is_dir() && metadata.permissions().mode() & 0o111 != 0)
    })
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
            let message = "aishe: aliases/functions query timed out; continuing";
            eprintln!(
                "{}",
                crate::ui::TerminalCapabilities::detect_stderr()
                    .paint(crate::ui::StyleToken::Muted, message)
            );
            HashSet::new()
        }
    }
}

/// Ceiling on how much probe stdout we keep. The builtin/alias listings are a
/// few KiB; anything past this is drained and discarded so the child still
/// reaches EOF instead of blocking on a full pipe.
const PROBE_MAX_OUTPUT_BYTES: u64 = 1024 * 1024;

/// How long we wait for the drainer thread after the shell itself has exited.
/// A forked grandchild can hold the write end open; startup must not hang on it.
const PROBE_DRAIN_GRACE: Duration = Duration::from_secs(1);

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

    // Drain stdout concurrently. Polling `try_wait()` and only calling
    // `wait_with_output()` after exit deadlocks: the OS pipe buffer is ~64 KiB,
    // so a plugin-heavy `.zshrc` that prints more than that blocks in write(2)
    // forever, the child never exits, and the timeout kills a probe that was
    // working fine (leaving the shell with no aliases/builtins).
    let stdout = match child.stdout.take() {
        Some(s) => s,
        // Can't happen with `Stdio::piped()`, but never leak the child if it does.
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut stdout = stdout;
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(PROBE_MAX_OUTPUT_BYTES)
            .read_to_end(&mut buf);
        // Keep draining past the cap (discarding) so the child still sees its
        // writes accepted and can exit.
        let _ = std::io::copy(&mut stdout, &mut std::io::sink());
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });

    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return rx.recv_timeout(PROBE_DRAIN_GRACE).ok(),
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

    #[test]
    fn declarative_question_grammar_is_unique_and_nonempty() {
        let mut pairs = HashSet::new();
        for rule in QUESTION_PAIR_RULES {
            assert!(!rule.first.is_empty());
            assert!(!rule.seconds.is_empty());
            for second in rule.seconds {
                assert!(
                    pairs.insert((rule.first, *second)),
                    "duplicate question pair {}:{second}",
                    rule.first
                );
            }
        }
        let mut trailing = HashSet::new();
        assert!(TRAILING_QUESTION_HEADS
            .iter()
            .all(|head| !head.is_empty() && trailing.insert(*head)));
        assert!(!QUESTION_SHELL_EVIDENCE.is_empty());
    }

    #[test]
    fn run_with_timeout_drains_large_output() {
        // ~164 KiB: more than an OS pipe buffer holds. Without a concurrent
        // drainer the child blocks in write(2), `try_wait()` never reports exit,
        // and the timeout kills a probe that had already produced its answer.
        let script = "i=0; while [ $i -lt 4000 ]; do \
                      echo 0123456789012345678901234567890123456789; i=$((i+1)); done";
        let out = run_with_timeout(
            Path::new("/bin/sh"),
            &["-c", script],
            Duration::from_secs(20),
        )
        .expect("large output must not be mistaken for a hang");
        assert_eq!(out.lines().count(), 4000);
        assert!(out.len() > 160_000, "got {} bytes", out.len());
    }

    #[test]
    fn run_with_timeout_still_kills_a_hang() {
        // The drainer must not defeat the timeout: a sleeping child is still
        // killed and reported as a failure.
        let start = std::time::Instant::now();
        let out = run_with_timeout(
            Path::new("/bin/sh"),
            &["-c", "sleep 30"],
            Duration::from_millis(200),
        );
        assert!(out.is_none());
        assert!(start.elapsed() < Duration::from_secs(5));
    }

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
    fn every_route_rule_has_stable_structured_evidence() {
        let cache = cache_with(&["echo", "grep", "install", "what"]);
        let cases = [
            ("", RouteKind::NaturalLanguage, RouteReason::EmptyInput),
            (
                "?echo hello",
                RouteKind::NaturalLanguage,
                RouteReason::ForcedAgent,
            ),
            ("!unknown", RouteKind::Shell, RouteReason::ForcedShell),
            ("/help", RouteKind::Builtin, RouteReason::SlashCommand),
            (
                "cd /tmp",
                RouteKind::Builtin,
                RouteReason::InterceptedBuiltin,
            ),
            ("./run.sh", RouteKind::Shell, RouteReason::ShellSyntax),
            (
                "greet() { echo hi; }",
                RouteKind::Shell,
                RouteReason::FunctionDefinition,
            ),
            (
                "while true; do echo hi; done",
                RouteKind::Shell,
                RouteReason::ControlStructure,
            ),
            ("NAME=value", RouteKind::Shell, RouteReason::Assignment),
            (
                "what is this",
                RouteKind::NaturalLanguage,
                RouteReason::QuestionGrammar,
            ),
            (
                "echo hi | grep h",
                RouteKind::Shell,
                RouteReason::CompoundShell,
            ),
            (
                "echo hi | missing",
                RouteKind::NaturalLanguage,
                RouteReason::CompoundUnknown,
            ),
            (
                "install kubectl please",
                RouteKind::Shell,
                RouteReason::KnownCommand,
            ),
            (
                "explain this repository",
                RouteKind::NaturalLanguage,
                RouteReason::UnknownInput,
            ),
        ];

        for (input, expected_kind, expected_reason) in cases {
            let actual = route(input, &cache);
            assert_eq!(actual.kind, expected_kind, "kind for {input:?}");
            assert_eq!(actual.reason, expected_reason, "reason for {input:?}");
        }

        let collision = route("install kubectl please", &cache);
        assert_eq!(collision.head.as_deref(), Some("install"));
        assert!(collision.known_command);
        assert!(collision.ambiguous);
    }

    #[test]
    fn sigils_strip_exactly_one_prefix_and_outer_whitespace() {
        let cache = cache_with(&["install", "echo"]);
        let bodies = [
            "plain words",
            "install kubectl please",
            "?nested question marker",
            "!nested shell marker",
            "# nested compatibility marker",
            "echo \u{1f642}",
        ];
        for body in bodies {
            for prefix in ['?', '#'] {
                let input = format!("  {prefix}  {body}  ");
                let actual = route(&input, &cache);
                assert_eq!(actual.kind, RouteKind::NaturalLanguage);
                assert_eq!(actual.reason, RouteReason::ForcedAgent);
                assert_eq!(actual.normalized, body);
                assert_eq!(actual.source, RouteSource::Explicit);
            }
            let input = format!("  !  {body}  ");
            let actual = route(&input, &cache);
            assert_eq!(actual.kind, RouteKind::Shell);
            assert_eq!(actual.reason, RouteReason::ForcedShell);
            assert_eq!(actual.normalized, body);
            assert_eq!(actual.source, RouteSource::Explicit);
        }
    }

    #[test]
    fn diagnostics_are_bounded_and_control_safe() {
        let cache = cache_with(&[]);
        let input = format!("?\u{1b}[31m{}\n", "x".repeat(2_000));
        let actual = route(&input, &cache);
        let diagnostic = actual.diagnostic();
        assert!(diagnostic.normalized.chars().count() <= ROUTE_DIAGNOSTIC_INPUT_CHARS + 1);
        let debug = format!("{actual:?}");
        assert!(!debug.contains('\u{1b}'));
        assert!(!debug.contains('\n'));
        assert!(debug.len() < 1_000);

        let json = serde_json::to_string(&diagnostic).unwrap();
        assert!(!json.contains('\u{1b}'));
        assert!(json.contains(r#""schema_version":1"#));
    }

    #[test]
    fn route_compatibility_adapter_preserves_payloads() {
        let cache = cache_with(&["echo"]);
        for input in ["?echo hello", "!echo hello", "/help models", "echo hello"] {
            assert_eq!(
                dispatch(input, &cache),
                route(input, &cache).into_dispatch()
            );
        }
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
        // `aishe …` is no longer intercepted wholesale: with `aishe` on PATH it
        // runs the real binary, so `aishe -c 'aishe doctor'` behaves like typing
        // it in any shell instead of hitting a slash handler table.
        let with_aishe = cache_with(&["aishe"]);
        assert!(matches!(
            dispatch("aishe help", &with_aishe),
            Dispatch::Shell(_)
        ));
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
    fn assignment_head_with_quoted_or_subst_value_is_shell() {
        let c = cache_with(&["echo", "typeset"]);
        // Quoted/substituted values contain spaces the tokenizer would split.
        assert!(matches!(
            dispatch("v='a b'; echo \"[$v]\"", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(
            dispatch("x=$(echo dyn); echo $x", &c),
            Dispatch::Shell(_)
        ));
        // Array-element assignment.
        assert!(matches!(
            dispatch("typeset -A m; m[k]=v; echo $m[k]", &c),
            Dispatch::Shell(_)
        ));
        // A bare assignment line on its own.
        assert!(matches!(dispatch("v='a b'", &c), Dispatch::Shell(_)));
        // Still NL when the head is an unknown word, not an assignment.
        assert!(matches!(
            dispatch("please do a thing", &c),
            Dispatch::NaturalLanguage(_)
        ));
    }

    #[test]
    fn operators_inside_parens_do_not_split() {
        let c = cache_with(&["echo"]);
        // `|` inside `$(( … ))` is arithmetic OR, not a pipe.
        assert!(matches!(
            dispatch("echo $((7 | 8))", &c),
            Dispatch::Shell(_)
        ));
        assert!(matches!(
            dispatch("echo $(printf a; printf b)", &c),
            Dispatch::Shell(_)
        ));
        // `>|` clobber redirect: the `|` is part of the redirect.
        assert!(matches!(
            dispatch("echo hi >| /dev/null; echo $?", &c),
            Dispatch::Shell(_)
        ));
    }

    #[test]
    fn split_top_level_respects_parens_and_clobber() {
        assert_eq!(split_top_level("echo $((7 | 8))"), vec!["echo $((7 | 8))"]);
        assert_eq!(
            split_top_level("a >| b; c"),
            vec!["a >| b".to_string(), "c".to_string()]
        );
        // a real pipe still splits.
        assert_eq!(
            split_top_level("ls | grep x"),
            vec!["ls".to_string(), "grep x".to_string()]
        );
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
    fn fast_shell_line_is_confident_only_for_shell() {
        assert_eq!(
            fast_shell_line("printf '%s\\n' ready").as_deref(),
            Some("printf '%s\\n' ready")
        );
        assert_eq!(
            fast_shell_line("!unknown-but-forced").as_deref(),
            Some("unknown-but-forced")
        );
        assert!(fast_shell_line("cd /tmp").is_none());
        assert!(fast_shell_line("please explain this directory").is_none());
        assert!(fast_shell_line("/echo-args hello").is_none());
        assert_eq!(
            fast_shell_line("/bin/sh -c 'exit 0'").as_deref(),
            Some("/bin/sh -c 'exit 0'")
        );
    }

    #[test]
    fn full_buffer_questions_beat_command_name_collisions() {
        let c = cache_with(&["what", "where", "who", "find"]);
        assert!(matches!(
            dispatch("what is the capital of France", &c),
            Dispatch::NaturalLanguage(_)
        ));
        assert!(matches!(
            dispatch("where is the ssh config", &c),
            Dispatch::NaturalLanguage(_)
        ));
        assert!(matches!(
            dispatch("who am i logged in as", &c),
            Dispatch::NaturalLanguage(_)
        ));
        assert!(matches!(dispatch("what app.o", &c), Dispatch::Shell(_)));
        assert!(matches!(dispatch("who -u", &c), Dispatch::Shell(_)));
        assert!(matches!(
            dispatch("find . -name foo?", &c),
            Dispatch::Shell(_)
        ));
        assert!(fast_shell_line("what is the capital of France").is_none());
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
    fn every_registered_slash_alias_routes_to_its_stable_identity() {
        let c = cache_with(&[]);
        for spec in crate::command_surface::COMMANDS {
            for alias in spec.slash_aliases {
                assert_eq!(
                    dispatch(&format!("/{alias} sample"), &c),
                    Dispatch::Builtin(vec!["aishe".into(), spec.id.into(), "sample".into()]),
                    "failed slash alias /{alias}"
                );
            }
        }
    }

    #[test]
    fn slash_paths_and_unregistered_names_keep_non_builtin_routing() {
        let c = cache_with(&[]);
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
