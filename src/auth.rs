//! CLI workflow for AWS-style named shared credentials.

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config::{Config, ProviderConfig};
use crate::credentials::{self, Source, Store};

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Sign in with a provider subscription through AIShe's private runtime.
    Login {
        /// OAuth provider: openai or xai.
        #[arg(value_enum)]
        provider: Option<crate::oauth::OAuthProvider>,
        /// Use the provider/profile bound to a named OAuth connection.
        #[arg(long, conflicts_with_all = ["provider", "profile"])]
        connection: Option<String>,
        /// Isolated OAuth account label (for example work or personal).
        #[arg(long)]
        profile: Option<String>,
        /// Use device authorization (recommended for SSH, containers, and VPS hosts).
        #[arg(long, conflicts_with = "browser")]
        headless: bool,
        /// Force a local loopback-browser flow, even when SSH is detected.
        #[arg(long, conflicts_with = "headless")]
        browser: bool,
    },
    /// Remove one provider OAuth credential from AIShe's private runtime.
    Logout {
        /// OAuth provider: openai or xai.
        #[arg(value_enum)]
        provider: Option<crate::oauth::OAuthProvider>,
        /// Use the provider/profile bound to a named OAuth connection.
        #[arg(long, conflicts_with_all = ["provider", "profile"])]
        connection: Option<String>,
        /// Isolated OAuth account label.
        #[arg(long)]
        profile: Option<String>,
        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Save or replace a credential. Secret values are never command arguments.
    Set {
        /// Credential profile (defaults to the active provider's profile).
        profile: Option<String>,
        /// Read the key from standard input.
        #[arg(long, conflicts_with = "from_env")]
        stdin: bool,
        /// Copy the key from an environment variable without printing it.
        #[arg(long, value_name = "VARIABLE", conflicts_with = "stdin")]
        from_env: Option<String>,
    },
    /// Show whether a credential is available and which layer supplies it.
    Status {
        /// Credential profile, or OAuth provider when --profile is supplied.
        target: Option<String>,
        /// Report the authentication state for a named connection.
        #[arg(long, conflicts_with_all = ["target", "profile"])]
        connection: Option<String>,
        /// Report an isolated OAuth profile instead of an API-key profile.
        #[arg(long)]
        profile: Option<String>,
        /// Emit stable JSON with no secret material.
        #[arg(long)]
        json: bool,
    },
    /// List saved profile names. Values are never displayed.
    List {
        /// Emit stable JSON with no secret material.
        #[arg(long)]
        json: bool,
    },
    /// Remove one saved credential profile.
    Remove {
        /// Credential profile (defaults to the active provider's profile).
        profile: Option<String>,
        /// Skip the interactive confirmation.
        #[arg(long)]
        yes: bool,
    },
    /// Print the active shared-credentials file path.
    Path,
}

fn configured_provider(config: &Config, profile: &str) -> Option<ProviderConfig> {
    let active = config
        .active_connection()
        .map(|connection| connection.provider_view())
        .unwrap_or_else(|| config.active_provider_config().clone());
    if active.credential_profile() == profile {
        return Some(active);
    }
    if let Some(provider) = config
        .connections
        .values()
        .map(|connection| connection.provider_view())
        .find(|provider| provider.credential_profile() == profile)
    {
        return Some(provider);
    }
    [&config.providers.anthropic, &config.providers.openai]
        .into_iter()
        .find(|provider| provider.credential_profile() == profile)
        .cloned()
}

/// Resolve a write/status target only from explicit input or user config. A
/// project overlay can never redirect a credential command.
fn target(requested: Option<&str>) -> Result<(String, Option<ProviderConfig>)> {
    let config_result = Config::load_quiet();
    if let Some(requested) = requested {
        let profile = credentials::normalize_profile(requested)?;
        let provider = config_result
            .ok()
            .flatten()
            .and_then(|config| configured_provider(&config, &profile));
        return Ok((profile, provider));
    }
    let config = config_result?.context(
        "no config exists to select an active credential profile; specify PROFILE or run `aishe setup`",
    )?;
    let provider = config
        .active_connection()
        .map(|connection| connection.provider_view())
        .unwrap_or_else(|| config.active_provider_config().clone());
    Ok((provider.credential_profile(), Some(provider)))
}

fn oauth_target(
    provider: Option<crate::oauth::OAuthProvider>,
    connection: Option<&str>,
    profile: Option<&str>,
) -> Result<(crate::oauth::OAuthProvider, Option<String>)> {
    if let Some(connection) = connection {
        let config = Config::load_quiet()?
            .context("no config exists to resolve the requested OAuth connection")?;
        let id = config.resolve_connection_id(connection)?;
        let configured = config
            .connections
            .get(&id)
            .with_context(|| format!("connection '{id}' disappeared"))?;
        let crate::config::ConnectionAuth::OAuth { profile } = &configured.auth else {
            anyhow::bail!(
                "connection '{id}' does not use OAuth; its auth type is {}",
                configured.auth_label()
            );
        };
        let provider = crate::oauth::OAuthProvider::from_base_url(&configured.settings.base_url)
            .with_context(|| format!("connection '{id}' has no supported OAuth endpoint"))?;
        return Ok((provider, Some(profile.clone())));
    }
    let provider = provider.context("specify PROVIDER or --connection ID")?;
    Ok((provider, profile.map(ToOwned::to_owned)))
}

/// Return `None` only for a migrated legacy-auto connection, whose existing
/// combined API-key/OAuth status format remains backward compatible.
fn connection_status(requested: Option<&str>, json: bool) -> Result<Option<u8>> {
    let Some(config) = Config::load_quiet()? else {
        return Ok(None);
    };
    let id = requested
        .map(|value| config.resolve_connection_id(value))
        .transpose()?
        .unwrap_or_else(|| config.active_connection_id().to_string());
    let connection = config
        .connections
        .get(&id)
        .with_context(|| format!("connection '{id}' disappeared"))?;
    if requested.is_none() && matches!(connection.auth, crate::config::ConnectionAuth::Auto) {
        return Ok(None);
    }
    let status = crate::connection::auth_status(connection);
    if json {
        crate::cli::json_contract::print_object(&serde_json::json!({
            "connection_id": id,
            "connection_label": connection.label,
            "provider": connection.provider,
            "auth": status,
        }))?;
    } else {
        println!("connection: {id} ({})", connection.label);
        println!("provider: {}", connection.provider);
        println!(
            "auth: {}{} · {}",
            status.kind,
            status
                .profile
                .as_deref()
                .map(|profile| format!(" · {profile}"))
                .unwrap_or_default(),
            status.detail
        );
        if !status.available {
            match &connection.auth {
                crate::config::ConnectionAuth::OAuth { profile } => {
                    if let Some(provider) =
                        crate::oauth::OAuthProvider::from_base_url(&connection.settings.base_url)
                    {
                        println!(
                            "next: aishe auth login {provider} --profile {}",
                            crate::commands::display_safe(profile)
                        );
                    }
                }
                crate::config::ConnectionAuth::ApiKey { credential, .. } => println!(
                    "next: aishe auth set {}",
                    crate::commands::display_safe(
                        credential
                            .as_deref()
                            .unwrap_or(&connection.settings.credential)
                    )
                ),
                crate::config::ConnectionAuth::None | crate::config::ConnectionAuth::Auto => {}
            }
        }
    }
    Ok(Some(if status.available { 0 } else { 1 }))
}

pub fn run(command: &AuthCommand) -> Result<u8> {
    match command {
        AuthCommand::Login {
            provider,
            connection,
            profile,
            headless,
            browser,
        } => {
            let bound_connection = connection.clone();
            let (provider, profile) =
                oauth_target(*provider, connection.as_deref(), profile.as_deref())?;
            let code = if let Some(profile) = &profile {
                crate::oauth::login_profile(provider, profile, *headless, *browser)?
            } else {
                crate::oauth::login(provider, *headless, *browser)?
            };
            if code == 0 {
                // Login only writes the private OAuth store. Ensure a selectable
                // connection exists so /connection and statusline can use it.
                // Skip when the user logged in against an explicit --connection.
                if bound_connection.is_none() {
                    let profile_label = profile.as_deref().unwrap_or("default");
                    if let Err(error) = ensure_oauth_connection_after_login(provider, profile_label)
                    {
                        eprintln!(
                            "aishe: OAuth is ready, but could not update connections: {}",
                            crate::commands::display_safe(&error.to_string())
                        );
                    }
                }
            }
            Ok(code)
        }
        AuthCommand::Logout {
            provider,
            connection,
            profile,
            yes,
        } => {
            let (provider, profile) =
                oauth_target(*provider, connection.as_deref(), profile.as_deref())?;
            if !*yes {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("refusing non-interactive OAuth logout without --yes");
                }
                let confirmed = crate::promptui::confirm(
                    &format!(
                        "Remove AIShe OAuth credential for '{}/{}'?",
                        provider,
                        profile.as_deref().unwrap_or("default")
                    ),
                    false,
                )?;
                if confirmed != Some(true) {
                    println!("OAuth logout cancelled");
                    return Ok(0);
                }
            }
            let removed = if let Some(profile) = &profile {
                crate::oauth::logout_profile(provider, profile)?
            } else {
                crate::oauth::logout(provider)?
            };
            if removed {
                println!(
                    "AIShe OAuth credential for '{}/{}' removed",
                    provider,
                    profile.as_deref().unwrap_or("default")
                );
                Ok(0)
            } else {
                eprintln!(
                    "aishe: no OAuth credential is saved for '{}/{}'",
                    provider,
                    profile.as_deref().unwrap_or("default")
                );
                Ok(1)
            }
        }
        AuthCommand::Path => {
            println!("{}", credentials::path().display());
            Ok(0)
        }
        AuthCommand::Set {
            profile,
            stdin,
            from_env,
        } => {
            let (profile, _) = target(profile.as_deref())?;
            let secret = if *stdin {
                let mut bytes = Vec::new();
                std::io::stdin()
                    .take((credentials::MAX_SECRET_BYTES + 2) as u64)
                    .read_to_end(&mut bytes)
                    .context("reading API key from stdin")?;
                if bytes.len() > credentials::MAX_SECRET_BYTES + 1 {
                    anyhow::bail!("API key is larger than the 16 KiB safety limit");
                }
                let mut value = String::from_utf8(bytes).context("API key input is not UTF-8")?;
                if value.ends_with('\n') {
                    value.pop();
                    if value.ends_with('\r') {
                        value.pop();
                    }
                }
                value
            } else if let Some(variable) = from_env {
                std::env::var(variable)
                    .with_context(|| format!("environment variable ${variable} is not set"))?
            } else {
                crate::promptui::secret(
                    &format!("API key for credential profile '{profile}'"),
                    credentials::MAX_SECRET_BYTES,
                )?
                .context("credential entry cancelled")?
            };
            credentials::validate_secret(&secret)?;
            let mut store = Store::load()?.unwrap_or_default();
            store.set(&profile, secret)?;
            store.save()?;
            let _ = crate::connection::invalidate_api_key_profile(&profile);
            println!(
                "credential profile '{}' saved to {}",
                profile,
                credentials::path().display()
            );
            Ok(0)
        }
        AuthCommand::Status {
            target: requested,
            connection,
            profile: oauth_profile,
            json,
        } => {
            if connection.is_some() || (requested.is_none() && oauth_profile.is_none()) {
                if let Some(code) = connection_status(connection.as_deref(), *json)? {
                    return Ok(code);
                }
            }
            if let Some(oauth_profile) = oauth_profile {
                let provider = requested
                    .as_deref()
                    .context("OAuth status requires PROVIDER when --profile is used")?
                    .parse::<crate::oauth::OAuthProvider>()?;
                let status = crate::oauth::status_profile(provider, oauth_profile)?;
                if *json {
                    crate::cli::json_contract::print_object(&status)?;
                } else {
                    println!(
                        "oauth: {}/{} ({})",
                        provider,
                        crate::commands::display_safe(oauth_profile),
                        if status.available { "ready" } else { "missing" }
                    );
                    println!("store: {}", status.path.display());
                }
                return Ok(if status.available { 0 } else { 1 });
            }
            let (profile, provider) = target(requested.as_deref())?;
            let oauth_provider = provider
                .as_ref()
                .and_then(|provider| crate::oauth::OAuthProvider::from_base_url(&provider.base_url))
                .or(match profile.as_str() {
                    "openai" => Some(crate::oauth::OAuthProvider::Openai),
                    "xai" => Some(crate::oauth::OAuthProvider::Xai),
                    _ => None,
                });
            let source = if let Some(provider) = &provider {
                credentials::resolve(provider)?.source
            } else {
                let stored =
                    Store::load()?.is_some_and(|store| store.contains(&profile).unwrap_or(false));
                if stored {
                    Source::CredentialsFile {
                        profile: profile.clone(),
                        path: credentials::path(),
                    }
                } else {
                    Source::Missing {
                        profile: profile.clone(),
                    }
                }
            };
            let oauth = oauth_provider.map(crate::oauth::status).transpose()?;
            let oauth_available = oauth.as_ref().is_some_and(|status| status.available);
            let selected = if source.is_available() {
                "api_key"
            } else if oauth_available {
                "oauth"
            } else {
                "none"
            };
            if *json {
                crate::cli::json_contract::print_object(&serde_json::json!({
                    "profile": profile,
                    "available": source.is_available() || oauth_available,
                    "api_key_available": source.is_available(),
                    "source": source,
                    "oauth": oauth,
                    "selected": selected,
                    "usable": source.is_available() || oauth_available,
                    "credentials_file": credentials::path(),
                }))?;
            } else {
                println!("profile: {profile}");
                println!("API key: {}", source.label());
                if let Some(oauth) = &oauth {
                    println!(
                        "OAuth: {}{}",
                        if oauth.available {
                            "available"
                        } else {
                            "not saved"
                        },
                        if oauth.access_expired && oauth.available {
                            " (access token will refresh on use)"
                        } else {
                            ""
                        }
                    );
                    println!("OAuth store: {}", oauth.path.display());
                }
                println!("selected: {selected}");
                println!("API key store: {}", credentials::path().display());
            }
            Ok(if source.is_available() || oauth_available {
                0
            } else {
                1
            })
        }
        AuthCommand::List { json } => {
            let store = Store::load()?.unwrap_or_default();
            let mut profiles: std::collections::BTreeSet<String> =
                store.profile_names().into_iter().collect();
            let oauth_statuses = [
                crate::oauth::status(crate::oauth::OAuthProvider::Openai)?,
                crate::oauth::status(crate::oauth::OAuthProvider::Xai)?,
            ];
            let mut oauth_profiles = std::collections::BTreeMap::<String, bool>::new();
            for status in &oauth_statuses {
                if status.credential_type.is_some() {
                    profiles.insert(status.provider.id().to_string());
                    oauth_profiles.insert(status.provider.id().to_string(), status.available);
                }
            }
            let config = Config::load_quiet().ok().flatten();
            if let Some(config) = &config {
                profiles.insert(config.providers.anthropic.credential_profile());
                profiles.insert(config.providers.openai.credential_profile());
                for connection in config.connections.values() {
                    let crate::config::ConnectionAuth::OAuth { profile } = &connection.auth else {
                        continue;
                    };
                    let Some(provider) =
                        crate::oauth::OAuthProvider::from_base_url(&connection.settings.base_url)
                    else {
                        continue;
                    };
                    let display = format!("{provider}/{profile}");
                    let available = crate::oauth::status_profile(provider, profile)?.available;
                    profiles.insert(display.clone());
                    oauth_profiles.insert(display, available);
                }
            }
            let rows: Vec<_> = profiles
                .into_iter()
                .map(|profile| {
                    let saved = store.contains(&profile).unwrap_or(false);
                    let oauth = oauth_profiles.get(&profile).copied().unwrap_or(false);
                    let active = config.as_ref().is_some_and(|config| {
                        let Some(connection) = config.active_connection() else {
                            return false;
                        };
                        match &connection.auth {
                            crate::config::ConnectionAuth::OAuth {
                                profile: active_profile,
                            } => format!("{}/{active_profile}", connection.provider) == profile,
                            _ => connection.settings.credential_profile() == profile,
                        }
                    });
                    serde_json::json!({
                        "profile": profile,
                        "saved": saved,
                        "oauth": oauth,
                        "active": active,
                    })
                })
                .collect();
            if *json {
                crate::cli::json_contract::print_envelope("profiles", &rows)?;
            } else if rows.is_empty() {
                println!("no credential profiles configured");
            } else {
                println!("PROFILE\tAPI KEY\tOAUTH\tACTIVE");
                for row in rows {
                    println!(
                        "{}\t{}\t{}\t{}",
                        row["profile"].as_str().unwrap_or(""),
                        if row["saved"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        },
                        if row["oauth"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        },
                        if row["active"].as_bool().unwrap_or(false) {
                            "yes"
                        } else {
                            "no"
                        }
                    );
                }
            }
            Ok(0)
        }
        AuthCommand::Remove { profile, yes } => {
            let (profile, _) = target(profile.as_deref())?;
            let mut store = Store::load()?.unwrap_or_default();
            if !store.contains(&profile)? {
                eprintln!("aishe: credential profile '{profile}' is not saved");
                return Ok(1);
            }
            if !*yes {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("refusing non-interactive removal without --yes");
                }
                let confirmed = crate::promptui::confirm(
                    &format!("Remove saved credential profile '{profile}'?"),
                    false,
                )?;
                if confirmed != Some(true) {
                    println!("credential removal cancelled");
                    return Ok(0);
                }
            }
            store.remove(&profile)?;
            store.save()?;
            let _ = crate::connection::invalidate_api_key_profile(&profile);
            println!("credential profile '{profile}' removed");
            Ok(0)
        }
    }
}

/// Stable list of connection ids that already bind this OAuth provider+profile.
/// Sorted for deterministic UX (no silent first-only drop when many match).
pub(crate) fn matching_oauth_connection_ids(
    config: &Config,
    provider: crate::oauth::OAuthProvider,
    profile: &str,
) -> Vec<String> {
    let mut ids: Vec<String> = config
        .connections
        .iter()
        .filter(|(_, connection)| {
            matches!(
                &connection.auth,
                crate::config::ConnectionAuth::OAuth { profile: p } if p == profile
            ) && crate::oauth::OAuthProvider::from_base_url(&connection.settings.base_url)
                == Some(provider)
        })
        .map(|(id, _)| id.clone())
        .collect();
    ids.sort();
    ids
}

/// Force the connection label to the canonical OAuth brand (plan default:
/// always sync on login so Codex/Grok brands stay authoritative).
pub(crate) fn apply_canonical_oauth_label(
    connection: &mut crate::config::ConnectionConfig,
    provider: crate::oauth::OAuthProvider,
    profile: &str,
) -> bool {
    let label = crate::config::oauth_connection_label(provider, profile);
    if connection.label == label {
        return false;
    }
    connection.label = label;
    true
}

/// Pure decision: after login, either create a new connection id, or list
/// existing matches (never create a duplicate when any match exists).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OAuthEnsurePlan {
    Create { id: String },
    Existing { ids: Vec<String> },
}

pub(crate) fn plan_oauth_ensure(
    config: &Config,
    provider: crate::oauth::OAuthProvider,
    profile: &str,
) -> Result<OAuthEnsurePlan> {
    let existing = matching_oauth_connection_ids(config, provider, profile);
    if existing.is_empty() {
        Ok(OAuthEnsurePlan::Create {
            id: unique_oauth_connection_id(config, provider, profile)?,
        })
    } else {
        Ok(OAuthEnsurePlan::Existing { ids: existing })
    }
}

/// After a successful bare `auth login`, ensure a selectable connection binds
/// that OAuth provider/profile so `/connection` and the statusline can use it.
fn ensure_oauth_connection_after_login(
    provider: crate::oauth::OAuthProvider,
    profile: &str,
) -> Result<()> {
    let profile = crate::oauth::normalize_profile(profile)?;
    let label = crate::config::oauth_connection_label(provider, &profile);
    let mut config = match Config::load_quiet()? {
        Some(config) => config,
        None => Config::load_or_init()?,
    };

    match plan_oauth_ensure(&config, provider, &profile)? {
        OAuthEnsurePlan::Existing { ids } => {
            if ids.len() == 1 {
                let id = &ids[0];
                println!(
                    "connection '{}' already uses {} — select it with `/connection` or `aishe connection use {}`",
                    crate::commands::display_safe(id),
                    crate::commands::display_safe(&label),
                    crate::commands::display_safe(id)
                );
            } else {
                println!(
                    "{} connections already use {} OAuth · {}:",
                    ids.len(),
                    provider,
                    crate::commands::display_safe(&profile)
                );
                for id in &ids {
                    let shown = config
                        .connections
                        .get(id)
                        .map(|c| c.label.as_str())
                        .unwrap_or(id.as_str());
                    println!(
                        "  {} ({})",
                        crate::commands::display_safe(id),
                        crate::commands::display_safe(shown)
                    );
                }
                println!(
                    "pick one with `/connection` or `aishe connection use ID` (no new connection created)"
                );
            }
            let mut labels_changed = false;
            for id in &ids {
                if let Some(connection) = config.connections.get_mut(id) {
                    if apply_canonical_oauth_label(connection, provider, &profile) {
                        labels_changed = true;
                    }
                }
            }
            if labels_changed {
                config.validate_connections()?;
                config.save()?;
            }
            // Offer the first sorted match; operator can still switch via /connection.
            offer_use_connection(&mut config, &ids[0])?;
            Ok(())
        }
        OAuthEnsurePlan::Create { id } => {
            let service_key = provider.id(); // openai | xai
            let mut settings = config.providers.openai.clone();
            if let Some(service) = crate::provider_catalog::find(service_key) {
                crate::provider_catalog::apply(service, &mut settings);
            }
            let auth = crate::config::ConnectionAuth::OAuth {
                profile: profile.clone(),
            };
            config.connections.insert(
                id.clone(),
                crate::config::ConnectionConfig {
                    provider: service_key.into(),
                    label: label.clone(),
                    settings,
                    auth,
                    reasoning_effort: None,
                },
            );
            config.validate_connections()?;
            config.save()?;
            println!(
                "created connection '{}' ({}) bound to {} OAuth · {}",
                crate::commands::display_safe(&id),
                crate::commands::display_safe(&label),
                provider,
                crate::commands::display_safe(&profile)
            );
            println!(
                "select it: /connection  or  aishe connection use {}",
                crate::commands::display_safe(&id)
            );
            offer_use_connection(&mut config, &id)?;
            Ok(())
        }
    }
}

fn unique_oauth_connection_id(
    config: &Config,
    provider: crate::oauth::OAuthProvider,
    profile: &str,
) -> Result<String> {
    let base = if profile == "default" {
        provider.id().to_string()
    } else {
        format!("{}-{}", provider.id(), profile)
    };
    let base = crate::config::normalize_connection_id(&base)?;
    if !config.connections.contains_key(&base) {
        return Ok(base);
    }
    for index in 2..100 {
        let candidate = format!("{base}-{index}");
        if !config.connections.contains_key(&candidate) {
            return Ok(candidate);
        }
    }
    anyhow::bail!("could not allocate a free connection id for {base}")
}

fn offer_use_connection(config: &mut Config, id: &str) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Ok(());
    }
    if config.active_connection_id() == id {
        return Ok(());
    }
    let use_it = crate::promptui::confirm(
        &format!(
            "Switch active connection to '{}' now?",
            crate::commands::display_safe(id)
        ),
        true,
    )?;
    if use_it != Some(true) {
        return Ok(());
    }
    config.select_connection(id)?;
    config.save()?;
    let _ = crate::connection::write_shell_selection(config, "default");
    println!(
        "connection = {} · model = {} (saved default)",
        crate::commands::display_safe(config.active_connection_id()),
        crate::commands::display_safe(config.active_model())
    );
    Ok(())
}

#[cfg(test)]
mod oauth_ensure_tests {
    use super::*;
    use crate::config::{ConnectionAuth, ConnectionConfig, ProviderConfig};
    use crate::oauth::OAuthProvider;

    fn oauth_conn(label: &str, profile: &str, base_url: &str) -> ConnectionConfig {
        ConnectionConfig {
            provider: "openai".into(),
            label: label.into(),
            settings: ProviderConfig {
                base_url: base_url.into(),
                model: "gpt-test".into(),
                api_key_env: "OPENAI_API_KEY".into(),
                credential: "default".into(),
                transport: "responses".into(),
                auth_required: Some(true),
            },
            auth: ConnectionAuth::OAuth {
                profile: profile.into(),
            },
            reasoning_effort: None,
        }
    }

    #[test]
    fn plan_zero_matches_creates_id() {
        let config = Config::default();
        // default config may already have openai api connection — clear oauth matches
        let mut config = config;
        config.connections.clear();
        let plan = plan_oauth_ensure(&config, OAuthProvider::Openai, "work").unwrap();
        match plan {
            OAuthEnsurePlan::Create { id } => {
                assert_eq!(id, "openai-work");
            }
            OAuthEnsurePlan::Existing { ids } => panic!("expected create, got {ids:?}"),
        }
    }

    #[test]
    fn plan_one_match_does_not_create() {
        let mut config = Config::default();
        config.connections.clear();
        config.connections.insert(
            "openai-work".into(),
            oauth_conn("OpenAI", "work", "https://api.openai.com"),
        );
        let plan = plan_oauth_ensure(&config, OAuthProvider::Openai, "work").unwrap();
        assert_eq!(
            plan,
            OAuthEnsurePlan::Existing {
                ids: vec!["openai-work".into()]
            }
        );
    }

    #[test]
    fn plan_many_matches_lists_all_sorted_no_create() {
        let mut config = Config::default();
        config.connections.clear();
        config.connections.insert(
            "z-work".into(),
            oauth_conn("legacy-z", "work", "https://api.openai.com"),
        );
        config.connections.insert(
            "a-work".into(),
            oauth_conn("legacy-a", "work", "https://api.openai.com"),
        );
        let plan = plan_oauth_ensure(&config, OAuthProvider::Openai, "work").unwrap();
        assert_eq!(
            plan,
            OAuthEnsurePlan::Existing {
                ids: vec!["a-work".into(), "z-work".into()]
            }
        );
    }

    #[test]
    fn apply_canonical_label_overwrites_stale() {
        let mut conn = oauth_conn("OpenAI", "work", "https://api.openai.com");
        assert!(apply_canonical_oauth_label(
            &mut conn,
            OAuthProvider::Openai,
            "work"
        ));
        assert_eq!(
            conn.label,
            crate::config::oauth_connection_label(OAuthProvider::Openai, "work")
        );
        // second apply is a no-op
        assert!(!apply_canonical_oauth_label(
            &mut conn,
            OAuthProvider::Openai,
            "work"
        ));
    }

    #[test]
    fn matching_ignores_different_profile_or_provider() {
        let mut config = Config::default();
        config.connections.clear();
        config.connections.insert(
            "openai-work".into(),
            oauth_conn("x", "work", "https://api.openai.com"),
        );
        config.connections.insert(
            "openai-home".into(),
            oauth_conn("y", "home", "https://api.openai.com"),
        );
        config.connections.insert(
            "xai-work".into(),
            oauth_conn("z", "work", "https://api.x.ai"),
        );
        let ids = matching_oauth_connection_ids(&config, OAuthProvider::Openai, "work");
        assert_eq!(ids, vec!["openai-work".to_string()]);
    }
}
