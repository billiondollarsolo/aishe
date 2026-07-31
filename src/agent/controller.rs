//! Aishe-owned prompt lifecycle.
//!
//! This is the only normal path from shell-routed natural language to the
//! managed OpenCode backend. It establishes the foreground authority lease
//! before prompt admission and distinguishes safe pre-admission fallback from
//! failures where retrying could duplicate cost or effects.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use rand::RngCore;

use super::{
    AgentBackend, AgentEvent, BackendHealth, ExecutionScope, Mode, NetworkPolicy, PromptRequest,
    SessionRequest, ToolWorker, UsageDelta,
};
use crate::backend::bridge::LeaseRegistration;
use crate::backend::control::SupervisorClient;
use crate::backend::opencode::{OpenCodeBackend, OpenCodeClient};
use crate::config::Config;

/// Process-local interrupt state shared by the native and managed loops.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Debug)]
pub struct TurnOptions {
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    pub interactive: bool,
    pub render: bool,
    pub output: String,
}

impl TurnOptions {
    pub fn from_config(config: &Config, mode: Mode, render: bool) -> Result<Self> {
        let scope = ExecutionScope::parse(&config.backend.default_scope)
            .context("backend.default_scope must be workspace or host")?;
        if scope == ExecutionScope::Host && !config.sandbox.allow_host_yolo && mode == Mode::Yolo {
            anyhow::bail!("organization configuration disables yolo host scope");
        }
        let network = if scope == ExecutionScope::Host {
            NetworkPolicy::Allow
        } else {
            NetworkPolicy::parse(&config.backend.workspace_network)
                .context("backend.workspace_network must be allow or deny")?
        };
        let output = std::env::var("AISHE_AGENT_OUTPUT")
            .unwrap_or_else(|_| config.backend.output.clone())
            .trim()
            .to_ascii_lowercase();
        if !matches!(output.as_str(), "focus" | "compact" | "detailed") {
            anyhow::bail!("backend.output must be focus, compact, or detailed");
        }
        Ok(Self {
            mode,
            scope,
            network,
            interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            render,
            output,
        })
    }
}

#[derive(Clone, Debug)]
pub struct TurnOutcome {
    pub session_id: String,
    pub workspace: PathBuf,
    pub text: String,
    pub events: Vec<AgentEvent>,
    pub usage: UsageDelta,
    pub elapsed_ms: u128,
}

#[derive(Debug)]
pub enum TurnFailure {
    /// It is safe for the caller to use the configured compatibility backend.
    PreAdmission(anyhow::Error),
    /// The prompt may have incurred cost or effects and must never be retried
    /// automatically through another backend.
    Admitted(anyhow::Error),
}

impl TurnFailure {
    pub fn admitted(&self) -> bool {
        matches!(self, Self::Admitted(_))
    }

    pub fn into_error(self) -> anyhow::Error {
        match self {
            Self::PreAdmission(error) | Self::Admitted(error) => error,
        }
    }
}

impl std::fmt::Display for TurnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PreAdmission(error) => write!(formatter, "{error}"),
            Self::Admitted(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for TurnFailure {}

pub fn run_turn(
    config: &Config,
    prompt: &str,
    options: TurnOptions,
) -> std::result::Result<TurnOutcome, TurnFailure> {
    let started_at = std::time::Instant::now();
    INTERRUPTED.store(false, Ordering::SeqCst);
    if config.backend.engine != "opencode" {
        return Err(TurnFailure::PreAdmission(anyhow::anyhow!(
            "managed agent engine is disabled"
        )));
    }
    let state =
        crate::backend::supervisor::ensure_running(config).map_err(TurnFailure::PreAdmission)?;
    let control = SupervisorClient::new(state).map_err(TurnFailure::PreAdmission)?;
    let client = OpenCodeClient::new(
        control.opencode_connection(),
        control.provider_id(),
        control.model_id(),
    )
    .map_err(TurnFailure::PreAdmission)?;
    let backend = OpenCodeBackend::new(client.clone()).map_err(TurnFailure::PreAdmission)?;
    match backend.health().map_err(TurnFailure::PreAdmission)? {
        BackendHealth::Ready => {}
        BackendHealth::Degraded { reason } | BackendHealth::Unavailable { reason } => {
            return Err(TurnFailure::PreAdmission(anyhow::anyhow!(reason)))
        }
    }

    let shell_id = current_shell_id().map_err(TurnFailure::PreAdmission)?;
    let requested_workspace = std::env::current_dir()
        .context("resolving the current workspace")
        .map_err(TurnFailure::PreAdmission)?;
    let session = backend
        .ensure_session(SessionRequest {
            shell_id: shell_id.clone(),
            workspace: requested_workspace,
            connection_id: config.active_connection_id().to_string(),
            model_id: config.active_model().to_string(),
            mode: options.mode,
            scope: options.scope,
            network: options.network,
            resume_id: None,
        })
        .map_err(TurnFailure::PreAdmission)?;
    let snapshot = backend
        .snapshot(&session)
        .context("reading managed-session usage before admission")
        .map_err(TurnFailure::PreAdmission)?;
    let price = crate::usage::budget_price_for(config.active_model(), &config.pricing);
    let baseline_spent_usd = snapshot.usage.cost_usd.or_else(|| {
        price.map(|price| {
            crate::usage::cost(
                crate::usage::Usage {
                    input: snapshot.usage.input_tokens,
                    output: snapshot.usage.output_tokens,
                    requests: 0,
                },
                price,
            )
        })
    });
    let budget_usd = (config.aishe.budget_usd > 0.0)
        .then_some(config.aishe.budget_usd)
        .filter(|_| price.is_some());

    // Provider-turn authorization and every proxy tool require this foreground
    // lease. It is deliberately established before submit.
    let audit_turn_id = new_turn_id();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker = ToolWorker::start(
        control,
        LeaseRegistration {
            aishe_shell_id: shell_id,
            backend_session_id: session.id.clone(),
            workspace: session.workspace.clone(),
            mode: options.mode,
            scope: options.scope,
            network: options.network,
            interactive: options.interactive,
            budget_usd,
            price,
            baseline_spent_usd: baseline_spent_usd.unwrap_or(0.0),
        },
        audit_turn_id.clone(),
        config.clone(),
        Arc::clone(&cancelled),
        streams_tool_output(&options.output),
    )
    .map_err(TurnFailure::PreAdmission)?;

    // OpenCode marks only subscribe/baseline failures as definitely
    // pre-admission. Once prompt_async is attempted, every error is
    // conservatively treated as admitted so fallback cannot duplicate provider
    // cost or effects.
    let handle = match backend.submit(PromptRequest {
        session: session.clone(),
        text: prompt.to_string(),
        mode: options.mode,
        max_output_tokens: (config.backend.max_output_tokens > 0)
            .then_some(config.backend.max_output_tokens),
    }) {
        Ok(handle) => handle,
        Err(error) => {
            audit_managed_event(
                config,
                &options,
                &session,
                &audit_turn_id,
                None,
                "ai_request",
                serde_json::json!({ "prompt": prompt, "admitted": false }),
            );
            audit_managed_error(
                config,
                &options,
                &session,
                &audit_turn_id,
                None,
                &error.to_string(),
                started_at.elapsed().as_millis(),
                false,
            );
            return Err(classify_submit_failure(error));
        }
    };
    audit_managed_request(
        config,
        &options,
        &session,
        &audit_turn_id,
        &handle.message_id,
        prompt,
    );

    let monitor_done = Arc::new(AtomicBool::new(false));
    let monitor_finished = Arc::clone(&monitor_done);
    let monitor_cancelled = Arc::clone(&cancelled);
    let monitor_client = client.clone();
    let monitor_session = session.clone();
    let monitor = std::thread::Builder::new()
        .name("aishe-agent-interrupt".into())
        .spawn(move || {
            while !monitor_finished.load(Ordering::SeqCst) {
                if INTERRUPTED.load(Ordering::SeqCst) {
                    monitor_cancelled.store(true, Ordering::SeqCst);
                    let _ = monitor_client.abort(&monitor_session);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })
        .map_err(|error| {
            // Submission has already been admitted. Abort explicitly so a
            // monitor allocation failure cannot leave an unobserved provider
            // turn running in the background.
            cancelled.store(true, Ordering::SeqCst);
            let _ = client.abort(&session);
            let error = anyhow::anyhow!("starting managed-turn interrupt monitor: {error}");
            audit_managed_error(
                config,
                &options,
                &session,
                &audit_turn_id,
                Some(&handle.message_id),
                &error.to_string(),
                started_at.elapsed().as_millis(),
                true,
            );
            TurnFailure::Admitted(error)
        })?;

    let mut renderer = options
        .render
        .then(|| super::renderer::AgentRenderer::new(&options.output));
    let mut callback = |event: &AgentEvent| {
        if let Some(renderer) = renderer.as_mut() {
            renderer.render(event);
        }
    };
    let events_result = backend.events_with(&handle, &mut callback);
    monitor_done.store(true, Ordering::SeqCst);
    let _ = monitor.join();
    drop(worker);
    let events = match events_result {
        Ok(events) => events,
        Err(error) => {
            audit_managed_error(
                config,
                &options,
                &session,
                &audit_turn_id,
                Some(&handle.message_id),
                &error.to_string(),
                started_at.elapsed().as_millis(),
                true,
            );
            return Err(TurnFailure::Admitted(error));
        }
    };
    audit_managed_events(
        config,
        &options,
        &session,
        &audit_turn_id,
        &handle.message_id,
        &events,
    );
    if cancelled.load(Ordering::SeqCst) {
        audit_managed_error(
            config,
            &options,
            &session,
            &audit_turn_id,
            Some(&handle.message_id),
            "agent turn interrupted; provider and tool processes were cancelled",
            started_at.elapsed().as_millis(),
            true,
        );
        return Err(TurnFailure::Admitted(anyhow::anyhow!(
            "agent turn interrupted; provider and tool processes were cancelled"
        )));
    }

    if let Some(error) = events.iter().rev().find_map(|event| match event {
        AgentEvent::Failed { error } => Some(error),
        _ => None,
    }) {
        audit_managed_error(
            config,
            &options,
            &session,
            &audit_turn_id,
            Some(&handle.message_id),
            &format!("{} ({})", error.message, error.code),
            started_at.elapsed().as_millis(),
            true,
        );
        return Err(TurnFailure::Admitted(anyhow::anyhow!(
            "{} ({})",
            error.message,
            error.code
        )));
    }
    let text = collect_text(&events);
    let reasoning = collect_reasoning(&events);
    let usage = collect_usage(&events);
    audit_managed_response(
        config,
        &options,
        &session,
        &audit_turn_id,
        &handle.message_id,
        &text,
        &reasoning,
        &usage,
        started_at.elapsed().as_millis(),
    );
    Ok(TurnOutcome {
        session_id: session.id,
        workspace: session.workspace,
        text,
        events,
        usage,
        elapsed_ms: started_at.elapsed().as_millis(),
    })
}

fn streams_tool_output(output: &str) -> bool {
    output.eq_ignore_ascii_case("detailed")
}

fn classify_submit_failure(error: anyhow::Error) -> TurnFailure {
    if error
        .downcast_ref::<crate::backend::opencode::PromptNotAdmitted>()
        .is_some()
    {
        TurnFailure::PreAdmission(error)
    } else {
        TurnFailure::Admitted(error)
    }
}

fn collect_text(events: &[AgentEvent]) -> String {
    let completed = events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextCompleted { text } if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if !completed.is_empty() {
        return completed.join("\n");
    }
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn collect_reasoning(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ReasoningDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn audit_managed_request(
    config: &Config,
    options: &TurnOptions,
    session: &super::BackendSession,
    turn_id: &str,
    message_id: &str,
    prompt: &str,
) {
    audit_managed_event(
        config,
        options,
        session,
        turn_id,
        Some(message_id),
        "ai_request",
        serde_json::json!({ "prompt": prompt }),
    );
}

#[allow(clippy::too_many_arguments)]
fn audit_managed_response(
    config: &Config,
    options: &TurnOptions,
    session: &super::BackendSession,
    turn_id: &str,
    message_id: &str,
    response: &str,
    reasoning: &str,
    usage: &UsageDelta,
    duration_ms: u128,
) {
    let mut fields = serde_json::json!({
        "response": response,
        "tokens_in": usage.input_tokens,
        "tokens_out": usage.output_tokens,
        "reasoning_tokens": usage.reasoning_tokens,
        "cache_read_tokens": usage.cache_read_tokens,
        "cache_write_tokens": usage.cache_write_tokens,
        "cost_usd": usage.cost_usd,
        "duration_ms": duration_ms,
    });
    if !reasoning.is_empty() {
        fields["reasoning"] = serde_json::Value::String(reasoning.to_string());
    }
    audit_managed_event(
        config,
        options,
        session,
        turn_id,
        Some(message_id),
        "ai_response",
        fields,
    );
}

#[allow(clippy::too_many_arguments)]
fn audit_managed_error(
    config: &Config,
    options: &TurnOptions,
    session: &super::BackendSession,
    turn_id: &str,
    message_id: Option<&str>,
    error: &str,
    duration_ms: u128,
    admitted: bool,
) {
    audit_managed_event(
        config,
        options,
        session,
        turn_id,
        message_id,
        "ai_error",
        serde_json::json!({
            "error": error,
            "duration_ms": duration_ms,
            "admitted": admitted,
        }),
    );
}

fn audit_managed_event(
    config: &Config,
    options: &TurnOptions,
    session: &super::BackendSession,
    turn_id: &str,
    message_id: Option<&str>,
    kind: &str,
    fields: serde_json::Value,
) {
    if !crate::audit::is_active() {
        return;
    }
    let mut record = serde_json::json!({
        "backend": "opencode",
        "turn_id": turn_id,
        "provider": config.aishe.provider,
        "model": config.active_model(),
        "mode": options.mode,
        "scope": options.scope,
        "network": options.network,
        "backend_session": session.id,
        "message_id": message_id,
        "workspace": session.workspace,
    });
    if let (Some(record), Some(fields)) = (record.as_object_mut(), fields.as_object()) {
        record.extend(fields.clone());
    }
    crate::audit::event(kind, record);
}

fn audit_managed_events(
    config: &Config,
    options: &TurnOptions,
    session: &super::BackendSession,
    turn_id: &str,
    message_id: &str,
    events: &[AgentEvent],
) {
    if !crate::audit::is_active() {
        return;
    }
    for event in events {
        let (kind, fields) = match event {
            AgentEvent::SessionCreated { session_id } => (
                "agent_event",
                serde_json::json!({ "event": "session_created", "created_session": session_id }),
            ),
            AgentEvent::Diff { diff } => (
                "file_change",
                serde_json::json!({ "path": diff.path, "patch": diff.patch }),
            ),
            AgentEvent::TodoUpdated { items } => (
                "agent_event",
                serde_json::json!({ "event": "todo_updated", "items": items }),
            ),
            AgentEvent::SubagentStarted {
                parent,
                child,
                agent,
            } => (
                "agent_event",
                serde_json::json!({
                    "event": "subagent_started",
                    "parent": parent,
                    "child": child,
                    "agent": agent,
                }),
            ),
            AgentEvent::SubagentCompleted { child, result } => (
                "agent_event",
                serde_json::json!({
                    "event": "subagent_completed",
                    "child": child,
                    "result": result,
                }),
            ),
            AgentEvent::Compacted => ("agent_event", serde_json::json!({ "event": "compacted" })),
            AgentEvent::WaitingForApproval { request } => (
                "agent_event",
                serde_json::json!({ "event": "waiting_for_approval", "request": request }),
            ),
            AgentEvent::WaitingForUser { request } => (
                "agent_event",
                serde_json::json!({ "event": "waiting_for_user", "request": request }),
            ),
            AgentEvent::Reconnecting { attempt } => (
                "agent_event",
                serde_json::json!({ "event": "reconnecting", "attempt": attempt }),
            ),
            AgentEvent::Reconciled => ("agent_event", serde_json::json!({ "event": "reconciled" })),
            AgentEvent::Aborted => ("agent_event", serde_json::json!({ "event": "aborted" })),
            AgentEvent::Completed { summary } => (
                "agent_event",
                serde_json::json!({ "event": "completed", "summary": summary }),
            ),
            AgentEvent::ToolFailed { call_id, error } => (
                "agent_event",
                serde_json::json!({
                    "event": "tool_failed",
                    "call_id": call_id,
                    "error": error,
                }),
            ),
            AgentEvent::Failed { error } => (
                "agent_event",
                serde_json::json!({ "event": "failed", "error": error }),
            ),
            AgentEvent::Connected
            | AgentEvent::UserPromptAccepted { .. }
            | AgentEvent::ReasoningStarted
            | AgentEvent::ReasoningDelta { .. }
            | AgentEvent::ReasoningCompleted
            | AgentEvent::TextDelta { .. }
            | AgentEvent::TextCompleted { .. }
            | AgentEvent::ToolQueued { .. }
            | AgentEvent::ToolStarted { .. }
            | AgentEvent::ToolOutput { .. }
            | AgentEvent::ToolCompleted { .. }
            | AgentEvent::Usage { .. } => continue,
        };
        audit_managed_event(
            config,
            options,
            session,
            turn_id,
            Some(message_id),
            kind,
            fields,
        );
    }
}

fn collect_usage(events: &[AgentEvent]) -> UsageDelta {
    let mut total = UsageDelta::default();
    for usage in events.iter().filter_map(|event| match event {
        AgentEvent::Usage { usage } => Some(usage),
        _ => None,
    }) {
        total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
        total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
        total.reasoning_tokens = total
            .reasoning_tokens
            .saturating_add(usage.reasoning_tokens);
        total.cache_read_tokens = total
            .cache_read_tokens
            .saturating_add(usage.cache_read_tokens);
        total.cache_write_tokens = total
            .cache_write_tokens
            .saturating_add(usage.cache_write_tokens);
        total.cost_usd = match (total.cost_usd, usage.cost_usd) {
            (Some(left), Some(right)) => Some(left + right),
            (None, Some(value)) => Some(value),
            (value, None) => value,
        };
    }
    total
}

pub fn current_shell_id() -> Result<String> {
    if let Ok(value) = std::env::var("AISHE_SHELL_ID") {
        if (16..=128).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Ok(value);
        }
        anyhow::bail!("AISHE_SHELL_ID is invalid");
    }
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn new_turn_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    format!(
        "turn_{}",
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_usage_are_collected_without_delta_duplication() {
        let events = vec![
            AgentEvent::TextDelta { text: "Par".into() },
            AgentEvent::TextDelta { text: "is".into() },
            AgentEvent::TextCompleted {
                text: "Paris".into(),
            },
            AgentEvent::Usage {
                usage: UsageDelta {
                    input_tokens: 10,
                    output_tokens: 2,
                    cost_usd: Some(0.01),
                    ..UsageDelta::default()
                },
            },
        ];
        assert_eq!(collect_text(&events), "Paris");
        assert_eq!(collect_reasoning(&events), "");
        let usage = collect_usage(&events);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 2);
        assert_eq!(usage.cost_usd, Some(0.01));
    }

    #[test]
    fn exposed_reasoning_deltas_are_collected_in_order() {
        let events = vec![
            AgentEvent::ReasoningDelta {
                text: "inspect ".into(),
            },
            AgentEvent::ReasoningDelta {
                text: "then verify".into(),
            },
        ];
        assert_eq!(collect_reasoning(&events), "inspect then verify");
    }

    #[test]
    fn only_detailed_mode_streams_foreground_tool_output() {
        assert!(!streams_tool_output("focus"));
        assert!(!streams_tool_output("compact"));
        assert!(streams_tool_output("detailed"));
        assert!(streams_tool_output("DETAILED"));
    }
}
