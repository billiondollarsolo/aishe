//! Optional heavy specialist backends — never the default lean controller.
//!
//! Design lock (2026-09-09):
//! - Hot path / agent path use in-process Provider HTTP + modes::yolo tools.
//! - OpenCode, Codex, and Claude Code are explicit specialists for hard jobs.
//! - Do not cold-start these from known-command or default NL routing.
//!
//! Wire points (stubs for W1+):
//! - HeavyBackend::OpenCode — legacy backend::supervisor / OpenCode sidecar
//!   via AISHE_LEGACY_OPENCODE=1 or a future /backend opencode slash.
//! - HeavyBackend::Codex — OpenAI Codex / Responses as a named tool or mode.
//! - HeavyBackend::ClaudeCode — Claude Code CLI/session as a named tool.
//!
//! Until a call site opts in, select_default returns None and lean keeps
//! using the warm in-process agent loop.
//!
//! Interactive note: lean `/backend` (and `docs/lean-hotpath.md`) point here.
//! There is **no** auto OpenCode on known-cmd or default NL.

#![allow(dead_code)]

/// Named heavy backends that may be invoked explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeavyBackend {
    OpenCode,
    Codex,
    ClaudeCode,
}

impl HeavyBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode => "opencode",
            Self::Codex => "codex",
            Self::ClaudeCode => "claude-code",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "opencode" | "legacy-opencode" => Some(Self::OpenCode),
            "codex" => Some(Self::Codex),
            "claude" | "claude-code" | "claude_code" => Some(Self::ClaudeCode),
            _ => None,
        }
    }
}

/// Default controller for lean agent turns: always in-process (no heavy sidecar).
pub fn select_default() -> Option<HeavyBackend> {
    None
}

/// True when the process explicitly requested the legacy OpenCode controller.
pub fn legacy_opencode_requested() -> bool {
    // Lean disabled means the legacy OpenCode/PTY path was explicitly selected.
    !crate::lean::enabled()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_in_process() {
        assert_eq!(select_default(), None);
    }

    #[test]
    fn parse_names() {
        assert_eq!(
            HeavyBackend::parse("opencode"),
            Some(HeavyBackend::OpenCode)
        );
        assert_eq!(HeavyBackend::parse("codex"), Some(HeavyBackend::Codex));
        assert_eq!(
            HeavyBackend::parse("claude-code"),
            Some(HeavyBackend::ClaudeCode)
        );
        assert_eq!(HeavyBackend::parse("nope"), None);
    }
}
