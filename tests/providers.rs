//! Provider HTTP tests using mockito, asserting request shapes for both APIs.

use llmsh::providers::anthropic::AnthropicProvider;
use llmsh::providers::openai_compat::OpenAiProvider;
use llmsh::providers::{AssistantMsg, Msg, Provider, ProviderError, ToolCall, ToolDef};
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
    let out = p.complete("SYS", &[Msg::User("hi".into())], true).unwrap();
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
        .complete("s", &[Msg::User("x".into())], false)
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
    // Expect two calls: original + one retry.
    let m = server
        .mock("POST", "/v1/messages")
        .with_status(429)
        .with_body(r#"{"error":{"message":"rate limited"}}"#)
        .expect(2)
        .create();

    let p = AnthropicProvider::new(server.url(), "k".into(), "m".into());
    let err = p
        .complete("s", &[Msg::User("x".into())], false)
        .unwrap_err();
    assert!(matches!(err, ProviderError::Api { status: 429, .. }));
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
    let out = p.complete("SYS", &[Msg::User("hi".into())], true).unwrap();
    assert!(out.contains("ok"));
    m.assert();
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
