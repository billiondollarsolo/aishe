use std::collections::HashMap;
use std::io::BufRead;
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::agent::{
    AgentBackend, AgentEvent, BackendHealth, BackendSession, PromptHandle, PromptRequest,
    SessionFilter, SessionRequest, SessionSnapshot, SessionSummary,
};

use super::session::{SessionBinding, SessionStore};
use super::OpenCodeClient;

pub struct OpenCodeBackend {
    client: OpenCodeClient,
    streams: Mutex<HashMap<String, Box<dyn BufRead + Send>>>,
    resume_idle: Mutex<HashMap<String, BackendSession>>,
    sessions: SessionStore,
}

impl OpenCodeBackend {
    pub fn new(client: OpenCodeClient) -> Result<Self> {
        Ok(Self {
            client,
            streams: Mutex::new(HashMap::new()),
            resume_idle: Mutex::new(HashMap::new()),
            sessions: SessionStore::from_default_root()?,
        })
    }

    #[cfg(test)]
    pub fn with_session_store(client: OpenCodeClient, sessions: SessionStore) -> Self {
        Self {
            client,
            streams: Mutex::new(HashMap::new()),
            resume_idle: Mutex::new(HashMap::new()),
            sessions,
        }
    }

    pub fn events_with(
        &self,
        handle: &PromptHandle,
        emit: &mut dyn FnMut(&AgentEvent),
    ) -> Result<Vec<AgentEvent>> {
        if self
            .resume_idle
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenCode resume registry is poisoned"))?
            .remove(&handle.message_id)
            .is_some()
        {
            let events = self.client.reconciled_events(handle)?;
            for event in &events {
                emit(event);
            }
            return Ok(events);
        }
        let stream = self
            .streams
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenCode stream registry is poisoned"))?
            .remove(&handle.message_id)
            .context("OpenCode prompt has no subscribed event stream")?;
        self.client.read_events_with(handle, stream, emit)
    }
}

impl AgentBackend for OpenCodeBackend {
    fn name(&self) -> &'static str {
        "opencode"
    }

    fn health(&self) -> Result<BackendHealth> {
        match self.client.health() {
            Ok(_) => Ok(BackendHealth::Ready),
            Err(error) => Ok(BackendHealth::Unavailable {
                reason: crate::redact::redact(&error.to_string()),
            }),
        }
    }

    fn ensure_session(&self, request: SessionRequest) -> Result<BackendSession> {
        let workspace = SessionStore::resolve_workspace(&request.workspace)?;
        let binding = SessionBinding::new(
            &request.connection_id,
            &request.model_id,
            request.mode,
            request.scope,
            request.network,
        );
        if let Some(resume_id) = request.resume_id.as_deref() {
            let session = BackendSession {
                id: resume_id.to_string(),
                workspace,
                backend: "opencode".into(),
            };
            self.client
                .session(&session)?
                .context("requested OpenCode session does not exist")?;
            self.sessions.bind(&request.shell_id, &session, binding)?;
            return Ok(session);
        }
        if let Some(mapping) = self.sessions.find(
            &request.shell_id,
            &workspace,
            &request.connection_id,
            &request.model_id,
        )? {
            if mapping.matches_authority(request.mode, request.scope, request.network) {
                let session = BackendSession {
                    id: mapping.backend_session_id,
                    workspace: mapping.workspace,
                    backend: "opencode".into(),
                };
                // Only a definite 404 permits replacement. Connectivity or auth
                // errors bubble up so they cannot create duplicate conversations.
                if self.client.session(&session)?.is_some() {
                    self.sessions.bind(&request.shell_id, &session, binding)?;
                    return Ok(session);
                }
            }
        }
        self.sessions.serialize_creation(|| {
            // Another process may have created this shell/workspace session
            // while we waited for the creation lock. Recheck before POSTing.
            if let Some(mapping) = self.sessions.find(
                &request.shell_id,
                &workspace,
                &request.connection_id,
                &request.model_id,
            )? {
                if mapping.matches_authority(request.mode, request.scope, request.network) {
                    let session = BackendSession {
                        id: mapping.backend_session_id,
                        workspace: mapping.workspace,
                        backend: "opencode".into(),
                    };
                    if self.client.session(&session)?.is_some() {
                        self.sessions.bind(&request.shell_id, &session, binding)?;
                        return Ok(session);
                    }
                }
            }
            let session = self.client.create_session(
                &workspace,
                "AIShe session",
                request.scope,
                request.network,
            )?;
            self.sessions.bind(&request.shell_id, &session, binding)?;
            Ok(session)
        })
    }

    fn submit(&self, request: PromptRequest) -> Result<PromptHandle> {
        let (handle, stream) = self.client.submit(&request)?;
        self.streams
            .lock()
            .map_err(|_| anyhow::anyhow!("OpenCode stream registry is poisoned"))?
            .insert(handle.message_id.clone(), stream);
        Ok(handle)
    }

    fn events(&self, handle: &PromptHandle) -> Result<Vec<AgentEvent>> {
        self.events_with(handle, &mut |_| {})
    }

    fn snapshot(&self, session: &BackendSession) -> Result<SessionSnapshot> {
        self.client.snapshot(session)
    }

    fn abort(&self, session: &BackendSession) -> Result<()> {
        self.client.abort(session)
    }

    fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionSummary>> {
        let mut sessions = self.client.list_sessions(filter.workspace.as_deref())?;
        if let Some(shell_id) = filter.shell_id.as_deref() {
            let allowed = self
                .sessions
                .records(Some(shell_id))?
                .into_iter()
                .map(|record| record.backend_session_id)
                .collect::<std::collections::HashSet<_>>();
            sessions.retain(|session| allowed.contains(&session.id));
        }
        Ok(sessions)
    }

    fn resume(&self, session: &BackendSession) -> Result<PromptHandle> {
        let (handle, stream, busy) = self.client.resume(session)?;
        if busy {
            self.streams
                .lock()
                .map_err(|_| anyhow::anyhow!("OpenCode stream registry is poisoned"))?
                .insert(handle.message_id.clone(), stream);
        } else {
            self.resume_idle
                .lock()
                .map_err(|_| anyhow::anyhow!("OpenCode resume registry is poisoned"))?
                .insert(handle.message_id.clone(), session.clone());
        }
        Ok(handle)
    }
}
