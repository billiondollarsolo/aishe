//! Background agent jobs and their git-isolated change lifecycle.

use std::fs::{self, File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::config::Config;

pub mod changes;
mod interactions;
mod metadata;
mod presentation;
pub mod scheduler;
pub mod workflows;
pub use changes::{
    apply_task_changes, task_change_review, ApplyChanges, ChangeFile, ChangeHunk, ChangeKind,
    ChangeReview,
};
pub use interactions::{
    acknowledge_followups, claim_followups, consume_interaction, create_approval, create_question,
    interaction_for_call, interaction_response, interaction_summary, queued_followups, Followup,
    FollowupStatus, InteractionBinding, InteractionKind, InteractionMailbox, InteractionRequest,
    InteractionResponse, InteractionStatus, InteractionSummary,
};
pub use metadata::{
    archive_task, mark_task_reviewed, mark_task_seen, pin_task, rename_task, TaskMetadata,
};
pub use presentation::{
    acknowledge_task, cached_task_entries, cached_task_entries_with_query, read_seen,
    refresh_task_cache, shell_status_text, task_details, task_entries, task_entries_with_query,
    task_patch_lines, task_status, task_timeline, EntryQuery, SeenTasks, TaskAttention,
    TaskCheckpoint, TaskDetails, TaskEntry, TaskStatus, TaskTimeline,
};

const SCHEMA_VERSION: u32 = 1;
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;
const MAX_OBJECTIVE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Starting,
    Running,
    Waiting,
    Blocked,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
    Applied,
    Discarded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Active,
    Completed,
    Blocked,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlanStep {
    pub id: u32,
    pub text: String,
    pub state: StepState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Budget {
    pub max_minutes: u32,
    pub max_provider_turns: u32,
    pub max_cost_usd: f64,
    #[serde(default)]
    pub max_tool_calls: u32,
    #[serde(default)]
    pub max_changed_files: u32,
    #[serde(default)]
    pub max_changed_bytes: u64,
    #[serde(default)]
    pub max_network_calls: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Record {
    pub schema_version: u32,
    pub id: String,
    pub objective: String,
    pub source_cwd: PathBuf,
    pub run_cwd: PathBuf,
    pub source_repo: Option<PathBuf>,
    pub worktree: Option<PathBuf>,
    pub base_head: Option<String>,
    pub source_branch: Option<String>,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub state: State,
    #[serde(default)]
    pub result_revision: u64,
    #[serde(default)]
    pub mailbox: InteractionMailbox,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<workflows::StageLink>,
    #[serde(default)]
    pub foreground: bool,
    /// A completed stage is sealed while its snapshot is captured and after it
    /// has been handed forward. Rework starts a new workflow after that point.
    #[serde(default)]
    pub workflow_frozen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default)]
    pub connection_id: String,
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<crate::config::ConnectionConfig>,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub network: String,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_started_at_ms: Option<u128>,
    #[serde(default)]
    pub steering_revision: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering: Vec<String>,
    pub pid: Option<u32>,
    pub process_start: Option<String>,
    pub exit_code: Option<i32>,
    pub budget: Budget,
    #[serde(default)]
    pub plan: Vec<PlanStep>,
    #[serde(default)]
    pub plan_revision: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applied_hunks: Vec<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied_patch_sha256: Option<String>,
    #[serde(default)]
    pub budget_exceeded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Action {
    Browse {
        id: Option<String>,
        all: bool,
        needs_you: bool,
        archived: bool,
    },
    Start {
        objective: String,
        no_isolation: bool,
        max_minutes: u32,
        max_turns: u32,
        max_cost: Option<f64>,
        max_tool_calls: u32,
        max_changed_files: u32,
        max_changed_bytes: u64,
        max_network_calls: u32,
    },
    List {
        json: bool,
    },
    Show {
        id: String,
        json: bool,
    },
    Tail {
        id: String,
        lines: usize,
    },
    Cancel {
        id: String,
    },
    Resume {
        id: String,
    },
    Review {
        id: String,
    },
    Rework {
        id: String,
        instructions: String,
    },
    Apply {
        id: String,
        hunks: Vec<usize>,
    },
    ApplyReviewed {
        id: String,
        revision: String,
        files: Vec<usize>,
        hunks: Vec<usize>,
    },
    Discard {
        id: String,
    },
    Plan {
        id: String,
        steps: Vec<String>,
    },
    Replan {
        id: String,
        steps: Vec<String>,
    },
    Step {
        id: String,
        step: u32,
        state: StepState,
        evidence: Option<String>,
    },
    Answer {
        id: String,
        request_id: String,
        text: String,
    },
    Approve {
        id: String,
        request_id: String,
    },
    Deny {
        id: String,
        request_id: String,
        reason: String,
    },
    Followup {
        id: String,
        text: String,
    },
    EditFollowup {
        id: String,
        revision: u32,
        text: String,
    },
    RemoveFollowup {
        id: String,
        revision: u32,
    },
    Rename {
        id: String,
        name: String,
    },
    Pin {
        id: String,
        pinned: bool,
    },
    Archive {
        id: String,
        archived: bool,
    },
    Reviewed {
        id: String,
    },
}

pub fn command(config: &Config, action: Action) -> Result<u8> {
    let result = match action {
        Action::Browse {
            id,
            all,
            needs_you,
            archived,
        } => crate::cli::taskui::browse_filtered(config, id.as_deref(), all, needs_you, archived),
        Action::Start {
            objective,
            no_isolation,
            max_minutes,
            max_turns,
            max_cost,
            max_tool_calls,
            max_changed_files,
            max_changed_bytes,
            max_network_calls,
        } => start(
            config,
            &objective,
            no_isolation,
            Budget {
                max_minutes: max_minutes.clamp(1, 24 * 60),
                max_provider_turns: max_turns.clamp(1, 1_000),
                max_cost_usd: max_cost.unwrap_or(config.aishe.budget_usd).max(0.0),
                max_tool_calls: max_tool_calls.clamp(1, 10_000),
                max_changed_files: max_changed_files.clamp(1, 10_000),
                max_changed_bytes: max_changed_bytes.clamp(1, 1024 * 1024 * 1024),
                max_network_calls: max_network_calls.clamp(1, 10_000),
            },
        ),
        Action::List { json } => list(json),
        Action::Show { id, json } => show(&id, json),
        Action::Tail { id, lines } => tail(&id, lines.clamp(1, 10_000)),
        Action::Cancel { id } => cancel(&id),
        Action::Resume { id } => resume(config, &id),
        Action::Review { id } => review(config, &id),
        Action::Rework { id, instructions } => rework(config, &id, &instructions),
        Action::Apply { id, hunks } => apply(&id, &hunks),
        Action::ApplyReviewed {
            id,
            revision,
            files,
            hunks,
        } => {
            let applied = apply_task_changes(&id, &revision, &files, &hunks)?;
            println!("{}", serde_json::to_string_pretty(&applied)?);
            Ok(0)
        }
        Action::Discard { id } => discard(&id),
        Action::Plan { id, steps } => set_plan(&id, steps, false),
        Action::Replan { id, steps } => set_plan(&id, steps, true),
        Action::Step {
            id,
            step,
            state,
            evidence,
        } => set_step(&id, step, state, evidence.as_deref()),
        Action::Answer {
            id,
            request_id,
            text,
        } => interactions::respond(
            config,
            &id,
            &request_id,
            InteractionResponse::Answer { text },
        ),
        Action::Approve { id, request_id } => {
            interactions::respond(config, &id, &request_id, InteractionResponse::Approved)
        }
        Action::Deny {
            id,
            request_id,
            reason,
        } => interactions::respond(
            config,
            &id,
            &request_id,
            InteractionResponse::Denied { reason },
        ),
        Action::Followup { id, text } => interactions::queue_followup(&id, &text),
        Action::EditFollowup { id, revision, text } => {
            interactions::edit_followup(&id, revision, &text)
        }
        Action::RemoveFollowup { id, revision } => interactions::remove_followup(&id, revision),
        Action::Rename { id, name } => rename_task(&id, &name).map(|_| 0),
        Action::Pin { id, pinned } => pin_task(&id, pinned).map(|_| 0),
        Action::Archive { id, archived } => archive_task(&id, archived).map(|_| 0),
        Action::Reviewed { id } => {
            let details = task_details(&id)?;
            mark_task_reviewed(&details.entry).map(|_| 0)
        }
    };
    refresh_status();
    result
}

/// Background workers own a new session as well as a process group. A group
/// alone still inherits the drawer's controlling terminal and can interfere
/// with its input or terminal state when the drawer resumes the task.
fn detach_worker(child: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and this closure accesses no
        // shared process state between fork and exec.
        unsafe {
            child.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    #[cfg(not(unix))]
    let _ = child;
}

fn start(config: &Config, objective: &str, no_isolation: bool, budget: Budget) -> Result<u8> {
    let objective = objective.trim();
    if objective.is_empty() || objective.len() > MAX_OBJECTIVE_BYTES {
        anyhow::bail!("task objective must contain 1..={MAX_OBJECTIVE_BYTES} bytes");
    }
    let source_cwd = std::env::current_dir()?.canonicalize()?;
    validate_host_admission(config, &config.backend.default_scope, &source_cwd)?;
    let id = new_id();
    let dir = task_dir(&id)?;
    fs::create_dir_all(&dir)?;
    set_private(&dir, 0o700);

    let git = git_identity(&source_cwd);
    let (source_repo, worktree, run_cwd, base_head, source_branch) = match git {
        Some((repo, head, branch)) if !no_isolation => {
            let worktree = dir.join("worktree");
            let output = Command::new("git")
                .args(["-C", &repo.display().to_string(), "worktree", "add", "--detach"])
                .arg(&worktree)
                .arg(&head)
                .output()
                .context("creating isolated git worktree")?;
            if !output.status.success() {
                anyhow::bail!(
                    "git worktree isolation failed: {}",
                    crate::commands::display_safe(&String::from_utf8_lossy(&output.stderr))
                );
            }
            (
                Some(repo),
                Some(worktree.clone()),
                worktree,
                Some(head),
                branch,
            )
        }
        Some((repo, head, branch)) => (Some(repo), None, source_cwd.clone(), Some(head), branch),
        None if no_isolation => (None, None, source_cwd.clone(), None, None),
        None => anyhow::bail!(
            "background writes require a git worktree; rerun with --no-isolation to explicitly use the current directory"
        ),
    };

    let now = now_ms();
    let record = Record {
        schema_version: SCHEMA_VERSION,
        id: id.clone(),
        objective: crate::redact::redact(objective),
        source_cwd,
        run_cwd: run_cwd.clone(),
        source_repo,
        worktree,
        base_head,
        source_branch,
        created_at_ms: now,
        updated_at_ms: now,
        state: State::Starting,
        result_revision: 0,
        mailbox: InteractionMailbox::default(),
        workflow: None,
        foreground: false,
        workflow_frozen: false,
        native_task_id: None,
        engine: Some(actual_engine(config, crate::lean::enabled()).into()),
        connection_id: config.active_connection_id().into(),
        provider: config.active_provider_name().into(),
        model: config.active_model().into(),
        connection: crate::tasks::snapshot_connection(config),
        role: std::env::var("AISHE_ROLE").unwrap_or_else(|_| "build".into()),
        scope: config.backend.default_scope.clone(),
        network: config.backend.workspace_network.clone(),
        elapsed_ms: 0,
        attempt_started_at_ms: Some(now),
        steering_revision: 0,
        steering: Vec::new(),
        pid: None,
        process_start: None,
        exit_code: None,
        budget,
        plan: Vec::new(),
        plan_revision: 0,
        applied_hunks: Vec::new(),
        applied_patch_sha256: None,
        budget_exceeded: false,
        error: None,
    };
    write_private(&request_path(&id)?, objective.as_bytes())?;
    save(&record)?;

    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(&id)?)?;
    set_private(&log_path(&id)?, 0o600);
    let stderr = log.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?);
    restrict_background_environment(&mut child, config);
    child
        .args([
            "--connection",
            config.active_connection_id(),
            "--model",
            config.active_model(),
            "--mode",
            "yolo",
            "--background-task",
            &id,
        ])
        .current_dir(&run_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .env("AISHE_SHELL_ID", format!("task-{id}"))
        .env("AISHE_BACKGROUND_TASK_ID", &id)
        .env("AISHE_TASK_ROLE", &record.role)
        .env("AISHE_TASK_SCOPE", &record.scope)
        .env(
            "AISHE_TASK_MAX_TOOL_CALLS",
            record.budget.max_tool_calls.to_string(),
        )
        .env(
            "AISHE_TASK_MAX_NETWORK_CALLS",
            record.budget.max_network_calls.to_string(),
        );
    set_budget_environment(&mut child, &record.budget);
    set_worker_engine_environment(&mut child, &record, config)?;
    detach_worker(&mut child);
    let child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            finish(&id, &Err(anyhow::anyhow!(error.to_string())));
            return Err(error).context("starting background agent");
        }
    };
    let identity = process_start(child.id());
    update(&id, |fresh| {
        // A fast worker may already have attached or finished its native task.
        // Never replace that checkpoint with the parent's pre-spawn snapshot.
        if fresh.state == State::Starting {
            fresh.pid = Some(child.id());
            fresh.process_start = identity;
            fresh.state = State::Running;
        }
        Ok(())
    })?;
    println!("started task {id}");
    println!("workspace: {}", run_cwd.display());
    println!("tail: aishe task tail {id}");
    Ok(0)
}

pub fn request(id: &str) -> Result<(String, Budget)> {
    validate_id(id)?;
    let record = load(id)?;
    if !matches!(
        record.state,
        State::Starting | State::Running | State::Interrupted
    ) {
        anyhow::bail!("task {id} cannot run from state {:?}", record.state);
    }
    let file = File::open(request_path(id)?)?;
    let mut bytes = Vec::new();
    file.take((MAX_OBJECTIVE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_OBJECTIVE_BYTES {
        anyhow::bail!("task request exceeds the configured bound");
    }
    Ok((
        String::from_utf8(bytes).context("task request is not UTF-8")?,
        record.budget,
    ))
}

/// Link the already-private native checkpoint before any detached effect.
/// Repeated saves are harmless; replacing a checkpoint with a new objective is
/// rejected so resume cannot accidentally restart the job.
pub fn attach_native_task(id: &str, native_task_id: &str) -> Result<()> {
    let checkpoint = crate::tasks::load(native_task_id)?;
    update(id, |record| {
        if !matches!(record.state, State::Starting | State::Running) {
            anyhow::bail!(
                "background task {id} is {:?}; no further effects are admitted",
                record.state
            );
        }
        validate_native_workspace(record, &checkpoint)?;
        if let Some(existing) = &record.native_task_id {
            if existing != native_task_id {
                anyhow::bail!("background task {id} already owns native checkpoint {existing}");
            }
        } else {
            record.native_task_id = Some(native_task_id.into());
        }
        // The worker also records its identity. If the launching process dies
        // between spawn and its PID checkpoint, the worker remains observable.
        if record.state == State::Starting {
            record.pid = Some(std::process::id());
            record.process_start = process_start(std::process::id());
            record.state = State::Running;
        }
        Ok(())
    })
}

/// Load the actual transcript, pending tool and cumulative reservations. A
/// fresh job has no native checkpoint yet; a resumed job is never reconstructed
/// from its original objective.
pub fn resume_checkpoint(id: &str) -> Result<Option<crate::tasks::Record>> {
    let record = load(id)?;
    let Some(native_id) = &record.native_task_id else {
        return Ok(None);
    };
    let mut checkpoint = crate::tasks::load(native_id)?;
    validate_native_workspace(&record, &checkpoint)?;
    checkpoint.execution.elapsed_ms = checkpoint.execution.elapsed_ms.max(elapsed(&record));
    if checkpoint.steering_revision > record.steering_revision {
        anyhow::bail!("background task {id} has an invalid steering checkpoint");
    }
    let has_new_steering = checkpoint.steering_revision < record.steering_revision;
    for instruction in record
        .steering
        .iter()
        .skip(checkpoint.steering_revision as usize)
    {
        checkpoint
            .messages
            .push(crate::providers::Msg::User(format!(
                "Rework request:\n{}",
                crate::redact::redact(instruction)
            )));
    }
    checkpoint.steering_revision = record.steering_revision;
    let has_followups = record
        .mailbox
        .followups
        .iter()
        .any(|entry| entry.status == FollowupStatus::Queued);
    if checkpoint.status == crate::tasks::Status::Completed && (has_new_steering || has_followups) {
        checkpoint.status = crate::tasks::Status::Interrupted;
    }
    Ok(Some(checkpoint))
}

pub fn worker_config(id: &str, current: &Config) -> Result<Config> {
    let record = load(id)?;
    let mut config = if let Some(native_id) = &record.native_task_id {
        crate::tasks::restore_config(&crate::tasks::load(native_id)?, current)?
    } else {
        let mut config = current.clone();
        if record.connection_id.is_empty() || record.provider.is_empty() || record.model.is_empty()
        {
            anyhow::bail!("background task {id} has no saved provider identity");
        }
        crate::tasks::restore_connection(
            &mut config,
            &record.connection_id,
            &record.provider,
            &record.model,
            record.connection.as_ref(),
        )?;
        config
    };
    if crate::agent::ExecutionScope::parse(&record.scope).is_none() {
        anyhow::bail!("background task {id} has no valid saved execution scope");
    }
    if crate::agent::NetworkPolicy::parse(&record.network).is_none() {
        anyhow::bail!("background task {id} has no valid saved network policy");
    }
    config.backend.default_scope.clone_from(&record.scope);
    config.backend.workspace_network.clone_from(&record.network);
    config.aishe.mode = "yolo".into();
    config.backend.engine = selected_engine(&record, current, crate::lean::enabled())?.into();
    Ok(config)
}

pub fn worker_role(id: &str) -> Result<String> {
    let record = load(id)?;
    if record.role.is_empty() {
        anyhow::bail!("background task {id} has no saved role");
    }
    Ok(record.role)
}

pub fn worker_cwd(id: &str) -> Result<PathBuf> {
    let record = load(id)?;
    if !record.run_cwd.is_absolute() || !record.run_cwd.is_dir() {
        anyhow::bail!(
            "background task {id} workspace {} is unavailable",
            record.run_cwd.display()
        );
    }
    Ok(record.run_cwd)
}

pub fn is_cancelled(id: &str) -> Result<bool> {
    Ok(load(id)?.state == State::Cancelled)
}

pub fn remaining_deadline_seconds(id: &str) -> Result<u32> {
    let record = load(id)?;
    let checkpoint_elapsed = record
        .native_task_id
        .as_deref()
        .and_then(|native_id| crate::tasks::load(native_id).ok())
        .map_or(0, |checkpoint| checkpoint.execution.elapsed_ms);
    let spent = elapsed(&record).max(checkpoint_elapsed);
    let remaining = u64::from(record.budget.max_minutes)
        .saturating_mul(60_000)
        .saturating_sub(spent);
    if remaining == 0 {
        anyhow::bail!("background task {id} wall-clock budget is exhausted");
    }
    Ok(remaining.div_ceil(1_000).min(u64::from(u32::MAX)) as u32)
}

pub fn arm_deadline(minutes: u32) {
    arm_deadline_seconds(minutes.saturating_mul(60));
}

pub fn arm_deadline_seconds(seconds: u32) {
    #[cfg(unix)]
    unsafe {
        libc::alarm(seconds);
    }
}

pub fn finish(id: &str, result: &Result<u8>) {
    let _ = update(id, |record| {
        let (state, code, error) = match result {
            Ok(0) => (State::Completed, 0, None),
            Ok(130) => (State::Cancelled, 130, Some("agent was cancelled".into())),
            Ok(2) => (
                State::Interrupted,
                2,
                Some("agent admission was declined".into()),
            ),
            Ok(code) => (
                State::Failed,
                i32::from(*code),
                Some(format!("agent exited with status {code}")),
            ),
            Err(error) => (
                State::Failed,
                1,
                Some(crate::redact::redact(&error.to_string())),
            ),
        };
        finish_record(record, state, code, error);
        Ok(())
    });
    refresh_status();
}

pub fn finish_native(id: &str, outcome: &crate::agent::NativeTurnOutcome) -> Result<u8> {
    let mut exit_code = outcome.exit_code();
    update(id, |record| {
        exit_code = finish_native_record(record, outcome, crate::tasks::mark_background_cancelled)?;
        Ok(())
    })?;
    refresh_status();
    if let Some(link) = load(id)?.workflow {
        let _ = scheduler::ensure_running(&link.run_id);
    }
    Ok(exit_code)
}

fn finish_native_record(
    record: &mut Record,
    outcome: &crate::agent::NativeTurnOutcome,
    reconcile_cancelled: impl FnOnce(&str) -> Result<()>,
) -> Result<u8> {
    if record.native_task_id.as_deref() != Some(outcome.task_id.as_str()) {
        anyhow::bail!(
            "background task {} outcome does not match its native checkpoint",
            record.id
        );
    }
    if record.state == State::Cancelled {
        // Cancellation can win after the model's final interrupt check and
        // before this callback. Align its checkpoint without dropping the
        // transcript, pending-tool uncertainty or spent reservations.
        reconcile_cancelled(&outcome.task_id)?;
        record.exit_code = Some(130);
        return Ok(130);
    }
    let required_failure = (outcome.state == crate::agent::NativeTurnState::Completed)
        .then(|| scheduler::required_evidence(record).err())
        .flatten();
    finish_record(
        record,
        if required_failure.is_some() {
            State::Failed
        } else {
            native_finish_state(outcome.state)
        },
        if required_failure.is_some() {
            1
        } else {
            i32::from(outcome.exit_code())
        },
        required_failure
            .map(|error| crate::redact::redact(&error.to_string()))
            .or_else(|| outcome.detail.as_deref().map(crate::redact::redact)),
    );
    Ok(record
        .exit_code
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(outcome.exit_code()))
}

fn native_finish_state(state: crate::agent::NativeTurnState) -> State {
    use crate::agent::NativeTurnState;
    match state {
        NativeTurnState::Completed => State::Completed,
        NativeTurnState::Waiting => State::Waiting,
        NativeTurnState::HandedOff => State::Interrupted,
        NativeTurnState::Cancelled => State::Cancelled,
        NativeTurnState::Declined => State::Interrupted,
        NativeTurnState::BudgetExhausted
        | NativeTurnState::IterationLimit
        | NativeTurnState::Failed => State::Failed,
    }
}

fn finish_record(record: &mut Record, state: State, code: i32, error: Option<String>) {
    // Cancellation, review/apply and an earlier terminal result win a race with
    // a delayed callback. Only the active attempt may publish a final outcome.
    if !matches!(
        record.state,
        State::Starting | State::Running | State::Interrupted
    ) {
        return;
    }
    settle_elapsed(record);
    record.state = state;
    record.exit_code = Some(code);
    record.error = error;
    // A paused worker is about to exit but may still be alive. Retain its
    // identity so an immediate response cannot race another writer against
    // the same native checkpoint. Waiting time is excluded by settle_elapsed.
    if state != State::Waiting {
        record.pid = None;
        record.process_start = None;
    }
    if state == State::Completed
        && record
            .mailbox
            .followups
            .iter()
            .any(|entry| entry.status == FollowupStatus::Queued)
    {
        record.state = State::Interrupted;
        record.exit_code = Some(75);
        record.error =
            Some("a follow-up arrived as the task finished; resume to deliver it".into());
    }
    if matches!(state, State::Cancelled | State::Applied | State::Discarded) {
        interactions::invalidate_pending(record);
    }
    if let Some((files, bytes)) = changed_usage(record) {
        if (record.budget.max_changed_files > 0 && files > record.budget.max_changed_files)
            || (record.budget.max_changed_bytes > 0 && bytes > record.budget.max_changed_bytes)
        {
            record.state = State::Failed;
            record.exit_code = Some(1);
            record.error = Some(format!(
                "task change budget exceeded: {files} files / {bytes} bytes"
            ));
            record.budget_exceeded = true;
        }
    }
    if record.budget_exceeded {
        interactions::invalidate_pending(record);
    } else if record.state != State::Waiting {
        // A budget/cancellation/failure can win just after request creation.
        // Do not leave an unanswerable question looking live in a failed task.
        for request in &mut record.mailbox.requests {
            if request.status == InteractionStatus::Pending {
                request.status = InteractionStatus::Invalidated;
            }
        }
    }
}

fn elapsed(record: &Record) -> u64 {
    record
        .elapsed_ms
        .saturating_add(record.attempt_started_at_ms.map_or(0, |started| {
            now_ms().saturating_sub(started).min(u128::from(u64::MAX)) as u64
        }))
}

fn settle_elapsed(record: &mut Record) {
    record.elapsed_ms = elapsed(record);
    record.attempt_started_at_ms = None;
}

fn refresh_status() {
    let Some(path) = std::env::var_os("AISHE_STATUS_FILE").filter(|value| !value.is_empty()) else {
        return;
    };
    let active = task_status(None, &SeenTasks::default())
        .map(|status| status.running)
        .unwrap_or_default();
    crate::usagelog::merge_status(
        Path::new(&path),
        &[(
            "tasks",
            if active == 0 {
                String::new()
            } else {
                format!("{active} task{}", if active == 1 { "" } else { "s" })
            },
        )],
    );
}

fn list(json: bool) -> Result<u8> {
    let mut records = records()?;
    for record in &mut records {
        reconcile(record)?;
    }
    records.retain(|record| !TaskEntry::from_record(record).archived);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "tasks": records,
            }))?
        );
    } else if records.is_empty() {
        println!("no background tasks");
    } else {
        for record in records {
            let entry = TaskEntry::from_record(&record);
            println!(
                "{}  {:?}  {}  {}{}{}",
                record.id,
                record.state,
                branch_label(&record),
                entry.title.chars().take(72).collect::<String>(),
                if entry.pinned { " · pinned" } else { "" },
                if entry.reviewed {
                    " · reviewed"
                } else if entry.seen {
                    " · seen"
                } else {
                    ""
                },
            );
        }
    }
    Ok(0)
}

pub fn inbox(config: &Config, json: bool) -> Result<u8> {
    if !json && std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        return crate::cli::taskui::browse_filtered(config, None, true, true, false);
    }
    let mut items = records()?;
    for record in &mut items {
        reconcile(record)?;
    }
    items.retain(|record| {
        record.state == State::Waiting
            && record
                .mailbox
                .requests
                .iter()
                .any(|request| request.status == InteractionStatus::Pending)
    });
    items.sort_by_key(|record| std::cmp::Reverse(record.updated_at_ms));
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "items": items,
            }))?
        );
    } else if items.is_empty() {
        println!("inbox zero · no tasks need your response");
    } else {
        for record in items {
            let entry = TaskEntry::from_record(&record);
            println!("{}  needs you  {}", record.id, entry.title);
            for request in record
                .mailbox
                .requests
                .iter()
                .filter(|request| request.status == InteractionStatus::Pending)
            {
                println!(
                    "  {}  {:?}  {}",
                    request.id,
                    request.kind,
                    crate::commands::display_safe_multiline(&request.prompt)
                );
            }
        }
    }
    Ok(0)
}

pub fn palette_summaries() -> Vec<(String, State, String)> {
    records()
        .unwrap_or_default()
        .into_iter()
        .filter(|record| !matches!(record.state, State::Applied | State::Discarded))
        .map(|record| (record.id, record.state, record.objective))
        .collect()
}

pub fn edit_plan(id: Option<&str>, preserve_completed: bool) -> Result<u8> {
    let records = records()?
        .into_iter()
        .filter(|record| !matches!(record.state, State::Applied | State::Discarded))
        .collect::<Vec<_>>();
    let id = match id {
        Some(id) => id.to_string(),
        None => {
            if records.is_empty() {
                anyhow::bail!(
                    "no background task is available; start one with `aishe agent --background`"
                );
            }
            let labels = records
                .iter()
                .map(|record| format!("{:?} · {} · {}", record.state, record.id, record.objective))
                .collect::<Vec<_>>();
            let crate::promptui::PickerResult::Use(index) =
                crate::promptui::filter_picker("Choose task plan", &labels, labels.len() - 1)?
            else {
                return Ok(0);
            };
            records[index].id.clone()
        }
    };
    let record = load(&id)?;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return show(&id, false);
    }
    let default = if record.plan.is_empty() {
        "inspect; implement; run focused tests".into()
    } else {
        record
            .plan
            .iter()
            .map(|step| step.text.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    };
    let Some(value) =
        crate::promptui::text("Plan steps (separate with semicolons)", &default, |value| {
            let count = value
                .split(';')
                .filter(|step| !step.trim().is_empty())
                .count();
            if !(1..=100).contains(&count) {
                anyhow::bail!("enter 1..=100 non-empty steps")
            }
            Ok(())
        })?
    else {
        return Ok(0);
    };
    if value == ":back" {
        return Ok(0);
    }
    let steps = value
        .split(';')
        .map(str::trim)
        .filter(|step| !step.is_empty())
        .map(ToOwned::to_owned)
        .collect();
    set_plan(&id, steps, preserve_completed)
}

fn show(id: &str, json: bool) -> Result<u8> {
    let mut record = load(id)?;
    reconcile(&mut record)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        println!("task: {}", record.id);
        println!("state: {:?}", record.state);
        println!("objective: {}", record.objective);
        println!("workspace: {}", record.run_cwd.display());
        println!("source: {}", record.source_cwd.display());
        println!("branch: {}", branch_label(&record));
        println!(
            "budget: {}m · {} turns · {}",
            record.budget.max_minutes,
            record.budget.max_provider_turns,
            if record.budget.max_cost_usd > 0.0 {
                format!("${:.2}", record.budget.max_cost_usd)
            } else {
                "existing session cap".into()
            }
        );
        println!(
            "limits: {} tools · {} network · {} files · {} bytes changed",
            record.budget.max_tool_calls,
            record.budget.max_network_calls,
            record.budget.max_changed_files,
            record.budget.max_changed_bytes,
        );
        if !record.plan.is_empty() {
            println!("plan:");
            for step in &record.plan {
                println!("  {}  {:?}  {}", step.id, step.state, step.text);
                if let Some(evidence) = &step.evidence {
                    println!("       evidence: {evidence}");
                }
            }
        }
        if let Some(error) = record.error {
            println!("error: {error}");
        }
    }
    Ok(0)
}

fn tail(id: &str, lines: usize) -> Result<u8> {
    validate_id(id)?;
    let path = log_path(id)?;
    let metadata = fs::metadata(&path).with_context(|| format!("no log for task {id}"))?;
    let start = metadata.len().saturating_sub(MAX_LOG_BYTES);
    let mut file = File::open(path)?;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(start))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let selected = text.lines().rev().take(lines).collect::<Vec<_>>();
    for line in selected.into_iter().rev() {
        println!("{}", crate::commands::display_safe(line));
    }
    Ok(0)
}

fn cancel(id: &str) -> Result<u8> {
    let mut snapshot = load(id)?;
    reconcile(&mut snapshot)?;
    let mut signalled = None;
    let mut cancelled = false;
    update(id, |record| {
        if !matches!(
            record.state,
            State::Starting | State::Running | State::Waiting | State::Blocked
        ) {
            return Ok(());
        }
        let pid_to_signal = record.pid.filter(|pid| {
            record.state != State::Waiting || same_process(*pid, record.process_start.as_deref())
        });
        if let Some(pid) = pid_to_signal {
            if !same_process(pid, record.process_start.as_deref()) {
                anyhow::bail!("refusing to signal task {id}: process identity changed");
            }
            #[cfg(unix)]
            unsafe {
                // The native command executor owns separate child process
                // groups. Give its SIGINT watchdog a chance to reap those
                // groups before terminating the worker itself.
                let target = if record.foreground {
                    pid as i32
                } else {
                    -(pid as i32)
                };
                if libc::kill(target, libc::SIGINT) != 0 {
                    anyhow::bail!("could not signal task {id}");
                }
            }
            #[cfg(not(unix))]
            anyhow::bail!("task cancellation is not yet supported on this platform");
            signalled = Some((pid, record.process_start.clone(), record.foreground));
        }
        settle_elapsed(record);
        record.state = State::Cancelled;
        record.exit_code = Some(130);
        record.error = Some("cancelled by user".into());
        interactions::invalidate_pending(record);
        if signalled.is_none() {
            record.pid = None;
            record.process_start = None;
            if let Some(native_id) = &record.native_task_id {
                crate::tasks::mark_background_cancelled(native_id)?;
            }
        }
        cancelled = true;
        Ok(())
    })?;
    if let Some((pid, identity, foreground)) = signalled {
        for _ in 0..20 {
            let fresh = load(id)?;
            if fresh.state != State::Cancelled
                || fresh.pid != Some(pid)
                || fresh.process_start != identity
                || !same_process(pid, identity.as_deref())
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        #[cfg(unix)]
        update(id, |fresh| {
            if fresh.state == State::Cancelled
                && fresh.pid == Some(pid)
                && fresh.process_start == identity
                && same_process(pid, identity.as_deref())
            {
                unsafe {
                    libc::kill(
                        if foreground {
                            pid as i32
                        } else {
                            -(pid as i32)
                        },
                        libc::SIGTERM,
                    );
                }
            }
            Ok(())
        })?;
        // Keep the process identity while a worker is still stopping. Resume
        // must not race a previous attempt writing the same native checkpoint.
        update(id, |record| {
            if record.state == State::Cancelled
                && record.pid == Some(pid)
                && !same_process(pid, identity.as_deref())
            {
                record.pid = None;
                record.process_start = None;
                if let Some(native_id) = &record.native_task_id {
                    crate::tasks::mark_background_cancelled(native_id)?;
                }
            }
            Ok(())
        })?;
    }
    if cancelled {
        if let Some(native_id) = load(id)?.native_task_id {
            let _ = crate::agent::native::handoff::discard_environment(&native_id);
        }
        let _ = crate::tasks::timeline::append_background(
            id,
            crate::tasks::timeline::EventKind::Finished,
            "Task cancelled",
            "Cancelled by the user; remaining workflow effects are not admitted.",
            Some(crate::tasks::timeline::EventOutcome::Cancelled),
        );
        println!("cancelled task {id}");
    } else {
        println!("task {id} is {:?}; nothing to cancel", load(id)?.state);
    }
    Ok(0)
}

fn resume(config: &Config, id: &str) -> Result<u8> {
    resume_with_environment(config, id, None, true)
}

fn resume_with_environment(
    config: &Config,
    id: &str,
    environment: Option<&std::collections::HashMap<String, String>>,
    announce: bool,
) -> Result<u8> {
    let mut record = load(id)?;
    reconcile(&mut record)?;
    if !matches!(
        record.state,
        State::Waiting | State::Interrupted | State::Failed | State::Cancelled
    ) {
        anyhow::bail!("task {id} cannot resume from state {:?}", record.state);
    }
    if record.state == State::Waiting {
        interactions::ready_to_resume(&record)?;
    }
    if !record.run_cwd.is_dir() {
        anyhow::bail!("task workspace {} is missing", record.run_cwd.display());
    }
    if record
        .pid
        .is_some_and(|pid| same_process(pid, record.process_start.as_deref()))
    {
        anyhow::bail!("task {id} is still stopping; resume after its worker has exited");
    }
    changes::ensure_no_partial_apply(&record)?;
    let checkpoint = resume_checkpoint(id)?.with_context(|| format!(
        "task {id} has no durable native checkpoint; start a new task explicitly instead of replaying its objective"
    ))?;
    validate_remaining_budget(&record, &checkpoint)?;
    let mut config = worker_config(id, config)?;
    crate::policy::constrain(&mut config)?;
    validate_host_admission(&config, &record.scope, &record.source_cwd)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(id)?)?;
    writeln!(&log, "\n--- resumed {} ---", now_ms()).ok();
    let stderr = log.try_clone()?;
    workflows::claim_slot(id, || {
        update(id, |fresh| {
            if !matches!(
                fresh.state,
                State::Waiting | State::Interrupted | State::Failed | State::Cancelled
            ) {
                anyhow::bail!("task {id} cannot resume from state {:?}", fresh.state);
            }
            if fresh.state == State::Waiting {
                interactions::ready_to_resume(fresh)?;
            }
            changes::ensure_no_partial_apply(fresh)?;
            if fresh.workflow_frozen {
                anyhow::bail!(
                    "workflow stage {id} was handed forward; run a new workflow to change it"
                );
            }
            fresh.state = State::Starting;
            fresh.foreground = false;
            fresh.attempt_started_at_ms = Some(now_ms());
            fresh.pid = None;
            fresh.process_start = None;
            fresh.exit_code = None;
            fresh.error = None;
            Ok(())
        })
    })?;
    let mut child = Command::new(std::env::current_exe()?);
    restrict_background_environment(&mut child, &config);
    if let Some(environment) = environment {
        let denied = crate::executor::sensitive_environment_names(&config);
        for (name, value) in environment {
            if crate::executor::agent_environment_allowed(name, &denied) {
                child.env(name, value);
            }
        }
    }
    child
        .args([
            "--connection",
            config.active_connection_id(),
            "--model",
            config.active_model(),
            "--mode",
            "yolo",
            "--background-task",
            id,
        ])
        .current_dir(&record.run_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .env("AISHE_SHELL_ID", format!("task-{id}"))
        .env("AISHE_BACKGROUND_TASK_ID", id)
        .env("AISHE_TASK_ROLE", &record.role)
        .env("AISHE_TASK_SCOPE", &record.scope);
    set_budget_environment(&mut child, &record.budget);
    set_worker_engine_environment(&mut child, &record, &config)?;
    detach_worker(&mut child);
    let child = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            finish(id, &Err(anyhow::anyhow!(error.to_string())));
            return Err(error).context("resuming background worker");
        }
    };
    let identity = process_start(child.id());
    update(id, |fresh| {
        if fresh.state == State::Starting {
            fresh.pid = Some(child.id());
            fresh.process_start = identity;
            fresh.state = State::Running;
        }
        Ok(())
    })?;
    if announce {
        println!("resumed task {id}");
    }
    let _ = crate::tasks::timeline::append_background(
        id,
        crate::tasks::timeline::EventKind::Resumed,
        "Checkpoint resumed",
        "Saved provider identity, scope and remaining cumulative budget retained.",
        None,
    );
    if let Some(link) = &record.workflow {
        let _ = scheduler::ensure_running(&link.run_id);
    }
    Ok(0)
}

/// Start only a durably claimed fresh workflow stage. Resumes always use the
/// native checkpoint path above, so a scheduler restart cannot replay effects.
fn launch_stage(config: &Config, id: &str) -> Result<()> {
    let record = load(id)?;
    if record.state != State::Starting || record.native_task_id.is_some() {
        anyhow::bail!("workflow stage {id} is not a fresh claimed task");
    }
    let mut selected = worker_config(id, config)?;
    crate::policy::constrain(&mut selected)?;
    validate_host_admission(&selected, &record.scope, &record.source_cwd)?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(id)?)?;
    set_private(&log_path(id)?, 0o600);
    let stderr = log.try_clone()?;
    let mut child = Command::new(std::env::current_exe()?);
    restrict_background_environment(&mut child, &selected);
    child
        .args([
            "--connection",
            selected.active_connection_id(),
            "--model",
            selected.active_model(),
            "--mode",
            "yolo",
            "--background-task",
            id,
        ])
        .current_dir(&record.run_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr))
        .env("AISHE_SHELL_ID", format!("task-{id}"))
        .env("AISHE_BACKGROUND_TASK_ID", id)
        .env("AISHE_TASK_ROLE", &record.role)
        .env("AISHE_TASK_SCOPE", &record.scope);
    set_budget_environment(&mut child, &record.budget);
    set_worker_engine_environment(&mut child, &record, &selected)?;
    detach_worker(&mut child);
    let child = child.spawn().context("starting workflow stage")?;
    let identity = process_start(child.id());
    update(id, |fresh| {
        if fresh.state == State::Starting {
            fresh.pid = Some(child.id());
            fresh.process_start = identity;
            fresh.state = State::Running;
        }
        Ok(())
    })
}

pub fn required_checks(id: &str) -> Result<Vec<String>> {
    Ok(load(id)?
        .workflow
        .map_or_else(Vec::new, |link| link.required_checks))
}

/// Move the saved native continuation into a detached owner. Provider state,
/// pending effects, budgets and authority are retained; no request is restarted.
pub fn adopt_saved_checkpoint(config: &Config, native_id: &str) -> Result<String> {
    adopt_saved_checkpoint_with_environment(config, native_id, std::collections::HashMap::new())
}

pub fn adopt_saved_checkpoint_with_environment(
    config: &Config,
    native_id: &str,
    environment: std::collections::HashMap<String, String>,
) -> Result<String> {
    let checkpoint = crate::tasks::load(native_id)?;
    crate::agent::native::handoff::wait_stopped(native_id, std::time::Duration::from_secs(5))?;
    if checkpoint.status != crate::tasks::Status::Interrupted
        || checkpoint.native_state.as_deref() != Some("handed_off")
    {
        anyhow::bail!("native task {native_id} has not parked for background handoff");
    }
    let existing = records()?
        .into_iter()
        .find(|record| record.native_task_id.as_deref() == Some(native_id));
    let id = if let Some(existing) = existing {
        update(&existing.id, |record| {
            if matches!(
                record.state,
                State::Cancelled | State::Applied | State::Discarded
            ) {
                anyhow::bail!("task {} cannot hand off from {:?}", record.id, record.state);
            }
            if record.foreground {
                settle_elapsed(record);
                record.state = State::Interrupted;
                record.pid = None;
                record.process_start = None;
                record.foreground = false;
            }
            Ok(())
        })?;
        existing.id
    } else {
        let selected = crate::tasks::restore_config(&checkpoint, config)?;
        let id = new_id();
        let dir = task_dir(&id)?;
        fs::create_dir_all(&dir)?;
        set_private(&dir, 0o700);
        let limits = checkpoint.execution_limits.clone().unwrap_or_default();
        let max_minutes = limits.elapsed.map_or(u32::MAX / 60, |value| {
            value.as_secs().div_ceil(60).clamp(1, u64::from(u32::MAX)) as u32
        });
        let workspace = checkpoint
            .workspace_root
            .clone()
            .unwrap_or_else(|| checkpoint.cwd.clone());
        let identity = git_identity(&workspace);
        let now = now_ms();
        let record = Record {
            schema_version: SCHEMA_VERSION,
            id: id.clone(),
            objective: checkpoint.objective.clone(),
            source_cwd: checkpoint.cwd.clone(),
            run_cwd: workspace,
            source_repo: identity.as_ref().map(|(repo, _, _)| repo.clone()),
            worktree: None,
            base_head: identity.as_ref().map(|(_, head, _)| head.clone()),
            source_branch: identity.and_then(|(_, _, branch)| branch),
            created_at_ms: now,
            updated_at_ms: now,
            state: State::Interrupted,
            result_revision: 0,
            mailbox: InteractionMailbox::default(),
            workflow: None,
            foreground: false,
            workflow_frozen: false,
            native_task_id: Some(native_id.into()),
            engine: Some("native".into()),
            connection_id: checkpoint.connection_id.clone(),
            provider: checkpoint.provider.clone(),
            model: checkpoint.model.clone(),
            connection: checkpoint.connection.clone(),
            role: "build".into(),
            scope: checkpoint.execution_scope.map_or_else(
                || selected.backend.default_scope.clone(),
                |scope| match scope {
                    crate::agent::ExecutionScope::Workspace => "workspace".into(),
                    crate::agent::ExecutionScope::Host => "host".into(),
                },
            ),
            network: checkpoint.network_policy.map_or_else(
                || selected.backend.workspace_network.clone(),
                |network| match network {
                    crate::agent::NetworkPolicy::Deny => "deny".into(),
                    crate::agent::NetworkPolicy::Allow => "allow".into(),
                },
            ),
            elapsed_ms: checkpoint.execution.elapsed_ms,
            attempt_started_at_ms: None,
            steering_revision: checkpoint.steering_revision,
            steering: Vec::new(),
            pid: None,
            process_start: None,
            exit_code: Some(75),
            budget: Budget {
                max_minutes,
                max_provider_turns: limits.provider_turns.unwrap_or(u32::MAX),
                max_cost_usd: limits.cost_usd.unwrap_or(0.0),
                max_tool_calls: limits.tool_calls.unwrap_or(u32::MAX),
                max_network_calls: limits.network_calls.unwrap_or(u32::MAX),
                max_changed_files: u32::MAX,
                max_changed_bytes: u64::MAX,
            },
            plan: Vec::new(),
            plan_revision: 0,
            applied_hunks: Vec::new(),
            applied_patch_sha256: None,
            budget_exceeded: false,
            error: Some("parked for background handoff".into()),
        };
        write_private(&request_path(&id)?, checkpoint.objective.as_bytes())?;
        save(&record)?;
        id
    };
    resume_with_environment(config, &id, Some(&environment), false)?;
    Ok(id)
}

/// Request the running native owner to park, then claim its exact checkpoint
/// for this terminal process. A stale callback cannot win after its worker exits.
pub fn claim_foreground(config: &Config, id: &str) -> Result<(Config, crate::tasks::Record)> {
    let mut record = load(id)?;
    reconcile(&mut record)?;
    if matches!(
        record.state,
        State::Blocked | State::Cancelled | State::Applied | State::Discarded | State::Completed
    ) {
        anyhow::bail!(
            "task {id} cannot move to foreground from {:?}",
            record.state
        );
    }
    if record.foreground {
        anyhow::bail!("task {id} already belongs to a foreground terminal");
    }
    if record.state == State::Waiting {
        interactions::ready_to_resume(&record)?;
    }
    let native_id = record
        .native_task_id
        .as_deref()
        .context("task has not saved a native checkpoint yet")?;
    if matches!(record.state, State::Starting | State::Running) {
        crate::agent::native::handoff::request_task(
            native_id,
            crate::agent::native::handoff::Direction::Foreground,
        )?;
    }
    crate::agent::native::handoff::wait_stopped(native_id, std::time::Duration::from_secs(5))?;
    let started = std::time::Instant::now();
    while record
        .pid
        .is_some_and(|pid| same_process(pid, record.process_start.as_deref()))
    {
        if started.elapsed() >= std::time::Duration::from_secs(5) {
            anyhow::bail!("foreground handoff is queued; retry after the worker exits");
        }
        std::thread::sleep(std::time::Duration::from_millis(40));
        record = load(id)?;
    }
    reconcile(&mut record)?;
    let checkpoint = resume_checkpoint(id)?.context("task has no durable native checkpoint")?;
    validate_remaining_budget(&record, &checkpoint)?;
    let mut selected = worker_config(id, config)?;
    crate::policy::constrain(&mut selected)?;
    workflows::claim_slot(id, || {
        update(id, |fresh| {
            if fresh.foreground
                || !matches!(
                    fresh.state,
                    State::Waiting | State::Interrupted | State::Failed
                )
                || fresh.native_task_id != record.native_task_id
                || fresh.result_revision != record.result_revision
                || fresh
                    .pid
                    .is_some_and(|pid| same_process(pid, fresh.process_start.as_deref()))
            {
                anyhow::bail!("task {id} changed while foreground handoff was pending");
            }
            if fresh.state == State::Waiting {
                interactions::ready_to_resume(fresh)?;
            }
            fresh.foreground = true;
            fresh.state = State::Running;
            fresh.pid = Some(std::process::id());
            fresh.process_start = process_start(std::process::id());
            fresh.attempt_started_at_ms = Some(now_ms());
            fresh.exit_code = None;
            fresh.error = None;
            Ok(())
        })
    })?;
    Ok((selected, checkpoint))
}

pub fn finish_foreground(id: &str, outcome: &crate::agent::NativeTurnOutcome) -> Result<u8> {
    let mut code = outcome.exit_code();
    update(id, |record| {
        if !record.foreground {
            return Ok(());
        }
        if record.pid != Some(std::process::id()) {
            anyhow::bail!("foreground task belongs to another terminal");
        }
        code = finish_native_record(record, outcome, crate::tasks::mark_background_cancelled)?;
        record.foreground = false;
        record.pid = None;
        record.process_start = None;
        Ok(())
    })?;
    refresh_status();
    if let Some(link) = load(id)?.workflow {
        let _ = scheduler::ensure_running(&link.run_id);
    }
    Ok(code)
}

fn review(_config: &Config, id: &str) -> Result<u8> {
    crate::cli::changeui::review_and_apply(id)?;
    Ok(0)
}

fn rework(config: &Config, id: &str, instructions: &str) -> Result<u8> {
    let instructions = instructions.trim();
    if instructions.is_empty() || instructions.len() > MAX_OBJECTIVE_BYTES / 2 {
        anyhow::bail!(
            "rework instructions must contain 1..={} bytes",
            MAX_OBJECTIVE_BYTES / 2
        );
    }
    update(id, |record| {
        if matches!(
            record.state,
            State::Running | State::Starting | State::Waiting | State::Applied | State::Discarded
        ) {
            anyhow::bail!("task {id} cannot be reworked from state {:?}", record.state);
        }
        changes::ensure_no_partial_apply(record)?;
        if record.workflow_frozen {
            anyhow::bail!(
                "workflow stage {id} was handed forward; run a new workflow to change it"
            );
        }
        let native_id = record
            .native_task_id
            .as_deref()
            .with_context(|| format!("task {id} has no durable native checkpoint to rework"))?;
        let _ = crate::agent::native::handoff::discard_environment(native_id);
        validate_remaining_budget(record, &crate::tasks::load(native_id)?)?;
        let total = record
            .steering
            .iter()
            .map(String::len)
            .sum::<usize>()
            .saturating_add(instructions.len());
        if total > MAX_OBJECTIVE_BYTES {
            anyhow::bail!("combined rework instructions exceed {MAX_OBJECTIVE_BYTES} bytes");
        }
        record.steering.push(crate::redact::redact(instructions));
        record.steering_revision = record.steering.len() as u32;
        record.state = State::Interrupted;
        record.error = None;
        Ok(())
    })?;
    resume(config, id)
}

fn apply(id: &str, hunks: &[usize]) -> Result<u8> {
    if !hunks.is_empty() {
        anyhow::bail!("--hunk requires --revision from task review");
    }
    let review = task_change_review(id)?;
    let applied = apply_task_changes(id, &review.revision, &[], hunks)?;
    println!("{}", serde_json::to_string_pretty(&applied)?);
    Ok(0)
}

fn discard(id: &str) -> Result<u8> {
    update(id, |record| {
        if matches!(
            record.state,
            State::Running | State::Starting | State::Waiting
        ) {
            anyhow::bail!("cancel task {id} before discarding it");
        }
        if let (Some(repo), Some(worktree)) = (&record.source_repo, &record.worktree) {
            let expected = task_dir(id)?.join("worktree");
            if worktree != &expected || !worktree.starts_with(task_root()?) {
                anyhow::bail!("refusing to remove an unowned task worktree");
            }
            let output = Command::new("git")
                .args([
                    "-C",
                    &repo.display().to_string(),
                    "worktree",
                    "remove",
                    "--force",
                ])
                .arg(worktree)
                .output()?;
            if !output.status.success() {
                anyhow::bail!(
                    "could not remove task worktree: {}",
                    crate::commands::display_safe(&String::from_utf8_lossy(&output.stderr))
                );
            }
        }
        record.state = State::Discarded;
        if let Some(native_id) = &record.native_task_id {
            let _ = crate::agent::native::handoff::discard_environment(native_id);
        }
        interactions::invalidate_pending(record);
        Ok(())
    })?;
    println!("discarded task {id} worktree");
    Ok(0)
}

fn set_plan(id: &str, steps: Vec<String>, preserve_completed: bool) -> Result<u8> {
    if steps.is_empty() || steps.len() > 100 {
        anyhow::bail!("a task plan needs 1..=100 steps");
    }
    update(id, |record| {
        record.plan = build_plan(&record.plan, &steps, preserve_completed);
        record.plan_revision = record.plan_revision.saturating_add(1);
        Ok(())
    })?;
    println!("updated task {id} plan");
    Ok(0)
}

fn build_plan(existing: &[PlanStep], steps: &[String], preserve_completed: bool) -> Vec<PlanStep> {
    let mut completed = existing
        .iter()
        .filter(|step| step.state == StepState::Completed)
        .map(|step| (step.text.clone(), step.evidence.clone()))
        .collect::<Vec<_>>();
    steps
        .iter()
        .enumerate()
        .map(|(index, text)| {
            let text = crate::redact::redact(text.trim());
            let preserved = preserve_completed
                .then(|| {
                    completed
                        .iter()
                        .position(|(old, _)| old == &text)
                        .map(|matched| completed.remove(matched).1)
                })
                .flatten();
            PlanStep {
                id: index as u32 + 1,
                text,
                state: if preserved.is_some() {
                    StepState::Completed
                } else {
                    StepState::Pending
                },
                evidence: preserved.flatten(),
            }
        })
        .collect()
}

fn set_step(id: &str, step: u32, state: StepState, evidence: Option<&str>) -> Result<u8> {
    update(id, |record| {
        let target = record
            .plan
            .iter_mut()
            .find(|value| value.id == step)
            .with_context(|| format!("task {id} has no plan step {step}"))?;
        target.state = state;
        if let Some(evidence) = evidence.map(str::trim).filter(|value| !value.is_empty()) {
            target.evidence = Some(crate::redact::redact(evidence));
        } else if state != StepState::Completed {
            target.evidence = None;
        }
        Ok(())
    })?;
    println!("task {id} step {step}: {state:?}");
    Ok(0)
}

fn changed_usage(record: &Record) -> Option<(u32, u64)> {
    let worktree = record.worktree.as_ref()?;
    let output = Command::new("git")
        .args([
            "-C",
            &worktree.display().to_string(),
            "ls-files",
            "-m",
            "-d",
            "-o",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|value| !value.is_empty());
    let mut files = 0u32;
    let mut bytes = 0u64;
    for line in paths {
        let relative = std::str::from_utf8(line).ok()?;
        if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
            return None;
        }
        files = files.saturating_add(1);
        bytes = bytes.saturating_add(
            std::fs::metadata(worktree.join(relative))
                .map(|m| m.len())
                .unwrap_or(0),
        );
    }
    Some((files, bytes))
}

/// Background agents inherit only runtime plumbing plus secret variables that
/// are explicitly referenced by the active configuration. Tool subprocesses
/// strip those names again unless an MCP definition deliberately consumes one.
fn restrict_background_environment(child: &mut Command, config: &Config) {
    let native = crate::lean::enabled() || config.backend.engine == "native";
    restrict_background_environment_from(child, config, std::env::vars_os(), native);
}

fn restrict_background_environment_from(
    child: &mut Command,
    config: &Config,
    environment: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    native: bool,
) {
    use std::collections::BTreeSet;

    child.env_clear();
    let mut names: BTreeSet<&str> = [
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "TMPDIR",
        "TMP",
        "TEMP",
        "LANG",
        "LC_ALL",
        "TERM",
        "NO_COLOR",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "AISHE_CONFIG_DIR",
        "AISHE_DATA_DIR",
        "AISHE_CREDENTIALS_FILE",
        "AISHE_POLICY_FILE",
        "AISHE_TASKS_DIR",
        "AISHE_LEGACY_OPENCODE",
        "AISHE_LEAN",
        "AISHE_CONNECTION",
        "AISHE_MODEL",
        "AISHE_REASONING",
        "AISHE_SCOPE",
        "AISHE_NETWORK",
        "AISHE_AGENT_OUTPUT",
        // Deterministic test providers are intentionally scoped like runtime
        // plumbing and contain no production credentials.
        "AISHE_FAKE_LLM",
        "AISHE_FAKE_TOOL",
        "AISHE_FAKE_USAGE",
    ]
    .into_iter()
    .collect();
    names.insert(&config.providers.anthropic.api_key_env);
    names.insert(&config.providers.openai.api_key_env);
    for connection in config.connections.values() {
        names.insert(&connection.settings.api_key_env);
        if let crate::config::ConnectionAuth::ApiKey {
            api_key_env: Some(name),
            ..
        } = &connection.auth
        {
            names.insert(name);
        }
    }
    for server in config.mcp_servers.values() {
        for reference in server.env.values().chain(server.headers.values()) {
            if let Some(name) = reference.strip_prefix("env:") {
                names.insert(name);
            }
        }
    }
    let denied = crate::executor::sensitive_environment_names(config);
    for (name, value) in environment {
        let Some(text) = name.to_str() else { continue };
        let upper = text.to_ascii_uppercase();
        if upper.starts_with("LD_")
            || upper.starts_with("DYLD_")
            || matches!(
                upper.as_str(),
                "ENV" | "BASH_ENV" | "SHELLOPTS" | "BASHOPTS" | "ZDOTDIR"
            )
        {
            continue;
        }
        if names.contains(text)
            || (native && crate::executor::agent_environment_allowed(text, &denied))
        {
            child.env(name, value);
        }
    }
}

fn set_budget_environment(child: &mut Command, budget: &Budget) {
    child
        .env("AISHE_TASK_MAX_MINUTES", budget.max_minutes.to_string())
        .env(
            "AISHE_TASK_MAX_PROVIDER_TURNS",
            budget.max_provider_turns.to_string(),
        )
        .env("AISHE_TASK_MAX_COST_USD", budget.max_cost_usd.to_string())
        .env(
            "AISHE_TASK_MAX_TOOL_CALLS",
            budget.max_tool_calls.to_string(),
        )
        .env(
            "AISHE_TASK_MAX_NETWORK_CALLS",
            budget.max_network_calls.to_string(),
        );
}

fn actual_engine(config: &Config, lean_enabled: bool) -> &'static str {
    if lean_enabled || config.backend.engine == "native" {
        "native"
    } else {
        "opencode"
    }
}

fn selected_engine(record: &Record, current: &Config, lean_enabled: bool) -> Result<&'static str> {
    // A linked checkpoint belongs to the native runtime, including records
    // created before engine choice became an explicit durable field.
    if record.native_task_id.is_some() {
        return Ok("native");
    }
    match record.engine.as_deref() {
        Some("native") => Ok("native"),
        Some("opencode") => Ok("opencode"),
        Some(_) => anyhow::bail!("background task {} has an invalid saved engine", record.id),
        None => Ok(actual_engine(current, lean_enabled)),
    }
}

fn set_worker_engine_environment(
    child: &mut Command,
    record: &Record,
    current: &Config,
) -> Result<()> {
    let native = selected_engine(record, current, crate::lean::enabled())? == "native";
    child
        .env("AISHE_LEAN", if native { "1" } else { "0" })
        .env("AISHE_LEGACY_OPENCODE", if native { "0" } else { "1" });
    Ok(())
}

fn validate_host_admission(config: &Config, scope: &str, source: &Path) -> Result<()> {
    match crate::agent::ExecutionScope::parse(scope) {
        Some(crate::agent::ExecutionScope::Host) => {
            if !config.sandbox.allow_host_yolo {
                anyhow::bail!("agent host scope is disabled by policy");
            }
            // Check the original source branch and caller's cloud/Kubernetes
            // environment before a detached worktree or filtered environment
            // can hide the protected target.
            crate::environment::confirm_protected_host(config, source)
        }
        Some(crate::agent::ExecutionScope::Workspace) => Ok(()),
        None => anyhow::bail!("task execution scope must be workspace or host"),
    }
}

fn validate_remaining_budget(record: &Record, checkpoint: &crate::tasks::Record) -> Result<()> {
    let used = checkpoint.execution;
    let reason = if used.provider_turns >= record.budget.max_provider_turns {
        Some("provider-turn")
    } else if record.budget.max_cost_usd > 0.0 && !used.cost_is_complete() {
        anyhow::bail!("task {} cost coverage is incomplete; resume cannot verify its remaining cost allowance", record.id);
    } else if record.budget.max_cost_usd > 0.0 && used.cost_usd >= record.budget.max_cost_usd {
        Some("cost")
    } else if used.elapsed_ms.max(elapsed(record)) >= u64::from(record.budget.max_minutes) * 60_000
    {
        Some("wall-clock")
    } else {
        None
    };
    if let Some(reason) = reason {
        anyhow::bail!(
            "task {} {reason} budget is exhausted; resume cannot reset spent allowances",
            record.id
        );
    }
    if record.budget_exceeded {
        anyhow::bail!(
            "task {} exceeded its change budget; review or discard it",
            record.id
        );
    }
    Ok(())
}

fn validate_native_workspace(record: &Record, checkpoint: &crate::tasks::Record) -> Result<()> {
    let expected_root = &record.run_cwd;
    let actual_root = checkpoint
        .workspace_root
        .as_deref()
        .unwrap_or(&checkpoint.cwd);
    if actual_root != expected_root {
        anyhow::bail!(
            "native checkpoint workspace does not match background task {}",
            record.id
        );
    }
    if checkpoint.execution_scope != Some(crate::agent::ExecutionScope::Host)
        && !checkpoint.cwd.starts_with(actual_root)
    {
        anyhow::bail!("native checkpoint cwd is outside its workspace");
    }
    Ok(())
}

fn git_identity(cwd: &Path) -> Option<(PathBuf, String, Option<String>)> {
    let repo = git_text(cwd, &["rev-parse", "--show-toplevel"])?;
    let repo = PathBuf::from(repo).canonicalize().ok()?;
    let head = git_text(&repo, &["rev-parse", "HEAD"])?;
    let branch = git_text(&repo, &["symbolic-ref", "--short", "-q", "HEAD"]);
    Some((repo, head, branch))
}

fn git_text(cwd: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(cwd);
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(name);
    }
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_CONFIG_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let output = changes::command_output(
        &mut command,
        None,
        64 * 1024,
        false,
        std::time::Instant::now() + std::time::Duration::from_secs(3),
    )
    .ok()?;
    Some(crate::commands::display_safe(
        String::from_utf8_lossy(&output).trim(),
    ))
}

fn branch_label(record: &Record) -> String {
    match (&record.source_branch, &record.base_head) {
        (Some(branch), _) => branch.clone(),
        (None, Some(head)) => format!("detached {}", &head[..head.len().min(12)]),
        _ => "non-git".into(),
    }
}

fn reconcile(record: &mut Record) -> Result<()> {
    if !matches!(record.state, State::Starting | State::Running) {
        return Ok(());
    }
    if record.state == State::Starting
        && record.pid.is_none()
        && now_ms().saturating_sub(record.updated_at_ms) < 5_000
    {
        return Ok(());
    }
    let live = record
        .pid
        .is_some_and(|pid| same_process(pid, record.process_start.as_deref()));
    if live {
        return Ok(());
    }
    // Process looks dead relative to the caller's snapshot. Take the record lock
    // and reload before writing Interrupted so a concurrent finish() checkpoint
    // (Completed/Failed) is never overwritten — a TOCTOU that flakes under fast
    // FAKE_LLM workers plus `task show` polling on Linux CI.
    let id = record.id.clone();
    let lock_path = task_dir(&id)?.join("record.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    set_private(&lock_path, 0o600);
    lock.lock_exclusive()?;
    let mut fresh = load(&id)?;
    if matches!(fresh.state, State::Starting | State::Running) {
        let still_live = fresh
            .pid
            .is_some_and(|pid| same_process(pid, fresh.process_start.as_deref()));
        if !still_live {
            settle_elapsed(&mut fresh);
            let paused = fresh
                .native_task_id
                .as_deref()
                .and_then(|native_id| crate::tasks::load(native_id).ok())
                .is_some_and(|checkpoint| checkpoint.native_state.as_deref() == Some("waiting"));
            fresh.state = if paused
                && fresh.mailbox.requests.iter().any(|request| {
                    matches!(
                        request.status,
                        InteractionStatus::Pending | InteractionStatus::Responded
                    )
                }) {
                State::Waiting
            } else {
                State::Interrupted
            };
            fresh.pid = None;
            fresh.process_start = None;
            fresh.updated_at_ms = now_ms().max(fresh.updated_at_ms.saturating_add(1));
            fresh.error = (fresh.state != State::Waiting)
                .then(|| "background process ended without a final checkpoint".into());
            save(&fresh)?;
            fresh = load(&id)?;
        }
    }
    *record = fresh;
    Ok(())
}

fn process_start(pid: u32) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        // Kernel start ticks identify PID reuse without spawning ps, and remain
        // stable across shell locale/time-zone changes.
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let start = stat.rsplit_once(") ")?.1.split_ascii_whitespace().nth(19)?;
        start.parse::<u64>().ok()?;
        Some(format!("proc:{start}"))
    }
    #[cfg(not(target_os = "linux"))]
    {
        ps_process_start(pid, true).map(|start| format!("utc:{start}"))
    }
}

fn ps_process_start(pid: u32, utc: bool) -> Option<String> {
    let mut command = Command::new("ps");
    if utc {
        command.env("TZ", "UTC").env("LC_ALL", "C");
    }
    let output = command
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn same_process(pid: u32, expected: Option<&str>) -> bool {
    expected.is_some_and(|expected| {
        if expected.starts_with("proc:") || expected.starts_with("utc:") {
            process_start(pid).as_deref() == Some(expected)
        } else {
            // Older records used unprefixed ps output in the caller's locale.
            ps_process_start(pid, false).as_deref() == Some(expected)
        }
    }) && process_running(pid)
}

fn process_running(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .and_then(|stat| {
                stat.rsplit_once(") ")
                    .map(|(_, fields)| fields.as_bytes()[0])
            })
            .is_some_and(|state| !matches!(state, b'Z' | b'X'))
    }
    #[cfg(not(target_os = "linux"))]
    {
        Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| !output.stdout.is_empty() && !output.stdout.contains(&b'Z'))
    }
}

fn records() -> Result<Vec<Record>> {
    let root = task_root()?;
    let mut records = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return Ok(records);
    };
    for entry in entries.flatten() {
        let path = entry.path().join("record.json");
        if let Ok(record) = load_path(&path) {
            records.push(record);
        }
    }
    records.sort_by_key(|record| record.updated_at_ms);
    Ok(records)
}

pub(crate) fn load(id: &str) -> Result<Record> {
    validate_id(id)?;
    load_path(&record_path(id)?)
}

fn load_path(path: &Path) -> Result<Record> {
    let record: Record = serde_json::from_slice(&fs::read(path)?)?;
    if record.schema_version != SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported background task schema {}",
            record.schema_version
        );
    }
    Ok(record)
}

fn save(record: &Record) -> Result<()> {
    validate_id(&record.id)?;
    let path = record_path(&record.id)?;
    let mut stored = record.clone();
    stored.mailbox.validate_size()?;
    if let Ok(previous) = load_path(&path) {
        advance_result_revision(&mut stored, &previous);
    }
    crate::config::write_atomic(&path, &serde_json::to_vec_pretty(&stored)?)?;
    set_private(&path, 0o600);
    // Presentation failure must never prevent the authoritative task checkpoint.
    let _ = presentation::record_saved(&stored);
    Ok(())
}

fn advance_result_revision(stored: &mut Record, previous: &Record) {
    stored.result_revision = previous.result_revision.max(stored.result_revision);
    if !matches!(
        stored.state,
        State::Starting | State::Running | State::Waiting
    ) && (previous.state != stored.state
        || previous.exit_code != stored.exit_code
        || previous.error != stored.error
        || previous.budget_exceeded != stored.budget_exceeded
        || previous.native_task_id != stored.native_task_id)
    {
        stored.result_revision = stored.result_revision.saturating_add(1);
    }
}

fn update(id: &str, change: impl FnOnce(&mut Record) -> Result<()>) -> Result<()> {
    let lock_path = task_dir(id)?.join("record.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    set_private(&lock_path, 0o600);
    lock.lock_exclusive()?;
    let mut record = load(id)?;
    change(&mut record)?;
    // Revision stamps remain distinct even when several actions finish in the
    // same millisecond or the wall clock moves backward.
    record.updated_at_ms = now_ms().max(record.updated_at_ms.saturating_add(1));
    save(&record)
}

fn task_root() -> Result<PathBuf> {
    let root = crate::config::data_root()
        .context("no data directory is available")?
        .join("aishe")
        .join("background-tasks");
    fs::create_dir_all(&root)?;
    set_private(&root, 0o700);
    Ok(root)
}

fn task_dir(id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(task_root()?.join(id))
}

fn record_path(id: &str) -> Result<PathBuf> {
    Ok(task_dir(id)?.join("record.json"))
}

fn request_path(id: &str) -> Result<PathBuf> {
    Ok(task_dir(id)?.join("request.txt"))
}

fn log_path(id: &str) -> Result<PathBuf> {
    Ok(task_dir(id)?.join("activity.log"))
}

fn validate_id(id: &str) -> Result<()> {
    if !(8..=80).contains(&id.len())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(crate::user_error::UserFacing::cli(
            "unknown_task",
            format!(
                "No task or session '{}'.",
                crate::commands::display_safe(id)
            ),
            "Run `aishe sessions` to list what can be resumed.",
        ));
    }
    Ok(())
}

fn new_id() -> String {
    use rand::RngCore;
    let mut random = [0u8; 8];
    rand::rng().fill_bytes(&mut random);
    format!(
        "{:x}-{}",
        now_ms(),
        random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::config::write_atomic(path, bytes)?;
    set_private(path, 0o600);
    Ok(())
}

#[cfg(unix)]
fn set_private(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_private(_path: &Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_ids_reject_path_escape() {
        assert!(validate_id("12345678-abcd").is_ok());
        assert!(validate_id("../record").is_err());
        assert!(validate_id("short").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn worker_identity_tracks_live_process_and_rejects_reused_or_exited_pid() {
        let mut child = Command::new("sleep")
            .arg("10")
            .env("TZ", "Pacific/Honolulu")
            .spawn()
            .unwrap();
        let pid = child.id();
        let identity = process_start(pid).unwrap();
        let legacy = ps_process_start(pid, false).unwrap();
        let alive = same_process(pid, Some(&identity));
        let legacy_alive = same_process(pid, Some(&legacy));
        let mismatch = same_process(pid, Some("proc:0"));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(alive);
        assert!(legacy_alive, "legacy ps fixture remains recognized");
        assert!(!mismatch);
        assert!(!same_process(pid, Some(&identity)));
        #[cfg(target_os = "linux")]
        assert!(identity.starts_with("proc:"));
        #[cfg(not(target_os = "linux"))]
        assert!(identity.starts_with("utc:"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn kernel_worker_identity_is_independent_of_ps_time_zone_display() {
        let pid = std::process::id();
        let identity = process_start(pid).unwrap();
        let ps_time = |timezone| {
            let output = Command::new("ps")
                .args(["-o", "lstart=", "-p", &pid.to_string()])
                .env("TZ", timezone)
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            assert!(output.status.success());
            String::from_utf8(output.stdout).unwrap()
        };
        assert_ne!(ps_time("UTC"), ps_time("Pacific/Honolulu"));
        assert_eq!(process_start(pid).as_deref(), Some(identity.as_str()));
        assert!(same_process(pid, Some(&identity)));
        let ticks = identity
            .strip_prefix("proc:")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        assert!(!same_process(pid, Some(&format!("proc:{}", ticks + 1))));
    }

    pub(super) fn fixture_record() -> Record {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "id": "12345678-abcd",
            "objective": "x",
            "source_cwd": "/tmp",
            "run_cwd": "/tmp",
            "source_repo": null,
            "worktree": null,
            "base_head": null,
            "source_branch": null,
            "created_at_ms": 0,
            "updated_at_ms": 0,
            "state": "completed",
            "pid": null,
            "process_start": null,
            "exit_code": 0,
            "budget": {"max_minutes": 30, "max_provider_turns": 20, "max_cost_usd": 0.0,
                "max_tool_calls": 200, "max_changed_files": 100, "max_changed_bytes": 10485760,
                "max_network_calls": 50},
            "plan": [],
            "plan_revision": 0,
            "error": null
            ,"budget_exceeded": false
        }))
        .unwrap()
    }

    #[test]
    fn branch_labels_are_explicit() {
        let mut record = fixture_record();
        assert_eq!(branch_label(&record), "non-git");
        record.source_branch = Some("main".into());
        assert_eq!(branch_label(&record), "main");
    }

    #[test]
    fn old_records_remain_readable_without_inventing_a_checkpoint() {
        let record = fixture_record();
        assert_eq!(record.schema_version, SCHEMA_VERSION);
        assert!(record.native_task_id.is_none());
        assert!(record.connection.is_none());
        assert_eq!(record.elapsed_ms, 0);
        assert!(record.attempt_started_at_ms.is_none());
        assert_eq!(record.steering_revision, 0);
        assert!(record.engine.is_none());
    }

    #[test]
    fn native_checkpoint_keeps_its_engine_after_defaults_and_hatches_change() {
        let mut record = fixture_record();
        record.native_task_id = Some("native-checkpoint".into());
        let mut current = Config::default();
        current.backend.engine = "opencode".into();
        assert_eq!(selected_engine(&record, &current, false).unwrap(), "native");
        record.engine = Some("opencode".into());
        assert_eq!(selected_engine(&record, &current, false).unwrap(), "native");
        let mut child = Command::new("ignored");
        child
            .env("AISHE_LEAN", "0")
            .env("AISHE_LEGACY_OPENCODE", "1");
        set_worker_engine_environment(&mut child, &record, &current).unwrap();
        let env: std::collections::BTreeMap<_, _> = child.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("AISHE_LEAN"))
                .copied()
                .flatten(),
            Some(std::ffi::OsStr::new("1"))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("AISHE_LEGACY_OPENCODE"))
                .copied()
                .flatten(),
            Some(std::ffi::OsStr::new("0"))
        );
    }

    #[test]
    fn fresh_workers_preserve_the_runtime_selected_at_task_start() {
        let mut current = Config::default();
        current.backend.engine = "opencode".into();
        assert_eq!(actual_engine(&current, true), "native");
        assert_eq!(actual_engine(&current, false), "opencode");
        current.backend.engine = "native".into();
        assert_eq!(actual_engine(&current, false), "native");
        let mut record = fixture_record();
        record.engine = Some("opencode".into());
        assert_eq!(
            selected_engine(&record, &current, true).unwrap(),
            "opencode"
        );
        let mut child = Command::new("ignored");
        child
            .env("AISHE_LEAN", "1")
            .env("AISHE_LEGACY_OPENCODE", "0");
        set_worker_engine_environment(&mut child, &record, &current).unwrap();
        let env: std::collections::BTreeMap<_, _> = child.get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("AISHE_LEAN"))
                .copied()
                .flatten(),
            Some(std::ffi::OsStr::new("0"))
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("AISHE_LEGACY_OPENCODE"))
                .copied()
                .flatten(),
            Some(std::ffi::OsStr::new("1"))
        );
    }

    #[test]
    fn older_worker_engine_fallback_stays_valid_and_invalid_saved_values_fail_closed() {
        let mut record = fixture_record();
        let mut current = Config::default();
        current.backend.engine = "opencode".into();
        assert_eq!(
            selected_engine(&record, &current, false).unwrap(),
            "opencode"
        );
        assert_eq!(selected_engine(&record, &current, true).unwrap(), "native");
        current.backend.engine = "native".into();
        assert_eq!(selected_engine(&record, &current, false).unwrap(), "native");
        record.engine = Some("other-runtime".into());
        assert!(selected_engine(&record, &current, true).is_err());
    }

    #[test]
    fn native_worker_preserves_project_environment_but_rejects_startup_controls() {
        let mut config = Config::default();
        config.providers.openai.api_key_env = "CUSTOM_ACCESS".into();
        config.providers.anthropic.api_key_env = "ENV".into();
        let environment = || {
            [
                ("PROJECT_TARGET", "development"),
                ("VIRTUAL_ENV", "/tmp/project-venv"),
                ("PATH", "/tmp/project-venv/bin:/usr/bin"),
                ("CUSTOM_ACCESS", "provider-only"),
                ("OTHER_SECRET", "must-not-inherit"),
                ("LD_PRELOAD", "must-not-load"),
                ("ENV", "must-not-source"),
                ("AISHE_TASK_MAX_TOOL_CALLS", "999999"),
            ]
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
        };
        let mut child = Command::new("ignored");
        restrict_background_environment_from(&mut child, &config, environment(), true);
        let env: std::collections::BTreeMap<_, _> = child
            .get_envs()
            .filter_map(|(name, value)| {
                value.map(|value| {
                    (
                        name.to_string_lossy().into_owned(),
                        value.to_string_lossy().into_owned(),
                    )
                })
            })
            .collect();
        assert_eq!(
            env.get("PROJECT_TARGET").map(String::as_str),
            Some("development")
        );
        assert_eq!(
            env.get("VIRTUAL_ENV").map(String::as_str),
            Some("/tmp/project-venv")
        );
        assert_eq!(
            env.get("CUSTOM_ACCESS").map(String::as_str),
            Some("provider-only")
        );
        for name in [
            "OTHER_SECRET",
            "LD_PRELOAD",
            "ENV",
            "AISHE_TASK_MAX_TOOL_CALLS",
        ] {
            assert!(!env.contains_key(name), "worker inherited {name}");
        }
        let mut legacy = Command::new("ignored");
        restrict_background_environment_from(&mut legacy, &config, environment(), false);
        assert!(!legacy
            .get_envs()
            .any(|(name, _)| name == "PROJECT_TARGET" || name == "VIRTUAL_ENV"));
    }

    #[test]
    fn checkpoint_cwd_can_change_without_changing_the_workspace_root() {
        let record = fixture_record();
        let mut checkpoint =
            crate::tasks::Active::start(&Config::default(), Path::new("/tmp"), "cd")
                .record()
                .clone();
        checkpoint.execution_scope = Some(crate::agent::ExecutionScope::Workspace);
        checkpoint.workspace_root = Some(PathBuf::from("/tmp"));
        checkpoint.cwd = PathBuf::from("/tmp/project/subdir");
        assert!(validate_native_workspace(&record, &checkpoint).is_ok());
        checkpoint.cwd = PathBuf::from("/outside");
        assert!(validate_native_workspace(&record, &checkpoint).is_err());
        checkpoint.execution_scope = Some(crate::agent::ExecutionScope::Host);
        assert!(validate_native_workspace(&record, &checkpoint).is_ok());
        checkpoint.workspace_root = Some(PathBuf::from("/outside"));
        assert!(validate_native_workspace(&record, &checkpoint).is_err());
    }

    #[test]
    fn native_stops_never_publish_completed_background_state() {
        use crate::agent::{NativeTurnOutcome, NativeTurnState};
        for (state, expected, exit_code) in [
            (NativeTurnState::Cancelled, State::Cancelled, 130),
            (NativeTurnState::BudgetExhausted, State::Failed, 124),
            (NativeTurnState::IterationLimit, State::Failed, 75),
            (NativeTurnState::Failed, State::Failed, 1),
            (NativeTurnState::Declined, State::Interrupted, 2),
        ] {
            let outcome = NativeTurnOutcome::new("native-task", state, Some("stop reason".into()));
            let mut record = fixture_record();
            record.state = State::Running;
            record.pid = Some(123);
            finish_record(
                &mut record,
                native_finish_state(outcome.state),
                i32::from(outcome.exit_code()),
                outcome.detail,
            );
            assert_eq!(record.state, expected);
            assert_eq!(record.exit_code, Some(exit_code));
            assert!(record.pid.is_none());
            assert_eq!(record.error.as_deref(), Some("stop reason"));
        }
    }

    #[test]
    fn waiting_stops_active_time_without_losing_the_departing_worker_identity() {
        let mut record = fixture_record();
        record.state = State::Running;
        record.pid = Some(4242);
        record.process_start = Some("worker-start".into());
        record.elapsed_ms = 100;
        record.attempt_started_at_ms = Some(now_ms().saturating_sub(25));
        finish_record(&mut record, State::Waiting, 75, Some("Needs you".into()));
        assert_eq!(
            native_finish_state(crate::agent::NativeTurnState::Waiting),
            State::Waiting
        );
        assert_eq!(record.state, State::Waiting);
        assert!(record.elapsed_ms >= 125);
        assert!(record.attempt_started_at_ms.is_none());
        assert_eq!(elapsed(&record), record.elapsed_ms);
        assert_eq!(record.pid, Some(4242));
        assert_eq!(record.process_start.as_deref(), Some("worker-start"));
    }

    #[test]
    fn completion_racing_an_accepted_followup_leaves_a_visible_resumable_result() {
        let mut record = fixture_record();
        record.state = State::Running;
        record.mailbox.followups.push(
            serde_json::from_value(serde_json::json!({
                "revision": 1, "text": "also check the cache", "status": "queued",
                "created_at_ms": 1, "updated_at_ms": 1,
            }))
            .unwrap(),
        );
        finish_record(&mut record, State::Completed, 0, None);
        assert_eq!(record.state, State::Interrupted);
        assert_eq!(record.exit_code, Some(75));
        assert!(record
            .error
            .as_deref()
            .unwrap()
            .contains("resume to deliver"));
        assert_eq!(record.mailbox.followups[0].status, FollowupStatus::Queued);
    }

    #[test]
    fn result_generations_ignore_mailbox_updates_but_distinguish_repeated_completion() {
        let mut previous = fixture_record();
        previous.result_revision = 5;
        let mut updated = previous.clone();
        updated.updated_at_ms += 10;
        updated.plan_revision += 1;
        updated.mailbox.next_followup_revision += 1;
        advance_result_revision(&mut updated, &previous);
        assert_eq!(updated.result_revision, 5);
        let mut running = updated.clone();
        running.state = State::Running;
        advance_result_revision(&mut running, &updated);
        assert_eq!(running.result_revision, 5);
        let mut completed = running.clone();
        completed.state = State::Completed;
        advance_result_revision(&mut completed, &running);
        assert_eq!(completed.result_revision, 6);
    }

    #[test]
    fn delayed_finish_preserves_cancellation_and_review_outcomes() {
        for state in [
            State::Cancelled,
            State::Completed,
            State::Failed,
            State::Applied,
            State::Discarded,
        ] {
            let mut record = fixture_record();
            record.state = state;
            record.exit_code = Some(130);
            record.error = Some("original outcome".into());
            finish_record(&mut record, State::Completed, 0, None);
            assert_eq!(record.state, state);
            assert_eq!(record.exit_code, Some(130));
            assert_eq!(record.error.as_deref(), Some("original outcome"));
        }
    }

    #[test]
    fn cancelled_background_reconciles_a_late_native_completion_without_losing_work() {
        let mut record = fixture_record();
        record.state = State::Cancelled;
        record.exit_code = Some(130);
        let mut checkpoint =
            crate::tasks::Active::start(&Config::default(), Path::new("/tmp"), "preserve")
                .record()
                .clone();
        checkpoint.status = crate::tasks::Status::Completed;
        checkpoint.native_state = Some("completed".into());
        checkpoint.messages = vec![
            crate::providers::Msg::User("original objective".into()),
            crate::providers::Msg::ToolResult {
                call_id: "finished-call".into(),
                content: "verified evidence".into(),
            },
        ];
        checkpoint.execution = crate::tasks::ExecutionCounters {
            provider_turns: 3,
            tool_calls: 2,
            network_calls: 1,
            elapsed_ms: 1234,
            cost_usd: 0.5,
            costed_provider_turns: Some(3),
        };
        checkpoint.usage = crate::tasks::UsageSummary {
            input: 100,
            output: 200,
            requests: 3,
        };
        let messages = serde_json::to_value(&checkpoint.messages).unwrap();
        let counters = checkpoint.execution;
        record.native_task_id = Some(checkpoint.id.clone());
        let outcome =
            crate::agent::NativeTurnOutcome::completed(&checkpoint.id, Some("done".into()));
        let code = finish_native_record(&mut record, &outcome, |native_id| {
            assert_eq!(native_id, checkpoint.id);
            crate::tasks::reconcile_background_cancellation(&mut checkpoint);
            Ok(())
        })
        .unwrap();
        assert_eq!(code, 130);
        assert_eq!(record.state, State::Cancelled);
        assert_eq!(record.exit_code, Some(130));
        assert_eq!(checkpoint.status, crate::tasks::Status::Interrupted);
        assert_eq!(checkpoint.native_state.as_deref(), Some("cancelled"));
        assert_eq!(
            serde_json::to_value(&checkpoint.messages).unwrap(),
            messages
        );
        assert_eq!(checkpoint.execution, counters);
        assert_eq!(checkpoint.usage.requests, 3);
    }

    #[test]
    fn paused_time_does_not_spend_budget_but_live_attempt_time_does() {
        let mut record = fixture_record();
        record.elapsed_ms = 250;
        assert_eq!(elapsed(&record), 250);
        record.attempt_started_at_ms = Some(now_ms().saturating_sub(100));
        assert!(elapsed(&record) >= 350);
        settle_elapsed(&mut record);
        assert!(record.elapsed_ms >= 350);
        assert!(record.attempt_started_at_ms.is_none());
        assert_eq!(elapsed(&record), record.elapsed_ms);
    }

    #[test]
    fn continuation_cannot_replenish_provider_cost_or_time_allowances() {
        let mut record = fixture_record();
        let mut checkpoint =
            crate::tasks::Active::start(&Config::default(), Path::new("/tmp"), "bounded")
                .record()
                .clone();
        checkpoint.execution.provider_turns = record.budget.max_provider_turns;
        assert!(validate_remaining_budget(&record, &checkpoint)
            .unwrap_err()
            .to_string()
            .contains("provider-turn"));
        checkpoint.execution.provider_turns = 0;
        record.budget.max_cost_usd = 0.1;
        checkpoint.execution.cost_usd = 0.1;
        assert!(validate_remaining_budget(&record, &checkpoint)
            .unwrap_err()
            .to_string()
            .contains("cost"));
        checkpoint.execution.cost_usd = 0.0;
        record.elapsed_ms = u64::from(record.budget.max_minutes) * 60_000;
        assert!(validate_remaining_budget(&record, &checkpoint)
            .unwrap_err()
            .to_string()
            .contains("wall-clock"));
        record.elapsed_ms = 0;
        checkpoint.execution.tool_calls = record.budget.max_tool_calls;
        checkpoint.execution.network_calls = record.budget.max_network_calls;
        // A final model response remains possible; admitting another tool is
        // independently denied by the native engine's persisted reservations.
        assert!(validate_remaining_budget(&record, &checkpoint).is_ok());
    }

    #[test]
    fn replan_preserves_each_identical_completed_step_once() {
        let existing = vec![PlanStep {
            id: 1,
            text: "test".into(),
            state: StepState::Completed,
            evidence: Some("587 passed".into()),
        }];
        let plan = build_plan(
            &existing,
            &["test".into(), "test".into(), "ship".into()],
            true,
        );
        assert_eq!(plan[0].state, StepState::Completed);
        assert_eq!(plan[0].evidence.as_deref(), Some("587 passed"));
        assert_eq!(plan[1].state, StepState::Pending);
        assert_eq!(plan[1].evidence, None);
        assert_eq!(plan[2].id, 3);
    }
}
