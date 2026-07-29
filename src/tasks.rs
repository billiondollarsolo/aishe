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
    pub cwd: PathBuf,
    pub objective: String,
    pub messages: Vec<Msg>,
    #[serde(default)]
    pub completed_tools: Vec<CompletedTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_tool: Option<PendingTool>,
    #[serde(default)]
    pub usage: UsageSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error_kind: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

pub struct Active {
    record: Record,
    path: Option<PathBuf>,
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
            provider: config.aishe.provider.clone(),
            model: config.active_model().into(),
            cwd: cwd.to_path_buf(),
            objective: crate::redact::redact(objective),
            messages: Vec::new(),
            completed_tools: Vec::new(),
            pending_tool: None,
            usage: UsageSummary::default(),
            last_error_kind: None,
            last_error: None,
        };
        let path = persistence_enabled()
            .then(|| task_path(&record.id))
            .flatten();
        let mut active = Self { record, path };
        active.save();
        active
    }

    pub fn resume(record: Record) -> Self {
        let path = task_path(&record.id);
        let mut active = Self { record, path };
        active.record.status = Status::Active;
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

    pub fn checkpoint_messages(&mut self, messages: &[Msg], usage: Usage) {
        self.record.messages = sanitize_messages(messages);
        self.record.usage = usage.into();
        self.record.updated_at_ms = now_ms();
        self.save();
    }

    pub fn pending(&mut self, call: &ToolCall, messages: &[Msg], usage: Usage) {
        self.record.messages = sanitize_messages(messages);
        self.record.pending_tool = Some(PendingTool {
            call: sanitize_tool_call(call),
            may_have_started: false,
        });
        self.record.usage = usage.into();
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

fn load_path(path: &Path) -> Result<Record> {
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
