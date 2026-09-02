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
                available: true,
                note: None,
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
            invocation: format!("aishe connection pick {}", shell_quote(id)),
            effect: "shell_state".into(),
            available: true,
            note: None,
        });
    }
    let live_ready = crate::capabilities::load(config).is_some_and(|report| report.live_verified());
    let mut add = |id: String,
                   invocation: String,
                   summary: String,
                   effect: &str,
                   available: bool,
                   note: Option<String>| {
        entries.push(Entry {
            id,
            label: format!("{invocation} — {summary}"),
            invocation,
            effect: effect.into(),
            available,
            note,
        });
    };
    add(
        "agent:new".into(),
        "aishe agent".into(),
        "guided foreground/background agent launcher".into(),
        "mixed",
        live_ready,
        (!live_ready).then(|| "model tools are not live-verified; run aishe test --live".into()),
    );
    for role in crate::roles::NAMES {
        add(
            format!("role:{role}"),
            format!("aishe agent --role {role}"),
            format!("launch with the {role} workload role"),
            "mixed",
            live_ready,
            (!live_ready).then(|| "provider not live-verified".into()),
        );
    }
    for (name, server) in &config.mcp_servers {
        add(
            format!("mcp:{name}"),
            format!("aishe mcp show {}", shell_quote(name)),
            format!(
                "inspect {} MCP server",
                if server.enabled {
                    "enabled"
                } else {
                    "disabled"
                }
            ),
            "read_only",
            true,
            None,
        );
    }
    if let Some(report) = crate::capabilities::load(config) {
        for model in report.models {
            add(
                format!("model:{model}"),
                format!("aishe model {}", shell_quote(&model)),
                "switch this shell's model".into(),
                "shell_state",
                true,
                None,
            );
        }
    }
    for (id, state, objective) in crate::background::palette_summaries() {
        add(
            format!("task:{id}"),
            format!("aishe task show {}", shell_quote(&id)),
            format!(
                "{:?} · {}",
                state,
                crate::commands::display_safe(&objective.chars().take(52).collect::<String>())
            ),
            "read_only",
            true,
            None,
        );
        if matches!(
            state,
            crate::background::State::Completed
                | crate::background::State::Failed
                | crate::background::State::Interrupted
        ) {
            add(
                format!("review:{id}"),
                format!("aishe task review {}", shell_quote(&id)),
                "review, apply, rework, or reject isolated changes".into(),
                "mixed",
                true,
                None,
            );
        }
    }
    for record in crate::backend::opencode::session::SessionStore::from_default_root()
        .and_then(|store| store.records(None))
        .unwrap_or_default()
    {
        add(
            format!("session:{}", record.backend_session_id),
            format!("aishe resume {}", shell_quote(&record.backend_session_id)),
            format!(
                "resume {} · {}",
                record.model_id,
                record.workspace.display()
            ),
            "conversation_state",
            true,
            None,
        );
    }
    for entry in &mut entries {
        if !entry.available {
            entry.label.push_str(" · needs live verification");
        }
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

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
        assert!(entries.iter().any(|entry| entry.id == "agent:new"));
    }

    #[test]
    fn contextual_invocations_are_shell_quoted() {
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
