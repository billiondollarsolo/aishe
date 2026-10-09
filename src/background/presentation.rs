//! A bounded read model for task badges and the task browser.
//!
//! Workers update one shared index while saving their authoritative records.
//! Shell watchers read that index, never task transcripts, git, or processes.
//! Reconciliation is throttled across shells and only checks active workers.
//! Opening details acknowledges that exact terminal result across shells.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::{Record, State, StepState};

const CACHE_SCHEMA: u32 = 2;
const MAX_ENTRIES: usize = 4096;
const MAX_CACHE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 1024 * 1024;
const MAX_CHECKPOINT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_LOG_PREVIEW_BYTES: u64 = 64 * 1024;
const MAX_PATCH_BYTES: usize = 256 * 1024;
const RECONCILE_INTERVAL_MS: u128 = 2_000;
const REDISCOVER_INTERVAL_MS: u128 = 60_000;
const MAX_UNTRACKED_PREVIEW_FILES: usize = 32;
// A valid mailbox text is at most 16 KiB. Escaping a one-byte terminal
// control can expand it fourfold, so preserve the entire accepted text.
const MAX_INTERACTION_PREVIEW_CHARS: usize = 4 * 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskAttention {
    NeedsYou,
    Queued,
    Running,
    Ready,
    Attention,
    Closed,
}

impl From<State> for TaskAttention {
    fn from(state: State) -> Self {
        match state {
            State::Blocked => Self::Queued,
            State::Waiting => Self::NeedsYou,
            State::Starting | State::Running => Self::Running,
            State::Completed => Self::Ready,
            State::Failed | State::Interrupted | State::Cancelled => Self::Attention,
            State::Applied | State::Discarded => Self::Closed,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskEntry {
    pub id: String,
    pub state: State,
    pub objective: String,
    pub title: String,
    pub pinned: bool,
    pub archived: bool,
    pub reviewed: bool,
    pub result_revision: String,
    pub metadata_revision: u64,
    pub pending_requests: usize,
    pub followups_queued: usize,
    pub followups_received: usize,
    pub source_cwd: PathBuf,
    pub project: PathBuf,
    pub updated_at_ms: u128,
    pub elapsed_ms: u64,
    pub attention: TaskAttention,
    pub isolated: bool,
    pub activity: String,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub stage_key: Option<String>,
    #[serde(default)]
    pub stage_name: Option<String>,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub workflow_completed: usize,
    #[serde(default)]
    pub workflow_total: usize,
}

impl TaskEntry {
    pub fn from_record(record: &Record) -> Self {
        let metadata = super::metadata::read_for(&record.id).unwrap_or_default();
        Self::with_metadata(record, metadata)
    }

    pub(super) fn with_metadata(record: &Record, metadata: super::metadata::TaskMetadata) -> Self {
        let result_revision = super::metadata::result_revision(record);
        let reviewed = !result_revision.is_empty()
            && metadata.reviewed_revision.as_deref() == Some(result_revision.as_str());
        let pending_requests = record
            .mailbox
            .requests
            .iter()
            .filter(|request| request.status == super::InteractionStatus::Pending)
            .count();
        let attention = if (record.state == State::Waiting && pending_requests == 0)
            || (record.state == State::Blocked && record.error.is_some())
        {
            TaskAttention::Attention
        } else {
            record.state.into()
        };
        Self {
            id: record.id.clone(),
            state: record.state,
            objective: safe_line(&record.objective, 320),
            title: metadata
                .title
                .unwrap_or_else(|| safe_line(&record.objective, 320)),
            pinned: metadata.pinned,
            // Old archived preferences never conceal an active/newly waiting task.
            archived: metadata.archived
                && !result_revision.is_empty()
                && pending_requests == 0
                && metadata.archived_revision.as_deref() == Some(result_revision.as_str()),
            reviewed,
            result_revision,
            metadata_revision: metadata.revision,
            pending_requests,
            followups_queued: record
                .mailbox
                .followups
                .iter()
                .filter(|entry| entry.status == super::FollowupStatus::Queued)
                .count(),
            followups_received: record
                .mailbox
                .followups
                .iter()
                .filter(|entry| entry.status == super::FollowupStatus::Received)
                .count(),
            source_cwd: record.source_cwd.clone(),
            project: record
                .source_repo
                .clone()
                .unwrap_or_else(|| record.source_cwd.clone()),
            updated_at_ms: record.updated_at_ms,
            elapsed_ms: super::elapsed(record),
            attention,
            isolated: record.worktree.is_some(),
            activity: record_activity(record),
            workflow_id: record
                .workflow
                .as_ref()
                .map(|stage| safe_line(&stage.run_id, 160)),
            stage_key: record
                .workflow
                .as_ref()
                .map(|stage| safe_line(&stage.stage_key, 160)),
            stage_name: record
                .workflow
                .as_ref()
                .map(|stage| safe_line(&stage.stage_name, 160)),
            dependencies: record
                .workflow
                .as_ref()
                .map(|stage| {
                    stage
                        .dependencies
                        .iter()
                        .take(32)
                        .map(|id| safe_line(id, 160))
                        .collect()
                })
                .unwrap_or_default(),
            workflow_completed: 0,
            workflow_total: 0,
        }
    }

    fn belongs_to(&self, project: Option<&Path>) -> bool {
        project.is_none_or(|project| {
            project.starts_with(&self.project) || self.source_cwd.starts_with(project)
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TaskStatus {
    pub needs_you: usize,
    pub running: usize,
    pub ready: usize,
    pub attention: usize,
    #[serde(default)]
    pub queued: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct SeenRevision {
    updated_at_ms: u128,
    state: State,
    #[serde(default)]
    result_revision: String,
}

/// Compatibility for old per-shell files. Current reviews persist in metadata;
/// merely listing tasks never marks them as reviewed.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SeenTasks {
    #[serde(default)]
    revisions: BTreeMap<String, SeenRevision>,
}

impl SeenTasks {
    pub fn is_seen(&self, entry: &TaskEntry) -> bool {
        entry.reviewed
            || self.revisions.get(&entry.id).is_some_and(|revision| {
                !entry.result_revision.is_empty()
                    && revision.result_revision == entry.result_revision
                    && revision.state == entry.state
            })
    }

    fn acknowledge(&mut self, entry: &TaskEntry) {
        if super::metadata::is_terminal(entry.state) {
            self.revisions.insert(
                entry.id.clone(),
                SeenRevision {
                    updated_at_ms: entry.updated_at_ms,
                    state: entry.state,
                    result_revision: entry.result_revision.clone(),
                },
            );
        }
        while self.revisions.len() > MAX_ENTRIES {
            let oldest = self
                .revisions
                .iter()
                .min_by_key(|(_, revision)| revision.updated_at_ms)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.revisions.remove(&oldest);
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskCheckpoint {
    pub id: String,
    pub status: crate::tasks::Status,
    pub updated_at_ms: u128,
    pub native_state: Option<String>,
    pub execution: crate::tasks::ExecutionCounters,
    #[serde(default)]
    pub execution_limits: Option<crate::agent::native::NativeLimits>,
    pub usage: crate::tasks::UsageSummary,
    pub pending_tool: Option<String>,
    pub completed_tools: usize,
    pub last_error: Option<String>,
    pub latest_result: Option<String>,
    pub check_summary: crate::tasks::CheckSummary,
    pub evidence: Vec<crate::tasks::CheckEvidence>,
    pub evidence_dropped: usize,
    pub workspace_revision: u64,
    pub timeline: Vec<crate::tasks::TaskTimelineEvent>,
    pub timeline_dropped: usize,
}

#[derive(Clone, Debug)]
pub struct TaskDetails {
    /// Display-only copy. Actions must continue loading the authoritative ID.
    pub record: Record,
    /// Exact result revision before display sanitization. Use this for review.
    pub entry: TaskEntry,
    pub interaction: super::InteractionSummary,
    pub checkpoint: Option<TaskCheckpoint>,
    pub log_lines: Vec<String>,
    pub activity: String,
    pub timeline: Vec<crate::tasks::TaskTimelineEvent>,
    pub timeline_dropped: usize,
    pub timeline_warning: Option<String>,
    pub handoff: Option<TaskHandoff>,
}

#[derive(Clone, Debug)]
pub struct TaskHandoff {
    pub direction: crate::agent::native::handoff::Direction,
    pub status: crate::agent::native::handoff::Status,
    pub requested: Option<crate::agent::native::handoff::Direction>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskTimeline {
    pub events: Vec<crate::tasks::TaskTimelineEvent>,
    pub dropped: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

pub fn task_timeline(id: &str) -> Result<TaskTimeline> {
    let details = task_details(id)?;
    Ok(TaskTimeline {
        events: details.timeline,
        dropped: details.timeline_dropped,
        warning: details.timeline_warning,
    })
}

impl TaskDetails {
    pub fn elapsed_ms(&self) -> u64 {
        super::elapsed(&self.record)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Cache {
    schema_version: u32,
    #[serde(default)]
    reconciled_at_ms: u128,
    #[serde(default)]
    discovered_at_ms: u128,
    entries: Vec<TaskEntry>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            schema_version: CACHE_SCHEMA,
            reconciled_at_ms: 0,
            discovered_at_ms: 0,
            entries: Vec::new(),
        }
    }
}

pub fn read_seen(path: &Path) -> SeenTasks {
    read_bounded(path, 512 * 1024)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SeenTasks>(&bytes).ok())
        .filter(|seen| seen.revisions.len() <= MAX_ENTRIES)
        .unwrap_or_default()
}

/// Acknowledge the record revision that was actually opened, never a newer
/// revision that might finish concurrently while the details page is visible.
pub fn acknowledge_task(entry: &TaskEntry, path: &Path) -> Result<()> {
    if !super::metadata::is_terminal(entry.state) {
        return Ok(());
    }
    super::metadata::mark_task_reviewed(entry)?;
    let parent = path.parent().context("seen file has no parent")?;
    fs::create_dir_all(parent)?;
    let lock_path = path.with_extension("lock");
    let lock = lock_file(&lock_path)?;
    lock.lock_exclusive()?;
    let mut seen = read_seen(path);
    seen.acknowledge(entry);
    super::write_private(path, &serde_json::to_vec(&seen)?)
}

/// Fast path: one bounded cache read, with no record, process, or git inspection.
pub fn cached_task_entries(project: Option<&Path>) -> Result<Vec<TaskEntry>> {
    cached_task_entries_with_query(project, EntryQuery::default())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EntryQuery {
    pub include_closed: bool,
    pub include_archived: bool,
}

pub fn cached_task_entries_with_query(
    project: Option<&Path>,
    query: EntryQuery,
) -> Result<Vec<TaskEntry>> {
    let mut cache = read_cache(&cache_path()?)?;
    annotate_workflows(&mut cache.entries);
    let mut entries = cache
        .entries
        .into_iter()
        .filter(|entry| {
            entry.belongs_to(project)
                && (query.include_closed || entry.attention != TaskAttention::Closed)
                && (query.include_archived || !entry.archived)
        })
        .collect::<Vec<_>>();
    sort_display_entries(&mut entries);
    Ok(entries)
}

pub fn task_status(project: Option<&Path>, seen: &SeenTasks) -> Result<TaskStatus> {
    Ok(status_for(&cached_task_entries(project)?, seen))
}

pub fn shell_status_text(project: Option<&Path>, seen_path: Option<&Path>) -> Result<String> {
    let seen = seen_path.map(read_seen).unwrap_or_default();
    let status = task_status(project, &seen)?;
    Ok(format!(
        "running\t{}\nready\t{}\nattention\t{}\nneeds_you\t{}\nqueued\t{}\n",
        status.running, status.ready, status.attention, status.needs_you, status.queued
    ))
}

fn status_for(entries: &[TaskEntry], seen: &SeenTasks) -> TaskStatus {
    let mut status = TaskStatus::default();
    for entry in entries {
        if entry.archived {
            continue;
        }
        match entry.attention {
            TaskAttention::NeedsYou => status.needs_you += 1,
            TaskAttention::Running => status.running += 1,
            TaskAttention::Queued => status.queued += 1,
            TaskAttention::Ready if !seen.is_seen(entry) => status.ready += 1,
            TaskAttention::Attention if !seen.is_seen(entry) => status.attention += 1,
            _ => {}
        }
    }
    status
}

/// Timer/browser path. A separate nonblocking lock and timestamp ensure many
/// shell sessions do not duplicate process checks. Saves continue independently.
pub fn refresh_task_cache() -> Result<()> {
    let path = cache_path()?;
    let cached = read_cache(&path)?;
    if cached.reconciled_at_ms > 0
        && cached.discovered_at_ms > 0
        && super::now_ms().saturating_sub(cached.discovered_at_ms) < REDISCOVER_INTERVAL_MS
        && !cached
            .entries
            .iter()
            .any(|entry| entry.attention == TaskAttention::Running)
    {
        // Completed history is immutable until a worker/action saves a change.
        // An idle watcher must not rescan or rewrite it every two seconds.
        return Ok(());
    }
    if cached.reconciled_at_ms > 0
        && super::now_ms().saturating_sub(cached.reconciled_at_ms) < RECONCILE_INTERVAL_MS
    {
        return Ok(());
    }
    let refresh_lock = lock_file(&path.with_extension("refresh.lock"))?;
    if refresh_lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let cached = read_cache(&path)?;
    if cached.reconciled_at_ms > 0
        && super::now_ms().saturating_sub(cached.reconciled_at_ms) < RECONCILE_INTERVAL_MS
    {
        return Ok(());
    }
    let discover = cached.discovered_at_ms == 0
        || super::now_ms().saturating_sub(cached.discovered_at_ms) >= REDISCOVER_INTERVAL_MS;
    let mut records = if discover {
        bounded_records()?
    } else {
        cached
            .entries
            .iter()
            .filter(|entry| entry.attention == TaskAttention::Running)
            .filter_map(|entry| {
                let path = super::record_path(&entry.id).ok()?;
                let bytes = read_bounded(&path, MAX_RECORD_BYTES).ok()?;
                let record: Record = serde_json::from_slice(&bytes).ok()?;
                (record.schema_version == super::SCHEMA_VERSION && record.id == entry.id)
                    .then_some(record)
            })
            .collect::<Vec<_>>()
    };
    for record in &mut records {
        super::reconcile(record)?;
    }
    mutate_cache(&path, |cache| {
        // Merge under the index lock: a concurrent worker checkpoint must not
        // be overwritten with the older snapshot read during reconciliation.
        for record in &records {
            replace_entry(&mut cache.entries, TaskEntry::from_record(record));
        }
        cache.reconciled_at_ms = super::now_ms();
        if discover {
            cache.discovered_at_ms = cache.reconciled_at_ms;
        }
    })
}

/// Explicit browser reads reconcile stale workers before showing their states.
pub fn task_entries(project: Option<&Path>, include_closed: bool) -> Result<Vec<TaskEntry>> {
    task_entries_with_query(
        project,
        EntryQuery {
            include_closed,
            include_archived: false,
        },
    )
}

pub fn task_entries_with_query(
    project: Option<&Path>,
    query: EntryQuery,
) -> Result<Vec<TaskEntry>> {
    // Explicit refresh discovers records written by older aishe versions and
    // imported task fixtures. Timer refreshes inspect only indexed workers.
    let mut records = bounded_records()?;
    for record in &mut records {
        super::reconcile(record)?;
    }
    mutate_cache(&cache_path()?, |cache| {
        for record in &records {
            replace_entry(&mut cache.entries, TaskEntry::from_record(record));
        }
        cache.reconciled_at_ms = super::now_ms();
        cache.discovered_at_ms = cache.reconciled_at_ms;
    })?;
    let mut entries = cached_task_entries_with_query(project, query)?;
    sort_display_entries(&mut entries);
    Ok(entries)
}

pub(super) fn record_saved(record: &Record) -> Result<()> {
    let path = cache_path()?;
    mutate_cache(&path, |cache| {
        replace_entry(&mut cache.entries, TaskEntry::from_record(record));
    })
}

fn replace_entry(entries: &mut Vec<TaskEntry>, mut new: TaskEntry) {
    if let Some(index) = entries.iter().position(|entry| entry.id == new.id) {
        if entries[index].updated_at_ms > new.updated_at_ms
            || (entries[index].updated_at_ms == new.updated_at_ms
                && entries[index].state != new.state)
        {
            return;
        }
        if entries[index].metadata_revision > new.metadata_revision {
            let previous = &entries[index];
            new.title = previous.title.clone();
            new.pinned = previous.pinned;
            new.archived = previous.archived && previous.result_revision == new.result_revision;
            new.reviewed = previous.reviewed && previous.result_revision == new.result_revision;
            new.metadata_revision = previous.metadata_revision;
        }
        entries.remove(index);
    }
    // Retain closed revisions as bounded tombstones, preventing a refresh that
    // read an older running record from reopening a concurrently applied task.
    entries.push(new);
}

fn trim_entries(entries: &mut Vec<TaskEntry>) {
    // Retain active workers and the newest failures before finished history.
    entries.sort_by_key(|entry| {
        let priority = match entry.attention {
            TaskAttention::NeedsYou => 0,
            TaskAttention::Running => 1,
            TaskAttention::Queued => 2,
            TaskAttention::Attention => 3,
            TaskAttention::Ready => 4,
            TaskAttention::Closed => 5,
        };
        (
            entry.archived,
            priority,
            !entry.pinned,
            std::cmp::Reverse(entry.updated_at_ms),
        )
    });
    entries.truncate(MAX_ENTRIES);
}

fn sort_display_entries(entries: &mut [TaskEntry]) {
    entries.sort_by_key(|entry| {
        let priority = match entry.attention {
            TaskAttention::NeedsYou => 0,
            TaskAttention::Attention => 1,
            TaskAttention::Running => 2,
            TaskAttention::Queued => 3,
            TaskAttention::Ready => 4,
            TaskAttention::Closed => 5,
        };
        (
            entry.attention != TaskAttention::NeedsYou,
            !entry.pinned,
            priority,
            std::cmp::Reverse(entry.updated_at_ms),
        )
    });
}

fn annotate_workflows(entries: &mut [TaskEntry]) {
    let mut groups: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for entry in entries.iter() {
        if let Some(id) = &entry.workflow_id {
            let group = groups.entry(id.clone()).or_default();
            group.1 += 1;
            group.0 += usize::from(matches!(entry.state, State::Completed | State::Applied));
        }
    }
    for entry in entries {
        if let Some((completed, total)) = entry.workflow_id.as_ref().and_then(|id| groups.get(id)) {
            entry.workflow_completed = *completed;
            entry.workflow_total = *total;
        }
    }
}

fn mutate_cache(path: &Path, change: impl FnOnce(&mut Cache)) -> Result<()> {
    let lock = lock_file(&path.with_extension("lock"))?;
    lock.lock_exclusive()?;
    let mut cache = read_cache(path)?;
    change(&mut cache);
    trim_entries(&mut cache.entries);
    let mut bytes = serde_json::to_vec(&cache)?;
    while bytes.len() as u64 > MAX_CACHE_BYTES && !cache.entries.is_empty() {
        cache.entries.pop();
        bytes = serde_json::to_vec(&cache)?;
    }
    super::write_private(path, &bytes)
}

fn cache_path() -> Result<PathBuf> {
    Ok(super::task_root()?.join("status-v1.json"))
}

fn read_cache(path: &Path) -> Result<Cache> {
    match read_bounded(path, MAX_CACHE_BYTES) {
        Ok(bytes) => Ok(serde_json::from_slice::<Cache>(&bytes)
            .ok()
            .filter(|cache| {
                cache.schema_version == CACHE_SCHEMA && cache.entries.len() <= MAX_ENTRIES
            })
            .unwrap_or_default()),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(Cache::default())
        }
        Err(error) => Err(error),
    }
}

fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    no_follow(&mut options);
    let lock = options.open(path)?;
    super::set_private(path, 0o600);
    Ok(lock)
}

fn bounded_records() -> Result<Vec<Record>> {
    let mut paths = fs::read_dir(super::task_root()?)?
        .flatten()
        .take(MAX_ENTRIES * 8)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let path = entry.path().join("record.json");
            let metadata = fs::symlink_metadata(&path).ok()?;
            (metadata.is_file() && metadata.len() <= MAX_RECORD_BYTES)
                .then_some((metadata.modified().ok(), path))
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    paths.truncate(MAX_ENTRIES);
    Ok(paths
        .into_iter()
        .filter_map(|(_, path)| {
            let bytes = read_bounded(&path, MAX_RECORD_BYTES).ok()?;
            let record: Record = serde_json::from_slice(&bytes).ok()?;
            (record.schema_version == super::SCHEMA_VERSION
                && super::validate_id(&record.id).is_ok()
                && path.parent()?.file_name()? == std::ffi::OsStr::new(&record.id))
            .then_some(record)
        })
        .collect())
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        anyhow::bail!("task presentation file exceeds the preview limit or is not a regular file");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    options
        .open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("task presentation file exceeds the preview limit");
    }
    Ok(bytes)
}

pub fn task_details(id: &str) -> Result<TaskDetails> {
    let mut record = read_record(id)?;
    super::reconcile(&mut record)?;
    let entry = TaskEntry::from_record(&record);
    let checkpoint = record
        .native_task_id
        .as_deref()
        .and_then(|id| load_checkpoint_summary(id, record.state));
    let handoff = record
        .native_task_id
        .as_deref()
        .and_then(|id| crate::agent::native::handoff::snapshot(id).ok())
        .map(|snapshot| TaskHandoff {
            direction: snapshot.mode,
            status: snapshot.status,
            requested: snapshot.requested,
        });
    let activity = checkpoint
        .as_ref()
        .and_then(|checkpoint| checkpoint.pending_tool.as_ref())
        .filter(|_| matches!(record.state, State::Starting | State::Running))
        .map(|tool| format!("Using {tool}"))
        .unwrap_or_else(|| record_activity(&record));
    let log_lines = preview_log(&super::log_path(id)?).unwrap_or_default();
    let (journal, timeline_warning) = match crate::tasks::timeline::read_background(id) {
        Ok(journal) => (journal, None),
        Err(error) => (
            crate::tasks::timeline::TimelineJournal::default(),
            Some(safe_line(
                &format!("Background timeline unavailable: {error}"),
                320,
            )),
        ),
    };
    let mut timeline = checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.timeline.clone())
        .unwrap_or_default();
    let mut timeline_dropped = journal.dropped.saturating_add(
        checkpoint
            .as_ref()
            .map_or(0, |checkpoint| checkpoint.timeline_dropped),
    );
    timeline.extend(journal.events);
    timeline.sort_by_key(|event| {
        (
            event.at_ms,
            match event.source {
                crate::tasks::timeline::EventSource::Native => 0,
                crate::tasks::timeline::EventSource::Background => 1,
            },
            event.sequence,
        )
    });
    if timeline.len() > crate::tasks::timeline::MAX_EVENTS {
        let dropped = timeline.len() - crate::tasks::timeline::MAX_EVENTS;
        timeline.drain(..dropped);
        timeline_dropped = timeline_dropped.saturating_add(dropped);
    }
    sanitize_record(&mut record);
    let interaction = super::InteractionSummary {
        pending: record
            .mailbox
            .requests
            .iter()
            .filter(|request| request.status == super::InteractionStatus::Pending)
            .cloned()
            .collect(),
        requests: record.mailbox.requests.clone(),
        followups: record.mailbox.followups.clone(),
        queued: entry.followups_queued,
        received: entry.followups_received,
    };
    Ok(TaskDetails {
        record,
        entry,
        interaction,
        checkpoint,
        log_lines,
        activity,
        timeline,
        timeline_dropped,
        timeline_warning,
        handoff,
    })
}

fn read_record(id: &str) -> Result<Record> {
    super::validate_id(id)?;
    let bytes = read_bounded(&super::record_path(id)?, MAX_RECORD_BYTES)?;
    let record: Record = serde_json::from_slice(&bytes)?;
    if record.schema_version != super::SCHEMA_VERSION || record.id != id {
        anyhow::bail!("invalid task record");
    }
    Ok(record)
}

fn load_checkpoint_summary(id: &str, state: State) -> Option<TaskCheckpoint> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let path = crate::tasks::root()?.join(format!("{id}.json"));
    let bytes = read_bounded(&path, MAX_CHECKPOINT_BYTES).ok()?;
    let mut record: crate::tasks::Record = serde_json::from_slice(&bytes).ok()?;
    if record.schema_version != crate::tasks::TASK_SCHEMA_VERSION || record.id != id {
        return None;
    }
    if !matches!(state, State::Starting | State::Running) {
        record.reconcile_unfinished_evidence(state == State::Cancelled);
    }
    Some(checkpoint_summary(&record))
}

fn checkpoint_summary(record: &crate::tasks::Record) -> TaskCheckpoint {
    let latest_result = record.messages.iter().rev().find_map(|message| {
        let assistant = match message {
            crate::providers::Msg::Assistant(assistant)
            | crate::providers::Msg::ProviderItems { assistant, .. } => assistant,
            _ => return None,
        };
        assistant
            .text
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .map(|text| safe_multiline(text, 16 * 1024))
    });
    TaskCheckpoint {
        id: record.id.clone(),
        status: record.status,
        updated_at_ms: record.updated_at_ms,
        native_state: record
            .native_state
            .as_deref()
            .map(|state| safe_line(state, 80)),
        execution: record.execution,
        execution_limits: record.execution_limits.clone(),
        usage: record.usage.clone(),
        pending_tool: record
            .pending_tool
            .as_ref()
            .map(|pending| safe_line(&pending.call.name, 120)),
        completed_tools: record.completed_tools.len(),
        last_error: record
            .last_error
            .as_deref()
            .map(|error| safe_multiline(error, 4096)),
        latest_result,
        check_summary: record.check_summary(),
        evidence: record
            .evidence
            .iter()
            .rev()
            .take(100)
            .rev()
            .cloned()
            .map(|mut evidence| {
                evidence.command = safe_line(&evidence.command, 2048);
                evidence.output = safe_multiline(&evidence.output, 8192);
                evidence.cwd = safe_path(&evidence.cwd);
                evidence
            })
            .collect(),
        evidence_dropped: record.evidence_dropped,
        workspace_revision: record.workspace_revision,
        timeline: record
            .timeline
            .iter()
            .rev()
            .take(crate::tasks::timeline::MAX_EVENTS)
            .rev()
            .cloned()
            .map(crate::tasks::TaskTimelineEvent::sanitized)
            .collect(),
        timeline_dropped: record.timeline_dropped.saturating_add(
            record
                .timeline
                .len()
                .saturating_sub(crate::tasks::timeline::MAX_EVENTS),
        ),
    }
}

fn record_activity(record: &Record) -> String {
    match record.state {
        State::Blocked if record.error.is_some() => "Dependency needs attention".into(),
        State::Blocked => "Queued · waiting for dependencies".into(),
        State::Waiting
            if record
                .mailbox
                .requests
                .iter()
                .any(|request| request.status == super::InteractionStatus::Pending) =>
        {
            "Needs your response".into()
        }
        State::Waiting => "Response saved · ready to resume".into(),
        State::Starting => "Starting".into(),
        State::Running => record
            .plan
            .iter()
            .find(|step| step.state == StepState::Active)
            .map(|step| safe_line(&step.text, 160))
            .unwrap_or_else(|| "Working".into()),
        State::Completed => "Finished · ready to inspect".into(),
        State::Failed => "Needs attention".into(),
        State::Interrupted => "Interrupted · can resume".into(),
        State::Cancelled => "Stopped".into(),
        State::Applied => "Applied".into(),
        State::Discarded => "Discarded".into(),
    }
}

fn sanitize_record(record: &mut Record) {
    record.objective = safe_multiline(&record.objective, 8192);
    record.error = record
        .error
        .as_deref()
        .map(|error| safe_multiline(error, 4096));
    record.source_branch = record
        .source_branch
        .as_deref()
        .map(|branch| safe_line(branch, 256));
    record.connection = None;
    record.process_start = None;
    if let Some(stage) = &mut record.workflow {
        stage.run_id = safe_line(&stage.run_id, 160);
        stage.stage_key = safe_line(&stage.stage_key, 160);
        stage.stage_name = safe_line(&stage.stage_name, 160);
        stage.dependencies.truncate(32);
        for id in &mut stage.dependencies {
            *id = safe_line(id, 160);
        }
        stage.required_checks.truncate(32);
        for command in &mut stage.required_checks {
            *command = safe_line(command, 2048);
        }
    }
    record.mailbox.requests.truncate(128);
    for request in &mut record.mailbox.requests {
        request.prompt = safe_multiline(&request.prompt, MAX_INTERACTION_PREVIEW_CHARS);
        request.choices.truncate(12);
        for choice in &mut request.choices {
            *choice = safe_multiline(choice, MAX_INTERACTION_PREVIEW_CHARS);
        }
        match &mut request.response {
            Some(super::InteractionResponse::Answer { text }) => {
                *text = safe_multiline(text, MAX_INTERACTION_PREVIEW_CHARS)
            }
            Some(super::InteractionResponse::Denied { reason }) => {
                *reason = safe_multiline(reason, MAX_INTERACTION_PREVIEW_CHARS)
            }
            _ => {}
        }
        request.binding.tool_name = safe_line(&request.binding.tool_name, 120);
        request.binding.cwd = safe_path(&request.binding.cwd);
        request.binding.workspace_root = safe_path(&request.binding.workspace_root);
        if let Some(preimage) = &mut request.binding.file_preimage {
            preimage.path = safe_path(&preimage.path);
        }
    }
    record.mailbox.followups.truncate(256);
    for followup in &mut record.mailbox.followups {
        followup.text = safe_multiline(&followup.text, MAX_INTERACTION_PREVIEW_CHARS);
    }
    record.steering.truncate(20);
    for steering in &mut record.steering {
        *steering = safe_multiline(steering, 2048);
    }
    record.plan.truncate(100);
    for step in &mut record.plan {
        step.text = safe_line(&step.text, 240);
        step.evidence = step
            .evidence
            .as_deref()
            .map(|evidence| safe_line(evidence, 1024));
    }
    for text in [
        &mut record.engine,
        &mut record.base_head,
        &mut record.applied_patch_sha256,
    ]
    .into_iter()
    .flatten()
    {
        *text = safe_line(text, 256);
    }
    for value in [
        &mut record.connection_id,
        &mut record.provider,
        &mut record.model,
        &mut record.role,
        &mut record.scope,
        &mut record.network,
    ] {
        *value = safe_line(value, 256);
    }
}

fn safe_path(path: &Path) -> PathBuf {
    PathBuf::from(safe_line(&path.to_string_lossy(), 4096))
}

fn preview_log(path: &Path) -> Result<Vec<String>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        anyhow::bail!("task log is not a regular file");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let mut file = options.open(path)?;
    let offset = metadata.len().saturating_sub(MAX_LOG_PREVIEW_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(MAX_LOG_PREVIEW_BYTES).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.lines().collect::<Vec<_>>();
    if offset > 0 && !lines.is_empty() {
        lines.remove(0); // Never display an accidentally sliced secret/token.
    }
    Ok(lines
        .into_iter()
        .rev()
        .take(100)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| safe_line(line, 1024))
        .collect())
}

/// A read-only patch preview. Disables external diff/textconv helpers and caps
/// bytes before rendering. Applying remains a separate explicit task action.
pub fn task_patch_lines(id: &str, max_lines: usize) -> Result<Vec<String>> {
    patch_lines_for_record(&read_record(id)?, max_lines)
}

fn patch_lines_for_record(record: &Record, max_lines: usize) -> Result<Vec<String>> {
    let worktree = record
        .worktree
        .as_ref()
        .context("task has no isolated worktree")?;
    let base = record
        .base_head
        .as_deref()
        .context("task has no recorded base commit")?;
    if !matches!(base.len(), 40 | 64) || !base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("task base commit is not an object ID");
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    let filters = configured_filters(worktree, deadline)?;
    let mut command = git_command(worktree, &filters);
    command.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-renames",
        base,
        "--",
    ]);
    let (mut bytes, mut clipped) = bounded_command(&mut command, MAX_PATCH_BYTES, true, deadline)?;
    let mut list = git_command(worktree, &filters);
    list.args(["ls-files", "--others", "--exclude-standard", "-z"]);
    let (mut untracked, list_clipped) = bounded_command(&mut list, 64 * 1024, false, deadline)?;
    if list_clipped && untracked.last() != Some(&0) {
        untracked.truncate(
            untracked
                .iter()
                .rposition(|byte| *byte == 0)
                .map_or(0, |index| index + 1),
        );
    }
    clipped |= list_clipped;
    for (index, raw) in untracked
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
        .enumerate()
    {
        if bytes.len() >= MAX_PATCH_BYTES
            || index >= MAX_UNTRACKED_PREVIEW_FILES
            || Instant::now() >= deadline
        {
            clipped = true;
            break;
        }
        let Ok(relative) = std::str::from_utf8(raw) else {
            clipped = true;
            continue;
        };
        if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
            anyhow::bail!("unsafe untracked task path");
        }
        let mut diff = git_command(worktree, &filters);
        diff.current_dir(worktree).args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--",
            "/dev/null",
            relative,
        ]);
        let (extra, extra_clipped) =
            bounded_command(&mut diff, MAX_PATCH_BYTES - bytes.len(), true, deadline)?;
        bytes.extend(extra);
        clipped |= extra_clipped;
    }
    let text = String::from_utf8_lossy(&bytes);
    let max_lines = max_lines.clamp(1, 2000);
    let mut lines = text
        .lines()
        .take(max_lines)
        .map(|line| safe_line(line, 2048))
        .collect::<Vec<_>>();
    if clipped || text.lines().count() > max_lines {
        lines.push("… preview clipped; use the task review command for the complete patch".into());
    }
    Ok(lines)
}

fn bounded_command(
    command: &mut Command,
    limit: usize,
    diff_exit: bool,
    deadline: Instant,
) -> Result<(Vec<u8>, bool)> {
    if Instant::now() >= deadline {
        return Ok((Vec::new(), true));
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().context("missing git stdout")?;
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    let received = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    let timed_out = matches!(&received, Err(std::sync::mpsc::RecvTimeoutError::Timeout));
    if timed_out {
        let _ = child.kill();
    }
    let mut bytes = match received {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(error)) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => receiver
            .recv_timeout(Duration::from_millis(100))
            .ok()
            .and_then(std::result::Result::ok)
            .unwrap_or_default(),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("task patch preview reader stopped");
        }
    };
    let mut clipped = timed_out || bytes.len() > limit;
    if clipped {
        let _ = child.kill();
        bytes.truncate(limit);
    }
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            clipped = true;
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !(clipped || status.success() || diff_exit && status.code() == Some(1)) {
        anyhow::bail!("could not read the task patch");
    }
    Ok((bytes, clipped))
}

fn git_command(worktree: &Path, filters: &[String]) -> Command {
    let mut command = Command::new("git");
    command.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
    ]);
    for filter in filters {
        for suffix in ["clean=", "process=", "required=false"] {
            command.arg("-c").arg(format!("filter.{filter}.{suffix}"));
        }
    }
    command
        .arg("-C")
        .arg(worktree)
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

fn configured_filters(worktree: &Path, deadline: Instant) -> Result<Vec<String>> {
    let mut command = git_command(worktree, &[]);
    command.args([
        "config",
        "--name-only",
        "--null",
        "--get-regexp",
        r"^filter\..*\.(clean|process)$",
    ]);
    let (bytes, clipped) = bounded_command(&mut command, 64 * 1024, true, deadline)?;
    if clipped {
        anyhow::bail!("task filter configuration exceeds the preview limit");
    }
    let mut filters = Vec::new();
    for key in bytes.split(|byte| *byte == 0).filter(|key| !key.is_empty()) {
        let key = std::str::from_utf8(key).context("invalid task filter configuration")?;
        let filter = key
            .strip_prefix("filter.")
            .and_then(|key| {
                key.strip_suffix(".clean")
                    .or_else(|| key.strip_suffix(".process"))
            })
            .context("invalid task filter configuration")?;
        if filter.len() > 256
            || !filter
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            anyhow::bail!("task filter configuration cannot be safely previewed");
        }
        if !filters.iter().any(|existing| existing == filter) {
            filters.push(filter.to_string());
        }
        if filters.len() > 64 {
            anyhow::bail!("task has too many configured filters to preview");
        }
    }
    Ok(filters)
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    {
        let _ = options;
    }
}

fn safe_line(value: &str, chars: usize) -> String {
    let redacted = crate::redact::redact(value);
    let safe = crate::commands::display_safe(&redacted);
    clipped_text(&safe, chars)
}

fn safe_multiline(value: &str, chars: usize) -> String {
    let redacted = crate::redact::redact(value);
    let safe = crate::commands::display_safe_multiline(&redacted);
    clipped_text(&safe, chars)
}

fn clipped_text(value: &str, chars: usize) -> String {
    let mut selected = value.chars().take(chars).collect::<String>();
    if value.chars().nth(chars).is_some() {
        selected.push('…');
    }
    selected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(state: State, updated_at_ms: u128) -> TaskEntry {
        let mut record = super::super::tests::fixture_record();
        record.state = state;
        record.updated_at_ms = updated_at_ms;
        record.result_revision = updated_at_ms as u64;
        let mut entry = TaskEntry::from_record(&record);
        if state == State::Waiting {
            entry.attention = TaskAttention::NeedsYou;
            entry.pending_requests = 1;
        }
        entry
    }

    #[test]
    fn badges_classify_finished_work_without_claiming_verification() {
        let entries = [
            entry(State::Starting, 1),
            entry(State::Running, 2),
            entry(State::Completed, 3),
            entry(State::Failed, 4),
            entry(State::Interrupted, 5),
            entry(State::Cancelled, 6),
            entry(State::Applied, 7),
            entry(State::Discarded, 8),
        ];
        assert_eq!(
            status_for(&entries, &SeenTasks::default()),
            TaskStatus {
                needs_you: 0,
                running: 2,
                ready: 1,
                attention: 3,
                queued: 0,
            }
        );
    }

    #[test]
    fn legacy_seen_files_acknowledge_only_exact_result_revision() {
        let finished = entry(State::Completed, 3);
        let mut first_shell = SeenTasks::default();
        let second_shell = SeenTasks::default();
        first_shell.acknowledge(&finished);
        assert_eq!(
            status_for(std::slice::from_ref(&finished), &first_shell).ready,
            0
        );
        assert_eq!(status_for(&[finished], &second_shell).ready, 1);
        assert_eq!(
            status_for(&[entry(State::Completed, 4)], &first_shell).ready,
            1
        );
        assert_eq!(
            status_for(&[entry(State::Failed, 3)], &first_shell).attention,
            1
        );
        first_shell.acknowledge(&entry(State::Running, 9));
        assert_eq!(
            status_for(&[entry(State::Completed, 9)], &first_shell).ready,
            1
        );
    }

    #[test]
    fn persistent_review_is_shared_without_hiding_new_results_or_requests() {
        let mut reviewed = entry(State::Completed, 3);
        reviewed.reviewed = true;
        assert_eq!(
            status_for(&[reviewed.clone()], &SeenTasks::default()).ready,
            0
        );
        let mut first_shell = SeenTasks::default();
        first_shell.acknowledge(&reviewed);
        let mut renamed = reviewed.clone();
        renamed.title = "My task".into();
        renamed.updated_at_ms = 999;
        renamed.pinned = true;
        assert_eq!(status_for(&[renamed], &SeenTasks::default()).ready, 0);
        assert_eq!(
            status_for(&[entry(State::Completed, 4)], &first_shell).ready,
            1
        );
        let waiting = entry(State::Waiting, 5);
        first_shell.acknowledge(&waiting);
        assert_eq!(status_for(&[waiting], &first_shell).needs_you, 1);
    }

    #[test]
    fn waiting_without_unanswered_requests_remains_visible_as_attention() {
        let mut record = super::super::tests::fixture_record();
        record.state = State::Waiting;
        let entry =
            TaskEntry::with_metadata(&record, super::super::metadata::TaskMetadata::default());
        assert_eq!(entry.attention, TaskAttention::Attention);
        assert_eq!(entry.pending_requests, 0);
        assert_eq!(status_for(&[entry], &SeenTasks::default()).attention, 1);
        assert_eq!(record_activity(&record), "Response saved · ready to resume");
    }

    #[test]
    fn needs_you_precedes_pinned_tasks_and_archived_results_are_quiet() {
        let mut pinned = entry(State::Completed, 8);
        pinned.id = "pinned".into();
        pinned.pinned = true;
        let mut waiting = entry(State::Waiting, 2);
        waiting.id = "waiting".into();
        let mut failed = entry(State::Failed, 9);
        failed.id = "failed".into();
        let mut archived = entry(State::Completed, 10);
        archived.archived = true;
        let mut entries = vec![failed, pinned, waiting];
        sort_display_entries(&mut entries);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["waiting", "pinned", "failed"]
        );
        assert_eq!(status_for(&[archived], &SeenTasks::default()).ready, 0);
    }

    #[test]
    fn project_filter_includes_nested_source_and_other_project_stays_separate() {
        let mut task = entry(State::Running, 1);
        task.project = "/work/project".into();
        task.source_cwd = "/work/project/sub".into();
        assert!(task.belongs_to(Some(Path::new("/work/project/sub"))));
        assert!(task.belongs_to(Some(Path::new("/work"))));
        assert!(!task.belongs_to(Some(Path::new("/work/project-other"))));
        assert!(task.belongs_to(None));
    }

    #[test]
    fn stale_cache_merge_cannot_overwrite_newer_checkpoint_or_reopen_closed_task() {
        let mut entries = vec![entry(State::Completed, 10)];
        replace_entry(&mut entries, entry(State::Running, 9));
        assert_eq!(entries[0].state, State::Completed);
        replace_entry(&mut entries, entry(State::Applied, 11));
        assert_eq!(entries[0].state, State::Applied);
        replace_entry(&mut entries, entry(State::Running, 9));
        assert_eq!(entries[0].state, State::Applied);
    }

    #[test]
    fn bounded_cache_keeps_workers_and_latest_attention_before_old_completions() {
        let mut entries = (0..MAX_ENTRIES)
            .map(|i| {
                let mut entry = entry(State::Completed, i as u128);
                entry.id = format!("finished-{i}");
                entry
            })
            .collect::<Vec<_>>();
        entries.push(entry(State::Running, 0));
        let mut failed = entry(State::Failed, 0);
        failed.id = "failed-task".into();
        entries.push(failed);
        trim_entries(&mut entries);
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries[0].state, State::Running);
        assert_eq!(entries[1].state, State::Failed);
    }

    #[test]
    fn previews_redact_before_truncating_and_escape_terminal_controls() {
        let source = format!("TOKEN={}\u{1b}[2J\r{}", "a".repeat(2000), "字".repeat(2000));
        let safe = safe_line(&source, 80);
        assert!(safe.contains("TOKEN=<redacted>"));
        assert!(!safe.contains("aaaa"));
        assert!(!safe.contains('\u{1b}'));
        assert!(!safe.contains('\r'));
        assert!(safe.chars().count() <= 81);
    }

    #[test]
    fn display_record_does_not_expose_connection_or_raw_error_and_plan_secrets() {
        let mut record = super::super::tests::fixture_record();
        record.objective = "PASSWORD=secret\u{1b}[2J".into();
        record.error = Some("Authorization: Bearer sensitive".into());
        record.plan.push(super::super::PlanStep {
            id: 1,
            text: "DB_PASSWORD=anothersecret".into(),
            state: StepState::Active,
            evidence: Some("https://user:pass@host/path".into()),
        });
        sanitize_record(&mut record);
        assert!(record.connection.is_none());
        assert!(!record.objective.contains("secret"));
        assert!(!record.error.unwrap().contains("sensitive"));
        assert!(!record.plan[0].text.contains("anothersecret"));
        assert!(!record.plan[0]
            .evidence
            .as_ref()
            .unwrap()
            .contains("user:pass"));
    }

    #[test]
    fn accepted_long_interaction_previews_preserve_action_and_choice_tails() {
        let mut record = super::super::tests::fixture_record();
        let action_tail = "\nactual action at the end";
        let prompt = format!("{}{action_tail}", "a".repeat(16 * 1024 - action_tail.len()));
        let choice_tail = "selected choice tail";
        // Escaping controls must not push a valid choice's tail past a preview cap.
        let choice = format!(
            "{}{choice_tail}",
            "\u{1b}".repeat(16 * 1024 - choice_tail.len())
        );
        record.mailbox.requests.push(
            serde_json::from_value(serde_json::json!({
                "id": "request", "nonce": "nonce", "kind": "approval",
                "prompt": prompt, "choices": [choice], "status": "pending",
                "created_at_ms": 1,
                "binding": {
                    "native_task_id": "native", "call_id": "call", "tool_name": "run_command",
                    "arguments_sha256": "0".repeat(64), "cwd": "/tmp", "workspace_root": "/tmp",
                    "scope": crate::agent::ExecutionScope::Host,
                    "network": crate::agent::NetworkPolicy::Allow,
                    "action_digest": "0".repeat(64)
                }
            }))
            .unwrap(),
        );
        sanitize_record(&mut record);
        let request = &record.mailbox.requests[0];
        assert_eq!(request.prompt.len(), 16 * 1024);
        assert!(request.prompt.ends_with(action_tail));
        assert!(request.choices[0].ends_with(choice_tail));
        assert!(!request.choices[0].contains('\u{1b}'));
        assert!(!request.prompt.ends_with('…'));
        assert!(!request.choices[0].ends_with('…'));
    }

    #[test]
    fn checkpoint_summary_exposes_final_answer_and_usage_without_tool_arguments() {
        let record: crate::tasks::Record = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "id": "native-task", "created_at_ms": 1,
            "updated_at_ms": 2, "status": "completed", "mode": "act",
            "provider": "fake", "model": "fake", "cwd": "/tmp",
            "objective": "inspect", "messages": [{"role": "assistant", "data": {
                "text": "Finished\nPASSWORD=secret\u{001b}[2J", "tool_calls": []
            }}],
            "pending_tool": {"call": {"id": "call", "name": "run_command",
                "arguments": {"command": "echo TOKEN=argumentsecret"}}, "may_have_started": true},
            "usage": {"input": 120, "output": 30, "requests": 2},
            "execution": {"provider_turns": 2, "tool_calls": 1, "network_calls": 0,
                "elapsed_ms": 250, "cost_usd": 0.01},
            "native_state": "completed"
        }))
        .unwrap();
        let summary = checkpoint_summary(&record);
        assert_eq!(summary.pending_tool.as_deref(), Some("run_command"));
        assert_eq!(summary.execution.provider_turns, 2);
        assert_eq!(summary.usage.input, 120);
        let result = summary.latest_result.as_deref().unwrap();
        assert!(result.starts_with("Finished\nPASSWORD=<redacted>"));
        assert!(!result.contains("secret"));
        assert!(!result.contains('\u{1b}'));
        assert!(!serde_json::to_string(&summary)
            .unwrap()
            .contains("argumentsecret"));
    }

    #[test]
    fn cache_read_does_not_reinspect_missing_task_directories_or_processes() {
        let dir = std::env::temp_dir().join(format!("aishe-task-cache-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("status-v1.json");
        let mut active = entry(State::Running, 20);
        active.source_cwd = dir.join("does-not-exist");
        mutate_cache(&path, |cache| cache.entries.push(active)).unwrap();
        let cache = read_cache(&path).unwrap();
        assert_eq!(status_for(&cache.entries, &SeenTasks::default()).running, 1);
        assert!(!dir.join("does-not-exist").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn private_preview_reads_reject_symlinks_and_oversized_files() {
        let dir = std::env::temp_dir().join(format!("aishe-task-preview-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("large");
        fs::write(&path, b"123456789").unwrap();
        assert!(read_bounded(&path, 8).is_err());
        #[cfg(unix)]
        {
            let link = dir.join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_bounded(&link, 100).is_err());
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn patch_preview_disables_git_helpers_and_caps_empty_untracked_files() {
        let dir = std::env::temp_dir().join(format!("aishe-task-patch-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "fixture@example.test"]);
        git(&["config", "user.name", "Fixture"]);
        fs::write(
            dir.join(".gitattributes"),
            "demo filter=custom diff=custom\n",
        )
        .unwrap();
        fs::write(dir.join("demo"), "before\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "base"]);
        let head = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout).unwrap();
        for key in [
            "filter.custom.clean",
            "filter.custom.process",
            "diff.custom.textconv",
            "diff.external",
            "core.fsmonitor",
        ] {
            git(&["config", key, "sh -c 'touch marker; cat'"]);
        }
        git(&["config", "filter.custom.required", "true"]);
        fs::write(dir.join("demo"), "after\nPASSWORD=previewsecret\n").unwrap();
        for index in 0..MAX_UNTRACKED_PREVIEW_FILES + 10 {
            fs::write(dir.join(format!("empty-{index:03}")), "").unwrap();
        }
        let mut record = super::super::tests::fixture_record();
        record.worktree = Some(dir.clone());
        record.base_head = Some(head.trim().to_string());
        let started = Instant::now();
        let lines = patch_lines_for_record(&record, 2000).unwrap();
        let text = lines.join("\n");
        assert!(text.contains("+after"), "{text}");
        assert!(text.contains("PASSWORD=<redacted>"), "{text}");
        assert!(!text.contains("previewsecret"));
        assert!(text.contains("preview clipped"));
        assert!(
            !dir.join("marker").exists(),
            "Git preview ran a configured helper"
        );
        assert_eq!(
            fs::read_to_string(dir.join("demo")).unwrap(),
            "after\nPASSWORD=previewsecret\n"
        );
        assert!(started.elapsed() < Duration::from_secs(4));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bounded_patch_process_deadline_kills_a_stalled_reader() {
        let started = Instant::now();
        let mut command = Command::new("sleep");
        command.arg("10");
        let (bytes, clipped) = bounded_command(
            &mut command,
            1024,
            false,
            started + Duration::from_millis(100),
        )
        .unwrap();
        assert!(clipped);
        assert!(bytes.is_empty());
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}
