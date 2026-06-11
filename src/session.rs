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
//! not the full tool-by-tool transcript of a yolo run, so it stays small. It is
//! capped by an approximate character budget and lives only for the interactive
//! process (never written to disk).

use crate::providers::{AssistantMsg, Msg};

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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
