//! Resumable setup state machine and terminal driver.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::capabilities::{self, Report, State};
use crate::config::Config;
use crate::profiles::{self, Profile};
use crate::promptui::{self, MenuResult};
use crate::provider_catalog::{self, Family};
use crate::usage::{self, Price};

const DRAFT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    Service,
    Endpoint,
    Credential,
    Model,
    Profile,
    Pricing,
    Status,
    Validation,
    Review,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptResult<T> {
    Value(T),
    Back,
    Cancel,
}

impl Step {
    fn next(self) -> Self {
        match self {
            Self::Service => Self::Endpoint,
            Self::Endpoint => Self::Credential,
            Self::Credential => Self::Model,
            Self::Model => Self::Profile,
            Self::Profile => Self::Pricing,
            Self::Pricing => Self::Status,
            Self::Status => Self::Validation,
            Self::Validation => Self::Review,
            Self::Review => Self::Review,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Service => Self::Service,
            Self::Endpoint => Self::Service,
            Self::Credential => Self::Endpoint,
            Self::Model => Self::Credential,
            Self::Profile => Self::Model,
            Self::Pricing => Self::Profile,
            Self::Status => Self::Pricing,
            Self::Validation => Self::Status,
            Self::Review => Self::Validation,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Draft {
    schema_version: u32,
    step: Step,
    service: String,
    config: Config,
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    pub resume: bool,
    pub restart: bool,
    pub verify_only: bool,
    pub non_interactive: bool,
    pub service: Option<String>,
    pub base_url: Option<String>,
    pub key_env: Option<String>,
    pub model: Option<String>,
    pub transport: Option<String>,
    pub profile: Option<Profile>,
    pub input_price: Option<f64>,
    pub output_price: Option<f64>,
    pub live: bool,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub applied: bool,
    pub config_path: PathBuf,
    pub backup: Option<PathBuf>,
    pub report: Option<Report>,
}

pub fn run(options: Options) -> Result<Outcome> {
    if options.verify_only {
        let config = Config::load_quiet()?.context("no config exists; run `aishe setup` first")?;
        let report = capabilities::validate(&config, options.live);
        print_report(&report);
        return Ok(Outcome {
            applied: false,
            config_path: Config::path(),
            backup: None,
            report: Some(report),
        });
    }
    if options.non_interactive {
        return run_non_interactive(options);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("setup needs an interactive terminal; use `aishe setup --non-interactive`");
    }
    run_interactive(options)
}

fn run_non_interactive(options: Options) -> Result<Outcome> {
    let existing = Config::load_quiet()?;
    let mut config = existing.clone().unwrap_or_default();
    if let Some(service_key) = options.service.as_deref() {
        let service = provider_catalog::find(service_key)
            .with_context(|| format!("unknown service '{service_key}'"))?;
        apply_service(&mut config, service);
    } else if existing.is_none() {
        anyhow::bail!("fresh non-interactive setup requires --service");
    }
    apply_overrides(&mut config, &options)?;
    validate_config(&config)?;
    let report = capabilities::validate(&config, options.live);
    let backup = save_applied(&config)?;
    discard_draft().ok();
    print_report(&report);
    println!("Saved config to {}", Config::path().display());
    Ok(Outcome {
        applied: true,
        config_path: Config::path(),
        backup,
        report: Some(report),
    })
}

fn run_interactive(options: Options) -> Result<Outcome> {
    println!("\n  aishe setup\n  ───────────");
    println!("  Configure, verify, and safely apply your Aishe environment.");
    println!("  Active config is not changed until the final Apply step.");

    if options.restart {
        discard_draft()?;
    }
    let baseline = Config::load_quiet()?.unwrap_or_default();
    let mut draft = if options.resume {
        load_draft()?.context("no resumable setup draft exists")?
    } else if let Some(saved) = load_draft()? {
        let choices = vec![
            "Resume saved setup".to_string(),
            "Start over (discard draft only)".to_string(),
            "Exit without changes".to_string(),
        ];
        match promptui::menu(
            "An unfinished setup was found",
            &choices,
            0,
            false,
            "The draft contains configuration choices but never credentials.",
        )? {
            MenuResult::Selected(0) => saved,
            MenuResult::Selected(1) => {
                discard_draft()?;
                fresh_draft(baseline.clone())
            }
            _ => {
                return Ok(Outcome {
                    applied: false,
                    config_path: Config::path(),
                    backup: None,
                    report: None,
                })
            }
        }
    } else {
        fresh_draft(baseline.clone())
    };

    let mut report = None;
    loop {
        match draft.step {
            Step::Service => {
                let labels: Vec<String> = provider_catalog::SERVICES
                    .iter()
                    .map(|service| format!("{} — {}", service.label, service.help))
                    .collect();
                let default = provider_catalog::SERVICES
                    .iter()
                    .position(|service| service.key == draft.service)
                    .unwrap_or(1);
                match promptui::menu(
                    "Provider service",
                    &labels,
                    default,
                    false,
                    "Choose the service that owns the endpoint. Custom keeps every field editable.",
                )? {
                    MenuResult::Selected(index) => {
                        let service = &provider_catalog::SERVICES[index];
                        apply_service(&mut draft.config, service);
                        draft.service = service.key.to_string();
                        advance(&mut draft)?;
                    }
                    _ => return cancel(draft),
                }
            }
            Step::Endpoint => {
                let provider = active_provider_mut(&mut draft.config);
                match promptui::text("API endpoint", &provider.base_url, validate_url)? {
                    Some(value) if value == ":back" => draft.step = draft.step.previous(),
                    Some(value) => {
                        provider.base_url = provider_catalog::normalize_base_url(&value);
                        if provider.auth_required.is_none() {
                            provider.auth_required =
                                Some(!crate::config::is_loopback_url(&provider.base_url));
                        }
                        advance(&mut draft)?;
                    }
                    None => return cancel(draft),
                }
            }
            Step::Credential => {
                let provider = active_provider_mut(&mut draft.config);
                if !provider.requires_auth() {
                    println!(
                        "\n  Credential: not required for local endpoint {}",
                        provider.base_url
                    );
                    advance(&mut draft)?;
                    continue;
                }
                match promptui::text(
                    "Environment variable containing the API key",
                    &provider.api_key_env,
                    validate_env_name,
                )? {
                    Some(value) if value == ":back" => draft.step = draft.step.previous(),
                    Some(value) => {
                        provider.api_key_env = value;
                        advance(&mut draft)?;
                    }
                    None => return cancel(draft),
                }
            }
            Step::Model => {
                let provider_name = draft.config.aishe.provider.clone();
                let current = active_provider(&draft.config).model.clone();
                let models = capabilities::list_models(&draft.config, &provider_name).ok();
                if let Some(models) = models {
                    let mut options: Vec<String> =
                        std::iter::once(current.clone()).chain(models).collect();
                    options.sort();
                    options.dedup();
                    options.truncate(30);
                    options.push("Enter a model manually…".into());
                    match promptui::menu(
                        "Model",
                        &options,
                        options.iter().position(|model| model == &current).unwrap_or(0),
                        true,
                        "The list comes from the configured endpoint; manual entry is always available.",
                    )? {
                        MenuResult::Selected(index) if index + 1 == options.len() => {
                            match prompt_manual_model(&mut draft)? {
                                PromptResult::Value(()) => advance(&mut draft)?,
                                PromptResult::Back => draft.step = draft.step.previous(),
                                PromptResult::Cancel => return cancel(draft),
                            }
                        }
                        MenuResult::Selected(index) => {
                            active_provider_mut(&mut draft.config).model = options[index].clone();
                            advance(&mut draft)?;
                        }
                        MenuResult::Back => draft.step = draft.step.previous(),
                        MenuResult::Cancel => return cancel(draft),
                    }
                } else {
                    match prompt_manual_model(&mut draft)? {
                        PromptResult::Value(()) => advance(&mut draft)?,
                        PromptResult::Back => draft.step = draft.step.previous(),
                        PromptResult::Cancel => return cancel(draft),
                    }
                }
            }
            Step::Profile => {
                let choices = vec![
                    "Conservative — suggest, confirm all tool commands".into(),
                    "Balanced — auto safe commands, confirm writes".into(),
                    "Autonomous — yolo with readiness checks".into(),
                    "Custom — preserve individual controls".into(),
                ];
                match promptui::menu(
                    "Safety profile",
                    &choices,
                    profile_index(&draft.config.aishe.safety_profile),
                    true,
                    "Profiles are transparent setting bundles; Settings shows every value.",
                )? {
                    MenuResult::Selected(index) => {
                        let profile = [
                            Profile::Conservative,
                            Profile::Balanced,
                            Profile::Autonomous,
                            Profile::Custom,
                        ][index];
                        let changes = profiles::apply(&mut draft.config, profile);
                        println!("  {} safety setting(s) changed", changes.len());
                        advance(&mut draft)?;
                    }
                    MenuResult::Back => draft.step = draft.step.previous(),
                    MenuResult::Cancel => return cancel(draft),
                }
            }
            Step::Pricing => {
                let model = draft.config.active_model().to_string();
                if usage::price_for(&model, &draft.config.pricing).is_some() {
                    println!(
                        "\n  Pricing: configured for {}",
                        crate::commands::display_safe(&model)
                    );
                    advance(&mut draft)?;
                    continue;
                }
                let choices = vec![
                    "Enter input/output rates (USD per 1M tokens)".into(),
                    "Leave price unknown".into(),
                    "Return to model selection".into(),
                ];
                match promptui::menu(
                    &format!("No price is known for {model}"),
                    &choices,
                    0,
                    true,
                    "Aishe will never invent a rate; unknown pricing disables cost budgets.",
                )? {
                    MenuResult::Selected(0) => {
                        let input = match prompt_rate("Input price")? {
                            PromptResult::Value(value) => value,
                            PromptResult::Back => continue,
                            PromptResult::Cancel => return cancel(draft),
                        };
                        let output = match prompt_rate("Output price")? {
                            PromptResult::Value(value) => value,
                            PromptResult::Back => continue,
                            PromptResult::Cancel => return cancel(draft),
                        };
                        draft.config.pricing.insert(model, Price { input, output });
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(1) => advance(&mut draft)?,
                    MenuResult::Selected(2) | MenuResult::Back => draft.step = Step::Model,
                    MenuResult::Cancel => return cancel(draft),
                    MenuResult::Selected(_) => unreachable!(),
                }
            }
            Step::Status => {
                let positions = vec![
                    "Right prompt — compact and persistent".into(),
                    "Below — Codex-style secondary prompt line".into(),
                    "Off — keep only per-call/exit summaries".into(),
                ];
                let position_default = match draft.config.aishe.status_line_position.as_str() {
                    "below" => 1,
                    "off" => 2,
                    _ => 0,
                };
                match promptui::menu(
                    "Live status-line placement",
                    &positions,
                    position_default,
                    true,
                    "Right is best for wide terminals; Below has room for more metrics.",
                )? {
                    MenuResult::Selected(2) => {
                        draft.config.aishe.status_line = false;
                        draft.config.aishe.status_line_position = "off".into();
                        println!("  preview: (status line off)");
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(position) => {
                        draft.config.aishe.status_line = true;
                        draft.config.aishe.status_line_position =
                            if position == 1 { "below" } else { "right" }.into();
                        if !choose_status_items(&mut draft)? {
                            continue;
                        }
                        print_status_preview(&draft.config);
                        advance(&mut draft)?;
                    }
                    MenuResult::Back => draft.step = draft.step.previous(),
                    MenuResult::Cancel => return cancel(draft),
                }
            }
            Step::Validation => {
                let Some(live) = promptui::confirm(
                    "Run live text/structured/tool/streaming checks? (may use tokens)",
                    true,
                )?
                else {
                    return cancel(draft);
                };
                let checked = capabilities::validate(&draft.config, live);
                print_report(&checked);
                report = Some(checked);
                advance(&mut draft)?;
            }
            Step::Review => {
                print_review(&baseline, &draft.config, report.as_ref())?;
                let Some(apply) = promptui::confirm("Apply this configuration", true)? else {
                    return cancel(draft);
                };
                if !apply {
                    draft.step = Step::Service;
                    save_draft(&draft)?;
                    continue;
                }
                validate_config(&draft.config)?;
                let backup = save_applied(&draft.config)?;
                discard_draft()?;
                println!("\n  Setup complete");
                println!("  config: {}", Config::path().display());
                if let Some(path) = &backup {
                    println!("  backup: {}", path.display());
                }
                if let Some(report) = &report {
                    println!(
                        "  provider: {}",
                        if report.verified() {
                            "verified"
                        } else {
                            "saved with warnings; run `aishe setup --verify --live`"
                        }
                    );
                }
                if promptui::confirm("Run the guided first-session tour now", true)?
                    .unwrap_or(false)
                {
                    crate::tour::run(crate::tour::Options::default())?;
                } else {
                    println!("  Next: run `aishe tour` when you are ready.");
                }
                return Ok(Outcome {
                    applied: true,
                    config_path: Config::path(),
                    backup,
                    report,
                });
            }
        }
    }
}

fn apply_service(config: &mut Config, service: &provider_catalog::Service) {
    match service.family {
        Family::Anthropic => {
            config.aishe.provider = "anthropic".into();
            provider_catalog::apply(service, &mut config.providers.anthropic);
        }
        Family::OpenAiCompatible => {
            config.aishe.provider = "openai".into();
            provider_catalog::apply(service, &mut config.providers.openai);
        }
    }
}

fn apply_overrides(config: &mut Config, options: &Options) -> Result<()> {
    let provider = active_provider_mut(config);
    if let Some(base_url) = &options.base_url {
        provider.base_url = provider_catalog::normalize_base_url(base_url);
    }
    if let Some(key_env) = &options.key_env {
        validate_env_name(key_env)?;
        provider.api_key_env = key_env.clone();
    }
    if let Some(model) = &options.model {
        if model.trim().is_empty() {
            anyhow::bail!("--model cannot be empty");
        }
        provider.model = model.trim().to_string();
    }
    if let Some(transport) = &options.transport {
        validate_transport(transport)?;
        provider.transport = transport.clone();
    }
    if let Some(profile) = options.profile {
        profiles::apply(config, profile);
    } else if config.aishe.safety_profile == "custom" && !Config::path().exists() {
        profiles::apply(config, Profile::Conservative);
    }
    match (options.input_price, options.output_price) {
        (Some(input), Some(output)) => {
            validate_rate(input)?;
            validate_rate(output)?;
            config
                .pricing
                .insert(config.active_model().to_string(), Price { input, output });
        }
        (None, None) => {}
        _ => anyhow::bail!("--input-price and --output-price must be provided together"),
    }
    Ok(())
}

fn active_provider(config: &Config) -> &crate::config::ProviderConfig {
    if config.aishe.provider == "openai" {
        &config.providers.openai
    } else {
        &config.providers.anthropic
    }
}

fn active_provider_mut(config: &mut Config) -> &mut crate::config::ProviderConfig {
    if config.aishe.provider == "openai" {
        &mut config.providers.openai
    } else {
        &mut config.providers.anthropic
    }
}

fn fresh_draft(config: Config) -> Draft {
    let service = if config.aishe.provider == "anthropic" {
        "anthropic"
    } else if config.providers.openai.base_url.trim_end_matches('/') == "https://api.openai.com" {
        "openai"
    } else {
        "custom"
    };
    Draft {
        schema_version: DRAFT_SCHEMA_VERSION,
        step: Step::Service,
        service: service.into(),
        config,
    }
}

fn advance(draft: &mut Draft) -> Result<()> {
    draft.step = draft.step.next();
    save_draft(draft)
}

fn cancel(draft: Draft) -> Result<Outcome> {
    save_draft(&draft)?;
    println!(
        "\n  Setup paused. Resume with `aishe setup --resume`; active config was not changed."
    );
    Ok(Outcome {
        applied: false,
        config_path: Config::path(),
        backup: None,
        report: None,
    })
}

fn prompt_manual_model(draft: &mut Draft) -> Result<PromptResult<()>> {
    let current = active_provider(&draft.config).model.clone();
    match promptui::text("Model", &current, |value| {
        if value.trim().is_empty() {
            anyhow::bail!("model cannot be empty");
        }
        Ok(())
    })? {
        Some(value) if value == ":back" => Ok(PromptResult::Back),
        Some(value) => {
            active_provider_mut(&mut draft.config).model = value;
            Ok(PromptResult::Value(()))
        }
        None => Ok(PromptResult::Cancel),
    }
}

fn prompt_rate(label: &str) -> Result<PromptResult<f64>> {
    let Some(value) = promptui::text(label, "0", |value| {
        let rate: f64 = value.parse().context("enter a number such as 0.25")?;
        validate_rate(rate)
    })?
    else {
        return Ok(PromptResult::Cancel);
    };
    if value == ":back" {
        return Ok(PromptResult::Back);
    }
    Ok(PromptResult::Value(value.parse()?))
}

fn choose_status_items(draft: &mut Draft) -> Result<bool> {
    let choices = vec![
        "Compact — model, mode, session cost, requests".into(),
        "Detailed — model, mode, last/session tokens and costs, requests".into(),
        "Identity — model and mode only".into(),
        "Custom ordered fields…".into(),
    ];
    match promptui::menu(
        "Status-line contents",
        &choices,
        0,
        true,
        "Fields: model, mode, last_tokens, last_cost, session_tokens, session_cost, requests.",
    )? {
        MenuResult::Selected(0) => {
            draft.config.aishe.status_line_items = ["model", "mode", "session_cost", "requests"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }
        MenuResult::Selected(1) => {
            draft.config.aishe.status_line_items = [
                "model",
                "mode",
                "last_tokens",
                "last_cost",
                "session_tokens",
                "session_cost",
                "requests",
            ]
            .into_iter()
            .map(str::to_string)
            .collect();
        }
        MenuResult::Selected(2) => {
            draft.config.aishe.status_line_items =
                ["model", "mode"].into_iter().map(str::to_string).collect();
        }
        MenuResult::Selected(3) => {
            let default = draft.config.aishe.status_line_items.join(",");
            let Some(value) = promptui::text("Comma-separated fields", &default, |value| {
                validate_status_items(
                    &value
                        .split(',')
                        .map(|item| item.trim().to_string())
                        .collect::<Vec<_>>(),
                )
            })?
            else {
                return Ok(false);
            };
            if value == ":back" {
                return Ok(false);
            }
            draft.config.aishe.status_line_items = value
                .split(',')
                .map(|item| item.trim().to_string())
                .collect();
        }
        MenuResult::Back | MenuResult::Cancel => return Ok(false),
        MenuResult::Selected(_) => unreachable!(),
    }
    Ok(true)
}

fn validate_status_items(items: &[String]) -> Result<()> {
    const ALLOWED: &[&str] = &[
        "model",
        "mode",
        "last_tokens",
        "last_cost",
        "session_tokens",
        "session_cost",
        "requests",
    ];
    if items.is_empty() || items.iter().all(|item| item.is_empty()) {
        anyhow::bail!("choose at least one status field");
    }
    if let Some(item) = items.iter().find(|item| !ALLOWED.contains(&item.as_str())) {
        anyhow::bail!("unknown status field '{item}'");
    }
    Ok(())
}

fn print_status_preview(config: &Config) {
    let sample = config
        .aishe
        .status_line_items
        .iter()
        .map(|item| match item.as_str() {
            "model" => config.active_model().to_string(),
            "mode" => config.aishe.mode.clone(),
            "last_tokens" => "last 1,697/374 tok".into(),
            "last_cost" => "last ~$0.0012".into(),
            "session_tokens" => "session 8,421/1,904 tok".into(),
            "session_cost" => "session ~$0.0174".into(),
            "requests" => "6 reqs".into(),
            _ => String::new(),
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
    println!(
        "  preview ({}): {sample}",
        config.aishe.status_line_position
    );
}

fn validate_rate(value: f64) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        anyhow::bail!("price must be a finite non-negative number");
    }
    Ok(())
}

fn validate_url(value: &str) -> Result<()> {
    let normalized = provider_catalog::normalize_base_url(value);
    if !(normalized.starts_with("http://") || normalized.starts_with("https://")) {
        anyhow::bail!("endpoint must use http:// or https://");
    }
    Ok(())
}

fn validate_env_name(value: &str) -> Result<()> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        anyhow::bail!("environment variable name cannot be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic())
        || chars.any(|character| !(character == '_' || character.is_ascii_alphanumeric()))
    {
        anyhow::bail!("use a shell variable name such as OPENAI_API_KEY");
    }
    Ok(())
}

fn validate_transport(value: &str) -> Result<()> {
    if matches!(value, "auto" | "responses" | "chat") {
        Ok(())
    } else {
        anyhow::bail!("transport must be auto, responses, or chat")
    }
}

pub(crate) fn validate_config(config: &Config) -> Result<()> {
    if !matches!(config.aishe.provider.as_str(), "anthropic" | "openai") {
        anyhow::bail!("provider must be anthropic or openai");
    }
    let provider = active_provider(config);
    validate_url(&provider.base_url)?;
    validate_env_name(&provider.api_key_env)?;
    if provider.model.trim().is_empty() {
        anyhow::bail!("model cannot be empty");
    }
    validate_transport(&provider.transport)?;
    if config.aishe.status_line
        && !matches!(
            config.aishe.status_line_position.as_str(),
            "right" | "below"
        )
    {
        anyhow::bail!("status_line_position must be right, below, or off");
    }
    validate_status_items(&config.aishe.status_line_items)?;
    Ok(())
}

fn profile_index(value: &str) -> usize {
    match Profile::parse(value) {
        Some(Profile::Conservative) => 0,
        Some(Profile::Balanced) => 1,
        Some(Profile::Autonomous) => 2,
        _ => 3,
    }
}

fn print_report(report: &Report) {
    println!(
        "\n  Provider check: {} · {} · {}",
        report.provider, report.model, report.transport
    );
    for (label, check) in [
        ("credential", &report.credential),
        ("reachability", &report.reachability),
        ("model list", &report.model_list),
        ("model", &report.model_available),
        ("text", &report.text),
        ("structured", &report.structured),
        ("tools", &report.tools),
        ("streaming", &report.streaming),
    ] {
        let marker = match check.state {
            State::Pass => "✓",
            State::Warn => "!",
            State::Fail => "✗",
            State::Skipped => "·",
        };
        println!(
            "    {marker} {label}: {}",
            crate::commands::display_safe(&check.detail)
        );
    }
}

fn print_review(baseline: &Config, configured: &Config, report: Option<&Report>) -> Result<()> {
    let before = toml::to_string_pretty(baseline)?;
    let after = toml::to_string_pretty(configured)?;
    println!("\n  Review");
    println!(
        "    provider: {}",
        crate::commands::display_safe(&configured.aishe.provider)
    );
    println!(
        "    endpoint: {}",
        crate::commands::display_safe(&active_provider(configured).base_url)
    );
    println!(
        "    model: {}",
        crate::commands::display_safe(configured.active_model())
    );
    println!(
        "    API key: ${} ({})",
        crate::commands::display_safe(&active_provider(configured).api_key_env),
        if active_provider(configured).requires_auth() {
            "required"
        } else {
            "not required"
        }
    );
    println!(
        "    profile: {}",
        crate::commands::display_safe(&configured.aishe.safety_profile)
    );
    println!(
        "    config: {}",
        crate::commands::display_safe(&Config::path().display().to_string())
    );
    if let Some(report) = report {
        println!(
            "    validation: {}",
            if report.verified() {
                "verified"
            } else {
                "warnings remain"
            }
        );
    }
    let diff = crate::undo::unified_diff(&before, &after);
    if diff.is_empty() {
        println!("\n  No configuration changes.");
    } else {
        println!("\n{diff}");
    }
    Ok(())
}

pub(crate) fn save_applied(config: &Config) -> Result<Option<PathBuf>> {
    let path = Config::path();
    let backup = if path.exists() {
        Some(create_setup_backup(&path)?)
    } else {
        None
    };
    config.save()?;
    let reloaded = Config::load_quiet()?.context("saved config could not be reloaded")?;
    validate_config(&reloaded)?;
    Ok(backup)
}

fn create_setup_backup(source: &Path) -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    for suffix in 0u32.. {
        let extension = if suffix == 0 {
            format!("toml.setup.{stamp}.bak")
        } else {
            format!("toml.setup.{stamp}.{suffix}.bak")
        };
        let backup = source.with_extension(extension);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut destination = match options.open(&backup) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("creating config backup {}", backup.display()))
            }
        };
        let result = (|| -> Result<()> {
            let mut original = std::fs::File::open(source)
                .with_context(|| format!("opening config {}", source.display()))?;
            std::io::copy(&mut original, &mut destination)
                .with_context(|| format!("backing up config to {}", backup.display()))?;
            destination
                .sync_all()
                .with_context(|| format!("syncing config backup {}", backup.display()))?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&backup);
            return Err(error);
        }
        set_private_permissions(&backup);
        return Ok(backup);
    }
    unreachable!("u32 backup suffix space exhausted")
}

fn draft_path() -> Option<PathBuf> {
    crate::config::data_root().map(|root| root.join("aishe").join("setup-draft.json"))
}

fn save_draft(draft: &Draft) -> Result<()> {
    let path = draft_path().context("data directory is unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_private_directory(parent);
    }
    let bytes = serde_json::to_vec_pretty(draft)?;
    crate::config::write_atomic(&path, &bytes)?;
    set_private_permissions(&path);
    Ok(())
}

fn load_draft() -> Result<Option<Draft>> {
    let Some(path) = draft_path() else {
        return Ok(None);
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let draft: Draft = serde_json::from_slice(&bytes)
        .with_context(|| format!("reading setup draft {}", path.display()))?;
    if draft.schema_version != DRAFT_SCHEMA_VERSION {
        anyhow::bail!(
            "setup draft schema {} is unsupported; use `aishe setup --restart`",
            draft.schema_version
        );
    }
    Ok(Some(draft))
}

fn discard_draft() -> Result<()> {
    let Some(path) = draft_path() else {
        return Ok(());
    };
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

#[cfg(unix)]
fn set_private_directory(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_state_machine_moves_forward_and_back() {
        assert_eq!(Step::Service.next(), Step::Endpoint);
        assert_eq!(Step::Review.next(), Step::Review);
        assert_eq!(Step::Model.previous(), Step::Credential);
        assert_eq!(Step::Service.previous(), Step::Service);
    }

    #[test]
    fn noninteractive_options_apply_service_profile_and_price() {
        let mut config = Config::default();
        let options = Options {
            service: Some("ollama".into()),
            model: Some("qwen-test".into()),
            profile: Some(Profile::Balanced),
            input_price: Some(0.0),
            output_price: Some(0.0),
            ..Options::default()
        };
        let service = provider_catalog::find("ollama").unwrap();
        apply_service(&mut config, service);
        apply_overrides(&mut config, &options).unwrap();
        assert_eq!(config.aishe.provider, "openai");
        assert_eq!(config.active_model(), "qwen-test");
        assert!(!config.providers.openai.requires_auth());
        assert_eq!(config.aishe.safety_profile, "balanced");
        assert_eq!(config.pricing["qwen-test"].input, 0.0);
    }

    #[test]
    fn validation_rejects_bad_env_and_rates() {
        assert!(validate_env_name("OPENAI_API_KEY").is_ok());
        assert!(validate_env_name("bad-name").is_err());
        assert!(validate_rate(f64::NAN).is_err());
        assert!(validate_rate(-1.0).is_err());
    }

    #[test]
    fn draft_contains_no_environment_values() {
        let draft = fresh_draft(Config::default());
        let encoded = serde_json::to_string(&draft).unwrap();
        assert!(encoded.contains("api_key_env"));
        assert!(!encoded.contains("sk-"));
        assert_eq!(draft.config.version, crate::config::CONFIG_SCHEMA_VERSION);
    }
}
