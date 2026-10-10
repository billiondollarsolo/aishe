//! Agent-launch choices and attachment validation, separate from CLI dispatch.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::config::Config;

/// Borrowed parsed intent, so resolving an agent does not depend on Clap.
pub struct Options<'a> {
    pub objective: &'a [String],
    pub background: bool,
    pub role: Option<&'a str>,
    pub connection: Option<&'a str>,
    pub model: Option<&'a str>,
    pub scope: Option<&'a str>,
    pub file: &'a [PathBuf],
    pub dir: &'a [PathBuf],
    pub diff: bool,
    pub clipboard: bool,
    pub no_isolation: bool,
    pub max_minutes: u32,
    pub max_turns: u32,
    pub max_cost: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct ResolvedAgent {
    pub objective: String,
    pub background: bool,
    pub role: String,
    pub connection: Option<String>,
    pub model: Option<String>,
    pub scope: String,
    pub no_isolation: bool,
    pub max_minutes: u32,
    pub max_turns: u32,
    pub max_cost: Option<f64>,
}

pub fn resolve(options: Options<'_>, config: &Config) -> Result<Option<ResolvedAgent>> {
    let guided = options.objective.is_empty();
    let objective = if guided {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!("agent objective is required outside an interactive terminal");
        }
        crate::promptui::header(
            "launch an AIShe agent",
            "Choose the work, authority, model role, and execution style in one place.",
            "Workspace scope and isolated background worktrees are the safe defaults.",
        );
        let Some(value) = crate::promptui::text(
            "Objective",
            "inspect this repository and recommend the next improvement",
            |value| {
                if value.trim().is_empty() || value.len() > 64 * 1024 {
                    anyhow::bail!("objective must contain 1..=65536 bytes")
                }
                Ok(())
            },
        )?
        else {
            return Ok(None);
        };
        if value == ":back" {
            return Ok(None);
        }
        value
    } else {
        options.objective.join(" ")
    };
    let background = if guided {
        let choices = vec![
            "Foreground · stream progress in this terminal".into(),
            "Background · isolated git worktree and inbox".into(),
        ];
        let crate::promptui::PickerResult::Use(index) =
            crate::promptui::filter_picker("Execution", &choices, usize::from(options.background))?
        else {
            return Ok(None);
        };
        index == 1
    } else {
        options.background
    };
    let role = if guided && options.role.is_none() {
        let choices = crate::roles::NAMES
            .iter()
            .map(|role| format!("{role} · workload-specific connection/model/reasoning"))
            .collect::<Vec<_>>();
        let default = crate::roles::NAMES
            .iter()
            .position(|role| *role == "build")
            .unwrap_or(0);
        let crate::promptui::PickerResult::Use(index) =
            crate::promptui::filter_picker("Model role", &choices, default)?
        else {
            return Ok(None);
        };
        crate::roles::NAMES[index].to_string()
    } else {
        options
            .role
            .map(str::to_owned)
            .unwrap_or_else(|| "build".into())
    };
    let scope = if guided && options.scope.is_none() {
        let choices = vec![
            "workspace · project-bound authority".into(),
            "host · explicit whole-machine authority".into(),
        ];
        let default = usize::from(config.backend.default_scope == "host");
        let crate::promptui::PickerResult::Use(index) =
            crate::promptui::filter_picker("Authority", &choices, default)?
        else {
            return Ok(None);
        };
        if index == 1 {
            "host".into()
        } else {
            "workspace".into()
        }
    } else {
        options
            .scope
            .map(str::to_owned)
            .unwrap_or_else(|| config.backend.default_scope.clone())
    };
    if options
        .max_cost
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        anyhow::bail!("--max-cost must be a finite non-negative number");
    }
    let mut objective = objective.trim().to_string();
    for path in options.file {
        objective.push(' ');
        objective.push_str(&attachment_reference("file", path)?);
    }
    for path in options.dir {
        objective.push(' ');
        objective.push_str(&attachment_reference("dir", path)?);
    }
    if options.diff {
        objective.push_str(" @diff");
    }
    if options.clipboard {
        objective.push_str(" @clipboard");
    }
    Ok(Some(ResolvedAgent {
        objective,
        background,
        role,
        connection: options.connection.map(str::to_owned),
        model: options.model.map(str::to_owned),
        scope,
        no_isolation: options.no_isolation,
        max_minutes: options.max_minutes,
        max_turns: options.max_turns,
        max_cost: options.max_cost,
    }))
}

fn attachment_reference(kind: &str, path: &std::path::Path) -> Result<String> {
    let value = path.to_str().context("attachment path is not UTF-8")?;
    if value.is_empty() || value.chars().any(char::is_control) {
        anyhow::bail!("attachment path is empty or contains control characters");
    }
    if !value.contains('"') {
        Ok(format!("@{kind}:\"{value}\""))
    } else if !value.contains('\'') {
        Ok(format!("@{kind}:'{value}'"))
    } else {
        anyhow::bail!("attachment paths containing both quote styles are not supported")
    }
}
