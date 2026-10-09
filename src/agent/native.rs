//! Shared admission, outcomes, and effect budgets for native agent turns.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::{ExecutionScope, NetworkPolicy};
use crate::config::Config;
use crate::executor::Executor;
use crate::providers::ToolCall;
use crate::tasks::ExecutionCounters;

/// Terminal states describe what actually happened, independently of the UI or
/// the foreground/background process which owns a turn.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeTurnState {
    Completed,
    Cancelled,
    BudgetExhausted,
    IterationLimit,
    Failed,
    Declined,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NativeTurnOutcome {
    pub task_id: String,
    pub state: NativeTurnState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl NativeTurnOutcome {
    pub fn new(task_id: &str, state: NativeTurnState, detail: Option<String>) -> Self {
        Self {
            task_id: task_id.into(),
            state,
            final_text: None,
            detail,
        }
    }

    pub fn completed(task_id: &str, final_text: Option<String>) -> Self {
        Self {
            task_id: task_id.into(),
            state: NativeTurnState::Completed,
            final_text,
            detail: None,
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self.state {
            NativeTurnState::Completed => 0,
            NativeTurnState::Cancelled => 130,
            NativeTurnState::BudgetExhausted => 124,
            NativeTurnState::IterationLimit => 75,
            NativeTurnState::Failed => 1,
            NativeTurnState::Declined => 2,
        }
    }
}

/// Install the accepted native authority before any model or tool work. Grant
/// UI belongs to the caller; this function never silently widens its scope.
pub fn prepare_executor(executor: &mut Executor, config: &Config, workspace: &Path) -> Result<()> {
    let policy = crate::policy::load()?;
    if let Some(policy) = &policy {
        policy.policy.validate_request(config)?;
    }
    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("backend.default_scope must be workspace or host")?;
    let network = if scope == ExecutionScope::Host {
        if !config.sandbox.allow_host_yolo {
            anyhow::bail!("agent host scope is disabled by policy");
        }
        if policy
            .as_ref()
            .is_some_and(|loaded| loaded.policy.allow_network == Some(false))
        {
            anyhow::bail!(
                "native host scope cannot enforce the organization's network restriction; use workspace scope"
            );
        }
        NetworkPolicy::Allow
    } else {
        NetworkPolicy::parse(&config.backend.workspace_network)
            .context("backend.workspace_network must be allow or deny")?
    };
    let workspace = workspace
        .canonicalize()
        .context("agent workspace is unavailable")?;
    let wrap = if scope == ExecutionScope::Workspace {
        #[cfg(target_os = "linux")]
        {
            let binary = match crate::dependencies::bubblewrap_probe() {
                crate::dependencies::BubblewrapState::Usable { path } => path,
                state => anyhow::bail!(
                    "agent workspace requires functional bubblewrap; current state: {state:?}"
                ),
            };
            let mut wrap = crate::sandbox::agent_bwrap_argv(&workspace, executor.cwd(), network)?;
            wrap[0] = binary.to_string_lossy().into_owned();
            wrap
        }
        #[cfg(not(target_os = "linux"))]
        {
            if config.sandbox.require_functional {
                anyhow::bail!("functional workspace isolation is unavailable on this platform");
            }
            // The native file guard and command policy still bind the accepted
            // root on platforms without the Linux isolation backend.
            crate::sandbox::agent_bwrap_argv(&workspace, executor.cwd(), network)?;
            Vec::new()
        }
    } else {
        Vec::new()
    };
    if scope == ExecutionScope::Host {
        crate::environment::confirm_protected_host_for_executor(config, executor)?;
    }
    // Validate first: a rejected admission must not discard an existing guard.
    executor.restrict_agent_environment(&crate::executor::sensitive_environment_names(config));
    executor.prefer_posix_capture();
    executor.set_sandbox_wrap(wrap);
    executor.set_lean_scope(Some((scope, workspace, network)));
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeLimits {
    pub tool_calls: Option<u32>,
    pub network_calls: Option<u32>,
    pub provider_turns: Option<u32>,
    pub elapsed: Option<Duration>,
    pub cost_usd: Option<f64>,
}

impl NativeLimits {
    pub fn from_environment() -> Result<Self> {
        Ok(Self {
            tool_calls: limit_env("AISHE_TASK_MAX_TOOL_CALLS")?,
            network_calls: limit_env("AISHE_TASK_MAX_NETWORK_CALLS")?,
            provider_turns: limit_env("AISHE_TASK_MAX_PROVIDER_TURNS")?,
            elapsed: limit_env::<u32>("AISHE_TASK_MAX_MINUTES")?
                .map(|minutes| Duration::from_secs(u64::from(minutes) * 60)),
            cost_usd: cost_limit_env("AISHE_TASK_MAX_COST_USD")?,
        })
    }

    /// A resumed task may receive stricter limits, never lose its original cap
    /// merely because a worker environment or saved shell setting changed.
    pub fn constrain(&self, current: &Self) -> Self {
        fn minimum<T: Copy + PartialOrd>(original: Option<T>, current: Option<T>) -> Option<T> {
            match (original, current) {
                (Some(original), Some(current)) => Some(if current < original {
                    current
                } else {
                    original
                }),
                (original, current) => original.or(current),
            }
        }
        Self {
            tool_calls: minimum(self.tool_calls, current.tool_calls),
            network_calls: minimum(self.network_calls, current.network_calls),
            provider_turns: minimum(self.provider_turns, current.provider_turns),
            elapsed: minimum(self.elapsed, current.elapsed),
            cost_usd: minimum(self.cost_usd, current.cost_usd),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self
            .cost_usd
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
            || self.tool_calls == Some(0)
            || self.network_calls == Some(0)
            || self.provider_turns == Some(0)
            || self.elapsed == Some(Duration::ZERO)
        {
            anyhow::bail!("task limits must be positive and finite when set");
        }
        Ok(())
    }
}

fn limit_env<T>(name: &str) -> Result<Option<T>>
where
    T: std::str::FromStr + PartialOrd + Default,
{
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    let parsed = value
        .to_str()
        .and_then(|value| value.parse::<T>().ok())
        .with_context(|| format!("{name} must be a non-negative number"))?;
    if parsed < T::default() {
        anyhow::bail!("{name} must be a non-negative number");
    }
    Ok((parsed > T::default()).then_some(parsed))
}

fn cost_limit_env(name: &str) -> Result<Option<f64>> {
    let limit = limit_env::<f64>(name)?;
    // NaN does not compare equal, greater, or less than zero.
    if std::env::var(name)
        .ok()
        .is_some_and(|value| value.parse::<f64>().is_ok_and(|value| !value.is_finite()))
    {
        anyhow::bail!("{name} must be finite and non-negative");
    }
    Ok(limit)
}

/// Counters are reserved and checkpointed before external effects. Resume
/// starts with the previous totals, so restarting a worker cannot reset them.
pub(crate) struct NativeBudget {
    limits: NativeLimits,
    counters: ExecutionCounters,
    started: Instant,
}

impl NativeBudget {
    pub fn new(limits: NativeLimits, counters: ExecutionCounters) -> Self {
        Self {
            limits,
            counters,
            started: Instant::now(),
        }
    }

    pub fn counters(&self) -> ExecutionCounters {
        let mut counters = self.counters;
        counters.elapsed_ms = counters
            .elapsed_ms
            .saturating_add(self.started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64);
        counters
    }

    pub fn record_cost(&mut self, cost_usd: f64) {
        self.counters.cost_usd += cost_usd.max(0.0);
    }

    pub fn requires_cost_accounting(&self) -> bool {
        self.limits.cost_usd.is_some()
    }

    #[cfg(test)]
    fn elapsed_limit(&mut self, elapsed: Duration) {
        self.limits.elapsed = Some(elapsed);
    }

    pub fn exhausted(&self) -> Option<String> {
        if self
            .limits
            .elapsed
            .is_some_and(|limit| Duration::from_millis(self.counters().elapsed_ms) >= limit)
        {
            return Some("task wall-clock budget is exhausted".into());
        }
        if self
            .limits
            .cost_usd
            .is_some_and(|max| self.counters.cost_usd >= max)
        {
            return Some("task cost budget is exhausted".into());
        }
        None
    }

    pub fn admit_provider(&mut self) -> std::result::Result<(), String> {
        self.check_provider()?;
        self.counters.provider_turns = self.counters.provider_turns.saturating_add(1);
        Ok(())
    }

    pub fn check_provider(&self) -> std::result::Result<(), String> {
        if let Some(reason) = self.exhausted() {
            return Err(reason);
        }
        if self
            .limits
            .provider_turns
            .is_some_and(|max| self.counters.provider_turns >= max)
        {
            return Err("task provider-turn budget is exhausted".into());
        }
        Ok(())
    }

    pub fn admit_tool(&mut self, call: &ToolCall) -> std::result::Result<(), String> {
        if let Some(reason) = self.exhausted() {
            return Err(reason);
        }
        if self
            .limits
            .tool_calls
            .is_some_and(|max| self.counters.tool_calls >= max)
        {
            return Err("task tool-call budget is exhausted".into());
        }
        let network = tool_uses_network(call);
        if network
            && self
                .limits
                .network_calls
                .is_some_and(|max| self.counters.network_calls >= max)
        {
            return Err("task network-call budget is exhausted".into());
        }
        self.counters.tool_calls = self.counters.tool_calls.saturating_add(1);
        if network {
            self.counters.network_calls = self.counters.network_calls.saturating_add(1);
        }
        Ok(())
    }

    pub fn command_timeout(&self, default: Duration) -> Duration {
        self.limits.elapsed.map_or(default, |limit| {
            default.min(limit.saturating_sub(Duration::from_millis(self.counters().elapsed_ms)))
        })
    }

    pub fn remaining(&self) -> Option<Duration> {
        self.limits
            .elapsed
            .map(|limit| limit.saturating_sub(Duration::from_millis(self.counters().elapsed_ms)))
    }
}

fn tool_uses_network(call: &ToolCall) -> bool {
    // An opaque MCP server may contact a remote service even when its name
    // sounds local; reserve a network operation conservatively.
    call.name == "fetch_url"
        || crate::mcp::is_mcp_tool(&call.name)
        || call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(crate::sandbox::is_network_command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn all_terminal_outcomes_have_distinct_non_success_exits() {
        let cases = [
            (NativeTurnState::Completed, 0),
            (NativeTurnState::Cancelled, 130),
            (NativeTurnState::BudgetExhausted, 124),
            (NativeTurnState::IterationLimit, 75),
            (NativeTurnState::Failed, 1),
            (NativeTurnState::Declined, 2),
        ];
        for (state, code) in cases {
            assert_eq!(NativeTurnOutcome::new("id", state, None).exit_code(), code);
        }
    }

    #[test]
    fn tool_budget_reserves_before_effect_and_counts_skill_loading() {
        let mut budget = NativeBudget::new(
            NativeLimits {
                tool_calls: Some(2),
                ..NativeLimits::default()
            },
            ExecutionCounters::default(),
        );
        assert!(budget
            .admit_tool(&call("use_skill", json!({"name":"build"})))
            .is_ok());
        assert!(budget
            .admit_tool(&call("write_file", json!({"path":"out"})))
            .is_ok());
        assert!(budget
            .admit_tool(&call(
                "run_command",
                json!({"command":"touch should-not-exist"})
            ))
            .is_err());
        assert_eq!(budget.counters().tool_calls, 2);
        assert_eq!(budget.counters().network_calls, 0);
    }

    #[test]
    fn network_budget_covers_web_commands_and_opaque_mcp_without_consuming_refused_slots() {
        let mut budget = NativeBudget::new(
            NativeLimits {
                network_calls: Some(2),
                ..NativeLimits::default()
            },
            ExecutionCounters::default(),
        );
        assert!(budget
            .admit_tool(&call(
                "run_command",
                json!({"command":"curl https://example.com"})
            ))
            .is_ok());
        assert!(budget
            .admit_tool(&call("mcp__local__lookup", json!({})))
            .is_ok());
        assert!(budget
            .admit_tool(&call("fetch_url", json!({"url":"https://example.com"})))
            .is_err());
        assert_eq!(budget.counters().network_calls, 2);
        assert_eq!(budget.counters().tool_calls, 2);
        assert!(budget
            .admit_tool(&call("read_file", json!({"path":"local"})))
            .is_ok());
    }

    #[test]
    fn resume_keeps_spent_provider_tool_network_cost_and_time_budgets() {
        let limits = NativeLimits {
            provider_turns: Some(4),
            tool_calls: Some(8),
            network_calls: Some(3),
            ..NativeLimits::default()
        };
        let counters = ExecutionCounters {
            provider_turns: 4,
            tool_calls: 8,
            network_calls: 3,
            elapsed_ms: 2000,
            cost_usd: 0.4,
        };
        let mut budget = NativeBudget::new(limits, counters);
        assert!(budget.admit_provider().is_err());
        assert!(budget.admit_tool(&call("write_file", json!({}))).is_err());
        assert!(budget.counters().elapsed_ms >= 2000);
        assert_eq!(budget.counters().cost_usd, 0.4);
    }

    #[test]
    fn restored_limits_cannot_be_removed_or_relaxed_but_can_be_tightened() {
        let original = NativeLimits {
            tool_calls: Some(3),
            provider_turns: Some(4),
            network_calls: Some(5),
            elapsed: Some(Duration::from_secs(60)),
            cost_usd: Some(0.5),
        };
        let relaxed = NativeLimits {
            tool_calls: Some(9),
            provider_turns: None,
            network_calls: Some(10),
            elapsed: None,
            cost_usd: Some(1.0),
        };
        let merged = original.constrain(&relaxed);
        assert_eq!(merged.tool_calls, Some(3));
        assert_eq!(merged.provider_turns, Some(4));
        assert_eq!(merged.network_calls, Some(5));
        assert_eq!(merged.elapsed, Some(Duration::from_secs(60)));
        assert_eq!(merged.cost_usd, Some(0.5));
        let tightened = original.constrain(&NativeLimits {
            tool_calls: Some(1),
            ..NativeLimits::default()
        });
        assert_eq!(tightened.tool_calls, Some(1));
    }

    #[test]
    fn cost_and_wall_clock_caps_admit_no_further_effects_and_clip_command_deadline() {
        let mut budget = NativeBudget::new(
            NativeLimits {
                cost_usd: Some(0.1),
                ..NativeLimits::default()
            },
            ExecutionCounters::default(),
        );
        budget.record_cost(0.11);
        assert!(budget.admit_provider().is_err());
        assert!(budget.admit_tool(&call("write_file", json!({}))).is_err());
        let mut budget = NativeBudget::new(
            NativeLimits::default(),
            ExecutionCounters {
                elapsed_ms: 1000,
                ..ExecutionCounters::default()
            },
        );
        budget.elapsed_limit(Duration::from_secs(2));
        assert!(budget.command_timeout(Duration::from_secs(60)) <= Duration::from_secs(1));
        budget.elapsed_limit(Duration::from_millis(500));
        assert!(budget.admit_provider().is_err());
        assert_eq!(
            budget.command_timeout(Duration::from_secs(60)),
            Duration::ZERO
        );
    }
}
