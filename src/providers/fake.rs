//! Deterministic fake provider for tests.
//!
//! Activated by setting `AISHE_FAKE_LLM` to the raw text the model should
//! "return" (e.g. a structured-output JSON for suggest/auto, or prose). It makes
//! no network calls and needs no API key, so the PTY scenario harness can drive
//! the real front-ends through many model outputs (good, malformed, prose,
//! command) and assert the pipeline handles each gracefully. Not for production
//! use; it is inert unless the env var is set.

use std::sync::Arc;

use super::{Completion, Msg, Provider, ProviderError, ResponseFormat, ToolDef};
use crate::usage::UsageMeter;

/// Environment variable that, when set, swaps in the fake provider returning its
/// value as the model response.
pub const ENV: &str = "AISHE_FAKE_LLM";
/// Alternative: a file whose contents are the response, re-read on every call.
/// Lets a test vary the response with arbitrary bytes (quotes, backticks) without
/// shell-quoting issues. Takes precedence over [`ENV`] when both are set.
pub const ENV_FILE: &str = "AISHE_FAKE_LLM_FILE";
/// Optional `"<input>,<output>"` token counts the fake records into its meter on
/// every call, so usage/cost/session-summary paths are testable without a real
/// provider. Unset (the default) records nothing, preserving prior behavior.
pub const ENV_USAGE: &str = "AISHE_FAKE_USAGE";
/// Optional deterministic delay for timeout-path integration tests. It is
/// ignored unless the fake provider is active and capped to 30 seconds.
pub const ENV_DELAY_MS: &str = "AISHE_FAKE_DELAY_MS";
/// Optional deterministic provider failure for error-contract integration
/// tests. Its value becomes a synthetic status-500 message.
pub const ENV_ERROR: &str = "AISHE_FAKE_ERROR";
/// Optional shell command the fake emits as a single `run_command` tool call on
/// the first agentic turn (then finishes with text), so the yolo loop — and its
/// dry-run overlay — is testable without a real model. Unset = no tool calls.
pub const ENV_TOOL: &str = "AISHE_FAKE_TOOL";

pub struct FakeProvider {
    response: String,
    file: Option<String>,
    meter: Arc<UsageMeter>,
}

impl FakeProvider {
    pub fn new(response: String) -> Self {
        Self {
            response,
            file: std::env::var(ENV_FILE).ok().filter(|s| !s.is_empty()),
            meter: Arc::new(UsageMeter::default()),
        }
    }

    fn body(&self) -> String {
        if let Some(path) = &self.file {
            if let Ok(text) = std::fs::read_to_string(path) {
                return text;
            }
        }
        self.response.clone()
    }

    /// Record the `AISHE_FAKE_USAGE="in,out"` token counts (if set) so usage,
    /// cost, and the session summary can be exercised deterministically. No-op
    /// when the var is unset or malformed.
    fn meter_fake_usage(&self) {
        if let Ok(spec) = std::env::var(ENV_USAGE) {
            if let Some((i, o)) = spec.split_once(',') {
                if let (Ok(i), Ok(o)) = (i.trim().parse::<u64>(), o.trim().parse::<u64>()) {
                    self.meter.record(i, o);
                }
            }
        }
    }

    fn delay_for_test(&self) {
        let millis = std::env::var(ENV_DELAY_MS)
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
            .min(30_000);
        if millis > 0 {
            std::thread::sleep(std::time::Duration::from_millis(millis));
        }
    }

    fn error_for_test(&self) -> Option<ProviderError> {
        std::env::var(ENV_ERROR)
            .ok()
            .filter(|message| !message.is_empty())
            .map(|message| ProviderError::Api {
                status: 500,
                message,
            })
    }
}

impl Provider for FakeProvider {
    fn complete(
        &self,
        _system: &str,
        _messages: &[Msg],
        _format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        self.delay_for_test();
        crate::lean::mark_nl_wire_ready();
        if let Some(error) = self.error_for_test() {
            return Err(error);
        }
        self.meter_fake_usage();
        Ok(self.body())
    }

    fn complete_stream(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
        sink: &mut dyn FnMut(&str),
    ) -> Result<String, ProviderError> {
        let _ = (system, messages, format);
        self.delay_for_test();
        crate::lean::mark_nl_wire_ready();
        if let Some(error) = self.error_for_test() {
            return Err(error);
        }
        self.meter_fake_usage();
        let full = stream_body_for_fake(&self.body());
        let chunk = stream_chunk_size();
        let mut i = 0;
        let mut n = 0u64;
        while i < full.len() {
            let mut end = (i + chunk).min(full.len());
            while end < full.len() && !full.is_char_boundary(end) {
                end += 1;
            }
            if end == i {
                end = full.len();
            }
            sink(&full[i..end]);
            n += 1;
            i = end;
        }
        if let Ok(path) = std::env::var("AISHE_SPY_STREAM_CHUNKS") {
            if !path.is_empty() {
                let _ = std::fs::write(path, n.to_string());
            }
        }
        Ok(full)
    }

    fn complete_with_tools(
        &self,
        _system: &str,
        messages: &[Msg],
        _tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        self.delay_for_test();
        if let Some(error) = self.error_for_test() {
            return Err(error);
        }
        self.meter_fake_usage();
        // Test hook: on the first turn (before any tool result is in the
        // conversation) emit one `run_command` tool call from AISHE_FAKE_TOOL, so
        // the yolo loop actually executes something; later turns finish with text.
        if let Ok(cmd) = std::env::var(ENV_TOOL) {
            let already_ran = messages.iter().any(|m| matches!(m, Msg::ToolResult { .. }));
            if !cmd.is_empty() && !already_ran {
                return Ok(Completion {
                    text: None,
                    tool_calls: vec![super::ToolCall {
                        id: "fake-tool-1".to_string(),
                        name: "run_command".to_string(),
                        arguments: serde_json::json!({"command": cmd, "reason": "fake"}),
                    }],
                    provider_items: Vec::new(),
                });
            }
        }
        // No tool calls: the yolo loop just surfaces the text and finishes.
        Ok(Completion {
            text: Some(self.body()),
            tool_calls: Vec::new(),
            provider_items: Vec::new(),
        })
    }

    fn embed(&self, texts: &[String], _model: &str) -> Result<Vec<Vec<f32>>, ProviderError> {
        Ok(texts.iter().map(|t| fake_embed(t)).collect())
    }

    fn meter(&self) -> Arc<UsageMeter> {
        self.meter.clone()
    }
}

/// Convert a structured fake JSON suggestion into the CMD:/prose stream protocol
/// when present; otherwise return the raw body. Lets lean ask streaming tests
/// exercise `complete_stream` without a network.
fn stream_body_for_fake(body: &str) -> String {
    let trimmed = body.trim();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        let kind = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let explanation = v
            .get("explanation")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if kind == "command" {
            let cmd = v
                .get("command")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .trim();
            if !cmd.is_empty() {
                return format!("CMD: {cmd}\nWHY: {explanation}");
            }
        }
        if kind == "answer" || kind == "command" {
            return explanation;
        }
    }
    body.to_string()
}

fn stream_chunk_size() -> usize {
    std::env::var("AISHE_FAKE_STREAM_CHUNK")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(4)
        .min(64)
}


/// Dimensionality of the fake embedding (small but enough to keep token
/// collisions rare for short commands).
const FAKE_DIM: usize = 256;

/// A deterministic bag-of-words embedding: each whitespace token is hashed (FNV-1a)
/// into one dimension and counted. Commands that share words land near each other,
/// so cosine similarity reflects lexical overlap — enough for tests to assert a
/// meaningful, reproducible ranking without any network or real embedder.
pub(crate) fn fake_embed(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; FAKE_DIM];
    for tok in text.split_whitespace() {
        let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
        for b in tok.to_ascii_lowercase().bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        v[(h as usize) % FAKE_DIM] += 1.0;
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_embed_is_deterministic_and_overlap_sensitive() {
        let a = fake_embed("docker run -v /data prometheus");
        let b = fake_embed("docker run -v /data prometheus");
        assert_eq!(a, b, "same text → same vector");
        let shared = fake_embed("docker prometheus volume");
        let unrelated = fake_embed("git commit message");
        let sim_shared = crate::semhist::cosine(&a, &shared);
        let sim_unrelated = crate::semhist::cosine(&a, &unrelated);
        assert!(
            sim_shared > sim_unrelated,
            "overlapping query should score higher: {sim_shared} vs {sim_unrelated}"
        );
    }

    #[test]
    fn complete_stream_emits_multiple_chunks_for_answer_json() {
        std::env::set_var(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","command":null,"explanation":"hello streaming world"}"#,
        );
        std::env::remove_var("AISHE_FAKE_STREAM_CHUNK");
        let p = FakeProvider::new(std::env::var("AISHE_FAKE_LLM").unwrap());
        let mut chunks = Vec::new();
        let full = p
            .complete_stream("sys", &[], &ResponseFormat::Text, &mut |d| {
                chunks.push(d.to_string());
            })
            .unwrap();
        assert_eq!(full, "hello streaming world");
        assert!(
            chunks.len() > 1,
            "expected chunked sink calls, got {}",
            chunks.len()
        );
        assert_eq!(chunks.concat(), full);
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn complete_stream_converts_command_json_to_cmd_protocol() {
        let p = FakeProvider::new(
            r#"{"type":"command","command":"ls -la","explanation":"list"}"#.into(),
        );
        let mut got = String::new();
        let full = p
            .complete_stream("sys", &[], &ResponseFormat::Text, &mut |d| got.push_str(d))
            .unwrap();
        assert!(full.starts_with("CMD: ls -la"), "{full}");
        assert!(full.contains("WHY: list"), "{full}");
        assert_eq!(got, full);
    }
}
