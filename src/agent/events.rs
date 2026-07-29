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
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct UserQuestion {
    pub request_id: String,
    pub prompt: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: String,
}

/// Stable Aishe event schema consumed by every renderer and scripting surface.
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
