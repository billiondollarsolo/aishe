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
pub mod fake;
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

/// POST a request for a Server-Sent Events stream, retrying transient failures
/// (429/5xx/connection errors) with backoff. Returns the streaming response for
/// [`read_sse`] to consume.
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
                if status == 401 {
                    return Err(ProviderError::Api {
                        status,
                        message: format!("API key invalid ({})", error_message(resp)),
                    });
                }
                if is_retryable_status(status) && attempt < MAX_RETRIES {
                    let wait = backoff(attempt + 1, retry_after_secs(&resp));
                    attempt += 1;
                    std::thread::sleep(wait);
                    continue;
                }
                return Err(ProviderError::Api {
                    status,
                    message: error_message(resp),
                });
            }
            Err(e) => {
                if attempt < MAX_RETRIES {
                    attempt += 1;
                    std::thread::sleep(backoff(attempt, None));
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
        // A read error mid-stream (truncation/connection reset) ends the stream
        // gracefully: any text delivered so far stands, rather than failing the
        // whole turn. SSE has no guaranteed terminator, so EOF is normal.
        let Ok(line) = line else { break };
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

/// Retry attempts (beyond the first try) for transient HTTP failures.
pub(crate) const MAX_RETRIES: u32 = 3;

/// Whether an HTTP status is transient and worth retrying (rate limit, request
/// timeout, or any 5xx).
pub(crate) fn is_retryable_status(status: u16) -> bool {
    status == 429 || status == 408 || status >= 500
}

/// The `Retry-After` hint in whole seconds, if the response carries one as an
/// integer (the HTTP-date form is ignored).
pub(crate) fn retry_after_secs(resp: &ureq::Response) -> Option<u64> {
    resp.header("retry-after")?.trim().parse::<u64>().ok()
}

/// Backoff before retry `attempt` (1-based): honor a `Retry-After` hint (capped),
/// else exponential 0.5s/1s/2s/4s (capped at 4s) plus a little jitter so
/// concurrent callers don't retry in lockstep.
pub(crate) fn backoff(attempt: u32, retry_after: Option<u64>) -> Duration {
    if let Some(secs) = retry_after {
        return Duration::from_secs(secs.min(15));
    }
    let base_ms = 500u64.saturating_mul(1u64 << (attempt.saturating_sub(1)).min(3));
    Duration::from_millis(base_ms.min(4000) + jitter_ms())
}

/// A small pseudo-random jitter (0-249 ms) derived from the clock, avoiding a
/// `rand` dependency.
fn jitter_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.subsec_nanos() as u64) % 250)
        .unwrap_or(0)
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
    // Test hook: a deterministic fake provider (no network, no API key) when
    // AISHE_FAKE_LLM[_FILE] is set. Inert otherwise.
    let fake_resp = std::env::var(fake::ENV)
        .ok()
        .or_else(|| std::env::var(fake::ENV_FILE).ok().map(|_| String::new()));
    if let Some(resp) = fake_resp {
        return Ok(Arc::new(fake::FakeProvider::new(resp)));
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(408));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(503));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn backoff_grows_and_honors_retry_after() {
        // Retry-After wins (capped at 15s).
        assert_eq!(backoff(1, Some(3)), Duration::from_secs(3));
        assert_eq!(backoff(1, Some(999)), Duration::from_secs(15));
        // Exponential base (0.5s, 1s, 2s, 4s) plus <250ms jitter, capped at 4s+.
        let bare = |a| backoff(a, None).as_millis();
        assert!((500..750).contains(&bare(1)), "{}", bare(1));
        assert!((1000..1250).contains(&bare(2)), "{}", bare(2));
        assert!((2000..2250).contains(&bare(3)), "{}", bare(3));
        assert!((4000..4250).contains(&bare(4)), "{}", bare(4));
        // Capped: never exceeds 4s + jitter.
        assert!(bare(9) < 4250, "{}", bare(9));
    }

    #[test]
    fn usage_parsing_both_shapes() {
        assert_eq!(
            usage_from_value(
                &serde_json::json!({"usage": {"input_tokens": 5, "output_tokens": 7}})
            ),
            (5, 7)
        );
        assert_eq!(
            usage_from_value(
                &serde_json::json!({"usage": {"prompt_tokens": 11, "completion_tokens": 13}})
            ),
            (11, 13)
        );
        // Missing usage -> zeros.
        assert_eq!(usage_from_value(&serde_json::json!({})), (0, 0));
    }
}
