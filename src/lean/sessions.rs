//! Durable lean session store — JSON/JSONL owned by the lean parent.
//!
//! Capability parity with `aishe sessions` without the OpenCode session map.
//! Layout (under XDG data):
//!   $XDG_DATA_HOME/aishe/lean-sessions/index.json
//!   $XDG_DATA_HOME/aishe/lean-sessions/<id>.jsonl
//!
//! `/reset` clears the in-memory Session and truncates the current durable
//! JSONL. List/resume/clear work from slash or CLI.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::providers::Msg;
use crate::session::Session;

const INDEX_NAME: &str = "index.json";
const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Meta {
    pub id: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub turns: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct Index {
    schema_version: u32,
    sessions: Vec<Meta>,
}

/// One lean interactive shell's durable conversation file.
#[derive(Debug)]
pub struct LeanSessionStore {
    root: PathBuf,
    id: String,
    path: PathBuf,
    meta: Meta,
}

impl LeanSessionStore {
    /// Create a fresh session id and open its JSONL under the lean store root.
    pub fn create(cwd: &str, model: &str) -> Self {
        let root = store_root();
        let _ = fs::create_dir_all(&root);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&root, fs::Permissions::from_mode(0o700));
        }
        let id = new_id();
        let path = root.join(format!("{id}.jsonl"));
        let now = now_ms();
        let meta = Meta {
            id: id.clone(),
            created_at_ms: now,
            updated_at_ms: now,
            title: String::new(),
            cwd: cwd.to_string(),
            model: model.to_string(),
            turns: 0,
        };
        let store = Self {
            root,
            id,
            path,
            meta,
        };
        store.persist_meta();
        let _ = crate::config::write_atomic(&store.path, b"");
        store
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn root_dir(&self) -> &Path {
        &self.root
    }

    /// Reload in-memory history from this session's JSONL (best-effort).
    pub fn load_into(&self, session: &mut Session) {
        *session = Session::load_persisted(&self.path);
    }

    /// Rewrite durable JSONL from the in-memory session and refresh index meta.
    pub fn persist(&mut self, session: &Session) {
        session.save_persisted(&self.path);
        self.meta.updated_at_ms = now_ms();
        self.meta.turns = session.turns();
        if self.meta.title.is_empty() {
            for msg in session.history() {
                if let Msg::User(text) = msg {
                    self.meta.title = text.chars().take(72).collect();
                    break;
                }
            }
        }
        self.persist_meta();
    }

    /// Clear in-memory + durable transcript for the current session id.
    pub fn clear(&mut self, session: &mut Session) {
        session.clear();
        let _ = crate::config::write_atomic(&self.path, b"");
        self.meta.updated_at_ms = now_ms();
        self.meta.turns = 0;
        self.meta.title.clear();
        self.persist_meta();
    }

    fn persist_meta(&self) {
        let mut index = load_index(&self.root);
        if let Some(existing) = index.sessions.iter_mut().find(|m| m.id == self.id) {
            *existing = self.meta.clone();
        } else {
            index.sessions.push(self.meta.clone());
        }
        if index.sessions.len() > 64 {
            let drop_n = index.sessions.len() - 64;
            let stale: Vec<String> = index
                .sessions
                .iter()
                .take(drop_n)
                .map(|m| m.id.clone())
                .collect();
            index.sessions.drain(0..drop_n);
            for id in stale {
                let _ = fs::remove_file(self.root.join(format!("{id}.jsonl")));
            }
        }
        save_index(&self.root, &index);
    }
}

/// XDG data root for lean sessions (overridable via `AISHE_LEAN_SESSIONS`).
pub fn store_root() -> PathBuf {
    if let Ok(p) = std::env::var("AISHE_LEAN_SESSIONS") {
        let p = p.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    crate::config::data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aishe")
        .join("lean-sessions")
}

pub fn list() -> Vec<Meta> {
    let root = store_root();
    let mut index = load_index(&root);
    index
        .sessions
        .retain(|m| root.join(format!("{}.jsonl", m.id)).is_file());
    index.sessions
}

pub fn load_session(id: &str) -> Option<Session> {
    let root = store_root();
    let path = root.join(format!("{id}.jsonl"));
    if !path.is_file() {
        return None;
    }
    Some(Session::load_persisted(&path))
}

#[allow(dead_code)]
pub fn clear_session(id: &str) -> bool {
    let root = store_root();
    let path = root.join(format!("{id}.jsonl"));
    let removed = match fs::remove_file(&path) {
        Ok(()) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => false,
    };
    let mut index = load_index(&root);
    let before = index.sessions.len();
    index.sessions.retain(|m| m.id != id);
    if index.sessions.len() != before {
        save_index(&root, &index);
    }
    removed || before != index.sessions.len()
}

#[allow(dead_code)]
pub fn clear_all() -> usize {
    let root = store_root();
    let mut index = load_index(&root);
    let n = index.sessions.len();
    for m in &index.sessions {
        let _ = fs::remove_file(root.join(format!("{}.jsonl", m.id)));
    }
    index.sessions.clear();
    save_index(&root, &index);
    n
}

fn load_index(root: &Path) -> Index {
    let path = root.join(INDEX_NAME);
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Index {
            schema_version: SCHEMA_VERSION,
            sessions: Vec::new(),
        },
    }
}

fn save_index(root: &Path, index: &Index) {
    let _ = fs::create_dir_all(root);
    let mut owned = index.clone();
    owned.schema_version = SCHEMA_VERSION;
    if let Ok(bytes) = serde_json::to_vec_pretty(&owned) {
        let _ = crate::config::write_atomic(&root.join(INDEX_NAME), &bytes);
    }
}

fn new_id() -> String {
    let ms = now_ms();
    let pid = std::process::id();
    format!("lean-{ms:x}-{pid:x}")
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
pub(crate) fn test_env_lock() -> std::sync::MutexGuard<'static, ()> {
    tests::env_lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    pub(crate) fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn create_persist_clear_round_trip() {
        let _guard = env_lock();
        let root = std::env::temp_dir().join(format!(
            "aishe-lean-sess-test-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::env::set_var("AISHE_LEAN_SESSIONS", &root);
        let mut store = LeanSessionStore::create("/tmp", "fake-model");
        let id = store.id().to_string();
        let mut session = Session::new(true);
        session.record_user("hello durable");
        session.record_assistant("hi back");
        store.persist(&session);
        assert!(store.path().is_file());
        let listed = list();
        assert!(listed.iter().any(|m| m.id == id));
        let loaded = load_session(&id).expect("load");
        assert_eq!(loaded.turns(), 1);
        store.clear(&mut session);
        assert!(session.history().is_empty());
        let again = Session::load_persisted(store.path());
        assert!(again.history().is_empty());
        assert!(
            clear_session(&id),
            "clear_session should remove {id} under {}",
            root.display()
        );
        std::env::remove_var("AISHE_LEAN_SESSIONS");
        let _ = fs::remove_dir_all(&root);
    }
}
