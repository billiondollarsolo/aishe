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

#[derive(Clone, Copy, Debug)]
pub struct TurnOptions {
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    pub interactive: bool,
    pub render: bool,
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
        Ok(Self {
            mode,
            scope,
            network,
            interactive: std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
            render,
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
        config.clone(),
        Arc::clone(&cancelled),
    )
    .map_err(TurnFailure::PreAdmission)?;

    // After the subscribed submit attempt begins, transport failure is
    // conservatively treated as potentially admitted. This prevents native
    // fallback from issuing a duplicate provider request.
    let handle = backend
        .submit(PromptRequest {
            session: session.clone(),
            text: prompt.to_string(),
            mode: options.mode,
            max_output_tokens: (config.backend.max_output_tokens > 0)
                .then_some(config.backend.max_output_tokens),
        })
        .map_err(TurnFailure::Admitted)?;

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
            TurnFailure::Admitted(anyhow::anyhow!(
                "starting managed-turn interrupt monitor: {error}"
            ))
        })?;

    let mut renderer = options
        .render
        .then(|| super::renderer::AgentRenderer::new(&config.backend.output));
    let mut callback = |event: &AgentEvent| {
        if let Some(renderer) = renderer.as_mut() {
            renderer.render(event);
        }
    };
    let events_result = backend.events_with(&handle, &mut callback);
    monitor_done.store(true, Ordering::SeqCst);
    let _ = monitor.join();
    drop(worker);
    let events = events_result.map_err(TurnFailure::Admitted)?;
    if cancelled.load(Ordering::SeqCst) {
        crate::audit::action(
            "agent:abort",
            &format!("backend=opencode session={}", session.id),
            Some(130),
        );
        return Err(TurnFailure::Admitted(anyhow::anyhow!(
            "agent turn interrupted; provider and tool processes were cancelled"
        )));
    }

    if let Some(error) = events.iter().rev().find_map(|event| match event {
        AgentEvent::Failed { error } => Some(error),
        _ => None,
    }) {
        return Err(TurnFailure::Admitted(anyhow::anyhow!(
            "{} ({})",
            error.message,
            error.code
        )));
    }
    let text = collect_text(&events);
    let usage = collect_usage(&events);
    crate::audit::action(
        "agent:turn",
        &format!(
            "backend=opencode session={} mode={:?} scope={:?}",
            session.id, options.mode, options.scope
        ),
        Some(0),
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
        let usage = collect_usage(&events);
        assert_eq!(usage.input_tokens, 10);
        assert_eq!(usage.output_tokens, 2);
        assert_eq!(usage.cost_usd, Some(0.01));
    }
}
