//! OpenAI-compatible Chat Completions provider
//! (`POST {base_url}/v1/chat/completions`). Works with OpenAI, Ollama,
//! OpenRouter, Together, etc. via `base_url`.

use serde_json::{json, Value};

use super::{
    Completion, Msg, Provider, ProviderError, ToolCall, ToolDef, HTTP_TIMEOUT_SECS, MAX_TOKENS,
};

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/v1/chat/completions", self.base_url)
    }

    /// Translate canonical messages into OpenAI's `messages[]`, with `system`
    /// prepended as the first message.
    fn build_messages(system: &str, messages: &[Msg]) -> Vec<Value> {
        let mut out = Vec::with_capacity(messages.len() + 1);
        out.push(json!({"role": "system", "content": system}));
        for m in messages {
            match m {
                Msg::User(text) => out.push(json!({"role": "user", "content": text})),
                Msg::Assistant(a) => {
                    let mut msg = json!({"role": "assistant"});
                    // OpenAI requires `content` to be present (may be null).
                    msg["content"] = match &a.text {
                        Some(t) if !t.is_empty() => json!(t),
                        _ => Value::Null,
                    };
                    if !a.tool_calls.is_empty() {
                        let calls: Vec<Value> = a
                            .tool_calls
                            .iter()
                            .map(|tc| {
                                json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": tc.arguments.to_string(),
                                    },
                                })
                            })
                            .collect();
                        msg["tool_calls"] = json!(calls);
                    }
                    out.push(msg);
                }
                Msg::ToolResult { call_id, content } => {
                    out.push(json!({
                        "role": "tool",
                        "tool_call_id": call_id,
                        "content": content,
                    }));
                }
            }
        }
        out
    }

    fn build_body(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
        json_mode: bool,
    ) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": Self::build_messages(system, messages),
        });
        if !tools.is_empty() {
            let tool_defs: Vec<Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.schema,
                        },
                    })
                })
                .collect();
            body["tools"] = json!(tool_defs);
        }
        if json_mode {
            body["response_format"] = json!({"type": "json_object"});
        }
        body
    }

    fn post(&self, body: &Value) -> Result<Value, ProviderError> {
        post_with_retry(&self.endpoint(), &self.api_key, body)
    }

    /// Parse `choices[0].message` into our `Completion`. Tool-call arguments
    /// arrive as a JSON *string*, which we parse into a Value.
    fn parse_completion(resp: &Value) -> Result<Completion, ProviderError> {
        let message = resp
            .get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("message"))
            .ok_or_else(|| ProviderError::Parse("response missing choices[0].message".into()))?;

        let text = message
            .get("content")
            .and_then(|c| c.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let mut tool_calls = Vec::new();
        if let Some(calls) = message.get("tool_calls").and_then(|t| t.as_array()) {
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let func = call.get("function");
                let name = func
                    .and_then(|f| f.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let args_str = func
                    .and_then(|f| f.get("arguments"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("{}");
                let arguments: Value = serde_json::from_str(args_str).unwrap_or(json!({}));
                tool_calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
        }
        Ok(Completion { text, tool_calls })
    }
}

impl Provider for OpenAiProvider {
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        json_mode: bool,
    ) -> Result<String, ProviderError> {
        let body = self.build_body(system, messages, &[], json_mode);
        let resp = match self.post(&body) {
            Ok(r) => r,
            // Ollama and some compat servers reject response_format; retry without it.
            Err(ProviderError::Api {
                status: 400,
                message,
            }) if json_mode && message.contains("response_format") => {
                let body = self.build_body(system, messages, &[], false);
                self.post(&body)?
            }
            Err(e) => return Err(e),
        };
        let completion = Self::parse_completion(&resp)?;
        Ok(completion.text.unwrap_or_default())
    }

    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        let body = self.build_body(system, messages, tools, false);
        let resp = self.post(&body)?;
        Self::parse_completion(&resp)
    }
}

fn post_with_retry(url: &str, api_key: &str, body: &Value) -> Result<Value, ProviderError> {
    let mut attempt = 0;
    loop {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build();
        let result = agent
            .post(url)
            .set("Authorization", &format!("Bearer {api_key}"))
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
                            "API key invalid — is your OPENAI_API_KEY set? ({message})"
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
    fn system_is_first_message() {
        let built = OpenAiProvider::build_messages("SYS", &[Msg::User("hi".into())]);
        assert_eq!(built[0]["role"], "system");
        assert_eq!(built[0]["content"], "SYS");
        assert_eq!(built[1]["role"], "user");
    }

    #[test]
    fn tool_call_arguments_are_stringified() {
        let msgs = vec![Msg::Assistant(AssistantMsg {
            text: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "run_command".into(),
                arguments: json!({"command": "ls"}),
            }],
        })];
        let built = OpenAiProvider::build_messages("s", &msgs);
        let args = built[1]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert!(args.contains("\"command\""));
    }

    #[test]
    fn parses_string_arguments() {
        let resp = json!({
            "choices": [{
                "message": {
                    "content": null,
                    "tool_calls": [{
                        "id": "c1",
                        "type": "function",
                        "function": {"name": "run_command", "arguments": "{\"command\": \"ls -la\"}"}
                    }]
                }
            }]
        });
        let c = OpenAiProvider::parse_completion(&resp).unwrap();
        assert_eq!(c.tool_calls.len(), 1);
        assert_eq!(c.tool_calls[0].arguments["command"], "ls -la");
    }
}
