//! Minimal MCP (Model Context Protocol) client. Connects to the servers named
//! in `[mcp_servers]`, does the JSON-RPC handshake, lists their tools, and
//! proxies `tools/call`. Tools are exposed to the yolo loop under a namespaced
//! name (`mcp__<server>__<tool>`) so the whole MCP ecosystem plugs in alongside
//! the built-in tools.
//!
//! Two transports are supported behind a small [`Transport`] abstraction:
//!
//! - **stdio**: newline-delimited JSON-RPC 2.0 over a child process's
//!   stdin/stdout. Each server runs on its own reader thread; calls are
//!   synchronous request/response, matched by id, with a timeout so a wedged
//!   server can't hang the shell. The child is killed when the registry drops.
//! - **Streamable HTTP**: JSON-RPC 2.0 POSTed to a URL, with the response
//!   delivered either as a single JSON object or as a `text/event-stream` (SSE)
//!   we read until the matching id arrives. The `Mcp-Session-Id` returned by
//!   `initialize` is echoed on later requests.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
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

/// How long a single stdio request waits for its matching response before giving up.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP connect timeout for the Streamable HTTP transport.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP read timeout for the Streamable HTTP transport (also bounds how long we
/// wait on an SSE stream for the matching response).
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Streamable-HTTP JSON-RPC responses share the stdio transport's message cap.
/// The extra byte distinguishes an exactly-at-limit body from an oversized one.
const MAX_HTTP_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Ceiling on a single newline-delimited JSON-RPC message from a stdio server.
/// Bounds the reader thread's memory against a server that never emits a
/// newline; see [`drain_jsonrpc`] for what happens to a message this large.
const MAX_RPC_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// Drain a stdio server's stdout, forwarding every parseable JSON-RPC message to
/// `tx`. Returns on EOF, on a real IO error, or once the receiver is gone.
///
/// Reads raw bytes and decodes lossily instead of using `BufRead::lines()`:
/// `lines()` is UTF-8-only and yields `Err(InvalidData)` for a single stray byte
/// anywhere in the stream (a latin-1 filename echoed into a server's log line is
/// enough). The old `let Ok(line) = line else { break }` turned that into reader
/// thread death, which drops `ChildStdout`; the server then takes SIGPIPE on its
/// next write and — because nothing marks the transport dead — every later
/// `request()` merely waits out [`RPC_TIMEOUT`]. Undecodable bytes now become
/// U+FFFD inside one message, which at worst fails that one message's JSON parse
/// while the connection keeps working.
///
/// ponytail: a message longer than [`MAX_RPC_LINE_BYTES`] is split rather than
/// buffered whole; the fragments fail to parse and are skipped, so an absurdly
/// large response is lost and its caller times out instead of exhausting RAM.
/// Upgrade path: a streaming JSON parser fed directly from the pipe.
fn drain_jsonrpc<R: Read>(reader: R, tx: mpsc::Sender<Value>) {
    let mut buf = BufReader::new(reader);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        // The limit is re-armed each iteration, so `Ok(0)` still means genuine EOF.
        match (&mut buf)
            .take(MAX_RPC_LINE_BYTES)
            .read_until(b'\n', &mut raw)
        {
            Ok(0) => return, // EOF: the server closed its stdout
            Ok(_) => {}
            Err(_) => return, // pipe closed / real IO error
        }
        let text = String::from_utf8_lossy(&raw);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            if tx.send(v).is_err() {
                return; // transport dropped
            }
        }
    }
}

/// A tool advertised by an MCP server (its real name, as the server knows it).
struct McpTool {
    name: String,
    description: String,
    schema: Value,
}

/// A prompt advertised by an MCP server (`prompts/list`).
struct McpPrompt {
    name: String,
    description: String,
    /// Declared argument names, in order, for mapping positional slash args.
    arg_names: Vec<String>,
}

/// The stdio transport: a child process, its stdin, and a channel fed by a
/// reader thread parsing the server's stdout lines into JSON values.
struct StdioTransport {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<Value>,
    next_id: u64,
}

impl StdioTransport {
    /// Spawn the server process and start its stdout reader thread.
    fn spawn(cfg: &McpServerConfig) -> Result<Self, String> {
        let command = cfg
            .command
            .as_deref()
            .ok_or("stdio server has no `command`")?;
        let mut cmd = Command::new(command);
        cmd.args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in &cfg.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| format!("spawn `{command}`: {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || drain_jsonrpc(stdout, tx));

        Ok(StdioTransport {
            child,
            stdin,
            rx,
            next_id: 1,
        })
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
                    return result_from_response(method, &v);
                }
                Err(_) => return Err(format!("{method}: timed out")),
            }
        }
    }

    /// Send a notification (no id, no response expected).
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Streamable HTTP transport: JSON-RPC POSTed to a URL. Each request opens a
/// fresh HTTP call whose body is one JSON-RPC object; the reply comes back as a
/// single JSON object or as an SSE stream we scan for the matching id.
struct HttpTransport {
    agent: ureq::Agent,
    url: String,
    headers: BTreeMap<String, String>,
    /// `Mcp-Session-Id` captured from the `initialize` response, echoed on later
    /// requests once known.
    session_id: Option<String>,
    next_id: u64,
}

impl HttpTransport {
    fn new(cfg: &McpServerConfig) -> Result<Self, String> {
        let url = cfg.url.as_deref().ok_or("HTTP server has no `url`")?;
        let agent = crate::providers::external_http_agent(
            HTTP_CONNECT_TIMEOUT,
            None,
            Some(HTTP_READ_TIMEOUT),
            Some(HTTP_READ_TIMEOUT),
        );
        Ok(HttpTransport {
            agent,
            url: url.to_string(),
            headers: cfg.headers.clone(),
            session_id: None,
            next_id: 1,
        })
    }

    /// Build a POST request with the standard MCP headers plus any configured
    /// extras and the session id (when known).
    fn post(&self) -> ureq::RequestBuilder<ureq::typestate::WithBody> {
        let mut req = self
            .agent
            .post(&self.url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (k, v) in &self.headers {
            req = req.header(k, v);
        }
        if let Some(sid) = &self.session_id {
            req = req.header("Mcp-Session-Id", sid);
        }
        req
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let resp = self
            .post()
            .send_json(body)
            .map_err(|e| format!("{method}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("{method}: {}", http_response_error(resp)));
        }

        // The session id is established by the initialize response; remember it.
        if self.session_id.is_none() {
            if let Some(sid) = resp
                .headers()
                .get("Mcp-Session-Id")
                .and_then(|value| value.to_str().ok())
            {
                if !sid.is_empty() {
                    self.session_id = Some(sid.to_string());
                }
            }
        }

        let content_type = resp
            .headers()
            .get("Content-Type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let mut reader = BufReader::new(resp.into_body().into_reader());
        let mut text = String::new();
        (&mut reader)
            .take(MAX_HTTP_BODY_BYTES + 1)
            .read_to_string(&mut text)
            .map_err(|e| format!("{method}: {e}"))?;
        if text.len() as u64 > MAX_HTTP_BODY_BYTES {
            return Err(format!(
                "{method}: HTTP response exceeded the {MAX_HTTP_BODY_BYTES}-byte limit"
            ));
        }
        parse_response_body(method, id, &text, &content_type)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({"jsonrpc": "2.0", "method": method, "params": params});
        // A notification has no id; any 2xx (typically 202 Accepted) is success.
        // The shared ureq agent deliberately leaves HTTP status handling to us
        // so a bounded error body can be included in the diagnostic.
        let response = self
            .post()
            .send_json(body)
            .map_err(|e| format!("{method}: {e}"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(format!("{method}: {}", http_response_error(response)))
        }
    }
}

/// The per-server transport. Both variants present the same request/notify API to
/// [`McpServer`], so the handshake and tool routing are transport-agnostic.
enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl Transport {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        match self {
            Transport::Stdio(t) => t.request(method, params),
            Transport::Http(t) => t.request(method, params),
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        match self {
            Transport::Stdio(t) => t.notify(method, params),
            Transport::Http(t) => t.notify(method, params),
        }
    }
}

/// One connected MCP server: its transport plus the loaded tool/prompt lists and
/// declared capabilities. The handshake and routing are identical across
/// transports.
struct McpServer {
    transport: Transport,
    tools: Vec<McpTool>,
    prompts: Vec<McpPrompt>,
    /// The server declared a `resources` capability (so `resources/list` /
    /// `resources/read` are available).
    has_resources: bool,
}

impl McpServer {
    /// Connect to the server, run the initialize handshake, and load its tools and
    /// prompts. A server with a `url` uses the HTTP transport; otherwise it spawns
    /// a stdio child. Returns an error string on any failure (bad command, network
    /// error, protocol error, timeout) so the caller can skip just this server.
    fn connect(cfg: &McpServerConfig) -> Result<Self, String> {
        let transport = if cfg.url.is_some() {
            Transport::Http(HttpTransport::new(cfg)?)
        } else if cfg.command.is_some() {
            Transport::Stdio(StdioTransport::spawn(cfg)?)
        } else {
            return Err("no `command` or `url` configured".to_string());
        };
        let mut server = McpServer {
            transport,
            tools: Vec::new(),
            prompts: Vec::new(),
            has_resources: false,
        };
        let caps = server.initialize()?;
        server.has_resources = caps.get("resources").is_some();
        server.load_tools()?;
        // Prompts are optional; a server that advertises the capability but errors
        // on `prompts/list` is tolerated (no prompts).
        if caps.get("prompts").is_some() {
            server.load_prompts();
        }
        Ok(server)
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.transport.request(method, params)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.transport.notify(method, params)
    }

    /// The MCP handshake: `initialize` (returning the server's declared
    /// capabilities), then the `initialized` notification.
    fn initialize(&mut self) -> Result<Value, String> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "aishe", "version": env!("CARGO_PKG_VERSION")}
            }),
        )?;
        self.notify("notifications/initialized", json!({}))?;
        Ok(result.get("capabilities").cloned().unwrap_or(Value::Null))
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

    /// Fetch the server's prompt list into `self.prompts` (best-effort).
    fn load_prompts(&mut self) {
        let Ok(result) = self.request("prompts/list", json!({})) else {
            return;
        };
        let prompts = result
            .get("prompts")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();
        for p in prompts {
            let name = p
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            if name.is_empty() {
                continue;
            }
            let description = p
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            let arg_names = p
                .get("arguments")
                .and_then(|a| a.as_array())
                .map(|args| {
                    args.iter()
                        .filter_map(|a| a.get("name").and_then(|n| n.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default();
            self.prompts.push(McpPrompt {
                name,
                description,
                arg_names,
            });
        }
    }

    /// `resources/list`, rendered as a `uri  name - description` listing.
    fn list_resources(&mut self) -> Result<String, String> {
        let result = self.request("resources/list", json!({}))?;
        let resources = result
            .get("resources")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        if resources.is_empty() {
            return Ok("(no resources)".to_string());
        }
        let mut out = String::new();
        for r in resources {
            let uri = r.get("uri").and_then(|u| u.as_str()).unwrap_or("");
            let name = r.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let desc = r.get("description").and_then(|d| d.as_str()).unwrap_or("");
            out.push_str(uri);
            if !name.is_empty() {
                out.push_str(&format!("  {name}"));
            }
            if !desc.is_empty() {
                out.push_str(&format!(" - {desc}"));
            }
            out.push('\n');
        }
        Ok(out.trim_end().to_string())
    }

    /// `resources/read`, rendered as the concatenated text of its contents.
    fn read_resource(&mut self, uri: &str) -> Result<String, String> {
        let result = self.request("resources/read", json!({"uri": uri}))?;
        Ok(render_resource_contents(&result))
    }

    /// `prompts/get`, rendered as the flattened text of the returned messages.
    fn get_prompt(&mut self, name: &str, args: Value) -> Result<String, String> {
        let result = self.request("prompts/get", json!({"name": name, "arguments": args}))?;
        Ok(render_prompt_messages(&result))
    }
}

/// Extract the `result` from a parsed JSON-RPC response object, or turn its
/// `error` into an error string prefixed with the method name.
fn result_from_response(method: &str, v: &Value) -> Result<Value, String> {
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        return Err(format!("{method}: {msg}"));
    }
    Ok(v.get("result").cloned().unwrap_or(Value::Null))
}

/// Parse an HTTP transport response body into a JSON-RPC result. A
/// `text/event-stream` content type is scanned for the SSE event whose JSON-RPC
/// id matches `id`; any other (e.g. `application/json`) body is parsed as a
/// single JSON-RPC response object.
fn parse_response_body(
    method: &str,
    id: u64,
    body: &str,
    content_type: &str,
) -> Result<Value, String> {
    if content_type
        .to_ascii_lowercase()
        .contains("text/event-stream")
    {
        let msg = sse_message_for_id(body, id)
            .ok_or_else(|| format!("{method}: no SSE response for id {id}"))?;
        result_from_response(method, &msg)
    } else {
        let v: Value = serde_json::from_str(body.trim()).map_err(|e| format!("{method}: {e}"))?;
        result_from_response(method, &v)
    }
}

/// Scan an SSE body for the `data:` event carrying a JSON-RPC message whose id
/// matches `id`, ignoring notifications and unrelated ids. SSE allows a single
/// event to span multiple `data:` lines; we join them per event (separated by a
/// blank line) before parsing.
fn sse_message_for_id(body: &str, id: u64) -> Option<Value> {
    let mut data = String::new();
    let consider = |data: &str| -> Option<Value> {
        let trimmed = data.trim();
        if trimmed.is_empty() {
            return None;
        }
        let v: Value = serde_json::from_str(trimmed).ok()?;
        if v.get("id").and_then(value_id) == Some(id) {
            Some(v)
        } else {
            None
        }
    };
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.trim().is_empty() {
            // Event boundary: try the accumulated data, then reset.
            if let Some(v) = consider(&data) {
                return Some(v);
            }
            data.clear();
        }
    }
    consider(&data)
}

/// Render a non-success response, reading at most the MCP body ceiling and
/// preserving a JSON-RPC error message when the server provides one.
fn http_response_error(mut response: crate::providers::HttpResponse) -> String {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_HTTP_BODY_BYTES)
        .read_to_string()
        .unwrap_or_default();
    let detail = serde_json::from_str::<Value>(body.trim())
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|er| er.get("message"))
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| body.trim().chars().take(200).collect());
    if detail.is_empty() {
        format!("HTTP {status}")
    } else {
        format!("HTTP {status}: {detail}")
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

/// Reduce a `resources/read` result to text: concatenate the `text` of each
/// content entry, noting binary (`blob`) entries. Falls back to compact JSON.
fn render_resource_contents(result: &Value) -> String {
    let mut out = String::new();
    if let Some(contents) = result.get("contents").and_then(|c| c.as_array()) {
        for item in contents {
            if let Some(t) = item.get("text").and_then(|t| t.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            } else if item.get("blob").is_some() {
                if !out.is_empty() {
                    out.push('\n');
                }
                let uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                out.push_str(&format!("[binary resource {uri} omitted]"));
            }
        }
    }
    if out.is_empty() {
        out = serde_json::to_string(result).unwrap_or_default();
    }
    out
}

/// Reduce a `prompts/get` result to a single text prompt: flatten each message's
/// text content, prefixed by role when not `user`.
fn render_prompt_messages(result: &Value) -> String {
    let mut out = String::new();
    if let Some(messages) = result.get("messages").and_then(|m| m.as_array()) {
        for msg in messages {
            let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let text = match msg.get("content") {
                // Content may be a single object or an array of content blocks.
                Some(Value::Object(_)) => msg
                    .get("content")
                    .and_then(|c| c.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string(),
                Some(Value::Array(blocks)) => blocks
                    .iter()
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n"),
                _ => String::new(),
            };
            if text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            if role != "user" {
                out.push_str(&format!("[{role}]\n"));
            }
            out.push_str(&text);
        }
    }
    out
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

/// What an exposed yolo tool name maps to on a server.
enum RouteKind {
    /// A real server tool, called via `tools/call` with this name.
    Tool(String),
    /// The synthetic `list_resources` tool (`resources/list`).
    ListResources,
    /// The synthetic `read_resource` tool (`resources/read`, arg `uri`).
    ReadResource,
}

/// An exposed yolo tool's routing target.
struct Route {
    server: String,
    kind: RouteKind,
}

/// An MCP prompt exposed as a `/<server>:<prompt>` slash-command.
struct PromptRoute {
    server: String,
    prompt: String,
    description: String,
    arg_names: Vec<String>,
}

/// All connected MCP servers and the routing tables: from each exposed yolo tool
/// name to a [`Route`], and from each `server:prompt` slash name to a
/// [`PromptRoute`]. Cloneable tool defs are precomputed at connect time;
/// per-server call state lives behind a `Mutex`, so the registry is shared as
/// `&McpRegistry` (mirroring `SkillRegistry`) and calls don't need `&mut`.
#[derive(Default)]
pub struct McpRegistry {
    servers: BTreeMap<String, Mutex<McpServer>>,
    defs: Vec<ToolDef>,
    routes: BTreeMap<String, Route>,
    prompts: BTreeMap<String, PromptRoute>,
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
            match McpServer::connect(cfg) {
                Ok(server) => {
                    for tool in &server.tools {
                        let exposed = sanitize(&format!("mcp__{name}__{}", tool.name));
                        reg.defs.push(ToolDef {
                            name: exposed.clone(),
                            description: tool.description.clone(),
                            schema: tool.schema.clone(),
                        });
                        reg.routes.insert(
                            exposed,
                            Route {
                                server: name.clone(),
                                kind: RouteKind::Tool(tool.name.clone()),
                            },
                        );
                    }
                    // Synthetic resource tools when the server supports resources.
                    if server.has_resources {
                        reg.add_resource_tools(name);
                    }
                    // Prompts become `/<server>:<prompt>` slash-commands.
                    for p in &server.prompts {
                        let exposed = format!("{name}:{}", p.name);
                        reg.prompts.insert(
                            exposed,
                            PromptRoute {
                                server: name.clone(),
                                prompt: p.name.clone(),
                                description: p.description.clone(),
                                arg_names: p.arg_names.clone(),
                            },
                        );
                    }
                    eprintln!(
                        "aishe: MCP server '{name}' connected ({} tools{}{})",
                        server.tools.len(),
                        if server.has_resources {
                            ", resources"
                        } else {
                            ""
                        },
                        if server.prompts.is_empty() {
                            String::new()
                        } else {
                            format!(", {} prompts", server.prompts.len())
                        },
                    );
                    reg.servers.insert(name.clone(), Mutex::new(server));
                }
                Err(e) => eprintln!("aishe: MCP server '{name}' unavailable: {e}"),
            }
        }
        reg
    }

    /// Register the two synthetic resource tools for `server`.
    fn add_resource_tools(&mut self, server: &str) {
        let list_name = sanitize(&format!("mcp__{server}__list_resources"));
        self.defs.push(ToolDef {
            name: list_name.clone(),
            description: format!("List the resources offered by the '{server}' MCP server."),
            schema: json!({"type": "object", "properties": {}}),
        });
        self.routes.insert(
            list_name,
            Route {
                server: server.to_string(),
                kind: RouteKind::ListResources,
            },
        );
        let read_name = sanitize(&format!("mcp__{server}__read_resource"));
        self.defs.push(ToolDef {
            name: read_name.clone(),
            description: format!(
                "Read a resource from the '{server}' MCP server by its uri (see list_resources)."
            ),
            schema: json!({
                "type": "object",
                "properties": {"uri": {"type": "string", "description": "the resource uri"}},
                "required": ["uri"]
            }),
        });
        self.routes.insert(
            read_name,
            Route {
                server: server.to_string(),
                kind: RouteKind::ReadResource,
            },
        );
    }

    /// True if no MCP tools are available.
    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// True if nothing at all is available (no tools and no prompts).
    pub fn is_fully_empty(&self) -> bool {
        self.defs.is_empty() && self.prompts.is_empty()
    }

    /// Tool definitions to offer the model (already namespaced and sanitized).
    pub fn tool_defs(&self) -> Vec<ToolDef> {
        self.defs.clone()
    }

    /// A short `tool · description` listing for `aishe mcp` / docs.
    pub fn list(&self) -> Vec<(String, String)> {
        self.defs
            .iter()
            .map(|d| (d.name.clone(), d.description.clone()))
            .collect()
    }

    /// The exposed prompt commands (`server:prompt`) and their descriptions.
    pub fn list_prompts(&self) -> Vec<(String, String)> {
        self.prompts
            .iter()
            .map(|(name, r)| (name.clone(), r.description.clone()))
            .collect()
    }

    /// True if `name` is an exposed MCP prompt command (`server:prompt`).
    pub fn is_prompt(&self, name: &str) -> bool {
        self.prompts.contains_key(name)
    }

    /// Fetch an MCP prompt by its `server:prompt` name, mapping the positional
    /// `args` to the prompt's declared argument names. Returns the rendered prompt
    /// text to run as a request, or an error string. `None` if unknown.
    pub fn prompt_text(&self, name: &str, args: &[&str]) -> Option<Result<String, String>> {
        let route = self.prompts.get(name)?;
        let mut named = serde_json::Map::new();
        for (i, key) in route.arg_names.iter().enumerate() {
            if let Some(v) = args.get(i) {
                named.insert(key.clone(), Value::String((*v).to_string()));
            }
        }
        let server = self.servers.get(&route.server)?;
        let mut guard = match server.lock() {
            Ok(g) => g,
            Err(_) => return Some(Err("MCP server lock poisoned.".into())),
        };
        Some(guard.get_prompt(&route.prompt, Value::Object(named)))
    }

    /// Call a namespaced MCP tool. Returns `(audit label, content for the model)`.
    pub fn call(&self, exposed: &str, args: &Value) -> (String, String) {
        let Some(route) = self.routes.get(exposed) else {
            return (
                exposed.to_string(),
                format!("Error: unknown MCP tool '{exposed}'."),
            );
        };
        let label = match &route.kind {
            RouteKind::Tool(t) => format!("{}:{t}", route.server),
            RouteKind::ListResources => format!("{}:list_resources", route.server),
            RouteKind::ReadResource => format!("{}:read_resource", route.server),
        };
        let Some(m) = self.servers.get(&route.server) else {
            return (label, format!("Error: no MCP server '{}'.", route.server));
        };
        let mut server = match m.lock() {
            Ok(g) => g,
            Err(_) => return (label, "Error: MCP server lock poisoned.".into()),
        };
        let result = match &route.kind {
            RouteKind::Tool(t) => server.call(t, args.clone()),
            RouteKind::ListResources => server.list_resources(),
            RouteKind::ReadResource => {
                let uri = args.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                if uri.is_empty() {
                    Err("read_resource needs a `uri`".to_string())
                } else {
                    server.read_resource(uri)
                }
            }
        };
        match result {
            Ok(content) => (label, content),
            Err(e) => (label, format!("Error: {e}")),
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
    fn jsonrpc_drain_survives_invalid_utf8() {
        // A latin-1 byte between two JSON-RPC messages. The old `.lines()` reader
        // died on it, dropping ChildStdout and wedging every later request();
        // the byte drainer must deliver the message that follows.
        let bytes: Vec<u8> = b"{\"id\":1}\n\xFF log line\n{\"id\":2}\n".to_vec();
        let (tx, rx) = mpsc::channel();
        drain_jsonrpc(std::io::Cursor::new(bytes), tx);
        assert_eq!(rx.recv().unwrap()["id"], json!(1));
        assert_eq!(rx.recv().unwrap()["id"], json!(2));
        assert!(rx.recv().is_err(), "channel should close at EOF");
    }

    #[test]
    fn jsonrpc_drain_bounds_a_newline_free_stream() {
        // A server that never emits a newline must not be able to grow the reader
        // thread's buffer without limit.
        let bytes = vec![b'x'; MAX_RPC_LINE_BYTES as usize + 16];
        let (tx, rx) = mpsc::channel();
        drain_jsonrpc(std::io::Cursor::new(bytes), tx);
        // Nothing parses, and the drain still terminates at EOF.
        assert!(rx.recv().is_err());
    }

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
    fn renders_resource_contents() {
        let r = json!({"contents": [
            {"uri": "file:///a", "text": "line one"},
            {"uri": "file:///b", "text": "line two"}
        ]});
        assert_eq!(render_resource_contents(&r), "line one\nline two");
        // A blob entry is noted, not dumped.
        let b = json!({"contents": [{"uri": "file:///img.png", "blob": "AAAA"}]});
        assert!(render_resource_contents(&b).contains("binary resource file:///img.png"));
    }

    #[test]
    fn renders_prompt_messages() {
        // Array-of-blocks and single-object content, with a non-user role prefix.
        let p = json!({"messages": [
            {"role": "user", "content": [{"type": "text", "text": "Summarize this:"}]},
            {"role": "assistant", "content": {"type": "text", "text": "ok"}}
        ]});
        let out = render_prompt_messages(&p);
        assert!(out.contains("Summarize this:"));
        assert!(out.contains("[assistant]"));
        assert!(out.contains("ok"));
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
    fn parses_json_response_body() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let r = parse_response_body("tools/list", 1, body, "application/json").unwrap();
        assert_eq!(r, json!({"tools": []}));
    }

    #[test]
    fn parses_json_response_with_charset() {
        // A Content-Type carrying parameters is still treated as JSON.
        let body = r#"{"jsonrpc":"2.0","id":7,"result":{"ok":true}}"#;
        let r = parse_response_body("x", 7, body, "application/json; charset=utf-8").unwrap();
        assert_eq!(r, json!({"ok": true}));
    }

    #[test]
    fn json_error_response_is_an_err() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"no method"}}"#;
        let e = parse_response_body("tools/call", 1, body, "application/json").unwrap_err();
        assert!(e.contains("no method"), "got: {e}");
    }

    #[test]
    fn picks_sse_event_with_matching_id() {
        // Multiple data events; only id 5 matches, and notifications/other ids are
        // skipped.
        let body = "event: message\n\
                    data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}\n\
                    \n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":4,\"result\":{\"wrong\":true}}\n\
                    \n\
                    data: {\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{\"right\":true}}\n\
                    \n";
        let r = parse_response_body("tools/call", 5, body, "text/event-stream").unwrap();
        assert_eq!(r, json!({"right": true}));
    }

    #[test]
    fn sse_extracts_by_id_directly() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"value\":42}}\n\n";
        let v = sse_message_for_id(body, 2).unwrap();
        assert_eq!(v.get("result").unwrap(), &json!({"value": 42}));
        // No event matches a different id.
        assert!(sse_message_for_id(body, 99).is_none());
    }

    #[test]
    fn sse_missing_id_is_an_err() {
        let body = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        let e = parse_response_body("tools/list", 2, body, "text/event-stream").unwrap_err();
        assert!(e.contains("no SSE response"), "got: {e}");
    }

    #[test]
    fn sse_handles_multiline_data() {
        // A single event split across two data: lines is joined before parsing.
        let body = "data: {\"jsonrpc\":\"2.0\",\n\
                    data: \"id\":3,\"result\":{\"ok\":1}}\n\n";
        let v = sse_message_for_id(body, 3).unwrap();
        assert_eq!(v.get("result").unwrap(), &json!({"ok": 1}));
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
