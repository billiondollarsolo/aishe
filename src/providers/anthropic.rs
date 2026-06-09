//! Anthropic Messages API provider (`POST {base_url}/v1/messages`).

use serde_json::{json, Value};

use super::{
    Completion, Msg, Provider, ProviderError, ToolCall, ToolDef, HTTP_TIMEOUT_SECS, MAX_TOKENS,
};

const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    base_url: String,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
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
                Msg::Assistant(a) => {
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
        post_with_retry(&self.endpoint(), &self.api_key, body)
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
        Ok(Completion { text, tool_calls })
    }
}

impl Provider for AnthropicProvider {
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        _json_mode: bool,
    ) -> Result<String, ProviderError> {
        // Anthropic has no native JSON mode; we rely on the prompt + defensive parsing.
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
}

/// POST with one retry on 429/5xx (after 2s), mapping errors to `ProviderError`.
fn post_with_retry(url: &str, api_key: &str, body: &Value) -> Result<Value, ProviderError> {
    let mut attempt = 0;
    loop {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();
        let result = agent
            .post(url)
            .set("x-api-key", api_key)
            .set("anthropic-version", ANTHROPIC_VERSION)
            .set("content-type", "application/json")
            .send_json(body.clone());

        match result {
            Ok(resp) => {
                return resp
                    .into_json::<Value>()
                    .map_err(|e| ProviderError::Parse(e.to_string()));
            }
            Err(ureq::Error::Status(status, resp)) => {
                let message = extract_error_message(resp);
                if status == 401 {
                    return Err(ProviderError::Api {
                        status,
                        message: format!(
                            "API key invalid — is your ANTHROPIC_API_KEY set? ({message})"
                        ),
                    });
                }
                if (status == 429 || status >= 500) && attempt == 0 {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                return Err(ProviderError::Api { status, message });
            }
            Err(e) => {
                if attempt == 0 {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_secs(2));
                    continue;
                }
                return Err(ProviderError::Http(e.to_string()));
            }
        }
    }
}

fn extract_error_message(resp: ureq::Response) -> String {
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
}
