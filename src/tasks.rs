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
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExecutionCounters {
    pub provider_turns: u32,
    pub tool_calls: u32,
    pub network_calls: u32,
    pub elapsed_ms: u64,
    pub cost_usd: f64,
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
    #[serde(default)]
    pub execution: ExecutionCounters,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_limits: Option<crate::agent::native::NativeLimits>,
    #[serde(default)]
    pub steering_revision: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_kind: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
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
        active.record.updated_at_ms = now_ms();
        active.save();
        active
    }

    pub fn id(&self) -> &str {
        &self.record.id
    }

    pub fn record(&self) -> &Record {
        &self.record
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
        self.record.execution = counters;
        self.save();
    }

    pub fn checkpoint_limits(&mut self, limits: crate::agent::native::NativeLimits) {
        self.record.execution_limits = Some(limits);
        self.save();
    }

    pub fn checkpoint_admission(&mut self, executor: &crate::executor::Executor) {
        if let Some((scope, root, network)) = executor.lean_scope() {
            self.record.execution_scope = Some(*scope);
            self.record.network_policy = Some(*network);
            self.record.workspace_root = Some(root.clone());
            self.save();
        }
    }

    pub fn checkpoint_cwd(&mut self, cwd: &Path) {
        self.record.cwd = cwd.to_path_buf();
        self.save();
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
        };
        self.record.status = status;
        self.record.native_state = Some(reason.into());
        self.record.last_error = outcome.detail.as_deref().map(crate::redact::redact);
        if status == Status::Completed {
            self.record.pending_tool = None;
            self.record.last_error_kind = None;
        }
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
        });
        self.record.usage = self.cumulative_usage(usage);
        self.record.updated_at_ms = now_ms();
        self.save();
    }

    pub fn mark_pending_started(&mut self) {
        if let Some(pending) = self.record.pending_tool.as_mut() {
            pending.may_have_started = true;
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
        self.record.pending_tool = None;
        self.checkpoint_messages(messages, usage);
    }

    pub fn clear_pending_with_result(&mut self, result: &str) -> Option<Msg> {
        let pending = self.record.pending_tool.take()?;
        let message = Msg::ToolResult {
            call_id: pending.call.id.clone(),
            content: crate::redact::redact(result),
        };
        self.record.messages.push(message.clone());
        self.record.updated_at_ms = now_ms();
        self.save();
        Some(message)
    }

    pub fn interrupted(&mut self, messages: &[Msg], usage: Usage) {
        self.record.status = Status::Interrupted;
        self.checkpoint_messages(messages, usage);
    }

    pub fn failed(&mut self, messages: &[Msg], usage: Usage, kind: ErrorKind, error: &str) {
        self.record.status = Status::Failed;
        self.record.last_error_kind = Some(kind);
        self.record.last_error = Some(crate::redact::redact(error));
        self.checkpoint_messages(messages, usage);
    }

    pub fn completed(&mut self, messages: &[Msg], usage: Usage) {
        self.record.status = Status::Completed;
        self.record.pending_tool = None;
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
            let objective = text
                .rsplit_once("User request:")
                .map(|(_, request)| request.trim())
                .unwrap_or(text);
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
    let record: Record = serde_json::from_slice(
        &std::fs::read(path).with_context(|| format!("reading task {}", path.display()))?,
    )?;
    if record.schema_version != TASK_SCHEMA_VERSION {
        anyhow::bail!(
            "task {} uses unsupported schema {}",
            record.id,
            record.schema_version
        );
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
        resumed.set_usage_baseline(Usage {
            input: 50,
            output: 10,
            requests: 1,
        });
        resumed.checkpoint_messages(
            &[],
            Usage {
                input: 80,
                output: 17,
                requests: 2,
            },
        );
        assert_eq!(resumed.record.usage.input, 130);
        assert_eq!(resumed.record.usage.output, 27);
        assert_eq!(resumed.record.usage.requests, 3);
        // Repeated checkpoints must not add the attempt twice.
        resumed.checkpoint_messages(
            &[],
            Usage {
                input: 90,
                output: 18,
                requests: 3,
            },
        );
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
            "native_state",
        ] {
            value.as_object_mut().unwrap().remove(field);
        }
        let old: Record = serde_json::from_value(value).unwrap();
        assert_eq!(old.execution, ExecutionCounters::default());
        let mut current = Config::default();
        current.backend.default_scope = "host".into();
        current.backend.workspace_network = "allow".into();
        let restored = restore_config(&old, &current).unwrap();
        assert_eq!(restored.backend.default_scope, "workspace");
        assert_eq!(restored.backend.workspace_network, "deny");
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
