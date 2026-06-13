//! Provider HTTP tests using mockito, asserting request shapes for both APIs.

use aishe::providers::anthropic::AnthropicProvider;
use aishe::providers::openai_compat::OpenAiProvider;
use aishe::providers::{
    AssistantMsg, Msg, Provider, ProviderError, ResponseFormat, ToolCall, ToolDef,
};
use mockito::Matcher;
use serde_json::json;

fn tool() -> ToolDef {
    ToolDef {
        name: "run_command".into(),
        description: "run a command".into(),
        schema: json!({
            "type": "object",
            "properties": {"command": {"type": "string"}},
            "required": ["command"]
        }),
    }
}

#[test]
fn anthropic_complete_request_and_parse() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/messages")
        .match_header("x-api-key", "secret")
        .match_header("anthropic-version", "2023-06-01")
        .match_body(Matcher::PartialJson(json!({
            "model": "claude-x",
            "max_tokens": 4096,
            "system": "SYS",
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"content":[{"type":"text","text":"hello world"}]}"#)
        .create();

    let p = AnthropicProvider::new(server.url(), "secret".into(), "claude-x".into());
    let out = p
        .complete("SYS", &[Msg::User("hi".into())], &ResponseFormat::Json)
        .unwrap();
    assert_eq!(out, "hello world");
    m.assert();
}

#[test]
fn anthropic_tool_schema_and_tool_result_translation() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/messages")
        // tools[].input_schema is the Anthropic shape.
        .match_body(Matcher::PartialJson(json!({
            "tools": [{"name": "run_command", "input_schema": {"type": "object"}}]
        })))
        .with_status(200)
        .with_body(r#"{"content":[{"type":"tool_use","id":"t1","name":"run_command","input":{"command":"ls"}}]}"#)
        .create();

    let p = AnthropicProvider::new(server.url(), "k".into(), "m".into());
    let msgs = vec![
        Msg::User("do it".into()),
        Msg::Assistant(AssistantMsg {
            text: None,
            tool_calls: vec![ToolCall {
                id: "t0".into(),
                name: "run_command".into(),
                arguments: json!({"command": "pwd"}),
            }],
        }),
        Msg::ToolResult {
            call_id: "t0".into(),
            content: "exit 0".into(),
        },
    ];
    let completion = p.complete_with_tools("SYS", &msgs, &[tool()]).unwrap();
    assert_eq!(completion.tool_calls.len(), 1);
    assert_eq!(completion.tool_calls[0].name, "run_command");
    m.assert();
}

#[test]
fn anthropic_401_message() {
    let mut server = mockito::Server::new();
    let _m = server
        .mock("POST", "/v1/messages")
        .with_status(401)
        .with_body(r#"{"error":{"message":"invalid"}}"#)
        .create();

    let p = AnthropicProvider::new(server.url(), "bad".into(), "m".into());
    let err = p
        .complete("s", &[Msg::User("x".into())], &ResponseFormat::Text)
        .unwrap_err();
    match err {
        ProviderError::Api { status, message } => {
            assert_eq!(status, 401);
            assert!(message.contains("API key invalid"), "msg: {message}");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[test]
fn anthropic_retries_on_429() {
    let mut server = mockito::Server::new();
    // A persistent 429 is retried: original call + MAX_RETRIES (3) retries = 4.
    let m = server
        .mock("POST", "/v1/messages")
        .with_status(429)
        .with_body(r#"{"error":{"message":"rate limited"}}"#)
        .expect(4)
        .create();

    let p = AnthropicProvider::new(server.url(), "k".into(), "m".into());
    let err = p
        .complete("s", &[Msg::User("x".into())], &ResponseFormat::Text)
        .unwrap_err();
    assert!(matches!(err, ProviderError::Api { status: 429, .. }));
    m.assert();
}

#[test]
fn anthropic_streams_text_deltas() {
    let mut server = mockito::Server::new();
    // A minimal Anthropic SSE stream: two text deltas across content blocks.
    let sse = concat!(
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\" world\"}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    );
    let m = server
        .mock("POST", "/v1/messages")
        .match_body(Matcher::PartialJson(json!({ "stream": true })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse)
        .create();

    let p = AnthropicProvider::new(server.url(), "k".into(), "m".into());
    let mut chunks: Vec<String> = Vec::new();
    let full = p
        .complete_stream(
            "SYS",
            &[Msg::User("hi".into())],
            &ResponseFormat::Text,
            &mut |d| chunks.push(d.to_string()),
        )
        .unwrap();
    assert_eq!(full, "Hello world");
    assert_eq!(chunks, vec!["Hello", " world"]);
    m.assert();
}

#[test]
fn openai_streams_content_deltas() {
    let mut server = mockito::Server::new();
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let m = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(json!({ "stream": true })))
        .with_status(200)
        .with_header("content-type", "text/event-stream")
        .with_body(sse)
        .create();

    let p = OpenAiProvider::new(server.url(), "tok".into(), "gpt-x".into());
    let mut got = String::new();
    let full = p
        .complete_stream(
            "SYS",
            &[Msg::User("hi".into())],
            &ResponseFormat::Text,
            &mut |d| got.push_str(d),
        )
        .unwrap();
    assert_eq!(full, "Hello");
    assert_eq!(got, "Hello");
    m.assert();
}

#[test]
fn openai_system_first_and_json_mode() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/chat/completions")
        .match_header("authorization", "Bearer tok")
        .match_body(Matcher::PartialJson(json!({
            "model": "gpt-x",
            "response_format": {"type": "json_object"},
            "messages": [{"role": "system", "content": "SYS"}]
        })))
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"choices":[{"message":{"content":"{\"ok\":true}"}}]}"#)
        .create();

    let p = OpenAiProvider::new(server.url(), "tok".into(), "gpt-x".into());
    let out = p
        .complete("SYS", &[Msg::User("hi".into())], &ResponseFormat::Json)
        .unwrap();
    assert!(out.contains("ok"));
    m.assert();
}

#[test]
fn openai_json_schema_format() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(json!({
            "response_format": {"type": "json_schema", "json_schema": {"strict": true}}
        })))
        .with_status(200)
        .with_body(r#"{"choices":[{"message":{"content":"{\"type\":\"answer\"}"}}]}"#)
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let fmt = ResponseFormat::JsonSchema {
        name: "aishe_suggestion".into(),
        schema: json!({"type": "object"}),
    };
    p.complete("SYS", &[Msg::User("hi".into())], &fmt).unwrap();
    m.assert();
}

#[test]
fn openai_steps_down_when_schema_unsupported() {
    let mut server = mockito::Server::new();
    // json_schema rejected → provider should retry with json_object.
    let schema_mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"response_format": {"type": "json_schema"}}),
        ))
        .with_status(400)
        .with_body(r#"{"error":{"message":"response_format json_schema not supported"}}"#)
        .create();
    let json_mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"response_format": {"type": "json_object"}}),
        ))
        .with_status(200)
        .with_body(r#"{"choices":[{"message":{"content":"{\"ok\":1}"}}]}"#)
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let fmt = ResponseFormat::JsonSchema {
        name: "s".into(),
        schema: json!({"type": "object"}),
    };
    let out = p.complete("SYS", &[Msg::User("hi".into())], &fmt).unwrap();
    assert!(out.contains("ok"));
    schema_mock.assert();
    json_mock.assert();
}

#[test]
fn openai_steps_down_all_the_way_to_text() {
    // A stricter server rejects BOTH json_schema and json_object; the provider
    // must keep stepping down to a plain (no response_format) request and
    // succeed there, rather than giving up after one hop.
    let mut server = mockito::Server::new();
    let schema_mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"response_format": {"type": "json_schema"}}),
        ))
        .with_status(400)
        .with_body(r#"{"error":{"message":"json_schema not supported"}}"#)
        .create();
    let json_mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(
            json!({"response_format": {"type": "json_object"}}),
        ))
        .with_status(400)
        .with_body(r#"{"error":{"message":"response_format is not supported"}}"#)
        .create();
    // The text retry carries no response_format at all; match its absence by
    // matching the messages and expecting a 200.
    let text_mock = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(json!({"model": "m"})))
        .with_status(200)
        .with_body(r#"{"choices":[{"message":{"content":"plain text answer"}}]}"#)
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let fmt = ResponseFormat::JsonSchema {
        name: "s".into(),
        schema: json!({"type": "object"}),
    };
    let out = p.complete("SYS", &[Msg::User("hi".into())], &fmt).unwrap();
    assert!(out.contains("plain text answer"), "out: {out}");
    schema_mock.assert();
    json_mock.assert();
    text_mock.assert();
}

#[test]
fn openai_gives_up_when_even_text_is_rejected() {
    // If every format (including no response_format) yields a format-shaped 400,
    // step_down eventually returns None and the original 400 surfaces. This is
    // the guard against an endless retry loop.
    let mut server = mockito::Server::new();
    let _m = server
        .mock("POST", "/v1/chat/completions")
        .with_status(400)
        .with_body(r#"{"error":{"message":"response_format rejected"}}"#)
        // schema + json + text = three attempts, then give up.
        .expect(3)
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let fmt = ResponseFormat::JsonSchema {
        name: "s".into(),
        schema: json!({"type": "object"}),
    };
    let err = p
        .complete("SYS", &[Msg::User("hi".into())], &fmt)
        .unwrap_err();
    assert!(matches!(err, ProviderError::Api { status: 400, .. }));
    _m.assert();
}

#[test]
fn openai_tool_schema_uses_function_parameters() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(json!({
            "tools": [{
                "type": "function",
                "function": {"name": "run_command", "parameters": {"type": "object"}}
            }]
        })))
        .with_status(200)
        .with_body(
            r#"{"choices":[{"message":{"content":null,"tool_calls":[
                {"id":"c1","type":"function","function":{"name":"run_command","arguments":"{\"command\":\"ls -la\"}"}}
            ]}}]}"#,
        )
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let completion = p
        .complete_with_tools("SYS", &[Msg::User("go".into())], &[tool()])
        .unwrap();
    assert_eq!(completion.tool_calls.len(), 1);
    // The string arguments must be parsed into JSON.
    assert_eq!(completion.tool_calls[0].arguments["command"], "ls -la");
    m.assert();
}

#[test]
fn openai_tool_result_uses_tool_role() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("POST", "/v1/chat/completions")
        .match_body(Matcher::PartialJson(json!({
            "messages": [
                {"role": "system"},
                {"role": "tool", "tool_call_id": "c0", "content": "exit 0"}
            ]
        })))
        .with_status(200)
        .with_body(r#"{"choices":[{"message":{"content":"done"}}]}"#)
        .create();

    let p = OpenAiProvider::new(server.url(), "k".into(), "m".into());
    let msgs = vec![Msg::ToolResult {
        call_id: "c0".into(),
        content: "exit 0".into(),
    }];
    let out = p.complete_with_tools("SYS", &msgs, &[]).unwrap();
    assert_eq!(out.text.as_deref(), Some("done"));
    m.assert();
}

// ---- Reachability probe (aishe doctor --probe) ----

/// A Config whose `openai` block points at `base_url` with no API key env set.
fn probe_config(base_url: &str) -> aishe::config::Config {
    let mut cfg = aishe::config::Config::default();
    cfg.aishe.provider = "openai".into();
    cfg.providers.openai.base_url = base_url.to_string();
    // A var name that is (almost certainly) unset, so the probe sends no auth.
    cfg.providers.openai.api_key_env = "AISHE_PROBE_NO_SUCH_KEY".into();
    cfg
}

#[test]
fn probe_reports_reachable_on_2xx() {
    let mut server = mockito::Server::new();
    let m = server
        .mock("GET", "/v1/models")
        .with_status(200)
        .with_body(r#"{"data":[]}"#)
        .create();
    let cfg = probe_config(&server.url());
    let pr = aishe::providers::probe(&cfg, "openai");
    assert!(
        matches!(pr.reach, aishe::providers::Reach::Up(200)),
        "got: {:?}",
        pr.reach
    );
    m.assert();
}

#[test]
fn probe_reports_unauthorized_on_401() {
    let mut server = mockito::Server::new();
    let _m = server
        .mock("GET", "/v1/models")
        .with_status(401)
        .with_body(r#"{"error":{"message":"bad key"}}"#)
        .create();
    let cfg = probe_config(&server.url());
    let pr = aishe::providers::probe(&cfg, "openai");
    assert!(
        matches!(pr.reach, aishe::providers::Reach::Unauthorized(401)),
        "got: {:?}",
        pr.reach
    );
}

#[test]
fn probe_reports_down_when_unreachable() {
    // A port nothing is listening on → transport error → Down. Port 1 is
    // privileged and unbound in test sandboxes.
    let cfg = probe_config("http://127.0.0.1:1");
    let pr = aishe::providers::probe(&cfg, "openai");
    assert!(
        matches!(pr.reach, aishe::providers::Reach::Down(_)),
        "got: {:?}",
        pr.reach
    );
}
