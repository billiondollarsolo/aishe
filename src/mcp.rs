//! Minimal MCP (Model Context Protocol) client. Spawns the stdio servers named
//! in `[mcp_servers]`, does the JSON-RPC handshake, lists their tools, and
//! proxies `tools/call`. Tools are exposed to the yolo loop under a namespaced
//! name (`mcp__<server>__<tool>`) so the whole MCP ecosystem plugs in alongside
//! the built-in tools.
//!
//! The transport is newline-delimited JSON-RPC 2.0 over the child's stdin/stdout
//! (the MCP stdio transport). Each server runs on its own reader thread; calls
//! are synchronous request/response, matched by id, with a timeout so a wedged
//! server can't hang the shell. Servers are killed when the registry drops.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::McpServerConfig;
use crate::providers::ToolDef;

/// MCP protocol version we advertise in the handshake. Servers negotiate down if
/// they only speak an older one.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// How long a single request waits for its matching response before giving up.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// A tool advertised by an MCP server (its real name, as the server knows it).
struct McpTool {
    name: String,
    description: String,
    schema: Value,
}

/// One connected MCP server: the child process, its stdin, and a channel fed by
/// a reader thread parsing the server's stdout lines into JSON values.
struct McpServer {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
    tools: Vec<McpTool>,
}

impl McpServer {
    /// Spawn the server process, run the initialize handshake, and load its tool
    /// list. Returns an error string on any failure (bad command, protocol
    /// error, timeout) so the caller can skip just this server.
    fn spawn(cfg: &McpServerConfig) -> Result<Self, String> {
        let mut cmd = Command::new(&cfg.command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn `{}`: {e}", cfg.command))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });

        let mut server = McpServer {
            child,
            stdin,
            rx,
            next_id: 1,
            tools: Vec::new(),
        };
        server.initialize()?;
        server.load_tools()?;
        Ok(server)
    }

    /// Write one JSON-RPC message followed by a newline.
    fn send(&mut self, msg: &Value) -> Result<(), String> {
        let line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush())
            .map_err(|e| e.to_string())
    }

    /// Send a request and wait (up to [`RPC_TIMEOUT`]) for the response with the
    /// matching id, ignoring notifications and unrelated messages in between.
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;

        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| format!("{method}: timed out"))?;
            match self.rx.recv_timeout(remaining) {
                Ok(v) => {
                    if v.get("id").and_then(value_id) != Some(id) {
                        continue; // notification or a different id
                    }
                    if let Some(err) = v.get("error") {
                        let msg = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        return Err(format!("{method}: {msg}"));
                    }
                    return Ok(v.get("result").cloned().unwrap_or(Value::Null));
                }
                Err(_) => return Err(format!("{method}: timed out")),
            }
        }
    }

    /// Send a notification (no id, no response expected).
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// The MCP handshake: `initialize`, then the `initialized` notification.
    fn initialize(&mut self) -> Result<(), String> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "aishe", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        self.notify("notifications/initialized", json!({}))
    }

    /// Fetch the server's tool list into `self.tools`.
    fn load_tools(&mut self) -> Result<(), String> {
        let result = self.request("tools/list", json!({}))?;
        let tools = result
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        for t in tools {
            let name = t
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let description = t
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let schema = t
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}}));
            self.tools.push(McpTool {
                name,
                description,
                schema,
            });
        }
        Ok(())
    }

    /// Invoke a tool by its real (server-side) name and render the result to text.
    fn call(&mut self, tool: &str, args: Value) -> Result<String, String> {
        let result = self.request("tools/call", json!({"name": tool, "arguments": args}))?;
        Ok(render_tool_result(&result))
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// JSON-RPC ids may be numbers or strings; we only issue numeric ids, so accept a
/// number (or a string that parses as one).
fn value_id(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

/// Reduce an MCP `tools/call` result to text for the model: concatenate `text`
/// content blocks, note non-text blocks, prefix errors. Falls back to raw JSON.
fn render_tool_result(result: &Value) -> String {
    let mut out = String::new();
    if let Some(content) = result.get("content").and_then(|c| c.as_array()) {
        for item in content {
            match item.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(t);
                    }
                }
                Some(other) => {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&format!("[{other} content omitted]"));
                }
                None => {}
            }
        }
    }
    if out.is_empty() {
        // No content array (or only structured output): hand back compact JSON.
        out = serde_json::to_string(result).unwrap_or_default();
    }
    if result
        .get("isError")
        .and_then(|e| e.as_bool())
        .unwrap_or(false)
    {
        format!("Error: {out}")
    } else {
        out
    }
}

/// Sanitize a namespaced tool name to the character set the model APIs accept
/// (`[A-Za-z0-9_-]`, capped length), so an unusual server/tool name can't produce
/// an invalid tool definition.
fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    s.truncate(64);
    s
}

/// All connected MCP servers and the routing table from each exposed tool name to
/// `(server, real tool)`. Cloneable tool defs are precomputed at connect time;
/// per-server call state lives behind a `Mutex`, so the registry is shared as
/// `&McpRegistry` (mirroring `SkillRegistry`) and calls don't need `&mut`.
#[derive(Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, Mutex<McpServer>>,
    defs: Vec<ToolDef>,
    /// exposed tool name -> (server name, real tool name)
    routes: BTreeMap<String, (String, String)>,
}

impl McpRegistry {
    /// Connect to every enabled configured server. Failures are reported and the
    /// server skipped; a missing/empty config yields an empty registry that spawns
    /// nothing.
    pub fn connect(configs: &BTreeMap<String, McpServerConfig>) -> Self {
        let mut reg = McpRegistry::default();
        for (name, cfg) in configs {
            if !cfg.enabled {
                continue;
            }
            match McpServer::spawn(cfg) {
                Ok(server) => {
                    for tool in &server.tools {
                        let exposed = sanitize(&format!("mcp__{name}__{}", tool.name));
                        reg.defs.push(ToolDef {
                            name: exposed.clone(),
                            description: tool.description.clone(),
                            schema: tool.schema.clone(),
                        });
                        reg.routes
                            .insert(exposed, (name.clone(), tool.name.clone()));
                    }
                    eprintln!(
                        "aishe: MCP server '{name}' connected ({} tools)",
                        server.tools.len()
                    );
                    reg.servers.insert(name.clone(), Mutex::new(server));
                }
                Err(e) => eprintln!("aishe: MCP server '{name}' unavailable: {e}"),
            }
        }
        reg
    }

    /// True if no MCP tools are available.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Tool definitions to offer the model (already namespaced and sanitized).
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }

    /// A short `server · tool` listing for `aishe mcp` / docs.
    pub fn list(&self) -> Vec<(String, String)> {
        self.defs
            .iter()
            .map(|d| (d.name.clone(), d.description.clone()))
            .collect()
    }

    /// Call a namespaced MCP tool. Returns `(audit label, content for the model)`.
    pub fn call(&self, exposed: &str, args: &Value) -> (String, String) {
        let Some((server_name, tool)) = self.routes.get(exposed) else {
            return (
                exposed.to_string(),
                format!("Error: unknown MCP tool '{exposed}'."),
            );
        };
        let label = format!("{server_name}:{tool}");
        match self.servers.get(server_name) {
            Some(m) => {
                let mut server = match m.lock() {
                    Ok(g) => g,
                    Err(_) => return (label, "Error: MCP server lock poisoned.".into()),
                };
                match server.call(tool, args.clone()) {
                    Ok(content) => (label, content),
                    Err(e) => (label, format!("Error: {e}")),
                }
            }
            None => (label, format!("Error: no MCP server '{server_name}'.")),
        }
    }
}

/// True if `name` is a namespaced MCP tool (offered by [`McpRegistry`]).
pub fn is_mcp_tool(name: &str) -> bool {
    name.starts_with("mcp__")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_text_content() {
        let r = json!({"content": [{"type": "text", "text": "hello"}, {"type": "text", "text": "world"}]});
        assert_eq!(render_tool_result(&r), "hello\nworld");
    }

    #[test]
    fn renders_error_flag() {
        let r = json!({"content": [{"type": "text", "text": "boom"}], "isError": true});
        assert_eq!(render_tool_result(&r), "Error: boom");
    }

    #[test]
    fn renders_non_text_and_empty() {
        let r = json!({"content": [{"type": "image", "data": "..."}]});
        assert_eq!(render_tool_result(&r), "[image content omitted]");
        // No content array: falls back to JSON.
        let r2 = json!({"structuredContent": {"x": 1}});
        assert!(render_tool_result(&r2).contains("structuredContent"));
    }

    #[test]
    fn sanitizes_tool_names() {
        assert_eq!(sanitize("mcp__fs__read_file"), "mcp__fs__read_file");
        assert_eq!(sanitize("mcp__a.b__do/it"), "mcp__a_b__do_it");
        assert_eq!(sanitize(&"x".repeat(100)).len(), 64);
    }

    #[test]
    fn is_mcp_tool_prefix() {
        assert!(is_mcp_tool("mcp__fs__read_file"));
        assert!(!is_mcp_tool("read_file"));
    }

    #[test]
    fn empty_registry_default() {
        let reg = McpRegistry::default();
        assert!(reg.is_empty());
        assert!(reg.tool_defs().is_empty());
        let (label, content) = reg.call("mcp__x__y", &json!({}));
        assert_eq!(label, "mcp__x__y");
        assert!(content.contains("unknown MCP tool"));
    }
}
