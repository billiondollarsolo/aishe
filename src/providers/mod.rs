//! Provider abstraction: a thin, synchronous HTTP layer over the Anthropic
//! Messages API and any OpenAI-compatible Chat Completions API.
//!
//! We deliberately own this layer (no vendor SDK crates) to keep the binary
//! small and the request/response shapes fully under our control.

use crate::config::Config;
use anyhow::Result;

pub mod anthropic;
pub mod openai_compat;

/// A single message in a conversation, in our canonical (provider-neutral) form.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A user turn (plain text).
    User(String),
    /// An assistant turn, possibly carrying text and/or tool calls.
    Assistant(AssistantMsg),
    /// The result of executing a tool call, fed back to the model.
    ToolResult { call_id: String, content: String },
}

/// An assistant message: optional prose plus zero or more tool calls.
#[derive(Debug, Clone, Default)]
pub struct AssistantMsg {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// A request from the model to invoke a tool.
#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A tool the model is allowed to call.
#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool's input object.
    pub schema: serde_json::Value,
}

/// The outcome of a `complete_with_tools` call.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// Errors that can arise while talking to a provider.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("network error: {0}")]
    Http(String),
    #[error("API error (status {status}): {message}")]
    Api { status: u16, message: String },
    #[error("failed to parse provider response: {0}")]
    Parse(String),
}

/// The provider interface used by the suggest and yolo modes.
pub trait Provider {
    /// Single-shot completion. `json_mode` requests a JSON object back where the
    /// provider supports it; callers must still parse defensively.
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        json_mode: bool,
    ) -> Result<String, ProviderError>;

    /// Tool-use completion for the agentic (yolo) loop.
    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError>;
}

/// Build the configured provider, reading the API key from the configured env var.
pub fn make(config: &Config) -> Result<Box<dyn Provider>> {
    match config.aishe.provider.as_str() {
        "anthropic" => {
            let p = &config.providers.anthropic;
            let key = read_key(&p.api_key_env)?;
            Ok(Box::new(anthropic::AnthropicProvider::new(
                p.base_url.clone(),
                key,
                p.model.clone(),
            )))
        }
        "openai" => {
            let p = &config.providers.openai;
            let key = read_key(&p.api_key_env)?;
            Ok(Box::new(openai_compat::OpenAiProvider::new(
                p.base_url.clone(),
                key,
                p.model.clone(),
            )))
        }
        other => anyhow::bail!("unknown provider '{other}' (expected 'anthropic' or 'openai')"),
    }
}

fn read_key(env_var: &str) -> Result<String> {
    match std::env::var(env_var) {
        Ok(v) if !v.trim().is_empty() => Ok(v),
        _ => anyhow::bail!(
            "API key not found — is ${env_var} set? \
             Export it, e.g. `export {env_var}=...`"
        ),
    }
}

/// Default per-request timeout for provider HTTP calls.
pub(crate) const HTTP_TIMEOUT_SECS: u64 = 60;
/// Max tokens requested from the model.
pub(crate) const MAX_TOKENS: u32 = 4096;
