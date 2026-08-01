//! Local, privacy-preserving product-discovery seen-state.
//!
//! This file records booleans only: never commands, prompts, paths, provider
//! data, or model output. Failure/typo hint rate limits remain separate because
//! their signatures have different lifetimes and reset semantics.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveryStatus {
    pub schema_version: u32,
    pub enabled: bool,
    pub launch_hint_seen: bool,
    pub first_answer_hint_seen: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DiscoveryState {
    #[serde(default = "schema_version")]
    schema_version: u32,
    #[serde(default)]
    launch_hint_seen: bool,
    #[serde(default)]
    first_answer_hint_seen: bool,
}

impl DiscoveryState {
    fn mark_first_answer_seen(&mut self) -> bool {
        if self.first_answer_hint_seen {
            return false;
        }
        self.first_answer_hint_seen = true;
        true
    }
}

fn schema_version() -> u32 {
    SCHEMA_VERSION
}

pub fn discovery_status(config: &crate::config::Config) -> Result<DiscoveryStatus> {
    let state = load()?;
    Ok(DiscoveryStatus {
        schema_version: SCHEMA_VERSION,
        enabled: config.aishe.discovery_hints,
        launch_hint_seen: state.launch_hint_seen,
        first_answer_hint_seen: state.first_answer_hint_seen,
    })
}

/// True only while the one-time launch hint is enabled and unseen. Read errors
/// fail closed so damaged local metadata never becomes recurring terminal noise.
pub fn launch_hint_pending(config: &crate::config::Config) -> bool {
    config.aishe.discovery_hints && load().is_ok_and(|state| !state.launch_hint_seen)
}

/// Mark the one-time launch hint as seen. Returns whether state changed.
pub fn mark_launch_hint_seen(config: &crate::config::Config) -> Result<bool> {
    if !config.aishe.discovery_hints {
        return Ok(false);
    }
    let mut state = load()?;
    if state.launch_hint_seen {
        return Ok(false);
    }
    state.launch_hint_seen = true;
    save(&state)?;
    Ok(true)
}

pub fn first_answer_hint_pending(config: &crate::config::Config) -> bool {
    config.aishe.discovery_hints && load().is_ok_and(|state| !state.first_answer_hint_seen)
}

/// Mark the first-answer next-action hint as seen. Returns whether state changed.
pub fn mark_first_answer_hint_seen(config: &crate::config::Config) -> Result<bool> {
    if !config.aishe.discovery_hints {
        return Ok(false);
    }
    let mut state = load()?;
    if !state.mark_first_answer_seen() {
        return Ok(false);
    }
    save(&state)?;
    Ok(true)
}

/// Static, non-color-dependent copy shared by stdout/stderr answer surfaces.
pub fn first_answer_next_action() -> &'static str {
    "AIShe tip · Next: ask a follow-up normally; prefix ! to force the shell; /help shows controls."
}

/// Atomically consume the one-time first-answer hint for presentation. Errors
/// fail closed so discovery metadata can never break an answer path.
pub fn take_first_answer_next_action(config: &crate::config::Config) -> Option<&'static str> {
    if !first_answer_hint_pending(config) {
        return None;
    }
    mark_first_answer_hint_seen(config)
        .ok()
        .filter(|changed| *changed)
        .map(|_| first_answer_next_action())
}

/// Reset discovery seen-state only. Configuration, history, sessions, failure
/// hints, and typo rate limits are deliberately untouched.
pub fn reset_discovery() -> Result<()> {
    save(&DiscoveryState {
        schema_version: SCHEMA_VERSION,
        launch_hint_seen: false,
        first_answer_hint_seen: false,
    })
}

fn path() -> Result<std::path::PathBuf> {
    Ok(crate::config::data_root()
        .context("no local data directory is available")?
        .join("aishe")
        .join("discovery-hints.json"))
}

fn load() -> Result<DiscoveryState> {
    let path = path()?;
    if !path.exists() {
        return Ok(DiscoveryState {
            schema_version: SCHEMA_VERSION,
            launch_hint_seen: false,
            first_answer_hint_seen: false,
        });
    }
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("inspecting discovery state {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 4_096 {
        anyhow::bail!("discovery state is not a bounded regular file");
    }
    let bytes = std::fs::read(&path)
        .with_context(|| format!("reading discovery state {}", path.display()))?;
    let state: DiscoveryState = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing discovery state {}", path.display()))?;
    if state.schema_version > SCHEMA_VERSION {
        anyhow::bail!(
            "discovery state schema {} is newer than supported schema {SCHEMA_VERSION}",
            state.schema_version
        );
    }
    Ok(state)
}

fn save(state: &DiscoveryState) -> Result<()> {
    let path = path()?;
    let parent = path
        .parent()
        .context("discovery state has no parent directory")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating discovery state directory {}", parent.display()))?;
    let mut document = serde_json::to_vec_pretty(state)?;
    document.push(b'\n');
    crate::config::write_atomic(&path, &document)
        .with_context(|| format!("writing discovery state {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_state_schema_contains_booleans_only() {
        let mut state = DiscoveryState {
            schema_version: SCHEMA_VERSION,
            launch_hint_seen: true,
            first_answer_hint_seen: true,
        };
        assert!(!state.mark_first_answer_seen());
        state.first_answer_hint_seen = false;
        assert!(state.mark_first_answer_seen());
        assert!(!state.mark_first_answer_seen());
        let document = serde_json::to_value(state).unwrap();
        assert_eq!(document["schema_version"], 1);
        assert_eq!(document["launch_hint_seen"], true);
        assert_eq!(document["first_answer_hint_seen"], true);
        assert_eq!(document.as_object().unwrap().len(), 3);
        assert!(!first_answer_next_action().contains('\u{1b}'));
    }
}
