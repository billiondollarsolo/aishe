//! Durable requests at native execution boundaries, with one execution owner.
//!
//! The kernel lock, rather than a PID or a model-written status, establishes
//! ownership. Requests name one random lease nonce so a stale terminal cannot
//! interrupt a later continuation of the same task.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};

const MAX_STATE_BYTES: u64 = 16 * 1024;
const MAX_EXPORT_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct ExportTransfer {
    task_id: String,
    nonce: String,
    environment: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Foreground,
    Background,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Self::Foreground => "foreground",
            Self::Background => "background",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Active,
    Queued,
    Parked,
    Finished,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub task_id: String,
    pub nonce: String,
    pub pid: u32,
    pub mode: Direction,
    pub status: Status,
    pub handoff_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<Direction>,
}

thread_local! {
    static CONTROL_PATH: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static LINKED_TASK: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn linked_task_id() -> Option<String> {
    LINKED_TASK.with(|value| value.borrow().clone())
}

/// A foreground continuation retains its existing mailbox and cancellation
/// identity without pretending its process is a detached worker.
pub struct LinkedTaskGuard(Option<String>);

impl LinkedTaskGuard {
    pub fn set(background_id: &str) -> Self {
        let previous = LINKED_TASK.with(|value| value.replace(Some(background_id.into())));
        Self(previous)
    }
}

impl Drop for LinkedTaskGuard {
    fn drop(&mut self) {
        LINKED_TASK.with(|value| {
            value.replace(self.0.take());
        });
    }
}

/// The lean IPC thread uses an explicit session path instead of mutating the
/// process environment while other shells and task workers are alive.
pub struct ControlGuard(Option<PathBuf>);

impl ControlGuard {
    pub fn set(path: &Path) -> Self {
        let previous = CONTROL_PATH.with(|value| value.replace(Some(path.to_path_buf())));
        Self(previous)
    }
}

impl Drop for ControlGuard {
    fn drop(&mut self) {
        CONTROL_PATH.with(|value| {
            value.replace(self.0.take());
        });
    }
}

fn control_path() -> Option<PathBuf> {
    CONTROL_PATH
        .with(|value| value.borrow().clone())
        .or_else(|| {
            std::env::var_os("AISHE_NATIVE_HANDOFF_CONTROL")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        })
}

pub fn control_available() -> bool {
    control_path().is_some()
}

pub struct Lease {
    root: PathBuf,
    lock: File,
    snapshot: Snapshot,
    control: Option<PathBuf>,
    parked: bool,
    exports: Option<PathBuf>,
}

impl Lease {
    pub fn acquire(task_id: &str, handoff_enabled: bool) -> Result<Self> {
        let worker_id = std::env::var("AISHE_BACKGROUND_TASK_ID")
            .ok()
            .filter(|value| !value.is_empty());
        let mode = if worker_id.is_some() {
            Direction::Background
        } else {
            Direction::Foreground
        };
        let background_task_id = worker_id.or_else(linked_task_id);
        let root = root()?;
        Self::acquire_in(
            root,
            task_id,
            mode,
            background_task_id,
            handoff_enabled,
            (mode == Direction::Foreground).then(control_path).flatten(),
        )
    }

    fn acquire_in(
        root: PathBuf,
        task_id: &str,
        mode: Direction,
        background_task_id: Option<String>,
        handoff_enabled: bool,
        control: Option<PathBuf>,
    ) -> Result<Self> {
        validate_id(task_id)?;
        ensure_private_directory(&root)?;
        if let Some(path) = &control {
            if !path.is_absolute() {
                anyhow::bail!("native handoff control path must be absolute");
            }
            validate_private_parent(path)?;
        }
        let lock = private_file(&root.join(format!("{task_id}.run.lock")))?;
        lock.try_lock_exclusive()
            .with_context(|| format!("task {task_id} already has an execution owner"))?;
        let mut random = [0_u8; 24];
        rand::rng().fill_bytes(&mut random);
        let snapshot = Snapshot {
            task_id: task_id.into(),
            nonce: random.iter().map(|byte| format!("{byte:02x}")).collect(),
            pid: std::process::id(),
            mode,
            status: Status::Active,
            handoff_enabled,
            background_task_id,
            requested: None,
        };
        let lease = Self {
            root,
            lock,
            snapshot,
            control,
            parked: false,
            exports: None,
        };
        let _guard = control_lock(&lease.root, task_id)?;
        if let Ok(retired) = read_snapshot(&state_path(&lease.root, task_id)) {
            if valid_nonce(&retired.nonce) {
                let _ = std::fs::remove_file(export_path(&lease.root, &retired));
            }
        }
        lease.publish(&lease.snapshot)?;
        Ok(lease)
    }

    /// Read a request only at a point with no unrecorded tool effect in flight.
    pub fn requested(&self) -> Result<Option<Direction>> {
        let fresh = read_snapshot(&state_path(&self.root, &self.snapshot.task_id))?;
        if fresh.task_id != self.snapshot.task_id
            || fresh.nonce != self.snapshot.nonce
            || fresh.pid != self.snapshot.pid
        {
            anyhow::bail!("native execution lease was replaced while task was running");
        }
        if matches!(fresh.status, Status::Active | Status::Queued) {
            Ok(fresh.requested)
        } else {
            Ok(None)
        }
    }

    /// Atomically choose normal completion or an already queued handoff.
    /// Closing requests while retaining the execution lock prevents a terminal
    /// from receiving a queued acknowledgment after the final boundary passed.
    pub fn close_requests_if_unrequested(&mut self) -> Result<bool> {
        let _guard = control_lock(&self.root, &self.snapshot.task_id)?;
        let mut fresh = read_snapshot(&state_path(&self.root, &self.snapshot.task_id))?;
        if fresh.task_id != self.snapshot.task_id
            || fresh.nonce != self.snapshot.nonce
            || fresh.pid != self.snapshot.pid
        {
            anyhow::bail!("native execution owner changed before final completion");
        }
        if fresh.requested.is_some() {
            return Ok(false);
        }
        if !matches!(fresh.status, Status::Active | Status::Finished) {
            anyhow::bail!("native execution lease cannot close from this state");
        }
        fresh.status = Status::Finished;
        self.publish(&fresh)?;
        self.snapshot = fresh;
        Ok(true)
    }

    /// Called only after the matching native checkpoint and counters are
    /// durable. Ownership remains held until the guard leaves the run wrapper.
    pub fn park(&mut self, direction: Direction) -> Result<()> {
        let _guard = control_lock(&self.root, &self.snapshot.task_id)?;
        let fresh = read_snapshot(&state_path(&self.root, &self.snapshot.task_id))?;
        if fresh.task_id != self.snapshot.task_id
            || fresh.nonce != self.snapshot.nonce
            || fresh.requested != Some(direction)
        {
            anyhow::bail!("native handoff request changed before its checkpoint was parked");
        }
        if fresh
            .background_task_id
            .as_deref()
            .is_some_and(|id| crate::background::is_cancelled(id).unwrap_or(true))
        {
            anyhow::bail!("task cancelled before the handoff checkpoint was released");
        }
        self.snapshot = fresh;
        self.snapshot.status = Status::Parked;
        self.publish(&self.snapshot)?;
        self.parked = true;
        Ok(())
    }

    /// A foreground claimant receives the worker's already filtered exports
    /// through this one-lease transfer. Values are never task history, model
    /// context, audit output, or a reusable saved workflow.
    pub fn transfer_environment(&mut self, environment: HashMap<String, String>) -> Result<()> {
        let bytes = serde_json::to_vec(&ExportTransfer {
            task_id: self.snapshot.task_id.clone(),
            nonce: self.snapshot.nonce.clone(),
            environment,
        })?;
        if bytes.len() as u64 > MAX_EXPORT_BYTES {
            anyhow::bail!("filtered execution exports exceed the handoff transfer bound");
        }
        let path = export_path(&self.root, &self.snapshot);
        crate::config::write_atomic(&path, &bytes)?;
        self.exports = Some(path);
        Ok(())
    }

    fn publish(&self, snapshot: &Snapshot) -> Result<()> {
        write_snapshot(&state_path(&self.root, &snapshot.task_id), snapshot)?;
        if let Some(path) = &self.control {
            write_snapshot(path, snapshot)?;
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.parked {
            if let Ok(_guard) = control_lock(&self.root, &self.snapshot.task_id) {
                if let Ok(mut fresh) =
                    read_snapshot(&state_path(&self.root, &self.snapshot.task_id))
                {
                    if fresh.nonce == self.snapshot.nonce {
                        fresh.status = Status::Finished;
                        let _ = self.publish(&fresh);
                    }
                }
            }
        }
        if !self.parked {
            if let Some(path) = &self.exports {
                let _ = std::fs::remove_file(path);
            }
        }
        let _ = FileExt::unlock(&self.lock);
    }
}

pub fn snapshot(task_id: &str) -> Result<Snapshot> {
    validate_id(task_id)?;
    let current = read_snapshot(&state_path(&root()?, task_id))?;
    if current.task_id != task_id {
        anyhow::bail!("native handoff state does not match its task ID");
    }
    Ok(current)
}

/// Consume the exports only for a parked foreground request whose execution
/// lock is released. A later lease cannot inherit a retired transfer.
pub fn take_environment(task_id: &str) -> Result<Option<HashMap<String, String>>> {
    validate_id(task_id)?;
    let task_root = root()?;
    take_environment_in(&task_root, task_id)
}

fn take_environment_in(task_root: &Path, task_id: &str) -> Result<Option<HashMap<String, String>>> {
    let _guard = control_lock(task_root, task_id)?;
    let current = read_snapshot(&state_path(task_root, task_id))?;
    if current.task_id != task_id {
        anyhow::bail!("native handoff state does not match its task ID");
    }
    if current.status != Status::Parked
        || current.requested != Some(Direction::Foreground)
        || !valid_nonce(&current.nonce)
        || !stopped_in(task_root, task_id)?
    {
        return Ok(None);
    }
    let path = export_path(task_root, &current);
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = match options.open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    validate_owned_file(&file, MAX_EXPORT_BYTES)?;
    let mut bytes = Vec::new();
    file.take(MAX_EXPORT_BYTES + 1).read_to_end(&mut bytes)?;
    std::fs::remove_file(&path)?;
    if bytes.len() as u64 > MAX_EXPORT_BYTES {
        anyhow::bail!("native handoff export transfer exceeds its bound");
    }
    let transfer: ExportTransfer = serde_json::from_slice(&bytes)?;
    if transfer.task_id != task_id || transfer.nonce != current.nonce {
        anyhow::bail!("native handoff exports belong to a retired execution lease");
    }
    Ok(Some(transfer.environment))
}

/// Cancellation and failed foreground admission can retire the local export
/// transfer without reading any of its values.
pub fn discard_environment(task_id: &str) -> Result<()> {
    let task_root = root()?;
    let current = snapshot(task_id)?;
    if valid_nonce(&current.nonce) {
        match std::fs::remove_file(export_path(&task_root, &current)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

pub fn request_task(task_id: &str, direction: Direction) -> Result<Snapshot> {
    validate_id(task_id)?;
    let task_root = root()?;
    let current = read_snapshot(&state_path(&task_root, task_id))?;
    if current.task_id != task_id {
        anyhow::bail!("native handoff state does not match its task ID");
    }
    request_in(&task_root, &current, direction)
}

/// A session control file can be stale after its task finishes; match its
/// nonce to the live task lease rather than retargeting a new continuation.
pub fn request_control(path: &Path, direction: Direction) -> Result<Snapshot> {
    let current = read_snapshot(path)?;
    validate_id(&current.task_id)?;
    request_in(&root()?, &current, direction)
}

fn request_in(task_root: &Path, expected: &Snapshot, direction: Direction) -> Result<Snapshot> {
    validate_id(&expected.task_id)?;
    let _guard = control_lock(task_root, &expected.task_id)?;
    let path = state_path(task_root, &expected.task_id);
    let mut current = read_snapshot(&path)?;
    if current.task_id != expected.task_id
        || current.nonce != expected.nonce
        || current.pid != expected.pid
    {
        anyhow::bail!("the foreground task changed; refresh before requesting a handoff");
    }
    if !current.handoff_enabled {
        anyhow::bail!("this native turn cannot hand off because its checkpoint or preview workspace is not transferable");
    }
    if direction == current.mode {
        anyhow::bail!(
            "task {} is already running in the {}",
            current.task_id,
            direction.label()
        );
    }
    if !matches!(current.status, Status::Active | Status::Queued)
        || stopped_in(task_root, &current.task_id)?
    {
        anyhow::bail!(
            "task {} has already stopped; resume its saved checkpoint",
            current.task_id
        );
    }
    if current
        .requested
        .is_some_and(|requested| requested != direction)
    {
        anyhow::bail!(
            "task {} already has a different handoff request",
            current.task_id
        );
    }
    current.requested = Some(direction);
    current.status = Status::Queued;
    write_snapshot(&path, &current)?;
    Ok(current)
}

/// This only observes release; callers must still atomically claim their
/// background record and then acquire the same native lease before resuming.
pub fn wait_stopped(task_id: &str, timeout: Duration) -> Result<()> {
    validate_id(task_id)?;
    let task_root = root()?;
    let start = Instant::now();
    loop {
        if stopped_in(&task_root, task_id)? {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            anyhow::bail!("handoff for task {task_id} is queued; waiting for its current provider or tool to finish. Retry foreground after it parks");
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn stopped_in(task_root: &Path, task_id: &str) -> Result<bool> {
    let lock = private_file(&task_root.join(format!("{task_id}.run.lock")))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&lock)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn root() -> Result<PathBuf> {
    Ok(crate::tasks::root()
        .context("native task storage is unavailable")?
        .join("handoff"))
}

fn state_path(task_root: &Path, task_id: &str) -> PathBuf {
    task_root.join(format!("{task_id}.state.json"))
}

fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 48 && nonce.chars().all(|character| character.is_ascii_hexdigit())
}

fn export_path(task_root: &Path, snapshot: &Snapshot) -> PathBuf {
    task_root.join(format!(
        "{}.{}.exports.json",
        snapshot.task_id, snapshot.nonce
    ))
}

fn validate_id(task_id: &str) -> Result<()> {
    if task_id.is_empty()
        || task_id.len() > 128
        || !task_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
    {
        anyhow::bail!("invalid native task ID");
    }
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = std::fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            anyhow::bail!("native handoff directory is not privately owned");
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn validate_private_parent(path: &Path) -> Result<()> {
    let parent = path.parent().context("native handoff path has no parent")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(parent)?;
        if !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            anyhow::bail!("native handoff control requires a private owned directory");
        }
    }
    Ok(())
}

fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options.open(path)?;
    validate_private_file(&file)?;
    Ok(file)
}

fn validate_private_file(file: &File) -> Result<()> {
    validate_owned_file(file, MAX_STATE_BYTES)
}

fn validate_owned_file(file: &File, limit: u64) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > limit {
        anyhow::bail!("native handoff file exceeds its bound or is not a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            anyhow::bail!("native handoff file is not privately owned");
        }
    }
    Ok(())
}

fn control_lock(task_root: &Path, task_id: &str) -> Result<File> {
    let file = private_file(&task_root.join(format!("{task_id}.control.lock")))?;
    FileExt::lock_exclusive(&file)?;
    Ok(file)
}

fn read_snapshot(path: &Path) -> Result<Snapshot> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let file = options
        .open(path)
        .context("no live native task is available for handoff")?;
    validate_private_file(&file)?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_STATE_BYTES {
        anyhow::bail!("native handoff state exceeds its bound");
    }
    let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
    validate_id(&snapshot.task_id)?;
    if !valid_nonce(&snapshot.nonce) {
        anyhow::bail!("native handoff state has an invalid execution nonce");
    }
    Ok(snapshot)
}

fn write_snapshot(path: &Path, snapshot: &Snapshot) -> Result<()> {
    crate::config::write_atomic(path, &serde_json::to_vec(snapshot)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aishe-handoff-test-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&path).unwrap();
            Self(path)
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

    #[test]
    fn exclusive_lease_fences_parallel_resume_and_releases_after_park() {
        let root = TestDir::new();
        let mut lease = Lease::acquire_in(
            root.path().to_path_buf(),
            "task-one",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        assert!(Lease::acquire_in(
            root.path().to_path_buf(),
            "task-one",
            Direction::Foreground,
            None,
            true,
            None
        )
        .is_err());
        let request = request_in(root.path(), &lease.snapshot, Direction::Background).unwrap();
        assert_eq!(request.status, Status::Queued);
        assert!(!stopped_in(root.path(), "task-one").unwrap());
        lease.park(Direction::Background).unwrap();
        assert!(!stopped_in(root.path(), "task-one").unwrap());
        drop(lease);
        assert!(stopped_in(root.path(), "task-one").unwrap());
        assert_eq!(
            read_snapshot(&state_path(root.path(), "task-one"))
                .unwrap()
                .status,
            Status::Parked
        );
    }

    #[test]
    fn stale_terminal_request_cannot_target_a_resumed_task() {
        let root = TestDir::new();
        let original = Lease::acquire_in(
            root.path().to_path_buf(),
            "task-two",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        let stale = original.snapshot.clone();
        drop(original);
        let resumed = Lease::acquire_in(
            root.path().to_path_buf(),
            "task-two",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        assert!(request_in(root.path(), &stale, Direction::Background).is_err());
        assert_eq!(resumed.requested().unwrap(), None);
    }

    #[test]
    fn duplicate_request_is_idempotent_and_finished_or_preview_task_rejects() {
        let root = TestDir::new();
        let lease = Lease::acquire_in(
            root.path().to_path_buf(),
            "task-three",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        let first = request_in(root.path(), &lease.snapshot, Direction::Background).unwrap();
        let second = request_in(root.path(), &lease.snapshot, Direction::Background).unwrap();
        assert_eq!(first.nonce, second.nonce);
        let stale = lease.snapshot.clone();
        drop(lease);
        assert!(request_in(root.path(), &stale, Direction::Background).is_err());
        let preview = Lease::acquire_in(
            root.path().to_path_buf(),
            "preview",
            Direction::Foreground,
            None,
            false,
            None,
        )
        .unwrap();
        assert!(request_in(root.path(), &preview.snapshot, Direction::Background).is_err());
    }

    #[test]
    fn final_completion_and_handoff_request_have_one_atomic_winner() {
        let root = TestDir::new();
        let mut completed = Lease::acquire_in(
            root.path().to_path_buf(),
            "completed",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        let terminal = completed.snapshot.clone();
        assert!(completed.close_requests_if_unrequested().unwrap());
        assert!(request_in(root.path(), &terminal, Direction::Background).is_err());
        assert!(!stopped_in(root.path(), "completed").unwrap());
        let mut queued = Lease::acquire_in(
            root.path().to_path_buf(),
            "queued",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        request_in(root.path(), &queued.snapshot, Direction::Background).unwrap();
        assert!(!queued.close_requests_if_unrequested().unwrap());
        assert_eq!(queued.requested().unwrap(), Some(Direction::Background));
        queued.park(Direction::Background).unwrap();
    }

    #[test]
    fn filtered_export_transfer_requires_released_lease_and_is_consumed_once() {
        let root = TestDir::new();
        let mut lease = Lease::acquire_in(
            root.path().to_path_buf(),
            "exports",
            Direction::Background,
            None,
            true,
            None,
        )
        .unwrap();
        request_in(root.path(), &lease.snapshot, Direction::Foreground).unwrap();
        let exports = HashMap::from([("VIRTUAL_ENV".into(), "/project/.venv".into())]);
        lease.transfer_environment(exports.clone()).unwrap();
        lease.park(Direction::Foreground).unwrap();
        assert!(take_environment_in(root.path(), "exports")
            .unwrap()
            .is_none());
        drop(lease);
        assert_eq!(
            take_environment_in(root.path(), "exports").unwrap(),
            Some(exports)
        );
        assert!(take_environment_in(root.path(), "exports")
            .unwrap()
            .is_none());
    }

    #[test]
    fn failed_or_retired_lease_cannot_replay_old_exports() {
        let root = TestDir::new();
        let mut lease = Lease::acquire_in(
            root.path().to_path_buf(),
            "retired",
            Direction::Background,
            None,
            true,
            None,
        )
        .unwrap();
        let exports = HashMap::from([("VIRTUAL_ENV".into(), "/old/.venv".into())]);
        lease.transfer_environment(exports).unwrap();
        let path = lease.exports.clone().unwrap();
        drop(lease);
        assert!(!path.exists());
        let mut parked = Lease::acquire_in(
            root.path().to_path_buf(),
            "retired",
            Direction::Background,
            None,
            true,
            None,
        )
        .unwrap();
        request_in(root.path(), &parked.snapshot, Direction::Foreground).unwrap();
        parked
            .transfer_environment(HashMap::from([(
                "VIRTUAL_ENV".into(),
                "/older/.venv".into(),
            )]))
            .unwrap();
        let parked_path = parked.exports.clone().unwrap();
        parked.park(Direction::Foreground).unwrap();
        drop(parked);
        let next = Lease::acquire_in(
            root.path().to_path_buf(),
            "retired",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        assert!(!parked_path.exists());
        assert!(take_environment_in(root.path(), "retired")
            .unwrap()
            .is_none());
        drop(next);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_and_world_readable_state_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let root = TestDir::new();
        let other = root.path().join("other");
        std::fs::write(&other, "unchanged").unwrap();
        symlink(&other, root.path().join("task.run.lock")).unwrap();
        assert!(Lease::acquire_in(
            root.path().to_path_buf(),
            "task",
            Direction::Foreground,
            None,
            true,
            None
        )
        .is_err());
        assert_eq!(std::fs::read_to_string(other).unwrap(), "unchanged");
        let lease = Lease::acquire_in(
            root.path().to_path_buf(),
            "valid",
            Direction::Foreground,
            None,
            true,
            None,
        )
        .unwrap();
        let path = state_path(root.path(), "valid");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(lease.requested().is_err());
    }
}
