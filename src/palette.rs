//! Command palette generated from the canonical command registry.

use anyhow::{Context, Result};

use crate::command_surface::{Lifecycle, SideEffectClass};
use crate::config::Config;

#[derive(Clone, Debug, serde::Serialize)]
pub struct Entry {
    pub id: String,
    pub label: String,
    pub invocation: String,
    pub effect: String,
}

pub fn entries(config: &Config) -> Vec<Entry> {
    let mut entries = crate::command_surface::COMMANDS
        .iter()
        .filter(|spec| matches!(spec.lifecycle, Lifecycle::Active))
        .filter_map(|spec| {
            let cli = spec.cli?;
            let mut words = vec!["aishe", cli.command];
            words.extend(cli.prefix_args);
            Some(Entry {
                id: spec.id.into(),
                label: format!("{} — {}", words.join(" "), spec.summary),
                invocation: words.join(" "),
                effect: effect(spec.side_effects).into(),
            })
        })
        .collect::<Vec<_>>();
    for (id, connection) in &config.connections {
        entries.push(Entry {
            id: format!("connection:{id}"),
            label: format!(
                "aishe connection pick {id} — switch to {}",
                connection.label
            ),
            invocation: format!("aishe connection pick {id}"),
            effect: "shell_state".into(),
        });
    }
    entries.sort_by(|a, b| a.invocation.cmp(&b.invocation));
    entries
}

pub fn command(config: &Config, query: Option<&str>, json: bool) -> Result<u8> {
    let entries = entries(config);
    if json {
        let matches = filtered(&entries, query.unwrap_or(""));
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "entries": matches,
            }))?
        );
        return Ok(0);
    }
    if let Some(query) = query {
        for entry in filtered(&entries, query) {
            println!("{}\t{}\t{}", entry.invocation, entry.effect, entry.label);
        }
        return Ok(0);
    }
    let labels = entries
        .iter()
        .map(|entry| entry.label.clone())
        .collect::<Vec<_>>();
    let crate::promptui::PickerResult::Use(index) =
        crate::promptui::filter_picker("AIShe command palette", &labels, 0)?
    else {
        return Ok(0);
    };
    let selected = entries
        .get(index)
        .context("palette selection is out of range")?;
    if let Some(path) = std::env::var_os("AISHE_PALETTE_FILE").filter(|value| !value.is_empty()) {
        let path = std::path::PathBuf::from(path);
        crate::config::write_atomic(&path, selected.invocation.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    } else {
        println!("{}", selected.invocation);
    }
    Ok(0)
}

fn filtered<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let labels = entries.iter().map(|entry| entry.label.clone()).collect();
    crate::fuzzy::rank(labels, query)
        .into_iter()
        .filter_map(|label| entries.iter().find(|entry| entry.label == label))
        .collect()
}

fn effect(effect: SideEffectClass) -> &'static str {
    match effect {
        SideEffectClass::ReadOnly => "read_only",
        SideEffectClass::ShellState => "shell_state",
        SideEffectClass::PersistentConfig => "persistent_config",
        SideEffectClass::Credentials => "credentials",
        SideEffectClass::ConversationState => "conversation_state",
        SideEffectClass::Mixed => "mixed",
        SideEffectClass::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_come_from_registry_and_have_effects() {
        let entries = entries(&Config::default());
        assert!(entries.iter().any(|entry| entry.id == "status"));
        assert!(entries.iter().all(|entry| !entry.invocation.is_empty()));
        assert!(entries.iter().all(|entry| !entry.effect.is_empty()));
    }
}
