//! Provider abstraction: a thin, synchronous HTTP layer over the Anthropic
//! Messages API and any OpenAI-compatible Chat Completions API.
//!
//! We deliberately own this layer (no vendor SDK crates) to keep the binary
//! small and the request/response shapes fully under our control.

use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::config::Config;

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

/// How the model's output should be constrained (best-effort; providers that
/// don't support a level silently fall back, and callers always parse
/// defensively).
#[derive(Clone, Debug)]
pub enum ResponseFormat {
    /// Unconstrained text.
    Text,
    /// Any syntactically valid JSON object (`response_format: json_object`).
    Json,
    /// Strict JSON Schema (`response_format: json_schema`) — guarantees shape on
    /// providers that support it.
    JsonSchema {
        name: String,
        schema: serde_json::Value,
    },
}

/// The provider interface used by the suggest and yolo modes. `Send + Sync` so a
/// single provider can be shared (via `Arc`) with the background ghost-text
/// worker, keeping token usage and the budget unified.
pub trait Provider: Send + Sync {
    /// Single-shot completion constrained to `format` where supported. Callers
    /// must still parse defensively.
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
    ) -> Result<String, ProviderError>;

    /// Streaming completion: invokes `sink` with text deltas as they arrive and
    /// returns the full concatenated text. The default implementation falls back
    /// to a single non-streaming call, so callers work even against a provider or
    /// endpoint without SSE support (the whole answer simply arrives at once).
    fn complete_stream(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
        sink: &mut dyn FnMut(&str),
    ) -> Result<String, ProviderError> {
        let full = self.complete(system, messages, format)?;
        sink(&full);
        Ok(full)
    }

    /// Tool-use completion for the agentic (yolo) loop.
    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError>;

    /// Streaming tool-use completion: invokes `sink` with assistant *text* deltas
    /// as they arrive, accumulates any tool calls, and returns the full
    /// `Completion`. The default falls back to the non-streaming call (emitting
    /// the whole text at once), so providers without streaming tool support still
    /// work.
    fn complete_with_tools_stream(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
        sink: &mut dyn FnMut(&str),
    ) -> Result<Completion, ProviderError> {
        let completion = self.complete_with_tools(system, messages, tools)?;
        if let Some(text) = &completion.text {
            sink(text);
        }
        Ok(completion)
    }

    /// The shared token meter this provider records usage into. Callers read it
    /// for cost display and budget enforcement.
    fn meter(&self) -> std::sync::Arc<crate::usage::UsageMeter>;
}

/// Pull `(input, output)` token counts from a response body, handling both the
/// Anthropic (`usage.input_tokens`/`output_tokens`) and OpenAI
/// (`usage.prompt_tokens`/`completion_tokens`) shapes. Missing → 0.
pub(crate) fn usage_from_value(v: &Value) -> (u64, u64) {
    let u = match v.get("usage") {
        Some(u) => u,
        None => return (0, 0),
    };
    let get = |keys: &[&str]| -> u64 {
        for k in keys {
            if let Some(n) = u.get(*k).and_then(|x| x.as_u64()) {
                return n;
            }
        }
        0
    };
    (
        get(&["input_tokens", "prompt_tokens"]),
        get(&["output_tokens", "completion_tokens"]),
    )
}

/// POST a request for a Server-Sent Events stream, retrying once on 429/5xx or a
/// connection error. Returns the streaming response for [`read_sse`] to consume.
pub(crate) fn stream_post(
    url: &str,
    headers: &[(&str, &str)],
    body: &Value,
) -> Result<ureq::Response, ProviderError> {
    let mut attempt = 0;
    loop {
        // Per-read timeout (not a whole-call deadline) so long streams aren't cut.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();
        let mut req = agent.post(url);
        for (k, v) in headers {
            req = req.set(k, v);
        }
        match req.send_json(body.clone()) {
            Ok(resp) => return Ok(resp),
            Err(ureq::Error::Status(status, resp)) => {
                let message = error_message(resp);
                if status == 401 {
                    return Err(ProviderError::Api {
                        status,
                        message: format!("API key invalid ({message})"),
                    });
                }
                if (status == 429 || status >= 500) && attempt == 0 {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                return Err(ProviderError::Api { status, message });
            }
            Err(e) => {
                if attempt == 0 {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                return Err(ProviderError::Http(e.to_string()));
            }
        }
    }
}

/// Read an SSE stream line by line, invoking `on_data` with the payload of each
/// `data:` line (skipping blanks and the `[DONE]` sentinel).
pub(crate) fn read_sse(
    resp: ureq::Response,
    mut on_data: impl FnMut(&str),
) -> Result<(), ProviderError> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(resp.into_reader());
    for line in reader.lines() {
        let line = line.map_err(|e| ProviderError::Http(e.to_string()))?;
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            on_data(data);
        }
    }
    Ok(())
}

/// Pull a human-readable message out of an error response body.
pub(crate) fn error_message(resp: ureq::Response) -> String {
    match resp.into_json::<Value>() {
        Ok(v) => v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| v.to_string()),
        Err(_) => "unknown error".to_string(),
    }
}

/// Build the configured provider, reading the API key from the configured env var.
/// Returned behind an `Arc` so it can be shared with the ghost-text worker.
pub fn make(config: &Config) -> Result<std::sync::Arc<dyn Provider>> {
    use std::sync::Arc;
    let inner: Arc<dyn Provider> = match config.aishe.provider.as_str() {
        "anthropic" => {
            let p = &config.providers.anthropic;
            let key = read_key(&p.api_key_env)?;
            Arc::new(anthropic::AnthropicProvider::new(
                p.base_url.clone(),
                key,
                p.model.clone(),
            ))
        }
        "openai" => {
            let p = &config.providers.openai;
            let key = read_key(&p.api_key_env)?;
            Arc::new(openai_compat::OpenAiProvider::new(
                p.base_url.clone(),
                key,
                p.model.clone(),
            ))
        }
        other => anyhow::bail!("unknown provider '{other}' (expected 'anthropic' or 'openai')"),
    };
    // Optionally wrap in a response cache so identical suggest-mode repeats are
    // instant and free. Streaming and the tool loop pass straight through.
    if config.aishe.cache && config.aishe.cache_ttl_secs > 0 {
        Ok(Arc::new(crate::cache::CachingProvider::new(
            inner,
            config.aishe.cache_ttl_secs,
            config.active_model().to_string(),
        )))
    } else {
        Ok(inner)
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
