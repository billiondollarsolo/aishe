//! Durable human requests and a checkpoint-acknowledged steering mailbox.
//!
//! This module stores decisions. It never runs an approved action, relaxes a
//! policy, or reconstructs a task from its objective. Approvals bind the exact
//! action and execution context and are spent before the runtime admits it.

use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::agent::{ExecutionScope, NetworkPolicy};
use crate::providers::{Msg, ToolCall};

use super::{now_ms, update, Record, State};

const MAX_REQUESTS: usize = 128;
const MAX_FOLLOWUPS: usize = 256;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_CHOICES: usize = 12;
const MAX_CHOICE_BYTES: usize = 512;
const MAX_PREIMAGE_BYTES: u64 = 1024 * 1024;
const MAX_MAILBOX_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionKind {
    Question,
    Approval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Pending,
    Responded,
    Consumed,
    Invalidated,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InteractionResponse {
    Answer { text: String },
    Approved,
    Denied { reason: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FilePreimage {
    pub path: PathBuf,
    /// None denotes an absent file, distinct from an empty file's digest.
    pub sha256: Option<String>,
    pub bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InteractionBinding {
    pub native_task_id: String,
    pub call_id: String,
    pub tool_name: String,
    pub arguments_sha256: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_preimage: Option<FilePreimage>,
    pub action_digest: String,
}

impl InteractionBinding {
    pub fn for_call(
        native_task_id: &str,
        call: &ToolCall,
        cwd: &Path,
        root: &Path,
        scope: ExecutionScope,
        network: NetworkPolicy,
    ) -> Result<Self> {
        if native_task_id.is_empty() || call.id.is_empty() || call.name.is_empty() {
            anyhow::bail!("interaction requires a native task and tool call identity");
        }
        let cwd = cwd
            .canonicalize()
            .context("interaction cwd is unavailable")?;
        let workspace_root = root
            .canonicalize()
            .context("interaction workspace is unavailable")?;
        if scope == ExecutionScope::Workspace && !cwd.starts_with(&workspace_root) {
            anyhow::bail!("interaction cwd is outside the accepted workspace");
        }
        let file_preimage = if matches!(call.name.as_str(), "write_file" | "edit_file") {
            let target = call.arguments["path"]
                .as_str()
                .filter(|path| !path.is_empty())
                .context("file approval requires a target path")?;
            Some(file_preimage(&cwd, &workspace_root, scope, target)?)
        } else {
            None
        };
        let mut binding = Self {
            native_task_id: native_task_id.into(),
            call_id: call.id.clone(),
            tool_name: call.name.clone(),
            arguments_sha256: digest(&serde_json::to_vec(&call.arguments)?),
            cwd,
            workspace_root,
            scope,
            network,
            file_preimage,
            action_digest: String::new(),
        };
        binding.action_digest = binding.compute_digest()?;
        Ok(binding)
    }

    fn compute_digest(&self) -> Result<String> {
        // Provider call IDs change when the model reissues a reviewed action.
        // The action, task, preimage and authority remain exactly bound.
        Ok(digest(&serde_json::to_vec(&(
            &self.native_task_id,
            &self.tool_name,
            &self.arguments_sha256,
            &self.cwd,
            &self.workspace_root,
            self.scope,
            self.network,
            &self.file_preimage,
        ))?))
    }

    fn validate(&self) -> Result<()> {
        if self.compute_digest()? != self.action_digest
            || self.arguments_sha256.len() != 64
            || self.call_id.is_empty()
        {
            anyhow::bail!("interaction action binding is invalid");
        }
        Ok(())
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn file_preimage(
    cwd: &Path,
    root: &Path,
    scope: ExecutionScope,
    target: &str,
) -> Result<FilePreimage> {
    let requested = if Path::new(target).is_absolute() {
        PathBuf::from(target)
    } else {
        cwd.join(target)
    };
    // Inspect before collapsing '..': a symlink followed by '..' resolves
    // differently in the actual file operation than in lexical normalization.
    let mut original_path = PathBuf::new();
    for component in requested.components() {
        original_path.push(component.as_os_str());
        if std::fs::symlink_metadata(&original_path)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            anyhow::bail!("file approvals cannot bind symlink targets");
        }
    }
    let mut path = PathBuf::new();
    for component in requested.components() {
        match component {
            Component::ParentDir => {
                path.pop();
            }
            Component::CurDir => {}
            _ => path.push(component.as_os_str()),
        }
    }
    if scope == ExecutionScope::Workspace && !path.starts_with(root) {
        anyhow::bail!("file approval target is outside the accepted workspace");
    }
    let mut component_path = PathBuf::new();
    for component in path.components() {
        component_path.push(component.as_os_str());
        match std::fs::symlink_metadata(&component_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                anyhow::bail!("file approvals cannot bind symlink targets");
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("checking file approval target"),
        }
    }
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FilePreimage {
            path,
            sha256: None,
            bytes: 0,
        }),
        Err(error) => Err(error).context("reading file approval target"),
        Ok(metadata) => {
            if !metadata.is_file() || metadata.len() > MAX_PREIMAGE_BYTES {
                anyhow::bail!("file approval requires a regular file of at most 1 MiB");
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.nlink() != 1 {
                    anyhow::bail!("file approvals cannot bind hard-linked targets");
                }
            }
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW);
            }
            let file = options.open(&path)?;
            let opened = file.metadata()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.dev() != opened.dev() || metadata.ino() != opened.ino() {
                    anyhow::bail!("file approval target changed while reading");
                }
            }
            use std::io::Read;
            let mut bytes = Vec::new();
            file.take(MAX_PREIMAGE_BYTES + 1).read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_PREIMAGE_BYTES {
                anyhow::bail!("file approval target exceeds 1 MiB");
            }
            Ok(FilePreimage {
                path,
                sha256: Some(digest(&bytes)),
                bytes: bytes.len() as u64,
            })
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InteractionRequest {
    pub id: String,
    pub nonce: String,
    pub kind: InteractionKind,
    pub prompt: String,
    #[serde(default)]
    pub choices: Vec<String>,
    pub binding: InteractionBinding,
    pub status: InteractionStatus,
    pub created_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responded_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumed_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<InteractionResponse>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowupStatus {
    Queued,
    Received,
    Removed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DeliveryClaim {
    native_task_id: String,
    worker_pid: u32,
    worker_start: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Followup {
    pub revision: u32,
    pub text: String,
    pub status: FollowupStatus,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<u128>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim: Option<DeliveryClaim>,
}

impl Followup {
    pub fn message(&self) -> String {
        format!("Follow-up #{}:\n{}", self.revision, self.text)
    }

    pub fn being_delivered(&self) -> bool {
        self.status == FollowupStatus::Queued && self.claim.is_some()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InteractionMailbox {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<InteractionRequest>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub followups: Vec<Followup>,
    #[serde(default)]
    pub next_followup_revision: u32,
}

impl InteractionMailbox {
    pub(super) fn validate_size(&self) -> Result<()> {
        if self.requests.len() > MAX_REQUESTS
            || self.followups.len() > MAX_FOLLOWUPS
            || serde_json::to_vec(self)?.len() > MAX_MAILBOX_BYTES
        {
            anyhow::bail!("task interaction mailbox exceeds its bounded history limit");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InteractionSummary {
    pub pending: Vec<InteractionRequest>,
    pub requests: Vec<InteractionRequest>,
    pub followups: Vec<Followup>,
    pub queued: usize,
    pub received: usize,
}

pub fn interaction_summary(id: &str) -> Result<InteractionSummary> {
    let mailbox = super::load(id)?.mailbox;
    Ok(InteractionSummary {
        pending: mailbox
            .requests
            .iter()
            .filter(|request| request.status == InteractionStatus::Pending)
            .cloned()
            .collect(),
        queued: mailbox
            .followups
            .iter()
            .filter(|entry| entry.status == FollowupStatus::Queued)
            .count(),
        received: mailbox
            .followups
            .iter()
            .filter(|entry| entry.status == FollowupStatus::Received)
            .count(),
        requests: mailbox.requests,
        followups: mailbox.followups,
    })
}

pub fn create_question(
    id: &str,
    binding: InteractionBinding,
    prompt: &str,
    choices: &[String],
) -> Result<InteractionRequest> {
    if choices.len() > MAX_CHOICES {
        anyhow::bail!("question has more than {MAX_CHOICES} choices");
    }
    let choices = choices
        .iter()
        .map(|choice| {
            if choice.len() > MAX_CHOICE_BYTES || choice.chars().any(char::is_control) {
                anyhow::bail!(
                    "question choices must be one line of at most {MAX_CHOICE_BYTES} bytes"
                );
            }
            let choice = bounded_text(choice)?;
            if choice.len() > MAX_CHOICE_BYTES {
                anyhow::bail!("redacted question choice exceeds {MAX_CHOICE_BYTES} bytes");
            }
            Ok(choice)
        })
        .collect::<Result<Vec<_>>>()?;
    create_request(id, binding, InteractionKind::Question, prompt, choices)
}

pub fn create_approval(
    id: &str,
    binding: InteractionBinding,
    summary: &str,
) -> Result<InteractionRequest> {
    create_request(id, binding, InteractionKind::Approval, summary, Vec::new())
}

fn create_request(
    id: &str,
    binding: InteractionBinding,
    kind: InteractionKind,
    prompt: &str,
    choices: Vec<String>,
) -> Result<InteractionRequest> {
    binding.validate()?;
    let prompt = bounded_text(prompt)?;
    let mut created = None;
    update(id, |record| {
        ensure_active(record)?;
        validate_record_binding(record, &binding)?;
        if let Some(existing) = record.mailbox.requests.iter().find(|request| {
            request.binding.native_task_id == binding.native_task_id
                && request.binding.call_id == binding.call_id
                && request.status != InteractionStatus::Invalidated
        }) {
            if existing.binding != binding || existing.kind != kind {
                anyhow::bail!("tool call already has a different interaction request");
            }
            if existing.status != InteractionStatus::Pending {
                anyhow::bail!("tool call's human request has already been answered");
            }
            created = Some(existing.clone());
            return Ok(());
        }
        if record.mailbox.requests.len() >= MAX_REQUESTS {
            anyhow::bail!("task reached its bounded human-request history limit");
        }
        let request = InteractionRequest {
            id: super::new_id(),
            nonce: super::new_id(),
            kind,
            prompt,
            choices,
            binding,
            status: InteractionStatus::Pending,
            created_at_ms: now_ms(),
            responded_at_ms: None,
            consumed_at_ms: None,
            response: None,
        };
        record.mailbox.requests.push(request.clone());
        created = Some(request);
        Ok(())
    })?;
    created.context("interaction request was not persisted")
}

pub fn interaction_for_call(
    id: &str,
    native_task_id: &str,
    call_id: &str,
) -> Result<Option<InteractionRequest>> {
    let record = super::load(id)?;
    if record.native_task_id.as_deref() != Some(native_task_id) {
        anyhow::bail!("interaction native task identity does not match");
    }
    Ok(record.mailbox.requests.into_iter().rev().find(|request| {
        request.binding.native_task_id == native_task_id
            && request.binding.call_id == call_id
            && request.status != InteractionStatus::Invalidated
    }))
}

fn matching_response<'a>(
    mailbox: &'a InteractionMailbox,
    binding: &InteractionBinding,
) -> Option<&'a InteractionRequest> {
    mailbox.requests.iter().rev().find(|request| {
        if request.binding.action_digest != binding.action_digest
            || request.status == InteractionStatus::Invalidated
        {
            return false;
        }
        match (&request.kind, &request.response) {
            (InteractionKind::Question, Some(InteractionResponse::Answer { .. })) => {
                request.status == InteractionStatus::Responded
                    && request.binding.call_id == binding.call_id
            }
            (InteractionKind::Approval, Some(InteractionResponse::Approved)) => {
                request.status == InteractionStatus::Responded
            }
            (InteractionKind::Approval, Some(InteractionResponse::Denied { .. })) => true,
            (InteractionKind::Question, Some(InteractionResponse::Denied { .. })) => {
                request.binding.call_id == binding.call_id
            }
            _ => false,
        }
    })
}

pub fn interaction_response(
    id: &str,
    binding: &InteractionBinding,
) -> Result<Option<InteractionResponse>> {
    binding.validate()?;
    let record = super::load(id)?;
    validate_record_binding(&record, binding)?;
    Ok(matching_response(&record.mailbox, binding).and_then(|request| request.response.clone()))
}

pub fn consume_interaction(
    id: &str,
    binding: &InteractionBinding,
) -> Result<Option<InteractionResponse>> {
    binding.validate()?;
    let mut response = None;
    update(id, |record| {
        ensure_active(record)?;
        validate_record_binding(record, binding)?;
        let Some(request_id) =
            matching_response(&record.mailbox, binding).map(|request| request.id.clone())
        else {
            return Ok(());
        };
        let request = record
            .mailbox
            .requests
            .iter_mut()
            .find(|request| request.id == request_id)
            .expect("matched request is present while locked");
        response = request.response.clone();
        request.status = InteractionStatus::Consumed;
        request.consumed_at_ms.get_or_insert_with(now_ms);
        Ok(())
    })?;
    Ok(response)
}

pub(super) fn respond(
    config: &crate::config::Config,
    id: &str,
    request_id: &str,
    response: InteractionResponse,
) -> Result<u8> {
    let response = match response {
        InteractionResponse::Answer { text } => InteractionResponse::Answer {
            text: bounded_text(&text)?,
        },
        InteractionResponse::Denied { reason } => InteractionResponse::Denied {
            reason: if reason.trim().is_empty() {
                "declined by user".into()
            } else {
                bounded_text(&reason)?
            },
        },
        InteractionResponse::Approved => InteractionResponse::Approved,
    };
    update(id, |record| {
        let binding = record
            .mailbox
            .requests
            .iter()
            .find(|request| request.id == request_id)
            .context("human request does not exist")?
            .binding
            .clone();
        validate_record_binding(record, &binding)?;
        let checkpoint = crate::tasks::load(&binding.native_task_id)?;
        if checkpoint.native_state.as_deref() != Some("waiting")
            || !checkpoint.pending_tool.as_ref().is_some_and(|pending| {
                pending.call.id == binding.call_id && !pending.may_have_started
            })
        {
            anyhow::bail!("human request has no matching safe waiting checkpoint");
        }
        record_response(record, request_id, response)
    })?;
    // Persist the response before starting a worker. If that worker has not
    // exited yet, it remains safely answered and an explicit resume can retry.
    for _ in 0..20 {
        let snapshot = super::load(id)?;
        if !snapshot
            .pid
            .is_some_and(|pid| super::same_process(pid, snapshot.process_start.as_deref()))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    super::resume(config, id)
        .context("response saved; could not resume yet (retry `aishe task resume`)")
}

fn record_response(
    record: &mut Record,
    request_id: &str,
    response: InteractionResponse,
) -> Result<()> {
    if record.state != State::Waiting {
        anyhow::bail!(
            "task {} is {:?}; it is not waiting for a response",
            record.id,
            record.state
        );
    }
    let request = record
        .mailbox
        .requests
        .iter_mut()
        .find(|request| request.id == request_id)
        .context("human request does not exist")?;
    request.binding.validate()?;
    if record.native_task_id.as_deref() != Some(&request.binding.native_task_id) {
        anyhow::bail!("human request belongs to a stale native checkpoint");
    }
    if request.status != InteractionStatus::Pending {
        anyhow::bail!("human request already has a response or is no longer active");
    }
    if !matches!(
        (request.kind, &response),
        (
            InteractionKind::Question,
            InteractionResponse::Answer { .. }
        ) | (InteractionKind::Approval, InteractionResponse::Approved)
            | (_, InteractionResponse::Denied { .. })
    ) {
        anyhow::bail!("response does not match this request type");
    }
    request.response = Some(response);
    request.responded_at_ms = Some(now_ms());
    request.status = InteractionStatus::Responded;
    Ok(())
}

fn bounded_text(text: &str) -> Result<String> {
    let text = text.trim();
    if text.is_empty() || text.len() > MAX_TEXT_BYTES || text.contains('\0') {
        anyhow::bail!("text must contain 1..={MAX_TEXT_BYTES} bytes without NUL");
    }
    Ok(crate::redact::redact(text))
}

pub(super) fn queue_followup(id: &str, text: &str) -> Result<u8> {
    let text = bounded_text(text)?;
    let mut revision = 0;
    update(id, |record| {
        if !matches!(
            record.state,
            State::Starting | State::Running | State::Waiting
        ) {
            anyhow::bail!(
                "task {} is {:?}; use rework for finished tasks",
                record.id,
                record.state
            );
        }
        if record.native_task_id.is_none() && record.engine.as_deref() != Some("native") {
            anyhow::bail!("live follow-ups require a native task");
        }
        if record.mailbox.followups.len() >= MAX_FOLLOWUPS {
            anyhow::bail!("task reached its bounded follow-up history limit");
        }
        revision = record
            .mailbox
            .next_followup_revision
            .checked_add(1)
            .context("follow-up revision limit reached")?;
        record.mailbox.next_followup_revision = revision;
        let now = now_ms();
        record.mailbox.followups.push(Followup {
            revision,
            text,
            status: FollowupStatus::Queued,
            created_at_ms: now,
            updated_at_ms: now,
            received_at_ms: None,
            claim: None,
        });
        Ok(())
    })?;
    println!("queued follow-up #{revision} for task {id}");
    Ok(0)
}

fn editable_followup(record: &mut Record, revision: u32) -> Result<&mut Followup> {
    let entry = record
        .mailbox
        .followups
        .iter_mut()
        .find(|entry| entry.revision == revision)
        .context("follow-up revision does not exist")?;
    if entry.status != FollowupStatus::Queued || entry.claim.is_some() {
        anyhow::bail!(
            "follow-up #{revision} has been received or is being delivered; send a new follow-up"
        );
    }
    Ok(entry)
}

pub(super) fn edit_followup(id: &str, revision: u32, text: &str) -> Result<u8> {
    let text = bounded_text(text)?;
    update(id, |record| {
        let entry = editable_followup(record, revision)?;
        entry.text = text;
        entry.updated_at_ms = now_ms().max(entry.updated_at_ms.saturating_add(1));
        Ok(())
    })?;
    println!("updated queued follow-up #{revision} for task {id}");
    Ok(0)
}

pub(super) fn remove_followup(id: &str, revision: u32) -> Result<u8> {
    update(id, |record| {
        let entry = editable_followup(record, revision)?;
        entry.status = FollowupStatus::Removed;
        entry.updated_at_ms = now_ms().max(entry.updated_at_ms.saturating_add(1));
        Ok(())
    })?;
    println!("removed queued follow-up #{revision} for task {id}");
    Ok(0)
}

pub fn queued_followups(id: &str) -> Result<Vec<Followup>> {
    Ok(super::load(id)?
        .mailbox
        .followups
        .into_iter()
        .filter(|entry| entry.status == FollowupStatus::Queued)
        .collect())
}

pub fn claim_followups(id: &str, native_task_id: &str) -> Result<Vec<Followup>> {
    // Most boundaries have no steering. Avoid an atomic record/cache write
    // when the mailbox is empty; arrivals will be claimed at the next boundary.
    if !super::load(id)?
        .mailbox
        .followups
        .iter()
        .any(|entry| entry.status == FollowupStatus::Queued)
    {
        return Ok(Vec::new());
    }
    let mut claimed = Vec::new();
    update(id, |record| {
        ensure_active(record)?;
        if record.native_task_id.as_deref() != Some(native_task_id) {
            anyhow::bail!("follow-up native task identity does not match");
        }
        let worker_pid = std::process::id();
        let worker_start = super::process_start(worker_pid);
        for entry in &mut record.mailbox.followups {
            if entry.status != FollowupStatus::Queued {
                continue;
            }
            if entry.claim.as_ref().is_some_and(|claim| {
                claim.worker_pid != worker_pid
                    && super::same_process(claim.worker_pid, claim.worker_start.as_deref())
            }) {
                anyhow::bail!("follow-up is being delivered by another live worker");
            }
            entry.claim = Some(DeliveryClaim {
                native_task_id: native_task_id.into(),
                worker_pid,
                worker_start: worker_start.clone(),
            });
            claimed.push(entry.clone());
        }
        Ok(())
    })?;
    Ok(claimed)
}

pub fn acknowledge_followups(id: &str, native_task_id: &str, revisions: &[u32]) -> Result<()> {
    // Reread the durable journal, rather than trusting an in-memory checkpoint
    // or a best-effort Active::checkpoint_messages call.
    let checkpoint = crate::tasks::load(native_task_id)?;
    update(id, |record| {
        ensure_active(record)?;
        super::validate_native_workspace(record, &checkpoint)?;
        if record.native_task_id.as_deref() != Some(native_task_id) {
            anyhow::bail!("follow-up native task identity does not match");
        }
        acknowledge_record_followups(record, &checkpoint, revisions)
    })
}

fn acknowledge_record_followups(
    record: &mut Record,
    checkpoint: &crate::tasks::Record,
    revisions: &[u32],
) -> Result<()> {
    for revision in revisions {
        let entry = record
            .mailbox
            .followups
            .iter_mut()
            .find(|entry| entry.revision == *revision)
            .context("follow-up revision does not exist")?;
        if entry.status == FollowupStatus::Received {
            continue;
        }
        if entry.status != FollowupStatus::Queued
            || entry
                .claim
                .as_ref()
                .is_none_or(|claim| claim.native_task_id != checkpoint.id)
            || !checkpoint
                .messages
                .iter()
                .any(|message| matches!(message, Msg::User(text) if text == &entry.message()))
        {
            anyhow::bail!("follow-up #{revision} has no matching durable transcript checkpoint");
        }
        entry.status = FollowupStatus::Received;
        entry.received_at_ms = Some(now_ms());
        entry.claim = None;
        entry.updated_at_ms = now_ms().max(entry.updated_at_ms.saturating_add(1));
    }
    Ok(())
}

fn ensure_active(record: &Record) -> Result<()> {
    if !matches!(record.state, State::Starting | State::Running) {
        anyhow::bail!(
            "task {} is {:?}; no interaction effects are admitted",
            record.id,
            record.state
        );
    }
    Ok(())
}

fn validate_record_binding(record: &Record, binding: &InteractionBinding) -> Result<()> {
    let checkpoint = crate::tasks::load(&binding.native_task_id)?;
    super::validate_native_workspace(record, &checkpoint)?;
    if record.native_task_id.as_deref() != Some(binding.native_task_id.as_str())
        || record.run_cwd != binding.workspace_root
        || ExecutionScope::parse(&record.scope) != Some(binding.scope)
        || checkpoint.execution_scope != Some(binding.scope)
        || checkpoint.network_policy != Some(binding.network)
    {
        anyhow::bail!("interaction does not match the task's saved workspace and authority");
    }
    Ok(())
}

pub(super) fn invalidate_pending(record: &mut Record) {
    for request in &mut record.mailbox.requests {
        if matches!(
            request.status,
            InteractionStatus::Pending | InteractionStatus::Responded
        ) {
            request.status = InteractionStatus::Invalidated;
        }
    }
}

pub(super) fn ready_to_resume(record: &Record) -> Result<()> {
    if record
        .mailbox
        .requests
        .iter()
        .any(|request| request.status == InteractionStatus::Pending)
    {
        anyhow::bail!(
            "task {} needs an answer or decision before it can resume",
            record.id
        );
    }
    if !record
        .mailbox
        .requests
        .iter()
        .any(|request| request.status == InteractionStatus::Responded)
    {
        anyhow::bail!("task {} has no answered human request to resume", record.id);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("aishe-interactions-{}", super::super::new_id()));
            std::fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: name.into(),
            arguments,
        }
    }

    fn binding(call: &ToolCall, root: &Path) -> InteractionBinding {
        InteractionBinding::for_call(
            "native-1",
            call,
            root,
            root,
            ExecutionScope::Host,
            NetworkPolicy::Allow,
        )
        .unwrap()
    }

    fn request(binding: InteractionBinding, kind: InteractionKind) -> InteractionRequest {
        InteractionRequest {
            id: "request-1".into(),
            nonce: "nonce-1".into(),
            kind,
            prompt: "Which target?".into(),
            choices: Vec::new(),
            binding,
            status: InteractionStatus::Pending,
            created_at_ms: 1,
            responded_at_ms: None,
            consumed_at_ms: None,
            response: None,
        }
    }

    fn running_record() -> Record {
        let mut record = super::super::tests::fixture_record();
        record.state = State::Running;
        record.native_task_id = Some("native-1".into());
        record.scope = "host".into();
        record.network = "deny".into();
        record
    }

    fn checkpoint() -> crate::tasks::Record {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": "native-1", "created_at_ms": 1,
            "updated_at_ms": 1, "status": "active", "mode": "yolo",
            "provider": "openai", "model": "fixture", "cwd": "/tmp",
            "objective": "fixture", "messages": [],
        }))
        .unwrap()
    }

    fn followup(revision: u32, text: &str) -> Followup {
        Followup {
            revision,
            text: text.into(),
            status: FollowupStatus::Queued,
            created_at_ms: 1,
            updated_at_ms: 1,
            received_at_ms: None,
            claim: None,
        }
    }

    #[test]
    fn approved_action_reissue_changes_call_id_but_not_action_or_authority() {
        let root = TestDir::new();
        let original = call(
            "old",
            "run_command",
            serde_json::json!({"command":"printf ok"}),
        );
        let original_binding = binding(&original, root.path());
        let mut reissued = original.clone();
        reissued.id = "new".into();
        assert_eq!(
            original_binding.action_digest,
            binding(&reissued, root.path()).action_digest
        );
        reissued.arguments["command"] = "printf other".into();
        assert_ne!(
            original_binding.action_digest,
            binding(&reissued, root.path()).action_digest
        );
        let mut changed_authority = original_binding.clone();
        changed_authority.network = NetworkPolicy::Deny;
        assert!(changed_authority.validate().is_err());
        changed_authority.action_digest = changed_authority.compute_digest().unwrap();
        assert_ne!(
            original_binding.action_digest,
            changed_authority.action_digest
        );
    }

    #[test]
    fn file_approval_binds_absence_empty_and_changed_preimage() {
        let root = TestDir::new();
        let call = call(
            "write",
            "write_file",
            serde_json::json!({"path":"target", "content":"new"}),
        );
        let absent = binding(&call, root.path());
        std::fs::write(root.path().join("target"), b"").unwrap();
        let empty = binding(&call, root.path());
        assert_ne!(absent.action_digest, empty.action_digest);
        std::fs::write(root.path().join("target"), b"changed").unwrap();
        assert_ne!(
            empty.action_digest,
            binding(&call, root.path()).action_digest
        );
    }

    #[cfg(unix)]
    #[test]
    fn file_approval_rejects_symlink_dotdot_and_hardlinks() {
        use std::os::unix::fs::symlink;
        let root = TestDir::new();
        let outside = TestDir::new();
        std::fs::create_dir(outside.path().join("dir")).unwrap();
        symlink(outside.path().join("dir"), root.path().join("link")).unwrap();
        let symlink_call = call(
            "write",
            "write_file",
            serde_json::json!({"path":"link/../target", "content":"new"}),
        );
        assert!(InteractionBinding::for_call(
            "native-1",
            &symlink_call,
            root.path(),
            root.path(),
            ExecutionScope::Host,
            NetworkPolicy::Allow
        )
        .is_err());
        std::fs::write(root.path().join("one"), b"before").unwrap();
        std::fs::hard_link(root.path().join("one"), root.path().join("two")).unwrap();
        let hardlink_call = call(
            "write",
            "write_file",
            serde_json::json!({"path":"one", "content":"new"}),
        );
        assert!(InteractionBinding::for_call(
            "native-1",
            &hardlink_call,
            root.path(),
            root.path(),
            ExecutionScope::Host,
            NetworkPolicy::Allow
        )
        .is_err());
    }

    #[test]
    fn workspace_approval_never_binds_escaping_target() {
        let root = TestDir::new();
        let call = call(
            "write",
            "write_file",
            serde_json::json!({"path":"../target", "content":"new"}),
        );
        assert!(InteractionBinding::for_call(
            "native-1",
            &call,
            root.path(),
            root.path(),
            ExecutionScope::Workspace,
            NetworkPolicy::Deny
        )
        .is_err());
    }

    #[test]
    fn consumed_approval_cannot_grant_again_but_a_denial_keeps_blocking_reissue() {
        let root = TestDir::new();
        let original = call(
            "old",
            "run_command",
            serde_json::json!({"command":"printf ok"}),
        );
        let original_binding = binding(&original, root.path());
        let mut approval = request(original_binding.clone(), InteractionKind::Approval);
        approval.status = InteractionStatus::Responded;
        approval.response = Some(InteractionResponse::Approved);
        let mut mailbox = InteractionMailbox::default();
        mailbox.requests.push(approval);
        assert!(matching_response(&mailbox, &original_binding).is_some());
        mailbox.requests[0].status = InteractionStatus::Consumed;
        assert!(matching_response(&mailbox, &original_binding).is_none());
        mailbox.requests[0].response = Some(InteractionResponse::Denied {
            reason: "declined".into(),
        });
        let mut reissued = original;
        reissued.id = "new".into();
        assert!(matching_response(&mailbox, &binding(&reissued, root.path())).is_some());
    }

    #[test]
    fn question_answer_is_bound_to_original_call_and_consumed_once() {
        let root = TestDir::new();
        let original = call(
            "question-1",
            "ask_user",
            serde_json::json!({"question":"Which target?"}),
        );
        let original_binding = binding(&original, root.path());
        let mut question = request(original_binding.clone(), InteractionKind::Question);
        question.status = InteractionStatus::Responded;
        question.response = Some(InteractionResponse::Answer {
            text: "staging".into(),
        });
        let mut mailbox = InteractionMailbox::default();
        mailbox.requests.push(question);
        assert!(matching_response(&mailbox, &original_binding).is_some());
        let mut other = original;
        other.id = "question-2".into();
        assert!(matching_response(&mailbox, &binding(&other, root.path())).is_none());
        mailbox.requests[0].status = InteractionStatus::Consumed;
        assert!(matching_response(&mailbox, &original_binding).is_none());
    }

    #[test]
    fn response_rejects_wrong_type_duplicate_and_cancel_race() {
        let root = TestDir::new();
        let mut record = running_record();
        record.state = State::Waiting;
        record.mailbox.requests.push(request(
            binding(
                &call("q", "ask_user", serde_json::json!({"question":"target?"})),
                root.path(),
            ),
            InteractionKind::Question,
        ));
        assert!(record_response(&mut record, "request-1", InteractionResponse::Approved).is_err());
        record_response(
            &mut record,
            "request-1",
            InteractionResponse::Answer {
                text: "staging".into(),
            },
        )
        .unwrap();
        assert!(record_response(
            &mut record,
            "request-1",
            InteractionResponse::Answer {
                text: "production".into()
            }
        )
        .is_err());
        record.state = State::Cancelled;
        invalidate_pending(&mut record);
        assert!(record_response(&mut record, "request-1", InteractionResponse::Approved).is_err());
        assert_eq!(
            record.mailbox.requests[0].status,
            InteractionStatus::Invalidated
        );
    }

    #[test]
    fn waiting_resume_requires_response_and_approval_is_not_an_answer() {
        let root = TestDir::new();
        let mut record = running_record();
        record.state = State::Waiting;
        assert!(ready_to_resume(&record).is_err());
        record.mailbox.requests.push(request(
            binding(
                &call(
                    "a",
                    "run_command",
                    serde_json::json!({"command":"printf ok"}),
                ),
                root.path(),
            ),
            InteractionKind::Approval,
        ));
        assert!(ready_to_resume(&record).is_err());
        assert!(record_response(
            &mut record,
            "request-1",
            InteractionResponse::Answer { text: "yes".into() }
        )
        .is_err());
        record_response(&mut record, "request-1", InteractionResponse::Approved).unwrap();
        assert!(ready_to_resume(&record).is_ok());
    }

    #[test]
    fn followup_ack_requires_exact_durable_transcript_and_claim_freezes_edits() {
        let mut record = running_record();
        let mut entry = followup(1, "Keep User request: unchanged in this instruction");
        entry.claim = Some(DeliveryClaim {
            native_task_id: "native-1".into(),
            worker_pid: 1,
            worker_start: None,
        });
        let message = entry.message();
        record.mailbox.followups.push(entry);
        assert!(editable_followup(&mut record, 1).is_err());
        let mut checkpoint = checkpoint();
        assert!(acknowledge_record_followups(&mut record, &checkpoint, &[1]).is_err());
        assert_eq!(record.mailbox.followups[0].status, FollowupStatus::Queued);
        checkpoint.messages.push(Msg::User(message));
        acknowledge_record_followups(&mut record, &checkpoint, &[1]).unwrap();
        assert_eq!(record.mailbox.followups[0].status, FollowupStatus::Received);
        assert!(record.mailbox.followups[0].received_at_ms.is_some());
        assert!(editable_followup(&mut record, 1).is_err());
    }

    #[test]
    fn followup_removal_is_a_tombstone_and_aggregate_size_is_bounded() {
        let mut record = running_record();
        record.mailbox.followups.push(followup(1, "queued"));
        editable_followup(&mut record, 1).unwrap().status = FollowupStatus::Removed;
        assert!(editable_followup(&mut record, 1).is_err());
        assert_eq!(record.mailbox.followups.len(), 1);
        for revision in 2..32 {
            record
                .mailbox
                .followups
                .push(followup(revision, &"a".repeat(MAX_TEXT_BYTES)));
        }
        assert!(record.mailbox.validate_size().is_err());
        assert!(bounded_text(" ").is_err());
        assert!(bounded_text("nul\0text").is_err());
    }
}
