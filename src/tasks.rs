//! Durable AI task records and lifecycle operations.
//!
//! Records contain only the selected provider/model, objective, redacted
//! canonical messages/tool state, opaque provider continuation identifiers,
//! encrypted reasoning state, and usage. Credentials and environment values are
//! never captured in plaintext. Every write is atomic and private.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::providers::{ErrorKind, Msg, ToolCall};
use crate::usage::Usage;

pub mod evidence;
pub mod timeline;
pub use evidence::{CheckEvidence, CheckSummary, ExecutionKind, ExecutionOutcome};
pub use timeline::{EventKind, EventOutcome, TaskTimelineEvent};

pub const TASK_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Interrupted,
    Completed,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingTool {
    pub call: ToolCall,
    pub may_have_started: bool,
    #[serde(default)]
    pub execution_evidence_recorded: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompletedTool {
    pub call_id: String,
    pub name: String,
    pub result: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub input: u64,
    pub output: u64,
    pub requests: u64,
}

impl From<Usage> for UsageSummary {
    fn from(value: Usage) -> Self {
        Self {
            input: value.input,
            output: value.output,
            requests: value.requests,
        }
    }
}

/// Cumulative native execution, carried across every checkpoint continuation.
/// Reservations are recorded before effects so interruption cannot replenish a
/// tool or network allowance by starting another process.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExecutionCounters {
    pub provider_turns: u32,
    pub tool_calls: u32,
    pub network_calls: u32,
    pub elapsed_ms: u64,
    pub cost_usd: f64,
    /// Provider reservations for which both usage and an exact model price were
    /// recorded. Missing on older checkpoints: their total is unverified.
    #[serde(default)]
    pub costed_provider_turns: Option<u32>,
}

impl Default for ExecutionCounters {
    fn default() -> Self {
        Self {
            provider_turns: 0,
            tool_calls: 0,
            network_calls: 0,
            elapsed_ms: 0,
            cost_usd: 0.0,
            costed_provider_turns: Some(0),
        }
    }
}

impl ExecutionCounters {
    pub fn cost_is_complete(&self) -> bool {
        self.costed_provider_turns == Some(self.provider_turns)
            && self.cost_usd.is_finite()
            && self.cost_usd >= 0.0
    }

    /// Cost is an estimate from recorded token usage, never a billing receipt.
    pub fn cost_label(&self) -> String {
        if self.cost_is_complete() {
            format!("${:.4} (recorded estimate)", self.cost_usd)
        } else if self.cost_usd.is_finite() && self.cost_usd > 0.0 {
            let coverage = self.costed_provider_turns.map_or_else(
                || "older usage unverified".into(),
                |turns| format!("{turns}/{} turns priced", self.provider_turns),
            );
            format!("partial ${:.4} · {coverage}", self.cost_usd)
        } else {
            "n/a · price or usage unavailable".into()
        }
    }
}

fn unverified_execution_counters() -> ExecutionCounters {
    ExecutionCounters {
        costed_provider_turns: None,
        ..ExecutionCounters::default()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub schema_version: u32,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub status: Status,
    pub mode: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub connection_id: String,
    /// Connection settings contain credential references, never key values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<crate::config::ConnectionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_scope: Option<crate::agent::ExecutionScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_policy: Option<crate::agent::NetworkPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
    pub cwd: PathBuf,
    pub objective: String,
    pub messages: Vec<Msg>,
    #[serde(default)]
    pub completed_tools: Vec<CompletedTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_tool: Option<PendingTool>,
    #[serde(default)]
    pub usage: UsageSummary,
    #[serde(default = "unverified_execution_counters")]
    pub execution: ExecutionCounters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_limits: Option<crate::agent::native::NativeLimits>,
    #[serde(default)]
    pub steering_revision: u32,
    #[serde(default)]
    pub followup_revision: u32,
    /// Command evidence is independent of model-written plans and conclusions.
    #[serde(default)]
    pub evidence: Vec<CheckEvidence>,
    #[serde(default)]
    pub workspace_revision: u64,
    #[serde(default)]
    pub evidence_dropped: usize,
    #[serde(default)]
    pub timeline: Vec<TaskTimelineEvent>,
    #[serde(default)]
    pub timeline_dropped: usize,
    #[serde(default)]
    pub timeline_sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_kind: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

impl Record {
    pub fn check_summary(&self) -> CheckSummary {
        evidence::summary(self)
    }

    /// Reconcile a stopped worker without inventing an observed command result.
    /// Presentation may use this on a checkpoint whose background worker died;
    /// resume persists it before admitting any continuation effects.
    pub fn reconcile_unfinished_evidence(&mut self, cancelled: bool) {
        for entry in &mut self.evidence {
            entry.interrupted(cancelled);
        }
    }
}

pub struct Active {
    record: Record,
    path: Option<PathBuf>,
    usage_base: UsageSummary,
    usage_meter_start: Usage,
}

impl Active {
    pub fn start(config: &Config, cwd: &Path, objective: &str) -> Self {
        let now = now_ms();
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = format!(
            "{:x}-{}-{}",
            now,
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        );
        let record = Record {
            schema_version: TASK_SCHEMA_VERSION,
            id,
            name: None,
            created_at_ms: now,
            updated_at_ms: now,
            status: Status::Active,
            mode: config.aishe.mode.clone(),
            provider: config.active_provider_name().into(),
            model: config.active_model().into(),
            connection_id: config.active_connection_id().into(),
            connection: snapshot_connection(config),
            execution_scope: None,
            network_policy: None,
            workspace_root: None,
            cwd: cwd.to_path_buf(),
            objective: crate::redact::redact(objective),
            messages: Vec::new(),
            completed_tools: Vec::new(),
            pending_tool: None,
            usage: UsageSummary::default(),
            execution: ExecutionCounters::default(),
            execution_limits: None,
            steering_revision: 0,
            followup_revision: 0,
            evidence: Vec::new(),
            workspace_revision: 0,
            evidence_dropped: 0,
            timeline: Vec::new(),
            timeline_dropped: 0,
            timeline_sequence: 0,
            native_state: None,
            last_error_kind: None,
            last_error: None,
        };
        let path = (persistence_enabled() || background_task_id().is_some())
            .then(|| task_path(&record.id))
            .flatten();
        let mut active = Self {
            record,
            path,
            usage_base: UsageSummary::default(),
            usage_meter_start: Usage::default(),
        };
        timeline::push(
            &mut active.record,
            EventKind::TaskStarted,
            "Task started",
            objective,
            None,
        );
        active.save();
        active
    }

    pub fn resume(record: Record) -> Self {
        let path = task_path(&record.id);
        let usage_base = record.usage.clone();
        let mut active = Self {
            record,
            path,
            usage_base,
            usage_meter_start: Usage::default(),
        };
        active.record.status = Status::Active;
        active.record.native_state = None;
        active.record.last_error = None;
        active.record.last_error_kind = None;
        active.record.reconcile_unfinished_evidence(false);
        active.record.updated_at_ms = now_ms();
        timeline::push(
            &mut active.record,
            EventKind::Resumed,
            "Checkpoint resumed",
            "The saved conversation, counters, and execution bounds were retained.",
            None,
        );
        active.save();
        active
    }

    pub fn id(&self) -> &str {
        &self.record.id
    }

    pub fn record(&self) -> &Record {
        &self.record
    }

    /// A handoff requires a checkpoint even when ordinary foreground session
    /// persistence was explicitly disabled.
    pub fn require_persistence(&mut self) -> Result<()> {
        if self.path.is_none() {
            anyhow::bail!("task handoff requires durable task persistence; enable session persistence before handing off");
        }
        self.ensure_persisted()
    }

    pub fn record_timeline_event(
        &mut self,
        kind: EventKind,
        subject: &str,
        detail: &str,
        outcome: Option<EventOutcome>,
    ) -> Result<()> {
        timeline::push(&mut self.record, kind, subject, detail, outcome);
        self.ensure_persisted()
    }

    pub fn background_cancelled(&self) -> bool {
        background_task_id().is_some_and(|id| crate::background::is_cancelled(&id).unwrap_or(false))
    }

    /// A detached worker must have both its task checkpoint and background
    /// linkage on disk before contacting a provider or starting a tool.
    pub fn ensure_persisted(&mut self) -> Result<()> {
        let background_id = background_task_id();
        let Some(path) = &self.path else {
            if background_id.is_some() {
                anyhow::bail!("background execution requires a durable task checkpoint");
            }
            return Ok(());
        };
        self.record.updated_at_ms = now_ms();
        save_record_to(path, &self.record)?;
        if let Some(id) = background_id.filter(|_| self.record.status == Status::Active) {
            crate::background::attach_native_task(&id, self.id())?;
        }
        Ok(())
    }

    pub fn checkpoint_execution(&mut self, counters: ExecutionCounters) {
        if counters.provider_turns > self.record.execution.provider_turns {
            // The latest bounded observations are enough even for an imported
            // checkpoint with an unexpectedly large reservation counter.
            let start = self.record.execution.provider_turns.saturating_add(1).max(
                counters
                    .provider_turns
                    .saturating_sub(timeline::MAX_EVENTS as u32),
            );
            for turn in start..=counters.provider_turns {
                timeline::push(&mut self.record, EventKind::ProviderTurn, &format!("Provider turn {turn}"), "Budget reserved before a provider request; this is not a claim that the request completed.", None);
            }
        }
        self.record.execution = counters;
        self.save();
    }

    pub fn checkpoint_limits(&mut self, limits: crate::agent::native::NativeLimits) {
        self.record.execution_limits = Some(limits);
        self.save();
    }

    pub fn checkpoint_admission(&mut self, executor: &crate::executor::Executor) {
        if let Some((scope, root, network)) = executor.lean_scope() {
            let changed = self.record.execution_scope != Some(*scope)
                || self.record.network_policy != Some(*network)
                || self.record.workspace_root.as_ref() != Some(root);
            self.record.execution_scope = Some(*scope);
            self.record.network_policy = Some(*network);
            self.record.workspace_root = Some(root.clone());
            if changed {
                timeline::push(
                    &mut self.record,
                    EventKind::Admission,
                    "Execution admitted",
                    &format!(
                        "Scope: {scope:?}\nNetwork: {network:?}\nRoot: {}",
                        root.display()
                    ),
                    None,
                );
            }
            self.save();
        }
    }

    pub fn checkpoint_cwd(&mut self, cwd: &Path) {
        self.record.cwd = cwd.to_path_buf();
        self.save();
    }

    /// Append and acknowledge a follow-up in the same durable checkpoint.
    /// The caller may mark its mailbox item received only after this succeeds.
    pub fn checkpoint_followup_revision(
        &mut self,
        revision: u32,
        messages: &[Msg],
        usage: Usage,
    ) -> Result<()> {
        if revision > self.record.followup_revision {
            timeline::push(
                &mut self.record,
                EventKind::FollowupReceived,
                &format!("Follow-up #{revision}"),
                "Added to the durable conversation at an execution boundary.",
                None,
            );
        }
        self.record.followup_revision = self.record.followup_revision.max(revision);
        self.record.messages = sanitize_messages(messages);
        self.record.usage = self.cumulative_usage(usage);
        self.ensure_persisted()
    }

    /// Invalidate prior checks before an effect that may change the workspace.
    /// Opaque commands and MCP calls are conservative even when they only read.
    pub fn note_workspace_change(&mut self) -> Result<()> {
        self.record.workspace_revision = self.record.workspace_revision.saturating_add(1);
        self.ensure_persisted()
    }

    /// Record a started command durably before passing it to the executor.
    /// An uncertain command is never made safe to replay by this journal.
    pub fn begin_execution_evidence(
        &mut self,
        call_id: &str,
        command: &str,
        cwd: &Path,
        kind: ExecutionKind,
    ) -> Result<()> {
        if self
            .record
            .evidence
            .iter()
            .any(|entry| entry.call_id == call_id && entry.outcome == ExecutionOutcome::Running)
        {
            anyhow::bail!("execution evidence for tool call {call_id} is already recorded");
        }
        // Both tool names accept opaque shell commands. A check can modify
        // sources too, so it cannot exempt earlier results from staleness.
        self.record.workspace_revision = self.record.workspace_revision.saturating_add(1);
        self.retain_bounded_evidence();
        self.record.evidence.push(CheckEvidence::started(
            call_id,
            command,
            cwd,
            kind,
            self.record.workspace_revision,
        ));
        if let Some(pending) = self
            .record
            .pending_tool
            .as_mut()
            .filter(|pending| pending.call.id == call_id)
        {
            pending.execution_evidence_recorded = true;
        }
        self.ensure_persisted()
    }

    pub fn finish_execution_evidence(
        &mut self,
        call_id: &str,
        exit_code: i32,
        output: &str,
        duration_ms: u64,
        cancelled: bool,
    ) -> Result<()> {
        let entry = self
            .record
            .evidence
            .iter_mut()
            .rev()
            .find(|entry| entry.call_id == call_id && entry.outcome == ExecutionOutcome::Running)
            .with_context(|| format!("no started execution evidence for tool call {call_id}"))?;
        entry.finish(exit_code, output, duration_ms, cancelled);
        let kind = entry.kind;
        let command = entry.command.clone();
        if kind == ExecutionKind::Check {
            timeline::push(
                &mut self.record,
                EventKind::CheckResult,
                &command,
                &format!("Exit code: {exit_code}\nDuration: {duration_ms} ms\n{output}"),
                Some(if cancelled {
                    EventOutcome::Cancelled
                } else if exit_code == 0 {
                    EventOutcome::Completed
                } else {
                    EventOutcome::Failed
                }),
            );
        }
        self.ensure_persisted()
    }

    /// Declined or refused checks remain visible without inventing an exit code.
    pub fn record_not_run_evidence(
        &mut self,
        call_id: &str,
        command: &str,
        cwd: &Path,
        kind: ExecutionKind,
        reason: &str,
    ) -> Result<()> {
        if self
            .record
            .evidence
            .iter()
            .any(|entry| entry.call_id == call_id && entry.outcome == ExecutionOutcome::Running)
        {
            anyhow::bail!("execution evidence for tool call {call_id} is already recorded");
        }
        self.retain_bounded_evidence();
        let mut entry =
            CheckEvidence::started(call_id, command, cwd, kind, self.record.workspace_revision);
        entry.not_run(reason);
        self.record.evidence.push(entry);
        if kind == ExecutionKind::Check {
            timeline::push(
                &mut self.record,
                EventKind::CheckResult,
                command,
                reason,
                Some(EventOutcome::NotExecuted),
            );
        }
        self.ensure_persisted()
    }

    fn retain_bounded_evidence(&mut self) {
        if self.record.evidence.len() >= evidence::MAX_EVIDENCE {
            let discarded = self.record.evidence.len() + 1 - evidence::MAX_EVIDENCE;
            self.record.evidence.drain(..discarded);
            self.record.evidence_dropped = self.record.evidence_dropped.saturating_add(discarded);
        }
    }

    pub fn set_usage_baseline(&mut self, usage: Usage) {
        self.usage_meter_start = usage;
    }

    fn cumulative_usage(&self, usage: Usage) -> UsageSummary {
        UsageSummary {
            input: self
                .usage_base
                .input
                .saturating_add(usage.input.saturating_sub(self.usage_meter_start.input)),
            output: self
                .usage_base
                .output
                .saturating_add(usage.output.saturating_sub(self.usage_meter_start.output)),
            requests: self.usage_base.requests.saturating_add(
                usage
                    .requests
                    .saturating_sub(self.usage_meter_start.requests),
            ),
        }
    }

    pub fn finish_native(
        &mut self,
        outcome: &crate::agent::native::NativeTurnOutcome,
        messages: &[Msg],
        usage: Usage,
    ) {
        use crate::agent::native::NativeTurnState;
        let (status, reason) = match outcome.state {
            NativeTurnState::Completed => (Status::Completed, "completed"),
            NativeTurnState::Cancelled => (Status::Interrupted, "cancelled"),
            NativeTurnState::BudgetExhausted => (Status::Interrupted, "budget_exhausted"),
            NativeTurnState::IterationLimit => (Status::Interrupted, "iteration_limit"),
            NativeTurnState::Failed => (Status::Failed, "failed"),
            NativeTurnState::Declined => (Status::Interrupted, "declined"),
            NativeTurnState::Waiting => (Status::Interrupted, "waiting"),
            NativeTurnState::HandedOff => (Status::Interrupted, "handed_off"),
        };
        self.record.status = status;
        self.record.native_state = Some(reason.into());
        self.record.last_error = outcome.detail.as_deref().map(crate::redact::redact);
        if status == Status::Completed {
            self.record.pending_tool = None;
            self.record.last_error_kind = None;
        }
        if !matches!(
            outcome.state,
            NativeTurnState::Waiting | NativeTurnState::HandedOff
        ) {
            self.record
                .reconcile_unfinished_evidence(outcome.state == NativeTurnState::Cancelled);
        }
        let event_outcome = match outcome.state {
            NativeTurnState::Completed => EventOutcome::Completed,
            NativeTurnState::Cancelled => EventOutcome::Cancelled,
            NativeTurnState::Failed => EventOutcome::Failed,
            NativeTurnState::Declined => EventOutcome::Declined,
            NativeTurnState::Waiting => EventOutcome::Waiting,
            NativeTurnState::HandedOff => EventOutcome::HandedOff,
            NativeTurnState::BudgetExhausted | NativeTurnState::IterationLimit => {
                EventOutcome::NotExecuted
            }
        };
        timeline::push(
            &mut self.record,
            EventKind::Finished,
            reason,
            outcome.detail.as_deref().unwrap_or(""),
            Some(event_outcome),
        );
        self.checkpoint_messages(messages, usage);
    }

    pub fn checkpoint_messages(&mut self, messages: &[Msg], usage: Usage) {
        self.record.messages = sanitize_messages(messages);
        self.record.usage = self.cumulative_usage(usage);
        self.record.updated_at_ms = now_ms();
        self.save();
    }

    pub fn pending(&mut self, call: &ToolCall, messages: &[Msg], usage: Usage) {
        self.record.messages = sanitize_messages(messages);
        self.record.pending_tool = Some(PendingTool {
            call: sanitize_tool_call(call),
            may_have_started: false,
            execution_evidence_recorded: false,
        });
        self.record.usage = self.cumulative_usage(usage);
        self.record.updated_at_ms = now_ms();
        timeline::push(
            &mut self.record,
            EventKind::ToolPlanned,
            &call.name,
            &timeline::tool_arguments(&call.arguments),
            None,
        );
        self.save();
    }

    pub fn mark_pending_started(&mut self) {
        if let Some(pending) = self.record.pending_tool.as_mut() {
            pending.may_have_started = true;
            let name = pending.call.name.clone();
            timeline::push(&mut self.record, EventKind::ToolStarted, &name, "Execution boundary entered; an interruption after this point may leave effects uncertain.", None);
        }
        self.record.updated_at_ms = now_ms();
        self.save();
    }

    pub fn tool_completed(
        &mut self,
        call: &ToolCall,
        result: &str,
        messages: &[Msg],
        usage: Usage,
    ) {
        let observed = self
            .record
            .pending_tool
            .as_ref()
            .filter(|pending| pending.call.id == call.id)
            .map(|pending| pending.may_have_started);
        self.record_unexecuted_check(call, result);
        if !self
            .record
            .completed_tools
            .iter()
            .any(|completed| completed.call_id == call.id)
        {
            self.record.completed_tools.push(CompletedTool {
                call_id: call.id.clone(),
                name: call.name.clone(),
                result: crate::redact::redact(result),
            });
        }
        timeline::push(
            &mut self.record,
            EventKind::ToolResult,
            &call.name,
            result,
            if observed == Some(false) {
                Some(EventOutcome::NotExecuted)
            } else {
                None
            },
        );
        self.record.pending_tool = None;
        self.checkpoint_messages(messages, usage);
    }

    pub fn clear_pending_with_result(&mut self, result: &str) -> Option<Msg> {
        self.clear_pending_result(result, true)
    }

    /// A parked approval has not attempted an execution. Approval itself is a
    /// mailbox result; only an explicit denial belongs in the check journal.
    pub fn clear_pending_interaction_result(&mut self, result: &str, denied: bool) -> Option<Msg> {
        self.clear_pending_result(result, denied)
    }

    fn clear_pending_result(&mut self, result: &str, record_not_run: bool) -> Option<Msg> {
        let pending = self.record.pending_tool.as_ref()?.clone();
        if record_not_run {
            self.record_unexecuted_check(&pending.call, result);
        }
        timeline::push(
            &mut self.record,
            EventKind::ToolResult,
            &pending.call.name,
            result,
            if pending.may_have_started {
                Some(EventOutcome::Uncertain)
            } else if record_not_run {
                Some(EventOutcome::NotExecuted)
            } else {
                None
            },
        );
        self.record.pending_tool = None;
        let message = Msg::ToolResult {
            call_id: pending.call.id.clone(),
            content: crate::redact::redact(result),
        };
        self.record.messages.push(message.clone());
        self.record.updated_at_ms = now_ms();
        self.save();
        Some(message)
    }

    fn record_unexecuted_check(&mut self, call: &ToolCall, result: &str) {
        let already_recorded = self
            .record
            .pending_tool
            .as_ref()
            .filter(|pending| pending.call.id == call.id)
            .map(|pending| pending.execution_evidence_recorded)
            .unwrap_or_else(|| {
                self.record
                    .evidence
                    .iter()
                    .any(|entry| entry.call_id == call.id)
            });
        if call.name != "run_check" || already_recorded {
            return;
        }
        let command = call
            .arguments
            .get("command")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        self.retain_bounded_evidence();
        let mut entry = CheckEvidence::started(
            &call.id,
            command,
            &self.record.cwd,
            ExecutionKind::Check,
            self.record.workspace_revision,
        );
        entry.not_run(result);
        self.record.evidence.push(entry);
        timeline::push(
            &mut self.record,
            EventKind::CheckResult,
            command,
            result,
            Some(EventOutcome::NotExecuted),
        );
    }

    pub fn interrupted(&mut self, messages: &[Msg], usage: Usage) {
        self.record.status = Status::Interrupted;
        self.record.reconcile_unfinished_evidence(false);
        timeline::push(
            &mut self.record,
            EventKind::Finished,
            "Interrupted",
            "No successful completion was observed.",
            Some(EventOutcome::Uncertain),
        );
        self.checkpoint_messages(messages, usage);
    }

    pub fn failed(&mut self, messages: &[Msg], usage: Usage, kind: ErrorKind, error: &str) {
        self.record.status = Status::Failed;
        self.record.last_error_kind = Some(kind);
        self.record.last_error = Some(crate::redact::redact(error));
        self.record.reconcile_unfinished_evidence(false);
        timeline::push(
            &mut self.record,
            EventKind::Finished,
            "Failed",
            error,
            Some(EventOutcome::Failed),
        );
        self.checkpoint_messages(messages, usage);
    }

    pub fn completed(&mut self, messages: &[Msg], usage: Usage) {
        self.record.status = Status::Completed;
        self.record.pending_tool = None;
        self.record.reconcile_unfinished_evidence(false);
        timeline::push(
            &mut self.record,
            EventKind::Finished,
            "Completed",
            "The task returned a final response; inspect recorded checks separately.",
            Some(EventOutcome::Completed),
        );
        self.checkpoint_messages(messages, usage);
    }

    fn save(&mut self) {
        let Some(path) = &self.path else { return };
        self.record.updated_at_ms = now_ms();
        let _ = save_record_to(path, &self.record);
    }
}

fn sanitize_messages(messages: &[Msg]) -> Vec<Msg> {
    messages.iter().map(sanitize_message).collect()
}

fn sanitize_message(message: &Msg) -> Msg {
    match message {
        Msg::User(text) => {
            // Only the initial generated context block has this prefix. A
            // follow-up or answer can quote "User request:" as ordinary text;
            // stripping that text would lose its durable delivery identity.
            let objective = if text.starts_with("OS: ") {
                text.split_once("\nUser request:")
                    .map(|(_, request)| request.trim())
                    .unwrap_or(text)
            } else {
                text
            };
            Msg::User(crate::redact::redact(objective))
        }
        Msg::Assistant(assistant) => Msg::Assistant(crate::providers::AssistantMsg {
            text: assistant.text.as_deref().map(crate::redact::redact),
            tool_calls: assistant
                .tool_calls
                .iter()
                .map(sanitize_tool_call)
                .collect(),
        }),
        Msg::ToolResult { call_id, content } => Msg::ToolResult {
            call_id: call_id.clone(),
            content: crate::redact::redact(content),
        },
        Msg::ProviderItems { items, assistant } => Msg::ProviderItems {
            items: items.iter().map(sanitize_provider_item).collect(),
            assistant: crate::providers::AssistantMsg {
                text: assistant.text.as_deref().map(crate::redact::redact),
                tool_calls: assistant
                    .tool_calls
                    .iter()
                    .map(sanitize_tool_call)
                    .collect(),
            },
        },
    }
}

/// Sanitize an item returned by a provider without corrupting the opaque state
/// required to continue a stateless Responses tool loop. Provider-generated
/// item IDs and call IDs are routing metadata, not user content. OpenAI
/// reasoning `encrypted_content` is an opaque client-side continuation token
/// when `store: false`; changing even one byte makes durable resume impossible.
///
/// All model-visible text, summaries, and tool arguments still pass through the
/// normal recursive redactor. This function is deliberately used only for
/// provider output items, never for user-controlled tool arguments.
fn sanitize_provider_item(value: &serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(values) = value else {
        return sanitize_json(value);
    };
    let item_type = values.get("type").and_then(serde_json::Value::as_str);
    serde_json::Value::Object(
        values
            .iter()
            .map(|(key, value)| {
                let preserve = matches!(key.as_str(), "id" | "call_id")
                    || (item_type == Some("reasoning") && key == "encrypted_content");
                (
                    key.clone(),
                    if preserve {
                        value.clone()
                    } else {
                        sanitize_json(value)
                    },
                )
            })
            .collect(),
    )
}

fn sanitize_json(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => serde_json::Value::String(crate::redact::redact(value)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(sanitize_json).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), sanitize_json(value)))
                .collect(),
        ),
        value => value.clone(),
    }
}

fn sanitize_tool_call(call: &ToolCall) -> ToolCall {
    ToolCall {
        id: call.id.clone(),
        name: call.name.clone(),
        arguments: sanitize_json(&call.arguments),
    }
}

pub fn root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("AISHE_TASKS_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(path));
    }
    crate::config::data_root().map(|root| root.join("aishe").join("tasks"))
}

fn persistence_enabled() -> bool {
    if matches!(
        std::env::var("AISHE_DISABLE_TASKS").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) {
        return false;
    }
    #[cfg(test)]
    {
        std::env::var_os("AISHE_TASKS_DIR").is_some()
    }
    #[cfg(not(test))]
    {
        // Rust integration tests run a harness from target/*/deps. They exercise
        // task checkpoint logic but must never write into the developer's real
        // data directory. End-to-end CLI tests execute the actual `aishe`
        // binary and isolate AISHE_DATA_DIR, so persistence remains covered.
        let under_test_harness = std::env::current_exe()
            .ok()
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .and_then(|path| path.file_name().map(|name| name.to_owned()))
            .is_some_and(|name| name == "deps");
        !under_test_harness
    }
}

fn background_task_id() -> Option<String> {
    std::env::var("AISHE_BACKGROUND_TASK_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(crate::agent::native::handoff::linked_task_id)
}

fn task_path(id: &str) -> Option<PathBuf> {
    valid_id(id)
        .then(|| root().map(|root| root.join(format!("{id}.json"))))
        .flatten()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn save_record_to(path: &Path, record: &Record) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_private(parent, 0o700);
    }
    crate::config::write_atomic(path, &serde_json::to_vec_pretty(record)?)?;
    set_private(path, 0o600);
    Ok(())
}

pub fn list() -> Vec<Record> {
    let Some(root) = root() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut records: Vec<Record> = entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
        .filter_map(|entry| load_path(&entry.path()).ok())
        .collect();
    records.sort_by_key(|record| record.updated_at_ms);
    records
}

pub fn load(id: &str) -> Result<Record> {
    let path = task_path(id).context("invalid task ID")?;
    load_path(&path)
}

/// Restore a task's provider identity and execution bounds without restoring a
/// grant. Callers must apply current organization policy and obtain fresh
/// admission before continuing the checkpoint.
pub fn restore_config(record: &Record, current: &Config) -> Result<Config> {
    let mut config = current.clone();
    let id = if record.connection_id.is_empty() {
        record.provider.as_str()
    } else {
        record.connection_id.as_str()
    };
    restore_connection(
        &mut config,
        id,
        &record.provider,
        &record.model,
        record.connection.as_ref(),
    )
    .with_context(|| format!("cannot restore task {} connection '{id}'", record.id))?;
    config.aishe.mode.clone_from(&record.mode);
    config.backend.default_scope = match record.execution_scope {
        Some(crate::agent::ExecutionScope::Host) => "host",
        Some(crate::agent::ExecutionScope::Workspace) | None => "workspace",
    }
    .into();
    config.backend.workspace_network = match record.network_policy {
        Some(crate::agent::NetworkPolicy::Allow) => "allow",
        Some(crate::agent::NetworkPolicy::Deny) | None => "deny",
    }
    .into();
    Ok(config)
}

pub(crate) fn restore_connection(
    config: &mut Config,
    id: &str,
    provider: &str,
    model: &str,
    snapshot: Option<&crate::config::ConnectionConfig>,
) -> Result<()> {
    if let Some(connection) = snapshot {
        if connection.provider != provider {
            anyhow::bail!("task connection does not match its saved provider");
        }
        if connection.settings.base_url.contains("<redacted>") {
            anyhow::bail!("saved task endpoint contains redacted private data; use credential references in the connection before resuming");
        }
        config.connections.insert(id.into(), connection.clone());
        // Canonical Auto still consults the compatibility provider block.
        if matches!(connection.auth, crate::config::ConnectionAuth::Auto)
            && id == connection.provider
        {
            if connection.provider == "anthropic" {
                config.providers.anthropic = connection.settings.clone();
            } else {
                config.providers.openai = connection.settings.clone();
            }
        }
    }
    config.select_connection(id)?;
    if config.active_provider_name() != provider {
        anyhow::bail!("saved task provider no longer matches connection '{id}'");
    }
    config.set_active_model(model.into());
    Ok(())
}

pub(crate) fn snapshot_connection(config: &Config) -> Option<crate::config::ConnectionConfig> {
    config.active_connection().cloned().map(|mut connection| {
        connection.settings = config.active_provider_config().clone();
        connection.label = crate::redact::redact(&connection.label);
        connection.settings.base_url = crate::redact::redact(&connection.settings.base_url);
        connection.settings.model = crate::redact::redact(&connection.settings.model);
        connection
    })
}

fn load_path(path: &Path) -> Result<Record> {
    if !path.exists() {
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown");
        return Err(crate::user_error::UserFacing::cli(
            "unknown_task",
            format!("No task or session '{id}'."),
            "Run `aishe sessions` to list what can be resumed.",
        ));
    }
    let mut record: Record = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("reading task {}", path.display()))?,
    )?;
    if record.schema_version != TASK_SCHEMA_VERSION {
        anyhow::bail!(
            "task {} uses unsupported schema {}",
            record.id,
            record.schema_version
        );
    }
    if record.status != Status::Active {
        record.reconcile_unfinished_evidence(record.native_state.as_deref() == Some("cancelled"));
    }
    Ok(record)
}

pub fn most_recent_resumable() -> Option<Record> {
    list().into_iter().rev().find(|record| {
        matches!(
            record.status,
            Status::Interrupted | Status::Failed | Status::Active
        )
    })
}

pub fn rename(id: &str, name: &str) -> Result<()> {
    let mut record = load(id)?;
    record.name = if name.trim().is_empty() {
        None
    } else {
        Some(crate::redact::redact(name.trim()))
    };
    record.updated_at_ms = now_ms();
    let path = task_path(id).context("invalid task ID")?;
    save_record_to(&path, &record)
}

pub fn delete(id: &str) -> Result<()> {
    let path = task_path(id).context("invalid task ID")?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("no task '{id}'")
        }
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn mark_background_cancelled(id: &str) -> Result<()> {
    let mut record = load(id)?;
    reconcile_background_cancellation(&mut record);
    save_record_to(&task_path(id).context("invalid task ID")?, &record)
}

pub(crate) fn reconcile_background_cancellation(record: &mut Record) {
    record.status = Status::Interrupted;
    record.native_state = Some("cancelled".into());
    record.last_error_kind = None;
    record.last_error = Some("Cancelled by user.".into());
    record.updated_at_ms = now_ms();
    record.reconcile_unfinished_evidence(true);
}

#[cfg(unix)]
fn set_private(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_private(_path: &Path, _mode: u32) {}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_partial_costs_never_appear_as_a_verified_zero() {
        let legacy: ExecutionCounters = serde_json::from_value(serde_json::json!({
            "provider_turns": 2, "tool_calls": 0, "network_calls": 0,
            "elapsed_ms": 1, "cost_usd": 0.0
        }))
        .unwrap();
        assert!(!legacy.cost_is_complete());
        assert!(legacy.cost_label().starts_with("n/a"));
        let partial = ExecutionCounters {
            provider_turns: 2,
            cost_usd: 0.2,
            costed_provider_turns: Some(1),
            ..ExecutionCounters::default()
        };
        assert!(!partial.cost_is_complete());
        assert!(partial.cost_label().contains("partial $0.2000"));
        assert!(partial.cost_label().contains("1/2 turns priced"));
        let saved: ExecutionCounters =
            serde_json::from_slice(&serde_json::to_vec(&partial).unwrap()).unwrap();
        assert_eq!(saved, partial);
        let free = ExecutionCounters {
            costed_provider_turns: Some(2),
            cost_usd: 0.0,
            ..partial
        };
        assert!(free.cost_is_complete());
        assert!(free.cost_label().contains("$0.0000 (recorded estimate)"));
        let task = Active::start(&Config::default(), Path::new("/tmp"), "old task");
        let mut old = serde_json::to_value(task.record()).unwrap();
        old.as_object_mut().unwrap().remove("execution");
        let old: Record = serde_json::from_value(old).unwrap();
        assert!(!old.execution.cost_is_complete());
        assert!(old.execution.cost_label().starts_with("n/a"));
    }

    #[test]
    fn execution_start_is_durable_and_completion_uses_the_actual_result() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        let dir = std::env::temp_dir().join(format!("aishe-evidence-{}", task.id()));
        task.path = Some(dir.join("task.json"));
        task.begin_execution_evidence(
            "check-1",
            "cargo test",
            Path::new("/tmp"),
            ExecutionKind::Check,
        )
        .unwrap();
        let pending = load_path(task.path.as_ref().unwrap()).unwrap();
        assert_eq!(pending.evidence[0].outcome, ExecutionOutcome::Running);
        assert_eq!(pending.evidence[0].exit_code, None);
        task.finish_execution_evidence("check-1", 1, "test failed", 123, false)
            .unwrap();
        let finished = load_path(task.path.as_ref().unwrap()).unwrap();
        assert_eq!(finished.evidence[0].outcome, ExecutionOutcome::Failed);
        assert_eq!(finished.evidence[0].exit_code, Some(1));
        assert_eq!(finished.evidence[0].duration_ms, Some(123));
        assert_eq!(finished.evidence[0].output, "test failed");
        assert_eq!(finished.check_summary().passed, 0);
        assert_eq!(finished.check_summary().failed, 1);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn checks_are_explicit_and_known_effects_make_earlier_results_stale() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        task.begin_execution_evidence("check-1", "test", Path::new("/tmp"), ExecutionKind::Check)
            .unwrap();
        task.finish_execution_evidence("check-1", 0, "passed", 1, false)
            .unwrap();
        assert_eq!(task.record.check_summary().passed, 1);
        task.begin_execution_evidence(
            "command-1",
            "printf 'all tests passed'",
            Path::new("/tmp"),
            ExecutionKind::Activity,
        )
        .unwrap();
        task.finish_execution_evidence("command-1", 0, "all tests passed", 1, false)
            .unwrap();
        let summary = task.record.check_summary();
        assert_eq!(summary.total, 1);
        assert_eq!(summary.passed, 0);
        assert_eq!(summary.stale, 1);
        task.begin_execution_evidence("check-2", "test", Path::new("/tmp"), ExecutionKind::Check)
            .unwrap();
        task.finish_execution_evidence("check-2", 0, "passed", 1, false)
            .unwrap();
        assert_eq!(task.record.check_summary().passed, 1);
        task.note_workspace_change().unwrap();
        assert_eq!(task.record.check_summary().passed, 0);
        assert_eq!(task.record.check_summary().stale, 2);
    }

    #[test]
    fn a_refused_check_has_no_exit_and_does_not_invalidate_completed_checks() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        task.begin_execution_evidence("check-1", "test", Path::new("/tmp"), ExecutionKind::Check)
            .unwrap();
        task.finish_execution_evidence("check-1", 0, "passed", 1, false)
            .unwrap();
        task.record_not_run_evidence(
            "check-2",
            "blocked check",
            Path::new("/tmp"),
            ExecutionKind::Check,
            "User declined.",
        )
        .unwrap();
        let summary = task.record.check_summary();
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.not_run, 1);
        assert_eq!(summary.stale, 0);
        assert_eq!(task.record.evidence[1].exit_code, None);
        assert!(!summary.unresolved.is_empty());
    }

    #[test]
    fn undispatched_checks_are_recorded_even_when_the_policy_path_returns_a_tool_error() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        let call = ToolCall {
            id: "refused-check".into(),
            name: "run_check".into(),
            arguments: serde_json::json!({"command":"blocked command", "reason":"check"}),
        };
        task.tool_completed(
            &call,
            "Not executed: budget exhausted.",
            &[],
            Usage::default(),
        );
        assert_eq!(task.record.check_summary().not_run, 1);
        assert_eq!(task.record.evidence[0].exit_code, None);
        assert_eq!(task.record.evidence[0].command, "blocked command");
        task.tool_completed(
            &call,
            "Not executed: budget exhausted.",
            &[],
            Usage::default(),
        );
        assert_eq!(task.record.evidence.len(), 1);
    }

    #[test]
    fn approval_responses_do_not_manufacture_a_not_run_check() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        let call = ToolCall {
            id: "parked-check".into(),
            name: "run_check".into(),
            arguments: serde_json::json!({"command":"test", "reason":"check"}),
        };
        task.pending(&call, &[], Usage::default());
        assert!(task
            .clear_pending_interaction_result("User approved the action.", false)
            .is_some());
        assert!(task.record.evidence.is_empty());
        task.pending(&call, &[], Usage::default());
        task.clear_pending_interaction_result("User declined the action.", true);
        assert_eq!(task.record.check_summary().not_run, 1);
    }

    #[test]
    fn stopped_and_resumed_checkpoints_preserve_uncertainty_without_replay() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        task.begin_execution_evidence("check-1", "test", Path::new("/tmp"), ExecutionKind::Check)
            .unwrap();
        task.record.reconcile_unfinished_evidence(false);
        assert_eq!(task.record.check_summary().uncertain, 1);
        assert_eq!(task.record.evidence[0].exit_code, None);
        assert!(task
            .finish_execution_evidence("check-1", 0, "invented", 1, false)
            .is_err());
    }

    #[test]
    fn reused_provider_call_ids_record_distinct_completed_turns_and_refusals() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        let call = ToolCall {
            id: "call".into(),
            name: "run_check".into(),
            arguments: serde_json::json!({"command":"test", "reason":"check"}),
        };
        for code in [0, 1, 0] {
            task.pending(&call, &[], Usage::default());
            task.begin_execution_evidence("call", "test", Path::new("/tmp"), ExecutionKind::Check)
                .unwrap();
            assert!(task
                .begin_execution_evidence("call", "test", Path::new("/tmp"), ExecutionKind::Check)
                .is_err());
            task.finish_execution_evidence("call", code, "observed", 1, false)
                .unwrap();
            task.tool_completed(&call, "observed", &[], Usage::default());
        }
        assert_eq!(task.record.evidence.len(), 3);
        assert_eq!(
            task.record
                .evidence
                .iter()
                .map(|e| e.exit_code)
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1), Some(0)]
        );
        task.pending(&call, &[], Usage::default());
        task.tool_completed(&call, "User declined this command.", &[], Usage::default());
        assert_eq!(task.record.evidence.len(), 4);
        assert_eq!(task.record.evidence[3].outcome, ExecutionOutcome::NotRun);
        assert_eq!(task.record.evidence[3].exit_code, None);
    }

    #[test]
    fn started_pending_evidence_is_not_replaced_by_not_run_when_skipped_on_resume() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        let call = ToolCall {
            id: "call".into(),
            name: "run_check".into(),
            arguments: serde_json::json!({"command":"test", "reason":"check"}),
        };
        task.pending(&call, &[], Usage::default());
        task.begin_execution_evidence("call", "test", Path::new("/tmp"), ExecutionKind::Check)
            .unwrap();
        task.mark_pending_started();
        task.record.reconcile_unfinished_evidence(false);
        task.clear_pending_with_result("Skipped on resume; effects may have occurred.");
        assert_eq!(task.record.evidence.len(), 1);
        assert_eq!(task.record.evidence[0].outcome, ExecutionOutcome::Uncertain);
    }

    #[test]
    fn journal_is_bounded_and_reports_omitted_history() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "check changes");
        task.path = None;
        for index in 0..=evidence::MAX_EVIDENCE {
            let id = format!("check-{index}");
            task.begin_execution_evidence(&id, "test", Path::new("/tmp"), ExecutionKind::Check)
                .unwrap();
            task.finish_execution_evidence(&id, 0, "passed", 1, false)
                .unwrap();
        }
        assert_eq!(task.record.evidence.len(), evidence::MAX_EVIDENCE);
        assert_eq!(task.record.evidence_dropped, 1);
        let summary = task.record.check_summary();
        assert_eq!(summary.passed, 1);
        assert_eq!(summary.stale, evidence::MAX_EVIDENCE - 1);
        assert_eq!(summary.omitted, 1);
        assert!(!summary.unresolved.is_empty());
    }

    #[test]
    fn sanitization_strips_context_and_secrets() {
        let messages = vec![Msg::User(
            "OS: Linux\nInstalled tools: x\nUser request: use sk-proj-abcdefghijklmnopqrstuvwxyz1234567890"
                .into(),
        )];
        let sanitized = sanitize_messages(&messages);
        let text = serde_json::to_string(&sanitized).unwrap();
        assert!(!text.contains("Installed tools"));
        assert!(!text.contains("abcdefghijklmnopqrstuvwxyz"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn sanitization_preserves_followup_identity_and_quoted_request_text() {
        let text = "Follow-up #7:\nUse the literal label User request: before the result";
        let sanitized = sanitize_message(&Msg::User(text.into()));
        assert!(matches!(sanitized, Msg::User(value) if value == text));
        let context =
            "OS: Linux\nShell backend: zsh\nUser request: Preserve User request: in my title";
        let sanitized = sanitize_message(&Msg::User(context.into()));
        assert!(
            matches!(sanitized, Msg::User(value) if value == "Preserve User request: in my title")
        );
    }

    #[test]
    fn ids_reject_path_traversal() {
        assert!(valid_id("123-abcd"));
        assert!(!valid_id("../task"));
        assert!(!valid_id("task/name"));
    }

    #[test]
    fn resume_preserves_usage_and_counts_only_the_new_attempt() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "continue");
        task.path = None;
        task.record.usage = UsageSummary {
            input: 100,
            output: 20,
            requests: 2,
        };
        let mut resumed = Active {
            record: task.record.clone(),
            path: None,
            usage_base: task.record.usage.clone(),
            usage_meter_start: Usage::default(),
        };
        resumed.set_usage_baseline(Usage::reported(50, 10, 1));
        resumed.checkpoint_messages(&[], Usage::reported(80, 17, 2));
        assert_eq!(resumed.record.usage.input, 130);
        assert_eq!(resumed.record.usage.output, 27);
        assert_eq!(resumed.record.usage.requests, 3);
        // Repeated checkpoints must not add the attempt twice.
        resumed.checkpoint_messages(&[], Usage::reported(90, 18, 3));
        assert_eq!(resumed.record.usage.input, 140);
        assert_eq!(resumed.record.usage.requests, 4);
    }

    #[test]
    fn native_terminal_reasons_are_durable_without_false_completion() {
        use crate::agent::{NativeTurnOutcome, NativeTurnState};
        for (state, reason, status) in [
            (NativeTurnState::Cancelled, "cancelled", Status::Interrupted),
            (
                NativeTurnState::BudgetExhausted,
                "budget_exhausted",
                Status::Interrupted,
            ),
            (
                NativeTurnState::IterationLimit,
                "iteration_limit",
                Status::Interrupted,
            ),
            (NativeTurnState::Failed, "failed", Status::Failed),
            (NativeTurnState::Declined, "declined", Status::Interrupted),
        ] {
            let mut task = Active::start(&Config::default(), Path::new("/tmp"), "test");
            task.path = None;
            let outcome = NativeTurnOutcome::new(task.id(), state, Some("bounded stop".into()));
            task.finish_native(&outcome, &[], Usage::default());
            assert_eq!(task.record.status, status);
            assert_eq!(task.record.native_state.as_deref(), Some(reason));
            assert_eq!(task.record.last_error.as_deref(), Some("bounded stop"));
        }
    }

    #[test]
    fn checkpoint_restores_original_endpoint_model_and_bounds() {
        let mut original = Config::default();
        original.select_connection("openai").unwrap();
        original.providers.openai.base_url = "http://127.0.0.1:8123/v1".into();
        original.set_active_model("original-model".into());
        let mut task = Active::start(&original, Path::new("/tmp"), "test");
        task.record.execution_scope = Some(crate::agent::ExecutionScope::Workspace);
        task.record.network_policy = Some(crate::agent::NetworkPolicy::Deny);
        let mut changed = original.clone();
        changed.providers.openai.base_url = "http://127.0.0.1:9999/v1".into();
        changed.set_active_model("different-model".into());
        changed.backend.default_scope = "host".into();
        changed.backend.workspace_network = "allow".into();
        let restored = restore_config(&task.record, &changed).unwrap();
        assert_eq!(
            restored.active_provider_config().base_url,
            "http://127.0.0.1:8123/v1"
        );
        assert_eq!(restored.active_model(), "original-model");
        assert_eq!(restored.backend.default_scope, "workspace");
        assert_eq!(restored.backend.workspace_network, "deny");
    }

    #[test]
    fn older_checkpoints_default_to_bounded_authority_and_empty_counters() {
        let task = Active::start(&Config::default(), Path::new("/tmp"), "old");
        let mut value = serde_json::to_value(task.record()).unwrap();
        for field in [
            "connection_id",
            "connection",
            "execution_scope",
            "network_policy",
            "workspace_root",
            "execution",
            "steering_revision",
            "followup_revision",
            "evidence",
            "workspace_revision",
            "evidence_dropped",
            "timeline",
            "timeline_dropped",
            "timeline_sequence",
            "native_state",
        ] {
            value.as_object_mut().unwrap().remove(field);
        }
        let old: Record = serde_json::from_value(value).unwrap();
        assert_eq!(old.execution, unverified_execution_counters());
        assert!(old.evidence.is_empty());
        assert_eq!(old.workspace_revision, 0);
        assert_eq!(old.followup_revision, 0);
        assert_eq!(old.evidence_dropped, 0);
        assert!(old.timeline.is_empty());
        assert_eq!(old.timeline_dropped, 0);
        assert_eq!(old.timeline_sequence, 0);
        let mut current = Config::default();
        current.backend.default_scope = "host".into();
        current.backend.workspace_network = "allow".into();
        let restored = restore_config(&old, &current).unwrap();
        assert_eq!(restored.backend.default_scope, "workspace");
        assert_eq!(restored.backend.workspace_network, "deny");
    }

    #[test]
    fn timeline_records_actual_checks_without_inventing_model_claims() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "run checks");
        task.path = None;
        task.checkpoint_messages(
            &[Msg::Assistant(crate::providers::AssistantMsg {
                text: Some("All tests passed and every command succeeded".into()),
                tool_calls: vec![],
            })],
            Usage::default(),
        );
        assert!(!task
            .record
            .timeline
            .iter()
            .any(|event| event.kind == EventKind::CheckResult));
        task.begin_execution_evidence(
            "check",
            "cargo test",
            Path::new("/tmp"),
            ExecutionKind::Check,
        )
        .unwrap();
        task.finish_execution_evidence("check", 1, "one failed check", 25, false)
            .unwrap();
        let actual = task
            .record
            .timeline
            .iter()
            .find(|event| event.kind == EventKind::CheckResult)
            .unwrap();
        assert_eq!(actual.outcome, Some(EventOutcome::Failed));
        assert!(actual.detail.contains("Exit code: 1"));
        assert!(!actual.detail.contains("All tests passed"));
        assert_eq!(task.record.check_summary().failed, 1);
    }

    #[test]
    fn timeline_keeps_uncertain_and_unexecuted_tools_distinct() {
        let mut task = Active::start(&Config::default(), Path::new("/tmp"), "continue safely");
        task.path = None;
        let call = ToolCall {
            id: "effect".into(),
            name: "run_command".into(),
            arguments: serde_json::json!({"command":"write effect"}),
        };
        task.pending(&call, &[], Usage::default());
        task.clear_pending_with_result("Budget refused this command");
        assert_eq!(
            task.record.timeline.last().unwrap().outcome,
            Some(EventOutcome::NotExecuted)
        );
        task.pending(&call, &[], Usage::default());
        task.mark_pending_started();
        task.clear_pending_with_result("Worker stopped while this command may have been executing");
        assert_eq!(
            task.record.timeline.last().unwrap().outcome,
            Some(EventOutcome::Uncertain)
        );
        assert!(task
            .record
            .timeline
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence));
    }

    #[test]
    fn timeline_excludes_private_continuation_items_and_bounds_native_history() {
        let mut task = Active::start(
            &Config::default(),
            Path::new("/tmp"),
            "private continuation",
        );
        task.path = None;
        task.checkpoint_messages(&[Msg::ProviderItems {
            items: vec![serde_json::json!({"type":"reasoning","encrypted_content":"opaque-private-continuation","summary":[{"text":"private-reasoning-summary"}]})],
            assistant: crate::providers::AssistantMsg { text: None, tool_calls: vec![] },
        }], Usage::default());
        for index in 0..timeline::MAX_EVENTS + 2 {
            task.record_timeline_event(
                EventKind::PlanNote,
                &format!("note {index}"),
                "A note is not check evidence",
                None,
            )
            .unwrap();
        }
        assert_eq!(task.record.timeline.len(), timeline::MAX_EVENTS);
        assert_eq!(task.record.timeline_dropped, 3);
        let json = serde_json::to_string(&task.record.timeline).unwrap();
        assert!(!json.contains("opaque-private-continuation"));
        assert!(!json.contains("private-reasoning-summary"));
        assert_eq!(task.record.check_summary().total, 0);
    }

    #[test]
    fn native_provider_items_preserve_protocol_state_but_redact_content() {
        let secret = "sk-proj-abcdefghijklmnopqrstuvwxyz123456";
        let reasoning_id = "rs_0123456789abcdefghijklmnopqrstuvwxyz_PROTOCOL";
        let call_id = "call_0123456789abcdefghijklmnopqrstuvwxyz_PROTOCOL";
        let encrypted = "gAAAAAB0123456789abcdefghijklmnopqrstuvwxyz_OPAQUE";
        let message = Msg::ProviderItems {
            items: vec![
                serde_json::json!({
                    "type": "reasoning",
                    "id": reasoning_id,
                    "encrypted_content": encrypted,
                    "summary": [{"text": format!("secret: {secret}")}],
                    "nested": [{"value": secret}],
                }),
                serde_json::json!({
                    "type": "function_call",
                    "id": "fc_0123456789abcdefghijklmnopqrstuvwxyz_PROTOCOL",
                    "call_id": call_id,
                    "name": "run_command",
                    "arguments": format!(r#"{{"command":"TOKEN={secret}"}}"#),
                }),
            ],
            assistant: crate::providers::AssistantMsg {
                text: Some(format!("never persist {secret}")),
                tool_calls: vec![ToolCall {
                    id: call_id.into(),
                    name: "shell".into(),
                    arguments: serde_json::json!({
                        "command": format!("export OPENAI_API_KEY={secret}"),
                        "nested": {"token": secret},
                    }),
                }],
            },
        };
        let serialized = serde_json::to_string(&sanitize_message(&message)).unwrap();
        assert!(!serialized.contains(secret));
        assert!(serialized.contains("<redacted>"));
        assert!(serialized.contains(reasoning_id));
        assert!(serialized.contains(call_id));
        assert!(serialized.contains(encrypted));

        // The exemption is scoped to top-level provider reasoning items. A
        // user-controlled object cannot smuggle a secret through the same key.
        let tool_argument = serde_json::json!({
            "type": "reasoning",
            "encrypted_content": secret,
        });
        assert!(!sanitize_json(&tool_argument).to_string().contains(secret));
    }
}
