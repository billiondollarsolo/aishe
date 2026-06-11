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

/// An MCP stdio server that declares resources + prompts capabilities and serves
/// `resources/list`, `resources/read`, `prompts/list`, and `prompts/get`.
const SERVER_RP_PY: &str = r#"
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
RES = [{"uri": "mem://notes", "name": "notes", "description": "team notes"}]
PROMPTS = [{"name": "greet", "description": "greet someone",
            "arguments": [{"name": "who", "required": True}]}]
for line in sys.stdin:
    line = line.strip()
    if not line: continue
    m = json.loads(line); mid = m.get("id"); method = m.get("method")
    if method == "initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2025-06-18",
            "capabilities":{"tools":{},"resources":{},"prompts":{}},
            "serverInfo":{"name":"rp","version":"0"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[]}})
    elif method == "resources/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"resources":RES}})
    elif method == "resources/read":
        uri = m.get("params",{}).get("uri")
        send({"jsonrpc":"2.0","id":mid,"result":{"contents":[
            {"uri":uri,"mimeType":"text/plain","text":"the notes body"}]}})
    elif method == "prompts/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"prompts":PROMPTS}})
    elif method == "prompts/get":
        who = m.get("params",{}).get("arguments",{}).get("who","world")
        send({"jsonrpc":"2.0","id":mid,"result":{"messages":[
            {"role":"user","content":{"type":"text","text":"Say hello to "+who}}]}})
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
fn mcp_resources_and_prompts() {
    let Some(py) = python() else {
        eprintln!("skipping: python3 not available");
        return;
    };
    let dir = std::env::temp_dir().join(format!("aishe-mcp-rp-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let server = dir.join("rp.py");
    std::fs::write(&server, SERVER_RP_PY).unwrap();

    let mut configs = BTreeMap::new();
    configs.insert(
        "rp".to_string(),
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

    // Resource capability exposes the two synthetic tools.
    let names: Vec<String> = reg.tool_defs().into_iter().map(|d| d.name).collect();
    assert!(
        names.contains(&"mcp__rp__list_resources".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"mcp__rp__read_resource".to_string()),
        "{names:?}"
    );

    // list_resources returns the resource listing; read_resource returns the body.
    let (_, listing) = reg.call("mcp__rp__list_resources", &json!({}));
    assert!(listing.contains("mem://notes"), "{listing}");
    let (_, body) = reg.call("mcp__rp__read_resource", &json!({"uri": "mem://notes"}));
    assert_eq!(body, "the notes body");
    // A missing uri is reported, not panicked.
    let (_, miss) = reg.call("mcp__rp__read_resource", &json!({}));
    assert!(miss.contains("needs a `uri`"), "{miss}");

    // The prompt is exposed as `rp:greet` and fetched with positional -> named args.
    assert!(reg.is_prompt("rp:greet"));
    let listed: Vec<String> = reg.list_prompts().into_iter().map(|(n, _)| n).collect();
    assert!(listed.contains(&"rp:greet".to_string()), "{listed:?}");
    let text = reg.prompt_text("rp:greet", &["Ada"]).unwrap().unwrap();
    assert_eq!(text, "Say hello to Ada");
    assert!(reg.prompt_text("rp:nope", &[]).is_none());

    std::fs::remove_dir_all(&dir).ok();
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
