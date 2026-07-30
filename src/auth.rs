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
        provider: crate::oauth::OAuthProvider,
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
        provider: crate::oauth::OAuthProvider,
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
        /// Credential profile (defaults to the active provider's profile).
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
    let active = match config.aishe.provider.as_str() {
        "openai" => &config.providers.openai,
        _ => &config.providers.anthropic,
    };
    if active.credential_profile() == profile {
        return Some(active.clone());
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
    let provider = match config.aishe.provider.as_str() {
        "openai" => config.providers.openai.clone(),
        _ => config.providers.anthropic.clone(),
    };
    Ok((provider.credential_profile(), Some(provider)))
}

pub fn run(command: &AuthCommand) -> Result<u8> {
    match command {
        AuthCommand::Login {
            provider,
            headless,
            browser,
        } => crate::oauth::login(*provider, *headless, *browser),
        AuthCommand::Logout { provider, yes } => {
            if !*yes {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("refusing non-interactive OAuth logout without --yes");
                }
                let confirmed = crate::promptui::confirm(
                    &format!("Remove Aishe OAuth credential for '{provider}'?"),
                    false,
                )?;
                if confirmed != Some(true) {
                    println!("OAuth logout cancelled");
                    return Ok(0);
                }
            }
            if crate::oauth::logout(*provider)? {
                println!("Aishe OAuth credential for '{provider}' removed");
                Ok(0)
            } else {
                eprintln!("aishe: no OAuth credential is saved for '{provider}'");
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
            println!(
                "credential profile '{}' saved to {}",
                profile,
                credentials::path().display()
            );
            Ok(0)
        }
        AuthCommand::Status { profile, json } => {
            let (profile, provider) = target(profile.as_deref())?;
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
            for status in &oauth_statuses {
                if status.credential_type.is_some() {
                    profiles.insert(status.provider.id().to_string());
                }
            }
            let config = Config::load_quiet().ok().flatten();
            if let Some(config) = &config {
                profiles.insert(config.providers.anthropic.credential_profile());
                profiles.insert(config.providers.openai.credential_profile());
            }
            let rows: Vec<_> = profiles
                .into_iter()
                .map(|profile| {
                    let saved = store.contains(&profile).unwrap_or(false);
                    let oauth = oauth_statuses
                        .iter()
                        .find(|status| status.provider.id() == profile)
                        .is_some_and(|status| status.available);
                    let active = config.as_ref().is_some_and(|config| {
                        let provider = if config.aishe.provider == "openai" {
                            &config.providers.openai
                        } else {
                            &config.providers.anthropic
                        };
                        provider.credential_profile() == profile
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
            println!("credential profile '{profile}' removed");
            Ok(0)
        }
    }
}
