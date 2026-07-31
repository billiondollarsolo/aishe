//! Interactive settings hub and effective-configuration provenance.
//! All edits happen against a draft and are written only after final review.

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
    let provider = active_provider(&config);
    let provider_prefix = format!("providers.{}", config.aishe.provider);
    let fields = vec![
        field("aishe.provider", json!(config.aishe.provider), &source),
        field(
            &format!("{provider_prefix}.base_url"),
            json!(provider.base_url),
            &source,
        ),
        field(
            &format!("{provider_prefix}.api_key_env"),
            json!(provider.api_key_env),
            &source,
        ),
        field(
            &format!("{provider_prefix}.credential"),
            json!(provider.credential_profile()),
            &source,
        ),
        field(
            &format!("{provider_prefix}.model"),
            json!(provider.model),
            &source,
        ),
        field(
            &format!("{provider_prefix}.transport"),
            json!(provider.transport),
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
        field(
            "aishe.failure_hints",
            json!(config.aishe.failure_hints),
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
            "  {:32} {:24} ← {}",
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
    let mut draft = baseline.clone();
    loop {
        promptui::header(
            "aishe settings",
            "Edit configuration in reviewable, transactional sections.",
            "Nothing is saved until you choose Review and apply.",
        );
        println!(
            "  {} · {} · {} · status {}",
            draft.aishe.provider,
            draft.active_model(),
            draft.aishe.safety_profile,
            if draft.aishe.status_line {
                draft.aishe.status_line_position.as_str()
            } else {
                "off"
            }
        );
        let options = vec![
            "Provider & model (transactional)".into(),
            "Shell, history & statusline".into(),
            "Mode & safety".into(),
            "Context & privacy".into(),
            "Cost & logging".into(),
            "Advanced".into(),
            "Verify provider capabilities".into(),
            "Review and apply".into(),
            "Exit without changes".into(),
        ];
        match promptui::menu(
            "Choose a section",
            &options,
            0,
            false,
            "Every section edits a draft. Nothing is saved until Review and apply.",
        )? {
            MenuResult::Selected(0) => provider_section(&mut draft)?,
            MenuResult::Selected(1) => shell_section(&mut draft)?,
            MenuResult::Selected(2) => safety_section(&mut draft)?,
            MenuResult::Selected(3) => context_section(&mut draft)?,
            MenuResult::Selected(4) => cost_section(&mut draft)?,
            MenuResult::Selected(5) => advanced_section(&mut draft)?,
            MenuResult::Selected(6) => {
                let Some(live) =
                    promptui::confirm("Make minimal live requests (uses tokens)", true)?
                else {
                    continue;
                };
                print_capabilities(&capabilities::validate(&draft, live));
            }
            MenuResult::Selected(7) => {
                let before = toml::to_string_pretty(&baseline)?;
                let after = toml::to_string_pretty(&draft)?;
                let diff = crate::undo::unified_diff(&before, &after);
                if diff.is_empty() {
                    println!("\n  No changes to apply.");
                    continue;
                }
                println!("\n{diff}");
                if promptui::confirm("Apply these settings", true)?.unwrap_or(false) {
                    crate::setup::validate_config(&draft)?;
                    let backup = crate::setup::save_applied(&draft)?;
                    println!("  saved: {}", Config::path().display());
                    if let Some(path) = backup {
                        println!("  backup: {}", path.display());
                    }
                    return Ok(true);
                }
            }
            MenuResult::Selected(8) | MenuResult::Cancel => return Ok(false),
            MenuResult::Back | MenuResult::Selected(_) => {}
        }
    }
}

fn provider_section(config: &mut Config) -> Result<()> {
    let before = config.clone();
    let labels: Vec<String> = provider_catalog::SERVICES
        .iter()
        .map(|service| format!("{} — {}", service.label, service.help))
        .chain(std::iter::once("Back".into()))
        .collect();
    let selection = promptui::menu(
        "Provider service",
        &labels,
        service_index(config),
        true,
        "The provider, endpoint, credential name, model, and transport are kept as one draft.",
    )?;
    let MenuResult::Selected(index) = selection else {
        return Ok(());
    };
    if index >= provider_catalog::SERVICES.len() {
        return Ok(());
    }
    let service = &provider_catalog::SERVICES[index];
    match service.family {
        Family::Anthropic => {
            config.select_connection("anthropic")?;
            provider_catalog::apply(service, active_provider_mut(config));
        }
        Family::OpenAiCompatible => {
            config.select_connection("openai")?;
            provider_catalog::apply(service, active_provider_mut(config));
        }
    }
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
        return Ok(());
    };
    if endpoint == ":back" {
        *config = before;
        return Ok(());
    }
    provider.base_url = provider_catalog::normalize_base_url(&endpoint);
    provider.auth_required = Some(!crate::config::is_loopback_url(&provider.base_url));
    if provider.requires_auth() {
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
            return Ok(());
        };
        if credential == ":back" {
            *config = before;
            return Ok(());
        }
        provider.credential = crate::credentials::normalize_profile(&credential)?;
        let Some(key_env) = promptui::text(
            "Environment override variable",
            &provider.api_key_env,
            validate_env_name,
        )?
        else {
            *config = before;
            return Ok(());
        };
        if key_env == ":back" {
            *config = before;
            return Ok(());
        }
        provider.api_key_env = key_env;
        println!(
            "  Secret values are managed separately; after Apply use `aishe auth set {}`.",
            crate::commands::display_safe(&provider.credential_profile())
        );
    }
    let oauth_provider =
        crate::oauth::OAuthProvider::from_base_url(&active_provider(config).base_url);
    let mut auth_options = vec!["API key".to_string()];
    if oauth_provider.is_some() {
        auth_options.push("OAuth profile".into());
    }
    if !active_provider(config).requires_auth() {
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
            let Some(profile) = promptui::text("OAuth profile label", "work", |value| {
                crate::oauth::normalize_profile(value)?;
                Ok(())
            })?
            else {
                *config = before;
                return Ok(());
            };
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
            let settings = active_provider(config).clone();
            if let Some(connection) = config.active_connection_mut() {
                connection.auth = crate::config::ConnectionAuth::ApiKey {
                    credential: Some(settings.credential_profile()),
                    api_key_env: Some(settings.api_key_env),
                };
            }
        }
        _ => {
            *config = before;
            return Ok(());
        }
    }
    let current_model = active_provider(config).model.clone();
    let Some(model) = promptui::text("Model", &current_model, |value| {
        if value.trim().is_empty() {
            anyhow::bail!("model cannot be empty")
        }
        Ok(())
    })?
    else {
        *config = before;
        return Ok(());
    };
    if model == ":back" {
        *config = before;
        return Ok(());
    }
    active_provider_mut(config).model = model;
    if config.aishe.provider == "openai" {
        let transports = vec![
            "Auto — Responses for official OpenAI, Chat for compatible endpoints".into(),
            "Responses API".into(),
            "Chat Completions".into(),
        ];
        let current = match active_provider(config).transport.as_str() {
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
                return Ok(());
            }
        }
    }
    let live = promptui::confirm("Validate this provider transaction now", true)?;
    if live.is_none() {
        *config = before;
        return Ok(());
    }
    let report = capabilities::validate(config, live.unwrap_or(false));
    print_capabilities(&report);
    if !promptui::confirm("Keep this provider draft", true)?.unwrap_or(false) {
        *config = before;
    }
    Ok(())
}

fn shell_section(config: &mut Config) -> Result<()> {
    loop {
        let choices = vec![
            format!("History sharing: {}", on_off(config.aishe.share_history)),
            format!("Branded prompt: {}", on_off(config.aishe.pty_prompt)),
            format!("Failure hints: {}", on_off(config.aishe.failure_hints)),
            format!(
                "AI hook timeout: {} seconds",
                config.aishe.hook_timeout_secs
            ),
            format!(
                "Statusline: {}",
                if config.aishe.status_line {
                    config.aishe.status_line_position.as_str()
                } else {
                    "off"
                }
            ),
            format!(
                "Status fields: {}",
                config.aishe.status_line_items.join(",")
            ),
            format!("Agent transcript: {}", config.backend.output),
            "Reset this section to defaults".into(),
            "Back".into(),
        ];
        match promptui::menu(
            "Shell, history & statusline",
            &choices,
            0,
            true,
            "History data is never deleted by changing these controls.",
        )? {
            MenuResult::Selected(0) => config.aishe.share_history = !config.aishe.share_history,
            MenuResult::Selected(1) => config.aishe.pty_prompt = !config.aishe.pty_prompt,
            MenuResult::Selected(2) => config.aishe.failure_hints = !config.aishe.failure_hints,
            MenuResult::Selected(3) => choose_hook_timeout(config)?,
            MenuResult::Selected(4) => choose_status_position(config)?,
            MenuResult::Selected(5) => choose_status_items(config)?,
            MenuResult::Selected(6) => choose_agent_output(config)?,
            MenuResult::Selected(7) => reset_shell_section(config),
            MenuResult::Selected(8) | MenuResult::Back | MenuResult::Cancel => return Ok(()),
            MenuResult::Selected(_) => {}
        }
    }
}

fn choose_agent_output(config: &mut Config) -> Result<()> {
    let choices = vec![
        "Focus — final answer; live activity stays off scrollback".into(),
        "Compact — persistent one-line tool activity".into(),
        "Detailed — expanded tool summaries, output, diffs, and usage".into(),
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
        "Ctrl-O or `details` toggles focus/detailed for only the current shell.",
    )? {
        config.backend.output = ["focus", "compact", "detailed"][index].into();
    }
    Ok(())
}

fn choose_hook_timeout(config: &mut Config) -> Result<()> {
    let default = config.aishe.hook_timeout_secs.to_string();
    let Some(value) = promptui::text("AI hook timeout seconds (1–600)", &default, |value| {
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
    let choices = vec![
        "Right prompt".into(),
        "Below (Codex-style)".into(),
        "Off".into(),
    ];
    let default = match config.aishe.status_line_position.as_str() {
        "below" => 1,
        "off" => 2,
        _ => 0,
    };
    if let MenuResult::Selected(index) = promptui::menu(
        "Statusline placement",
        &choices,
        default,
        true,
        "Preview is shown after selection.",
    )? {
        config.aishe.status_line = index != 2;
        config.aishe.status_line_position = ["right", "below", "off"][index].into();
        print_status_preview(config);
    }
    Ok(())
}

fn choose_status_items(config: &mut Config) -> Result<()> {
    let supported = [
        "connection",
        "model",
        "reasoning",
        "mode",
        "backend",
        "scope",
        "task",
        "elapsed",
        "context",
        "last_tokens",
        "last_cost",
        "session_tokens",
        "session_cost",
        "requests",
    ];
    let default = config.aishe.status_line_items.join(",");
    let Some(value) = promptui::text("Ordered comma-separated status fields", &default, |value| {
        let items: Vec<&str> = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .collect();
        if items.is_empty() {
            anyhow::bail!("choose at least one field")
        }
        for item in items {
            if !supported.contains(&item) {
                anyhow::bail!("unsupported status field '{item}'")
            }
        }
        Ok(())
    })?
    else {
        return Ok(());
    };
    if value != ":back" {
        config.aishe.status_line_items = value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect();
        print_status_preview(config);
    }
    Ok(())
}

fn print_status_preview(config: &Config) {
    let values: Vec<&str> = config
        .aishe
        .status_line_items
        .iter()
        .filter_map(|item| match item.as_str() {
            "connection" => Some(config.active_connection_id()),
            "model" => Some(config.active_model()),
            "reasoning" => Some(config.active_reasoning_effort()),
            "mode" => Some(config.aishe.mode.as_str()),
            "backend" => Some(config.backend.engine.as_str()),
            "scope" => Some(config.backend.default_scope.as_str()),
            "task" => Some("task repo-audit"),
            "elapsed" => Some("last 4.2s"),
            "context" => Some("context 8.4K tok"),
            "last_tokens" => Some("last 1,697/374 tok"),
            "last_cost" => Some("last cost n/a"),
            "session_tokens" => Some("session 1,697/374 tok"),
            "session_cost" => Some("session cost n/a"),
            "requests" => Some("2 reqs"),
            _ => None,
        })
        .collect();
    println!(
        "  preview ({}): {}",
        config.aishe.status_line_position,
        values.join(" · ")
    );
}

fn safety_section(config: &mut Config) -> Result<()> {
    let before = config.clone();
    let choices = vec![
        "Conservative — suggest, confirm all".into(),
        "Balanced — auto safe, confirm writes".into(),
        "Autonomous — yolo with readiness checks".into(),
        "Custom — keep individual controls".into(),
        "Back".into(),
    ];
    match promptui::menu(
        "Mode & safety",
        &choices,
        profile_index(&config.aishe.safety_profile),
        true,
        "Profiles display and apply a documented bundle; budget is never changed.",
    )? {
        MenuResult::Selected(index @ 0..=3) => {
            let profile = [
                Profile::Conservative,
                Profile::Balanced,
                Profile::Autonomous,
                Profile::Custom,
            ][index];
            let changes = profiles::apply(config, profile);
            for change in changes {
                println!("  {}: {} → {}", change.field, change.before, change.after);
            }
        }
        MenuResult::Cancel => *config = before,
        _ => {}
    }
    Ok(())
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
    let model = config.active_model().to_string();
    let current = usage::price_for(&model, &config.pricing);
    let choices = vec![
        format!(
            "Model price: {}",
            current
                .map(|price| format!("${}/${} per 1M", price.input, price.output))
                .unwrap_or_else(|| "unknown".into())
        ),
        format!(
            "Session budget: {}",
            if config.aishe.budget_usd > 0.0 {
                format!("${:.2}", config.aishe.budget_usd)
            } else {
                "unlimited".into()
            }
        ),
        format!("Per-call usage output: {}", on_off(config.aishe.show_usage)),
        format!("Audit logging: {}", on_off(config.logging.enabled)),
        format!("Audit redaction: {}", on_off(config.logging.redact)),
        "Back".into(),
    ];
    match promptui::menu(
        "Cost & logging",
        &choices,
        0,
        true,
        "Pricing is exact-model only. Unknown is safer than an inferred rate.",
    )? {
        MenuResult::Selected(0) => {
            let input_default = current.map(|price| price.input).unwrap_or(0.0).to_string();
            let output_default = current.map(|price| price.output).unwrap_or(0.0).to_string();
            let Some(input) =
                promptui::text("Input USD per 1M", &input_default, validate_rate_text)?
            else {
                return Ok(());
            };
            let Some(output) =
                promptui::text("Output USD per 1M", &output_default, validate_rate_text)?
            else {
                return Ok(());
            };
            if input != ":back" && output != ":back" {
                config.pricing.insert(
                    model,
                    Price {
                        input: input.parse()?,
                        output: output.parse()?,
                    },
                );
            }
        }
        MenuResult::Selected(1) => {
            let default = config.aishe.budget_usd.to_string();
            if let Some(value) = promptui::text(
                "Session budget USD (0 = unlimited)",
                &default,
                validate_rate_text,
            )? {
                if value != ":back" {
                    config.aishe.budget_usd = value.parse()?;
                }
            }
        }
        MenuResult::Selected(2) => config.aishe.show_usage = !config.aishe.show_usage,
        MenuResult::Selected(3) => config.logging.enabled = !config.logging.enabled,
        MenuResult::Selected(4) => config.logging.redact = !config.logging.redact,
        _ => {}
    }
    Ok(())
}

fn advanced_section(config: &mut Config) -> Result<()> {
    let choices = vec![
        format!("Reasoning effort: {}", config.aishe.reasoning_effort),
        format!("Structured output: {}", config.aishe.structured),
        format!("Streaming: {}", on_off(config.aishe.stream)),
        format!("Response cache: {}", on_off(config.aishe.cache)),
        "Reset this section to defaults".into(),
        "Back".into(),
    ];
    match promptui::menu(
        "Advanced",
        &choices,
        0,
        true,
        "Auto reasoning follows provider defaults; GPT-5.6 tools use Responses.",
    )? {
        MenuResult::Selected(0) => {
            let options: Vec<String> = ["auto", "none", "low", "medium", "high", "xhigh", "max"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect();
            if let MenuResult::Selected(index) = promptui::menu(
                "Reasoning effort",
                &options,
                options
                    .iter()
                    .position(|value| value == &config.aishe.reasoning_effort)
                    .unwrap_or(0),
                true,
                "Auto omits an explicit effort unless compatibility requires none.",
            )? {
                config.aishe.reasoning_effort = options[index].clone();
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
        _ => {}
    }
    Ok(())
}

fn reset_shell_section(config: &mut Config) {
    let defaults = Config::default();
    config.aishe.share_history = defaults.aishe.share_history;
    config.aishe.pty_prompt = defaults.aishe.pty_prompt;
    config.aishe.hook_timeout_secs = defaults.aishe.hook_timeout_secs;
    config.aishe.failure_hints = defaults.aishe.failure_hints;
    config.aishe.status_line = defaults.aishe.status_line;
    config.aishe.status_line_position = defaults.aishe.status_line_position;
    config.aishe.status_line_items = defaults.aishe.status_line_items;
    config.backend.output = defaults.backend.output;
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
    config.aishe.reasoning_effort = defaults.aishe.reasoning_effort;
    config.aishe.structured = defaults.aishe.structured;
    config.aishe.stream = defaults.aishe.stream;
    config.aishe.cache = defaults.aishe.cache;
}

fn active_provider(config: &Config) -> &crate::config::ProviderConfig {
    config.active_provider_config()
}

fn active_provider_mut(config: &mut Config) -> &mut crate::config::ProviderConfig {
    let id = config.active_connection_id().to_string();
    if config.connections.contains_key(&id) {
        &mut config.connections.get_mut(&id).expect("checked").settings
    } else if config.aishe.provider == "openai" {
        &mut config.providers.openai
    } else {
        &mut config.providers.anthropic
    }
}

fn service_index(config: &Config) -> usize {
    provider_catalog::SERVICES
        .iter()
        .position(|service| {
            service.family
                == if config.aishe.provider == "anthropic" {
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
    if config
        .aishe
        .context_exclude
        .iter()
        .any(|item| item == section)
    {
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
    println!(
        "\n  {} · {} · {}",
        crate::commands::display_safe(&report.provider),
        crate::commands::display_safe(&report.model),
        crate::commands::display_safe(&report.transport)
    );
    for (name, check) in [
        ("credential", &report.credential),
        ("reachability", &report.reachability),
        ("model", &report.model_available),
        ("text", &report.text),
        ("structured", &report.structured),
        ("tools", &report.tools),
        ("streaming", &report.streaming),
    ] {
        println!(
            "  {:?} {name}: {}",
            check.state,
            crate::commands::display_safe(&check.detail)
        );
    }
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
    fn section_resets_restore_only_the_selected_section() {
        let defaults = Config::default();
        let mut config = Config::default();
        config.aishe.provider = "openai".into();
        config.aishe.mode = "yolo".into();
        config.aishe.budget_usd = 42.0;

        config.aishe.share_history = !defaults.aishe.share_history;
        config.aishe.pty_prompt = !defaults.aishe.pty_prompt;
        config.aishe.failure_hints = !defaults.aishe.failure_hints;
        config.aishe.hook_timeout_secs = 1;
        config.aishe.status_line = !defaults.aishe.status_line;
        config.aishe.status_line_position = "off".into();
        config.aishe.status_line_items = vec!["requests".into()];
        config.backend.output = "detailed".into();
        reset_shell_section(&mut config);
        assert_eq!(config.aishe.share_history, defaults.aishe.share_history);
        assert_eq!(config.aishe.pty_prompt, defaults.aishe.pty_prompt);
        assert_eq!(config.aishe.failure_hints, defaults.aishe.failure_hints);
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
