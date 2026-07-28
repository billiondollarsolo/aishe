//! Trust store for project-local config overlays.
//!
//! A project `.aishe/config.toml` (see `config::apply_project_overlay`) can ride
//! along in any cloned repository, so its *sensitive* keys (provider/endpoint,
//! MCP servers, audit logging, or safety toggles) are only honored after the
//! user explicitly trusts that file with `aishe trust`. Safe, cosmetic keys
//! always apply; this gate is only about the sensitive ones.
//!
//! Trust is keyed by the config file's absolute path plus a content hash, so
//! editing the file (or a `git pull` that changes it) drops trust and requires
//! re-running `aishe trust`. The hash is a stable, non-cryptographic checksum:
//! it detects changes, it is not a tamper-proof signature. Trusting a repo means
//! you vouch for it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Where the trust store lives: `$XDG_DATA_HOME/aishe/trusted-projects.json`.
fn store_path() -> PathBuf {
    crate::config::data_root()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("aishe")
        .join("trusted-projects.json")
}

/// FNV-1a 64-bit hash of the file contents, hex-encoded. Stable across versions
/// and platforms (so a Rust upgrade does not silently invalidate every trust),
/// and good enough to detect that a trusted file has changed. Not a security
/// signature.
pub fn content_hash(content: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    /// Absolute config-file path -> trusted content hash.
    #[serde(default)]
    trusted: BTreeMap<String, String>,
}

fn load() -> Store {
    let path = store_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Store::default(),
    }
}

fn save(store: &Store) -> Result<()> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating data dir {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(store).context("serializing trust store")?;
    std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// Normalize a config-file path to a stable key (absolute, symlinks resolved
/// where possible).
fn key_for(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// Is this exact file (path + current contents) trusted?
pub fn is_trusted(path: &Path, content: &str) -> bool {
    let store = load();
    store
        .trusted
        .get(&key_for(path))
        .map(|h| h == &content_hash(content))
        .unwrap_or(false)
}

/// Trust a config file at its current contents. Returns the stored key.
pub fn trust(path: &Path, content: &str) -> Result<String> {
    let key = key_for(path);
    let mut store = load();
    store.trusted.insert(key.clone(), content_hash(content));
    save(&store)?;
    Ok(key)
}

/// Drop trust for a config file. Returns whether an entry was removed.
pub fn untrust(path: &Path) -> Result<bool> {
    let key = key_for(path);
    let mut store = load();
    let removed = store.trusted.remove(&key).is_some();
    if removed {
        save(&store)?;
    }
    Ok(removed)
}

/// Drop all trusted entries. Returns how many were removed.
pub fn untrust_all() -> Result<usize> {
    let mut store = load();
    let n = store.trusted.len();
    store.trusted.clear();
    save(&store)?;
    Ok(n)
}

/// All trusted (path, hash) entries.
pub fn list() -> Vec<(String, String)> {
    load().trusted.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_hash_is_stable_and_sensitive() {
        // Stable for identical input, different for changed input.
        assert_eq!(content_hash("hello"), content_hash("hello"));
        assert_ne!(content_hash("hello"), content_hash("hello!"));
        // A known FNV-1a 64 vector ("" hashes to the offset basis).
        assert_eq!(content_hash(""), "cbf29ce484222325");
    }
}
