//! Interactive settings hub and effective-configuration provenance.
//! All edits happen against a draft and are written only after final review.

use std::collections::{BTreeMap, BTreeSet};
use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};

use crate::capabilities;
use crate::config::Config;
use crate::profiles::{self, Profile};
use crate::promptui::{self, MenuResult};
use crate::provider_catalog::{self, Family};
use crate::usage::{self, Price};

#[derive(Clone, Debug, Serialize)]
pub struct Field {
    pub path: String,
    pub value: Value,
    pub source: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Provenance {
    pub config_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_path: Option<PathBuf>,
    pub fields: Vec<Field>,
}

pub fn provenance() -> Result<(Config, Provenance)> {
    let user_exists = Config::path().exists();
    let mut config = Config::load_quiet()?.unwrap_or_default();
    let base_source = if user_exists {
        format!("user:{}", Config::path().display())
    } else {
        "compiled_default".into()
    };
    let mut project_path = None;
    let mut project_fields = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(outcome) = config.apply_project_overlay(&cwd) {
            project_path = Some(outcome.path.clone());
            if outcome.error.is_none() {
                project_fields = outcome.applied;
            }
        }
    }
    let source = |path: &str| {
        let short = path.strip_prefix("aishe.").unwrap_or(path).to_string();
        if project_fields
            .iter()
            .any(|field| field == path || field == &short)
        {
            format!(
                "project:{}",
                project_path
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_default()
            )
        } else {
            base_source.clone()
        }
    };
    let connection_id = config.active_connection_id().to_string();
    let connection = config.active_connection();
    let provider = active_provider(&config);
    let connection_prefix = format!("connections.{connection_id}");
    let mut fields = vec![
        field("aishe.connection", json!(connection_id), &source),
        field("aishe.provider", json!(config.aishe.provider), &source),
        field(
            &format!("{connection_prefix}.label"),
            json!(connection.map(|value| value.label.as_str()).unwrap_or("")),
            &source,
        ),
        field(
            &format!("{connection_prefix}.provider"),
            json!(config.active_provider_name()),
            &source,
        ),
        field(
            &format!("{connection_prefix}.base_url"),
            json!(provider.base_url),
            &source,
        ),
        field(
            &format!("{connection_prefix}.model"),
            json!(provider.model),
            &source,
        ),
        field(
            &format!("{connection_prefix}.transport"),
            json!(provider.transport),
            &source,
        ),
        field(
            &format!("{connection_prefix}.reasoning_effort"),
            json!(config.active_reasoning_effort()),
            &source,
        ),
        field(
            "aishe.safety_profile",
            json!(config.aishe.safety_profile),
            &source,
        ),
        field("aishe.mode", json!(config.aishe.mode), &source),
        field(
            "aishe.share_history",
            json!(config.aishe.share_history),
            &source,
        ),
        field("aishe.pty_prompt", json!(config.aishe.pty_prompt), &source),
        field(
            "aishe.hook_timeout_secs",
            json!(config.aishe.hook_timeout_secs),
            &source,
        ),
        field(
            "aishe.status_line_position",
            json!(config.aishe.status_line_position),
            &source,
        ),
        field(
            "aishe.status_line_items",
            json!(config.aishe.status_line_items),
            &source,
        ),
        field("backend.output", json!(config.backend.output), &source),
        field("ui.theme", json!(config.ui.theme), &source),
        field("ui.color_depth", json!(config.ui.color_depth), &source),
        field("ui.unicode", json!(config.ui.unicode), &source),
        field("ui.motion", json!(config.ui.motion), &source),
        field(
            "aishe.failure_hints",
            json!(config.aishe.failure_hints),
            &source,
        ),
        field(
            "aishe.discovery_hints",
            json!(config.aishe.discovery_hints),
            &source,
        ),
        field(
            "aishe.context_exclude",
            json!(config.aishe.context_exclude),
            &source,
        ),
        field(
            "aishe.redact_secrets",
            json!(config.aishe.redact_secrets),
            &source,
        ),
        field("aishe.budget_usd", json!(config.aishe.budget_usd), &source),
        field("logging.enabled", json!(config.logging.enabled), &source),
        field("logging.redact", json!(config.logging.redact), &source),
        field(
            "aishe.reasoning_effort",
            json!(config.aishe.reasoning_effort),
            &source,
        ),
        field("aishe.structured", json!(config.aishe.structured), &source),
    ];
    if let Some(connection) = connection {
        let (auth_type, auth_profile, auth_env) = match &connection.auth {
            crate::config::ConnectionAuth::ApiKey {
                credential,
                api_key_env,
            } => (
                "api_key",
                credential
                    .as_deref()
                    .unwrap_or(&connection.settings.credential),
                api_key_env
                    .as_deref()
                    .unwrap_or(&connection.settings.api_key_env),
            ),
            crate::config::ConnectionAuth::OAuth { profile } => ("oauth", profile.as_str(), ""),
            crate::config::ConnectionAuth::None => ("none", "", ""),
            crate::config::ConnectionAuth::Auto => ("auto", "", ""),
        };
        fields.push(field(
            &format!("{connection_prefix}.auth.type"),
            json!(auth_type),
            &source,
        ));
        if !auth_profile.is_empty() {
            fields.push(field(
                &format!("{connection_prefix}.auth.profile"),
                json!(auth_profile),
                &source,
            ));
        }
        if !auth_env.is_empty() {
            fields.push(field(
                &format!("{connection_prefix}.auth.api_key_env"),
                json!(auth_env),
                &source,
            ));
        }
    }
    Ok((
        config,
        Provenance {
            config_path: Config::path(),
            project_path,
            fields,
        },
    ))
}

fn field(path: &str, value: Value, source: &impl Fn(&str) -> String) -> Field {
    Field {
        path: path.into(),
        value,
        source: source(path),
    }
}

pub fn print_provenance(report: &Provenance) {
    println!("effective configuration");
    println!("config: {}", report.config_path.display());
    if let Some(path) = &report.project_path {
        println!("project: {}", path.display());
    }
    for field in &report.fields {
        println!(
            "  {:32} {:24} <- {}",
            field.path,
            compact_value(&field.value),
            field.source
        );
    }
}

fn compact_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        _ => value.to_string(),
    }
}

pub fn run() -> Result<bool> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "settings needs an interactive terminal; use `aishe settings --json` to inspect"
        );
    }
    let baseline = Config::load_quiet()?.context("no config exists; run `aishe setup` first")?;
    crate::ui::configure(&baseline.ui);
    let managed_policy = crate::policy::load()?;
    let mut draft = baseline.clone();
    loop {
        let changes = draft_changes(&baseline, &draft)?;
        promptui::header(
            "AIShe settings",
            "Defaults for new shells. Edit, review, then save.",
            if changes.is_empty() {
                "No unsaved changes."
            } else {
                "Draft only. Nothing has been saved."
            },
        );
        if let Some(loaded) = &managed_policy {
            promptui::key_value("Managed policy", &loaded.path.display().to_string());
            promptui::note("Organization policy applies at review and in new shells.");
        }
        let options = vec![
            format!(
                "Connection & model: {} / {}",
                connection_label(&draft),
                draft.active_model()
            ),
            format!(
                "Terminal & history: {} / {}",
                draft.ui.theme, draft.backend.output
            ),
            format!(
                "Mode & safety: {} / {}",
                mode_label(&draft),
                draft.aishe.safety_profile
            ),
            format!(
                "Context & privacy: memory {} / redaction {}",
                on_off(draft.aishe.memory),
                on_off(draft.aishe.redact_secrets)
            ),
            format!(
                "Usage & logging: budget {} / logging {}",
                budget_label(&draft),
                on_off(draft.logging.enabled)
            ),
            format!(
                "Response tuning: reasoning {} / streaming {}",
                draft.active_reasoning_effort(),
                on_off(draft.aishe.stream)
            ),
            "Check connection".into(),
            if changes.is_empty() {
                "Review changes".into()
            } else {
                format!("Review and apply ({} changes)", changes.len())
            },
            if changes.is_empty() {
                "Done".into()
            } else {
                "Discard changes and exit".into()
            },
        ];
        match promptui::menu(
            "Choose a section",
            &options,
            0,
            false,
            "Saved settings apply to new shells. Use /model or /connection to change this shell.",
        )? {
            MenuResult::Selected(0) => provider_section(&mut draft)?,
            MenuResult::Selected(1) => shell_section(&mut draft)?,
            MenuResult::Selected(2) => safety_section(&mut draft)?,
            MenuResult::Selected(3) => context_section(&mut draft)?,
            MenuResult::Selected(4) => cost_section(&mut draft)?,
            MenuResult::Selected(5) => advanced_section(&mut draft)?,
            MenuResult::Selected(6) => check_connection(&draft)?,
            MenuResult::Selected(7) => {
                if let Some(loaded) = crate::policy::load()? {
                    match constrained_draft(&draft, &loaded.policy) {
                        Ok((constrained, managed)) => {
                            if !managed.is_empty() {
                                promptui::section("Managed by organization");
                                promptui::note("Policy requires these values. The final review includes the enforced defaults.");
                                for change in &managed {
                                    promptui::key_value(
                                        &review_label(&change.path),
                                        &format!(
                                            "{} -> {}",
                                            review_value(change.before.as_ref()),
                                            review_value(change.after.as_ref())
                                        ),
                                    );
                                }
                            }
                            draft = constrained;
                        }
                        Err(error) => {
                            promptui::warning(&format!(
                                "Managed by organization: {}. Your draft is still available.",
                                crate::redact::redact(&error.to_string())
                            ));
                            continue;
                        }
                    }
                }
                let changes = draft_changes(&baseline, &draft)?;
                if changes.is_empty() {
                    promptui::note("No changes to apply.");
                    continue;
                }
                print_review(&changes);
                if let Err(error) = crate::setup::validate_config(&draft) {
                    promptui::warning(&format!(
                        "Draft needs attention: {}",
                        crate::redact::redact(&error.to_string())
                    ));
                    continue;
                }
                if promptui::confirm("Save these defaults", true)?.unwrap_or(false) {
                    match crate::setup::save_settings_transactional(&draft) {
                        Ok(backup) => {
                            promptui::success("Settings saved for new shells.");
                            promptui::key_value("Saved", &Config::path().display().to_string());
                            if let Some(path) = backup {
                                promptui::key_value("Backup", &path.display().to_string());
                            }
                            promptui::note("Use /model or /connection to change this shell. Other defaults take effect when you start a new shell.");
                            if draft.active_connection().is_some_and(|connection| {
                                matches!(
                                    connection.auth,
                                    crate::config::ConnectionAuth::OAuth { .. }
                                )
                            }) {
                                promptui::warning("OAuth connections require the managed runtime and AISHE_LEGACY_OPENCODE=1 when starting a new shell.");
                            }
                            return Ok(true);
                        }
                        Err(error) => promptui::warning(&format!(
                            "Could not save settings: {}. Your draft is still available.",
                            crate::redact::redact(&error.to_string())
                        )),
                    }
                }
            }
            MenuResult::Selected(8) => {
                promptui::note(if changes.is_empty() {
                    "No settings changed."
                } else {
                    "Unsaved changes discarded. Saved settings are unchanged."
                });
                return Ok(false);
            }
            MenuResult::Cancel => {
                if changes.is_empty()
                    || promptui::confirm("Discard unsaved changes and exit", false)?
                        .unwrap_or(false)
                {
                    promptui::note("Saved settings are unchanged.");
                    return Ok(false);
                }
            }
            MenuResult::Back | MenuResult::Selected(_) => {}
        }
    }
}

fn connection_label(config: &Config) -> &str {
    config
        .active_connection()
        .map(|connection| connection.label.as_str())
        .unwrap_or(config.active_connection_id())
}

fn mode_label(config: &Config) -> String {
    match config.aishe.mode.as_str() {
        "auto" | "allow" => "allow".into(),
        "yolo" | "agent" => format!("agent:{}", config.backend.default_scope),
        _ => "ask".into(),
    }
}

fn budget_label(config: &Config) -> String {
    if config.aishe.budget_usd > 0.0 {
        format!("${:.2}", config.aishe.budget_usd)
    } else {
        "unlimited".into()
    }
}

fn check_connection(config: &Config) -> Result<()> {
    let choices = vec![
        "Endpoint and model list (no generation)".into(),
        "Full capability check (uses tokens)".into(),
        "Back".into(),
    ];
    if let MenuResult::Selected(index @ 0..=1) = promptui::menu(
        "Check connection", &choices, 0, true,
        "Checks the draft connection. Both checks contact its endpoint; only the full check generates responses.",
    )? { print_capabilities(&capabilities::validate(config, index == 1)); }
    Ok(())
}

fn constrained_draft(
    draft: &Config,
    policy: &crate::policy::OrganizationPolicy,
) -> Result<(Config, Vec<DraftChange>)> {
    let mut constrained = draft.clone();
    policy.constrain(&mut constrained)?;
    policy.validate_request(&constrained)?;
    let managed = draft_changes(draft, &constrained)?;
    Ok((constrained, managed))
}

#[derive(Debug)]
struct DraftChange {
    path: String,
    before: Option<Value>,
    after: Option<Value>,
}

fn draft_changes(before: &Config, after: &Config) -> Result<Vec<DraftChange>> {
    fn flatten(path: &str, value: &Value, output: &mut BTreeMap<String, Value>) {
        if let Value::Object(fields) = value {
            for (key, value) in fields {
                flatten(
                    &if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    },
                    value,
                    output,
                );
            }
        } else {
            output.insert(path.into(), value.clone());
        }
    }
    let mut old = BTreeMap::new();
    let mut new = BTreeMap::new();
    flatten("", &serde_json::to_value(before)?, &mut old);
    flatten("", &serde_json::to_value(after)?, &mut new);
    let paths: BTreeSet<&String> = old.keys().chain(new.keys()).collect();
    Ok(paths
        .into_iter()
        .filter(|path| old.get(*path) != new.get(*path))
        // Named connections are authoritative; legacy provider blocks mirror
        // these edits for backwards compatibility and need no duplicate review.
        .filter(|path| after.connections.is_empty() || !path.starts_with("providers."))
        .map(|path| DraftChange {
            path: path.clone(),
            before: old.get(path).cloned(),
            after: new.get(path).cloned(),
        })
        .collect())
}

fn review_group(path: &str) -> String {
    let mut parts = path.split('.');
    match parts.next().unwrap_or("") {
        "connections" => format!("Connection: {}", parts.next().unwrap_or("default")),
        "providers" => format!("Legacy connection: {}", parts.next().unwrap_or("default")),
        "ui" => "Terminal & history".into(),
        "logging" => "Usage & logging".into(),
        "pricing" => format!(
            "Pricing: {}",
            path.strip_prefix("pricing.")
                .and_then(|value| value.rsplit_once('.'))
                .map(|(model, _)| model)
                .unwrap_or("model")
        ),
        "sandbox" => "Mode & safety".into(),
        "backend" => if path.ends_with("output") {
            "Terminal & history"
        } else {
            "Mode & safety"
        }
        .into(),
        "aishe" => match parts.next().unwrap_or("") {
            "connection" | "provider" => "Default connection",
            "mode"
            | "safety_profile"
            | "yolo_confirm"
            | "yolo_plan"
            | "yolo_preview"
            | "yolo_sandbox"
            | "max_yolo_iterations"
            | "sandbox_backend" => "Mode & safety",
            "project_context" | "project_tasks" | "host_profile" | "context_exclude"
            | "redact_secrets" | "memory" => "Context & privacy",
            "budget_usd" | "show_usage" => "Usage & logging",
            "reasoning_effort" | "structured" | "stream" | "cache" => "Response tuning",
            _ => "Terminal & history",
        }
        .into(),
        other => other.replace('_', " "),
    }
}

fn review_label(path: &str) -> String {
    let key = path.rsplit('.').next().unwrap_or(path);
    match key {
        "base_url" => "Endpoint",
        "api_key_env" if path.contains(".auth.") => "Auth key var",
        "api_key_env" => "Key variable",
        "reasoning_effort" => "Reasoning",
        "hook_timeout_secs" => "Timeout (sec)",
        "budget_usd" => "Budget (USD)",
        "status_line_position" => "Status placement",
        "status_line_items" => "Status fields",
        "share_history" => "Shared history",
        "pty_prompt" => "AIShe prompt",
        "max_yolo_iterations" => "Agent steps",
        "type" => "Auth method",
        "profile" if path.contains(".auth.") => "OAuth profile",
        "credential" if path.contains(".auth.") => "Auth profile",
        "credential" => "Key profile",
        "auth_required" => "Auth required",
        "default_scope" => "Agent scope",
        "output" if path.starts_with("backend.") => "Agent output",
        _ => return key.replace('_', " "),
    }
    .into()
}

fn review_value(value: Option<&Value>) -> String {
    let value = match value {
        None | Some(Value::Null) => "default".into(),
        Some(Value::Bool(value)) => on_off(*value).into(),
        Some(Value::Array(items)) if items.is_empty() => "none".into(),
        Some(Value::Array(items)) => items
            .iter()
            .map(compact_value)
            .collect::<Vec<_>>()
            .join(", "),
        Some(value) => compact_value(value),
    };
    crate::commands::display_safe(&crate::redact::redact(&value))
}

fn print_review(changes: &[DraftChange]) {
    promptui::header(
        "Review changes",
        &format!(
            "{} changed settings. Saved values -> draft values.",
            changes.len()
        ),
        "Secret values are redacted. Nothing is saved until you confirm.",
    );
    let mut groups: BTreeMap<String, Vec<&DraftChange>> = BTreeMap::new();
    for change in changes {
        groups
            .entry(review_group(&change.path))
            .or_default()
            .push(change);
    }
    for (group, changes) in groups {
        promptui::section(&group);
        for change in changes {
            promptui::key_value(
                &review_label(&change.path),
                &format!(
                    "{} -> {}",
                    review_value(change.before.as_ref()),
                    review_value(change.after.as_ref())
                ),
            );
        }
    }
}

fn provider_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("Default connection: {}", connection_label(config)),
            format!("Model: {}", config.active_model()),
            format!(
                "Endpoint & authentication: {}",
                active_provider(config).base_url
            ),
            "Back".into(),
        ];
        match promptui::menu(
            "Connection & model",
            &choices,
            1,
            true,
            "Edit the default for new shells. Use /model or /connection for this shell.",
        )? {
            MenuResult::Selected(0) => {
                choose_connection(config)?;
            }
            MenuResult::Selected(1) => {
                if let Some(model) = promptui::text(
                    "Model",
                    config.active_model(),
                    crate::connection::validate_model_id,
                )? {
                    if model != ":back" {
                        set_model(config, model.trim());
                    }
                }
            }
            MenuResult::Selected(2) => {
                let mut candidate = config.clone();
                if provider_transaction(&mut candidate)? {
                    *config = candidate;
                }
            }
            MenuResult::Selected(3) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn provider_transaction(config: &mut Config) -> Result<bool> {
    let before = config.clone();
    let labels: Vec<String> = provider_catalog::SERVICES
        .iter()
        .map(|service| format!("{} - {}", service.label, service.help))
        .chain(std::iter::once("Back".into()))
        .collect();
    let selection = promptui::menu(
        "Provider",
        &labels,
        service_index(config),
        true,
        "The provider, endpoint, credential name, model, and transport are kept as one draft.",
    )?;
    let MenuResult::Selected(index) = selection else {
        return Ok(false);
    };
    if index >= provider_catalog::SERVICES.len() {
        return Ok(false);
    }
    let service = &provider_catalog::SERVICES[index];
    apply_service_to_active_connection(config, service);
    let provider = active_provider_mut(config);
    let Some(endpoint) = promptui::text("Endpoint", &provider.base_url, |value| {
        if value.starts_with("http://") || value.starts_with("https://") {
            Ok(())
        } else {
            anyhow::bail!("enter an http:// or https:// URL")
        }
    })?
    else {
        *config = before;
        return Ok(false);
    };
    if endpoint == ":back" {
        *config = before;
        return Ok(false);
    }
    provider.base_url = provider_catalog::normalize_base_url(&endpoint);
    provider.auth_required = Some(!crate::config::is_loopback_url(&provider.base_url));
    let oauth_provider =
        crate::oauth::OAuthProvider::from_base_url(&draft_provider(config).base_url);
    let mut auth_options = vec!["API key".to_string()];
    if oauth_provider.is_some() {
        auth_options.push("OAuth profile".into());
    }
    if !draft_provider(config).requires_auth() {
        auth_options.push("No authentication".into());
    }
    auth_options.push("Legacy automatic resolution".into());
    let current_auth = config
        .active_connection()
        .map(|connection| match connection.auth {
            crate::config::ConnectionAuth::ApiKey { .. } => 0,
            crate::config::ConnectionAuth::OAuth { .. } if oauth_provider.is_some() => 1,
            crate::config::ConnectionAuth::None => auth_options
                .iter()
                .position(|value| value == "No authentication")
                .unwrap_or(0),
            crate::config::ConnectionAuth::Auto => auth_options.len() - 1,
            _ => 0,
        })
        .unwrap_or(0);
    match promptui::menu(
        "Authentication method",
        &auth_options,
        current_auth,
        true,
        "Explicit methods never fall through to another credential type.",
    )? {
        MenuResult::Selected(index) if auth_options[index] == "OAuth profile" => {
            let Some(profile) = promptui::text(
                "OAuth profile label",
                &oauth_profile_default(config),
                |value| {
                    crate::oauth::normalize_profile(value)?;
                    Ok(())
                },
            )?
            else {
                *config = before;
                return Ok(false);
            };
            if profile == ":back" {
                return Ok(false);
            }
            if let Some(connection) = config.active_connection_mut() {
                connection.auth = crate::config::ConnectionAuth::OAuth { profile };
            }
        }
        MenuResult::Selected(index) if auth_options[index] == "No authentication" => {
            if let Some(connection) = config.active_connection_mut() {
                connection.auth = crate::config::ConnectionAuth::None;
            }
        }
        MenuResult::Selected(index) if auth_options[index] == "Legacy automatic resolution" => {
            if let Some(connection) = config.active_connection_mut() {
                connection.auth = crate::config::ConnectionAuth::Auto;
            }
        }
        MenuResult::Selected(_) => {
            {
                let provider = active_provider_mut(config);
                let Some(credential) = promptui::text(
                    "Saved credential profile",
                    &provider.credential_profile(),
                    |value| {
                        crate::credentials::normalize_profile(value)?;
                        Ok(())
                    },
                )?
                else {
                    *config = before;
                    return Ok(false);
                };
                if credential == ":back" {
                    *config = before;
                    return Ok(false);
                }
                provider.credential = crate::credentials::normalize_profile(&credential)?;
                let Some(key_env) = promptui::text(
                    "Environment override variable",
                    &provider.api_key_env,
                    validate_env_name,
                )?
                else {
                    *config = before;
                    return Ok(false);
                };
                if key_env == ":back" {
                    *config = before;
                    return Ok(false);
                }
                provider.api_key_env = key_env;
                println!(
                    "  Secret values are managed separately; after Apply use `aishe auth set {}`.",
                    crate::commands::display_safe(&provider.credential_profile())
                );
            }
            let settings = draft_provider(config).clone();
            if let Some(connection) = config.active_connection_mut() {
                connection.auth = crate::config::ConnectionAuth::ApiKey {
                    credential: Some(settings.credential_profile()),
                    api_key_env: Some(settings.api_key_env),
                };
            }
        }
        _ => {
            *config = before;
            return Ok(false);
        }
    }
    let current_model = draft_provider(config).model.clone();
    let Some(model) = promptui::text(
        "Model",
        &current_model,
        crate::connection::validate_model_id,
    )?
    else {
        *config = before;
        return Ok(false);
    };
    if model == ":back" {
        *config = before;
        return Ok(false);
    }
    set_model(config, model.trim());
    if config.active_provider_name() != "anthropic" {
        let transports = vec![
            "Auto - Responses for OpenAI; Chat for compatible endpoints".into(),
            "Responses API".into(),
            "Chat Completions".into(),
        ];
        let current = match draft_provider(config).transport.as_str() {
            "responses" => 1,
            "chat" => 2,
            _ => 0,
        };
        match promptui::menu(
            "Transport",
            &transports,
            current,
            true,
            "GPT-5.6 reasoning plus tools requires the Responses API.",
        )? {
            MenuResult::Selected(index) => {
                active_provider_mut(config).transport = ["auto", "responses", "chat"][index].into()
            }
            _ => {
                *config = before;
                return Ok(false);
            }
        }
    }
    sync_legacy_provider(config);
    promptui::note("Connection changes added to your draft. Use Check connection from the settings hub before saving if needed.");
    Ok(true)
}

fn oauth_profile_default(config: &Config) -> String {
    match config
        .active_connection()
        .map(|connection| &connection.auth)
    {
        Some(crate::config::ConnectionAuth::OAuth { profile }) => profile.clone(),
        _ => "work".into(),
    }
}

fn shell_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!(
                "Appearance: {} / {} / {}",
                config.ui.theme, config.ui.unicode, config.ui.motion
            ),
            format!(
                "Prompt & status: prompt {} / status {}",
                on_off(config.aishe.pty_prompt),
                on_off(config.aishe.status_line)
            ),
            format!("Agent output: {}", config.backend.output),
            format!(
                "Shared shell history: {}",
                on_off(config.aishe.share_history)
            ),
            format!(
                "Hints: failures {} / discovery {}",
                on_off(config.aishe.failure_hints),
                on_off(config.aishe.discovery_hints)
            ),
            format!(
                "AI hook timeout: {} seconds",
                config.aishe.hook_timeout_secs
            ),
            "Restore terminal defaults".into(),
            "Back".into(),
        ];
        match promptui::menu("Terminal & history", &choices, 0, true,
            "Appearance previews immediately here. Saved shell defaults apply to new shells; history data is kept.")? {
            MenuResult::Selected(0) => appearance_section(config)?,
            MenuResult::Selected(1) => prompt_section(config)?,
            MenuResult::Selected(2) => choose_agent_output(config)?,
            MenuResult::Selected(3) => config.aishe.share_history = !config.aishe.share_history,
            MenuResult::Selected(4) => hints_section(config)?,
            MenuResult::Selected(5) => choose_hook_timeout(config)?,
            MenuResult::Selected(6) => reset_shell_section(config),
            MenuResult::Selected(7) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
        crate::ui::configure(&config.ui);
    }
}

fn appearance_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("Theme: {}", config.ui.theme),
            format!("Color depth: {}", config.ui.color_depth),
            format!("Characters: {}", config.ui.unicode),
            format!("Motion: {}", config.ui.motion),
            "Back".into(),
        ];
        match promptui::menu("Appearance", &choices, 0, true, "Preferences preview in this settings session. NO_COLOR and environment overrides still take precedence.")? {
            MenuResult::Selected(0) => choose_ui_value("Terminal theme", &mut config.ui.theme,
                &["auto", "dark", "light", "mono", "none"], "Auto follows the terminal background; none disables styling.")?,
            MenuResult::Selected(1) => choose_ui_value("Color depth", &mut config.ui.color_depth,
                &["auto", "16", "256", "truecolor", "none"], "Use 16 colors for basic or remote terminals.")?,
            MenuResult::Selected(2) => choose_ui_value("Character set", &mut config.ui.unicode,
                &["auto", "unicode", "ascii"], "ASCII uses plain symbols and borders.")?,
            MenuResult::Selected(3) => choose_ui_value("Terminal motion", &mut config.ui.motion,
                &["auto", "live", "static"], "Static keeps durable lines and avoids cursor redraws.")?,
            MenuResult::Selected(4) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
        crate::ui::configure(&config.ui);
    }
}

fn prompt_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("AIShe prompt: {}", on_off(config.aishe.pty_prompt)),
            format!("Right-side status: {}", on_off(config.aishe.status_line)),
            format!(
                "Status fields: {}",
                config.aishe.status_line_items.join(", ")
            ),
            "Preview prompt".into(),
            "Back".into(),
        ];
        match promptui::menu("Prompt & status", &choices, 0, true,
            "The lean prompt keeps mode and agent scope on the left. Right-side metadata yields to your command.")? {
            MenuResult::Selected(0) => config.aishe.pty_prompt = !config.aishe.pty_prompt,
            MenuResult::Selected(1) => choose_status_position(config)?,
            MenuResult::Selected(2) => choose_status_items(config)?,
            MenuResult::Selected(3) => print_status_preview(config),
            MenuResult::Selected(4) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn hints_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!(
                "Failed-command hints: {}",
                on_off(config.aishe.failure_hints)
            ),
            format!(
                "Command discovery hints: {}",
                on_off(config.aishe.discovery_hints)
            ),
            "Back".into(),
        ];
        match promptui::menu(
            "Shell hints",
            &choices,
            0,
            true,
            "Choose whether shell failures and unfamiliar commands suggest an AI follow-up.",
        )? {
            MenuResult::Selected(0) => config.aishe.failure_hints = !config.aishe.failure_hints,
            MenuResult::Selected(1) => config.aishe.discovery_hints = !config.aishe.discovery_hints,
            MenuResult::Selected(2) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn choose_ui_value(title: &str, value: &mut String, choices: &[&str], help: &str) -> Result<()> {
    let options = choices
        .iter()
        .map(|choice| (*choice).to_string())
        .collect::<Vec<_>>();
    let default = choices
        .iter()
        .position(|choice| choice.eq_ignore_ascii_case(value))
        .unwrap_or(0);
    if let MenuResult::Selected(index) = promptui::menu(title, &options, default, true, help)? {
        *value = choices[index].to_string();
    }
    Ok(())
}

fn choose_agent_output(config: &mut Config) -> Result<()> {
    let choices = vec![
        "Focus - final answer; live activity stays off scrollback".into(),
        "Compact - persistent one-line tool activity".into(),
        "Detailed - tools, output, diffs, and usage".into(),
    ];
    let default = match config.backend.output.as_str() {
        "compact" => 1,
        "detailed" => 2,
        _ => 0,
    };
    if let MenuResult::Selected(index) = promptui::menu(
        "Agent transcript density",
        &choices,
        default,
        true,
        "Ctrl-O or /details cycles focus, compact, and detailed for this shell.",
    )? {
        config.backend.output = ["focus", "compact", "detailed"][index].into();
    }
    Ok(())
}

fn choose_hook_timeout(config: &mut Config) -> Result<()> {
    let default = config.aishe.hook_timeout_secs.to_string();
    let Some(value) = promptui::text("AI hook timeout seconds (1-600)", &default, |value| {
        let seconds: u32 = value.parse().context("enter a whole number")?;
        if !(1..=600).contains(&seconds) {
            anyhow::bail!("timeout must be between 1 and 600 seconds")
        }
        Ok(())
    })?
    else {
        return Ok(());
    };
    if value != ":back" {
        config.aishe.hook_timeout_secs = value.parse()?;
    }
    Ok(())
}

fn choose_status_position(config: &mut Config) -> Result<()> {
    let choices = vec!["On - right-side metadata".into(), "Off".into()];
    let default = usize::from(!config.aishe.status_line);
    if let MenuResult::Selected(index) = promptui::menu(
        "Right-side status",
        &choices,
        default,
        true,
        "Mode and agent scope stay on the left when the AIShe prompt is enabled.",
    )? {
        config.aishe.status_line = index == 0;
        config.aishe.status_line_position = ["right", "off"][index].into();
        print_status_preview(config);
    }
    Ok(())
}

const STATUS_FIELDS: &[&str] = &[
    "identity",
    "connection",
    "provider",
    "endpoint",
    "auth",
    "selection",
    "model",
    "reasoning",
    "mode",
    "backend",
    "scope",
    "branch",
    "environment",
    "tasks",
    "task",
    "elapsed",
    "context",
    "last_tokens",
    "last_cost",
    "session_tokens",
    "session_cost",
    "requests",
    "plan",
];

fn parse_status_items(value: &str) -> Result<Vec<String>> {
    let mut items = Vec::new();
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        if !STATUS_FIELDS.contains(&item) {
            anyhow::bail!("unsupported status field '{item}'");
        }
        if !items.iter().any(|value| value == item) {
            items.push(item.to_string());
        }
    }
    if items.is_empty() {
        anyhow::bail!("choose at least one field");
    }
    Ok(items)
}

fn choose_status_items(config: &mut Config) -> Result<()> {
    let choices = vec![
        "Minimal - model and connection".into(),
        "Balanced - identity and recent activity".into(),
        "Usage - tokens, spend, and requests".into(),
        "Custom fields".into(),
    ];
    let presets: [&[&str]; 3] = [
        &["model", "connection"],
        &["model", "connection", "task", "elapsed"],
        &[
            "model",
            "connection",
            "last_tokens",
            "session_cost",
            "requests",
        ],
    ];
    let current = presets
        .iter()
        .position(|preset| {
            config
                .aishe
                .status_line_items
                .iter()
                .map(String::as_str)
                .eq(preset.iter().copied())
        })
        .unwrap_or(3);
    match promptui::menu("Status fields", &choices, current, true,
        "The lean prompt always includes model and connection. Extra fields appear when data exists and space permits; custom fields also serve legacy shells.")? {
        MenuResult::Selected(index @ 0..=2) => config.aishe.status_line_items = presets[index].iter().map(|value| (*value).into()).collect(),
        MenuResult::Selected(3) => {
            promptui::note(&format!("Available fields: {}", STATUS_FIELDS.join(", ")));
            if let Some(value) = promptui::text("Comma-separated fields", &config.aishe.status_line_items.join(","), |value| parse_status_items(value).map(|_| ()))? {
                if value != ":back" { config.aishe.status_line_items = parse_status_items(&value)?; }
            }
        }
        _ => return Ok(()),
    }
    print_status_preview(config);
    Ok(())
}

fn print_status_preview(config: &Config) {
    promptui::section("Prompt preview (illustrative)");
    if !config.aishe.pty_prompt {
        promptui::note(
            "Your own shell prompt is retained. AIShe's mode indicator requires the AIShe prompt.",
        );
    } else {
        promptui::key_value("Left", &format!("~/project  {}  >", mode_label(config)));
    }
    if !config.aishe.status_line {
        promptui::key_value("Right", "off");
        return;
    }
    let mut values: Vec<String> = vec![
        config.active_model().into(),
        connection_label(config).into(),
    ];
    for item in &config.aishe.status_line_items {
        if let Some(example) = match item.as_str() {
            "task" => Some("task <name>"),
            "elapsed" => Some("last <time>"),
            "context" => Some("context <tokens>"),
            "last_tokens" => Some("last <in>/<out> tok"),
            "last_cost" => Some("last $<cost>"),
            "session_tokens" => Some("session <in>/<out> tok"),
            "session_cost" => Some("session $<cost>"),
            "requests" => Some("<n> reqs"),
            _ => None,
        } {
            values.push(example.into());
        }
    }
    promptui::key_value("Right", &values.join(" / "));
    promptui::note("Placeholders show where real activity appears. Metadata shrinks on narrow terminals and yields to a long command; mode and scope stay visible.");
}

fn safety_section(config: &mut Config) -> Result<()> {
    promptui::note("Startup defaults only. Allow and agent require a fresh grant in each shell.");
    loop {
        let choices = vec![
            format!("Startup mode: {}", mode_label(config)),
            format!("Agent scope: {}", config.backend.default_scope),
            format!("Safety profile: {}", config.aishe.safety_profile),
            "Agent safeguards".into(),
            "Back".into(),
        ];
        match promptui::menu("Mode & safety", &choices, 0, true,
            "Allow and agent still ask for permission in each new shell. Changing a default never grants authority to this shell.")? {
            MenuResult::Selected(0) => {
                let options = vec!["Ask - review each proposed command".into(), "Allow - run safe commands after a shell grant".into(), "Agent - complete tasks within the selected scope".into()];
                let default = match config.aishe.mode.as_str() { "auto" | "allow" => 1, "yolo" | "agent" => 2, _ => 0 };
                if let MenuResult::Selected(index) = promptui::menu("Startup mode", &options, default, true, "This is a saved default, not an active permission grant.")? {
                    config.aishe.mode = ["suggest", "auto", "yolo"][index].into();
                    config.aishe.safety_profile = "custom".into();
                }
            }
            MenuResult::Selected(1) => {
                let options = vec!["Workspace - limit agent work to the project".into(), "Host - permit host-level tasks after an explicit grant".into()];
                if let MenuResult::Selected(index) = promptui::menu("Agent scope", &options, usize::from(config.backend.default_scope == "host"), true,
                    "Workspace uses configured isolation. Host scope requires a separate explicit grant each shell.")? {
                    config.backend.default_scope = ["workspace", "host"][index].into();
                }
            }
            MenuResult::Selected(2) => choose_safety_profile(config)?,
            MenuResult::Selected(3) => safeguards_section(config)?,
            MenuResult::Selected(4) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn choose_safety_profile(config: &mut Config) -> Result<()> {
    let choices = vec![
        "Conservative - ask before every command".into(),
        "Balanced - allow safe commands, confirm writes".into(),
        "Autonomous - agent with permission checks".into(),
        "Custom - keep current controls".into(),
    ];
    if let MenuResult::Selected(index @ 0..=3) = promptui::menu("Safety profile", &choices, profile_index(&config.aishe.safety_profile), true,
        "Profiles update startup mode and agent safeguards together. Scope and budget stay unchanged.")? {
        let profile = [Profile::Conservative, Profile::Balanced, Profile::Autonomous, Profile::Custom][index];
        for change in profiles::apply(config, profile) {
            promptui::key_value(&review_label(change.field), &format!("{} -> {}", change.before, change.after));
        }
    }
    Ok(())
}

fn safeguards_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("Confirmation: {}", config.aishe.yolo_confirm),
            format!("Review plan: {}", on_off(config.aishe.yolo_plan)),
            format!("Preview commands: {}", on_off(config.aishe.yolo_preview)),
            format!("Step limit: {}", config.aishe.max_yolo_iterations),
            "Back".into(),
        ];
        match promptui::menu(
            "Agent safeguards",
            &choices,
            0,
            true,
            "Individual edits set the safety profile to Custom. Isolation and scope still apply.",
        )? {
            MenuResult::Selected(0) => {
                let options = vec![
                    "All commands".into(),
                    "Writes and dangerous commands".into(),
                    "Dangerous commands".into(),
                ];
                let values = ["all", "writes", "dangerous"];
                if let MenuResult::Selected(index) = promptui::menu(
                    "Agent confirmation",
                    &options,
                    values
                        .iter()
                        .position(|value| *value == config.aishe.yolo_confirm)
                        .unwrap_or(0),
                    true,
                    "Choose which proposed agent commands require confirmation.",
                )? {
                    config.aishe.yolo_confirm = values[index].into();
                    config.aishe.safety_profile = "custom".into();
                }
            }
            MenuResult::Selected(1) => {
                config.aishe.yolo_plan = !config.aishe.yolo_plan;
                config.aishe.safety_profile = "custom".into();
            }
            MenuResult::Selected(2) => {
                config.aishe.yolo_preview = !config.aishe.yolo_preview;
                config.aishe.safety_profile = "custom".into();
            }
            MenuResult::Selected(3) => {
                if let Some(value) = promptui::text(
                    "Agent step limit (1-100)",
                    &config.aishe.max_yolo_iterations.to_string(),
                    |value| {
                        let steps: usize = value.parse().context("enter a whole number")?;
                        if !(1..=100).contains(&steps) {
                            anyhow::bail!("choose a limit between 1 and 100");
                        }
                        Ok(())
                    },
                )? {
                    if value != ":back" {
                        config.aishe.max_yolo_iterations = value.parse()?;
                        config.aishe.safety_profile = "custom".into();
                    }
                }
            }
            MenuResult::Selected(4) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn context_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!(
                "Project context: {}",
                included(config, "project_context", config.aishe.project_context)
            ),
            format!(
                "Project tasks: {}",
                included(config, "project_tasks", config.aishe.project_tasks)
            ),
            format!(
                "Host profile: {}",
                included(config, "host_profile", config.aishe.host_profile)
            ),
            format!("Secret redaction: {}", on_off(config.aishe.redact_secrets)),
            format!("Conversation memory: {}", on_off(config.aishe.memory)),
            "Reset this section to defaults".into(),
            "Back".into(),
        ];
        match promptui::menu(
            "Context & privacy",
            &choices,
            0,
            true,
            "Core cwd/shell facts remain required. Excluded optional sections are never rendered.",
        )? {
            MenuResult::Selected(0) => toggle_context(config, "project_context"),
            MenuResult::Selected(1) => toggle_context(config, "project_tasks"),
            MenuResult::Selected(2) => toggle_context(config, "host_profile"),
            MenuResult::Selected(3) => config.aishe.redact_secrets = !config.aishe.redact_secrets,
            MenuResult::Selected(4) => config.aishe.memory = !config.aishe.memory,
            MenuResult::Selected(5) => reset_context_section(config),
            MenuResult::Selected(6) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn cost_section(config: &mut Config) -> Result<()> {
    loop {
        let model = config.active_model().to_string();
        let current = usage::budget_price_for(&model, &config.pricing);
        let choices = vec![
            format!(
                "Exact model price: {}",
                current
                    .map(|price| format!("${} in / ${} out per 1M", price.input, price.output))
                    .unwrap_or_else(|| "unknown".into())
            ),
            format!("Session budget: {}", budget_label(config)),
            format!("Per-call usage: {}", on_off(config.aishe.show_usage)),
            format!("Audit logging: {}", on_off(config.logging.enabled)),
            format!("Audit redaction: {}", on_off(config.logging.redact)),
            "Back".into(),
        ];
        match promptui::menu("Usage & logging", &choices, 0, true,
            "Budgets need an exact model price. Display estimates may use broader rates. Token usage is still available when pricing is unknown.")? {
            MenuResult::Selected(0) => {
                promptui::key_value("Model", &model);
                let input_default = current.map(|price| price.input).unwrap_or(0.0).to_string();
                let output_default = current.map(|price| price.output).unwrap_or(0.0).to_string();
                let Some(input) = promptui::text("Input USD per 1M", &input_default, validate_rate_text)? else { continue; };
                if input == ":back" { continue; }
                let Some(output) = promptui::text("Output USD per 1M", &output_default, validate_rate_text)? else { continue; };
                if output == ":back" { continue; }
                config.pricing.insert(model, Price { input: input.parse()?, output: output.parse()? });
            }
            MenuResult::Selected(1) => {
                if let Some(value) = promptui::text("Session budget USD (0 = unlimited)", &config.aishe.budget_usd.to_string(), validate_rate_text)? {
                    if value != ":back" { config.aishe.budget_usd = value.parse()?; }
                }
            }
            MenuResult::Selected(2) => config.aishe.show_usage = !config.aishe.show_usage,
            MenuResult::Selected(3) => config.logging.enabled = !config.logging.enabled,
            MenuResult::Selected(4) => config.logging.redact = !config.logging.redact,
            MenuResult::Selected(5) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn advanced_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("Reasoning effort: {}", config.active_reasoning_effort()),
            format!("Structured output: {}", config.aishe.structured),
            format!("Streaming: {}", on_off(config.aishe.stream)),
            format!("Response cache: {}", on_off(config.aishe.cache)),
            "Reset this section to defaults".into(),
            "Back".into(),
        ];
        match promptui::menu(
            "Response tuning",
            &choices,
            0,
            true,
            "Auto reasoning follows provider defaults; GPT-5.6 tools use Responses.",
        )? {
            MenuResult::Selected(0) => {
                let options: Vec<String> =
                    ["auto", "none", "low", "medium", "high", "xhigh", "max"]
                        .into_iter()
                        .map(ToOwned::to_owned)
                        .collect();
                if let MenuResult::Selected(index) = promptui::menu(
                    "Reasoning effort",
                    &options,
                    options
                        .iter()
                        .position(|value| value == config.active_reasoning_effort())
                        .unwrap_or(0),
                    true,
                    "Auto omits an explicit effort unless compatibility requires none.",
                )? {
                    config.set_active_reasoning_effort(options[index].clone());
                }
            }
            MenuResult::Selected(1) => {
                let options = vec!["schema".into(), "json".into(), "prompt".into()];
                if let MenuResult::Selected(index) = promptui::menu(
                    "Structured output",
                    &options,
                    options
                        .iter()
                        .position(|value| value == &config.aishe.structured)
                        .unwrap_or(0),
                    true,
                    "Strict schema is preferred; the provider can step down when unsupported.",
                )? {
                    config.aishe.structured = options[index].clone();
                }
            }
            MenuResult::Selected(2) => config.aishe.stream = !config.aishe.stream,
            MenuResult::Selected(3) => config.aishe.cache = !config.aishe.cache,
            MenuResult::Selected(4) => reset_advanced_section(config),
            MenuResult::Selected(5) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn reset_shell_section(config: &mut Config) {
    let defaults = Config::default();
    config.aishe.share_history = defaults.aishe.share_history;
    config.aishe.pty_prompt = defaults.aishe.pty_prompt;
    config.aishe.hook_timeout_secs = defaults.aishe.hook_timeout_secs;
    config.aishe.failure_hints = defaults.aishe.failure_hints;
    config.aishe.discovery_hints = defaults.aishe.discovery_hints;
    config.aishe.status_line = defaults.aishe.status_line;
    config.aishe.status_line_position = defaults.aishe.status_line_position;
    config.aishe.status_line_items = defaults.aishe.status_line_items;
    config.backend.output = defaults.backend.output;
    config.ui = defaults.ui;
}

fn reset_context_section(config: &mut Config) {
    let defaults = Config::default();
    config.aishe.project_context = defaults.aishe.project_context;
    config.aishe.project_tasks = defaults.aishe.project_tasks;
    config.aishe.host_profile = defaults.aishe.host_profile;
    config.aishe.context_exclude = defaults.aishe.context_exclude;
    config.aishe.redact_secrets = defaults.aishe.redact_secrets;
    config.aishe.memory = defaults.aishe.memory;
}

fn reset_advanced_section(config: &mut Config) {
    let defaults = Config::default();
    if let Some(connection) = config.active_connection_mut() {
        connection.reasoning_effort = None;
    }
    config.aishe.reasoning_effort = defaults.aishe.reasoning_effort;
    config.aishe.structured = defaults.aishe.structured;
    config.aishe.stream = defaults.aishe.stream;
    config.aishe.cache = defaults.aishe.cache;
}

fn active_provider(config: &Config) -> &crate::config::ProviderConfig {
    config.active_provider_config()
}

/// During a provider transaction the named draft is authoritative. Canonical
/// legacy-auto connections otherwise temporarily fall back to their old mirror
/// while the endpoint, authentication, and transport are being edited.
fn draft_provider(config: &Config) -> &crate::config::ProviderConfig {
    config
        .active_connection()
        .map(|connection| &connection.settings)
        .unwrap_or_else(|| config.active_provider_config())
}

fn legacy_provider_key(config: &Config) -> Option<&str> {
    let connection = config.active_connection()?;
    (matches!(connection.auth, crate::config::ConnectionAuth::Auto)
        && config.active_connection_id() == connection.provider
        && matches!(connection.provider.as_str(), "anthropic" | "openai"))
    .then_some(connection.provider.as_str())
}

fn set_model(config: &mut Config, model: &str) {
    let mirror = legacy_provider_key(config).map(ToOwned::to_owned);
    active_provider_mut(config).model = model.into();
    if let Some(provider) = mirror {
        if provider == "anthropic" {
            config.providers.anthropic.model = model.into();
        } else {
            config.providers.openai.model = model.into();
        }
    }
}

fn sync_legacy_provider(config: &mut Config) {
    let Some(provider) = legacy_provider_key(config).map(ToOwned::to_owned) else {
        return;
    };
    let settings = draft_provider(config).clone();
    if provider == "anthropic" {
        config.providers.anthropic = settings;
    } else {
        config.providers.openai = settings;
    }
}

fn active_provider_mut(config: &mut Config) -> &mut crate::config::ProviderConfig {
    let id = config.active_connection_id().to_string();
    if config.connections.contains_key(&id) {
        &mut config.connections.get_mut(&id).expect("checked").settings
    } else if config.aishe.provider == "anthropic" {
        &mut config.providers.anthropic
    } else {
        &mut config.providers.openai
    }
}

fn choose_connection(config: &mut Config) -> Result<bool> {
    let ids: Vec<String> = config.connections.keys().cloned().collect();
    let mut labels: Vec<String> = ids
        .iter()
        .map(|id| {
            let connection = &config.connections[id];
            format!(
                "{} ({}) / {} / {} / {}",
                crate::commands::display_safe(&connection.label),
                crate::commands::display_safe(id),
                crate::commands::display_safe(&connection.provider),
                crate::commands::display_safe(&connection.settings.model),
                crate::commands::display_safe(&connection.auth_label().replace('·', "/"))
            )
        })
        .collect();
    labels.push("Back".into());
    let default = ids
        .iter()
        .position(|id| id == config.active_connection_id())
        .unwrap_or(0);
    match promptui::menu(
        "Default connection",
        &labels,
        default,
        true,
        "Select the default connection for new shells. Endpoint and model edits apply only to this named connection.",
    )? {
        MenuResult::Selected(index) if index < ids.len() => {
            config.select_connection(&ids[index])?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn apply_service_to_active_connection(config: &mut Config, service: &provider_catalog::Service) {
    let provider_name = match service.family {
        Family::Anthropic => "anthropic",
        Family::OpenAiCompatible => service.key,
    };
    // Mutate the chosen identity before changing the compatibility provider
    // selector, which could otherwise redirect lookup to a different sibling.
    if let Some(connection) = config.active_connection_mut() {
        connection.provider = provider_name.into();
        provider_catalog::apply(service, &mut connection.settings);
    } else {
        let provider = if service.family == Family::Anthropic {
            &mut config.providers.anthropic
        } else {
            &mut config.providers.openai
        };
        provider_catalog::apply(service, provider);
    }
    config.aishe.provider = provider_name.into();
    // A named edit must not rewrite a different canonical Auto connection's
    // effective legacy settings. Its own mirror is synchronized on completion.
}

fn service_index(config: &Config) -> usize {
    provider_catalog::SERVICES
        .iter()
        .position(|service| {
            service.family
                == if config.active_provider_name() == "anthropic" {
                    Family::Anthropic
                } else {
                    Family::OpenAiCompatible
                }
                && !service.base_url.is_empty()
                && service.base_url == active_provider(config).base_url
        })
        .unwrap_or_else(|| {
            provider_catalog::SERVICES
                .iter()
                .position(|service| service.key == "custom")
                .unwrap_or(0)
        })
}

fn validate_env_name(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("environment variable name cannot be empty")
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || !chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
    {
        anyhow::bail!("use a shell variable name such as OPENAI_API_KEY")
    }
    Ok(())
}

fn validate_rate_text(value: &str) -> Result<()> {
    let rate: f64 = value.parse().context("enter a number")?;
    if !rate.is_finite() || rate < 0.0 {
        anyhow::bail!("rate must be finite and non-negative")
    }
    Ok(())
}

fn toggle_context(config: &mut Config, section: &str) {
    let enabled = match section {
        "project_context" => &mut config.aishe.project_context,
        "project_tasks" => &mut config.aishe.project_tasks,
        "host_profile" => &mut config.aishe.host_profile,
        _ => return,
    };
    let excluded = config
        .aishe
        .context_exclude
        .iter()
        .any(|item| item == section);
    if !*enabled || excluded {
        *enabled = true;
        config.aishe.context_exclude.retain(|item| item != section);
    } else {
        config.aishe.context_exclude.push(section.into());
    }
}

fn included(config: &Config, section: &str, legacy_flag: bool) -> &'static str {
    if legacy_flag
        && !config
            .aishe
            .context_exclude
            .iter()
            .any(|item| item == section)
    {
        "included"
    } else {
        "excluded"
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

fn profile_index(value: &str) -> usize {
    match Profile::parse(value) {
        Some(Profile::Conservative) => 0,
        Some(Profile::Balanced) => 1,
        Some(Profile::Autonomous) => 2,
        _ => 3,
    }
}

fn print_capabilities(report: &capabilities::Report) {
    crate::cli::settings::print_capability_report(report);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_toggle_is_reversible() {
        let mut config = Config::default();
        toggle_context(&mut config, "host_profile");
        assert_eq!(included(&config, "host_profile", true), "excluded");
        toggle_context(&mut config, "host_profile");
        assert_eq!(included(&config, "host_profile", true), "included");
    }

    #[test]
    fn context_toggle_reenables_a_legacy_disabled_section() {
        let mut config = Config::default();
        config.aishe.host_profile = false;
        assert_eq!(
            included(&config, "host_profile", config.aishe.host_profile),
            "excluded"
        );
        toggle_context(&mut config, "host_profile");
        assert_eq!(
            included(&config, "host_profile", config.aishe.host_profile),
            "included"
        );
        toggle_context(&mut config, "host_profile");
        assert_eq!(
            included(&config, "host_profile", config.aishe.host_profile),
            "excluded"
        );
    }

    #[test]
    fn custom_status_fields_validate_deduplicate_and_keep_order() {
        assert_eq!(
            parse_status_items("requests, model,requests,last_cost").unwrap(),
            ["requests", "model", "last_cost"]
        );
        assert!(parse_status_items("unknown").is_err());
        assert!(parse_status_items(" , ").is_err());
    }

    #[test]
    fn canonical_auto_model_edit_preserves_credentials_and_endpoint() {
        let mut config = Config::default();
        config.select_connection("openai").unwrap();
        // A direct legacy edit is still the effective provider configuration.
        config.providers.openai.base_url = "https://custom.example/v1".into();
        config.providers.openai.credential = "private-account".into();
        let before = config.clone();
        set_model(&mut config, "new-model");
        assert_eq!(config.active_model(), "new-model");
        assert_eq!(
            config.active_provider_config().base_url,
            before.active_provider_config().base_url
        );
        assert_eq!(
            config.active_provider_config().credential,
            before.active_provider_config().credential
        );
        assert_eq!(
            config.connections["openai"].auth,
            before.connections["openai"].auth
        );
        assert_eq!(
            config.connections["openai"].settings.base_url,
            before.connections["openai"].settings.base_url
        );
        assert_eq!(
            config.connections["anthropic"],
            before.connections["anthropic"]
        );
        let mut expected = before;
        expected
            .connections
            .get_mut("openai")
            .unwrap()
            .settings
            .model = "new-model".into();
        expected.providers.openai.model = "new-model".into();
        assert_eq!(
            serde_json::to_value(config).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn canonical_xai_edits_and_sync_preserve_openai_sibling_effective_settings() {
        let mut config = Config::default();
        let openai_before = config.connections["openai"].clone();
        let legacy_before = config.providers.openai.clone();
        let mut xai = openai_before.clone();
        xai.provider = "xai".into();
        xai.label = "xAI".into();
        xai.settings.base_url = "https://api.x.ai".into();
        xai.settings.model = "grok-old".into();
        xai.settings.credential = "xai-account".into();
        xai.settings.api_key_env = "XAI_API_KEY".into();
        config.connections.insert("xai".into(), xai);
        config.select_connection("xai").unwrap();
        assert_eq!(config.active_model(), "grok-old");
        set_model(&mut config, "grok-new");
        active_provider_mut(&mut config).base_url = "https://xai.example/v1".into();
        sync_legacy_provider(&mut config);
        assert_eq!(config.active_model(), "grok-new");
        assert_eq!(
            config.active_provider_config().base_url,
            "https://xai.example/v1"
        );
        assert_eq!(
            config.connections["xai"].auth,
            crate::config::ConnectionAuth::Auto
        );
        assert_eq!(config.connections["openai"], openai_before);
        assert_eq!(config.providers.openai, legacy_before);
        config.select_connection("openai").unwrap();
        assert_eq!(config.active_provider_config(), &legacy_before);
        assert_eq!(config.active_connection().unwrap().auth, openai_before.auth);
    }

    #[test]
    fn review_changes_include_named_connection_and_skip_legacy_mirror() {
        let baseline = Config::default();
        let mut draft = baseline.clone();
        draft.connections.get_mut("openai").unwrap().settings.model = "custom-model".into();
        draft.providers.openai.model = "custom-model".into();
        draft.aishe.hook_timeout_secs = 75;
        let changes = draft_changes(&baseline, &draft).unwrap();
        assert_eq!(changes.len(), 2);
        assert!(changes
            .iter()
            .any(|change| change.path == "connections.openai.model"));
        assert!(changes
            .iter()
            .any(|change| change.path == "aishe.hook_timeout_secs"));
        assert!(draft_changes(&baseline, &baseline).unwrap().is_empty());
    }

    #[test]
    fn review_redacts_endpoint_credentials_and_escapes_controls() {
        let value = json!("https://alice:private-password@host.example/v1\n\u{1b}[2J");
        let shown = review_value(Some(&value));
        assert!(!shown.contains("private-password"));
        assert!(shown.contains("<redacted>@host.example"));
        assert!(!shown.contains('\u{1b}'));
        assert!(!shown.contains('\n'));
    }

    #[test]
    fn managed_review_enforces_scope_logging_redaction_and_budget_without_mutating_input() {
        let mut draft = Config::default();
        draft.backend.default_scope = "host".into();
        draft.logging.enabled = false;
        draft.logging.redact = false;
        draft.aishe.redact_secrets = false;
        draft.aishe.budget_usd = 20.0;
        let policy = crate::policy::OrganizationPolicy {
            allow_host_yolo: Some(false),
            require_audit_logging: Some(true),
            require_redaction: Some(true),
            max_budget_usd: Some(5.0),
            ..Default::default()
        };
        let (saved, changes) = constrained_draft(&draft, &policy).unwrap();
        assert_eq!(saved.backend.default_scope, "workspace");
        assert!(saved.logging.enabled && saved.logging.redact && saved.aishe.redact_secrets);
        assert_eq!(saved.aishe.budget_usd, 5.0);
        assert!(changes
            .iter()
            .any(|change| change.path == "backend.default_scope"));
        assert_eq!(draft.backend.default_scope, "host");
        assert!(!draft.logging.enabled);
    }

    #[test]
    fn review_groups_authority_and_exact_model_pricing() {
        assert_eq!(review_group("aishe.yolo_confirm"), "Mode & safety");
        assert_eq!(review_group("backend.default_scope"), "Mode & safety");
        assert_eq!(review_group("pricing.gpt-5.6.input"), "Pricing: gpt-5.6");
        assert_eq!(
            review_label("connections.work.auth.api_key_env"),
            "Auth key var"
        );
        assert_eq!(review_label("connections.work.api_key_env"), "Key variable");
    }

    #[test]
    fn status_fields_have_stable_order() {
        let mut config = Config::default();
        config.aishe.status_line_items =
            vec!["requests".into(), "model".into(), "last_cost".into()];
        assert_eq!(
            config.aishe.status_line_items,
            ["requests", "model", "last_cost"]
        );
    }

    #[test]
    fn service_edits_preserve_the_exact_named_connection() {
        let mut config = Config::default();
        let template = config.connections["openai"].clone();
        config.connections.clear();
        config
            .connections
            .insert("openai-personal".into(), template.clone());
        config.connections.insert("openai-work".into(), template);
        config.aishe.connection = "openai-personal".into();
        config.aishe.connection_fallback = "openai-work".into();
        config.aishe.provider = "openai".into();

        let untouched = config.connections["openai-work"].clone();
        apply_service_to_active_connection(&mut config, provider_catalog::find("xai").unwrap());

        assert_eq!(config.active_connection_id(), "openai-personal");
        assert_eq!(config.active_provider_name(), "xai");
        assert_eq!(config.active_provider_config().base_url, "https://api.x.ai");
        assert_eq!(config.connections["openai-work"], untouched);
        config.set_active_reasoning_effort("high".into());
        assert_eq!(
            config.connections["openai-personal"]
                .reasoning_effort
                .as_deref(),
            Some("high")
        );
        assert_eq!(config.connections["openai-work"].reasoning_effort, None);
    }

    #[test]
    fn named_provider_edit_preserves_canonical_auto_sibling_effective_settings() {
        let mut config = Config::default();
        let canonical = config.connections["openai"].clone();
        let canonical_effective = config.providers.openai.clone();
        config.connections.insert("work".into(), canonical.clone());
        config.select_connection("work").unwrap();
        apply_service_to_active_connection(&mut config, provider_catalog::find("xai").unwrap());
        sync_legacy_provider(&mut config);
        assert_eq!(config.connections["openai"], canonical);
        assert_eq!(config.providers.openai, canonical_effective);
        config.select_connection("openai").unwrap();
        assert_eq!(config.active_provider_config(), &canonical_effective);
    }

    #[test]
    fn section_resets_restore_only_the_selected_section() {
        let defaults = Config::default();
        let mut config = Config::default();
        config.aishe.provider = "openai".into();
        config.aishe.mode = "yolo".into();
        config.aishe.budget_usd = 42.0;

        config.aishe.share_history = !defaults.aishe.share_history;
        config.aishe.pty_prompt = !defaults.aishe.pty_prompt;
        config.aishe.failure_hints = !defaults.aishe.failure_hints;
        config.aishe.discovery_hints = !defaults.aishe.discovery_hints;
        config.aishe.hook_timeout_secs = 1;
        config.aishe.status_line = !defaults.aishe.status_line;
        config.aishe.status_line_position = "off".into();
        config.aishe.status_line_items = vec!["requests".into()];
        config.backend.output = "detailed".into();
        reset_shell_section(&mut config);
        assert_eq!(config.aishe.share_history, defaults.aishe.share_history);
        assert_eq!(config.aishe.pty_prompt, defaults.aishe.pty_prompt);
        assert_eq!(config.aishe.failure_hints, defaults.aishe.failure_hints);
        assert_eq!(config.aishe.discovery_hints, defaults.aishe.discovery_hints);
        assert_eq!(
            config.aishe.hook_timeout_secs,
            defaults.aishe.hook_timeout_secs
        );
        assert_eq!(config.aishe.status_line, defaults.aishe.status_line);
        assert_eq!(
            config.aishe.status_line_position,
            defaults.aishe.status_line_position
        );
        assert_eq!(
            config.aishe.status_line_items,
            defaults.aishe.status_line_items
        );
        assert_eq!(config.backend.output, defaults.backend.output);

        config.aishe.project_context = !defaults.aishe.project_context;
        config.aishe.project_tasks = !defaults.aishe.project_tasks;
        config.aishe.host_profile = !defaults.aishe.host_profile;
        config.aishe.context_exclude = vec!["history".into()];
        config.aishe.redact_secrets = !defaults.aishe.redact_secrets;
        config.aishe.memory = !defaults.aishe.memory;
        reset_context_section(&mut config);
        assert_eq!(config.aishe.project_context, defaults.aishe.project_context);
        assert_eq!(config.aishe.project_tasks, defaults.aishe.project_tasks);
        assert_eq!(config.aishe.host_profile, defaults.aishe.host_profile);
        assert_eq!(config.aishe.context_exclude, defaults.aishe.context_exclude);
        assert_eq!(config.aishe.redact_secrets, defaults.aishe.redact_secrets);
        assert_eq!(config.aishe.memory, defaults.aishe.memory);

        config.aishe.reasoning_effort = "high".into();
        config.aishe.structured = "prompt".into();
        config.aishe.stream = !defaults.aishe.stream;
        config.aishe.cache = !defaults.aishe.cache;
        reset_advanced_section(&mut config);
        assert_eq!(
            config.aishe.reasoning_effort,
            defaults.aishe.reasoning_effort
        );
        assert_eq!(config.aishe.structured, defaults.aishe.structured);
        assert_eq!(config.aishe.stream, defaults.aishe.stream);
        assert_eq!(config.aishe.cache, defaults.aishe.cache);

        assert_eq!(config.aishe.provider, "openai");
        assert_eq!(config.aishe.mode, "yolo");
        assert_eq!(config.aishe.budget_usd, 42.0);
    }
}
