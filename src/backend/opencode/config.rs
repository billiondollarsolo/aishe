//! Deterministic OpenCode configuration generated only from trusted Aishe
//! configuration. Project/user OpenCode configuration is never merged.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{Config, ProviderConfig};

pub const PROVIDER_KEY_ENV: &str = "AISHE_PROVIDER_API_KEY";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSpec {
    pub provider_id: String,
    pub model_id: String,
    pub npm: String,
    pub base_url: String,
    pub requires_auth: bool,
    pub price: Option<crate::usage::Price>,
}

/// Contains a secret and therefore deliberately does not implement Debug or
/// Serialize. It exists only in the foreground bootstrap process and the
/// supervisor's short-lived launch memory.
pub struct ProviderLaunch {
    pub spec: ProviderSpec,
    pub api_key: Option<String>,
}

impl ProviderLaunch {
    pub fn from_aishe(config: &Config) -> Result<Self> {
        let provider = active_provider(config);
        let resolved = crate::credentials::resolve(provider)?;
        let api_key = resolved.into_secret();
        if provider.requires_auth() && api_key.is_none() {
            let profile = provider.credential_profile();
            anyhow::bail!(
                "API key missing for credential profile '{profile}' — run `aishe auth set {profile}`"
            );
        }
        Ok(Self {
            spec: ProviderSpec::from_aishe(config)?,
            api_key,
        })
    }
}

impl ProviderSpec {
    pub fn from_aishe(config: &Config) -> Result<Self> {
        let provider = active_provider(config);
        validate_model(&provider.model)?;
        let base = crate::provider_catalog::normalize_base_url(&provider.base_url);
        let parsed = url::Url::parse(&base).context("active provider base URL is invalid")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            anyhow::bail!("active provider base URL must use HTTP or HTTPS");
        }
        let official_openai = parsed.host_str() == Some("api.openai.com");
        let local = crate::config::is_loopback_url(&base);
        let anthropic = config.aishe.provider == "anthropic";
        let responses =
            provider.transport == "responses" || (provider.transport == "auto" && official_openai);
        let (provider_id, npm) = if anthropic {
            ("aishe-anthropic", "@ai-sdk/anthropic")
        } else if local {
            ("aishe-local", "@ai-sdk/openai-compatible")
        } else if official_openai && responses {
            ("aishe-openai", "@ai-sdk/openai")
        } else if responses {
            ("aishe-compatible", "@ai-sdk/openai")
        } else {
            ("aishe-compatible", "@ai-sdk/openai-compatible")
        };
        Ok(Self {
            provider_id: provider_id.into(),
            model_id: provider.model.clone(),
            npm: npm.into(),
            base_url: append_v1(&base),
            requires_auth: provider.requires_auth(),
            price: crate::usage::price_for(&provider.model, &config.pricing),
        })
    }
}

pub fn generated_config(plugin_path: &Path, provider: Option<&ProviderSpec>) -> Result<Value> {
    let plugin_url = url::Url::from_file_path(plugin_path)
        .map_err(|_| anyhow::anyhow!("trusted plugin path cannot be represented as a file URL"))?;
    let disabled_tools = disabled_builtin_tools();
    let mut config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "share": "disabled",
        "autoupdate": false,
        "snapshot": false,
        "mcp": {},
        "lsp": false,
        "formatter": false,
        "instructions": [],
        "skills": {"paths":[]},
        "plugin": [plugin_url.as_str()],
        "permission": {"*":"deny"},
        "tools": disabled_tools,
        "default_agent": "aishe-suggest",
        "subagent_depth": 1,
        "agent": {},
        "compaction": {"auto":true,"prune":true},
        "tool_output": {"max_lines":2000,"max_bytes":524288}
    });

    let mut agent_config = Map::new();
    agent_config.insert(
        "aishe-suggest".into(),
        serde_json::json!({
            "description":"Aishe one-turn answer and shell-command suggestion agent.",
            "mode":"primary",
            "prompt":SUGGEST_PROMPT,
            "permission":{"*":"deny"},
            "tools":disabled_builtin_tools(),
            "steps":1
        }),
    );
    agent_config.insert(
        "aishe-auto".into(),
        serde_json::json!({
            "description":"Aishe approval-gated command-line agent.",
            "mode":"primary",
            "prompt":AUTO_PROMPT,
            "permission":bridge_permissions(true),
            "tools":disabled_builtin_tools(),
            "steps":50
        }),
    );
    agent_config.insert(
        "aishe-yolo".into(),
        serde_json::json!({
            "description":"Aishe scope-authorized autonomous command-line agent.",
            "mode":"primary",
            "prompt":YOLO_PROMPT,
            "permission":bridge_permissions(true),
            "tools":disabled_builtin_tools(),
            "steps":100
        }),
    );
    for name in ["general", "explore"] {
        agent_config.insert(
            name.into(),
            serde_json::json!({
                "description":"Aishe child agent. All host effects remain behind the foreground Aishe bridge.",
                "mode":"subagent",
                "prompt":CHILD_PROMPT,
                "permission":bridge_permissions(false),
                "tools":disabled_builtin_tools(),
                "steps":50
            }),
        );
    }
    config["agent"] = Value::Object(agent_config);

    if let Some(provider) = provider {
        let mut options = serde_json::json!({"baseURL":provider.base_url});
        if provider.requires_auth {
            options["apiKey"] = Value::String(format!("{{env:{PROVIDER_KEY_ENV}}}"));
        }
        config["provider"] = serde_json::json!({
            provider.provider_id.clone(): {
                "id": provider.provider_id,
                "name": "Aishe managed provider",
                "npm": provider.npm,
                "options": options,
                "models": {
                    provider.model_id.clone(): {
                        "id": provider.model_id,
                        "name": provider.model_id,
                        "tool_call": true
                    }
                }
            }
        });
        if let Some(price) = provider.price {
            config["provider"][&provider.provider_id]["models"][&provider.model_id]["cost"] =
                serde_json::json!({"input":price.input,"output":price.output});
        }
        config["enabled_providers"] = serde_json::json!([provider.provider_id]);
        let model = format!("{}/{}", provider.provider_id, provider.model_id);
        config["model"] = Value::String(model.clone());
        config["small_model"] = Value::String(model.clone());
        for name in [
            "aishe-suggest",
            "aishe-auto",
            "aishe-yolo",
            "general",
            "explore",
        ] {
            config["agent"][name]["model"] = Value::String(model.clone());
        }
    }
    Ok(config)
}

fn active_provider(config: &Config) -> &ProviderConfig {
    if config.aishe.provider == "openai" {
        &config.providers.openai
    } else {
        &config.providers.anthropic
    }
}

fn append_v1(base: &str) -> String {
    format!("{}/v1", base.trim_end_matches('/'))
}

fn bridge_permissions(subagents: bool) -> Value {
    let mut permissions = serde_json::json!({
        "*": "deny",
        "aishe_*": "allow",
        "todowrite": "allow",
        "todoread": "allow"
    });
    if subagents {
        permissions["task"] = Value::String("allow".into());
    }
    permissions
}

const SUGGEST_PROMPT: &str = r#"You are Aishe's concise command-line assistant.
Answer the user's full request. If the best response is one runnable shell command,
return type=command and put only that command in command. Otherwise return
type=answer, leave command empty, and put the answer in explanation. Never invent
a command for a factual or conversational question. The required JSON schema is
the authoritative output contract."#;

const AUTO_PROMPT: &str = r#"You are Aishe's approval-gated command-line agent.
Work to completion using only aishe_* proxy tools. Safe read-only actions may run
immediately; Aishe decides whether any other action needs approval. Never request
OpenCode built-in host tools or claim an action happened without a successful tool
result. Keep terminal-facing explanations concise."#;

const YOLO_PROMPT: &str = r#"You are Aishe's autonomous command-line agent.
Work to completion using only aishe_* proxy tools and optional child agents.
Aishe has already obtained the user's session-scoped authorization and enforces
workspace/host/network boundaries outside the model. Never ask for per-action
approval, never attempt to widen scope, and never claim an action happened without
a successful tool result. Inspect, edit, test, and verify your work."#;

const CHILD_PROMPT: &str = r#"You are an Aishe child agent. Complete the delegated
subtask using only aishe_* proxy tools. You inherit the parent Aishe lease and may
not widen its workspace, host, or network authority. Report concise evidence."#;

fn disabled_builtin_tools() -> Value {
    serde_json::json!({
        "bash": false,
        "read": false,
        "write": false,
        "edit": false,
        "patch": false,
        "apply_patch": false,
        "glob": false,
        "grep": false,
        "webfetch": false,
        "websearch": false,
        "skill": false,
        "lsp": false,
        "list_mcp_resources": false,
        "list_mcp_resource_templates": false,
        "read_mcp_resource": false
    })
}

fn validate_model(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 512 {
        anyhow::bail!("active model must contain 1–512 characters");
    }
    if value.chars().any(char::is_control) {
        anyhow::bail!("active model cannot contain control characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_for(
        provider_name: &str,
        base_url: &str,
        transport: &str,
        auth_required: bool,
    ) -> ProviderSpec {
        let mut config = Config::default();
        config.aishe.provider = if provider_name == "anthropic" {
            "anthropic"
        } else {
            "openai"
        }
        .into();
        let provider = if config.aishe.provider == "anthropic" {
            &mut config.providers.anthropic
        } else {
            &mut config.providers.openai
        };
        provider.base_url = base_url.into();
        provider.transport = transport.into();
        provider.auth_required = Some(auth_required);
        provider.model = "model/test".into();
        ProviderSpec::from_aishe(&config).unwrap()
    }

    #[test]
    fn maps_supported_provider_families_exactly() {
        let cases = [
            (
                "anthropic",
                "https://api.anthropic.com",
                "auto",
                "aishe-anthropic",
                "@ai-sdk/anthropic",
            ),
            (
                "openai",
                "https://api.openai.com",
                "responses",
                "aishe-openai",
                "@ai-sdk/openai",
            ),
            (
                "groq",
                "https://api.groq.com/openai",
                "chat",
                "aishe-compatible",
                "@ai-sdk/openai-compatible",
            ),
            (
                "openrouter",
                "https://openrouter.ai/api",
                "chat",
                "aishe-compatible",
                "@ai-sdk/openai-compatible",
            ),
            (
                "together",
                "https://api.together.xyz",
                "chat",
                "aishe-compatible",
                "@ai-sdk/openai-compatible",
            ),
            (
                "ollama",
                "http://localhost:11434",
                "chat",
                "aishe-local",
                "@ai-sdk/openai-compatible",
            ),
            (
                "custom",
                "https://models.example.test",
                "responses",
                "aishe-compatible",
                "@ai-sdk/openai",
            ),
        ];
        for (name, url, transport, provider_id, npm) in cases {
            let spec = spec_for(name, url, transport, name != "ollama");
            assert_eq!(spec.provider_id, provider_id, "{name}");
            assert_eq!(spec.npm, npm, "{name}");
            assert!(spec.base_url.ends_with("/v1"), "{name}");
        }
    }

    #[test]
    fn generated_config_isolated_and_default_deny_for_every_agent() {
        let spec = spec_for("openai", "https://api.openai.com/v1", "responses", true);
        let config = generated_config(Path::new("/private/aishe-plugin.mjs"), Some(&spec)).unwrap();
        assert_eq!(config["permission"]["*"], "deny");
        assert!(config["permission"].get("aishe_*").is_none());
        assert_eq!(config["tools"]["bash"], false);
        assert_eq!(config["mcp"], serde_json::json!({}));
        assert_eq!(config["share"], "disabled");
        assert_eq!(
            config["provider"]["aishe-openai"]["options"]["apiKey"],
            "{env:AISHE_PROVIDER_API_KEY}"
        );
        for agent in [
            "aishe-suggest",
            "aishe-auto",
            "aishe-yolo",
            "general",
            "explore",
        ] {
            assert_eq!(config["agent"][agent]["permission"]["*"], "deny");
            assert_eq!(config["agent"][agent]["tools"]["read"], false);
            assert_eq!(config["agent"][agent]["model"], "aishe-openai/model/test");
        }
        assert!(config["agent"]["aishe-suggest"]["permission"]
            .get("aishe_*")
            .is_none());
        assert_eq!(
            config["agent"]["aishe-auto"]["permission"]["aishe_*"],
            "allow"
        );
        assert_eq!(config["agent"]["aishe-yolo"]["permission"]["task"], "allow");
    }
}
