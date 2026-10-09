//! Presentation preferences live separately from the immutable task request.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Record, State, TaskEntry};

const SCHEMA_VERSION: u32 = 1;
const MAX_METADATA_BYTES: u64 = 16 * 1024;
const MAX_NAME_BYTES: usize = 2048;
const MAX_NAME_CHARS: usize = 120;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TaskMetadata {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub archived: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewed_revision: Option<String>,
    #[serde(default)]
    pub revision: u64,
}

impl Default for TaskMetadata {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            title: None,
            pinned: false,
            archived: false,
            archived_revision: None,
            reviewed_revision: None,
            revision: 0,
        }
    }
}

pub fn rename_task(id: &str, name: &str) -> Result<()> {
    let title = validated_name(name)?;
    mutate(id, move |_, metadata| {
        metadata.title = title;
        Ok(())
    })
}

pub fn pin_task(id: &str, pinned: bool) -> Result<()> {
    mutate(id, |_, metadata| {
        metadata.pinned = pinned;
        Ok(())
    })
}

pub fn archive_task(id: &str, archived: bool) -> Result<()> {
    mutate(id, |record, metadata| {
        set_archived(record, metadata, archived)
    })
}

/// Marks only the result the user actually inspected. A concurrent new result
/// remains unread, even when a details view for the previous result is open.
pub fn mark_task_reviewed(entry: &TaskEntry) -> Result<()> {
    if !is_terminal(entry.state) || entry.result_revision.is_empty() {
        return Ok(());
    }
    mutate(&entry.id, |record, metadata| {
        review_exact(record, metadata, entry);
        Ok(())
    })
}

fn set_archived(record: &Record, metadata: &mut TaskMetadata, archived: bool) -> Result<()> {
    if archived
        && (!is_terminal(record.state)
            || record
                .mailbox
                .requests
                .iter()
                .any(|request| request.status == super::InteractionStatus::Pending))
    {
        anyhow::bail!("live tasks and tasks waiting for your response cannot be archived");
    }
    metadata.archived = archived;
    metadata.archived_revision = archived.then(|| result_revision(record));
    Ok(())
}

fn review_exact(record: &Record, metadata: &mut TaskMetadata, entry: &TaskEntry) {
    if !entry.result_revision.is_empty() && result_revision(record) == entry.result_revision {
        metadata.reviewed_revision = Some(entry.result_revision.clone());
    }
}

pub(super) fn is_terminal(state: State) -> bool {
    !matches!(
        state,
        State::Starting | State::Running | State::Waiting | State::Blocked
    )
}

/// The durable terminal generation distinguishes repeated successful attempts;
/// the fingerprint also supports old records with no generation field. Activity,
/// plans, mailbox traffic and presentation preferences are deliberately absent.
pub(super) fn result_revision(record: &Record) -> String {
    if !is_terminal(record.state) {
        return String::new();
    }
    let outcome = serde_json::json!({
        "id": record.id,
        "generation": record.result_revision,
        "state": record.state,
        "checkpoint": record.native_task_id,
        "exit": record.exit_code,
        "elapsed": record.elapsed_ms,
        "steering": record.steering_revision,
        "exceeded": record.budget_exceeded,
        "error": record.error,
        "applied": record.applied_patch_sha256,
    });
    format!("{:x}", Sha256::digest(outcome.to_string().as_bytes()))
}

pub(super) fn read_for(id: &str) -> Result<TaskMetadata> {
    super::validate_id(id)?;
    read_at(&super::task_dir(id)?)
}

fn read_at(dir: &Path) -> Result<TaskMetadata> {
    let path = dir.join("metadata.json");
    let bytes = match read_bounded(&path, MAX_METADATA_BYTES) {
        Ok(bytes) => bytes,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(TaskMetadata::default())
        }
        Err(error) => return Err(error),
    };
    let mut metadata: TaskMetadata = serde_json::from_slice(&bytes)?;
    if metadata.schema_version != SCHEMA_VERSION {
        anyhow::bail!("unsupported task metadata schema");
    }
    metadata.title = metadata
        .title
        .as_deref()
        .map(validated_name)
        .transpose()?
        .flatten();
    if [&metadata.reviewed_revision, &metadata.archived_revision]
        .into_iter()
        .flatten()
        .any(|revision| {
            revision.len() != 64 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        anyhow::bail!("invalid reviewed task revision");
    }
    Ok(metadata)
}

fn validated_name(name: &str) -> Result<Option<String>> {
    if name.len() > MAX_NAME_BYTES {
        anyhow::bail!("task name is too long (maximum {MAX_NAME_CHARS} characters)");
    }
    let safe = crate::commands::display_safe(&crate::redact::redact(name));
    let safe = safe.trim();
    if safe.chars().count() > MAX_NAME_CHARS {
        anyhow::bail!("task name is too long (maximum {MAX_NAME_CHARS} characters)");
    }
    Ok((!safe.is_empty()).then(|| safe.to_string()))
}

fn mutate(id: &str, change: impl FnOnce(&Record, &mut TaskMetadata) -> Result<()>) -> Result<()> {
    super::validate_id(id)?;
    let dir = super::task_dir(id)?;
    let record = mutate_at(&dir, id, change)?;
    super::presentation::record_saved(&record)
}

fn mutate_at(
    dir: &Path,
    id: &str,
    change: impl FnOnce(&Record, &mut TaskMetadata) -> Result<()>,
) -> Result<Record> {
    if !fs::symlink_metadata(dir)?.is_dir() {
        anyhow::bail!("task directory is not a regular directory");
    }
    // Same lock order as task actions: authoritative record, metadata, cache.
    // Thus archive cannot race admission or a newly waiting request.
    let record_lock = lock_file(&dir.join("record.lock"))?;
    record_lock.lock_exclusive()?;
    let metadata_lock = lock_file(&dir.join("metadata.lock"))?;
    metadata_lock.lock_exclusive()?;
    let bytes = read_bounded(&dir.join("record.json"), 1024 * 1024)?;
    let record: Record = serde_json::from_slice(&bytes)?;
    if record.id != id || record.schema_version != super::SCHEMA_VERSION {
        anyhow::bail!("invalid task record");
    }
    let mut metadata = read_at(dir)?;
    change(&record, &mut metadata)?;
    metadata.revision = metadata
        .revision
        .checked_add(1)
        .context("task metadata revision exhausted")?;
    super::write_private(&dir.join("metadata.json"), &serde_json::to_vec(&metadata)?)?;
    Ok(record)
}

fn lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    no_follow(&mut options);
    let lock = options.open(path)?;
    super::set_private(path, 0o600);
    Ok(lock)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.len() > limit {
        anyhow::bail!("task metadata is not a bounded regular file");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let mut bytes = Vec::new();
    options
        .open(path)?
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("task metadata exceeds the size limit");
    }
    Ok(bytes)
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(not(unix))]
    let _ = options;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn fixture() -> (PathBufCleanup, Record) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aishe-task-metadata-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        super::super::set_private(&path, 0o700);
        let record = super::super::tests::fixture_record();
        fs::write(
            path.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        fs::write(path.join("request.txt"), &record.objective).unwrap();
        (PathBufCleanup(path), record)
    }

    struct PathBufCleanup(std::path::PathBuf);
    impl Drop for PathBufCleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn renaming_and_pinning_persist_without_rewriting_the_request() {
        let (dir, record) = fixture();
        let before = fs::read(dir.0.join("record.json")).unwrap();
        mutate_at(&dir.0, &record.id, |_, metadata| {
            metadata.title = validated_name("Release checks").unwrap();
            metadata.pinned = true;
            Ok(())
        })
        .unwrap();
        let metadata = read_at(&dir.0).unwrap();
        assert_eq!(metadata.title.as_deref(), Some("Release checks"));
        assert!(metadata.pinned);
        assert_eq!(fs::read(dir.0.join("record.json")).unwrap(), before);
        assert_eq!(
            fs::read_to_string(dir.0.join("request.txt")).unwrap(),
            record.objective
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.0.join("metadata.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn review_is_exact_and_independent_of_plan_activity_and_preferences() {
        let (_, mut record) = fixture();
        record.result_revision = 3;
        let reviewed = result_revision(&record);
        record.updated_at_ms = 999;
        record.plan_revision = 7;
        assert_eq!(result_revision(&record), reviewed);
        record.result_revision += 1;
        assert_ne!(result_revision(&record), reviewed);
        record.state = State::Running;
        assert!(result_revision(&record).is_empty());
    }

    #[test]
    fn terminal_archive_policy_never_hides_live_or_waiting_work() {
        let (dir, mut record) = fixture();
        for state in [State::Starting, State::Running, State::Waiting] {
            record.state = state;
            fs::write(
                dir.0.join("record.json"),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
            assert!(
                mutate_at(&dir.0, &record.id, |record, metadata| set_archived(
                    record, metadata, true
                ))
                .is_err()
            );
            assert!(!read_at(&dir.0).unwrap().archived);
        }
        record.state = State::Completed;
        fs::write(
            dir.0.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        mutate_at(&dir.0, &record.id, |record, metadata| {
            set_archived(record, metadata, true)
        })
        .unwrap();
        let metadata = read_at(&dir.0).unwrap();
        assert!(metadata.archived);
        assert_eq!(
            metadata.archived_revision.as_deref(),
            Some(result_revision(&record).as_str())
        );
    }

    #[test]
    fn reviewing_old_details_never_acknowledges_a_newer_attempt() {
        let (dir, mut record) = fixture();
        let entry = TaskEntry::from_record(&record);
        mutate_at(&dir.0, &record.id, |record, metadata| {
            review_exact(record, metadata, &entry);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            read_at(&dir.0).unwrap().reviewed_revision.as_deref(),
            Some(entry.result_revision.as_str())
        );
        record.result_revision += 1;
        fs::write(
            dir.0.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        mutate_at(&dir.0, &record.id, |record, metadata| {
            review_exact(record, metadata, &entry);
            Ok(())
        })
        .unwrap();
        assert_ne!(
            read_at(&dir.0).unwrap().reviewed_revision.as_deref(),
            Some(result_revision(&record).as_str())
        );
    }

    #[test]
    fn reopened_metadata_keeps_review_shared_and_new_attempts_visible() {
        let (dir, mut record) = fixture();
        let entry = TaskEntry::with_metadata(&record, TaskMetadata::default());
        mutate_at(&dir.0, &record.id, |record, metadata| {
            review_exact(record, metadata, &entry);
            set_archived(record, metadata, true)?;
            metadata.title = Some("Pinned checks".into());
            metadata.pinned = true;
            Ok(())
        })
        .unwrap();
        let reopened = TaskEntry::with_metadata(&record, read_at(&dir.0).unwrap());
        assert!(super::super::SeenTasks::default().is_seen(&reopened));
        assert!(reopened.archived);
        assert!(reopened.pinned);
        assert_eq!(reopened.title, "Pinned checks");
        record.updated_at_ms += 100;
        record.plan_revision += 1;
        assert!(TaskEntry::with_metadata(&record, read_at(&dir.0).unwrap()).reviewed);
        record.state = State::Running;
        let resumed = TaskEntry::with_metadata(&record, read_at(&dir.0).unwrap());
        assert!(!resumed.archived);
        assert!(!resumed.reviewed);
        record.state = State::Completed;
        record.result_revision += 1;
        let later = TaskEntry::with_metadata(&record, read_at(&dir.0).unwrap());
        assert!(!later.archived);
        assert!(!super::super::SeenTasks::default().is_seen(&later));
        assert!(later.pinned);
        assert_eq!(later.title, "Pinned checks");
    }

    #[test]
    fn names_are_bounded_redacted_and_safe_for_the_terminal() {
        assert!(validated_name(&"x".repeat(121)).is_err());
        assert_eq!(validated_name(" ").unwrap(), None);
        let name = validated_name("TOKEN=secret\u{1b}[2J\n").unwrap().unwrap();
        assert!(!name.contains("secret"));
        assert!(!name.contains('\u{1b}'));
        assert!(!name.contains('\n'));
        assert!(name.contains("<redacted>"));
    }

    #[test]
    fn metadata_rejects_symlinks_and_oversized_records() {
        let (dir, _) = fixture();
        fs::write(
            dir.0.join("metadata.json"),
            vec![b'x'; MAX_METADATA_BYTES as usize + 1],
        )
        .unwrap();
        assert!(read_at(&dir.0).is_err());
        #[cfg(unix)]
        {
            fs::remove_file(dir.0.join("metadata.json")).unwrap();
            std::os::unix::fs::symlink(dir.0.join("request.txt"), dir.0.join("metadata.json"))
                .unwrap();
            assert!(read_at(&dir.0).is_err());
        }
    }
}
