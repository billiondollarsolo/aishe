use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::{AgentEvent, ExecutionScope, Mode, NetworkPolicy, UsageDelta};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendHealth {
    Ready,
    Degraded { reason: String },
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRequest {
    pub shell_id: String,
    pub workspace: PathBuf,
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    pub resume_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BackendSession {
    pub id: String,
    pub workspace: PathBuf,
    pub backend: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptRequest {
    pub session: BackendSession,
    pub text: String,
    pub max_output_tokens: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PromptHandle {
    pub session_id: String,
    pub message_id: String,
    pub workspace: PathBuf,
    pub prompt_text: String,
    pub resumed: bool,
    /// Once true, compatibility fallback is forbidden because retrying could
    /// duplicate provider cost or side effects.
    pub admitted: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionFilter {
    pub workspace: Option<PathBuf>,
    pub shell_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub workspace: PathBuf,
    pub updated_at_ms: u128,
    pub backend: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub events: Vec<AgentEvent>,
    pub usage: UsageDelta,
    pub busy: bool,
}

pub trait AgentBackend: Send + Sync {
    fn name(&self) -> &'static str;
    fn health(&self) -> Result<BackendHealth>;
    fn ensure_session(&self, request: SessionRequest) -> Result<BackendSession>;
    fn submit(&self, request: PromptRequest) -> Result<PromptHandle>;
    fn events(&self, handle: &PromptHandle) -> Result<Vec<AgentEvent>>;
    fn snapshot(&self, session: &BackendSession) -> Result<SessionSnapshot>;
    fn abort(&self, session: &BackendSession) -> Result<()>;
    fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionSummary>>;
    fn resume(&self, session: &BackendSession) -> Result<PromptHandle>;
}
