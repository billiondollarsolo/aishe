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

pub struct FakeProvider {
    response: String,
    meter: Arc<UsageMeter>,
}

impl FakeProvider {
    pub fn new(response: String) -> Self {
        Self {
            response,
            meter: Arc::new(UsageMeter::default()),
        }
    }
}

impl Provider for FakeProvider {
    fn complete(
        &self,
        _system: &str,
        _messages: &[Msg],
        _format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        Ok(self.response.clone())
    }

    fn complete_with_tools(
        &self,
        _system: &str,
        _messages: &[Msg],
        _tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        // No tool calls: the yolo loop just surfaces the text and finishes.
        Ok(Completion {
            text: Some(self.response.clone()),
            tool_calls: Vec::new(),
        })
    }

    fn meter(&self) -> Arc<UsageMeter> {
        self.meter.clone()
    }
}
