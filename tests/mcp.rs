//! Integration test for the MCP client against a real stdio server. Spawns a
//! tiny Python MCP server (newline-delimited JSON-RPC 2.0) and drives the full
//! path: handshake, tools/list, and tools/call. Skipped when python3 is absent.

use std::collections::BTreeMap;

use aishe::config::McpServerConfig;
use aishe::mcp::McpRegistry;
use serde_json::json;

/// A minimal MCP stdio server: `echo(text)` uppercases, `add(a,b)` sums.
const SERVER_PY: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
TOOLS = [
    {"name": "echo", "description": "Echo text uppercased.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
    {"name": "add", "description": "Add two integers.",
     "inputSchema": {"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}}}},
]
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    m = json.loads(line); mid = m.get("id"); method = m.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"t","version":"0"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":TOOLS}})
    elif method == "tools/call":
        p = m.get("params", {}); name = p.get("name"); a = p.get("arguments", {})
        if name == "echo":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":str(a.get("text","")).upper()}]}})
        elif name == "add":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":str(a.get("a",0)+a.get("b",0))}]}})
        else:
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"?"}],"isError":True}})
    elif mid is not None:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"no method"}})
"#;

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        std::process::Command::new(p)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

#[test]
fn mcp_handshake_list_and_call() {
    let Some(py) = python() else {
        eprintln!("skipping: python3 not available");
        return;
    };

    let dir = std::env::temp_dir().join(format!("aishe-mcp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let server = dir.join("server.py");
    std::fs::write(&server, SERVER_PY).unwrap();

    let mut configs = BTreeMap::new();
    configs.insert(
        "demo".to_string(),
        McpServerConfig {
            command: Some(py.to_string()),
            args: vec![server.to_string_lossy().to_string()],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
        },
    );

    let reg = McpRegistry::connect(&configs);
    assert!(!reg.is_empty(), "expected the MCP server to connect");

    // tools/list: both tools are exposed, namespaced.
    let names: Vec<String> = reg.tool_defs().into_iter().map(|d| d.name).collect();
    assert!(
        names.contains(&"mcp__demo__echo".to_string()),
        "got: {names:?}"
    );
    assert!(
        names.contains(&"mcp__demo__add".to_string()),
        "got: {names:?}"
    );

    // tools/call: echo uppercases, add sums.
    let (label, out) = reg.call("mcp__demo__echo", &json!({"text": "hello"}));
    assert_eq!(label, "demo:echo");
    assert_eq!(out, "HELLO");

    let (_, sum) = reg.call("mcp__demo__add", &json!({"a": 17, "b": 25}));
    assert_eq!(sum, "42");

    // Unknown tool is reported, not panicked.
    let (_, miss) = reg.call("mcp__demo__nope", &json!({}));
    assert!(miss.contains("unknown MCP tool"), "got: {miss}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn mcp_bad_command_is_skipped() {
    // A server whose command does not exist is skipped, leaving an empty registry
    // (no panic, no hang).
    let mut configs = BTreeMap::new();
    configs.insert(
        "broken".to_string(),
        McpServerConfig {
            command: Some("this-binary-does-not-exist-aishe".to_string()),
            args: vec![],
            env: BTreeMap::new(),
            url: None,
            headers: BTreeMap::new(),
            enabled: true,
        },
    );
    let reg = McpRegistry::connect(&configs);
    assert!(reg.is_empty());
}
