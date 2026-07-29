//! In-session conversation memory for the interactive REPL.
//!
//! Natural-language turns (suggest / auto / yolo) are otherwise stateless: each
//! request is sent on its own, so follow-ups like "now do the same for the other
//! file" have no idea what "the same" was. A [`Session`] keeps a rolling
//! transcript of recent user requests and assistant replies and primes each new
//! request with it.
//!
//! It is intentionally lightweight: it stores the user's request text and the
//! assistant's reply (a suggested command/answer, or a yolo run's final summary),
//! not the full tool-by-tool transcript of a yolo run, so it stays small, capped
//! by an approximate character budget. The reedline REPL keeps it in memory; the
//! shell-hook front-ends (zsh-PTY / `init zsh`), whose NL calls are separate
//! processes, persist it to a per-session file (`load_persisted`/`save_persisted`)
//! so follow-ups still share context.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::providers::{AssistantMsg, Msg};

/// One persisted conversation turn (role + text) for shell-hook session memory.
#[derive(Serialize, Deserialize)]
struct PersistedTurn {
    role: String,
    content: String,
}

/// Approximate upper bound on transcript size, in characters. Roughly four
/// characters per token, so about 3k tokens of history.
const DEFAULT_MAX_CHARS: usize = 12_000;

/// A rolling conversation transcript for one interactive session.
#[derive(Debug)]
pub struct Session {
    enabled: bool,
    max_chars: usize,
    history: Vec<Msg>,
}

impl Default for Session {
    fn default() -> Self {
        Self::new(true)
    }
}

impl Session {
    /// Create a session. When `enabled` is false it is a no-op (no priming, no
    /// recording), used for one-shot `-c` and shell-hook paths.
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            max_chars: DEFAULT_MAX_CHARS,
            history: Vec::new(),
        }
    }

    /// Prior turns to prime a request with. Empty when memory is off.
    pub fn history(&self) -> Vec<Msg> {
        if self.enabled {
            self.history.clone()
        } else {
            Vec::new()
        }
    }

    /// Record a user request (plain text, no environment context block).
    pub fn record_user(&mut self, text: &str) {
        if !self.enabled || text.trim().is_empty() {
            return;
        }
        self.history.push(Msg::User(text.to_string()));
        self.trim();
    }

    /// Record an assistant reply (a suggested command/answer, or a yolo summary).
    pub fn record_assistant(&mut self, text: &str) {
        if !self.enabled || text.trim().is_empty() {
            return;
        }
        self.history.push(Msg::Assistant(AssistantMsg {
            text: Some(text.to_string()),
            tool_calls: Vec::new(),
        }));
        self.trim();
    }

    /// Forget the conversation so far.
    pub fn clear(&mut self) {
        self.history.clear();
    }

    /// Load a persisted transcript (JSONL of `{"role","content"}` turns) so that
    /// stateless per-call shell-hook invocations (`--suggest-line`/`--auto-line`)
    /// still share conversation memory. Returns an enabled session primed with
    /// the turns; a missing or unreadable file yields an empty (enabled) session.
    pub fn load_persisted(path: &Path) -> Self {
        let mut s = Self::new(true);
        if let Ok(text) = std::fs::read_to_string(path) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Ok(turn) = serde_json::from_str::<PersistedTurn>(line) {
                    match turn.role.as_str() {
                        "user" => s.record_user(&turn.content),
                        "assistant" => s.record_assistant(&turn.content),
                        _ => {}
                    }
                }
            }
        }
        s
    }

    /// Write the (char-budget-trimmed) transcript back as JSONL. Best-effort.
    ///
    /// This is a whole-file rewrite, not an append: the transcript is
    /// char-budget-trimmed (oldest turns dropped) so each save replaces the
    /// entire file rather than adding to it. An `O_APPEND` write is therefore
    /// wrong here, and a plain `fs::write` could leave a truncated/corrupt JSONL
    /// file on a crash. So we write to a temp file in the same directory and
    /// `rename` it over the destination, an atomic swap on POSIX.
    pub fn save_persisted(&self, path: &Path) {
        use std::fmt::Write as _;
        let mut out = String::new();
        for m in &self.history {
            let (role, content) = match m {
                Msg::User(t) => ("user", t.as_str()),
                Msg::Assistant(a) => ("assistant", a.text.as_deref().unwrap_or("")),
                _ => continue,
            };
            if content.trim().is_empty() {
                continue;
            }
            let turn = PersistedTurn {
                role: role.to_string(),
                content: content.to_string(),
            };
            if let Ok(j) = serde_json::to_string(&turn) {
                let _ = writeln!(out, "{j}");
            }
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = crate::config::write_atomic(path, out.as_bytes());
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Number of user turns currently remembered.
    pub fn turns(&self) -> usize {
        self.history
            .iter()
            .filter(|m| matches!(m, Msg::User(_)))
            .count()
    }

    /// Drop the oldest messages until the transcript is under the char budget,
    /// always keeping at least the most recent message.
    fn trim(&mut self) {
        let mut total: usize = self.history.iter().map(msg_len).sum();
        while total > self.max_chars && self.history.len() > 1 {
            let removed = self.history.remove(0);
            total -= msg_len(&removed);
        }
    }
}

fn msg_len(m: &Msg) -> usize {
    match m {
        Msg::User(s) => s.len(),
        Msg::Assistant(a) => a.text.as_deref().map(str::len).unwrap_or(0),
        Msg::ToolResult { content, .. } => content.len(),
        Msg::ProviderItems { items, assistant } => {
            let text_len = assistant.text.as_deref().map_or(0, str::len);
            text_len + serde_json::to_string(items).map_or(0, |json| json.len())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_session_round_trips() {
        let dir = std::env::temp_dir().join(format!("aishe-sess-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mem.jsonl");

        let mut s = Session::new(true);
        s.record_user("install spaceship");
        s.record_assistant("cloned spaceship-prompt; set ZSH_THEME=spaceship");
        s.save_persisted(&path);

        // A fresh process (new Session) sees the prior turns.
        let loaded = Session::load_persisted(&path);
        let h = loaded.history();
        assert_eq!(h.len(), 2);
        assert!(matches!(&h[0], Msg::User(t) if t == "install spaceship"));
        assert!(
            matches!(&h[1], Msg::Assistant(a) if a.text.as_deref() == Some("cloned spaceship-prompt; set ZSH_THEME=spaceship"))
        );
        assert_eq!(loaded.turns(), 1);

        // A missing file is an empty (but enabled) session, not an error.
        let empty = Session::load_persisted(&dir.join("nope.jsonl"));
        assert!(empty.history().is_empty());
        assert!(empty.enabled());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_persisted_round_trips_and_leaves_no_tmp() {
        let dir =
            std::env::temp_dir().join(format!("aishe-sess-atomic-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mem.jsonl");

        let mut s = Session::new(true);
        s.record_user("first request");
        s.record_assistant("first reply");
        s.record_user("second request");
        s.save_persisted(&path);

        // The stored turns survive a reload (whole-file atomic rewrite).
        let loaded = Session::load_persisted(&path);
        let h = loaded.history();
        assert_eq!(h.len(), 3);
        assert!(matches!(&h[0], Msg::User(t) if t == "first request"));
        assert!(matches!(&h[1], Msg::Assistant(a) if a.text.as_deref() == Some("first reply")));
        assert!(matches!(&h[2], Msg::User(t) if t == "second request"));
        assert_eq!(loaded.turns(), 2);

        // No leftover temp file from the atomic write should remain in the dir.
        let leftover = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftover, 0, "a .tmp. file was left behind");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disabled_session_is_noop() {
        let mut s = Session::new(false);
        s.record_user("hello");
        s.record_assistant("hi");
        assert!(s.history().is_empty());
        assert_eq!(s.turns(), 0);
    }

    #[test]
    fn records_and_primes() {
        let mut s = Session::new(true);
        s.record_user("list files");
        s.record_assistant("ls -la");
        let h = s.history();
        assert_eq!(h.len(), 2);
        assert!(matches!(&h[0], Msg::User(t) if t == "list files"));
        assert!(matches!(&h[1], Msg::Assistant(a) if a.text.as_deref() == Some("ls -la")));
        assert_eq!(s.turns(), 1);
    }

    #[test]
    fn blank_text_not_recorded() {
        let mut s = Session::new(true);
        s.record_user("   ");
        s.record_assistant("");
        assert!(s.history().is_empty());
    }

    #[test]
    fn trims_to_budget_keeping_recent() {
        let mut s = Session::new(true);
        s.max_chars = 20;
        s.record_user(&"a".repeat(15));
        s.record_assistant(&"b".repeat(15));
        s.record_user(&"c".repeat(15));
        // Total would be 45 > 20; oldest dropped, most recent kept.
        let h = s.history();
        assert!(h.len() <= 2);
        assert!(matches!(h.last(), Some(Msg::User(t)) if t.starts_with('c')));
    }

    #[test]
    fn clear_empties() {
        let mut s = Session::new(true);
        s.record_user("x");
        s.clear();
        assert!(s.history().is_empty());
    }
}
