//! CLI workflow for AWS-style named shared credentials.

use std::io::{IsTerminal, Read};

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::config::{Config, ProviderConfig};
use crate::credentials::{self, Source, Store};

#[derive(Subcommand, Debug)]
pub enum AuthCommand {
    /// Sign in with a provider subscription through Aishe's private runtime.
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
    /// Remove one provider OAuth credential from Aishe's private runtime.
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
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "connection_id": id,
                "connection_label": connection.label,
                "provider": connection.provider,
                "auth": status,
            }))?
        );
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
            let (provider, profile) =
                oauth_target(*provider, connection.as_deref(), profile.as_deref())?;
            if let Some(profile) = profile {
                crate::oauth::login_profile(provider, &profile, *headless, *browser)
            } else {
                crate::oauth::login(provider, *headless, *browser)
            }
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
                        "Remove Aishe OAuth credential for '{}/{}'?",
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
                    "Aishe OAuth credential for '{}/{}' removed",
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
                    println!("{}", serde_json::to_string_pretty(&status)?);
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
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "profile": profile,
                        "available": source.is_available() || oauth_available,
                        "api_key_available": source.is_available(),
                        "source": source,
                        "oauth": oauth,
                        "selected": selected,
                        "usable": source.is_available() || oauth_available,
                        "credentials_file": credentials::path(),
                    }))?
                );
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
                println!("{}", serde_json::to_string_pretty(&rows)?);
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
