//! Read-only administrator policy that constrains user and project settings.
//!
//! Policy never contains provider credentials and never grants authority. It
//! can only require, cap, restrict, or disable features. The test/deployment
//! override is intentionally a file path rather than inline TOML so secrets
//! cannot accidentally enter process listings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

pub const POLICY_SCHEMA_VERSION: u32 = 1;
const MAX_POLICY_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrganizationPolicy {
    pub version: u32,
    /// `required`, `allowed`, or `disabled`.
    pub opencode: Option<String>,
    pub runtime_base_url: Option<String>,
    pub approved_runtime_sha256: Vec<String>,
    pub require_bubblewrap: Option<bool>,
    pub allow_host_yolo: Option<bool>,
    pub allowed_provider_hosts: Vec<String>,
    pub allowed_connections: Vec<String>,
    pub allowed_models: Vec<String>,
    pub require_audit_logging: Option<bool>,
    pub require_redaction: Option<bool>,
    pub allow_network: Option<bool>,
    pub allow_user_mcp: Option<bool>,
    pub allow_user_skills: Option<bool>,
    pub max_budget_usd: Option<f64>,
    pub max_output_tokens: Option<u64>,
    pub support_bundle_exclusions: Vec<String>,
}

impl Default for OrganizationPolicy {
    fn default() -> Self {
        Self {
            version: POLICY_SCHEMA_VERSION,
            opencode: None,
            runtime_base_url: None,
            approved_runtime_sha256: Vec::new(),
            require_bubblewrap: None,
            allow_host_yolo: None,
            allowed_provider_hosts: Vec::new(),
            allowed_connections: Vec::new(),
            allowed_models: Vec::new(),
            require_audit_logging: None,
            require_redaction: None,
            allow_network: None,
            allow_user_mcp: None,
            allow_user_skills: None,
            max_budget_usd: None,
            max_output_tokens: None,
            support_bundle_exclusions: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadedPolicy {
    pub path: PathBuf,
    pub policy: OrganizationPolicy,
}

impl OrganizationPolicy {
    pub fn validate(&self) -> Result<()> {
        if self.version != POLICY_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported organization policy schema {} (expected {})",
                self.version,
                POLICY_SCHEMA_VERSION
            );
        }
        if self
            .opencode
            .as_deref()
            .is_some_and(|value| !matches!(value, "required" | "allowed" | "disabled"))
        {
            anyhow::bail!("policy opencode must be required, allowed, or disabled");
        }
        if let Some(url) = &self.runtime_base_url {
            validate_https_or_loopback(url, "runtime_base_url")?;
        }
        for hash in &self.approved_runtime_sha256 {
            if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                anyhow::bail!("policy approved_runtime_sha256 contains an invalid SHA-256");
            }
        }
        for host in &self.allowed_provider_hosts {
            validate_host(host)?;
        }
        for connection in &self.allowed_connections {
            crate::config::normalize_connection_id(connection)
                .context("policy contains an invalid connection ID")?;
        }
        for pattern in &self.allowed_models {
            validate_pattern(pattern, "model")?;
        }
        if self
            .max_budget_usd
            .is_some_and(|value| !value.is_finite() || value < 0.0)
        {
            anyhow::bail!("policy max_budget_usd must be finite and non-negative");
        }
        if self.max_output_tokens == Some(0) {
            anyhow::bail!("policy max_output_tokens must be greater than zero");
        }
        for field in &self.support_bundle_exclusions {
            validate_pattern(field, "support-bundle exclusion")?;
        }
        Ok(())
    }

    /// Apply only constraints. A disallowed provider/model is rejected rather
    /// than silently replaced with an administrator-invented preference.
    pub fn constrain(&self, config: &mut Config) -> Result<()> {
        self.validate()?;
        match self.opencode.as_deref() {
            Some("required") => config.backend.engine = "opencode".into(),
            Some("disabled") if config.backend.engine == "opencode" => {
                config.backend.engine = "native".into()
            }
            _ => {}
        }
        if self.require_bubblewrap == Some(true) {
            config.sandbox.linux_backend = "bwrap".into();
            config.sandbox.require_functional = true;
        }
        if self.allow_host_yolo == Some(false) {
            config.sandbox.allow_host_yolo = false;
            if config.backend.default_scope == "host" {
                config.backend.default_scope = "workspace".into();
            }
        }
        if self.allow_network == Some(false) {
            config.backend.workspace_network = "deny".into();
        }
        if self.require_audit_logging == Some(true) {
            config.logging.enabled = true;
        }
        if self.require_redaction == Some(true) {
            config.logging.redact = true;
            config.aishe.redact_secrets = true;
        }
        if self.allow_user_mcp == Some(false) {
            config.mcp_servers.clear();
        }
        if let Some(maximum) = self.max_budget_usd {
            if config.aishe.budget_usd == 0.0 || config.aishe.budget_usd > maximum {
                config.aishe.budget_usd = maximum;
            }
        }
        if let Some(maximum) = self.max_output_tokens {
            config.backend.max_output_tokens = match config.backend.max_output_tokens {
                0 => maximum,
                configured => configured.min(maximum),
            };
        }
        self.validate_provider(config)
    }

    pub fn validate_request(&self, config: &Config) -> Result<()> {
        self.validate()?;
        if self.opencode.as_deref() == Some("required") && config.backend.engine != "opencode" {
            anyhow::bail!("organization policy requires the OpenCode backend");
        }
        if self.opencode.as_deref() == Some("disabled") && config.backend.engine == "opencode" {
            anyhow::bail!("organization policy disables the OpenCode backend");
        }
        if self.require_bubblewrap == Some(true)
            && (!cfg!(target_os = "linux")
                || config.sandbox.linux_backend != "bwrap"
                || !config.sandbox.require_functional)
        {
            anyhow::bail!("organization policy requires functional bubblewrap");
        }
        if self.allow_host_yolo == Some(false) && config.backend.default_scope == "host" {
            anyhow::bail!("organization policy disables host scope");
        }
        if self.allow_network == Some(false) && config.backend.workspace_network != "deny" {
            anyhow::bail!("organization policy disables agent network access");
        }
        if self.require_audit_logging == Some(true) && !config.logging.enabled {
            anyhow::bail!("organization policy requires audit logging");
        }
        if self.require_redaction == Some(true)
            && (!config.logging.redact || !config.aishe.redact_secrets)
        {
            anyhow::bail!("organization policy requires secret redaction");
        }
        if self.allow_user_mcp == Some(false) && !config.mcp_servers.is_empty() {
            anyhow::bail!("organization policy disables user MCP servers");
        }
        if let Some(maximum) = self.max_budget_usd {
            if config.aishe.budget_usd == 0.0 || config.aishe.budget_usd > maximum {
                anyhow::bail!("organization policy caps session budget at ${maximum:.4}");
            }
        }
        if let Some(maximum) = self.max_output_tokens {
            if config.backend.max_output_tokens == 0 || config.backend.max_output_tokens > maximum {
                anyhow::bail!("organization policy caps provider output at {maximum} tokens");
            }
        }
        self.validate_provider(config)
    }

    pub fn permits_user_skills(&self) -> bool {
        self.allow_user_skills != Some(false)
    }

    pub fn runtime_base_url(&self) -> Option<&str> {
        self.runtime_base_url.as_deref()
    }

    pub fn validate_runtime_hash(&self, hash: &str) -> Result<()> {
        if !self.approved_runtime_sha256.is_empty()
            && !self
                .approved_runtime_sha256
                .iter()
                .any(|approved| approved.eq_ignore_ascii_case(hash))
        {
            anyhow::bail!("managed runtime hash is not approved by organization policy");
        }
        Ok(())
    }

    fn validate_provider(&self, config: &Config) -> Result<()> {
        if !self.allowed_connections.is_empty()
            && !self
                .allowed_connections
                .iter()
                .any(|value| value == config.active_connection_id())
        {
            anyhow::bail!(
                "connection '{}' is denied by organization policy",
                crate::commands::display_safe(config.active_connection_id())
            );
        }
        let provider = config.active_provider_config();
        if !self.allowed_provider_hosts.is_empty() {
            let parsed = url::Url::parse(&provider.base_url)
                .context("parsing provider URL for organization policy")?;
            let host = parsed
                .host_str()
                .context("provider URL has no host for organization policy")?;
            if !self
                .allowed_provider_hosts
                .iter()
                .any(|pattern| host_matches(host, pattern))
            {
                anyhow::bail!(
                    "provider host '{}' is denied by organization policy",
                    crate::commands::display_safe(host)
                );
            }
        }
        if !self.allowed_models.is_empty()
            && !self
                .allowed_models
                .iter()
                .any(|pattern| wildcard_matches(&provider.model, pattern))
        {
            anyhow::bail!(
                "model '{}' is denied by organization policy",
                crate::commands::display_safe(&provider.model)
            );
        }
        Ok(())
    }
}

pub fn path() -> PathBuf {
    if let Some(value) = std::env::var_os("AISHE_POLICY_FILE").filter(|value| !value.is_empty()) {
        return PathBuf::from(value);
    }
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/Aishe/policy.toml")
    } else {
        PathBuf::from("/etc/aishe/policy.toml")
    }
}

pub fn load() -> Result<Option<LoadedPolicy>> {
    load_from(&path())
}

pub fn load_from(path: &Path) -> Result<Option<LoadedPolicy>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("organization policy must be a regular non-symlink file");
    }
    if metadata.len() > MAX_POLICY_BYTES {
        anyhow::bail!("organization policy exceeds 1 MiB");
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading organization policy {}", path.display()))?;
    let policy: OrganizationPolicy = toml::from_str(&text)
        .with_context(|| format!("parsing organization policy {}", path.display()))?;
    policy.validate()?;
    Ok(Some(LoadedPolicy {
        path: path.to_path_buf(),
        policy,
    }))
}

pub fn constrain(config: &mut Config) -> Result<Option<LoadedPolicy>> {
    let loaded = load()?;
    if let Some(loaded) = &loaded {
        loaded.policy.constrain(config)?;
    }
    Ok(loaded)
}

fn validate_https_or_loopback(value: &str, field: &str) -> Result<()> {
    let parsed = url::Url::parse(value).with_context(|| format!("invalid policy {field}"))?;
    let loopback = parsed
        .host_str()
        .and_then(|host| host.parse::<std::net::IpAddr>().ok())
        .is_some_and(|address| address.is_loopback())
        || matches!(parsed.host_str(), Some("localhost"));
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        anyhow::bail!("policy {field} must use HTTPS (HTTP is allowed only on loopback)");
    }
    Ok(())
}

fn validate_host(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 253
        || value.chars().any(char::is_control)
        || value.contains('/')
        || value.contains(':')
        || (value.contains('*') && !value.starts_with("*."))
    {
        anyhow::bail!("invalid allowed provider host pattern");
    }
    Ok(())
}

fn validate_pattern(value: &str, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
        anyhow::bail!("invalid policy {label} pattern");
    }
    Ok(())
}

fn host_matches(host: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host != suffix && host.ends_with(&format!(".{suffix}"))
    } else {
        host.eq_ignore_ascii_case(pattern)
    }
}

fn wildcard_matches(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    let mut remainder = value;
    let mut first = true;
    for part in pattern.split('*') {
        if part.is_empty() {
            continue;
        }
        let Some(position) = remainder.find(part) else {
            return false;
        };
        if first && !pattern.starts_with('*') && position != 0 {
            return false;
        }
        remainder = &remainder[position + part.len()..];
        first = false;
    }
    pattern.ends_with('*') || remainder.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_constrains_without_widening() {
        let mut config = Config::default();
        config.backend.engine = "native".into();
        config.backend.default_scope = "host".into();
        config.backend.workspace_network = "allow".into();
        config.aishe.budget_usd = 50.0;
        let policy = OrganizationPolicy {
            version: 1,
            opencode: Some("required".into()),
            require_bubblewrap: Some(true),
            allow_host_yolo: Some(false),
            allow_network: Some(false),
            require_audit_logging: Some(true),
            require_redaction: Some(true),
            max_budget_usd: Some(2.0),
            max_output_tokens: Some(4096),
            ..OrganizationPolicy::default()
        };
        policy.constrain(&mut config).unwrap();
        assert_eq!(config.backend.engine, "opencode");
        assert_eq!(config.backend.default_scope, "workspace");
        assert_eq!(config.backend.workspace_network, "deny");
        assert!(config.sandbox.require_functional);
        assert!(config.logging.enabled);
        assert_eq!(config.aishe.budget_usd, 2.0);
        assert_eq!(config.backend.max_output_tokens, 4096);
    }

    #[test]
    fn provider_and_model_allowlists_are_exact_and_wildcarded() {
        let mut config = Config::default();
        config.aishe.provider = "openai".into();
        config.providers.openai.base_url = "https://api.openai.com".into();
        config.providers.openai.model = "gpt-5.6-luna".into();
        let policy = OrganizationPolicy {
            version: 1,
            allowed_provider_hosts: vec!["api.openai.com".into()],
            allowed_models: vec!["gpt-5.*".into()],
            ..OrganizationPolicy::default()
        };
        assert!(policy.validate_request(&config).is_ok());
        config.providers.openai.model = "other".into();
        assert!(policy.validate_request(&config).is_err());
    }

    #[test]
    fn connection_allowlist_distinguishes_profiles_for_the_same_provider() {
        let mut config = Config::default();
        let duplicate = config.connections["openai"].clone();
        config.connections.insert("openai-work".into(), duplicate);
        config.select_connection("openai-work").unwrap();
        let policy = OrganizationPolicy {
            version: 1,
            allowed_connections: vec!["openai-work".into()],
            ..OrganizationPolicy::default()
        };
        assert!(policy.validate_request(&config).is_ok());

        config.select_connection("openai").unwrap();
        assert!(policy.validate_request(&config).is_err());
    }

    #[test]
    fn subdomain_host_pattern_does_not_match_apex_or_suffix_trick() {
        assert!(host_matches("api.example.com", "*.example.com"));
        assert!(!host_matches("example.com", "*.example.com"));
        assert!(!host_matches("badexample.com", "*.example.com"));
    }
}
