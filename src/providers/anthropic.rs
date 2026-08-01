//! Anthropic Messages API provider (`POST {base_url}/v1/messages`).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};

use super::{
    external_http_agent, read_sse, status_is_accepted, stream_post, usage_from_value, Completion,
    HttpResponse, Msg, Provider, ProviderError, ResponseFormat, ToolCall, ToolDef,
    HTTP_TIMEOUT_SECS, MAX_PROVIDER_BODY_BYTES, MAX_TOKENS,
};
use crate::usage::UsageMeter;

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    model: String,
    meter: Arc<UsageMeter>,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url: crate::provider_catalog::normalize_base_url(&base_url),
            api_key,
            model,
            meter: Arc::new(UsageMeter::default()),
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url)
    }

    /// Translate our canonical messages into Anthropic's `messages[]` shape.
    fn build_messages(messages: &[Msg]) -> Vec<Value> {
        let mut out = Vec::with_capacity(messages.len());
        for m in messages {
            match m {
                Msg::User(text) => out.push(json!({"role": "user", "content": text})),
                Msg::Assistant(a) | Msg::ProviderItems { assistant: a, .. } => {
                    let mut blocks: Vec<Value> = Vec::new();
                    if let Some(t) = &a.text {
                        if !t.is_empty() {
                            blocks.push(json!({"type": "text", "text": t}));
                        }
                    }
                    for tc in &a.tool_calls {
                        blocks.push(json!({
                            "type": "tool_use",
                            "id": tc.id,
                            "name": tc.name,
                            "input": tc.arguments,
                        }));
                    }
                    out.push(json!({"role": "assistant", "content": blocks}));
                }
                Msg::ToolResult { call_id, content } => {
                    out.push(json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": content,
                        }],
                    }));
                }
            }
        }
        out
    }

    fn build_body(&self, system: &str, messages: &[Msg], tools: &[ToolDef]) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "messages": Self::build_messages(messages),
        });
        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.schema,
                    })
                })
                .collect();
            body["tools"] = json!(tool_defs);
        }
        body
    }

    fn post(&self, body: &Value) -> Result<Value, ProviderError> {
        let resp = post_with_retry(&self.endpoint(), &self.api_key, body)?;
        let (i, o) = usage_from_value(&resp);
        self.meter.record(i, o);
        Ok(resp)
    }

    /// Parse Anthropic's `content[]` blocks into our `Completion`.
    fn parse_completion(resp: &Value) -> Result<Completion, ProviderError> {
        let content = resp
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| ProviderError::Parse("response missing `content` array".into()))?;

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        for block in content {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text_parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    let id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let arguments = block.get("input").cloned().unwrap_or(json!({}));
                    tool_calls.push(ToolCall {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
            }
        }
        let text = if text_parts.is_empty() {
            None
        } else {
            Some(text_parts.join(""))
        };
        Ok(Completion {
            text,
            tool_calls,
            provider_items: Vec::new(),
        })
    }
}

impl Provider for AnthropicProvider {
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        _format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        // Anthropic has no native JSON/schema mode; rely on the prompt + parse.
        let body = self.build_body(system, messages, &[]);
        let resp = self.post(&body)?;
        let completion = Self::parse_completion(&resp)?;
        Ok(completion.text.unwrap_or_default())
    }

    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        let body = self.build_body(system, messages, tools);
        let resp = self.post(&body)?;
        Self::parse_completion(&resp)
    }

    fn complete_with_tools_stream(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
        sink: &mut dyn FnMut(&str),
    ) -> Result<Completion, ProviderError> {
        let mut body = self.build_body(system, messages, tools);
        body["stream"] = json!(true);
        let resp = match stream_post(
            &self.endpoint(),
            &[
                ("x-api-key", &self.api_key),
                ("anthropic-version", ANTHROPIC_VERSION),
                ("content-type", "application/json"),
            ],
            &body,
        ) {
            Ok(r) => r,
            // Fall back to the non-streaming call if streaming setup fails.
            Err(_) => return self.complete_with_tools(system, messages, tools),
        };

        let mut text = String::new();
        // tool_use blocks keyed by content index: (id, name, partial_json).
        let mut blocks: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
        let (mut input, mut output) = (0u64, 0u64);
        read_sse(resp, |data| {
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return,
            };
            match v.get("type").and_then(|t| t.as_str()) {
                Some("message_start") => {
                    if let Some(n) = v
                        .pointer("/message/usage/input_tokens")
                        .and_then(|n| n.as_u64())
                    {
                        input = n;
                    }
                }
                Some("content_block_start") => {
                    let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let cb = v.get("content_block");
                    if cb.and_then(|c| c.get("type")).and_then(|t| t.as_str()) == Some("tool_use") {
                        let id = cb
                            .and_then(|c| c.get("id"))
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        let name = cb
                            .and_then(|c| c.get("name"))
                            .and_then(|s| s.as_str())
                            .unwrap_or_default()
                            .to_string();
                        blocks.insert(idx, (id, name, String::new()));
                    }
                }
                Some("content_block_delta") => {
                    let idx = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let delta = v.get("delta");
                    match delta.and_then(|d| d.get("type")).and_then(|t| t.as_str()) {
                        Some("text_delta") => {
                            if let Some(t) =
                                delta.and_then(|d| d.get("text")).and_then(|t| t.as_str())
                            {
                                text.push_str(t);
                                sink(t);
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(p) = delta
                                .and_then(|d| d.get("partial_json"))
                                .and_then(|t| t.as_str())
                            {
                                if let Some(b) = blocks.get_mut(&idx) {
                                    b.2.push_str(p);
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Some("message_delta") => {
                    if let Some(n) = v.pointer("/usage/output_tokens").and_then(|n| n.as_u64()) {
                        output = n;
                    }
                }
                _ => {}
            }
        })?;
        self.meter.record(input, output);

        let tool_calls = blocks
            .into_values()
            .map(|(id, name, partial)| ToolCall {
                id,
                name,
                arguments: serde_json::from_str(&partial).unwrap_or_else(|_| json!({})),
            })
            .collect();
        Ok(Completion {
            text: (!text.is_empty()).then_some(text),
            tool_calls,
            provider_items: Vec::new(),
        })
    }

    fn complete_stream(
        &self,
        system: &str,
        messages: &[Msg],
        _format: &ResponseFormat,
        sink: &mut dyn FnMut(&str),
    ) -> Result<String, ProviderError> {
        let mut body = self.build_body(system, messages, &[]);
        body["stream"] = json!(true);
        let resp = stream_post(
            &self.endpoint(),
            &[
                ("x-api-key", &self.api_key),
                ("anthropic-version", ANTHROPIC_VERSION),
                ("content-type", "application/json"),
            ],
            &body,
        )?;
        let mut full = String::new();
        let (mut input, mut output) = (0u64, 0u64);
        read_sse(resp, |data| {
            if let Some(t) = Self::text_delta(data) {
                full.push_str(&t);
                sink(&t);
            }
            // `message_start` carries input_tokens; `message_delta` the running
            // output_tokens. Capture both for the meter.
            if let Some((i, o)) = Self::stream_usage(data) {
                if i > 0 {
                    input = i;
                }
                if o > 0 {
                    output = o;
                }
            }
        })?;
        self.meter.record(input, output);
        Ok(full)
    }

    fn meter(&self) -> Arc<UsageMeter> {
        Arc::clone(&self.meter)
    }
}

impl AnthropicProvider {
    /// Extract the text of a `content_block_delta` SSE event, if present.
    fn text_delta(data: &str) -> Option<String> {
        let v: Value = serde_json::from_str(data).ok()?;
        if v.get("type").and_then(|t| t.as_str()) != Some("content_block_delta") {
            return None;
        }
        v.get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
    }

    /// Extract `(input, output)` token counts from `message_start` /
    /// `message_delta` SSE events (either may be 0/absent).
    fn stream_usage(data: &str) -> Option<(u64, u64)> {
        let v: Value = serde_json::from_str(data).ok()?;
        let usage = match v.get("type").and_then(|t| t.as_str()) {
            Some("message_start") => v.get("message")?.get("usage")?,
            Some("message_delta") => v.get("usage")?,
            _ => return None,
        };
        let i = usage
            .get("input_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        let o = usage
            .get("output_tokens")
            .and_then(|n| n.as_u64())
            .unwrap_or(0);
        Some((i, o))
    }
}

/// POST with retries on 429/5xx/connection errors (backoff + `Retry-After`),
/// mapping errors to `ProviderError`.
fn post_with_retry(url: &str, api_key: &str, body: &Value) -> Result<Value, ProviderError> {
    use super::{backoff, is_retryable_status, retry_after_secs, MAX_RETRIES};
    let mut attempt = 0;
    loop {
        let agent = external_http_agent(
            std::time::Duration::from_secs(HTTP_TIMEOUT_SECS),
            Some(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS)),
            None,
            None,
        );
        let result = agent
            .post(url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .send_json(body.clone());

        match result {
            Ok(mut resp) if status_is_accepted(resp.status()) => {
                return resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_PROVIDER_BODY_BYTES)
                    .read_json::<Value>()
                    .map_err(|e| ProviderError::Parse(e.to_string()));
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                if status == 401 {
                    return Err(ProviderError::Api {
                        status,
                        message: format!(
                            "API key invalid — is your ANTHROPIC_API_KEY set? ({})",
                            extract_error_message(resp)
                        ),
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
                    message: extract_error_message(resp),
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

fn extract_error_message(mut resp: HttpResponse) -> String {
    match resp
        .body_mut()
        .with_config()
        .limit(MAX_PROVIDER_BODY_BYTES)
        .read_json::<Value>()
    {
        Ok(v) => v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| v.to_string()),
        Err(_) => "unknown error".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::AssistantMsg;

    #[test]
    fn builds_tool_result_as_user_block() {
        let msgs = vec![
            Msg::User("hi".into()),
            Msg::Assistant(AssistantMsg {
                text: Some("ok".into()),
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "run_command".into(),
                    arguments: json!({"command": "ls"}),
                }],
            }),
            Msg::ToolResult {
                call_id: "call_1".into(),
                content: "exit 0".into(),
            },
        ];
        let built = AnthropicProvider::build_messages(&msgs);
        assert_eq!(built[0]["role"], "user");
        assert_eq!(built[1]["role"], "assistant");
        assert_eq!(built[1]["content"][1]["type"], "tool_use");
        assert_eq!(built[2]["role"], "user");
        assert_eq!(built[2]["content"][0]["type"], "tool_result");
        assert_eq!(built[2]["content"][0]["tool_use_id"], "call_1");
    }

    #[test]
    fn parses_text_and_tool_use() {
        let resp = json!({
            "content": [
                {"type": "text", "text": "let me check"},
                {"type": "tool_use", "id": "t1", "name": "run_command", "input": {"command": "ls"}}
            ]
        });
        let c = AnthropicProvider::parse_completion(&resp).unwrap();
        assert_eq!(c.text.as_deref(), Some("let me check"));
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].name, "run_command");
    }

    #[test]
    fn versioned_base_url_does_not_duplicate_the_api_prefix() {
        let provider = AnthropicProvider::new(
            "https://api.anthropic.com/v1/".into(),
            "test".into(),
            "claude-test".into(),
        );
        assert_eq!(provider.endpoint(), "https://api.anthropic.com/v1/messages");
    }
}
