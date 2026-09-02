//! Optional model/connection overrides for distinct agent workloads.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const NAMES: &[&str] = &["compose", "answer", "build", "review", "embed"];

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RoleConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
}

pub fn apply(
    config: &mut Config,
    role: Option<&str>,
    explicit_connection: bool,
    explicit_model: bool,
) -> Result<Option<String>> {
    let Some(role) = role else { return Ok(None) };
    let Some(binding) = config.roles.get(role).cloned() else {
        return Ok(Some(role.to_string()));
    };
    if !explicit_connection {
        if let Some(connection) = binding.connection.as_deref() {
            let id = config.resolve_connection_id(connection)?;
            config.select_connection(&id)?;
        }
    }
    if !explicit_model {
        if let Some(model) = binding.model.filter(|value| !value.trim().is_empty()) {
            set_model(config, &model);
        }
    }
    if let Some(reasoning) = binding.reasoning {
        validate_reasoning(&reasoning)?;
        config.set_active_reasoning_effort(reasoning);
    }
    Ok(Some(role.to_string()))
}

pub fn list(config: &Config, json: bool) -> Result<u8> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "roles": config.roles,
            }))?
        );
    } else {
        for name in NAMES {
            if let Some(role) = config.roles.get(*name) {
                println!(
                    "{name}: connection={} · model={} · reasoning={}",
                    role.connection.as_deref().unwrap_or("active"),
                    role.model.as_deref().unwrap_or("active"),
                    role.reasoning.as_deref().unwrap_or("active")
                );
            } else {
                println!("{name}: active selection");
            }
        }
    }
    Ok(0)
}

pub fn set(name: &str, binding: RoleConfig) -> Result<u8> {
    validate_name(name)?;
    if binding == RoleConfig::default() {
        anyhow::bail!("role set requires --connection, --model, or --reasoning");
    }
    if let Some(reasoning) = binding.reasoning.as_deref() {
        validate_reasoning(reasoning)?;
    }
    let mut config = Config::load_quiet()?.unwrap_or_default();
    if let Some(connection) = binding.connection.as_deref() {
        config
            .resolve_connection_id(connection)
            .with_context(|| format!("invalid connection for role {name}"))?;
    }
    config.roles.insert(name.to_string(), binding);
    config.save()?;
    println!("saved {name} role");
    Ok(0)
}

pub fn remove(name: &str) -> Result<u8> {
    validate_name(name)?;
    let mut config = Config::load_quiet()?.unwrap_or_default();
    if config.roles.remove(name).is_some() {
        config.save()?;
        println!("removed {name} role override");
    } else {
        println!("{name} already uses the active selection");
    }
    Ok(0)
}

fn set_model(config: &mut Config, model: &str) {
    let provider = config.active_provider_name().to_string();
    if let Some(connection) = config.active_connection_mut() {
        connection.settings.model = model.to_string();
    }
    if provider == "anthropic" {
        config.providers.anthropic.model = model.to_string();
    } else {
        config.providers.openai.model = model.to_string();
    }
}

fn validate_name(name: &str) -> Result<()> {
    if !NAMES.contains(&name) {
        anyhow::bail!("role must be one of: {}", NAMES.join(", "));
    }
    Ok(())
}

fn validate_reasoning(value: &str) -> Result<()> {
    if !matches!(
        value,
        "auto" | "none" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        anyhow::bail!("reasoning must be auto, none, low, medium, high, xhigh, or max");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_override_respects_explicit_model() {
        let mut config = Config::default();
        let original = config.active_model().to_string();
        config.roles.insert(
            "compose".into(),
            RoleConfig {
                model: Some("cheap-model".into()),
                ..RoleConfig::default()
            },
        );
        apply(&mut config, Some("compose"), false, true).unwrap();
        assert_eq!(config.active_model(), original);
        apply(&mut config, Some("compose"), false, false).unwrap();
        assert_eq!(config.active_model(), "cheap-model");
    }
}
