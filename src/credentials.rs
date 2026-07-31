//! AWS-style shared credentials for provider API keys.
//!
//! Ordinary provider settings remain in `config.toml`; secrets live in a
//! separate private, versioned file and are resolved through one precedence
//! chain: environment override, setup-only staged value, shared file.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{self, ProviderConfig};

pub const CREDENTIALS_SCHEMA_VERSION: u32 = 1;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_SECRET_BYTES: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    version: u32,
    #[serde(default)]
    profiles: BTreeMap<String, Entry>,
}

impl Default for Store {
    fn default() -> Self {
        Self {
            version: CREDENTIALS_SCHEMA_VERSION,
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    Environment { variable: String },
    Staged { profile: String },
    CredentialsFile { profile: String, path: PathBuf },
    NotRequired,
    Missing { profile: String },
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Self::Environment { variable } => format!("environment:${variable}"),
            Self::Staged { profile } => format!("setup_memory:{profile}"),
            Self::CredentialsFile { profile, .. } => {
                format!("credentials_file:{profile}")
            }
            Self::NotRequired => "not_required".to_string(),
            Self::Missing { .. } => "missing".to_string(),
        }
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, Self::Missing { .. })
    }
}

/// A resolved secret and its safe-to-display provenance. Deliberately does not
/// implement `Debug` or `Serialize`.
pub struct Resolved {
    secret: Option<String>,
    pub source: Source,
}

impl Resolved {
    pub fn into_secret(self) -> Option<String> {
        self.secret
    }

    pub fn secret(&self) -> Option<&str> {
        self.secret.as_deref()
    }
}

thread_local! {
    static STAGED: RefCell<BTreeMap<String, String>> = const {
        RefCell::new(BTreeMap::new())
    };
}

/// Use a secret only while `operation` runs. Setup uses this to validate before
/// Apply without serializing the key into its resumable draft.
pub fn with_staged<T>(profile: &str, secret: String, operation: impl FnOnce() -> T) -> Result<T> {
    let profile = normalize_profile(profile)?;
    validate_secret(&secret)?;
    STAGED.with(|values| {
        values.borrow_mut().insert(profile.clone(), secret);
    });
    struct Clear(String);
    impl Drop for Clear {
        fn drop(&mut self) {
            STAGED.with(|values| {
                values.borrow_mut().remove(&self.0);
            });
        }
    }
    let _clear = Clear(profile);
    Ok(operation())
}

pub fn path() -> PathBuf {
    match std::env::var_os("AISHE_CREDENTIALS_FILE") {
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ => crate::config::Config::path().with_file_name("credentials.toml"),
    }
}

/// Stable profile alias for schema-2 configuration. Known environment names
/// become the service name; arbitrary names are normalized conservatively.
pub fn profile_from_env(variable: &str) -> String {
    let trimmed = variable.trim();
    let base = trimmed
        .strip_suffix("_API_KEY")
        .or_else(|| trimmed.strip_suffix("_TOKEN"))
        .unwrap_or(trimmed);
    let normalized: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let normalized = normalized.trim_matches('-');
    if normalized.is_empty() {
        "default".to_string()
    } else {
        normalized.chars().take(64).collect()
    }
}

pub fn normalize_profile(profile: &str) -> Result<String> {
    let value = profile.trim().to_ascii_lowercase();
    if value.is_empty() || value.len() > 64 {
        anyhow::bail!("credential profile must contain 1–64 characters");
    }
    if !value
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        anyhow::bail!(
            "credential profile '{value}' must start with a letter or digit and \
             contain only letters, digits, '.', '_', or '-'"
        );
    }
    Ok(value)
}

pub fn validate_secret(secret: &str) -> Result<()> {
    if secret.is_empty() {
        anyhow::bail!("API key cannot be empty");
    }
    if secret.len() > MAX_SECRET_BYTES {
        anyhow::bail!("API key is larger than the 16 KiB safety limit");
    }
    if secret
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        anyhow::bail!("API key cannot contain whitespace or control characters");
    }
    Ok(())
}

impl Store {
    pub fn load() -> Result<Option<Self>> {
        Self::load_from(&path())
    }

    fn load_from(file_path: &Path) -> Result<Option<Self>> {
        let metadata = match std::fs::symlink_metadata(file_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", file_path.display()))
            }
        };
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "credentials file {} is a symlink; replace it with a private regular file",
                file_path.display()
            );
        }
        if !metadata.is_file() {
            anyhow::bail!(
                "credentials path {} is not a regular file",
                file_path.display()
            );
        }
        if metadata.len() > MAX_FILE_BYTES {
            anyhow::bail!(
                "credentials file {} exceeds the 1 MiB safety limit",
                file_path.display()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                anyhow::bail!(
                    "credentials file {} has insecure mode {:03o}; run `chmod 600 {}` \
                     or `aishe doctor --fix`",
                    file_path.display(),
                    mode,
                    file_path.display()
                );
            }
        }

        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(file_path)
            .with_context(|| format!("opening credentials file {}", file_path.display()))?;
        let opened_metadata = file
            .metadata()
            .with_context(|| format!("inspecting open credentials file {}", file_path.display()))?;
        if !opened_metadata.is_file() || opened_metadata.len() > MAX_FILE_BYTES {
            anyhow::bail!("credentials file changed while it was being opened");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if opened_metadata.permissions().mode() & 0o077 != 0 {
                anyhow::bail!(
                    "credentials file {} became insecure while it was being opened",
                    file_path.display()
                );
            }
        }
        let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
        file.take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)
            .with_context(|| format!("reading credentials file {}", file_path.display()))?;
        if bytes.len() as u64 > MAX_FILE_BYTES {
            anyhow::bail!(
                "credentials file {} exceeds the 1 MiB safety limit",
                file_path.display()
            );
        }
        let text = std::str::from_utf8(&bytes)
            .with_context(|| format!("credentials file {} is not UTF-8", file_path.display()))?;
        let mut store: Self = toml::from_str(text).map_err(|_| {
            anyhow::anyhow!(
                "credentials file {} contains malformed TOML; no file contents were displayed",
                file_path.display()
            )
        })?;
        if store.version != CREDENTIALS_SCHEMA_VERSION {
            anyhow::bail!(
                "credentials schema {} is unsupported (this AIShe supports {})",
                store.version,
                CREDENTIALS_SCHEMA_VERSION
            );
        }
        let mut normalized = BTreeMap::new();
        for (name, entry) in store.profiles {
            let normalized_name = normalize_profile(&name)?;
            validate_secret(&entry.api_key)
                .with_context(|| format!("validating credential profile '{normalized_name}'"))?;
            if normalized.insert(normalized_name.clone(), entry).is_some() {
                anyhow::bail!(
                    "credentials file contains duplicate normalized profile '{normalized_name}'"
                );
            }
        }
        store.profiles = normalized;
        Ok(Some(store))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&path())
    }

    fn save_to(&self, file_path: &Path) -> Result<()> {
        let parent = file_path.parent().unwrap_or_else(|| Path::new("."));
        let parent_existed = parent.exists();
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating credentials directory {}", parent.display()))?;
        // Always protect AIShe's own config directory. For an explicit
        // AISHE_CREDENTIALS_FILE, protect a directory we created but do not
        // chmod an existing mount/shared parent behind the user's back.
        let explicit_override =
            std::env::var_os("AISHE_CREDENTIALS_FILE").is_some_and(|value| !value.is_empty());
        if !parent_existed || (!explicit_override && file_path == path()) {
            config::set_private_dir(parent);
        }
        match std::fs::symlink_metadata(file_path) {
            Ok(_) => {
                // Validate the existing target before replacing it. This
                // rejects even broken symlinks instead of silently repairing
                // or following them.
                let _ = Self::load_from(file_path)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", file_path.display()))
            }
        }
        let mut persisted = self.clone();
        persisted.version = CREDENTIALS_SCHEMA_VERSION;
        for (name, entry) in &persisted.profiles {
            normalize_profile(name)?;
            validate_secret(&entry.api_key)
                .with_context(|| format!("validating credential profile '{name}'"))?;
        }
        let text = toml::to_string_pretty(&persisted).context("serializing credentials")?;
        config::write_atomic(file_path, text.as_bytes())
            .with_context(|| format!("writing credentials file {}", file_path.display()))?;
        config::set_private_file(file_path);
        Ok(())
    }

    pub fn profile_names(&self) -> Vec<String> {
        self.profiles.keys().cloned().collect()
    }

    pub fn contains(&self, profile: &str) -> Result<bool> {
        Ok(self.profiles.contains_key(&normalize_profile(profile)?))
    }

    pub fn set(&mut self, profile: &str, secret: String) -> Result<String> {
        let profile = normalize_profile(profile)?;
        validate_secret(&secret)?;
        self.profiles
            .insert(profile.clone(), Entry { api_key: secret });
        Ok(profile)
    }

    pub fn remove(&mut self, profile: &str) -> Result<bool> {
        Ok(self.profiles.remove(&normalize_profile(profile)?).is_some())
    }

    fn secret(&self, profile: &str) -> Option<&str> {
        self.profiles
            .get(profile)
            .map(|entry| entry.api_key.as_str())
    }
}

pub fn resolve(provider: &ProviderConfig) -> Result<Resolved> {
    match std::env::var(&provider.api_key_env) {
        Ok(secret) if !secret.trim().is_empty() => {
            validate_secret(&secret)
                .with_context(|| format!("validating ${}", provider.api_key_env))?;
            return Ok(Resolved {
                secret: Some(secret),
                source: Source::Environment {
                    variable: provider.api_key_env.clone(),
                },
            });
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            anyhow::bail!("${} is not valid Unicode", provider.api_key_env)
        }
        _ => {}
    }

    let profile = provider.credential_profile();
    if let Some(secret) = STAGED.with(|values| values.borrow().get(&profile).cloned()) {
        return Ok(Resolved {
            secret: Some(secret),
            source: Source::Staged { profile },
        });
    }

    if let Some(store) = Store::load()? {
        if let Some(secret) = store.secret(&profile) {
            return Ok(Resolved {
                secret: Some(secret.to_string()),
                source: Source::CredentialsFile {
                    profile,
                    path: path(),
                },
            });
        }
    }

    if provider.requires_auth() {
        Ok(Resolved {
            secret: None,
            source: Source::Missing { profile },
        })
    } else {
        Ok(Resolved {
            secret: None,
            source: Source::NotRequired,
        })
    }
}

pub fn require(provider: &ProviderConfig) -> Result<String> {
    let profile = provider.credential_profile();
    let environment = provider.api_key_env.clone();
    let resolved = resolve(provider)?;
    resolved.into_secret().ok_or_else(|| {
        anyhow::anyhow!(
            "API key missing for credential profile '{profile}' — run \
             `aishe auth set {profile}` or set ${environment} for an override"
        )
    })
}

pub fn optional(provider: &ProviderConfig) -> Result<String> {
    let resolved = resolve(provider)?;
    match resolved.into_secret() {
        Some(secret) => Ok(secret),
        None if !provider.requires_auth() => Ok(String::new()),
        None => require(provider),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aishe-credentials-{label}-{}-{}.toml",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ))
    }

    #[test]
    fn profiles_are_normalized_and_validated() {
        assert_eq!(normalize_profile(" OpenAI ").unwrap(), "openai");
        assert!(normalize_profile("../bad").is_err());
        assert!(normalize_profile("-bad").is_err());
        assert_eq!(profile_from_env("OPENAI_API_KEY"), "openai");
        assert_eq!(profile_from_env("MY_PRIVATE_TOKEN"), "my-private");
    }

    #[test]
    fn store_round_trip_and_exact_removal() {
        let file = temp_path("round-trip");
        let _ = std::fs::remove_file(&file);
        let mut store = Store::default();
        store.set("OpenAI", "secret-one".into()).unwrap();
        store.set("groq", "secret-two".into()).unwrap();
        store.save_to(&file).unwrap();
        let mut loaded = Store::load_from(&file).unwrap().unwrap();
        assert_eq!(loaded.profile_names(), vec!["groq", "openai"]);
        assert!(loaded.remove("openai").unwrap());
        assert!(loaded.contains("groq").unwrap());
        assert!(!loaded.contains("openai").unwrap());
        let _ = std::fs::remove_file(file);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_permissive_and_symlinked_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let file = temp_path("permissive");
        let link = file.with_extension("link");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_file(&link);
        std::fs::write(&file, "version = 1\n").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Store::load_from(&file)
            .err()
            .unwrap()
            .to_string()
            .contains("insecure mode"));
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&file, &link).unwrap();
        assert!(Store::load_from(&link)
            .err()
            .unwrap()
            .to_string()
            .contains("symlink"));
        std::fs::remove_file(&link).unwrap();
        symlink(file.with_extension("missing"), &link).unwrap();
        let mut replacement = Store::default();
        replacement.set("openai", "replacement-key".into()).unwrap();
        assert!(replacement
            .save_to(&link)
            .err()
            .unwrap()
            .to_string()
            .contains("symlink"));
        let _ = std::fs::remove_file(link);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn secret_validation_never_accepts_shell_history_shape() {
        assert!(validate_secret("").is_err());
        assert!(validate_secret("key with spaces").is_err());
        assert!(validate_secret("key\nnext").is_err());
        assert!(validate_secret("safe-key_123").is_ok());
    }

    #[test]
    fn malformed_errors_never_repeat_file_contents() {
        let file = temp_path("malformed-redaction");
        let _ = std::fs::remove_file(&file);
        let marker = "low-entropy-secret-that-must-not-appear";
        std::fs::write(
            &file,
            format!("version = 1\n[profiles.openai]\napi_key = {marker}\n"),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let error = Store::load_from(&file).err().unwrap().to_string();
        assert!(error.contains("malformed TOML"));
        assert!(!error.contains(marker));
        let _ = std::fs::remove_file(file);
    }
}
