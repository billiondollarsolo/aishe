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
}

impl Provider for FakeProvider {
    fn complete(
        &self,
        _system: &str,
        _messages: &[Msg],
        _format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        Ok(self.body())
    }

    fn complete_with_tools(
        &self,
        _system: &str,
        _messages: &[Msg],
        _tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        // No tool calls: the yolo loop just surfaces the text and finishes.
        Ok(Completion {
            text: Some(self.body()),
            tool_calls: Vec::new(),
        })
    }

    fn meter(&self) -> Arc<UsageMeter> {
        self.meter.clone()
    }
}
