//! Central named-connection and authentication resolution.
//!
//! Every provider launch flows through this module so an explicit OAuth
//! connection can never fall back to an ambient API key (and vice versa).

use anyhow::{Context, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::config::{Config, ConnectionAuth, ConnectionConfig, ProviderConfig};
use crate::oauth::OAuthProvider;

const MAX_SELECTION_BYTES: u64 = 4096;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedAuth {
    ApiKey {
        source: String,
    },
    OAuth {
        provider: OAuthProvider,
        profile: String,
    },
    None,
}

/// Deliberately has no `Debug` or serialization implementation: `api_key` is
/// transient launch material and must never enter logs or audit records.
pub struct ResolvedConnection {
    pub id: String,
    pub label: String,
    pub provider: String,
    pub settings: ProviderConfig,
    pub auth: ResolvedAuth,
    pub api_key: Option<String>,
    pub launch_identity: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConnectionAuthStatus {
    pub kind: String,
    pub profile: Option<String>,
    pub available: bool,
    pub detail: String,
}

pub fn auth_status(connection: &ConnectionConfig) -> ConnectionAuthStatus {
    match &connection.auth {
        ConnectionAuth::OAuth { profile } => {
            let Some(provider) = OAuthProvider::from_base_url(&connection.settings.base_url) else {
                return ConnectionAuthStatus {
                    kind: "oauth".into(),
                    profile: Some(profile.clone()),
                    available: false,
                    detail: "invalid OAuth endpoint".into(),
                };
            };
            match crate::oauth::status_profile(provider, profile) {
                Ok(status) => ConnectionAuthStatus {
                    kind: "oauth".into(),
                    profile: Some(profile.clone()),
                    available: status.available,
                    detail: if status.available { "ready" } else { "missing" }.into(),
                },
                Err(error) => ConnectionAuthStatus {
                    kind: "oauth".into(),
                    profile: Some(profile.clone()),
                    available: false,
                    detail: format!("error: {}", crate::redact::redact(&error.to_string())),
                },
            }
        }
        ConnectionAuth::ApiKey { .. } => {
            let provider = connection.provider_view();
            match crate::credentials::resolve(&provider) {
                Ok(resolved) => ConnectionAuthStatus {
                    kind: "api_key".into(),
                    profile: Some(provider.credential_profile()),
                    available: resolved.source.is_available(),
                    detail: resolved.source.label(),
                },
                Err(error) => ConnectionAuthStatus {
                    kind: "api_key".into(),
                    profile: Some(provider.credential_profile()),
                    available: false,
                    detail: format!("error: {}", crate::redact::redact(&error.to_string())),
                },
            }
        }
        ConnectionAuth::None => ConnectionAuthStatus {
            kind: "none".into(),
            profile: None,
            available: true,
            detail: "not required".into(),
        },
        ConnectionAuth::Auto => {
            let provider = connection.provider_view();
            let resolution = crate::credentials::resolve(&provider);
            let key_source = resolution
                .as_ref()
                .ok()
                .filter(|resolved| resolved.secret().is_some())
                .map(|resolved| resolved.source.label());
            ConnectionAuthStatus {
                kind: "auto".into(),
                profile: Some(provider.credential_profile()),
                available: resolve_legacy_auth(connection),
                detail: key_source.unwrap_or_else(|| "legacy automatic resolution".into()),
            }
        }
    }
}

fn resolve_legacy_auth(connection: &ConnectionConfig) -> bool {
    let provider = connection.provider_view();
    if crate::credentials::resolve(&provider)
        .ok()
        .is_some_and(|resolved| resolved.secret().is_some())
    {
        return true;
    }
    if !provider.requires_auth() {
        return true;
    }
    OAuthProvider::from_base_url(&provider.base_url)
        .and_then(|oauth| crate::oauth::status(oauth).ok())
        .is_some_and(|status| status.available)
}

pub fn resolve(config: &Config) -> Result<ResolvedConnection> {
    resolve_id(config, config.active_connection_id())
}

pub fn resolve_id(config: &Config, id: &str) -> Result<ResolvedConnection> {
    let connection = config
        .connections
        .get(id)
        .with_context(|| format!("named connection '{id}' does not exist"))?;
    resolve_connection(config, id, connection)
}

fn resolve_connection(
    _config: &Config,
    id: &str,
    connection: &ConnectionConfig,
) -> Result<ResolvedConnection> {
    let mut effective_connection = connection.clone();
    if id == _config.active_connection_id() {
        effective_connection.settings = _config.active_provider_config().clone();
    }
    let settings = effective_connection.provider_view();
    let (auth, api_key) = match &connection.auth {
        ConnectionAuth::ApiKey { .. } => {
            let resolved = crate::credentials::resolve(&settings)?;
            let source = source_label(&resolved.source);
            let secret = resolved.into_secret().with_context(|| {
                format!(
                    "API key missing for connection '{id}' — run `aishe auth set {}` or set ${}",
                    settings.credential_profile(),
                    settings.api_key_env
                )
            })?;
            (ResolvedAuth::ApiKey { source }, Some(secret))
        }
        ConnectionAuth::OAuth { profile } => {
            let provider = OAuthProvider::from_base_url(&settings.base_url).with_context(|| {
                format!(
                    "connection '{id}' cannot use OAuth: endpoint is not exactly api.openai.com or api.x.ai"
                )
            })?;
            let status = crate::oauth::status_profile(provider, profile)?;
            if !status.available {
                anyhow::bail!(
                    "OAuth credential missing for connection '{id}' — run `aishe auth login {provider} --profile {}`",
                    crate::commands::display_safe(profile)
                );
            }
            (
                ResolvedAuth::OAuth {
                    provider,
                    profile: profile.clone(),
                },
                None,
            )
        }
        ConnectionAuth::None => (ResolvedAuth::None, None),
        ConnectionAuth::Auto => {
            let resolved = crate::credentials::resolve(&settings)?;
            if let Some(secret) = resolved.into_secret() {
                (
                    ResolvedAuth::ApiKey {
                        source: "legacy auto".into(),
                    },
                    Some(secret),
                )
            } else if let Some(provider) = OAuthProvider::from_base_url(&settings.base_url) {
                if crate::oauth::status(provider)?.available {
                    (
                        ResolvedAuth::OAuth {
                            provider,
                            profile: "default".into(),
                        },
                        None,
                    )
                } else if settings.requires_auth() {
                    anyhow::bail!(
                        "API key missing for credential profile '{}' (set it with `aishe auth set {}` or ${}); OAuth is also unavailable for legacy connection '{id}'",
                        settings.credential_profile(),
                        settings.credential_profile(),
                        settings.api_key_env
                    );
                } else {
                    (ResolvedAuth::None, None)
                }
            } else if settings.requires_auth() {
                anyhow::bail!("authentication missing for legacy connection '{id}'");
            } else {
                (ResolvedAuth::None, None)
            }
        }
    };

    let launch_identity = safe_launch_identity(id, &effective_connection, &auth);
    Ok(ResolvedConnection {
        id: id.to_string(),
        label: connection.label.clone(),
        provider: connection.provider.clone(),
        settings,
        auth,
        api_key,
        launch_identity,
    })
}

fn source_label(source: &crate::credentials::Source) -> String {
    match source {
        crate::credentials::Source::Environment { variable } => format!("environment:{variable}"),
        crate::credentials::Source::Staged { profile } => format!("staged:{profile}"),
        crate::credentials::Source::CredentialsFile { profile, .. } => {
            format!("credentials:{profile}")
        }
        crate::credentials::Source::Missing { profile } => format!("missing:{profile}"),
        crate::credentials::Source::NotRequired => "not-required".into(),
    }
}

fn safe_launch_identity(id: &str, connection: &ConnectionConfig, auth: &ResolvedAuth) -> String {
    let auth_identity = match &connection.auth {
        ConnectionAuth::ApiKey {
            credential,
            api_key_env,
        } => format!(
            "api-key:{}:{}",
            credential
                .as_deref()
                .unwrap_or(&connection.settings.credential),
            api_key_env
                .as_deref()
                .unwrap_or(&connection.settings.api_key_env)
        ),
        ConnectionAuth::OAuth { profile } => format!("oauth:{profile}"),
        ConnectionAuth::None => "none".into(),
        ConnectionAuth::Auto => match auth {
            ResolvedAuth::ApiKey { .. } => "auto:api-key".into(),
            ResolvedAuth::OAuth { provider, profile } => {
                format!("auto:oauth:{}:{profile}", provider.id())
            }
            ResolvedAuth::None => "auto:none".into(),
        },
    };
    let value = format!(
        "{id}\0{}\0{}\0{}\0{}\0{}\0{auth_identity}",
        connection.provider,
        connection.settings.base_url,
        connection.settings.model,
        connection.settings.transport,
        connection.settings.requires_auth(),
    );
    let digest = Sha256::digest(value.as_bytes());
    format!("c-{}", hex_prefix(&digest, 32))
}

/// Stable supervisor identity computable without reading a secret. Used to
/// invalidate exactly one connection after its credential or settings change.
pub fn configured_launch_identity(id: &str, connection: &ConnectionConfig) -> String {
    let placeholder = match &connection.auth {
        ConnectionAuth::OAuth { profile } => {
            OAuthProvider::from_base_url(&connection.settings.base_url)
                .map(|provider| ResolvedAuth::OAuth {
                    provider,
                    profile: profile.clone(),
                })
                .unwrap_or(ResolvedAuth::None)
        }
        ConnectionAuth::None => ResolvedAuth::None,
        ConnectionAuth::ApiKey { .. } | ConnectionAuth::Auto => ResolvedAuth::ApiKey {
            source: "configured".into(),
        },
    };
    safe_launch_identity(id, connection, &placeholder)
}

fn configured_launch_identities(id: &str, connection: &ConnectionConfig) -> Vec<String> {
    let mut identities = vec![configured_launch_identity(id, connection)];
    if matches!(connection.auth, ConnectionAuth::Auto) {
        identities.push(safe_launch_identity(id, connection, &ResolvedAuth::None));
        if let Some(provider) = OAuthProvider::from_base_url(&connection.settings.base_url) {
            identities.push(safe_launch_identity(
                id,
                connection,
                &ResolvedAuth::OAuth {
                    provider,
                    profile: "default".into(),
                },
            ));
        }
    }
    identities.sort();
    identities.dedup();
    identities
}

pub fn invalidate_connection(id: &str, connection: &ConnectionConfig) -> Result<usize> {
    let mut stopped = 0;
    for identity in configured_launch_identities(id, connection) {
        if crate::backend::control::request_stop_for(&identity).unwrap_or(false) {
            stopped += 1;
        }
    }
    Ok(stopped)
}

pub fn invalidate_api_key_profile(profile: &str) -> Result<usize> {
    let profile = crate::credentials::normalize_profile(profile)?;
    let Some(config) = Config::load_quiet()? else {
        return Ok(0);
    };
    let mut stopped = 0;
    for (id, connection) in &config.connections {
        let uses_profile = match &connection.auth {
            ConnectionAuth::ApiKey { credential, .. } => credential
                .as_deref()
                .unwrap_or(&connection.settings.credential)
                .eq_ignore_ascii_case(&profile),
            ConnectionAuth::Auto => connection.settings.credential_profile() == profile,
            ConnectionAuth::OAuth { .. } | ConnectionAuth::None => false,
        };
        if uses_profile {
            stopped += invalidate_connection(id, connection).unwrap_or(0);
        }
    }
    Ok(stopped)
}

pub fn invalidate_oauth_profile(provider: OAuthProvider, profile: Option<&str>) -> Result<usize> {
    let normalized = profile.map(crate::oauth::normalize_profile).transpose()?;
    let Some(config) = Config::load_quiet()? else {
        return Ok(0);
    };
    let mut stopped = 0;
    for (id, connection) in &config.connections {
        let endpoint_provider = OAuthProvider::from_base_url(&connection.settings.base_url);
        let uses_profile = match &connection.auth {
            ConnectionAuth::OAuth {
                profile: configured,
            } => {
                endpoint_provider == Some(provider)
                    && normalized.as_deref()
                        == Some(crate::oauth::normalize_profile(configured)?.as_str())
            }
            ConnectionAuth::Auto => endpoint_provider == Some(provider) && normalized.is_none(),
            ConnectionAuth::ApiKey { .. } | ConnectionAuth::None => false,
        };
        if uses_profile {
            stopped += invalidate_connection(id, connection).unwrap_or(0);
        }
    }
    Ok(stopped)
}

fn hex_prefix(bytes: &[u8], count: usize) -> String {
    bytes
        .iter()
        .flat_map(|byte| {
            [
                b"0123456789abcdef"[(byte >> 4) as usize],
                b"0123456789abcdef"[(byte & 15) as usize],
            ]
        })
        .take(count)
        .map(char::from)
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellSelection {
    pub connection_id: String,
    pub connection_label: String,
    pub provider: String,
    pub endpoint_host: String,
    pub auth_label: String,
    pub model_id: String,
    pub reasoning_effort: String,
    /// `shell` for an ephemeral override or `default` for the durable choice.
    pub selection_scope: String,
}

pub fn selection_path() -> Option<PathBuf> {
    std::env::var_os("AISHE_SELECTION_FILE")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn selection_is_shell_local() -> bool {
    selection_path()
        .and_then(|path| read_selection(&path).ok())
        .is_some_and(|selection| selection.selection_scope == "shell")
}

pub fn apply_shell_selection(config: &mut Config) -> Result<bool> {
    let Some(path) = selection_path() else {
        return Ok(false);
    };
    let selection = read_selection(&path)?;
    config.select_connection(&selection.connection_id)?;
    validate_model_id(&selection.model_id)?;
    config.set_active_model(selection.model_id);
    if !selection.reasoning_effort.is_empty() {
        validate_reasoning(&selection.reasoning_effort)?;
        config.set_active_reasoning_effort(selection.reasoning_effort);
    }
    Ok(true)
}

pub fn write_shell_selection(config: &Config, selection_scope: &str) -> Result<bool> {
    if let Some(model_file) = std::env::var_os("AISHE_MODEL_FILE").filter(|v| !v.is_empty()) {
        crate::config::write_atomic(
            Path::new(&model_file),
            crate::commands::display_safe(config.active_model()).as_bytes(),
        )?;
    }
    let Some(path) = selection_path() else {
        return Ok(false);
    };
    write_selection(
        &path,
        &ShellSelection {
            connection_id: config.active_connection_id().to_string(),
            connection_label: config
                .active_connection()
                .map(|value| value.label.clone())
                .unwrap_or_else(|| config.active_connection_id().to_string()),
            provider: config.active_provider_name().to_string(),
            endpoint_host: url::Url::parse(&config.active_provider_config().base_url)
                .ok()
                .and_then(|url| url.host_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "unknown".into()),
            auth_label: config
                .active_connection()
                .map(crate::config::ConnectionConfig::auth_label)
                .unwrap_or_else(|| "Auto (legacy)".into()),
            model_id: config.active_model().to_string(),
            reasoning_effort: config.active_reasoning_effort().to_string(),
            selection_scope: selection_scope.to_string(),
        },
    )?;
    if let (Some(status_path), Some(usage_path)) = (
        std::env::var_os("AISHE_STATUS_FILE").filter(|value| !value.is_empty()),
        std::env::var_os("AISHE_USAGE_FILE").filter(|value| !value.is_empty()),
    ) {
        crate::usagelog::write_status_for_connection(
            Path::new(&status_path),
            Path::new(&usage_path),
            &config.pricing,
            None,
            &config.aishe.status_line_items,
            config.active_connection_id(),
        );
    }
    Ok(true)
}

pub fn write_selection(path: &Path, selection: &ShellSelection) -> Result<()> {
    crate::config::normalize_connection_id(&selection.connection_id)?;
    for (name, value) in [
        ("connection label", &selection.connection_label),
        ("provider", &selection.provider),
        ("endpoint host", &selection.endpoint_host),
        ("auth label", &selection.auth_label),
    ] {
        if value.is_empty()
            || value.len() > 512
            || value.contains(['\n', '\r', '\0'])
            || value.chars().any(char::is_control)
        {
            anyhow::bail!("{name} is not safe for a shell selection handoff");
        }
    }
    validate_model_id(&selection.model_id)?;
    validate_reasoning(&selection.reasoning_effort)?;
    validate_selection_scope(&selection.selection_scope)?;
    let text = format!(
        "{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n",
        selection.connection_id,
        selection.connection_label,
        selection.provider,
        selection.endpoint_host,
        selection.auth_label,
        selection.model_id,
        selection.reasoning_effort,
        selection.selection_scope
    );
    crate::config::write_atomic(path, text.as_bytes())?;
    crate::config::set_private_file(path);
    Ok(())
}

pub fn read_selection(path: &Path) -> Result<ShellSelection> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("reading shell selection {}", path.display()))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_SELECTION_BYTES
    {
        anyhow::bail!("AISHE_SELECTION_FILE is not a bounded regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            anyhow::bail!("AISHE_SELECTION_FILE is not owned by the current user");
        }
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(path)?
        .take(MAX_SELECTION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SELECTION_BYTES {
        anyhow::bail!("AISHE_SELECTION_FILE exceeds 4 KiB");
    }
    let text = String::from_utf8(bytes).context("AISHE_SELECTION_FILE is not UTF-8")?;
    let mut lines = text.lines();
    let connection_id = lines.next().unwrap_or_default().to_string();
    let remaining: Vec<&str> = lines.collect();
    let current_format = remaining.len() >= 7;
    let selection = ShellSelection {
        connection_label: if current_format {
            remaining[0].to_string()
        } else {
            connection_id.clone()
        },
        provider: if current_format {
            remaining[1].to_string()
        } else {
            "unknown".into()
        },
        endpoint_host: if current_format {
            remaining[2].to_string()
        } else {
            "unknown".into()
        },
        auth_label: if current_format {
            remaining[3].to_string()
        } else {
            "Auto (legacy)".into()
        },
        connection_id,
        model_id: remaining
            .get(if current_format { 4 } else { 0 })
            .copied()
            .unwrap_or_default()
            .to_string(),
        reasoning_effort: remaining
            .get(if current_format { 5 } else { 1 })
            .copied()
            .unwrap_or("auto")
            .to_string(),
        // Three-line v0.6.0 handoffs represented a shell-local selection.
        selection_scope: remaining
            .get(if current_format { 6 } else { 2 })
            .copied()
            .unwrap_or("shell")
            .to_string(),
    };
    crate::config::normalize_connection_id(&selection.connection_id)?;
    validate_model_id(&selection.model_id)?;
    validate_reasoning(&selection.reasoning_effort)?;
    validate_selection_scope(&selection.selection_scope)?;
    Ok(selection)
}

fn validate_selection_scope(value: &str) -> Result<()> {
    if matches!(value, "shell" | "default") {
        Ok(())
    } else {
        anyhow::bail!("selection scope must be shell or default")
    }
}

pub fn validate_model_id(value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.len() > 512
        || value.contains(['\n', '\r', '\0'])
        || value.chars().any(char::is_control)
    {
        anyhow::bail!("model must be 1–512 printable characters");
    }
    Ok(())
}

pub fn validate_reasoning(value: &str) -> Result<()> {
    if matches!(
        value,
        "auto" | "none" | "low" | "medium" | "high" | "xhigh" | "max"
    ) {
        Ok(())
    } else {
        anyhow::bail!("invalid reasoning effort '{value}'")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_connection(label: &str, environment: &str) -> ConnectionConfig {
        let mut settings = ProviderConfig {
            base_url: "https://api.openai.com".into(),
            model: "gpt-test".into(),
            api_key_env: environment.into(),
            credential: label.into(),
            transport: "responses".into(),
            auth_required: Some(true),
        };
        settings.credential = label.into();
        ConnectionConfig {
            provider: "openai".into(),
            label: label.into(),
            settings,
            auth: ConnectionAuth::ApiKey {
                credential: Some(label.into()),
                api_key_env: Some(environment.into()),
            },
            reasoning_effort: Some("high".into()),
        }
    }

    #[test]
    fn oauth_identity_contains_no_token_material() {
        let mut config = Config::default();
        let connection = config.connections.get_mut("openai").unwrap();
        connection.auth = ConnectionAuth::OAuth {
            profile: "Work Account".into(),
        };
        let identity = safe_launch_identity(
            "openai",
            connection,
            &ResolvedAuth::OAuth {
                provider: OAuthProvider::Openai,
                profile: "Work Account".into(),
            },
        );
        assert!(identity.starts_with("c-"));
        assert!(!identity.contains("Work Account"));
    }

    #[test]
    fn compatibility_invalidation_covers_key_oauth_and_no_auth_launches() {
        let mut connection = api_connection("legacy", "LEGACY_KEY");
        connection.auth = ConnectionAuth::Auto;
        let identities = configured_launch_identities("legacy", &connection);
        assert_eq!(identities.len(), 3);
        assert!(identities.contains(&safe_launch_identity(
            "legacy",
            &connection,
            &ResolvedAuth::None,
        )));
        assert!(identities.contains(&safe_launch_identity(
            "legacy",
            &connection,
            &ResolvedAuth::OAuth {
                provider: OAuthProvider::Openai,
                profile: "default".into(),
            },
        )));
    }

    #[test]
    fn duplicate_provider_api_connections_resolve_distinct_credentials_and_identities() {
        let work_env = "AISHE_TEST_OPENAI_WORK_KEY";
        let personal_env = "AISHE_TEST_OPENAI_PERSONAL_KEY";
        std::env::set_var(work_env, "work-secret-value");
        std::env::set_var(personal_env, "personal-secret-value");
        let mut config = Config::default();
        config.connections.clear();
        config
            .connections
            .insert("openai-work".into(), api_connection("work", work_env));
        config.connections.insert(
            "openai-personal".into(),
            api_connection("personal", personal_env),
        );
        config.aishe.connection = "openai-work".into();
        config.aishe.provider = "openai".into();
        let work = resolve_id(&config, "openai-work").unwrap();
        let personal = resolve_id(&config, "openai-personal").unwrap();
        assert_eq!(work.api_key.as_deref(), Some("work-secret-value"));
        assert_eq!(personal.api_key.as_deref(), Some("personal-secret-value"));
        assert_ne!(work.launch_identity, personal.launch_identity);
        assert!(!work.launch_identity.contains("secret"));
        assert!(!personal.launch_identity.contains("secret"));
        std::env::remove_var(work_env);
        std::env::remove_var(personal_env);
    }

    #[test]
    fn explicit_oauth_never_falls_back_to_an_api_key() {
        let environment = "AISHE_TEST_OAUTH_MUST_IGNORE_THIS_KEY";
        std::env::set_var(environment, "ambient-api-secret");
        let mut config = Config::default();
        let connection = config.connections.get_mut("openai").unwrap();
        connection.settings.api_key_env = environment.into();
        connection.auth = ConnectionAuth::OAuth {
            profile: format!("missing-resolver-test-{}", std::process::id()),
        };
        let error = resolve_id(&config, "openai").err().unwrap().to_string();
        assert!(error.contains("OAuth credential missing"));
        assert!(!error.contains("ambient-api-secret"));
        std::env::remove_var(environment);
    }

    #[test]
    fn shell_selection_round_trips_atomically_without_secret_fields() {
        let path = std::env::temp_dir().join(format!(
            "aishe-selection-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ));
        let selection = ShellSelection {
            connection_id: "openai-work".into(),
            connection_label: "OpenAI work".into(),
            provider: "openai".into(),
            endpoint_host: "api.openai.com".into(),
            auth_label: "OAuth · work".into(),
            model_id: "gpt-test".into(),
            reasoning_effort: "high".into(),
            selection_scope: "shell".into(),
        };
        write_selection(&path, &selection).unwrap();
        assert_eq!(read_selection(&path).unwrap(), selection);
        let raw = std::fs::read_to_string(&path).unwrap();
        assert_eq!(raw.lines().count(), 8);
        assert!(!raw.to_ascii_lowercase().contains("token"));
        let _ = std::fs::remove_file(path);
    }
}
