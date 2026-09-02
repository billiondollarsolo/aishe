//! Durable mapping between an AIShe shell/workspace and OpenCode sessions.
//!
//! The mapping deliberately contains no credentials or local-control tokens.
//! One advisory lock protects the atomically replaced index so independent
//! AIShe hook processes from the same shell cannot create parallel sessions.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::agent::{BackendSession, ExecutionScope, Mode, NetworkPolicy};

const STORE_SCHEMA_VERSION: u32 = 3;
const AUTHORITY_CONTEXT_REVISION: u32 = 1;
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
    #[serde(default)]
    pub connection_id: String,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub repository_id: Option<PathBuf>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub head: Option<String>,
    pub mode: Mode,
    pub scope: ExecutionScope,
    #[serde(default = "default_network_policy")]
    pub network: NetworkPolicy,
    /// Version of the mode/scope/network context used when this backend
    /// conversation was created. v0.5.0 records deserialize as zero, forcing a
    /// one-time safe rotation instead of trusting their mutable scope label.
    #[serde(default)]
    pub authority_revision: u32,
    pub created_at: u128,
    pub updated_at: u128,
}

#[derive(Clone, Copy, Debug)]
pub struct SessionBinding<'a> {
    pub connection_id: &'a str,
    pub model_id: &'a str,
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
}

impl<'a> SessionBinding<'a> {
    pub fn new(
        connection_id: &'a str,
        model_id: &'a str,
        mode: Mode,
        scope: ExecutionScope,
        network: NetworkPolicy,
    ) -> Self {
        Self {
            connection_id,
            model_id,
            mode,
            scope,
            network,
        }
    }
}

impl SessionMapping {
    pub fn matches_authority(
        &self,
        mode: Mode,
        scope: ExecutionScope,
        network: NetworkPolicy,
    ) -> bool {
        self.authority_revision == AUTHORITY_CONTEXT_REVISION
            && self.mode == mode
            && self.scope == scope
            && self.network == network
    }
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

    pub fn find(
        &self,
        shell_id: &str,
        workspace: &Path,
        connection_id: &str,
        model_id: &str,
    ) -> Result<Option<SessionMapping>> {
        validate_shell_id(shell_id)?;
        let workspace = Self::resolve_workspace(workspace)?;
        let repo = repository_identity(&workspace);
        self.with_index(false, |index| {
            Ok(index
                .records
                .iter()
                .find(|record| {
                    record.aishe_shell_id == shell_id
                        && record.workspace == workspace
                        && record.backend == "opencode"
                        && record.connection_id == connection_id
                        && record.model_id == model_id
                        && same_conversation_branch(record, &repo)
                })
                .cloned())
        })
    }

    pub fn bind(
        &self,
        shell_id: &str,
        session: &BackendSession,
        binding: SessionBinding<'_>,
    ) -> Result<SessionMapping> {
        validate_shell_id(shell_id)?;
        validate_backend_id(&session.id)?;
        let workspace = Self::resolve_workspace(&session.workspace)?;
        let repo = repository_identity(&workspace);
        self.with_index(true, |index| {
            let now = unix_millis();
            if let Some(record) = index.records.iter_mut().find(|record| {
                record.aishe_shell_id == shell_id
                    && record.workspace == workspace
                    && record.backend == "opencode"
                    && record.connection_id == binding.connection_id
                    && record.model_id == binding.model_id
                    && same_conversation_branch(record, &repo)
            }) {
                record.backend_session_id.clone_from(&session.id);
                record.connection_id = binding.connection_id.to_string();
                record.model_id = binding.model_id.to_string();
                record.repository_id = repo.repository.clone();
                record.branch = repo.branch.clone();
                record.head = repo.head.clone();
                record.mode = binding.mode;
                record.scope = binding.scope;
                record.network = binding.network;
                record.authority_revision = AUTHORITY_CONTEXT_REVISION;
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
                connection_id: binding.connection_id.to_string(),
                model_id: binding.model_id.to_string(),
                repository_id: repo.repository,
                branch: repo.branch,
                head: repo.head,
                mode: binding.mode,
                scope: binding.scope,
                network: binding.network,
                authority_revision: AUTHORITY_CONTEXT_REVISION,
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

    /// Detach the active conversation for one live shell/workspace.
    ///
    /// The OpenCode conversation itself is deliberately not deleted. Returning
    /// the mapping lets the caller identify and resume the retained session,
    /// while the next natural-language turn creates a fresh conversation.
    pub fn reset(
        &self,
        shell_id: &str,
        workspace: &Path,
        connection_id: &str,
        model_id: &str,
    ) -> Result<Option<SessionMapping>> {
        validate_shell_id(shell_id)?;
        let workspace = Self::resolve_workspace(workspace)?;
        let repo = repository_identity(&workspace);
        self.with_index(true, |index| {
            let position = index.records.iter().position(|record| {
                record.aishe_shell_id == shell_id
                    && record.workspace == workspace
                    && record.backend == "opencode"
                    && record.connection_id == connection_id
                    && record.model_id == model_id
                    && same_conversation_branch(record, &repo)
            });
            Ok(position.map(|position| index.records.remove(position)))
        })
    }

    /// Serialize the narrow find/create/bind window across AIShe processes.
    ///
    /// OpenCode 1.18.27 can reject a large cold burst of simultaneous
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
        if matches!(index.schema_version, 1 | 2) {
            index.schema_version = STORE_SCHEMA_VERSION;
            for record in &mut index.records {
                record.schema_version = STORE_SCHEMA_VERSION;
            }
        } else if index.schema_version != STORE_SCHEMA_VERSION {
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

#[derive(Default)]
struct RepositoryIdentity {
    repository: Option<PathBuf>,
    branch: Option<String>,
    head: Option<String>,
}

fn repository_identity(workspace: &Path) -> RepositoryIdentity {
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|value| !value.is_empty())
    };
    let repository = git(&["rev-parse", "--git-common-dir"]).and_then(|value| {
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        };
        path.canonicalize().ok()
    });
    RepositoryIdentity {
        repository,
        branch: git(&["symbolic-ref", "--short", "-q", "HEAD"]),
        head: git(&["rev-parse", "HEAD"]),
    }
}

fn same_conversation_branch(record: &SessionMapping, current: &RepositoryIdentity) -> bool {
    if record.repository_id != current.repository {
        return false;
    }
    match (&record.branch, &current.branch) {
        (Some(old), Some(new)) => old == new,
        (None, None) if current.repository.is_some() => record.head == current.head,
        (None, None) => true,
        _ => false,
    }
}

fn validate_shell_id(value: &str) -> Result<()> {
    if value.len() < 16
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid AIShe shell identity");
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

fn default_network_policy() -> NetworkPolicy {
    NetworkPolicy::Deny
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
                SessionBinding::new(
                    "openai-work",
                    "gpt-test",
                    Mode::Auto,
                    ExecutionScope::Workspace,
                    NetworkPolicy::Deny,
                ),
            )
            .unwrap();
        let found = store
            .find(
                "0123456789abcdef",
                &workspace.join("src"),
                "openai-work",
                "gpt-test",
            )
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
                    SessionBinding::new(
                        "openai-work",
                        "gpt-test",
                        Mode::Yolo,
                        ExecutionScope::Host,
                        NetworkPolicy::Allow,
                    ),
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
    fn authority_changes_and_legacy_records_require_rotation() {
        let store = temp_store("authority");
        let workspace = store.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let mapping = store
            .bind(
                "0123456789abcdef",
                &BackendSession {
                    id: "ses_authority".into(),
                    workspace: workspace.clone(),
                    backend: "opencode".into(),
                },
                SessionBinding::new(
                    "openai-work",
                    "gpt-test",
                    Mode::Yolo,
                    ExecutionScope::Workspace,
                    NetworkPolicy::Deny,
                ),
            )
            .unwrap();
        assert!(mapping.matches_authority(
            Mode::Yolo,
            ExecutionScope::Workspace,
            NetworkPolicy::Deny
        ));
        assert!(!mapping.matches_authority(Mode::Yolo, ExecutionScope::Host, NetworkPolicy::Allow));
        assert!(!mapping.matches_authority(
            Mode::Auto,
            ExecutionScope::Workspace,
            NetworkPolicy::Deny
        ));

        let mut legacy = mapping;
        legacy.authority_revision = 0;
        assert!(!legacy.matches_authority(
            Mode::Yolo,
            ExecutionScope::Workspace,
            NetworkPolicy::Deny
        ));
        fs::remove_dir_all(&store.root).unwrap();
    }

    #[test]
    fn reset_detaches_mapping_without_deleting_session_identity() {
        let store = temp_store("reset");
        let workspace = store.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        store
            .bind(
                "fedcba9876543210",
                &BackendSession {
                    id: "ses_retained".into(),
                    workspace: workspace.clone(),
                    backend: "opencode".into(),
                },
                SessionBinding::new(
                    "openai-work",
                    "gpt-test",
                    Mode::Yolo,
                    ExecutionScope::Host,
                    NetworkPolicy::Allow,
                ),
            )
            .unwrap();

        let detached = store
            .reset("fedcba9876543210", &workspace, "openai-work", "gpt-test")
            .unwrap()
            .unwrap();
        assert_eq!(detached.backend_session_id, "ses_retained");
        assert!(store
            .find("fedcba9876543210", &workspace, "openai-work", "gpt-test",)
            .unwrap()
            .is_none());
        assert!(store
            .reset("fedcba9876543210", &workspace, "openai-work", "gpt-test",)
            .unwrap()
            .is_none());
        fs::remove_dir_all(&store.root).unwrap();
    }

    #[test]
    fn connection_and_model_selections_keep_independent_sessions() {
        let store = temp_store("selection-identity");
        let workspace = store.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        for (session_id, connection_id, model_id) in [
            ("ses_work", "openai-work", "gpt-test"),
            ("ses_personal", "openai-personal", "gpt-test"),
            ("ses_other_model", "openai-work", "gpt-other"),
        ] {
            store
                .bind(
                    "0123456789abcdef",
                    &BackendSession {
                        id: session_id.into(),
                        workspace: workspace.clone(),
                        backend: "opencode".into(),
                    },
                    SessionBinding::new(
                        connection_id,
                        model_id,
                        Mode::Auto,
                        ExecutionScope::Workspace,
                        NetworkPolicy::Deny,
                    ),
                )
                .unwrap();
        }
        assert_eq!(store.records(None).unwrap().len(), 3);
        assert_eq!(
            store
                .find("0123456789abcdef", &workspace, "openai-work", "gpt-test")
                .unwrap()
                .unwrap()
                .backend_session_id,
            "ses_work"
        );
        assert_eq!(
            store
                .find(
                    "0123456789abcdef",
                    &workspace,
                    "openai-personal",
                    "gpt-test"
                )
                .unwrap()
                .unwrap()
                .backend_session_id,
            "ses_personal"
        );
        fs::remove_dir_all(&store.root).unwrap();
    }

    #[test]
    fn schema_two_migrates_and_branch_switches_keep_separate_sessions() {
        let store = temp_store("branch-migration");
        let workspace = store.root.join("workspace");
        fs::create_dir_all(&workspace).unwrap();
        let git = |args: &[&str]| {
            assert!(std::process::Command::new("git")
                .args(args)
                .current_dir(&workspace)
                .status()
                .unwrap()
                .success());
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.email", "aishe@example.invalid"]);
        git(&["config", "user.name", "AIShe Test"]);
        fs::write(workspace.join("tracked"), "base\n").unwrap();
        git(&["add", "tracked"]);
        git(&["commit", "-qm", "base"]);

        let bind = |id: &str| {
            store
                .bind(
                    "0123456789abcdef",
                    &BackendSession {
                        id: id.into(),
                        workspace: workspace.clone(),
                        backend: "opencode".into(),
                    },
                    SessionBinding::new(
                        "openai-work",
                        "gpt-test",
                        Mode::Auto,
                        ExecutionScope::Workspace,
                        NetworkPolicy::Deny,
                    ),
                )
                .unwrap();
        };
        bind("ses_main");

        let path = store.root.join("mappings.json");
        let legacy = fs::read_to_string(&path)
            .unwrap()
            .replace("\"schema_version\": 3", "\"schema_version\": 2");
        fs::write(&path, legacy).unwrap();
        assert_eq!(store.records(None).unwrap().len(), 1);

        git(&["switch", "-qc", "feature"]);
        assert!(store
            .find("0123456789abcdef", &workspace, "openai-work", "gpt-test")
            .unwrap()
            .is_none());
        bind("ses_feature");
        git(&["switch", "-q", "main"]);
        assert_eq!(
            store
                .find("0123456789abcdef", &workspace, "openai-work", "gpt-test")
                .unwrap()
                .unwrap()
                .backend_session_id,
            "ses_main"
        );
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
