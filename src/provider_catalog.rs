//! Provider/service metadata shared by setup, settings, Doctor, and runtime
//! validation. Keeping one catalog prevents the first-run UI and provider
//! implementation from drifting apart.

use crate::config::ProviderConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Family {
    Anthropic,
    OpenAiCompatible,
}

#[derive(Clone, Copy, Debug)]
pub struct Service {
    pub key: &'static str,
    pub label: &'static str,
    pub family: Family,
    pub base_url: &'static str,
    pub model: &'static str,
    pub key_env: &'static str,
    pub transport: &'static str,
    pub auth_required: bool,
    pub help: &'static str,
}

pub const SERVICES: &[Service] = &[
    Service {
        key: "anthropic",
        label: "Anthropic",
        family: Family::Anthropic,
        base_url: "https://api.anthropic.com",
        model: "claude-sonnet-4-20250514",
        key_env: "ANTHROPIC_API_KEY",
        transport: "auto",
        auth_required: true,
        help: "Claude models through the Anthropic Messages API.",
    },
    Service {
        key: "openai",
        label: "OpenAI",
        family: Family::OpenAiCompatible,
        base_url: "https://api.openai.com",
        model: "gpt-5.6-luna",
        key_env: "OPENAI_API_KEY",
        transport: "responses",
        auth_required: true,
        help: "Official OpenAI. Responses is used for reasoning and tools.",
    },
    Service {
        key: "groq",
        label: "Groq",
        family: Family::OpenAiCompatible,
        base_url: "https://api.groq.com/openai",
        model: "llama-3.3-70b-versatile",
        key_env: "GROQ_API_KEY",
        transport: "chat",
        auth_required: true,
        help: "Groq's OpenAI-compatible endpoint.",
    },
    Service {
        key: "openrouter",
        label: "OpenRouter",
        family: Family::OpenAiCompatible,
        base_url: "https://openrouter.ai/api",
        model: "openai/gpt-4o",
        key_env: "OPENROUTER_API_KEY",
        transport: "chat",
        auth_required: true,
        help: "OpenRouter's multi-provider OpenAI-compatible endpoint.",
    },
    Service {
        key: "together",
        label: "Together AI",
        family: Family::OpenAiCompatible,
        base_url: "https://api.together.xyz",
        model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        key_env: "TOGETHER_API_KEY",
        transport: "chat",
        auth_required: true,
        help: "Together AI's OpenAI-compatible endpoint.",
    },
    Service {
        key: "ollama",
        label: "Ollama (local)",
        family: Family::OpenAiCompatible,
        base_url: "http://localhost:11434",
        model: "llama3.1",
        key_env: "OLLAMA_API_KEY",
        transport: "chat",
        auth_required: false,
        help: "Local Ollama; no dummy API key is required.",
    },
    Service {
        key: "custom",
        label: "Other / custom endpoint",
        family: Family::OpenAiCompatible,
        base_url: "",
        model: "",
        key_env: "OPENAI_API_KEY",
        transport: "auto",
        auth_required: true,
        help: "Any endpoint implementing Responses or Chat Completions.",
    },
];

pub fn find(key: &str) -> Option<&'static Service> {
    SERVICES
        .iter()
        .find(|service| service.key.eq_ignore_ascii_case(key.trim()))
}

pub fn apply(service: &Service, provider: &mut ProviderConfig) {
    if !service.base_url.is_empty() {
        provider.base_url = service.base_url.to_string();
    }
    if !service.model.is_empty() {
        provider.model = service.model.to_string();
    }
    provider.api_key_env = service.key_env.to_string();
    provider.transport = service.transport.to_string();
    provider.auth_required = Some(service.auth_required);
}

/// Normalize a host root. A bare localhost gets HTTP because local model servers
/// generally do not terminate TLS; other bare hosts default to HTTPS.
pub fn normalize_base_url(input: &str) -> String {
    let value = input.trim().trim_end_matches('/');
    if value.is_empty() {
        return "https://api.openai.com".to_string();
    }
    let rooted = if value.contains("://") {
        value.to_string()
    } else if value == "localhost"
        || value.starts_with("localhost:")
        || value == "127.0.0.1"
        || value.starts_with("127.0.0.1:")
        || value == "[::1]"
        || value.starts_with("[::1]:")
    {
        format!("http://{value}")
    } else {
        format!("https://{value}")
    };
    // Provider implementations append their own versioned resource path.
    // Accept the common copy/paste form ending in `/v1` without producing
    // `/v1/v1/...`; this also makes official OpenAI detection deterministic.
    if rooted.to_ascii_lowercase().ends_with("/v1") {
        rooted[..rooted.len() - 3].trim_end_matches('/').to_string()
    } else {
        rooted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_are_unique_and_complete() {
        let mut keys = std::collections::BTreeSet::new();
        for service in SERVICES {
            assert!(keys.insert(service.key));
            assert!(!service.label.is_empty());
            assert!(!service.key_env.is_empty());
            assert!(matches!(service.transport, "auto" | "responses" | "chat"));
        }
    }

    #[test]
    fn local_urls_default_to_http() {
        assert_eq!(
            normalize_base_url("localhost:11434/"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_base_url("api.example.test"),
            "https://api.example.test"
        );
        assert_eq!(
            normalize_base_url("https://api.openai.com/v1/"),
            "https://api.openai.com"
        );
        assert_eq!(
            normalize_base_url("https://api.groq.com/openai/v1"),
            "https://api.groq.com/openai"
        );
    }

    #[test]
    fn ollama_does_not_require_auth() {
        let service = find("ollama").unwrap();
        assert!(!service.auth_required);
        let mut provider = ProviderConfig::default();
        apply(service, &mut provider);
        assert!(!provider.requires_auth());
    }
}
