//! Provider abstraction: a thin, synchronous HTTP layer over the Anthropic
//! Messages API, OpenAI's Responses API, and OpenAI-compatible Chat
//! Completions APIs.
//!
//! We deliberately own this layer (no vendor SDK crates) to keep the binary
//! small and the request/response shapes fully under our control.

use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;

pub mod anthropic;
pub mod fake;
pub mod fallback;
pub mod openai_compat;

/// A single message in a conversation, in our canonical (provider-neutral) form.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", content = "data", rename_all = "snake_case")]
pub enum Msg {
    /// A user turn (plain text).
    User(String),
    /// An assistant turn, possibly carrying text and/or tool calls.
    Assistant(AssistantMsg),
    /// The result of executing a tool call, fed back to the model.
    ToolResult { call_id: String, content: String },
    /// Opaque output items that must be replayed to the provider on a tool-loop
    /// continuation. OpenAI reasoning models require their reasoning items to
    /// accompany the function-call output on the next Responses API request.
    ProviderItems {
        items: Vec<Value>,
        /// Canonical equivalent used if a fallback switches providers in the
        /// middle of the tool loop.
        assistant: AssistantMsg,
    },
}

/// An assistant message: optional prose plus zero or more tool calls.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AssistantMsg {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

/// A request from the model to invoke a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Provider-native output items needed to continue this tool turn. Empty
    /// for providers whose canonical assistant/tool messages are sufficient.
    pub provider_items: Vec<Value>,
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

/// Stable classification used by Setup, Doctor, support bundles, and
/// user-facing recovery messages. It is derived from the provider response and
/// never contains credentials or request bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    MissingCredential,
    InvalidCredential,
    Permission,
    ModelNotFound,
    UnsupportedParameter,
    UnsupportedTools,
    UnsupportedFormat,
    RateLimited,
    Quota,
    Timeout,
    Network,
    Server,
    MalformedResponse,
    Unknown,
}

impl ProviderError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::Http(message) => {
                let lower = message.to_ascii_lowercase();
                if lower.contains("timed out") || lower.contains("timeout") {
                    ErrorKind::Timeout
                } else {
                    ErrorKind::Network
                }
            }
            Self::Parse(_) => ErrorKind::MalformedResponse,
            Self::Api { status, message } => classify_api_error(*status, message),
        }
    }
}

/// Redacted, deterministic recovery text for an end-user provider failure.
/// Keeping this mapping next to `ErrorKind` prevents setup, shell modes, and
/// future front ends from offering contradictory next actions.
pub fn actionable_error(error: &ProviderError) -> String {
    let base = crate::redact::redact(&error.to_string());
    let next = match error.kind() {
        ErrorKind::MissingCredential => {
            "Set the named API-key environment variable, then run `aishe doctor --live`."
        }
        ErrorKind::InvalidCredential => {
            "Check the configured API-key environment variable, then run `aishe doctor --live`."
        }
        ErrorKind::Permission => {
            "Verify that this API project can access the selected model and endpoint."
        }
        ErrorKind::ModelNotFound => {
            "Run `aishe models --refresh`, then select an available model with `aishe model MODEL`."
        }
        ErrorKind::UnsupportedTools => {
            "Run `aishe settings`; for GPT-5.6 reasoning plus tools choose the Responses transport."
        }
        ErrorKind::UnsupportedParameter | ErrorKind::UnsupportedFormat => {
            "Run `aishe doctor --live` to verify this model/transport combination, then open `aishe settings`."
        }
        ErrorKind::RateLimited => "The retry budget was exhausted; wait briefly and retry.",
        ErrorKind::Quota => "Check provider billing or quota before retrying.",
        ErrorKind::Timeout | ErrorKind::Network => {
            "Check connectivity and the endpoint with `aishe doctor --probe`."
        }
        ErrorKind::Server => "The provider returned a server error after retries; retry later.",
        ErrorKind::MalformedResponse => {
            "Run `aishe doctor --live`; the endpoint returned an incompatible response shape."
        }
        ErrorKind::Unknown => "Run `aishe doctor --live` for a classified compatibility report.",
    };
    format!("{base}\nNext: {next}")
}

pub fn classify_api_error(status: u16, message: &str) -> ErrorKind {
    let lower = message.to_ascii_lowercase();
    if lower.contains("api key not found")
        || lower.contains("key is not set")
        || lower.contains("missing api key")
    {
        return ErrorKind::MissingCredential;
    }
    match status {
        401 => return ErrorKind::InvalidCredential,
        403 => return ErrorKind::Permission,
        408 => return ErrorKind::Timeout,
        429 => return ErrorKind::RateLimited,
        500..=599 => return ErrorKind::Server,
        _ => {}
    }
    if lower.contains("quota")
        || lower.contains("billing")
        || lower.contains("insufficient_quota")
        || lower.contains("credit balance")
    {
        ErrorKind::Quota
    } else if lower.contains("model")
        && (lower.contains("not found")
            || lower.contains("does not exist")
            || lower.contains("not have access"))
    {
        ErrorKind::ModelNotFound
    } else if lower.contains("tool")
        && (lower.contains("not supported")
            || lower.contains("unsupported")
            || lower.contains("cannot use"))
    {
        ErrorKind::UnsupportedTools
    } else if lower.contains("response_format")
        || lower.contains("text.format")
        || lower.contains("json_schema")
        || (lower.contains("structured") && lower.contains("not supported"))
    {
        ErrorKind::UnsupportedFormat
    } else if lower.contains("unsupported parameter")
        || lower.contains("unrecognized request argument")
        || lower.contains("unknown parameter")
        || lower.contains("invalid parameter")
    {
        ErrorKind::UnsupportedParameter
    } else {
        ErrorKind::Unknown
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;

    #[test]
    fn actionable_messages_are_classified_and_redacted() {
        let error = ProviderError::Api {
            status: 400,
            message: "Function tools are not supported with reasoning_effort".into(),
        };
        let text = actionable_error(&error);
        assert!(text.contains("Responses transport"));
        assert!(text.contains("Next:"));

        let auth = ProviderError::Api {
            status: 401,
            message: "Bearer sk-proj-abcdefghijklmnopqrstuvwxyz1234567890 rejected".into(),
        };
        let text = actionable_error(&auth);
        assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(text.contains("API-key environment variable"));
    }
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

    /// Embed each text into a dense vector using the named embedding `model`,
    /// returning one vector per input (same order). Used by semantic history
    /// search. The default returns an error: only providers with an embeddings
    /// endpoint (OpenAI-compatible `/v1/embeddings`) support it.
    fn embed(&self, _texts: &[String], _model: &str) -> Result<Vec<Vec<f32>>, ProviderError> {
        Err(ProviderError::Api {
            status: 0,
            message: "this provider has no embeddings endpoint \
                      (set `embedding_provider` to an OpenAI-compatible block, \
                      e.g. openai or a local Ollama)"
                .into(),
        })
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
        // Fast-fail the TCP connect so an unreachable endpoint doesn't sit on the
        // read timeout, but keep the per-read timeout (not a whole-call deadline)
        // so legitimate slow streams aren't cut.
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
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

/// Ceiling on a single SSE line. Real provider frames are a few KiB; this only
/// bounds memory against a stream that never sends a newline. Oversized lines are
/// split, so the tail of such a frame is not recognized as `data:` and is
/// dropped — a lost token beats an unbounded allocation.
const MAX_SSE_LINE_BYTES: u64 = 1024 * 1024;

/// Read an SSE stream line by line, invoking `on_data` with the payload of each
/// `data:` line (skipping blanks and the `[DONE]` sentinel).
pub(crate) fn read_sse(
    resp: ureq::Response,
    on_data: impl FnMut(&str),
) -> Result<(), ProviderError> {
    read_sse_lines(resp.into_reader(), on_data);
    Ok(())
}

/// The reader half of [`read_sse`], split out so it can be exercised against a
/// plain byte stream instead of a live HTTP response.
///
/// Drains raw bytes and decodes each line lossily rather than using
/// `BufRead::lines()`. `lines()` is UTF-8-only, and one invalid byte — a chunk
/// boundary that splits a multi-byte character, a provider echoing latin-1 in an
/// error frame — yields `Err(InvalidData)`, which the old `let Ok(line) = line
/// else { break }` could not tell apart from end-of-stream: the model's answer
/// was silently cut off mid-sentence with `Ok(())` returned. A decode error is no
/// longer possible here; only a genuine IO error or EOF ends the loop.
fn read_sse_lines<R: std::io::Read>(reader: R, mut on_data: impl FnMut(&str)) {
    use std::io::{BufRead, Read};
    let mut buf = std::io::BufReader::new(reader);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        // The limit is re-armed each iteration, so `Ok(0)` still means genuine EOF.
        match (&mut buf)
            .take(MAX_SSE_LINE_BYTES)
            .read_until(b'\n', &mut raw)
        {
            Ok(0) => return, // EOF: SSE has no guaranteed terminator, this is normal
            Ok(_) => {}
            // A real IO error mid-stream (truncation/connection reset) ends the
            // stream gracefully: any text delivered so far stands, rather than
            // failing the whole turn.
            Err(_) => return,
        }
        let line = String::from_utf8_lossy(&raw);
        // The trailing newline (and any CR) is removed by the payload `trim()`.
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            on_data(data);
        }
    }
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
        // Clamp to [1, 15]s: honor the hint but never retry with zero delay (a
        // `Retry-After: 0` would otherwise hammer a rate-limiting server).
        return Duration::from_secs(secs.clamp(1, 15));
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
    if let Some(p) = fake_from_env() {
        return Ok(p);
    }
    // Primary provider (errors if its key is missing — same as before).
    let active = config.active_connection_id().to_string();
    let primary = build_one(config, &active)?;
    let mut chain: Vec<(String, Arc<dyn Provider>)> = vec![(active, primary)];
    // Append any configured fallbacks, skipping the primary, duplicates, and any
    // that can't be built (e.g. a missing API key) so a bad fallback never breaks
    // the primary.
    for name in &config.aishe.provider_fallback {
        if chain.iter().any(|(n, _)| n == name) {
            continue;
        }
        if let Ok(p) = build_one(config, name) {
            chain.push((name.clone(), p));
        }
    }
    let inner: Arc<dyn Provider> = if chain.len() > 1 {
        Arc::new(fallback::FallbackProvider::new(chain))
    } else {
        chain.into_iter().next().unwrap().1
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

/// The deterministic fake provider when `AISHE_FAKE_LLM[_FILE]` is set (no
/// network, no API key); `None` otherwise. Shared by `make` and `embedder`.
fn fake_from_env() -> Option<std::sync::Arc<dyn Provider>> {
    let resp = std::env::var(fake::ENV)
        .ok()
        .or_else(|| std::env::var(fake::ENV_FILE).ok().map(|_| String::new()))?;
    Some(std::sync::Arc::new(fake::FakeProvider::new(resp)))
}

/// Outcome of a provider reachability probe ([`probe`]).
#[derive(Debug)]
pub enum Reach {
    /// The endpoint responded with an HTTP status. Even a 4xx means it's up.
    Up(u16),
    /// Reachable, but the API key was rejected (401/403).
    Unauthorized(u16),
    /// OAuth is injected and refreshed inside the managed OpenCode transport,
    /// so a direct anonymous `/models` probe would be misleading.
    ManagedOAuth(crate::oauth::OAuthProvider),
    /// No HTTP response at all — connection refused, DNS failure, or timeout.
    Down(String),
}

/// A reachability probe result for one chain member.
#[derive(Debug)]
pub struct Probe {
    /// The provider block name (e.g. `anthropic`, `openai`).
    pub name: String,
    /// The base URL probed (for display).
    pub endpoint: String,
    pub reach: Reach,
}

/// Probe a configured provider block by `name`: a short-timeout `GET
/// {base_url}/v1/models` with the block's auth header. Cheap and read-only — it
/// never sends a completion, so it costs no tokens. *Any* HTTP response (even a
/// 4xx) means the endpoint is up; only a transport error means unreachable. A
/// 401/403 is surfaced distinctly so "reachable but key rejected" reads clearly.
/// This makes the offline/fallback story (e.g. a local Ollama) actually testable
/// from `aishe doctor --probe`.
pub fn probe(config: &Config, name: &str) -> Probe {
    let id = config
        .resolve_connection_id(name)
        .unwrap_or_else(|_| config.active_connection_id().to_string());
    let resolved = match crate::connection::resolve_id(config, &id) {
        Ok(resolved) => resolved,
        Err(error) => {
            let endpoint = config
                .connections
                .get(&id)
                .map(|connection| {
                    crate::provider_catalog::normalize_base_url(&connection.settings.base_url)
                })
                .unwrap_or_default();
            return Probe {
                name: id,
                endpoint,
                reach: Reach::Down(crate::redact::redact(&error.to_string())),
            };
        }
    };
    if let crate::connection::ResolvedAuth::OAuth { provider, .. } = resolved.auth {
        return Probe {
            name: id,
            endpoint: crate::provider_catalog::normalize_base_url(&resolved.settings.base_url),
            reach: Reach::ManagedOAuth(provider),
        };
    }
    let is_openai = resolved.provider != "anthropic";
    let base_url = resolved.settings.base_url;
    let auth_header = resolved.api_key.map(|key| {
        if is_openai {
            ("Authorization".to_string(), format!("Bearer {key}"))
        } else {
            ("x-api-key".to_string(), key)
        }
    });
    let base = crate::provider_catalog::normalize_base_url(&base_url);
    let url = format!("{base}/v1/models");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout(Duration::from_secs(6))
        .build();
    let mut req = agent.get(&url);
    if let Some((k, v)) = &auth_header {
        req = req.set(k, v);
    }
    if !is_openai {
        req = req.set("anthropic-version", "2023-06-01");
    }
    let reach = match req.call() {
        Ok(resp) => Reach::Up(resp.status()),
        Err(ureq::Error::Status(code, _)) => {
            if code == 401 || code == 403 {
                Reach::Unauthorized(code)
            } else {
                // Any other status still proves the endpoint answered.
                Reach::Up(code)
            }
        }
        Err(ureq::Error::Transport(t)) => Reach::Down(t.to_string()),
    };
    Probe {
        name: id,
        endpoint: base,
        reach,
    }
}

/// The ordered, de-duplicated provider chain: the active provider followed by any
/// configured fallbacks. Used by the reachability probe and the fallback build.
pub fn chain_names(config: &Config) -> Vec<String> {
    let mut chain = vec![config.active_connection_id().to_string()];
    for n in &config.aishe.provider_fallback {
        if !chain.contains(n) {
            chain.push(n.clone());
        }
    }
    chain
}

/// Build a provider to serve embeddings for semantic history search: the block
/// named by `embedding_provider`, or the active `provider` when that is empty.
/// Honors the fake-provider test hook. Errors if the block is unknown or its API
/// key is missing.
pub fn embedder(config: &Config) -> Result<std::sync::Arc<dyn Provider>> {
    if let Some(p) = fake_from_env() {
        return Ok(p);
    }
    let name = if config.aishe.embedding_provider.trim().is_empty() {
        config.aishe.provider.as_str()
    } else {
        config.aishe.embedding_provider.as_str()
    };
    build_one(config, name)
}

/// Build one provider by name from its configured block. Errors if the name is
/// unknown or the block's API key is missing.
fn build_one(config: &Config, name: &str) -> Result<std::sync::Arc<dyn Provider>> {
    use std::sync::Arc;
    if config.connections.contains_key(name) {
        let resolved = crate::connection::resolve_id(config, name)?;
        if matches!(resolved.auth, crate::connection::ResolvedAuth::OAuth { .. }) {
            anyhow::bail!(
                "connection '{name}' uses OAuth and requires the managed OpenCode backend"
            );
        }
        let key = resolved.api_key.unwrap_or_default();
        let p = resolved.settings;
        return match resolved.provider.as_str() {
            "anthropic" => Ok(Arc::new(anthropic::AnthropicProvider::new(
                p.base_url, key, p.model,
            ))),
            _ => Ok(Arc::new(openai_compat::OpenAiProvider::with_options(
                p.base_url,
                key,
                p.model,
                &p.transport,
                config.active_reasoning_effort(),
            ))),
        };
    }
    match name {
        "anthropic" => {
            let p = &config.providers.anthropic;
            let key = crate::credentials::require(p)?;
            Ok(Arc::new(anthropic::AnthropicProvider::new(
                p.base_url.clone(),
                key,
                p.model.clone(),
            )))
        }
        "openai" => {
            let p = &config.providers.openai;
            let key = crate::credentials::optional(p)?;
            Ok(Arc::new(openai_compat::OpenAiProvider::with_options(
                p.base_url.clone(),
                key,
                p.model.clone(),
                &p.transport,
                config.active_reasoning_effort(),
            )))
        }
        other => anyhow::bail!("unknown provider '{other}' (expected 'anthropic' or 'openai')"),
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
    fn sse_survives_invalid_utf8_mid_stream() {
        // A stray byte between two frames. The old `.lines()` loop treated the
        // decode error as end-of-stream and returned Ok(()), so the answer was
        // truncated mid-sentence with nothing surfaced to the caller.
        let bytes: Vec<u8> = b"data: one\n\xFF\ndata: two\ndata: [DONE]\n".to_vec();
        let mut got = Vec::new();
        read_sse_lines(std::io::Cursor::new(bytes), |d| got.push(d.to_string()));
        assert_eq!(got, vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn sse_handles_crlf_and_unterminated_final_frame() {
        // CRLF framing, and a last frame with no trailing newline (a server that
        // closes the connection right after the final token).
        let bytes: Vec<u8> = b"data: a\r\n\r\ndata: b".to_vec();
        let mut got = Vec::new();
        read_sse_lines(std::io::Cursor::new(bytes), |d| got.push(d.to_string()));
        assert_eq!(got, vec!["a".to_string(), "b".to_string()]);
    }

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
        // Retry-After wins (clamped to [1, 15]s; a 0 never means "retry now").
        assert_eq!(backoff(1, Some(3)), Duration::from_secs(3));
        assert_eq!(backoff(1, Some(999)), Duration::from_secs(15));
        assert_eq!(backoff(1, Some(0)), Duration::from_secs(1));
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
