use std::collections::HashSet;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::Engine;
use serde_json::Value;

use crate::agent::{
    AgentEvent, BackendSession, ExecutionScope, Mode, NetworkPolicy, PromptHandle, PromptRequest,
    SessionSnapshot, SessionSummary, UsageDelta,
};

use super::mapper::EventMapper;

const MAX_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RECONNECT_ATTEMPTS: u32 = 3;
const MESSAGE_BIND_ATTEMPTS: u32 = 100;
const MESSAGE_BIND_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Clone)]
pub struct OpenCodeConnection {
    pub base_url: String,
    pub username: String,
    pub password: String,
    pub version: String,
}

#[derive(Clone)]
pub struct OpenCodeClient {
    connection: OpenCodeConnection,
    provider_id: String,
    model_id: String,
}

impl OpenCodeClient {
    pub fn new(
        connection: OpenCodeConnection,
        provider_id: impl Into<String>,
        model_id: impl Into<String>,
    ) -> Result<Self> {
        let parsed = url::Url::parse(&connection.base_url)?;
        if parsed.scheme() != "http"
            || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost" | "::1"))
        {
            anyhow::bail!("OpenCode client accepts only private loopback HTTP endpoints");
        }
        if connection.password.is_empty() {
            anyhow::bail!("OpenCode server password is missing");
        }
        Ok(Self {
            connection,
            provider_id: provider_id.into(),
            model_id: model_id.into(),
        })
    }

    pub fn health(&self) -> Result<Value> {
        let value = self.get_json("/global/health", None)?;
        if value.get("healthy").and_then(Value::as_bool) != Some(true)
            || value.get("version").and_then(Value::as_str)
                != Some(self.connection.version.as_str())
        {
            anyhow::bail!("OpenCode server health identity mismatch");
        }
        Ok(value)
    }

    pub fn create_session(
        &self,
        workspace: &Path,
        title: &str,
        scope: ExecutionScope,
        network: NetworkPolicy,
    ) -> Result<BackendSession> {
        let value = self.post_json(
            "/session",
            Some(workspace),
            &serde_json::json!({
                "title": title,
                "agent": "aishe-suggest",
                "model": {"id": self.model_id, "providerID": self.provider_id},
                "metadata": {
                    "aishe_scope": match scope { ExecutionScope::Workspace => "workspace", ExecutionScope::Host => "host" },
                    "aishe_network": match network { NetworkPolicy::Deny => "deny", NetworkPolicy::Allow => "allow" }
                }
            }),
        )?;
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .context("OpenCode create-session response omitted id")?;
        Ok(BackendSession {
            id: id.to_string(),
            workspace: workspace.to_path_buf(),
            backend: "opencode".into(),
        })
    }

    pub fn session(&self, session: &BackendSession) -> Result<Option<Value>> {
        let path = format!("/session/{}", encode_segment(&session.id));
        self.get_json_optional(&path, Some(&session.workspace))
    }

    /// Subscribe before admission, then submit the prompt. The returned reader
    /// must be consumed through `read_events`; dropping it is a cancellation of
    /// local rendering, not a server abort.
    pub fn submit(
        &self,
        request: &PromptRequest,
    ) -> Result<(PromptHandle, Box<dyn BufRead + Send>)> {
        let stream = self.subscribe()?;
        // OpenCode compares message IDs lexicographically to determine whether
        // a newly admitted user turn follows the previous assistant turn. Its
        // IDs contain a server-process-local monotonic counter, so a client
        // cannot safely mint a compatible ID. Snapshot the current messages
        // after subscribing, omit messageID on admission, and bind the open
        // stream to the authoritative ID that OpenCode creates.
        let baseline = message_ids(&self.session_messages(&request.session)?);
        let path = format!(
            "/session/{}/prompt_async",
            encode_segment(&request.session.id)
        );
        let agent = agent_for_mode(request.mode);
        let body = serde_json::json!({
            "model": {"providerID": self.provider_id, "modelID": self.model_id},
            "agent": agent,
            "parts": [{"type":"text","text":request.text}]
        });
        // OpenCode 1.18.9's submit API accepts `format=json_schema`, but its
        // durable message-read API rejects the same persisted format (including
        // the defaulted retryCount). That poisons snapshot/resume for the whole
        // session. The compatibility-pinned adapter therefore uses the strict
        // JSON protocol in the trusted aishe-suggest system prompt and validates
        // it in Rust, without sending the broken upstream format field.
        self.post_no_content(&path, Some(&request.session.workspace), &body)?;
        let message_id = self
            .await_admitted_user_message(request, agent, &baseline)
            .context("binding admitted OpenCode prompt to its authoritative message ID")?;
        Ok((
            PromptHandle {
                session_id: request.session.id.clone(),
                message_id,
                workspace: request.session.workspace.clone(),
                prompt_text: request.text.clone(),
                resumed: false,
                admitted: true,
            },
            stream,
        ))
    }

    /// Subscribe before inspecting current state, closing the event gap between
    /// the snapshot and the live stream.
    pub fn resume(
        &self,
        session: &BackendSession,
    ) -> Result<(PromptHandle, Box<dyn BufRead + Send>, bool)> {
        let stream = self.subscribe()?;
        let busy = self.session_busy(session)?;
        let messages = self.session_messages(session)?;
        let message_id = latest_user_message_id(&messages)
            .context("OpenCode session has no user message to resume")?;
        Ok((
            PromptHandle {
                session_id: session.id.clone(),
                message_id,
                workspace: session.workspace.clone(),
                prompt_text: String::new(),
                resumed: true,
                admitted: true,
            },
            stream,
            busy,
        ))
    }

    pub fn read_events(
        &self,
        handle: &PromptHandle,
        reader: Box<dyn BufRead + Send>,
    ) -> Result<Vec<AgentEvent>> {
        self.read_events_with(handle, reader, &mut |_| {})
    }

    pub fn read_events_with(
        &self,
        handle: &PromptHandle,
        mut reader: Box<dyn BufRead + Send>,
        emit: &mut dyn FnMut(&AgentEvent),
    ) -> Result<Vec<AgentEvent>> {
        let mut mapper = EventMapper::new(&handle.session_id, &handle.message_id);
        let mut events = Vec::new();
        push_event(&mut events, emit, AgentEvent::Connected);
        if handle.resumed {
            push_event(&mut events, emit, AgentEvent::Reconciled);
        } else {
            push_event(
                &mut events,
                emit,
                AgentEvent::UserPromptAccepted {
                    text: handle.prompt_text.clone(),
                },
            );
        }
        let session = BackendSession {
            id: handle.session_id.clone(),
            workspace: handle.workspace.clone(),
            backend: "opencode".into(),
        };
        let mut reconnect_attempt = 0;
        loop {
            let Some(value) = super::sse::next_json(reader.as_mut())? else {
                reconnect_attempt += 1;
                push_event(
                    &mut events,
                    emit,
                    AgentEvent::Reconnecting {
                        attempt: reconnect_attempt,
                    },
                );
                if reconnect_attempt > MAX_RECONNECT_ATTEMPTS {
                    push_event(
                        &mut events,
                        emit,
                        AgentEvent::Failed {
                            error: crate::agent::UserFacingError {
                                code: "opencode_stream_lost".into(),
                                message:
                                    "The agent event stream disconnected and could not be restored."
                                        .into(),
                                retryable: true,
                            },
                        },
                    );
                    break;
                }
                std::thread::sleep(Duration::from_millis(
                    50_u64.saturating_mul(1 << (reconnect_attempt - 1)),
                ));
                // Subscribe first, then reconcile from durable server state so
                // events occurring during the snapshot remain observable.
                reader = self.subscribe()?;
                push_events(
                    &mut events,
                    emit,
                    self.reconcile_events(&session, &mut mapper)?,
                );
                push_event(&mut events, emit, AgentEvent::Reconciled);
                if !self.session_busy(&session)? {
                    push_event(
                        &mut events,
                        emit,
                        AgentEvent::Completed {
                            summary: String::new(),
                        },
                    );
                    break;
                }
                continue;
            };
            reconnect_attempt = 0;
            let mapped = mapper.map(&value);
            let done = mapped.iter().any(|event| {
                matches!(
                    event,
                    AgentEvent::Completed { .. } | AgentEvent::Failed { .. } | AgentEvent::Aborted
                )
            });
            push_events(&mut events, emit, mapped);
            if done {
                break;
            }
        }
        Ok(events)
    }

    pub fn reconciled_events(&self, handle: &PromptHandle) -> Result<Vec<AgentEvent>> {
        let session = BackendSession {
            id: handle.session_id.clone(),
            workspace: handle.workspace.clone(),
            backend: "opencode".into(),
        };
        let mut mapper = EventMapper::new(&handle.session_id, &handle.message_id);
        let mut events = vec![AgentEvent::Connected, AgentEvent::Reconciled];
        events.extend(self.reconcile_events(&session, &mut mapper)?);
        events.push(AgentEvent::Completed {
            summary: String::new(),
        });
        Ok(events)
    }

    pub fn abort(&self, session: &BackendSession) -> Result<()> {
        let path = format!("/session/{}/abort", encode_segment(&session.id));
        self.post_no_content(&path, Some(&session.workspace), &serde_json::json!({}))
    }

    pub fn list_sessions(&self, workspace: Option<&Path>) -> Result<Vec<SessionSummary>> {
        let value = self.get_json("/session", workspace)?;
        let rows = value
            .as_array()
            .context("OpenCode session listing was not an array")?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(SessionSummary {
                    id: row.get("id")?.as_str()?.to_string(),
                    title: row
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Untitled")
                        .to_string(),
                    workspace: row
                        .get("directory")
                        .and_then(Value::as_str)
                        .map(std::path::PathBuf::from)
                        .unwrap_or_default(),
                    updated_at_ms: row
                        .get("time")
                        .and_then(|time| time.get("updated"))
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u128,
                    backend: "opencode".into(),
                })
            })
            .collect())
    }

    pub fn snapshot(&self, session: &BackendSession) -> Result<SessionSnapshot> {
        let busy = self.session_busy(session)?;
        let messages = self.session_messages(session)?;
        let mut usage = UsageDelta::default();
        if let Some(rows) = messages.as_array() {
            for row in rows {
                let info = row.get("info").unwrap_or(&Value::Null);
                if info.get("role").and_then(Value::as_str) != Some("assistant") {
                    continue;
                }
                let tokens = info.get("tokens").unwrap_or(&Value::Null);
                usage.input_tokens = usage
                    .input_tokens
                    .saturating_add(json_u64(tokens.get("input")));
                usage.output_tokens = usage
                    .output_tokens
                    .saturating_add(json_u64(tokens.get("output")));
                usage.reasoning_tokens = usage
                    .reasoning_tokens
                    .saturating_add(json_u64(tokens.get("reasoning")));
                let cost = info.get("cost").and_then(Value::as_f64);
                usage.cost_usd = match (usage.cost_usd, cost) {
                    (Some(total), Some(delta)) => Some(total + delta),
                    (None, Some(delta)) => Some(delta),
                    (value, None) => value,
                };
            }
        }
        Ok(SessionSnapshot {
            session_id: session.id.clone(),
            events: Vec::new(),
            usage,
            busy,
        })
    }

    fn session_busy(&self, session: &BackendSession) -> Result<bool> {
        let status = self.get_json("/session/status", Some(&session.workspace))?;
        Ok(status
            .get(&session.id)
            .and_then(|value| value.get("type"))
            .and_then(Value::as_str)
            .is_some_and(|value| value != "idle"))
    }

    fn session_messages(&self, session: &BackendSession) -> Result<Value> {
        let path = format!("/session/{}/message", encode_segment(&session.id));
        self.get_json(&path, Some(&session.workspace))
    }

    fn await_admitted_user_message(
        &self,
        request: &PromptRequest,
        agent: &str,
        baseline: &HashSet<String>,
    ) -> Result<String> {
        let mut last_error = None;
        for attempt in 0..MESSAGE_BIND_ATTEMPTS {
            match self.session_messages(&request.session) {
                Ok(messages) => {
                    if let Some(message_id) = matching_new_user_message_id(
                        &messages,
                        baseline,
                        agent,
                        &self.provider_id,
                        &self.model_id,
                        &request.text,
                    ) {
                        return Ok(message_id);
                    }
                }
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < MESSAGE_BIND_ATTEMPTS {
                std::thread::sleep(MESSAGE_BIND_INTERVAL);
            }
        }
        if let Some(error) = last_error {
            return Err(error)
                .context("OpenCode admitted the prompt but its message list could not be read");
        }
        anyhow::bail!(
            "OpenCode admitted the prompt but did not publish its user message within {} ms",
            MESSAGE_BIND_INTERVAL.as_millis() * u128::from(MESSAGE_BIND_ATTEMPTS)
        )
    }

    fn reconcile_events(
        &self,
        session: &BackendSession,
        mapper: &mut EventMapper,
    ) -> Result<Vec<AgentEvent>> {
        let messages = self.session_messages(session)?;
        let mut events = Vec::new();
        for message in messages.as_array().into_iter().flatten() {
            let info = message.get("info").unwrap_or(&Value::Null);
            let message_id = info.get("id").and_then(Value::as_str);
            let relevant = info.get("role").and_then(Value::as_str) == Some("assistant")
                && info.get("parentID").and_then(Value::as_str) == Some(mapper.user_message_id());
            if !relevant {
                continue;
            }
            events.extend(mapper.map(&serde_json::json!({
                "payload": {
                    "type": "message.updated",
                    "properties": {"sessionID":session.id,"info":info}
                }
            })));
            for part in message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if message_id.is_some() {
                    events.extend(mapper.map(&serde_json::json!({
                        "payload": {
                            "type": "message.part.updated",
                            "properties": {"sessionID":session.id,"part":part}
                        }
                    })));
                }
            }
        }
        Ok(events)
    }

    fn subscribe(&self) -> Result<Box<dyn BufRead + Send>> {
        let response = self
            .request(ureq::get(&format!(
                "{}/global/event",
                self.connection.base_url.trim_end_matches('/')
            )))
            .set("Accept", "text/event-stream")
            .timeout(Duration::from_secs(24 * 60 * 60))
            .call()
            .context("subscribing to OpenCode events")?;
        let content_type = response.header("content-type").unwrap_or("");
        if !content_type.starts_with("text/event-stream") {
            anyhow::bail!("OpenCode event endpoint returned {content_type}, not SSE");
        }
        Ok(Box::new(BufReader::new(response.into_reader())))
    }

    fn get_json(&self, path: &str, workspace: Option<&Path>) -> Result<Value> {
        let url = self.url(path, workspace)?;
        let response = self
            .request(ureq::get(&url))
            .timeout(Duration::from_secs(30))
            .call()
            .with_context(|| format!("OpenCode GET {path} failed"))?;
        read_json_bounded(response)
    }

    fn get_json_optional(&self, path: &str, workspace: Option<&Path>) -> Result<Option<Value>> {
        let url = self.url(path, workspace)?;
        match self
            .request(ureq::get(&url))
            .timeout(Duration::from_secs(30))
            .call()
        {
            Ok(response) => read_json_bounded(response).map(Some),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(error).with_context(|| format!("OpenCode GET {path} failed")),
        }
    }

    fn post_json(&self, path: &str, workspace: Option<&Path>, body: &Value) -> Result<Value> {
        let url = self.url(path, workspace)?;
        let response = self
            .request(ureq::post(&url))
            .timeout(Duration::from_secs(30))
            .send_json(body.clone())
            .with_context(|| format!("OpenCode POST {path} failed"))?;
        read_json_bounded(response)
    }

    fn post_no_content(&self, path: &str, workspace: Option<&Path>, body: &Value) -> Result<()> {
        let url = self.url(path, workspace)?;
        self.request(ureq::post(&url))
            .timeout(Duration::from_secs(30))
            .send_json(body.clone())
            .with_context(|| format!("OpenCode POST {path} failed"))?;
        Ok(())
    }

    fn request(&self, request: ureq::Request) -> ureq::Request {
        let authorization = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!(
                "{}:{}",
                self.connection.username, self.connection.password
            ))
        );
        request
            .set("Authorization", &authorization)
            .set("User-Agent", concat!("aishe/", env!("CARGO_PKG_VERSION")))
    }

    fn url(&self, path: &str, workspace: Option<&Path>) -> Result<String> {
        let mut url = url::Url::parse(&format!(
            "{}{}",
            self.connection.base_url.trim_end_matches('/'),
            path
        ))?;
        if let Some(workspace) = workspace {
            url.query_pairs_mut()
                .append_pair("directory", &workspace.to_string_lossy());
        }
        Ok(url.into())
    }
}

fn push_event(events: &mut Vec<AgentEvent>, emit: &mut dyn FnMut(&AgentEvent), event: AgentEvent) {
    emit(&event);
    events.push(event);
}

fn push_events(
    events: &mut Vec<AgentEvent>,
    emit: &mut dyn FnMut(&AgentEvent),
    incoming: Vec<AgentEvent>,
) {
    for event in incoming {
        push_event(events, emit, event);
    }
}

fn agent_for_mode(mode: Mode) -> &'static str {
    match mode {
        Mode::Suggest => "aishe-suggest",
        Mode::Auto => "aishe-auto",
        Mode::Yolo => "aishe-yolo",
    }
}

fn read_json_bounded(response: ureq::Response) -> Result<Value> {
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON_BYTES {
        anyhow::bail!("OpenCode JSON response exceeds the 16 MiB limit");
    }
    serde_json::from_slice(&bytes).context("decoding OpenCode JSON response")
}

fn encode_segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn json_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_f64().map(|n| n.max(0.0) as u64))
        })
        .unwrap_or(0)
}

fn latest_user_message_id(messages: &Value) -> Option<String> {
    messages.as_array()?.iter().rev().find_map(|message| {
        let info = message.get("info")?;
        (info.get("role")?.as_str()? == "user")
            .then(|| info.get("id")?.as_str().map(ToString::to_string))
            .flatten()
    })
}

fn message_ids(messages: &Value) -> HashSet<String> {
    messages
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|message| {
            message
                .get("info")
                .and_then(|info| info.get("id"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect()
}

fn matching_new_user_message_id(
    messages: &Value,
    baseline: &HashSet<String>,
    agent: &str,
    provider_id: &str,
    model_id: &str,
    prompt: &str,
) -> Option<String> {
    messages.as_array()?.iter().rev().find_map(|message| {
        let info = message.get("info")?;
        let id = info.get("id")?.as_str()?;
        if baseline.contains(id)
            || info.get("role").and_then(Value::as_str) != Some("user")
            || info.get("agent").and_then(Value::as_str) != Some(agent)
            || info
                .get("model")
                .and_then(|model| model.get("providerID"))
                .and_then(Value::as_str)
                != Some(provider_id)
            || info
                .get("model")
                .and_then(|model| model.get("modelID"))
                .and_then(Value::as_str)
                != Some(model_id)
        {
            return None;
        }
        let has_prompt = message
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .any(|part| {
                part.get("type").and_then(Value::as_str) == Some("text")
                    && part.get("text").and_then(Value::as_str) == Some(prompt)
            });
        has_prompt.then(|| id.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};

    #[test]
    fn refuses_non_loopback_servers() {
        assert!(OpenCodeClient::new(
            OpenCodeConnection {
                base_url: "https://example.com".into(),
                username: "aishe".into(),
                password: "secret".into(),
                version: "1.18.9".into(),
            },
            "aishe-openai",
            "test",
        )
        .is_err());
    }

    #[test]
    fn modes_map_only_to_aishe_owned_agents() {
        assert_eq!(agent_for_mode(Mode::Suggest), "aishe-suggest");
        assert_eq!(agent_for_mode(Mode::Auto), "aishe-auto");
        assert_eq!(agent_for_mode(Mode::Yolo), "aishe-yolo");
    }

    #[test]
    fn pinned_openapi_fixture_contains_every_adapter_endpoint() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/opencode/v1.18.9/openapi-contract.json"
        ))
        .unwrap();
        assert_eq!(fixture["info"]["version"], "1.18.9");
        let paths = fixture["paths"].as_object().unwrap();
        for (path, method) in [
            ("/global/health", "get"),
            ("/global/event", "get"),
            ("/session", "get"),
            ("/session", "post"),
            ("/session/status", "get"),
            ("/session/{sessionID}", "get"),
            ("/session/{sessionID}/message", "get"),
            ("/session/{sessionID}/prompt_async", "post"),
            ("/session/{sessionID}/abort", "post"),
            ("/experimental/tool/ids", "get"),
        ] {
            assert!(
                paths.get(path).and_then(|item| item.get(method)).is_some(),
                "missing {method} {path}"
            );
        }
    }

    #[test]
    fn subscribes_before_prompt_and_maps_a_complete_contract_stream() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut events, _) = listener.accept().unwrap();
            let event_request = read_test_request(&mut events);
            assert!(event_request.starts_with("GET /global/event HTTP/1.1\r\n"));
            assert!(event_request.contains("Accept: text/event-stream"));
            write!(
                events,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            events.flush().unwrap();

            let (mut baseline, _) = listener.accept().unwrap();
            let baseline_request = read_test_request(&mut baseline);
            assert!(baseline_request.starts_with("GET /session/ses_1/message?directory="));
            write!(
                baseline,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]"
            )
            .unwrap();
            baseline.flush().unwrap();

            let (mut prompt, _) = listener.accept().unwrap();
            let prompt_request = read_test_request(&mut prompt);
            assert!(prompt_request.starts_with("POST /session/ses_1/prompt_async?directory="));
            let body = prompt_request.split("\r\n\r\n").nth(1).unwrap();
            let value: Value = serde_json::from_str(body).unwrap();
            assert_eq!(value["parts"][0]["text"], "capital of France?");
            assert!(
                value.get("format").is_none(),
                "OpenCode 1.18.9 cannot durably reread json_schema user messages"
            );
            assert!(
                value.get("messageID").is_none(),
                "OpenCode must mint its own monotonic message ID"
            );
            write!(
                prompt,
                "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            prompt.flush().unwrap();

            let user_message = "msg_019d2a4f41af0001servermessage1";
            let admitted_body = serde_json::json!([{
                "info": {
                    "id": user_message,
                    "role": "user",
                    "agent": "aishe-suggest",
                    "model": {
                        "providerID": "aishe-openai",
                        "modelID": "model"
                    }
                },
                "parts": [{
                    "id": "prt_server",
                    "messageID": user_message,
                    "type": "text",
                    "text": "capital of France?"
                }]
            }])
            .to_string();
            let (mut admitted, _) = listener.accept().unwrap();
            let admitted_request = read_test_request(&mut admitted);
            assert!(admitted_request.starts_with("GET /session/ses_1/message?directory="));
            write!(
                admitted,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                admitted_body.len(),
                admitted_body
            )
            .unwrap();
            admitted.flush().unwrap();

            let frames = [
                serde_json::json!({"payload":{"type":"message.part.updated","properties":{
                    "sessionID":"ses_1","part":{"id":"prt_1","messageID":"msg_assistant",
                    "type":"text","text":"Paris.",
                    "time":{"start":1,"end":2}}}}}),
                serde_json::json!({"payload":{"type":"message.updated","properties":{
                    "sessionID":"ses_1","info":{"id":"msg_assistant","role":"assistant",
                    "parentID":user_message,"time":{"completed":2},
                    "tokens":{"input":10,"output":2},"cost":0.001}}}}),
                serde_json::json!({"payload":{"type":"session.status","properties":{
                    "sessionID":"ses_1","status":{"type":"idle"}}}}),
            ];
            for frame in frames {
                write!(events, "data: {frame}\n\n").unwrap();
            }
            events.flush().unwrap();
        });

        let client = OpenCodeClient::new(
            OpenCodeConnection {
                base_url: format!("http://127.0.0.1:{port}"),
                username: "aishe".into(),
                password: "private".into(),
                version: "1.18.9".into(),
            },
            "aishe-openai",
            "model",
        )
        .unwrap();
        let workspace = std::env::temp_dir();
        let (handle, stream) = client
            .submit(&PromptRequest {
                session: BackendSession {
                    id: "ses_1".into(),
                    workspace,
                    backend: "opencode".into(),
                },
                text: "capital of France?".into(),
                mode: Mode::Suggest,
                max_output_tokens: None,
            })
            .unwrap();
        let events = client.read_events(&handle, stream).unwrap();
        server.join().unwrap();
        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::UserPromptAccepted { text } if text == "capital of France?")
        ));
        assert!(events
            .iter()
            .any(|event| matches!(event, AgentEvent::TextCompleted { text } if text == "Paris.")));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, AgentEvent::Usage { .. }))
                .count(),
            1
        );
        assert!(matches!(events.last(), Some(AgentEvent::Completed { .. })));
    }

    #[test]
    fn binds_only_a_new_exact_server_created_user_message() {
        let baseline = ["msg_old".to_string()].into_iter().collect();
        let messages = serde_json::json!([
            {
                "info":{"id":"msg_old","role":"user","agent":"aishe-auto",
                    "model":{"providerID":"p","modelID":"m"}},
                "parts":[{"type":"text","text":"same"}]
            },
            {
                "info":{"id":"msg_wrong_agent","role":"user","agent":"aishe-yolo",
                    "model":{"providerID":"p","modelID":"m"}},
                "parts":[{"type":"text","text":"same"}]
            },
            {
                "info":{"id":"msg_new","role":"user","agent":"aishe-auto",
                    "model":{"providerID":"p","modelID":"m"}},
                "parts":[{"type":"text","text":"same"}]
            }
        ]);
        assert_eq!(
            matching_new_user_message_id(&messages, &baseline, "aishe-auto", "p", "m", "same")
                .as_deref(),
            Some("msg_new")
        );
    }

    fn read_test_request(stream: &mut TcpStream) -> String {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
        let header_end = loop {
            let mut chunk = [0u8; 1024];
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&chunk[..read]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let head = String::from_utf8(bytes[..header_end].to_vec()).unwrap();
        let length = head
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while bytes.len() - header_end < length {
            let mut chunk = [0u8; 1024];
            let read = stream.read(&mut chunk).unwrap();
            assert!(read > 0);
            bytes.extend_from_slice(&chunk[..read]);
        }
        String::from_utf8(bytes).unwrap()
    }
}
