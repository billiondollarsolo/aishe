//! Backend-neutral agent contracts.
//!
//! OpenCode is an implementation detail behind these types. Shell routing,
//! rendering, policy, accounting, and the foreground tool executor depend only
//! on this module so a protocol upgrade cannot leak into the zsh interface.

pub mod backend;
pub mod events;
pub mod policy;
pub mod tool_worker;

pub use backend::{
    AgentBackend, BackendHealth, BackendSession, PromptHandle, PromptRequest, SessionFilter,
    SessionRequest, SessionSnapshot, SessionSummary,
};
pub use events::{
    AgentEvent, ApprovalRequest, DiffView, OutputStream, TodoItem, ToolCallView, ToolResultView,
    UsageDelta, UserFacingError, UserQuestion,
};
pub use policy::{ExecutionScope, Mode, NetworkPolicy};
pub use tool_worker::ToolWorker;
