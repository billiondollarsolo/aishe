//! Durable mapping between an Aishe shell/workspace and OpenCode sessions.
//!
//! The mapping deliberately contains no credentials or local-control tokens.
//! One advisory lock protects the atomically replaced index so independent
//! Aishe hook processes from the same shell cannot create parallel sessions.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::agent::{BackendSession, ExecutionScope, Mode};

const STORE_SCHEMA_VERSION: u32 = 1;
const MAX_RECORDS: usize = 10_000;

#[derive(Clone, Debug)]
pub struct SessionStore {
    root: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionMapping {
    pub schema_version: u32,
    pub aishe_shell_id: String,
    pub workspace: PathBuf,
    pub backend: String,
    pub backend_session_id: String,
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Default, Serialize, Deserialize)]
struct SessionIndex {
    schema_version: u32,
    records: Vec<SessionMapping>,
}

impl SessionStore {
    pub fn from_default_root() -> Result<Self> {
        Ok(Self::new(
            super::super::supervisor::backend_root()?.join("sessions"),
        ))
    }

    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn resolve_workspace(path: &Path) -> Result<PathBuf> {
        let candidate = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let canonical = candidate
            .canonicalize()
            .with_context(|| format!("resolving workspace {}", candidate.display()))?;
        for ancestor in canonical.ancestors() {
            if [".git", ".hg", ".svn"]
                .iter()
                .any(|marker| ancestor.join(marker).exists())
            {
                return Ok(ancestor.to_path_buf());
            }
        }
        Ok(canonical)
    }

    pub fn find(&self, shell_id: &str, workspace: &Path) -> Result<Option<SessionMapping>> {
        validate_shell_id(shell_id)?;
        let workspace = Self::resolve_workspace(workspace)?;
        self.with_index(false, |index| {
            Ok(index
                .records
                .iter()
                .find(|record| {
                    record.aishe_shell_id == shell_id
                        && record.workspace == workspace
                        && record.backend == "opencode"
                })
                .cloned())
        })
    }

    pub fn bind(
        &self,
        shell_id: &str,
        session: &BackendSession,
        mode: Mode,
        scope: ExecutionScope,
    ) -> Result<SessionMapping> {
        validate_shell_id(shell_id)?;
        validate_backend_id(&session.id)?;
        let workspace = Self::resolve_workspace(&session.workspace)?;
        self.with_index(true, |index| {
            let now = unix_millis();
            if let Some(record) = index.records.iter_mut().find(|record| {
                record.aishe_shell_id == shell_id
                    && record.workspace == workspace
                    && record.backend == "opencode"
            }) {
                record.backend_session_id.clone_from(&session.id);
                record.mode = mode;
                record.scope = scope;
                record.updated_at = now;
                return Ok(record.clone());
            }
            if index.records.len() >= MAX_RECORDS {
                index.records.sort_by_key(|record| record.updated_at);
                index.records.remove(0);
            }
            let record = SessionMapping {
                schema_version: STORE_SCHEMA_VERSION,
                aishe_shell_id: shell_id.to_string(),
                workspace,
                backend: "opencode".into(),
                backend_session_id: session.id.clone(),
                mode,
                scope,
                created_at: now,
                updated_at: now,
            };
            index.records.push(record.clone());
            Ok(record)
        })
    }

    pub fn records(&self, shell_id: Option<&str>) -> Result<Vec<SessionMapping>> {
        if let Some(shell_id) = shell_id {
            validate_shell_id(shell_id)?;
        }
        self.with_index(false, |index| {
            Ok(index
                .records
                .iter()
                .filter(|record| shell_id.is_none_or(|value| record.aishe_shell_id == value))
                .cloned()
                .collect())
        })
    }

    /// Serialize the narrow find/create/bind window across Aishe processes.
    ///
    /// OpenCode 1.18.9 can reject a large cold burst of simultaneous
    /// `POST /session` requests. This separate advisory lock also prevents two
    /// hooks for the same new shell/workspace from both creating a conversation
    /// before either mapping is durable. Mapping reads/writes use their own lock,
    /// so the callback may safely call `find` and `bind`.
    pub fn serialize_creation<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating session directory {}", self.root.display()))?;
        crate::config::set_private_dir(&self.root);
        let path = self.root.join("creation.lock");
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&path)
            .with_context(|| format!("opening session-creation lock {}", path.display()))?;
        lock.lock_exclusive()
            .context("locking OpenCode session creation")?;
        let result = operation();
        FileExt::unlock(&lock).context("unlocking OpenCode session creation")?;
        result
    }

    fn with_index<T>(
        &self,
        write: bool,
        operation: impl FnOnce(&mut SessionIndex) -> Result<T>,
    ) -> Result<T> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("creating session directory {}", self.root.display()))?;
        crate::config::set_private_dir(&self.root);
        let lock_path = self.root.join("mappings.lock");
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = options
            .open(&lock_path)
            .with_context(|| format!("opening session lock {}", lock_path.display()))?;
        lock.lock_exclusive().context("locking session mappings")?;

        let path = self.root.join("mappings.json");
        let mut index = if path.exists() {
            let metadata = fs::metadata(&path)?;
            if metadata.len() > 8 * 1024 * 1024 {
                anyhow::bail!("OpenCode session mapping exceeds the 8 MiB limit");
            }
            serde_json::from_slice::<SessionIndex>(&fs::read(&path)?)
                .context("OpenCode session mapping is invalid")?
        } else {
            SessionIndex {
                schema_version: STORE_SCHEMA_VERSION,
                records: Vec::new(),
            }
        };
        if index.schema_version != STORE_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported OpenCode session mapping schema {}",
                index.schema_version
            );
        }
        index
            .records
            .retain(|record| record.schema_version == STORE_SCHEMA_VERSION);
        let result = operation(&mut index)?;
        if write {
            let bytes = serde_json::to_vec_pretty(&index)?;
            crate::config::write_atomic(&path, &bytes)
                .with_context(|| format!("writing session mapping {}", path.display()))?;
        }
        FileExt::unlock(&lock).context("unlocking session mappings")?;
        Ok(result)
    }
}

fn validate_shell_id(value: &str) -> Result<()> {
    if value.len() < 16
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid Aishe shell identity");
    }
    Ok(())
}

fn validate_backend_id(value: &str) -> Result<()> {
    if !value.starts_with("ses_")
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid OpenCode session identity");
    }
    Ok(())
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};
    use std::time::Duration;

    fn temp_store(name: &str) -> SessionStore {
        SessionStore::new(std::env::temp_dir().join(format!(
            "aishe-opencode-session-{name}-{}-{}",
            std::process::id(),
            unix_millis()
        )))
    }

    #[test]
    fn finds_nearest_vcs_root_and_reuses_mapping() {
        let store = temp_store("reuse");
        let workspace = store.root.join("project");
        let nested = workspace.join("src").join("nested");
        fs::create_dir_all(workspace.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        let session = BackendSession {
            id: "ses_one".into(),
            workspace: nested.clone(),
            backend: "opencode".into(),
        };
        store
            .bind(
                "0123456789abcdef",
                &session,
                Mode::Auto,
                ExecutionScope::Workspace,
            )
            .unwrap();
        let found = store
            .find("0123456789abcdef", &workspace.join("src"))
            .unwrap()
            .unwrap();
        assert_eq!(found.workspace, workspace.canonicalize().unwrap());
        assert_eq!(found.backend_session_id, "ses_one");
        fs::remove_dir_all(&store.root).unwrap();
    }

    #[test]
    fn replaces_binding_without_persisting_credentials() {
        let store = temp_store("replace");
        let workspace = store.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        for id in ["ses_one", "ses_two"] {
            store
                .bind(
                    "fedcba9876543210",
                    &BackendSession {
                        id: id.into(),
                        workspace: workspace.clone(),
                        backend: "opencode".into(),
                    },
                    Mode::Yolo,
                    ExecutionScope::Host,
                )
                .unwrap();
        }
        let records = store.records(Some("fedcba9876543210")).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].backend_session_id, "ses_two");
        let raw = fs::read_to_string(store.root.join("mappings.json")).unwrap();
        assert!(!raw.contains("token"));
        assert!(!raw.contains("password"));
        fs::remove_dir_all(&store.root).unwrap();
    }

    #[test]
    fn creation_lock_serializes_process_equivalent_callers() {
        let store = temp_store("creation-lock");
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(8));
        let threads = (0..8)
            .map(|_| {
                let store = store.clone();
                let active = Arc::clone(&active);
                let maximum = Arc::clone(&maximum);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    store
                        .serialize_creation(|| {
                            let count = active.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum.fetch_max(count, Ordering::SeqCst);
                            std::thread::sleep(Duration::from_millis(10));
                            active.fetch_sub(1, Ordering::SeqCst);
                            Ok(())
                        })
                        .unwrap();
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        fs::remove_dir_all(&store.root).unwrap();
    }
}
