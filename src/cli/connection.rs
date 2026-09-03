use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::config::Config;

/// Parsed connection action transferred from the binary's Clap surface.
#[derive(Clone, Debug)]
pub enum Action {
    List {
        json: bool,
    },
    Show {
        id: Option<String>,
        json: bool,
    },
    Pick {
        value: Option<String>,
        default: bool,
    },
    Add {
        id: String,
        provider: String,
        label: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        transport: String,
        auth: String,
        profile: Option<String>,
        credential: Option<String>,
        key_env: Option<String>,
        reasoning: Option<String>,
    },
    Edit {
        id: String,
        label: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        transport: Option<String>,
        auth: Option<String>,
        profile: Option<String>,
        credential: Option<String>,
        key_env: Option<String>,
        reasoning: Option<String>,
    },
    Remove {
        id: String,
        yes: bool,
    },
    Use {
        id: String,
        model: Option<String>,
        default: bool,
    },
}

/// Shared policy for connection/model picks: only an explicit yes promotes a
/// shell-local selection to the durable default.
fn confirm_promote_to_default(prompt: &str) -> bool {
    matches!(
        crate::promptui::confirm(prompt, crate::promptui::PROMOTE_DEFAULT_CONFIRM_DEFAULT),
        Ok(Some(true))
    )
}

pub fn apply_flag(
    config: &mut Config,
    connection: Option<&str>,
    model: Option<&str>,
) -> Result<()> {
    if let Some(value) = connection {
        let id = config.resolve_connection_id(value)?;
        config.select_connection(&id)?;
        if let Some(model) = model {
            crate::connection::validate_model_id(model)?;
            config.set_active_model(model.to_string());
        }
    }
    Ok(())
}

pub fn reasoning(effective: &Config, value: Option<&str>, save_default: bool) -> u8 {
    let Some(value) = value else {
        println!(
            "reasoning: {} ({})",
            effective.active_reasoning_effort(),
            if crate::connection::selection_is_shell_local() {
                "this shell"
            } else {
                "default for new shells"
            }
        );
        return 0;
    };
    if let Err(error) = crate::connection::validate_reasoning(value) {
        eprintln!("aishe: {error}");
        return 1;
    }
    let mut selected = effective.clone();
    selected.set_active_reasoning_effort(value.to_string());
    let shell_local = crate::connection::selection_path().is_some() && !save_default;
    let result = if shell_local {
        crate::connection::write_shell_selection(&selected, "shell").map(|_| ())
    } else {
        (|| -> Result<()> {
            let mut durable = Config::load_or_init()?;
            durable.select_connection(selected.active_connection_id())?;
            durable.set_active_model(selected.active_model().to_string());
            durable.set_active_reasoning_effort(value.to_string());
            durable.save()?;
            let _ = crate::connection::write_shell_selection(&selected, "default");
            Ok(())
        })()
    };
    if let Err(error) = result {
        eprintln!("aishe: {error}");
        return 1;
    }
    println!(
        "reasoning = {} ({})",
        value,
        if shell_local {
            "this shell"
        } else {
            "saved default"
        }
    );
    0
}

pub fn model(
    effective: &Config,
    value: Option<&str>,
    requested_connection: Option<&str>,
    save_default: bool,
) -> u8 {
    let mut selected = effective.clone();
    let mut save_default = save_default;
    if value == Some("default") && requested_connection.is_none() {
        let durable = match Config::load_or_init() {
            Ok(config) => config,
            Err(error) => {
                eprintln!("aishe: {error}");
                return 1;
            }
        };
        if let Err(error) = crate::connection::write_shell_selection(&durable, "default") {
            eprintln!("aishe: {error}");
            return 1;
        }
        println!(
            "connection = {} · model = {} (restored default for new shells)",
            crate::commands::display_safe(durable.active_connection_id()),
            crate::commands::display_safe(durable.active_model())
        );
        return 0;
    }
    if value.is_none() && requested_connection.is_none() {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            println!(
                "connection: {} · model: {} · reasoning: {} ({})",
                crate::commands::display_safe(selected.active_connection_id()),
                crate::commands::display_safe(selected.active_model()),
                crate::commands::display_safe(selected.active_reasoning_effort()),
                if crate::connection::selection_is_shell_local() {
                    "this shell"
                } else {
                    "default"
                }
            );
            println!(
                "use `aishe model MODEL` for this connection, `aishe connection pick` for accounts"
            );
            return 0;
        }
        let connection_id = selected.active_connection_id().to_string();
        let models = crate::capabilities::known_models(&selected, &connection_id)
            .unwrap_or_else(|_| vec![selected.active_model().to_string()]);
        let connection = selected
            .connections
            .get(&connection_id)
            .expect("active connection exists");
        let rows: Vec<String> = models
            .iter()
            .map(|model| format!("{:<28} {}", model, connection.auth_label()))
            .collect();
        let default = models
            .iter()
            .position(|model| model == selected.active_model())
            .unwrap_or(0);
        let title = format!(
            "Select a model · {} ({})",
            crate::commands::display_safe(&connection.label),
            crate::commands::display_safe(&connection_id)
        );
        let result = match crate::promptui::filter_picker(&title, &rows, default) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("aishe: {error:#}");
                return 1;
            }
        };
        let index = match result {
            crate::promptui::PickerResult::Use(index) => index,
            crate::promptui::PickerResult::SaveDefault(index) => {
                save_default = true;
                index
            }
            crate::promptui::PickerResult::Cancel => {
                println!("model selection cancelled");
                return 0;
            }
        };
        selected.set_active_model(models[index].clone());
        if !save_default {
            if let Ok(Some(durable)) = Config::load_quiet() {
                let durable_model = durable
                    .connections
                    .get(selected.active_connection_id())
                    .map(|c| c.settings.model.as_str())
                    .unwrap_or(durable.active_model());
                if crate::promptui::should_offer_promote_to_default(
                    save_default,
                    durable_model != selected.active_model(),
                ) {
                    // Default to No so Enter stays shell-local; only an explicit
                    // y promotes the choice to the default for new shells.
                    if confirm_promote_to_default(&format!(
                        "Make model '{}' the default for new shells on this connection?",
                        crate::commands::display_safe(selected.active_model())
                    )) {
                        save_default = true;
                    }
                }
            }
        }
    } else {
        if let Some(connection) = requested_connection {
            let id = match selected.resolve_connection_id(connection) {
                Ok(id) => id,
                Err(error) => return crate::cli::error_contract::emit_from(error.as_ref()),
            };
            if let Err(error) = selected.select_connection(&id) {
                return crate::cli::error_contract::emit_from(error.as_ref());
            }
        }
        if let Some(value) = value {
            // Scripting form connection/model still accepted; interactive account
            // switching is `/connection` (or `aishe connection pick`).
            if requested_connection.is_none() {
                if let Some((candidate, model)) = value.split_once('/') {
                    let connection_prefix = selected.connections.contains_key(candidate)
                        || selected.connections.values().any(|connection| {
                            connection.label.eq_ignore_ascii_case(candidate)
                                || connection.provider.eq_ignore_ascii_case(candidate)
                        });
                    if connection_prefix {
                        let id = match selected.resolve_connection_id(candidate) {
                            Ok(id) => id,
                            Err(error) => {
                                eprintln!("aishe: {error}");
                                return 1;
                            }
                        };
                        if let Err(error) = selected.select_connection(&id) {
                            eprintln!("aishe: {error}");
                            return 1;
                        }
                        if let Err(error) = crate::connection::validate_model_id(model) {
                            eprintln!("aishe: {error}");
                            return 1;
                        }
                        selected.set_active_model(model.to_string());
                    } else if let Err(error) = crate::connection::validate_model_id(value) {
                        eprintln!("aishe: {error}");
                        return 1;
                    } else {
                        selected.set_active_model(value.to_string());
                    }
                } else if selected.connections.contains_key(value)
                    || selected
                        .connections
                        .values()
                        .any(|connection| connection.label.eq_ignore_ascii_case(value))
                {
                    eprintln!(
                        "aishe: '{value}' looks like a connection; use `aishe connection pick` or `/connection`"
                    );
                    return 1;
                } else if let Err(error) = crate::connection::validate_model_id(value) {
                    eprintln!("aishe: {error}");
                    return 1;
                } else {
                    selected.set_active_model(value.to_string());
                }
            } else if let Err(error) = crate::connection::validate_model_id(value) {
                eprintln!("aishe: {error}");
                return 1;
            } else {
                selected.set_active_model(value.to_string());
            }
        }
    }

    let shell_local = crate::connection::selection_path().is_some() && !save_default;
    let result = if shell_local {
        crate::connection::write_shell_selection(&selected, "shell").map(|_| ())
    } else {
        (|| -> Result<()> {
            let mut durable = Config::load_or_init()?;
            let id = selected.active_connection_id();
            durable.select_connection(id)?;
            durable.set_active_model(selected.active_model().to_string());
            durable.set_active_reasoning_effort(selected.active_reasoning_effort().to_string());
            durable.save()?;
            let _ = crate::connection::write_shell_selection(&selected, "default");
            Ok(())
        })()
    };
    if let Err(error) = result {
        eprintln!("aishe: {error}");
        return 1;
    }
    println!(
        "connection = {} · model = {} ({})",
        crate::commands::display_safe(selected.active_connection_id()),
        crate::commands::display_safe(selected.active_model()),
        if shell_local {
            "this shell"
        } else {
            "default for new shells"
        }
    );
    0
}

fn pick(effective: &Config, value: Option<&str>, save_default: bool) -> u8 {
    let mut selected = effective.clone();
    let mut save_default = save_default;
    if let Some(value) = value {
        if value == "default" {
            return model(effective, Some("default"), None, false);
        }
        let id = match selected.resolve_connection_id(value) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("aishe: {error}");
                return 1;
            }
        };
        if let Err(error) = selected.select_connection(&id) {
            eprintln!("aishe: {error}");
            return 1;
        }
    } else {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            println!(
                "connection: {} · model: {} ({})",
                crate::commands::display_safe(selected.active_connection_id()),
                crate::commands::display_safe(selected.active_model()),
                if crate::connection::selection_is_shell_local() {
                    "this shell"
                } else {
                    "default"
                }
            );
            println!("use `aishe connection pick ID` or run in a terminal for the picker");
            println!("add a new account: aishe setup  |  aishe connection add --help");
            return 0;
        }
        let choices: Vec<(String, String)> = selected
            .connections
            .iter()
            .map(|(id, connection)| (id.clone(), connection.label.clone()))
            .collect();
        if choices.is_empty() {
            println!("No connections configured yet.");
            println!("  aishe setup");
            println!("  aishe connection add ID --provider openai --auth oauth --profile work");
            println!("  /help accounts");
            return 0;
        }
        println!("  Add a new account: aishe setup  ·  /help accounts\n");
        let rows: Vec<String> = choices
            .iter()
            .map(|(id, _)| {
                let connection = &selected.connections[id];
                format!(
                    "{:<28} {:<22} {}",
                    connection.label,
                    connection.auth_label(),
                    connection.settings.model
                )
            })
            .collect();
        let default = choices
            .iter()
            .position(|(id, _)| id == selected.active_connection_id())
            .unwrap_or(0);
        let result = match crate::promptui::filter_picker("Select a connection", &rows, default) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("aishe: {error:#}");
                return 1;
            }
        };
        let index = match result {
            crate::promptui::PickerResult::Use(index) => index,
            crate::promptui::PickerResult::SaveDefault(index) => {
                save_default = true;
                index
            }
            crate::promptui::PickerResult::Cancel => {
                println!("connection selection cancelled");
                return 0;
            }
        };
        if let Err(error) = selected.select_connection(&choices[index].0) {
            eprintln!("aishe: {error}");
            return 1;
        }
        // After Enter (shell-only), offer to promote to durable default when
        // the choice differs from the saved default connection.
        if !save_default {
            if let Ok(Some(durable)) = Config::load_quiet() {
                if crate::promptui::should_offer_promote_to_default(
                    save_default,
                    durable.active_connection_id() != selected.active_connection_id(),
                ) {
                    // Default to No: Enter is shell-local; an explicit y makes
                    // the connection the default for new shells.
                    if confirm_promote_to_default(&format!(
                        "Make '{}' the default connection for new shells?",
                        crate::commands::display_safe(
                            selected
                                .active_connection()
                                .map(|c| c.label.as_str())
                                .unwrap_or(selected.active_connection_id())
                        )
                    )) {
                        save_default = true;
                    }
                }
            }
        }
    }

    let shell_local = crate::connection::selection_path().is_some() && !save_default;
    let result = if shell_local {
        crate::connection::write_shell_selection(&selected, "shell").map(|_| ())
    } else {
        (|| -> Result<()> {
            let mut durable = Config::load_or_init()?;
            durable.select_connection(selected.active_connection_id())?;
            durable.set_active_model(selected.active_model().to_string());
            durable.set_active_reasoning_effort(selected.active_reasoning_effort().to_string());
            durable.save()?;
            let _ = crate::connection::write_shell_selection(&selected, "default");
            Ok(())
        })()
    };
    if let Err(error) = result {
        eprintln!("aishe: {error}");
        return 1;
    }
    println!(
        "connection = {} · model = {} ({})",
        crate::commands::display_safe(selected.active_connection_id()),
        crate::commands::display_safe(selected.active_model()),
        if shell_local {
            "this shell"
        } else {
            "default for new shells"
        }
    );
    0
}

pub fn command(effective: &Config, command: &Action) -> Result<u8> {
    match command {
        Action::Pick { value, default } => Ok(pick(effective, value.as_deref(), *default)),
        Action::List { json } => {
            if *json {
                let rows: Vec<_> = effective
                    .connections
                    .iter()
                    .map(|(id, connection)| {
                        serde_json::json!({
                            "id": id,
                            "label": connection.label,
                            "provider": connection.provider,
                            "model": connection.settings.model,
                            "auth": connection.auth_label(),
                            "active": id == effective.active_connection_id(),
                        })
                    })
                    .collect();
                crate::cli::json_contract::print_envelope("connections", &rows)?;
            } else {
                println!("connections:");
                for (id, connection) in &effective.connections {
                    println!(
                        "  {} {:<20} {:<12} {:<24} {}",
                        if id == effective.active_connection_id() {
                            ">"
                        } else {
                            " "
                        },
                        crate::commands::display_safe(id),
                        crate::commands::display_safe(&connection.provider),
                        crate::commands::display_safe(&connection.settings.model),
                        crate::commands::display_safe(&connection.auth_label()),
                    );
                }
            }
            Ok(0)
        }
        Action::Show { id, json } => {
            let id = id
                .as_deref()
                .map(|id| effective.resolve_connection_id(id))
                .transpose()?
                .unwrap_or_else(|| effective.active_connection_id().to_string());
            let connection = &effective.connections[&id];
            let auth_state = auth_state(connection);
            if *json {
                crate::cli::json_contract::print_object(&serde_json::json!({
                    "id": id,
                    "label": connection.label,
                    "provider": connection.provider,
                    "base_url": connection.settings.base_url,
                    "model": connection.settings.model,
                    "transport": connection.settings.transport,
                    "reasoning_effort": connection.reasoning_effort,
                    "auth": connection.auth,
                    "auth_state": auth_state,
                    "active": id == effective.active_connection_id(),
                }))?;
            } else {
                println!("connection: {id} ({})", connection.label);
                println!(
                    "  provider: {} · model: {} · reasoning: {}",
                    connection.provider,
                    connection.settings.model,
                    connection
                        .reasoning_effort
                        .as_deref()
                        .unwrap_or(&effective.aishe.reasoning_effort)
                );
                println!(
                    "  endpoint: {} · transport: {}",
                    connection.settings.base_url, connection.settings.transport
                );
                println!("  auth: {} · state: {auth_state}", connection.auth_label());
            }
            Ok(0)
        }
        Action::Add {
            id,
            provider,
            label,
            base_url,
            model,
            transport,
            auth,
            profile,
            credential,
            key_env,
            reasoning,
        } => {
            let id = crate::config::normalize_connection_id(id)?;
            let mut config = Config::load_or_init()?;
            if config.connections.contains_key(&id) {
                anyhow::bail!("connection '{id}' already exists");
            }
            let mut settings = if provider.eq_ignore_ascii_case("anthropic") {
                config.providers.anthropic.clone()
            } else {
                config.providers.openai.clone()
            };
            if let Some(service) = crate::provider_catalog::find(provider) {
                crate::provider_catalog::apply(service, &mut settings);
            }
            if let Some(value) = base_url {
                settings.base_url = crate::provider_catalog::normalize_base_url(value);
            }
            if let Some(value) = model {
                crate::connection::validate_model_id(value)?;
                settings.model.clone_from(value);
            }
            settings.transport.clone_from(transport);
            let connection_auth = build_connection_auth(
                auth,
                profile.as_deref(),
                credential.as_deref(),
                key_env.as_deref(),
                &settings,
            )?;
            let default_label = crate::config::branded_connection_label(
                &provider.to_ascii_lowercase(),
                &settings.base_url,
                &connection_auth,
            )
            .unwrap_or_else(|| id.clone());
            config.connections.insert(
                id.clone(),
                crate::config::ConnectionConfig {
                    provider: provider.to_ascii_lowercase(),
                    label: label.clone().unwrap_or(default_label),
                    settings,
                    auth: connection_auth,
                    reasoning_effort: reasoning.clone(),
                },
            );
            config.validate_connections()?;
            config.save()?;
            println!("connection '{id}' added");
            Ok(0)
        }
        Action::Edit {
            id,
            label,
            base_url,
            model,
            transport,
            auth,
            profile,
            credential,
            key_env,
            reasoning,
        } => {
            let mut config = Config::load_or_init()?;
            let id = config.resolve_connection_id(id)?;
            let old_connection = config.connections[&id].clone();
            let connection = config
                .connections
                .get_mut(&id)
                .context("connection disappeared")?;
            if let Some(value) = label {
                if value.trim().is_empty() {
                    anyhow::bail!("connection label cannot be empty");
                }
                connection.label.clone_from(value);
            }
            if let Some(value) = base_url {
                connection.settings.base_url = crate::provider_catalog::normalize_base_url(value);
            }
            if let Some(value) = model {
                crate::connection::validate_model_id(value)?;
                connection.settings.model.clone_from(value);
            }
            if let Some(value) = transport {
                connection.settings.transport.clone_from(value);
            }
            if let Some(value) = reasoning {
                crate::connection::validate_reasoning(value)?;
                connection.reasoning_effort = Some(value.clone());
            }
            if let Some(kind) = auth {
                connection.auth = build_connection_auth(
                    kind,
                    profile.as_deref(),
                    credential.as_deref(),
                    key_env.as_deref(),
                    &connection.settings,
                )?;
            } else {
                match &mut connection.auth {
                    crate::config::ConnectionAuth::OAuth { profile: current } => {
                        if let Some(value) = profile {
                            current.clone_from(value);
                        }
                    }
                    crate::config::ConnectionAuth::ApiKey {
                        credential: current,
                        api_key_env: current_env,
                    } => {
                        if let Some(value) = credential {
                            *current = Some(value.clone());
                        }
                        if let Some(value) = key_env {
                            *current_env = Some(value.clone());
                        }
                    }
                    _ => {}
                }
            }
            if label.is_none() {
                if let Some(branded) = crate::config::branded_connection_label(
                    &connection.provider,
                    &connection.settings.base_url,
                    &connection.auth,
                ) {
                    connection.label = branded;
                }
            }
            config.validate_connections()?;
            config.save()?;
            let _ = crate::connection::invalidate_connection(&id, &old_connection);
            println!("connection '{id}' updated");
            Ok(0)
        }
        Action::Remove { id, yes } => {
            let mut config = Config::load_or_init()?;
            let id = config.resolve_connection_id(id)?;
            if id == config.active_connection_id() {
                anyhow::bail!("cannot remove the active connection; select another one first");
            }
            if config.connections.len() == 1 {
                anyhow::bail!("cannot remove the only connection");
            }
            if !*yes {
                if !std::io::stdin().is_terminal() {
                    anyhow::bail!("refusing non-interactive removal without --yes");
                }
                if crate::promptui::confirm(&format!("Remove connection '{id}'?"), false)?
                    != Some(true)
                {
                    println!("connection removal cancelled");
                    return Ok(0);
                }
            }
            let removed_connection = config
                .connections
                .get(&id)
                .context("connection disappeared")?
                .clone();
            config.connections.remove(&id);
            if config.aishe.connection_fallback == id {
                config.aishe.connection_fallback = config.active_connection_id().to_string();
            }
            config.save()?;
            let _ = crate::connection::invalidate_connection(&id, &removed_connection);
            println!("connection '{id}' removed; credentials were preserved");
            Ok(0)
        }
        Action::Use { id, model, default } => {
            Ok(self::model(effective, model.as_deref(), Some(id), *default))
        }
    }
}

fn build_connection_auth(
    kind: &str,
    profile: Option<&str>,
    credential: Option<&str>,
    key_env: Option<&str>,
    settings: &crate::config::ProviderConfig,
) -> Result<crate::config::ConnectionAuth> {
    Ok(match kind {
        "api-key" => crate::config::ConnectionAuth::ApiKey {
            credential: Some(crate::credentials::normalize_profile(
                credential.unwrap_or(settings.credential.as_str()),
            )?),
            api_key_env: Some(key_env.unwrap_or(&settings.api_key_env).to_string()),
        },
        "oauth" => crate::config::ConnectionAuth::OAuth {
            profile: profile
                .context("--profile is required for OAuth connections")?
                .to_string(),
        },
        "none" => crate::config::ConnectionAuth::None,
        "auto" => crate::config::ConnectionAuth::Auto,
        _ => anyhow::bail!("auth must be api-key, oauth, none, or auto"),
    })
}

pub fn auth_state(connection: &crate::config::ConnectionConfig) -> String {
    crate::connection::auth_status(connection).detail
}

/// Back the small show/set subcommands. With no `value`, print the effective
/// current value. With a value, persist it to the *user* config file:
/// we reload a fresh `Config` (no project overlay, no this-invocation flags) so a
/// project overlay or a `--mode`/`--provider` flag can't get baked into the saved
/// file. Clap already validated the enumerated settings against their allowed
/// sets.
/// `aishe mode` inside an AIShe shell means *this* shell, like `/mode`; outside
/// one it is the durable default. `--default` writes config either way.
pub fn mode(effective: &Config, value: Option<&str>, save_default: bool) -> u8 {
    let in_shell = std::env::var_os("AISHE_SHELL_ID").is_some_and(|id| !id.is_empty());
    let Some(value) = value else {
        let current = std::env::var("AISHE_MODE")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| effective.aishe.mode.clone());
        let scope = if in_shell {
            "this shell"
        } else {
            "default for new shells"
        };
        println!(
            "mode: {} ({scope})",
            crate::commands::display_safe(&current)
        );
        return 0;
    };
    let (hand_off, save) = mode_targets(in_shell, save_default);
    if hand_off {
        if let Some(path) = std::env::var_os("AISHE_PENDING_FILE").filter(|p| !p.is_empty()) {
            if let Err(error) = std::fs::write(&path, format!("mode\n{value}\n")) {
                eprintln!("aishe: {error}");
                return 1;
            }
        }
        println!("mode: {value} (this shell)");
    }
    if save {
        let mut cfg = match Config::load_or_init() {
            Ok(cfg) => cfg,
            Err(error) => return crate::cli::error_contract::emit_from(error.as_ref()),
        };
        cfg.aishe.mode = value.to_string();
        cfg.aishe.safety_profile = "custom".to_string();
        if let Err(error) = cfg.save() {
            eprintln!("aishe: {error}");
            return 1;
        }
        println!("mode: {value} (default for new shells)");
    }
    0
}

/// (hand off to the parent shell, write the config file)
fn mode_targets(in_shell: bool, save_default: bool) -> (bool, bool) {
    (in_shell, save_default || !in_shell)
}

pub fn set_or_show(field: &str, value: Option<&str>, effective: &Config) -> u8 {
    let Some(value) = value else {
        let current = match field {
            "mode" => effective.aishe.mode.clone(),
            "provider" => effective.active_connection_id().to_string(),
            "scope" => effective.backend.default_scope.clone(),
            "network" => effective.backend.workspace_network.clone(),
            "output" => effective.backend.output.clone(),
            "reasoning" => effective.active_reasoning_effort().to_string(),
            _ => effective.active_model().to_string(),
        };
        println!("{field}: {current}");
        return 0;
    };
    let mut cfg = match Config::load_or_init() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("aishe: {e}");
            return 1;
        }
    };
    match field {
        "mode" => {
            cfg.aishe.mode = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        "provider" => {
            let id = match cfg.resolve_connection_id(value) {
                Ok(id) => id,
                Err(error) => return crate::cli::error_contract::emit_from(error.as_ref()),
            };
            if let Err(error) = cfg.select_connection(&id) {
                return crate::cli::error_contract::emit_from(error.as_ref());
            }
        }
        "scope" => {
            cfg.backend.default_scope = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        "network" => {
            cfg.backend.workspace_network = value.to_string();
            cfg.aishe.safety_profile = "custom".to_string();
        }
        "output" => cfg.backend.output = value.to_string(),
        "reasoning" => cfg.set_active_reasoning_effort(value.to_string()),
        _ => cfg.set_active_model(value.to_string()),
    }
    if let Err(e) = cfg.save() {
        eprintln!("aishe: {e}");
        return 1;
    }
    if field == "model" {
        // Under the PTY front-end, let the parent zsh prompt pick up the saved
        // model on its very next precmd. This is best-effort: config persistence
        // is the source of truth, and non-PTY invocations have no state file.
        if let Some(path) = std::env::var_os("AISHE_MODEL_FILE").filter(|p| !p.is_empty()) {
            let _ = std::fs::write(path, crate::commands::display_safe(cfg.active_model()));
        }
    }
    if field == "scope" {
        if let Some(path) = std::env::var_os("AISHE_SCOPE_FILE").filter(|p| !p.is_empty()) {
            let _ = std::fs::write(
                path,
                crate::commands::display_safe(&cfg.backend.default_scope),
            );
        }
    }
    if field == "output" {
        // The PTY wrapper exports a session override, so hand the new
        // persistent setting back to its parent shell for the next prompt.
        if let Ok(Some(path)) = output_handoff_path() {
            let _ = std::fs::write(path, crate::commands::display_safe(&cfg.backend.output));
        }
    }
    println!(
        "{} = {}  (saved to {})",
        crate::commands::display_safe(field),
        crate::commands::display_safe(value),
        crate::commands::display_safe(&Config::path().display().to_string())
    );
    0
}

fn output_handoff_path() -> Result<Option<std::path::PathBuf>> {
    let Some(path) = std::env::var_os("AISHE_OUTPUT_FILE")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
    else {
        return Ok(None);
    };
    let shell_id =
        std::env::var("AISHE_SHELL_ID").context("AISHE_OUTPUT_FILE requires AISHE_SHELL_ID")?;
    if !(16..=128).contains(&shell_id.len())
        || !shell_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("AISHE_SHELL_ID is invalid");
    }
    let expected = format!("aishe-output-{shell_id}");
    if path.file_name().and_then(|value| value.to_str()) != Some(expected.as_str()) {
        anyhow::bail!("AISHE_OUTPUT_FILE does not match this shell identity");
    }
    let expected_parent = std::env::temp_dir()
        .canonicalize()
        .context("resolving the temporary directory")?;
    let parent = path
        .parent()
        .context("AISHE_OUTPUT_FILE has no parent")?
        .canonicalize()
        .context("resolving AISHE_OUTPUT_FILE parent")?;
    if parent != expected_parent {
        anyhow::bail!("AISHE_OUTPUT_FILE must be in the shell temporary directory");
    }
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_targets_follow_the_shell_it_runs_in() {
        // Inside an AIShe shell `aishe mode auto` means this shell, like /mode.
        assert_eq!(mode_targets(true, false), (true, false));
        // --default also writes config without losing the live shell change.
        assert_eq!(mode_targets(true, true), (true, true));
        // Outside a shell there is nothing to hand off to; save the default.
        assert_eq!(mode_targets(false, false), (false, true));
    }
}
