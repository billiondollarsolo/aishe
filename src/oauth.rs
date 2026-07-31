//! Aishe-owned access to the private OAuth credential store used by the pinned
//! OpenCode runtime.
//!
//! OAuth tokens are deliberately never copied into Aishe configuration,
//! command arguments, audit records, or provider subprocess environments. The
//! managed runtime reads and refreshes this private file directly.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use clap::ValueEnum;
use serde::Serialize;
use serde_json::Value;

use crate::backend::{InstallSource, RuntimeManager, RuntimeStatus};
use crate::config::Config;

const MAX_AUTH_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OAuthProvider {
    Openai,
    Xai,
}

impl OAuthProvider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Xai => "xai",
        }
    }

    fn browser_method(self) -> &'static str {
        match self {
            Self::Openai => "ChatGPT Pro/Plus (browser)",
            Self::Xai => "xAI Grok OAuth (SuperGrok Subscription)",
        }
    }

    fn headless_method(self) -> &'static str {
        match self {
            Self::Openai => "ChatGPT Pro/Plus (headless)",
            Self::Xai => "xAI Grok OAuth (Headless / Remote / VPS)",
        }
    }

    pub fn from_base_url(base_url: &str) -> Option<Self> {
        let normalized = crate::provider_catalog::normalize_base_url(base_url);
        match normalized.as_str() {
            "https://api.openai.com" => Some(Self::Openai),
            "https://api.x.ai" => Some(Self::Xai),
            _ => None,
        }
    }
}

impl fmt::Display for OAuthProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.id())
    }
}

impl std::str::FromStr for OAuthProvider {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" => Ok(Self::Openai),
            "xai" | "grok" => Ok(Self::Xai),
            _ => anyhow::bail!("OAuth provider must be openai or xai"),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OAuthStatus {
    pub provider: OAuthProvider,
    pub profile: String,
    pub available: bool,
    pub credential_type: Option<String>,
    pub access_expires_at_ms: Option<u64>,
    pub access_expired: bool,
    pub path: PathBuf,
}

impl OAuthStatus {
    fn missing(provider: OAuthProvider, profile: &str, path: PathBuf) -> Self {
        Self {
            provider,
            profile: profile.to_string(),
            available: false,
            credential_type: None,
            access_expires_at_ms: None,
            access_expired: false,
            path,
        }
    }
}

/// Exact OpenCode store path inside Aishe's private backend layout.
pub fn path() -> Result<PathBuf> {
    Ok(crate::backend::supervisor::backend_root()?
        .join("opencode")
        .join("xdg")
        .join("data")
        .join("opencode")
        .join("auth.json"))
}

/// Normalize a user-facing OAuth profile label for its private filesystem root.
/// Display always uses the configured original label; this value is only a safe
/// stable path component.
pub fn normalize_profile(value: &str) -> Result<String> {
    crate::config::normalize_connection_id(value)
        .context("OAuth profile must be a safe non-empty label")
}

pub fn path_for(provider: OAuthProvider, profile: &str) -> Result<PathBuf> {
    let profile = normalize_profile(profile)?;
    Ok(crate::backend::supervisor::backend_root()?
        .join("opencode")
        .join("profiles")
        .join(provider.id())
        .join(profile)
        .join("xdg")
        .join("data")
        .join("opencode")
        .join("auth.json"))
}

pub fn status(provider: OAuthProvider) -> Result<OAuthStatus> {
    status_from(&path()?, provider, "default")
}

pub fn status_profile(provider: OAuthProvider, profile: &str) -> Result<OAuthStatus> {
    status_from(&path_for(provider, profile)?, provider, profile)
}

pub fn active_provider(config: &Config) -> Result<Option<OAuthProvider>> {
    let provider_config = config.active_provider_config();
    if !provider_config.requires_auth() {
        return Ok(None);
    }
    let Some(provider) = OAuthProvider::from_base_url(&provider_config.base_url) else {
        return Ok(None);
    };
    if let Some(connection) = config.active_connection() {
        match &connection.auth {
            crate::config::ConnectionAuth::OAuth { profile } => {
                return Ok(status_profile(provider, profile)?
                    .available
                    .then_some(provider));
            }
            crate::config::ConnectionAuth::ApiKey { .. } | crate::config::ConnectionAuth::None => {
                return Ok(None)
            }
            crate::config::ConnectionAuth::Auto => {}
        }
    }
    Ok(status(provider)?.available.then_some(provider))
}

pub fn login(provider: OAuthProvider, headless: bool, browser: bool) -> Result<u8> {
    login_inner(provider, None, headless, browser)
}

pub fn login_profile(
    provider: OAuthProvider,
    profile: &str,
    headless: bool,
    browser: bool,
) -> Result<u8> {
    let normalized_profile = normalize_profile(profile)?;
    login_inner(
        provider,
        Some((&normalized_profile, profile)),
        headless,
        browser,
    )
}

fn login_inner(
    provider: OAuthProvider,
    profile: Option<(&str, &str)>,
    headless: bool,
    browser: bool,
) -> Result<u8> {
    let manager = RuntimeManager::new()?;
    if !matches!(manager.status(), RuntimeStatus::Ready { .. }) {
        println!(
            "installing the pinned OpenCode {} runtime for OAuth...",
            manager.manifest().version
        );
        manager
            .install(InstallSource::Default, false)
            .context("installing the managed OAuth runtime")?;
    }
    manager.verify()?;

    let prepared = if let Some((normalized, _)) = profile {
        crate::backend::supervisor::prepare_profile_layout(provider.id(), normalized)?
    } else {
        crate::backend::supervisor::prepare_layout()?
    };
    let remote = std::env::var_os("SSH_CONNECTION").is_some()
        || std::env::var_os("SSH_TTY").is_some()
        || !std::io::stdout().is_terminal();
    let use_headless = headless || (!browser && remote);
    let method = if use_headless {
        provider.headless_method()
    } else {
        provider.browser_method()
    };
    println!("OAuth provider: {provider}");
    println!(
        "profile: {}",
        profile.map(|(_, label)| label).unwrap_or("default")
    );
    println!(
        "flow: {}",
        if use_headless {
            "headless/device"
        } else {
            "local browser"
        }
    );

    let auth_config = serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "share": "disabled",
        "autoupdate": false,
        "snapshot": false,
        "mcp": {},
        "lsp": false,
        "formatter": false,
        "instructions": [],
        "skills": {"paths": []},
        "plugin": [],
        "permission": {"*": "deny"},
        "enabled_providers": [provider.id()]
    });
    let mut command = Command::new(manager.binary_path());
    command
        .args([
            "auth",
            "login",
            "--provider",
            provider.id(),
            "--method",
            method,
        ])
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .current_dir(&prepared.root)
        .env_clear()
        .env("HOME", &prepared.home)
        .env("XDG_CONFIG_HOME", prepared.root.join("xdg").join("config"))
        .env("XDG_DATA_HOME", &prepared.data_dir)
        .env("XDG_CACHE_HOME", &prepared.cache_dir)
        .env("XDG_STATE_HOME", &prepared.state_dir)
        .env("OPENCODE_CONFIG_DIR", &prepared.auth_config_dir)
        .env(
            "OPENCODE_CONFIG_CONTENT",
            serde_json::to_string(&auth_config)?,
        )
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
        .env("OPENCODE_DISABLE_LSP_DOWNLOAD", "1")
        .env("NO_COLOR", "1");
    crate::backend::supervisor::copy_safe_environment(&mut command);

    let result = command
        .status()
        .with_context(|| format!("starting managed OAuth flow for {provider}"))?;
    if !result.success() {
        return Ok(result.code().unwrap_or(1).clamp(1, 255) as u8);
    }
    let credential = if let Some((normalized, _)) = profile {
        status_profile(provider, normalized)?
    } else {
        status(provider)?
    };
    if !credential.available {
        anyhow::bail!(
            "OAuth flow exited successfully but did not save a valid {provider} OAuth credential"
        );
    }
    if let Some(parent) = credential.path.parent() {
        crate::config::set_private_dir(parent);
    }
    crate::config::set_private_file(&credential.path);
    let _ = crate::connection::invalidate_oauth_profile(
        provider,
        profile.map(|(normalized, _)| normalized),
    );
    println!(
        "Aishe OAuth credential for {provider} profile '{}' is ready",
        profile.map(|(_, label)| label).unwrap_or("default")
    );
    Ok(0)
}

pub fn logout(provider: OAuthProvider) -> Result<bool> {
    let removed = remove_from(&path()?, provider)?;
    if removed {
        let _ = crate::connection::invalidate_oauth_profile(provider, None);
    }
    Ok(removed)
}

pub fn logout_profile(provider: OAuthProvider, profile: &str) -> Result<bool> {
    let removed = remove_from(&path_for(provider, profile)?, provider)?;
    if removed {
        let _ = crate::connection::invalidate_oauth_profile(provider, Some(profile));
    }
    Ok(removed)
}

fn status_from(file: &Path, provider: OAuthProvider, profile: &str) -> Result<OAuthStatus> {
    let Some(values) = load_from(file)? else {
        return Ok(OAuthStatus::missing(provider, profile, file.to_path_buf()));
    };
    let Some(entry) = values.get(provider.id()).and_then(Value::as_object) else {
        return Ok(OAuthStatus::missing(provider, profile, file.to_path_buf()));
    };
    let credential_type = entry
        .get("type")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let expires = entry.get("expires").and_then(Value::as_u64);
    let valid_oauth = credential_type.as_deref() == Some("oauth")
        && entry
            .get("refresh")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        && entry
            .get("access")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
        && expires.is_some();
    Ok(OAuthStatus {
        provider,
        profile: profile.to_string(),
        available: valid_oauth,
        credential_type,
        access_expires_at_ms: expires,
        access_expired: expires.is_some_and(|expires| expires <= now_ms()),
        path: file.to_path_buf(),
    })
}

fn remove_from(file: &Path, provider: OAuthProvider) -> Result<bool> {
    let Some(mut values) = load_from(file)? else {
        return Ok(false);
    };
    if values.remove(provider.id()).is_none() {
        return Ok(false);
    }
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .with_context(|| format!("creating OAuth credential directory {}", parent.display()))?;
    crate::config::set_private_dir(parent);
    let mut bytes = serde_json::to_vec_pretty(&values)?;
    bytes.push(b'\n');
    crate::config::write_atomic(file, &bytes)
        .with_context(|| format!("writing OAuth credential store {}", file.display()))?;
    crate::config::set_private_file(file);
    Ok(true)
}

fn load_from(file: &Path) -> Result<Option<BTreeMap<String, Value>>> {
    let metadata = match fs::symlink_metadata(file) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("inspecting {}", file.display())),
    };
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "refusing symlinked OAuth credential store {}",
            file.display()
        );
    }
    if !metadata.is_file() {
        anyhow::bail!(
            "OAuth credential store is not a regular file: {}",
            file.display()
        );
    }
    if metadata.len() > MAX_AUTH_BYTES {
        anyhow::bail!("OAuth credential store exceeds the 1 MiB safety limit");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 {
            anyhow::bail!(
                "OAuth credential store has insecure mode {:o}; expected 600",
                metadata.permissions().mode() & 0o777
            );
        }
        if metadata.uid() != unsafe { libc::geteuid() } {
            anyhow::bail!("OAuth credential store is not owned by the current user");
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let opened = options
        .open(file)
        .with_context(|| format!("opening OAuth credential store {}", file.display()))?;
    let opened_metadata = opened
        .metadata()
        .with_context(|| format!("inspecting open OAuth credential store {}", file.display()))?;
    if !opened_metadata.is_file() || opened_metadata.len() > MAX_AUTH_BYTES {
        anyhow::bail!("OAuth credential store changed while it was being opened");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if opened_metadata.permissions().mode() & 0o077 != 0
            || opened_metadata.uid() != unsafe { libc::geteuid() }
        {
            anyhow::bail!("OAuth credential store became insecure while it was being opened");
        }
    }
    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    opened
        .take(MAX_AUTH_BYTES + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("reading {}", file.display()))?;
    if bytes.len() as u64 > MAX_AUTH_BYTES {
        anyhow::bail!("OAuth credential store exceeds the 1 MiB safety limit");
    }
    let values = serde_json::from_slice::<BTreeMap<String, Value>>(&bytes)
        .context("OAuth credential store is malformed JSON")?;
    Ok(Some(values))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aishe-oauth-{label}-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    fn write(file: &Path, value: Value) {
        let _ = fs::remove_file(file);
        fs::write(file, serde_json::to_vec(&value).unwrap()).unwrap();
        crate::config::set_private_file(file);
    }

    #[test]
    fn recognizes_only_complete_oauth_entries_without_exposing_tokens() {
        let file = file("status");
        write(
            &file,
            serde_json::json!({
                "openai": {"type":"oauth","refresh":"refresh-secret","access":"access-secret","expires":1},
                "xai": {"type":"api","key":"api-secret"}
            }),
        );
        let openai = status_from(&file, OAuthProvider::Openai, "default").unwrap();
        assert!(openai.available);
        assert!(openai.access_expired);
        assert_eq!(openai.credential_type.as_deref(), Some("oauth"));
        let serialized = serde_json::to_string(&openai).unwrap();
        assert!(!serialized.contains("secret"));
        let xai = status_from(&file, OAuthProvider::Xai, "default").unwrap();
        assert!(!xai.available);
        assert_eq!(xai.credential_type.as_deref(), Some("api"));
        let _ = fs::remove_file(file);
    }

    #[test]
    fn exact_logout_preserves_other_provider_credentials() {
        let file = file("logout");
        write(
            &file,
            serde_json::json!({
                "openai": {"type":"oauth","refresh":"one","access":"two","expires":9999999999999u64},
                "xai": {"type":"oauth","refresh":"three","access":"four","expires":9999999999999u64}
            }),
        );
        assert!(remove_from(&file, OAuthProvider::Openai).unwrap());
        assert!(
            !status_from(&file, OAuthProvider::Openai, "default")
                .unwrap()
                .available
        );
        assert!(
            status_from(&file, OAuthProvider::Xai, "default")
                .unwrap()
                .available
        );
        let _ = fs::remove_file(file);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_permissive_and_symlinked_stores() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let file = file("permissions");
        let link = file.with_extension("link");
        write(&file, serde_json::json!({}));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(status_from(&file, OAuthProvider::Openai, "default")
            .unwrap_err()
            .to_string()
            .contains("insecure mode"));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        let _ = fs::remove_file(&link);
        symlink(&file, &link).unwrap();
        assert!(status_from(&link, OAuthProvider::Openai, "default")
            .unwrap_err()
            .to_string()
            .contains("symlinked"));
        let _ = fs::remove_file(link);
        let _ = fs::remove_file(file);
    }

    #[test]
    fn endpoint_binding_is_exact() {
        assert_eq!(
            OAuthProvider::from_base_url("https://api.openai.com/v1"),
            Some(OAuthProvider::Openai)
        );
        assert_eq!(
            OAuthProvider::from_base_url("https://api.x.ai"),
            Some(OAuthProvider::Xai)
        );
        assert_eq!(OAuthProvider::from_base_url("https://openai.example"), None);
        assert_eq!(OAuthProvider::from_base_url("http://api.openai.com"), None);
        assert_eq!(
            OAuthProvider::from_base_url("https://api.openai.com/tenant"),
            None
        );
    }

    #[test]
    fn labeled_profiles_have_distinct_private_store_paths() {
        let work = path_for(OAuthProvider::Openai, "work").unwrap();
        let personal = path_for(OAuthProvider::Openai, "personal").unwrap();
        let xai = path_for(OAuthProvider::Xai, "work").unwrap();
        assert_ne!(work, personal);
        assert_ne!(work, xai);
        assert!(work.ends_with("work/xdg/data/opencode/auth.json"));
        assert!(personal.ends_with("personal/xdg/data/opencode/auth.json"));
    }
}
