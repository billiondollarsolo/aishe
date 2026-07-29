//! Strict authenticated loopback control protocol for the private backend
//! supervisor. This is intentionally small and versioned; OpenCode's public API
//! remains on a separate independently authenticated listener.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backend::bridge::{
    Bridge, BridgeFailure, ChildRegistration, LeaseIdentity, LeaseRegistration, PluginToolRequest,
    ProviderTurnRequest, ProviderUsageReport, ToolCompletion, ToolStarted, ToolWork,
};
use crate::backend::opencode::OpenCodeConnection;

pub const SUPERVISOR_PROTOCOL_VERSION: u32 = 2;
const STATE_SCHEMA_VERSION: u32 = 2;
const MAX_HEADER_BYTES: usize = 32 * 1024;
// Tool schemas permit a 4 MiB replacement or patch. Leave bounded envelope
// room for JSON escaping, metadata, and the other small request fields.
const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize)]
pub struct SupervisorState {
    pub schema_version: u32,
    pub protocol_version: u32,
    pub supervisor_pid: u32,
    pub opencode_pid: u32,
    pub control_url: String,
    pub opencode_url: String,
    pub runtime_version: String,
    pub plugin_sha256: String,
    pub provider_id: String,
    pub model_id: String,
    pub startup_nonce: String,
    pub started_at_ms: u128,
    pub control_token: String,
    pub opencode_password: String,
}

impl SupervisorState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        supervisor_pid: u32,
        opencode_pid: u32,
        control_url: String,
        opencode_url: String,
        runtime_version: String,
        plugin_sha256: String,
        provider_id: String,
        model_id: String,
        startup_nonce: String,
        started_at_ms: u128,
        control_token: String,
        opencode_password: String,
    ) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            protocol_version: SUPERVISOR_PROTOCOL_VERSION,
            supervisor_pid,
            opencode_pid,
            control_url,
            opencode_url,
            runtime_version,
            plugin_sha256,
            provider_id,
            model_id,
            startup_nonce,
            started_at_ms,
            control_token,
            opencode_password,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != STATE_SCHEMA_VERSION {
            anyhow::bail!("backend state schema mismatch");
        }
        if self.protocol_version != SUPERVISOR_PROTOCOL_VERSION {
            anyhow::bail!("backend supervisor protocol mismatch");
        }
        validate_loopback_url(&self.control_url)?;
        validate_loopback_url(&self.opencode_url)?;
        if self.control_token.len() != 64
            || self.opencode_password.len() != 64
            || self.startup_nonce.len() != 64
            || self.plugin_sha256.len() != 64
        {
            anyhow::bail!("backend state contains invalid private identities");
        }
        Ok(())
    }

    pub fn opencode_connection(&self) -> OpenCodeConnection {
        OpenCodeConnection {
            base_url: self.opencode_url.clone(),
            username: "aishe".into(),
            password: self.opencode_password.clone(),
            version: self.runtime_version.clone(),
        }
    }
}

#[derive(Clone)]
pub struct SupervisorClient {
    state: SupervisorState,
}

impl SupervisorClient {
    pub fn new(state: SupervisorState) -> Result<Self> {
        state.validate()?;
        Ok(Self { state })
    }

    pub fn opencode_connection(&self) -> OpenCodeConnection {
        self.state.opencode_connection()
    }

    pub fn provider_id(&self) -> &str {
        &self.state.provider_id
    }

    pub fn model_id(&self) -> &str {
        &self.state.model_id
    }

    pub fn register(&self, registration: &LeaseRegistration) -> Result<LeaseIdentity> {
        self.post("/v1/lease/register", registration, Duration::from_secs(5))
    }

    pub fn heartbeat(&self, identity: &LeaseIdentity) -> Result<()> {
        let _: Value = self.post("/v1/lease/heartbeat", identity, Duration::from_secs(5))?;
        Ok(())
    }

    pub fn next(&self, identity: &LeaseIdentity) -> Result<Option<ToolWork>> {
        self.post("/v1/lease/next", identity, Duration::from_secs(30))
    }

    pub fn started(&self, started: &ToolStarted) -> Result<()> {
        let _: Value = self.post("/v1/lease/started", started, Duration::from_secs(5))?;
        Ok(())
    }

    pub fn complete(&self, completion: &ToolCompletion) -> Result<()> {
        let _: Value = self.post("/v1/lease/complete", completion, Duration::from_secs(10))?;
        Ok(())
    }

    pub fn unregister(&self, identity: &LeaseIdentity) -> Result<()> {
        let _: Value = self.post("/v1/lease/unregister", identity, Duration::from_secs(5))?;
        Ok(())
    }

    fn post<T: Serialize, U: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &T,
        timeout: Duration,
    ) -> Result<U> {
        let url = format!("{}{}", self.state.control_url.trim_end_matches('/'), path);
        let agent = ureq::AgentBuilder::new().redirects(0).build();
        let response = agent
            .post(&url)
            .set(
                "Authorization",
                &format!("Bearer {}", self.state.control_token),
            )
            .set("Content-Type", "application/json")
            .timeout(timeout)
            .send_json(serde_json::to_value(body)?)
            .map_err(|error| {
                anyhow::anyhow!(
                    "backend control request failed: {}",
                    crate::redact::redact(&control_error(error))
                )
            })?;
        response
            .into_json()
            .context("backend control response is invalid")
    }
}

#[derive(Clone)]
pub struct ServerContext {
    pub state: SupervisorState,
    pub control_token: String,
    pub plugin_token: String,
    pub shutdown: Arc<AtomicBool>,
    pub last_activity: Arc<Mutex<Instant>>,
    pub bridge: Arc<Bridge>,
}

#[derive(Serialize, Deserialize)]
struct HealthResponse {
    healthy: bool,
    protocol_version: u32,
    runtime_version: String,
    plugin_sha256: String,
    startup_nonce: String,
    supervisor_pid: u32,
    opencode_pid: u32,
}

pub fn state_path() -> Result<PathBuf> {
    Ok(super::supervisor::backend_root()?.join("supervisor.json"))
}

pub fn write_state(state: &SupervisorState) -> Result<()> {
    state.validate()?;
    let path = state_path()?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    crate::config::set_private_dir(parent);
    crate::config::write_atomic(&path, &serde_json::to_vec_pretty(state)?)
        .with_context(|| format!("writing backend state {}", path.display()))?;
    crate::config::set_private_file(&path);
    Ok(())
}

pub fn load_state() -> Result<Option<SupervisorState>> {
    let path = state_path()?;
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_BODY_BYTES as u64
    {
        anyhow::bail!("backend state is not a bounded private regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!("backend state permissions are insecure; run `aishe doctor --fix`");
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(&path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_BODY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_BODY_BYTES {
        anyhow::bail!("backend state exceeds the 1 MiB limit");
    }
    let state: SupervisorState =
        serde_json::from_slice(&bytes).context("backend state is invalid")?;
    state.validate()?;
    Ok(Some(state))
}

pub fn remove_state_if_nonce(nonce: &str) -> Result<()> {
    let Some(state) = load_state()? else {
        return Ok(());
    };
    if constant_time_eq(state.startup_nonce.as_bytes(), nonce.as_bytes()) {
        let path = state_path()?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn verified_state() -> Result<Option<SupervisorState>> {
    let Some(state) = load_state()? else {
        return Ok(None);
    };
    if !process_exists(state.supervisor_pid) || !process_exists(state.opencode_pid) {
        return Ok(None);
    }
    let url = format!("{}/v1/health", state.control_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let response = match agent
        .get(&url)
        .set("Authorization", &format!("Bearer {}", state.control_token))
        .timeout(Duration::from_secs(2))
        .call()
    {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    let health: HealthResponse = response.into_json()?;
    if !health.healthy
        || health.protocol_version != SUPERVISOR_PROTOCOL_VERSION
        || health.runtime_version != state.runtime_version
        || health.plugin_sha256 != state.plugin_sha256
        || health.supervisor_pid != state.supervisor_pid
        || health.opencode_pid != state.opencode_pid
        || !constant_time_eq(
            health.startup_nonce.as_bytes(),
            state.startup_nonce.as_bytes(),
        )
    {
        anyhow::bail!("backend control identity mismatch");
    }
    Ok(Some(state))
}

pub fn state_processes_exist(state: &SupervisorState) -> bool {
    process_exists(state.supervisor_pid) && process_exists(state.opencode_pid)
}

pub fn request_stop() -> Result<bool> {
    let Some(state) = verified_state()? else {
        return Ok(false);
    };
    let url = format!("{}/v1/stop", state.control_url.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    agent
        .post(&url)
        .set("Authorization", &format!("Bearer {}", state.control_token))
        .set("Content-Type", "application/json")
        .timeout(Duration::from_secs(3))
        .send_json(serde_json::json!({}))?;
    Ok(true)
}

pub fn serve_connection(mut stream: TcpStream, context: &ServerContext) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            return write_error(
                &mut stream,
                400,
                "invalid_request",
                "Malformed control request",
            )
        }
    };
    let expected_host = context
        .state
        .control_url
        .strip_prefix("http://")
        .unwrap_or("");
    if request.headers.get("host").map(String::as_str) != Some(expected_host) {
        return write_error(&mut stream, 400, "invalid_host", "Invalid Host header");
    }
    let token = request
        .headers
        .get("authorization")
        .and_then(|value| value.strip_prefix("Bearer "));
    let required = if request.path.starts_with("/v1/plugin/") {
        &context.plugin_token
    } else {
        &context.control_token
    };
    if !token.is_some_and(|value| constant_time_eq(value.as_bytes(), required.as_bytes())) {
        std::thread::sleep(Duration::from_millis(25));
        return write_error(&mut stream, 401, "unauthorized", "Authentication failed");
    }
    if let Ok(mut activity) = context.last_activity.lock() {
        *activity = Instant::now();
    }
    match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/health") => write_json(
            &mut stream,
            200,
            &HealthResponse {
                healthy: true,
                protocol_version: SUPERVISOR_PROTOCOL_VERSION,
                runtime_version: context.state.runtime_version.clone(),
                plugin_sha256: context.state.plugin_sha256.clone(),
                startup_nonce: context.state.startup_nonce.clone(),
                supervisor_pid: context.state.supervisor_pid,
                opencode_pid: context.state.opencode_pid,
            },
        ),
        ("POST", "/v1/stop") => {
            if require_json(&request).is_err() {
                return write_error(
                    &mut stream,
                    400,
                    "invalid_json",
                    "A JSON request body is required",
                );
            }
            context.shutdown.store(true, Ordering::SeqCst);
            write_json(&mut stream, 202, &serde_json::json!({"stopping":true}))
        }
        ("POST", "/v1/lease/register") => {
            let registration: LeaseRegistration = match parse_json(&request) {
                Ok(value) => value,
                Err(()) => {
                    return write_error(
                        &mut stream,
                        400,
                        "invalid_json",
                        "Lease registration is invalid",
                    )
                }
            };
            match context.bridge.register(registration) {
                Ok(identity) => write_json(&mut stream, 200, &identity),
                Err(error) => write_error(
                    &mut stream,
                    400,
                    "invalid_lease",
                    &crate::redact::redact(&error.to_string()),
                ),
            }
        }
        ("POST", "/v1/lease/heartbeat") => bridge_unit(
            &mut stream,
            parse_json::<LeaseIdentity>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|identity| context.bridge.heartbeat(&identity)),
        ),
        ("POST", "/v1/lease/unregister") => bridge_unit(
            &mut stream,
            parse_json::<LeaseIdentity>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|identity| context.bridge.unregister(&identity)),
        ),
        ("POST", "/v1/lease/next") => {
            let identity: LeaseIdentity = match parse_json(&request) {
                Ok(value) => value,
                Err(()) => return write_bridge_failure(&mut stream, &invalid_bridge_request()),
            };
            match context.bridge.next(&identity, Duration::from_secs(1)) {
                Ok(work) => write_json(&mut stream, 200, &work),
                Err(error) => write_bridge_failure(&mut stream, &error),
            }
        }
        ("POST", "/v1/lease/started") => bridge_unit(
            &mut stream,
            parse_json::<ToolStarted>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|started| context.bridge.started(&started)),
        ),
        ("POST", "/v1/lease/complete") => bridge_unit(
            &mut stream,
            parse_json::<ToolCompletion>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|completion| context.bridge.complete(completion)),
        ),
        ("POST", "/v1/plugin/provider-turn") => {
            let value: ProviderTurnRequest = match parse_json(&request) {
                Ok(value) => value,
                Err(()) => {
                    return write_error(
                        &mut stream,
                        400,
                        "invalid_json",
                        "Provider-turn request is invalid",
                    )
                }
            };
            match context.bridge.authorize_provider_turn(&value) {
                Ok(decision) => write_json(&mut stream, 200, &decision),
                Err(error) => write_bridge_failure(&mut stream, &error),
            }
        }
        ("POST", "/v1/plugin/usage") => bridge_unit(
            &mut stream,
            parse_json::<ProviderUsageReport>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|usage| context.bridge.record_provider_usage(usage)),
        ),
        ("POST", "/v1/plugin/child") => bridge_unit(
            &mut stream,
            parse_json::<ChildRegistration>(&request)
                .map_err(|_| invalid_bridge_request())
                .and_then(|child| context.bridge.register_child(child)),
        ),
        ("POST", "/v1/plugin/tool") => {
            let tool: PluginToolRequest = match parse_json(&request) {
                Ok(value) => value,
                Err(()) => {
                    return write_error(
                        &mut stream,
                        400,
                        "invalid_json",
                        "Tool bridge request is invalid",
                    )
                }
            };
            match context.bridge.admit_and_wait(tool) {
                Ok(outcome) => write_json(&mut stream, 200, &outcome),
                Err(error) => write_bridge_failure(&mut stream, &error),
            }
        }
        _ => write_error(&mut stream, 404, "not_found", "Control route not found"),
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    #[allow(dead_code)]
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    let mut bytes = Vec::with_capacity(4096);
    let header_end = loop {
        if bytes.len() >= MAX_HEADER_BYTES {
            anyhow::bail!("control request headers exceed 32 KiB");
        }
        let mut chunk = [0u8; 1024];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            anyhow::bail!("control request ended before headers");
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let head =
        std::str::from_utf8(&bytes[..header_end]).context("control headers are not UTF-8")?;
    let mut lines = head[..head.len() - 4].split("\r\n");
    let request_line = lines.next().context("control request line is missing")?;
    let mut fields = request_line.split(' ');
    let method = fields.next().unwrap_or("");
    let path = fields.next().unwrap_or("");
    let version = fields.next().unwrap_or("");
    if fields.next().is_some()
        || !matches!(method, "GET" | "POST")
        || !path.starts_with('/')
        || path.contains('?')
        || version != "HTTP/1.1"
    {
        anyhow::bail!("invalid control request line");
    }
    let mut headers = HashMap::new();
    for line in lines {
        let (name, value) = line.split_once(':').context("invalid control header")?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || value.chars().any(char::is_control)
        {
            anyhow::bail!("invalid control header");
        }
        let name = name.to_ascii_lowercase();
        if headers.insert(name, value.trim().to_string()).is_some() {
            anyhow::bail!("duplicate control header");
        }
    }
    let length = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .context("invalid Content-Length header")?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        anyhow::bail!("control request body exceeds 1 MiB");
    }
    if headers.contains_key("transfer-encoding") {
        anyhow::bail!("control requests may not use Transfer-Encoding");
    }
    let mut body = bytes[header_end..].to_vec();
    if body.len() > length {
        anyhow::bail!("control request contains trailing bytes");
    }
    body.resize(length, 0);
    if bytes.len() - header_end < length {
        stream.read_exact(&mut body[bytes.len() - header_end..])?;
    }
    Ok(HttpRequest {
        method: method.into(),
        path: path.into(),
        headers,
        body,
    })
}

fn require_json(request: &HttpRequest) -> Result<()> {
    if request.headers.get("content-type").map(String::as_str) != Some("application/json") {
        anyhow::bail!("control POST requires application/json");
    }
    if !request.body.is_empty() {
        let _: serde_json::Value =
            serde_json::from_slice(&request.body).context("control body is invalid JSON")?;
    }
    Ok(())
}

fn parse_json<T: serde::de::DeserializeOwned>(request: &HttpRequest) -> Result<T, ()> {
    require_json(request).map_err(|_| ())?;
    serde_json::from_slice(&request.body).map_err(|_| ())
}

fn invalid_bridge_request() -> BridgeFailure {
    BridgeFailure {
        status: 400,
        code: "invalid_request",
        message: "Foreground bridge request is invalid".into(),
    }
}

fn bridge_unit(stream: &mut TcpStream, result: Result<(), BridgeFailure>) -> Result<()> {
    match result {
        Ok(()) => write_json(stream, 200, &serde_json::json!({"ok":true})),
        Err(error) => write_bridge_failure(stream, &error),
    }
}

fn write_bridge_failure(stream: &mut TcpStream, failure: &BridgeFailure) -> Result<()> {
    write_error(
        stream,
        failure.status,
        failure.code,
        &crate::redact::redact(&failure.message),
    )
}

fn write_error(stream: &mut TcpStream, status: u16, code: &str, message: &str) -> Result<()> {
    write_json(
        stream,
        status,
        &serde_json::json!({"error":{"code":code,"message":message}}),
    )
}

fn write_json(stream: &mut TcpStream, status: u16, body: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(body)?;
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n",
        bytes.len()
    )?;
    stream.write_all(&bytes)?;
    stream.flush()?;
    Ok(())
}

fn validate_loopback_url(value: &str) -> Result<()> {
    let parsed = url::Url::parse(value)?;
    if parsed.scheme() != "http"
        || parsed.username() != ""
        || parsed.password().is_some()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.host_str() != Some("127.0.0.1")
        || parsed.port().is_none()
    {
        anyhow::bail!("backend endpoint is not a strict IPv4 loopback URL");
    }
    Ok(())
}

fn control_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(status, response) => {
            let body = response
                .into_string()
                .unwrap_or_else(|_| "request rejected".into());
            format!(
                "status {status}: {}",
                body.chars().take(1024).collect::<String>()
            )
        }
        ureq::Error::Transport(error) => error.to_string(),
    }
}

fn process_exists(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 || pid > i32::MAX as u32 {
            return false;
        }
        let result = unsafe { libc::kill(pid as i32, 0) };
        result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        pid != 0
    }
}

pub fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    let length = left.len().max(right.len());
    for index in 0..length {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge_path(port: u16) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aishe-control-bridge-test-{}-{port}.json",
            std::process::id()
        ))
    }

    fn context_for(port: u16) -> ServerContext {
        let state = SupervisorState::new(
            std::process::id(),
            std::process::id(),
            format!("http://127.0.0.1:{port}"),
            "http://127.0.0.1:2345".into(),
            "1.18.9".into(),
            "e".repeat(64),
            "aishe-openai".into(),
            "model".into(),
            "a".repeat(64),
            1,
            "b".repeat(64),
            "c".repeat(64),
        );
        ServerContext {
            state,
            control_token: "b".repeat(64),
            plugin_token: "d".repeat(64),
            shutdown: Arc::new(AtomicBool::new(false)),
            last_activity: Arc::new(Mutex::new(Instant::now())),
            bridge: Arc::new(Bridge::open(bridge_path(port)).unwrap()),
        }
    }

    #[test]
    fn authenticated_health_binds_identity() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let context = context_for(port);
        let server = context.clone();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &server).unwrap();
        });
        let response: serde_json::Value =
            ureq::get(&format!("{}/v1/health", context.state.control_url))
                .set(
                    "Authorization",
                    &format!("Bearer {}", context.control_token),
                )
                .call()
                .unwrap()
                .into_json()
                .unwrap();
        worker.join().unwrap();
        std::fs::remove_file(bridge_path(port)).unwrap();
        assert_eq!(response["healthy"], true);
        assert_eq!(response["startup_nonce"], "a".repeat(64));
        assert_eq!(response["protocol_version"], SUPERVISOR_PROTOCOL_VERSION);
    }

    #[test]
    fn plugin_token_cannot_stop_supervisor() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let context = context_for(listener.local_addr().unwrap().port());
        let server = context.clone();
        let worker = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve_connection(stream, &server).unwrap();
        });
        let result = ureq::post(&format!("{}/v1/stop", context.state.control_url))
            .set("Authorization", &format!("Bearer {}", context.plugin_token))
            .set("Content-Type", "application/json")
            .send_json(serde_json::json!({}));
        worker.join().unwrap();
        std::fs::remove_file(bridge_path(
            context
                .state
                .control_url
                .rsplit(':')
                .next()
                .unwrap()
                .parse()
                .unwrap(),
        ))
        .unwrap();
        assert!(matches!(result, Err(ureq::Error::Status(401, _))));
        assert!(!context.shutdown.load(Ordering::SeqCst));
    }

    #[test]
    fn constant_time_comparison_is_exact() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"sane"));
        assert!(!constant_time_eq(b"same", b"same-longer"));
    }

    #[test]
    fn state_rejects_non_loopback_or_short_tokens() {
        let mut state = SupervisorState::new(
            1,
            2,
            "http://127.0.0.1:1234".into(),
            "http://127.0.0.1:2345".into(),
            "1.18.9".into(),
            "e".repeat(64),
            "aishe-openai".into(),
            "model".into(),
            "a".repeat(64),
            1,
            "b".repeat(64),
            "c".repeat(64),
        );
        assert!(state.validate().is_ok());
        state.control_url = "http://0.0.0.0:1234".into();
        assert!(state.validate().is_err());
    }
}
