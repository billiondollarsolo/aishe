use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::agent::{
    AgentEvent, DiffView, TodoItem, ToolCallView, ToolResultView, UsageDelta, UserFacingError,
    UserQuestion,
};

const MAX_PENDING_PART_EVENTS: usize = 256;

enum PendingPartEvent {
    Delta(Value),
    Updated(Value),
}

#[derive(Default)]
pub struct EventMapper {
    session_id: String,
    user_message_id: String,
    part_kinds: HashMap<String, String>,
    completed_parts: HashSet<String>,
    tool_statuses: HashMap<String, String>,
    emitted_usage_messages: HashSet<String>,
    emitted_structured_messages: HashSet<String>,
    child_sessions: HashSet<String>,
    completed_children: HashSet<String>,
    reasoning_active: bool,
    turn_observed: bool,
    assistant_message_ids: HashSet<String>,
    pending_part_events: Vec<PendingPartEvent>,
}

impl EventMapper {
    pub fn new(session_id: impl Into<String>, user_message_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            user_message_id: user_message_id.into(),
            ..Self::default()
        }
    }

    pub fn user_message_id(&self) -> &str {
        &self.user_message_id
    }

    /// Map one `/global/event` envelope. Unknown event types/fields are ignored
    /// so a compatible additive OpenCode update cannot break the terminal.
    pub fn map(&mut self, envelope: &Value) -> Vec<AgentEvent> {
        let payload = envelope.get("payload").unwrap_or(envelope);
        let Some(kind) = payload.get("type").and_then(Value::as_str) else {
            return Vec::new();
        };
        let properties = payload.get("properties").unwrap_or(&Value::Null);
        if let Some(session) = properties.get("sessionID").and_then(Value::as_str) {
            if session != self.session_id {
                if kind == "session.created"
                    && properties
                        .get("info")
                        .and_then(|info| info.get("parentID"))
                        .and_then(Value::as_str)
                        == Some(self.session_id.as_str())
                {
                    self.child_sessions.insert(session.to_string());
                    return vec![AgentEvent::SubagentStarted {
                        parent: self.session_id.clone(),
                        child: session.to_string(),
                        agent: properties
                            .get("info")
                            .and_then(|info| info.get("agent"))
                            .and_then(Value::as_str)
                            .unwrap_or("subagent")
                            .to_string(),
                    }];
                }
                if self.child_sessions.contains(session) {
                    return match kind {
                        "message.updated" => self.child_message_updated(properties),
                        "session.idle" => self.child_completed(session),
                        "session.status"
                            if properties
                                .get("status")
                                .and_then(|status| status.get("type"))
                                .and_then(Value::as_str)
                                == Some("idle") =>
                        {
                            self.child_completed(session)
                        }
                        _ => Vec::new(),
                    };
                }
                return Vec::new();
            }
        }

        match kind {
            "message.updated" => self.message_updated(properties),
            "message.part.delta" => self.part_delta(properties),
            "message.part.updated" => self.part_updated(properties),
            "session.status" => self.session_status(properties),
            "session.idle" => self.session_idle(),
            "session.error" => vec![AgentEvent::Failed {
                error: map_error(properties.get("error")),
            }],
            "session.compacted" => vec![AgentEvent::Compacted],
            "session.diff" => properties
                .get("diff")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(map_diff)
                .map(|diff| AgentEvent::Diff { diff })
                .collect(),
            "todo.updated" => vec![AgentEvent::TodoUpdated {
                items: properties
                    .get("todos")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .map(|(index, item)| TodoItem {
                        id: format!("{}:{index}", self.session_id),
                        content: item
                            .get("content")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        status: item
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("pending")
                            .to_string(),
                    })
                    .collect(),
            }],
            // AIShe owns every approval. OpenCode is configured default-deny
            // with only trusted proxy tools allowed, so this event is a policy
            // escape or configuration defect, never a user-facing approval.
            "permission.asked" => vec![AgentEvent::Failed {
                error: UserFacingError {
                    code: "opencode_permission_escape".into(),
                    message:
                        "The agent backend requested a forbidden built-in permission; the request was denied."
                            .into(),
                    retryable: false,
                },
            }],
            "question.asked" => vec![AgentEvent::WaitingForUser {
                request: UserQuestion {
                    request_id: properties
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    prompt: properties
                        .get("questions")
                        .and_then(Value::as_array)
                        .and_then(|items| items.first())
                        .and_then(|item| item.get("question"))
                        .and_then(Value::as_str)
                        .unwrap_or("The agent needs more information.")
                        .to_string(),
                },
            }],
            _ => Vec::new(),
        }
    }

    fn child_message_updated(&mut self, properties: &Value) -> Vec<AgentEvent> {
        let info = properties.get("info").unwrap_or(&Value::Null);
        if info.get("role").and_then(Value::as_str) != Some("assistant")
            || info
                .get("time")
                .and_then(|time| time.get("completed"))
                .is_none()
        {
            return Vec::new();
        }
        let Some(message_id) = info.get("id").and_then(Value::as_str) else {
            return Vec::new();
        };
        if !self.emitted_usage_messages.insert(message_id.to_string()) {
            return Vec::new();
        }
        vec![AgentEvent::Usage {
            usage: usage_from(info),
        }]
    }

    fn child_completed(&mut self, child: &str) -> Vec<AgentEvent> {
        if !self.completed_children.insert(child.to_string()) {
            return Vec::new();
        }
        vec![AgentEvent::SubagentCompleted {
            child: child.to_string(),
            result: String::new(),
        }]
    }

    fn message_updated(&mut self, properties: &Value) -> Vec<AgentEvent> {
        let info = properties.get("info").unwrap_or(&Value::Null);
        if info.get("role").and_then(Value::as_str) != Some("assistant") {
            return Vec::new();
        }
        if info.get("parentID").and_then(Value::as_str) != Some(self.user_message_id.as_str()) {
            return Vec::new();
        }
        let Some(message_id) = info.get("id").and_then(Value::as_str) else {
            return Vec::new();
        };
        self.turn_observed = true;
        self.assistant_message_ids.insert(message_id.to_string());
        let mut events = self.drain_pending_parts();
        if info
            .get("time")
            .and_then(|time| time.get("completed"))
            .is_none()
        {
            return events;
        }
        let aborted = info
            .get("error")
            .and_then(|error| error.get("name"))
            .and_then(Value::as_str)
            == Some("MessageAbortedError");
        if let Some(structured) = info.get("structured") {
            if !structured.is_null()
                && self
                    .emitted_structured_messages
                    .insert(message_id.to_string())
            {
                events.push(AgentEvent::TextCompleted {
                    text: serde_json::to_string(structured).unwrap_or_default(),
                });
            }
        }
        if self.emitted_usage_messages.insert(message_id.to_string()) {
            events.push(AgentEvent::Usage {
                usage: usage_from(info),
            });
        }
        if aborted {
            events.push(AgentEvent::Aborted);
        }
        events
    }

    fn part_delta(&mut self, properties: &Value) -> Vec<AgentEvent> {
        if !self.turn_observed {
            self.queue_pending_part(PendingPartEvent::Delta(properties.clone()));
            return Vec::new();
        }
        if !self.relevant_part(properties) {
            return Vec::new();
        }
        if properties.get("field").and_then(Value::as_str) != Some("text") {
            return Vec::new();
        }
        let delta = properties
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or("");
        if delta.is_empty() {
            return Vec::new();
        }
        let part_id = properties
            .get("partID")
            .and_then(Value::as_str)
            .unwrap_or("");
        match self.part_kinds.get(part_id).map(String::as_str) {
            Some("reasoning") => {
                let mut events = Vec::new();
                if !self.reasoning_active {
                    self.reasoning_active = true;
                    events.push(AgentEvent::ReasoningStarted);
                }
                events.push(AgentEvent::ReasoningDelta {
                    text: delta.to_string(),
                });
                events
            }
            _ => vec![AgentEvent::TextDelta {
                text: delta.to_string(),
            }],
        }
    }

    fn part_updated(&mut self, properties: &Value) -> Vec<AgentEvent> {
        if !self.turn_observed {
            self.queue_pending_part(PendingPartEvent::Updated(properties.clone()));
            return Vec::new();
        }
        if !self.relevant_part(properties) {
            return Vec::new();
        }
        let part = properties.get("part").unwrap_or(&Value::Null);
        let part_id = part
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let kind = part
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if !part_id.is_empty() {
            self.part_kinds.insert(part_id.clone(), kind.to_string());
        }
        match kind {
            "text" => {
                if part.get("time").and_then(|time| time.get("end")).is_some()
                    && self.completed_parts.insert(part_id)
                {
                    vec![AgentEvent::TextCompleted {
                        text: part
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    }]
                } else {
                    Vec::new()
                }
            }
            "reasoning" => {
                let mut events = Vec::new();
                if !self.reasoning_active {
                    self.reasoning_active = true;
                    events.push(AgentEvent::ReasoningStarted);
                }
                if part.get("time").and_then(|time| time.get("end")).is_some()
                    && self.completed_parts.insert(part_id)
                {
                    self.reasoning_active = false;
                    events.push(AgentEvent::ReasoningCompleted);
                }
                events
            }
            "tool" => self.tool_updated(part),
            // `message.updated` is the authoritative per-assistant-message
            // usage record. Step parts may repeat the same totals and must not
            // be emitted independently.
            "step-finish" => Vec::new(),
            _ => Vec::new(),
        }
    }

    fn queue_pending_part(&mut self, event: PendingPartEvent) {
        if self.pending_part_events.len() < MAX_PENDING_PART_EVENTS {
            self.pending_part_events.push(event);
        }
    }

    fn drain_pending_parts(&mut self) -> Vec<AgentEvent> {
        let pending = std::mem::take(&mut self.pending_part_events);
        let mut events = Vec::new();
        for event in pending {
            events.extend(match event {
                PendingPartEvent::Delta(properties) => self.part_delta(&properties),
                PendingPartEvent::Updated(properties) => self.part_updated(&properties),
            });
        }
        events
    }

    fn relevant_part(&self, properties: &Value) -> bool {
        let message_id = properties
            .get("messageID")
            .and_then(Value::as_str)
            .or_else(|| {
                properties
                    .get("part")
                    .and_then(|part| part.get("messageID"))
                    .and_then(Value::as_str)
            });
        message_id.is_some_and(|id| self.assistant_message_ids.contains(id))
    }

    fn tool_updated(&mut self, part: &Value) -> Vec<AgentEvent> {
        let state = part.get("state").unwrap_or(&Value::Null);
        let status = state
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending");
        let call_id = part
            .get("callID")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !call_id.is_empty()
            && self.tool_statuses.get(&call_id).map(String::as_str) == Some(status)
        {
            return Vec::new();
        }
        if !call_id.is_empty() {
            self.tool_statuses
                .insert(call_id.clone(), status.to_string());
        }
        let call = ToolCallView {
            call_id: call_id.clone(),
            name: part
                .get("tool")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
            arguments: state.get("input").cloned().unwrap_or(Value::Null),
            title: state
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        };
        match status {
            "pending" => vec![AgentEvent::ToolQueued { call }],
            "running" => vec![AgentEvent::ToolStarted { call }],
            "completed" => {
                let part_id = part
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if !self.completed_parts.insert(part_id) {
                    return Vec::new();
                }
                vec![AgentEvent::ToolCompleted {
                    call_id,
                    result: ToolResultView {
                        success: true,
                        exit_code: state
                            .get("metadata")
                            .and_then(|value| value.get("exit_code"))
                            .and_then(Value::as_i64)
                            .map(|value| value as i32),
                        output: state
                            .get("output")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        metadata: state.get("metadata").cloned().unwrap_or(Value::Null),
                    },
                }]
            }
            "error" => vec![AgentEvent::ToolFailed {
                call_id,
                error: UserFacingError {
                    code: "tool_error".into(),
                    message: state
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("OpenCode tool failed")
                        .to_string(),
                    retryable: false,
                },
            }],
            _ => Vec::new(),
        }
    }

    fn session_status(&mut self, properties: &Value) -> Vec<AgentEvent> {
        match properties
            .get("status")
            .and_then(|status| status.get("type"))
            .and_then(Value::as_str)
        {
            Some("idle") => self.session_idle(),
            Some("retry") => {
                vec![AgentEvent::Reconnecting {
                    attempt: properties
                        .get("status")
                        .and_then(|status| status.get("attempt"))
                        .and_then(Value::as_u64)
                        .unwrap_or(1) as u32,
                }]
            }
            Some(_) => Vec::new(),
            None => Vec::new(),
        }
    }

    fn session_idle(&self) -> Vec<AgentEvent> {
        if !self.turn_observed {
            // `/global/event` may publish the session's existing idle state
            // immediately after subscribe and before prompt_async admission.
            // Treating that as this turn's completion drops the foreground
            // lease before the new provider request begins.
            return Vec::new();
        }
        vec![AgentEvent::Completed {
            summary: String::new(),
        }]
    }
}

fn usage_from(value: &Value) -> UsageDelta {
    let tokens = value.get("tokens").unwrap_or(&Value::Null);
    UsageDelta {
        input_tokens: number_u64(tokens.get("input")),
        output_tokens: number_u64(tokens.get("output")),
        reasoning_tokens: number_u64(tokens.get("reasoning")),
        cache_read_tokens: number_u64(tokens.get("cache").and_then(|cache| cache.get("read"))),
        cache_write_tokens: number_u64(tokens.get("cache").and_then(|cache| cache.get("write"))),
        cost_usd: value.get("cost").and_then(Value::as_f64),
    }
}

fn number_u64(value: Option<&Value>) -> u64 {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_f64().map(|n| n.max(0.0) as u64))
        })
        .unwrap_or(0)
}

fn map_error(value: Option<&Value>) -> UserFacingError {
    let value = value.unwrap_or(&Value::Null);
    UserFacingError {
        code: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("opencode_error")
            .to_string(),
        message: value
            .get("data")
            .and_then(|data| data.get("message"))
            .and_then(Value::as_str)
            .or_else(|| value.get("message").and_then(Value::as_str))
            .unwrap_or("OpenCode agent failed")
            .to_string(),
        retryable: false,
    }
}

fn map_diff(value: &Value) -> Option<DiffView> {
    let path = value
        .get("file")
        .or_else(|| value.get("path"))
        .and_then(Value::as_str)?;
    Some(DiffView {
        path: path.to_string(),
        patch: value
            .get("patch")
            .or_else(|| value.get("diff"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn envelope(kind: &str, properties: Value) -> Value {
        serde_json::json!({"directory":"/tmp","payload":{"type":kind,"properties":properties}})
    }

    #[test]
    fn maps_deltas_tools_usage_and_idle_without_duplicates() {
        let mut mapper = EventMapper::new("ses_1", "msg_user");
        let stale_idle = envelope(
            "session.status",
            serde_json::json!({"sessionID":"ses_1","status":{"type":"idle"}}),
        );
        assert!(
            mapper.map(&stale_idle).is_empty(),
            "an idle state observed before current-turn activity is stale"
        );
        let assistant_started = envelope(
            "message.updated",
            serde_json::json!({"sessionID":"ses_1","info":{
                "id":"msg_assistant","role":"assistant","parentID":"msg_user",
                "time":{"created":1}
            }}),
        );
        assert!(mapper.map(&assistant_started).is_empty());
        let running = envelope(
            "message.part.updated",
            serde_json::json!({
                "sessionID":"ses_1",
                "part":{"id":"prt_1","messageID":"msg_assistant","type":"tool",
                    "callID":"call_1","tool":"aishe_read_file",
                    "state":{"status":"running","input":{"path":"a"},"title":"Read","time":{"start":1}}}
            }),
        );
        assert!(matches!(
            mapper.map(&running).as_slice(),
            [AgentEvent::ToolStarted { .. }]
        ));
        let completed = envelope(
            "message.part.updated",
            serde_json::json!({
                "sessionID":"ses_1",
                "part":{"id":"prt_1","messageID":"msg_assistant","type":"tool",
                    "callID":"call_1","tool":"aishe_read_file",
                    "state":{"status":"completed","input":{"path":"a"},"output":"ok","title":"Read",
                        "metadata":{},"time":{"start":1,"end":2}}}
            }),
        );
        assert!(matches!(
            mapper.map(&completed).as_slice(),
            [AgentEvent::ToolCompleted { .. }]
        ));
        assert!(mapper.map(&completed).is_empty());

        let idle = envelope(
            "session.status",
            serde_json::json!({"sessionID":"ses_1","status":{"type":"idle"}}),
        );
        assert!(matches!(
            mapper.map(&idle).as_slice(),
            [AgentEvent::Completed { .. }]
        ));
    }

    #[test]
    fn ignores_other_sessions_except_direct_children() {
        let mut mapper = EventMapper::new("ses_parent", "msg_user");
        let other = envelope(
            "message.part.delta",
            serde_json::json!({"sessionID":"ses_other","partID":"p","field":"text","delta":"no"}),
        );
        assert!(mapper.map(&other).is_empty());
        let child = envelope(
            "session.created",
            serde_json::json!({"sessionID":"ses_child","info":{"parentID":"ses_parent","agent":"explore"}}),
        );
        assert!(matches!(
            mapper.map(&child).as_slice(),
            [AgentEvent::SubagentStarted { .. }]
        ));
    }

    #[test]
    fn usage_is_authoritative_once_and_opencode_approvals_fail_closed() {
        let mut mapper = EventMapper::new("ses_1", "msg_user");
        let step = envelope(
            "message.part.updated",
            serde_json::json!({
                "sessionID":"ses_1",
                "part":{"id":"prt_step","messageID":"msg_assistant","type":"step-finish",
                    "tokens":{"input":5,"output":2},"cost":0.01}
            }),
        );
        assert!(mapper.map(&step).is_empty());
        let message = envelope(
            "message.updated",
            serde_json::json!({
                "sessionID":"ses_1",
                "info":{
                    "id":"msg_assistant","role":"assistant","parentID":"msg_user",
                    "time":{"completed":2},"tokens":{"input":5,"output":2},"cost":0.01
                }
            }),
        );
        assert!(matches!(
            mapper.map(&message).as_slice(),
            [AgentEvent::Usage { .. }]
        ));
        assert!(mapper.map(&message).is_empty());

        let permission = envelope(
            "permission.asked",
            serde_json::json!({"sessionID":"ses_1","id":"per_1","permission":"bash"}),
        );
        assert!(matches!(
            mapper.map(&permission).as_slice(),
            [AgentEvent::Failed { error }] if error.code == "opencode_permission_escape"
        ));
    }

    #[test]
    fn structured_output_and_child_usage_are_authoritative_once() {
        let mut mapper = EventMapper::new("ses_parent", "msg_user");
        let structured = envelope(
            "message.updated",
            serde_json::json!({
                "sessionID":"ses_parent",
                "info":{
                    "id":"msg_answer","role":"assistant","parentID":"msg_user",
                    "time":{"completed":2},
                    "structured":{"type":"answer","command":"","explanation":"Paris"},
                    "tokens":{"input":9,"output":3},"cost":0.01
                }
            }),
        );
        let mapped = mapper.map(&structured);
        assert!(matches!(
            mapped.as_slice(),
            [AgentEvent::TextCompleted { text }, AgentEvent::Usage { .. }]
                if text.contains("\"explanation\":\"Paris\"")
        ));
        assert!(mapper.map(&structured).is_empty());

        let child = envelope(
            "session.created",
            serde_json::json!({
                "sessionID":"ses_child",
                "info":{"parentID":"ses_parent","agent":"explore"}
            }),
        );
        assert!(matches!(
            mapper.map(&child).as_slice(),
            [AgentEvent::SubagentStarted { .. }]
        ));
        let child_usage = envelope(
            "message.updated",
            serde_json::json!({
                "sessionID":"ses_child",
                "info":{
                    "id":"msg_child_answer","role":"assistant","parentID":"msg_child_user",
                    "time":{"completed":3},"tokens":{"input":4,"output":2},"cost":0.005
                }
            }),
        );
        assert!(matches!(
            mapper.map(&child_usage).as_slice(),
            [AgentEvent::Usage { usage }] if usage.input_tokens == 4
        ));
        assert!(mapper.map(&child_usage).is_empty());
        let idle = envelope(
            "session.status",
            serde_json::json!({"sessionID":"ses_child","status":{"type":"idle"}}),
        );
        assert!(matches!(
            mapper.map(&idle).as_slice(),
            [AgentEvent::SubagentCompleted { .. }]
        ));
        assert!(mapper.map(&idle).is_empty());
    }

    #[test]
    fn pinned_event_fixture_maps_the_supported_runtime_surface() {
        let mut mapper = EventMapper::new("ses_fixture", "msg_user");
        let mut mapped = Vec::new();
        for line in include_str!("../../../tests/fixtures/opencode/v1.18.9/events.jsonl").lines() {
            mapped.extend(mapper.map(&serde_json::from_str(line).unwrap()));
        }
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::ReasoningStarted)));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::ReasoningCompleted)));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolStarted { .. })));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolCompleted { .. })));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::TodoUpdated { .. })));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::Diff { .. })));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::Compacted)));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::TextCompleted { text } if text == "Done.")));
        assert!(mapped
            .iter()
            .any(|event| matches!(event, AgentEvent::Usage { usage } if usage.input_tokens == 12)));
        assert!(matches!(mapped.last(), Some(AgentEvent::Completed { .. })));
    }
}
