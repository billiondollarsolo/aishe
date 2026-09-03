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
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

const PRIMARY_ACTIONS: &[&str] = &[
    "agent",
    "ask",
    "last",
    "undo",
    "inbox",
    "task",
    "plan",
    "sessions",
    "reset",
    "mode",
    "model",
    "connection",
    "reasoning",
    "scope",
    "network",
    "context",
    "status",
    "usage",
    "settings",
    "help",
];

pub fn entries(_config: &Config) -> Vec<Entry> {
    let mut entries = crate::command_surface::COMMANDS
        .iter()
        .filter(|spec| matches!(spec.lifecycle, Lifecycle::Active))
        // Detail rows belong in their command's own picker; recursive or
        // argument-only shortcuts are noise in the top-level action palette.
        .filter(|spec| !matches!(spec.id, "palette" | "resume" | "role"))
        .filter_map(|spec| {
            // Fill the slash form when one exists: `/mode auto` is shell-local,
            // while `aishe mode auto` saves config and leaves the live prompt
            // on the old mode.
            let invocation = match spec.slash_aliases.first() {
                Some(alias) => format!("/{alias}"),
                None => {
                    let cli = spec.cli?;
                    let mut words = vec!["aishe", cli.command];
                    words.extend(cli.prefix_args);
                    words.join(" ")
                }
            };
            Some(Entry {
                id: spec.id.into(),
                label: format!(
                    "{invocation} — {} · {}",
                    spec.summary,
                    crate::product_help::effect_label(spec)
                ),
                invocation,
                effect: effect(spec.side_effects).into(),
                available: true,
                note: None,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        action_rank(&a.id)
            .cmp(&action_rank(&b.id))
            .then_with(|| a.invocation.cmp(&b.invocation))
    });
    entries
}

fn action_rank(id: &str) -> usize {
    PRIMARY_ACTIONS
        .iter()
        .position(|candidate| *candidate == id)
        .unwrap_or(usize::MAX)
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
    if !selected.available {
        anyhow::bail!(
            "{}",
            selected
                .note
                .as_deref()
                .unwrap_or("this action is unavailable")
        );
    }
    if let Some(path) = std::env::var_os("AISHE_PALETTE_FILE").filter(|value| !value.is_empty()) {
        let path = std::path::PathBuf::from(path);
        crate::config::write_atomic(&path, selected.invocation.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    } else if let Some(path) =
        std::env::var_os("AISHE_PENDING_FILE").filter(|value| !value.is_empty())
    {
        // Typed as `/palette` (no widget handoff): stage the choice on the next
        // prompt the same way a suggestion is staged.
        crate::config::write_atomic(
            std::path::Path::new(&path),
            format!("fill\n{}\n", selected.invocation).as_bytes(),
        )?;
    } else {
        println!("{}", selected.invocation);
    }
    Ok(0)
}

fn filtered<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    if query.trim().is_empty() {
        return entries.iter().collect();
    }
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
    fn entries_fill_slash_forms_and_name_their_effect() {
        let entries = entries(&Config::default());
        let mode = entries.iter().find(|e| e.id == "mode").expect("mode entry");
        assert_eq!(mode.invocation, "/mode");
        assert!(mode.label.starts_with("/mode — "), "{}", mode.label);
        assert!(mode.label.contains(" · "), "{}", mode.label);
        assert!(
            entries
                .iter()
                .all(|e| !e.label.starts_with("aishe ") || e.invocation.starts_with("aishe ")),
            "slash-aliased commands must fill their slash form"
        );
    }

    #[test]
    fn entries_come_from_registry_and_have_effects() {
        let entries = entries(&Config::default());
        assert!(entries.iter().any(|entry| entry.id == "status"));
        assert!(entries.iter().all(|entry| !entry.invocation.is_empty()));
        assert!(entries.iter().all(|entry| !entry.effect.is_empty()));
        assert_eq!(
            entries.first().map(|entry| entry.id.as_str()),
            Some("agent")
        );
        assert!(entries.iter().any(|entry| entry.id == "sessions"));
        assert!(entries.iter().all(|entry| entry.id != "resume"));
        assert!(entries.iter().all(|entry| !entry.id.contains(':')));
        let unique = entries
            .iter()
            .map(|entry| entry.invocation.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), entries.len());
        assert_eq!(filtered(&entries, "")[0].id, "agent");
    }
}
