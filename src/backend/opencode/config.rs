//! Deterministic OpenCode configuration generated only from trusted AIShe
//! configuration. Project/user OpenCode configuration is never merged.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::config::{Config, ProviderConfig};

pub const PROVIDER_KEY_ENV: &str = "AISHE_PROVIDER_API_KEY";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProviderSpec {
    #[serde(default)]
    pub connection_id: String,
    #[serde(default)]
    pub launch_identity: String,
    pub provider_id: String,
    pub model_id: String,
    pub npm: String,
    pub base_url: String,
    pub requires_auth: bool,
    /// Exact built-in OpenCode OAuth hook required for this launch. `None`
    /// means the provider uses an AIShe API key or needs no authentication.
    pub oauth_provider: Option<String>,
    /// User-labeled isolated OAuth runtime profile. Never contains tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_profile: Option<String>,
    pub price: Option<crate::usage::Price>,
    /// OpenCode model option. `None` lets the provider/model choose its default.
    pub reasoning_effort: Option<String>,
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
        let resolved = crate::connection::resolve(config)?;
        let (oauth_provider, oauth_profile) = match &resolved.auth {
            crate::connection::ResolvedAuth::OAuth { provider, profile } => {
                (Some(*provider), Some(profile.clone()))
            }
            crate::connection::ResolvedAuth::ApiKey { .. }
            | crate::connection::ResolvedAuth::None => (None, None),
        };
        let mut spec = ProviderSpec::from_aishe_with_oauth(config, oauth_provider)?;
        spec.connection_id = resolved.id;
        spec.launch_identity = resolved.launch_identity;
        spec.oauth_profile = oauth_profile;
        Ok(Self {
            spec,
            api_key: resolved.api_key,
        })
    }
}

impl ProviderSpec {
    pub fn from_aishe(config: &Config) -> Result<Self> {
        Self::from_aishe_with_oauth(config, None)
    }

    fn from_aishe_with_oauth(
        config: &Config,
        oauth_provider: Option<crate::oauth::OAuthProvider>,
    ) -> Result<Self> {
        let provider = active_provider(config);
        validate_model(&provider.model)?;
        let base = crate::provider_catalog::normalize_base_url(&provider.base_url);
        let parsed = url::Url::parse(&base).context("active provider base URL is invalid")?;
        if !matches!(parsed.scheme(), "http" | "https") {
            anyhow::bail!("active provider base URL must use HTTP or HTTPS");
        }
        let official_openai = parsed.host_str() == Some("api.openai.com");
        let official_xai = parsed.host_str() == Some("api.x.ai");
        let local = crate::config::is_loopback_url(&base);
        let anthropic = config.active_provider_name() == "anthropic";
        if matches!(oauth_provider, Some(crate::oauth::OAuthProvider::Openai)) && !official_openai {
            anyhow::bail!("OpenAI OAuth can only be bound to api.openai.com");
        }
        if matches!(oauth_provider, Some(crate::oauth::OAuthProvider::Xai)) && !official_xai {
            anyhow::bail!("xAI OAuth can only be bound to api.x.ai");
        }
        let responses =
            provider.transport == "responses" || (provider.transport == "auto" && official_openai);
        let (provider_id, npm) = if let Some(oauth) = oauth_provider {
            match oauth {
                crate::oauth::OAuthProvider::Openai => ("openai", "@ai-sdk/openai"),
                crate::oauth::OAuthProvider::Xai => ("xai", "@ai-sdk/xai"),
            }
        } else if anthropic {
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
            connection_id: config.active_connection_id().to_string(),
            launch_identity: String::new(),
            provider_id: provider_id.into(),
            model_id: provider.model.clone(),
            npm: npm.into(),
            base_url: append_v1(&base),
            requires_auth: provider.requires_auth() && oauth_provider.is_none(),
            oauth_provider: oauth_provider.map(|provider| provider.id().to_string()),
            oauth_profile: oauth_provider.map(|_| "default".to_string()),
            price: if oauth_provider.is_some() {
                None
            } else {
                crate::usage::price_for(&provider.model, &config.pricing)
            },
            reasoning_effort: (!anthropic)
                .then(|| explicit_reasoning_effort(config.active_reasoning_effort()))
                .flatten(),
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
            "description":"AIShe one-turn answer and shell-command suggestion agent.",
            "mode":"primary",
            "prompt":SUGGEST_PROMPT,
            "permission":{"*":"deny"},
            "tools":suggest_disabled_tools(),
            "steps":1
        }),
    );
    agent_config.insert(
        "aishe-auto".into(),
        serde_json::json!({
            "description":"AIShe approval-gated command-line agent.",
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
            "description":"AIShe scope-authorized autonomous command-line agent.",
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
                "description":"AIShe child agent. All host effects remain behind the foreground AIShe bridge.",
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
                "name": "AIShe managed provider",
                "npm": provider.npm,
                "options": options,
                "models": {
                    provider.model_id.clone(): {
                        "id": provider.model_id,
                        "name": provider.model_id,
                        "tool_call": true,
                        "options": {}
                    }
                }
            }
        });
        if let Some(effort) = &provider.reasoning_effort {
            config["provider"][&provider.provider_id]["models"][&provider.model_id]["options"]
                ["reasoningEffort"] = Value::String(effort.clone());
        }
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
    config.active_provider_config()
}

fn append_v1(base: &str) -> String {
    format!("{}/v1", base.trim_end_matches('/'))
}

fn explicit_reasoning_effort(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "none" | "low" | "medium" | "high" | "xhigh" | "max" => {
            Some(value.trim().to_ascii_lowercase())
        }
        _ => None,
    }
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

const SUGGEST_PROMPT: &str = r#"You are AIShe's concise command-line assistant.
Answer the user's full request. If the best response is one runnable shell command,
return type=command and put only that command in command. Otherwise return
type=answer, leave command empty, and put the answer in explanation. Never invent
a command for a factual or conversational question. The required JSON schema is
the authoritative output contract."#;

const AUTO_PROMPT: &str = r#"You are AIShe's approval-gated command-line agent.
Work to completion using only aishe_* proxy tools. Safe read-only actions may run
immediately; AIShe decides whether any other action needs approval. Never request
OpenCode built-in host tools or claim an action happened without a successful tool
result. Keep terminal-facing explanations concise."#;

const YOLO_PROMPT: &str = r#"You are AIShe's autonomous command-line agent.
Work to completion using only aishe_* proxy tools and optional child agents.
AIShe has already obtained the user's session-scoped authorization and enforces
workspace/host/network boundaries outside the model. Never ask for per-action
approval, never attempt to widen scope, and never claim an action happened without
a successful tool result. Inspect, edit, test, and verify your work."#;

const CHILD_PROMPT: &str = r#"You are an AIShe child agent. Complete the delegated
subtask using only aishe_* proxy tools. You inherit the parent AIShe lease and may
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

fn suggest_disabled_tools() -> Value {
    let mut tools = disabled_builtin_tools();
    let object = tools
        .as_object_mut()
        .expect("disabled tool configuration is always an object");
    for name in [
        "aishe_run_command",
        "aishe_read_file",
        "aishe_write_file",
        "aishe_edit_file",
        "aishe_apply_patch",
        "aishe_list_dir",
        "aishe_search_files",
        "aishe_fetch_url",
        "aishe_use_skill",
        "aishe_mcp_call",
        "aishe_ask_user",
        "task",
        "todowrite",
        "todoread",
    ] {
        object.insert(name.into(), Value::Bool(false));
    }
    tools
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
    fn oauth_uses_exact_builtin_provider_identity_without_api_key_injection() {
        for (oauth, base_url, provider_id, npm) in [
            (
                crate::oauth::OAuthProvider::Openai,
                "https://api.openai.com",
                "openai",
                "@ai-sdk/openai",
            ),
            (
                crate::oauth::OAuthProvider::Xai,
                "https://api.x.ai",
                "xai",
                "@ai-sdk/xai",
            ),
        ] {
            let mut aishe = Config::default();
            aishe.aishe.provider = "openai".into();
            aishe.providers.openai.base_url = base_url.into();
            aishe.providers.openai.model = "oauth-model".into();
            let spec = ProviderSpec::from_aishe_with_oauth(&aishe, Some(oauth)).unwrap();
            assert_eq!(spec.provider_id, provider_id);
            assert_eq!(spec.npm, npm);
            assert!(!spec.requires_auth);
            assert_eq!(spec.oauth_provider.as_deref(), Some(provider_id));
            assert!(spec.price.is_none());
            let generated =
                generated_config(Path::new("/private/aishe-plugin.mjs"), Some(&spec)).unwrap();
            assert!(generated["provider"][provider_id]["options"]
                .get("apiKey")
                .is_none());
            assert_eq!(generated["enabled_providers"][0], provider_id);
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
        for tool in [
            "aishe_run_command",
            "aishe_read_file",
            "aishe_write_file",
            "aishe_apply_patch",
            "task",
            "todowrite",
            "todoread",
        ] {
            assert_eq!(
                config["agent"]["aishe-suggest"]["tools"][tool], false,
                "suggest must hide {tool} from the model"
            );
        }
        assert_eq!(
            config["agent"]["aishe-auto"]["permission"]["aishe_*"],
            "allow"
        );
        assert_eq!(config["agent"]["aishe-yolo"]["permission"]["task"], "allow");
    }

    #[test]
    fn managed_openai_reasoning_effort_is_an_explicit_model_option() {
        let mut aishe = Config::default();
        aishe.aishe.provider = "openai".into();
        aishe.aishe.reasoning_effort = "HIGH".into();
        aishe.providers.openai.base_url = "https://api.openai.com".into();
        aishe.providers.openai.transport = "responses".into();
        aishe.providers.openai.model = "gpt-5.6-sol".into();
        let spec = ProviderSpec::from_aishe(&aishe).unwrap();
        assert_eq!(spec.reasoning_effort.as_deref(), Some("high"));
        let generated =
            generated_config(Path::new("/private/aishe-plugin.mjs"), Some(&spec)).unwrap();
        assert_eq!(
            generated["provider"]["aishe-openai"]["models"]["gpt-5.6-sol"]["options"]
                ["reasoningEffort"],
            "high"
        );

        aishe.aishe.reasoning_effort = "auto".into();
        let spec = ProviderSpec::from_aishe(&aishe).unwrap();
        assert_eq!(spec.reasoning_effort, None);
    }
}
