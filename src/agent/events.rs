use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCallView {
    pub call_id: String,
    pub name: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default)]
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolResultView {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub output: String,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiffView {
    pub path: String,
    pub patch: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageDelta {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserFacingError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub request_id: String,
    pub title: String,
    pub detail: String,
    pub dangerous: bool,
    /// Backend-neutral identity of the agent requesting approval, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Durable task/session identity blocked on this approval, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub request_id: String,
    pub prompt: String,
    /// Backend-neutral identity of the agent asking, when the adapter supplies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Durable task/session identity waiting for the answer, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

/// Stable AIShe event schema consumed by every renderer and scripting surface.
/// Unknown backend events are intentionally not represented here; adapters log
/// and ignore them instead of leaking raw backend payloads to the terminal.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Connected,
    SessionCreated {
        session_id: String,
    },
    UserPromptAccepted {
        text: String,
    },
    ReasoningStarted,
    ReasoningDelta {
        text: String,
    },
    ReasoningCompleted,
    TextDelta {
        text: String,
    },
    TextCompleted {
        text: String,
    },
    ToolQueued {
        call: ToolCallView,
    },
    ToolStarted {
        call: ToolCallView,
    },
    ToolOutput {
        call_id: String,
        stream: OutputStream,
        chunk: String,
    },
    ToolCompleted {
        call_id: String,
        result: ToolResultView,
    },
    ToolFailed {
        call_id: String,
        error: UserFacingError,
    },
    Diff {
        diff: DiffView,
    },
    TodoUpdated {
        items: Vec<TodoItem>,
    },
    SubagentStarted {
        parent: String,
        child: String,
        agent: String,
    },
    SubagentCompleted {
        child: String,
        result: String,
    },
    Usage {
        usage: UsageDelta,
    },
    Compacted,
    WaitingForApproval {
        request: ApprovalRequest,
    },
    WaitingForUser {
        request: UserQuestion,
    },
    Reconnecting {
        attempt: u32,
    },
    Reconciled,
    Aborted,
    Completed {
        summary: String,
    },
    Failed {
        error: UserFacingError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_waiting_events_deserialize_without_identity_fields() {
        let question: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "waiting_for_user",
            "request": {"request_id": "q1", "prompt": "Choose a region"}
        }))
        .unwrap();
        let AgentEvent::WaitingForUser { request } = question else {
            panic!("expected waiting_for_user");
        };
        assert_eq!(request.agent, None);
        assert_eq!(request.task, None);

        let approval: AgentEvent = serde_json::from_value(serde_json::json!({
            "type": "waiting_for_approval",
            "request": {
                "request_id": "a1",
                "title": "Run it?",
                "detail": "host action",
                "dangerous": true
            }
        }))
        .unwrap();
        let AgentEvent::WaitingForApproval { request } = approval else {
            panic!("expected waiting_for_approval");
        };
        assert_eq!(request.agent, None);
        assert_eq!(request.task, None);
    }

    #[test]
    fn waiting_identity_round_trips_when_present() {
        let event = AgentEvent::WaitingForUser {
            request: UserQuestion {
                request_id: "q2".into(),
                prompt: "Continue?".into(),
                agent: Some("planner".into()),
                task: Some("task-9".into()),
            },
        };
        let encoded = serde_json::to_value(&event).unwrap();
        assert_eq!(encoded["request"]["agent"], "planner");
        assert_eq!(encoded["request"]["task"], "task-9");
        assert_eq!(
            serde_json::from_value::<AgentEvent>(encoded).unwrap(),
            event
        );
    }
}
