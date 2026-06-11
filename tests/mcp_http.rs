//! Integration test for the MCP Streamable HTTP transport against a tiny
//! localhost server. A Python `http.server` fixture answers JSON-RPC over POST:
//! `initialize` hands back an `Mcp-Session-Id` and the rest of the handshake,
//! `tools/list` advertises one tool, and `tools/call` replies as an SSE stream
//! (`text/event-stream`) so the SSE response path is exercised too. The server
//! also asserts the session id is echoed back on later requests. Skipped when
//! python3 is absent.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use aishe::config::McpServerConfig;
use aishe::mcp::McpRegistry;
use serde_json::json;

/// A minimal MCP Streamable HTTP server. Prints the chosen port on its first
/// stdout line, then serves JSON-RPC. `tools/call` of `echo` uppercases its
/// `text` argument and is returned as an SSE event; everything else is plain
/// JSON. It exits non-zero if a post-initialize request lacks the session id.
const SERVER_PY: &str = r#"
import sys, json
from http.server import BaseHTTPRequestHandler, HTTPServer

SESSION = "sess-abc-123"
TOOLS = [
    {"name": "echo", "description": "Echo text uppercased.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
]

class H(BaseHTTPRequestHandler):
    def log_message(self, *a):  # quiet
        pass
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        m = json.loads(self.rfile.read(n) or b"{}")
        mid = m.get("id"); method = m.get("method")
        sid = self.headers.get("Mcp-Session-Id")
        if method == "initialize":
            body = {"jsonrpc": "2.0", "id": mid,
                    "result": {"protocolVersion": "2025-06-18",
                               "capabilities": {"tools": {}},
                               "serverInfo": {"name": "t", "version": "0"}}}
            data = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Mcp-Session-Id", SESSION)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers(); self.wfile.write(data); return
        # Every non-initialize call must echo the session id back.
        if sid != SESSION:
            self.send_response(400); self.end_headers()
            self.wfile.write(b'{"error":{"message":"missing session"}}'); return
        if method == "notifications/initialized":
            self.send_response(202); self.end_headers(); return
        if method == "tools/list":
            body = {"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}}
            data = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers(); self.wfile.write(data); return
        if method == "tools/call":
            p = m.get("params", {}); a = p.get("arguments", {})
            text = str(a.get("text", "")).upper()
            res = {"jsonrpc": "2.0", "id": mid,
                   "result": {"content": [{"type": "text", "text": text}]}}
            # Reply as an SSE stream with an unrelated event first.
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            self.wfile.write(b"event: message\n")
            self.wfile.write(b'data: {"jsonrpc":"2.0","method":"notifications/progress"}\n\n')
            self.wfile.write(("data: " + json.dumps(res) + "\n\n").encode())
            self.wfile.flush(); return
        body = {"jsonrpc": "2.0", "id": mid,
                "error": {"code": -32601, "message": "no method"}}
        data = json.dumps(body).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers(); self.wfile.write(data)

srv = HTTPServer(("127.0.0.1", 0), H)
print(srv.server_address[1], flush=True)
srv.serve_forever()
"#;

fn python() -> Option<&'static str> {
    ["python3", "python"].into_iter().find(|p| {
        Command::new(p)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    })
}

/// A spawned fixture server, killed on drop.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start the fixture and read the port it printed on stdout.
fn start(py: &str, script: &std::path::Path) -> Option<Server> {
    let mut child = Command::new(py)
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut line = String::new();
    while Instant::now() < deadline {
        line.clear();
        use std::io::Read;
        // Read the first line (the port). BufRead::read_line blocks until a
        // newline, which the server emits immediately after binding.
        if reader.read_line(&mut line).ok()? == 0 {
            // EOF before a port: the server failed to start.
            let _ = (&mut reader).bytes().count();
            return None;
        }
        if let Ok(port) = line.trim().parse::<u16>() {
            return Some(Server { child, port });
        }
    }
    None
}

#[test]
fn mcp_http_handshake_list_and_call() {
    let Some(py) = python() else {
        eprintln!("skipping: python3 not available");
        return;
    };

    let dir = std::env::temp_dir().join(format!("aishe-mcp-http-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER_PY).unwrap();

    let Some(server) = start(py, &script) else {
        eprintln!("skipping: fixture server did not start");
        std::fs::remove_dir_all(&dir).ok();
        return;
    };

    let mut configs = BTreeMap::new();
    configs.insert(
        "remote".to_string(),
        McpServerConfig {
            command: None,
            args: vec![],
            env: BTreeMap::new(),
            url: Some(format!("http://127.0.0.1:{}/mcp", server.port)),
            headers: BTreeMap::new(),
            enabled: true,
        },
    );

    let reg = McpRegistry::connect(&configs);
    assert!(!reg.is_empty(), "expected the HTTP MCP server to connect");

    // tools/list (plain JSON response): the tool is exposed, namespaced.
    let names: Vec<String> = reg.tool_defs().into_iter().map(|d| d.name).collect();
    assert!(
        names.contains(&"mcp__remote__echo".to_string()),
        "got: {names:?}"
    );

    // tools/call (SSE response): echo uppercases, the matching id is picked out
    // of the SSE stream past an unrelated notification event.
    let (label, out) = reg.call("mcp__remote__echo", &json!({"text": "hello"}));
    assert_eq!(label, "remote:echo");
    assert_eq!(out, "HELLO");

    drop(server);
    std::fs::remove_dir_all(&dir).ok();
}
