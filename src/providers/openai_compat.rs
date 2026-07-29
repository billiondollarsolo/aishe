//! OpenAI-compatible Chat Completions provider
//! (`POST {base_url}/v1/chat/completions`). Works with OpenAI, Ollama,
//! OpenRouter, Together, etc. via `base_url`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};

use super::{
    read_sse, stream_post, usage_from_value, Completion, Msg, Provider, ProviderError,
    ResponseFormat, ToolCall, ToolDef, HTTP_TIMEOUT_SECS, MAX_TOKENS,
};
use crate::usage::UsageMeter;

pub struct OpenAiProvider {
    base_url: String,
    api_key: String,
    model: String,
    meter: Arc<UsageMeter>,
    token_limit_param: AtomicU8,
    token_limit_known: AtomicU8,
    token_limit_cache: Option<PathBuf>,
}

impl OpenAiProvider {
    pub fn new(base_url: String, api_key: String, model: String) -> Self {
        let token_limit_cache = token_limit_cache_path(&base_url, &model);
        Self::with_token_limit_cache(base_url, api_key, model, token_limit_cache)
    }

    fn with_token_limit_cache(
        base_url: String,
        api_key: String,
        model: String,
        token_limit_cache: Option<PathBuf>,
    ) -> Self {
        let stored = token_limit_cache
            .as_deref()
            .and_then(load_token_limit_param);
        let token_limit_param = stored.unwrap_or_else(|| TokenLimitParam::default_for(&base_url));
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            meter: Arc::new(UsageMeter::default()),
            token_limit_param: AtomicU8::new(token_limit_param as u8),
            token_limit_known: AtomicU8::new(u8::from(stored.is_some())),
            token_limit_cache,
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
        format: &ResponseFormat,
    ) -> (Value, TokenLimitParam) {
        let token_limit_param = self.token_limit_param();
        let mut body = json!({
            "model": self.model,
            "messages": Self::build_messages(system, messages),
        });
        body[token_limit_param.name()] = json!(MAX_TOKENS);
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
        match format {
            ResponseFormat::Text => {}
            ResponseFormat::Json => {
                body["response_format"] = json!({"type": "json_object"});
            }
            ResponseFormat::JsonSchema { name, schema } => {
                body["response_format"] = json!({
                    "type": "json_schema",
                    "json_schema": {"name": name, "strict": true, "schema": schema},
                });
            }
        }
        (body, token_limit_param)
    }

    fn post(&self, body: &Value) -> Result<Value, ProviderError> {
        let resp = post_with_retry(&self.endpoint(), &self.api_key, body)?;
        let (i, o) = usage_from_value(&resp);
        self.meter.record(i, o);
        Ok(resp)
    }

    fn token_limit_param(&self) -> TokenLimitParam {
        TokenLimitParam::from_u8(self.token_limit_param.load(Ordering::Relaxed))
    }

    /// Switch to the alternate token-limit spelling after the endpoint rejects
    /// the one used by this request. The choice is only persisted after a later
    /// request succeeds, so a transient or misleading error cannot poison the
    /// cross-process cache.
    fn switch_rejected_token_limit(
        &self,
        rejected: TokenLimitParam,
        message: &str,
        already_switched: &mut bool,
    ) -> bool {
        if *already_switched || !is_token_limit_error(message, rejected) {
            return false;
        }
        self.token_limit_param
            .store(rejected.other() as u8, Ordering::Relaxed);
        self.token_limit_known.store(0, Ordering::Relaxed);
        *already_switched = true;
        true
    }

    /// Record a parameter that the endpoint accepted. A provider/model cache
    /// file means subsequent `aishe` child processes start with this spelling
    /// and do not pay for the same rejected probe on every shell command.
    fn remember_accepted_token_limit(&self, accepted: TokenLimitParam) {
        self.token_limit_param
            .store(accepted as u8, Ordering::Relaxed);
        if self.token_limit_known.swap(1, Ordering::Relaxed) != 0 {
            return;
        }
        let Some(path) = &self.token_limit_cache else {
            return;
        };
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        let _ = crate::config::write_atomic(path, accepted.name().as_bytes());
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
        format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        // Try the requested format; on a `response_format`/schema rejection (a
        // server that doesn't support it), step down to a looser one and retry.
        let mut fmt = format.clone();
        let mut token_limit_switched = false;
        loop {
            let (body, token_limit_param) = self.build_body(system, messages, &[], &fmt);
            match self.post(&body) {
                Ok(resp) => {
                    self.remember_accepted_token_limit(token_limit_param);
                    return Ok(Self::parse_completion(&resp)?.text.unwrap_or_default());
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if self.switch_rejected_token_limit(
                    token_limit_param,
                    &message,
                    &mut token_limit_switched,
                ) =>
                {
                    continue
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if is_format_error(&message) => match step_down(&fmt) {
                    Some(next) => fmt = next,
                    None => {
                        return Err(ProviderError::Api {
                            status: 400,
                            message,
                        })
                    }
                },
                Err(e) => return Err(e),
            }
        }
    }

    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        let mut token_limit_switched = false;
        loop {
            let (body, token_limit_param) =
                self.build_body(system, messages, tools, &ResponseFormat::Text);
            match self.post(&body) {
                Ok(resp) => {
                    self.remember_accepted_token_limit(token_limit_param);
                    return Self::parse_completion(&resp);
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if self.switch_rejected_token_limit(
                    token_limit_param,
                    &message,
                    &mut token_limit_switched,
                ) =>
                {
                    continue
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn complete_with_tools_stream(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
        sink: &mut dyn FnMut(&str),
    ) -> Result<Completion, ProviderError> {
        let auth = format!("Bearer {}", self.api_key);
        let headers = [
            ("Authorization", auth.as_str()),
            ("content-type", "application/json"),
        ];
        let mut token_limit_switched = false;
        let resp = loop {
            let (mut body, token_limit_param) =
                self.build_body(system, messages, tools, &ResponseFormat::Text);
            body["stream"] = json!(true);
            body["stream_options"] = json!({"include_usage": true});
            match stream_post(&self.endpoint(), &headers, &body) {
                Ok(r) => {
                    self.remember_accepted_token_limit(token_limit_param);
                    break r;
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if self.switch_rejected_token_limit(
                    token_limit_param,
                    &message,
                    &mut token_limit_switched,
                ) =>
                {
                    continue
                }
                // Fall back to the non-streaming call if streaming setup fails.
                Err(_) => return self.complete_with_tools(system, messages, tools),
            }
        };

        let mut text = String::new();
        // tool calls keyed by their `index`: (id, name, arguments-fragment).
        let mut calls: BTreeMap<u64, (String, String, String)> = BTreeMap::new();
        let (mut input, mut output) = (0u64, 0u64);
        read_sse(resp, |data| {
            let v: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(_) => return,
            };
            let (i, o) = usage_from_value(&v);
            if i > 0 {
                input = i;
            }
            if o > 0 {
                output = o;
            }
            let delta = v.pointer("/choices/0/delta");
            if let Some(t) = delta
                .and_then(|d| d.get("content"))
                .and_then(|c| c.as_str())
                .filter(|s| !s.is_empty())
            {
                text.push_str(t);
                sink(t);
            }
            if let Some(tcs) = delta
                .and_then(|d| d.get("tool_calls"))
                .and_then(|t| t.as_array())
            {
                for tc in tcs {
                    let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
                    let entry = calls.entry(idx).or_default();
                    if let Some(id) = tc
                        .get("id")
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        entry.0 = id.to_string();
                    }
                    let func = tc.get("function");
                    if let Some(name) = func
                        .and_then(|f| f.get("name"))
                        .and_then(|s| s.as_str())
                        .filter(|s| !s.is_empty())
                    {
                        entry.1 = name.to_string();
                    }
                    if let Some(args) = func
                        .and_then(|f| f.get("arguments"))
                        .and_then(|s| s.as_str())
                    {
                        entry.2.push_str(args);
                    }
                }
            }
        })?;
        self.meter.record(input, output);

        let tool_calls = calls
            .into_values()
            .map(|(id, name, args)| ToolCall {
                id,
                name,
                arguments: serde_json::from_str(&args).unwrap_or_else(|_| json!({})),
            })
            .collect();
        Ok(Completion {
            text: (!text.is_empty()).then_some(text),
            tool_calls,
        })
    }

    fn complete_stream(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
        sink: &mut dyn FnMut(&str),
    ) -> Result<String, ProviderError> {
        let auth = format!("Bearer {}", self.api_key);
        let headers = [
            ("Authorization", auth.as_str()),
            ("content-type", "application/json"),
        ];
        let mut fmt = format.clone();
        let mut token_limit_switched = false;
        let resp = loop {
            let (mut body, token_limit_param) = self.build_body(system, messages, &[], &fmt);
            body["stream"] = json!(true);
            // Ask for a trailing usage chunk (supported by OpenAI, Groq, …);
            // servers that ignore it simply omit usage.
            body["stream_options"] = json!({"include_usage": true});
            match stream_post(&self.endpoint(), &headers, &body) {
                Ok(r) => {
                    self.remember_accepted_token_limit(token_limit_param);
                    break r;
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if self.switch_rejected_token_limit(
                    token_limit_param,
                    &message,
                    &mut token_limit_switched,
                ) =>
                {
                    continue
                }
                Err(ProviderError::Api {
                    status: 400,
                    message,
                }) if is_format_error(&message) => match step_down(&fmt) {
                    Some(next) => fmt = next,
                    None => {
                        return Err(ProviderError::Api {
                            status: 400,
                            message,
                        })
                    }
                },
                Err(e) => return Err(e),
            }
        };

        let mut full = String::new();
        let (mut input, mut output) = (0u64, 0u64);
        read_sse(resp, |data| {
            if let Some(t) = Self::content_delta(data) {
                full.push_str(&t);
                sink(&t);
            }
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                let (i, o) = usage_from_value(&v);
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

    fn embed(&self, texts: &[String], model: &str) -> Result<Vec<Vec<f32>>, ProviderError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = json!({ "model": model, "input": texts });
        let resp = post_with_retry(&url, &self.api_key, &body)?;
        let data = resp
            .get("data")
            .and_then(|d| d.as_array())
            .ok_or_else(|| ProviderError::Parse("embeddings response missing data[]".into()))?;
        let mut out = Vec::with_capacity(data.len());
        for item in data {
            let v = item
                .get("embedding")
                .and_then(|e| e.as_array())
                .ok_or_else(|| ProviderError::Parse("embedding entry missing vector".into()))?;
            out.push(
                v.iter()
                    .filter_map(|x| x.as_f64().map(|f| f as f32))
                    .collect(),
            );
        }
        Ok(out)
    }

    fn meter(&self) -> Arc<UsageMeter> {
        Arc::clone(&self.meter)
    }
}

/// The two token-limit spellings used across Chat Completions implementations.
///
/// OpenAI deprecates `max_tokens` in favor of `max_completion_tokens`, while
/// some compatible servers still only implement the older spelling.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum TokenLimitParam {
    MaxTokens = 0,
    MaxCompletionTokens = 1,
}

impl TokenLimitParam {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::MaxCompletionTokens,
            _ => Self::MaxTokens,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::MaxTokens => "max_tokens",
            Self::MaxCompletionTokens => "max_completion_tokens",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::MaxTokens => Self::MaxCompletionTokens,
            Self::MaxCompletionTokens => Self::MaxTokens,
        }
    }

    fn default_for(base_url: &str) -> Self {
        if base_url
            .trim_end_matches('/')
            .eq_ignore_ascii_case("https://api.openai.com")
        {
            Self::MaxCompletionTokens
        } else {
            Self::MaxTokens
        }
    }
}

/// A stable, non-sensitive filename for one endpoint/model pair. Base URLs can
/// contain credentials on unusual compatible services, so neither the URL nor
/// model name is written to disk verbatim.
fn token_limit_cache_key(base_url: &str, model: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in base_url
        .trim_end_matches('/')
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(0xff))
        .chain(model.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn token_limit_cache_path(base_url: &str, model: &str) -> Option<PathBuf> {
    crate::config::data_root().map(|root| {
        root.join("aishe").join("provider-compat").join(format!(
            "openai-{}.token-limit",
            token_limit_cache_key(base_url, model)
        ))
    })
}

fn load_token_limit_param(path: &Path) -> Option<TokenLimitParam> {
    match std::fs::read_to_string(path).ok()?.trim() {
        "max_tokens" => Some(TokenLimitParam::MaxTokens),
        "max_completion_tokens" => Some(TokenLimitParam::MaxCompletionTokens),
        _ => None,
    }
}

/// Does a 400 message indicate that the endpoint rejected the token-limit
/// spelling used in this request?
fn is_token_limit_error(message: &str, used: TokenLimitParam) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains(used.name())
        && [
            "unsupported parameter",
            "not supported",
            "does not support",
            "unrecognized",
            "unknown parameter",
            "invalid parameter",
        ]
        .iter()
        .any(|needle| message.contains(needle))
}

/// The next looser response format to retry with (schema → json → text → none).
fn step_down(f: &ResponseFormat) -> Option<ResponseFormat> {
    match f {
        ResponseFormat::JsonSchema { .. } => Some(ResponseFormat::Json),
        ResponseFormat::Json => Some(ResponseFormat::Text),
        ResponseFormat::Text => None,
    }
}

/// Does a 400 message indicate the server rejected our `response_format`?
fn is_format_error(message: &str) -> bool {
    let m = message.to_lowercase();
    m.contains("response_format") || m.contains("json_schema") || m.contains("schema")
}

impl OpenAiProvider {
    /// Extract `choices[0].delta.content` from a streaming chunk, if present.
    fn content_delta(data: &str) -> Option<String> {
        let v: Value = serde_json::from_str(data).ok()?;
        v.get("choices")
            .and_then(|c| c.as_array())
            .and_then(|a| a.first())
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(|t| t.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }
}

fn post_with_retry(url: &str, api_key: &str, body: &Value) -> Result<Value, ProviderError> {
    use super::{backoff, is_retryable_status, retry_after_secs, MAX_RETRIES};
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
                if status == 401 {
                    return Err(ProviderError::Api {
                        status,
                        message: format!(
                            "API key invalid — is your OPENAI_API_KEY set? ({})",
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
    use mockito::Matcher;

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

    #[test]
    fn step_down_walks_schema_to_json_to_text_to_none() {
        // schema → json → text → (give up). The chain must terminate so the
        // retry loop in `complete` cannot spin forever.
        let schema = ResponseFormat::JsonSchema {
            name: "s".into(),
            schema: json!({"type": "object"}),
        };
        let next = step_down(&schema).expect("schema steps down");
        assert!(matches!(next, ResponseFormat::Json));
        let next = step_down(&next).expect("json steps down");
        assert!(matches!(next, ResponseFormat::Text));
        assert!(step_down(&ResponseFormat::Text).is_none());
    }

    #[test]
    fn format_errors_are_recognized_loosely() {
        // Real 400 bodies vary in wording; any of these means "drop the
        // response_format and retry looser".
        assert!(is_format_error(
            "Invalid value for 'response_format': json_schema is not supported"
        ));
        assert!(is_format_error("This model does not support json_schema."));
        assert!(is_format_error("Unrecognized request argument: schema"));
        assert!(is_format_error("RESPONSE_FORMAT rejected")); // case-insensitive
                                                              // Unrelated 400s must NOT be mistaken for a format error (we would hide
                                                              // a real client bug behind endless step-downs otherwise).
        assert!(!is_format_error("context length exceeded"));
        assert!(!is_format_error("invalid api key"));
    }

    #[test]
    fn official_openai_uses_the_current_token_limit_parameter() {
        let provider = OpenAiProvider::with_token_limit_cache(
            "https://api.openai.com".into(),
            "k".into(),
            "gpt-x".into(),
            None,
        );
        let (body, used) =
            provider.build_body("SYS", &[Msg::User("hi".into())], &[], &ResponseFormat::Text);
        assert_eq!(used, TokenLimitParam::MaxCompletionTokens);
        assert_eq!(body["max_completion_tokens"], MAX_TOKENS);
        assert!(body.get("max_tokens").is_none());
    }

    #[test]
    fn token_limit_fallback_is_persisted_for_the_endpoint_and_model() {
        let mut server = mockito::Server::new();
        let legacy = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "model": "learn-me",
                "max_tokens": MAX_TOKENS
            })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":{"message":"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."}}"#,
            )
            .expect(1)
            .create();
        let current = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "model": "learn-me",
                "max_completion_tokens": MAX_TOKENS
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"Paris"}}]}"#)
            .expect(2)
            .create();

        let dir = std::env::temp_dir().join(format!(
            "aishe-openai-compat-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cache = dir.join("token-limit");
        let provider = OpenAiProvider::with_token_limit_cache(
            server.url(),
            "k".into(),
            "learn-me".into(),
            Some(cache.clone()),
        );
        assert_eq!(
            provider
                .complete(
                    "SYS",
                    &[Msg::User("question".into())],
                    &ResponseFormat::Text
                )
                .unwrap(),
            "Paris"
        );
        assert_eq!(
            std::fs::read_to_string(&cache).unwrap(),
            "max_completion_tokens"
        );

        // A fresh provider represents the next `aishe` child process. It reads
        // the accepted spelling and goes straight to the successful request.
        let next_process = OpenAiProvider::with_token_limit_cache(
            server.url(),
            "k".into(),
            "learn-me".into(),
            Some(cache),
        );
        assert_eq!(
            next_process
                .complete("SYS", &[Msg::User("again".into())], &ResponseFormat::Text)
                .unwrap(),
            "Paris"
        );
        legacy.assert();
        current.assert();
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn streaming_retries_with_the_alternate_token_limit_parameter() {
        let mut server = mockito::Server::new();
        let legacy = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "max_tokens": MAX_TOKENS,
                "stream": true
            })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":{"message":"Unsupported parameter: max_tokens; use max_completion_tokens"}}"#,
            )
            .create();
        let current = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "max_completion_tokens": MAX_TOKENS,
                "stream": true
            })))
            .with_status(200)
            .with_header("content-type", "text/event-stream")
            .with_body(
                "data: {\"choices\":[{\"delta\":{\"content\":\"Paris\"}}]}\n\ndata: [DONE]\n\n",
            )
            .create();

        let provider = OpenAiProvider::with_token_limit_cache(
            server.url(),
            "k".into(),
            "stream-model".into(),
            None,
        );
        let mut streamed = String::new();
        let answer = provider
            .complete_stream(
                "SYS",
                &[Msg::User("question".into())],
                &ResponseFormat::Text,
                &mut |delta| streamed.push_str(delta),
            )
            .unwrap();
        assert_eq!(answer, "Paris");
        assert_eq!(streamed, "Paris");
        legacy.assert();
        current.assert();
    }

    #[test]
    fn tool_requests_retry_with_the_alternate_token_limit_parameter() {
        let mut server = mockito::Server::new();
        let legacy = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "max_tokens": MAX_TOKENS,
                "tools": [{"type": "function"}]
            })))
            .with_status(400)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"error":{"message":"Unsupported parameter: max_tokens; use max_completion_tokens"}}"#,
            )
            .create();
        let current = server
            .mock("POST", "/v1/chat/completions")
            .match_body(Matcher::PartialJson(json!({
                "max_completion_tokens": MAX_TOKENS,
                "tools": [{"type": "function"}]
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"choices":[{"message":{"content":"done"}}]}"#)
            .create();
        let provider = OpenAiProvider::with_token_limit_cache(
            server.url(),
            "k".into(),
            "tool-model".into(),
            None,
        );
        let tools = [ToolDef {
            name: "run_command".into(),
            description: "run a command".into(),
            schema: json!({"type": "object"}),
        }];

        let completion = provider
            .complete_with_tools("SYS", &[Msg::User("go".into())], &tools)
            .unwrap();
        assert_eq!(completion.text.as_deref(), Some("done"));
        legacy.assert();
        current.assert();
    }

    #[test]
    fn compatibility_cache_is_scoped_to_endpoint_and_model() {
        assert_eq!(
            token_limit_cache_key("https://example.test/", "model-a"),
            token_limit_cache_key("https://example.test", "model-a")
        );
        assert_ne!(
            token_limit_cache_key("https://example.test", "model-a"),
            token_limit_cache_key("https://example.test", "model-b")
        );
        assert_ne!(
            token_limit_cache_key("https://one.example", "model-a"),
            token_limit_cache_key("https://two.example", "model-a")
        );
    }

    #[test]
    fn token_limit_errors_are_specific_to_the_parameter_used() {
        assert!(is_token_limit_error(
            "Unsupported parameter: 'max_tokens' is not supported with this model",
            TokenLimitParam::MaxTokens
        ));
        assert!(is_token_limit_error(
            "Unrecognized request argument supplied: max_completion_tokens",
            TokenLimitParam::MaxCompletionTokens
        ));
        assert!(!is_token_limit_error(
            "context length exceeded",
            TokenLimitParam::MaxTokens
        ));
    }
}
