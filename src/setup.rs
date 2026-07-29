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
    pub credential_profile: Option<String>,
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
    promptui::header(
        "aishe setup",
        "Configure, verify, and safely apply your Aishe environment.",
        "Active config is not changed until the final Apply step.",
    );

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

    // Secret material is intentionally process-local. A resumed draft that had
    // advanced past Credential must return there when no saved/environment
    // source now exists.
    let mut pending_credential: Option<(String, String)> = None;
    if matches!(
        draft.step,
        Step::Model
            | Step::Profile
            | Step::Pricing
            | Step::Status
            | Step::Validation
            | Step::Review
    ) && active_provider(&draft.config).requires_auth()
        && crate::credentials::resolve(active_provider(&draft.config))
            .map(|resolved| resolved.secret().is_none())
            .unwrap_or(true)
    {
        println!("  The resumed draft contains no credential; returning to the Credential step.");
        draft.step = Step::Credential;
        save_draft(&draft)?;
    }

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
                        pending_credential = None;
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
                let provider = active_provider(&draft.config).clone();
                if !provider.requires_auth() {
                    promptui::success(&format!(
                        "Credential not required for local endpoint {}",
                        provider.base_url
                    ));
                    pending_credential = None;
                    advance(&mut draft)?;
                    continue;
                }
                let profile = provider.credential_profile();
                let existing = crate::credentials::resolve(&provider);
                if let Err(error) = &existing {
                    println!(
                        "  ! Credential store unavailable: {}",
                        crate::commands::display_safe(&crate::redact::redact(&error.to_string()))
                    );
                    println!(
                        "    Repair it with `aishe doctor --fix`, then retry saved-key entry."
                    );
                }
                let mut choices = Vec::new();
                let existing_index = if let Ok(resolved) = &existing {
                    if resolved.secret().is_some() {
                        choices.push(format!(
                            "Use existing credential ({})",
                            resolved.source.label()
                        ));
                        Some(0)
                    } else {
                        None
                    }
                } else {
                    None
                };
                choices.push("Enter and save an API key locally (recommended)".into());
                choices.push(format!(
                    "Use environment variable only (${env})",
                    env = provider.api_key_env
                ));
                match promptui::menu(
                    &format!("Credential profile '{profile}'"),
                    &choices,
                    existing_index.unwrap_or(0),
                    true,
                    "Saved keys live in a private credentials file; environment variables remain available for automation and overrides.",
                )? {
                    MenuResult::Selected(index) if Some(index) == existing_index => {
                        pending_credential = None;
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(index)
                        if index == usize::from(existing_index.is_some()) =>
                    {
                        let Some(secret) = promptui::secret(
                            &format!("API key for '{profile}'"),
                            crate::credentials::MAX_SECRET_BYTES,
                        )?
                        else {
                            return cancel(draft);
                        };
                        crate::credentials::validate_secret(&secret)?;
                        pending_credential = Some((profile, secret));
                        advance(&mut draft)?;
                    }
                    MenuResult::Selected(_) => {
                        let provider = active_provider_mut(&mut draft.config);
                        match promptui::text(
                            "Environment override variable",
                            &provider.api_key_env,
                            validate_env_name,
                        )? {
                            Some(value) if value == ":back" => {}
                            Some(value) => {
                                provider.api_key_env = value;
                                pending_credential = None;
                                advance(&mut draft)?;
                            }
                            None => return cancel(draft),
                        }
                    }
                    MenuResult::Back => draft.step = draft.step.previous(),
                    MenuResult::Cancel => return cancel(draft),
                }
            }
            Step::Model => {
                let provider_name = draft.config.aishe.provider.clone();
                let current = active_provider(&draft.config).model.clone();
                let catalog = with_pending(&pending_credential, || {
                    capabilities::list_models(&draft.config, &provider_name)
                })?;
                if let Ok(models) = catalog {
                    let auth_required = active_provider(&draft.config).requires_auth();
                    promptui::success(&format!(
                        "{}; /v1/models returned {} model(s)",
                        if auth_required {
                            "Credential accepted"
                        } else {
                            "Endpoint reached"
                        },
                        models.len()
                    ));
                    const VISIBLE_MODEL_LIMIT: usize = 24;
                    let mut visible = Vec::new();
                    if models.iter().any(|model| model == &current) {
                        visible.push(current.clone());
                    }
                    visible.extend(
                        models
                            .iter()
                            .filter(|model| *model != &current)
                            .take(VISIBLE_MODEL_LIMIT.saturating_sub(visible.len()))
                            .cloned(),
                    );
                    let mut options = vec!["Type a model ID…".to_string()];
                    options.extend(visible.iter().cloned());
                    let default = visible
                        .iter()
                        .position(|model| model == &current)
                        .map(|position| position + 1)
                        .unwrap_or(0);
                    if models.len() > visible.len() {
                        promptui::warning(&format!(
                            "Showing {} of {} models; type any returned model ID to select it",
                            visible.len(),
                            models.len()
                        ));
                    }
                    match promptui::menu(
                        "Available models (refreshed from /v1/models)",
                        &options,
                        default,
                        true,
                        "Listed models are validated by exact catalog membership. A typed ID is checked against the full catalog, then with one minimal request only if it was not listed.",
                    )? {
                        MenuResult::Selected(0) => {
                            match prompt_manual_model(
                                &mut draft,
                                Some(&models),
                                &pending_credential,
                            )? {
                                PromptResult::Value(()) => advance(&mut draft)?,
                                PromptResult::Back => {}
                                PromptResult::Cancel => return cancel(draft),
                            }
                        }
                        MenuResult::Selected(index) => {
                            let model = visible[index - 1].clone();
                            active_provider_mut(&mut draft.config).model = model.clone();
                            promptui::success(&format!(
                                "Model '{model}' is present in the current endpoint catalog"
                            ));
                            advance(&mut draft)?;
                        }
                        MenuResult::Back => draft.step = draft.step.previous(),
                        MenuResult::Cancel => return cancel(draft),
                    }
                } else {
                    let error = catalog.unwrap_err();
                    let detail = crate::redact::redact(&error.to_string());
                    promptui::error(&format!(
                        "Could not load /v1/models ({:?}): {detail}",
                        error.kind()
                    ));
                    let credential_rejected = matches!(
                        error.kind(),
                        crate::providers::ErrorKind::MissingCredential
                            | crate::providers::ErrorKind::InvalidCredential
                    );
                    let choices = if credential_rejected {
                        vec![
                            "Back to credential".into(),
                            "Retry /v1/models".into(),
                            "Cancel setup".into(),
                        ]
                    } else {
                        vec![
                            "Retry /v1/models".into(),
                            "Type a model ID and validate with one minimal request".into(),
                            "Back to credential or endpoint".into(),
                            "Cancel setup".into(),
                        ]
                    };
                    match promptui::menu(
                        "Model discovery needs attention",
                        &choices,
                        0,
                        false,
                        "A successful catalog request validates the endpoint and credential without spending tokens. Manual fallback makes one small generation request.",
                    )? {
                        MenuResult::Selected(0) if credential_rejected => {
                            draft.step = draft.step.previous()
                        }
                        MenuResult::Selected(1) if credential_rejected => {}
                        MenuResult::Selected(0) => {}
                        MenuResult::Selected(1) => {
                            match prompt_manual_model(&mut draft, None, &pending_credential)? {
                                PromptResult::Value(()) => advance(&mut draft)?,
                                PromptResult::Back => {}
                                PromptResult::Cancel => return cancel(draft),
                            }
                        }
                        MenuResult::Selected(2) if !credential_rejected => {
                            draft.step = draft.step.previous()
                        }
                        MenuResult::Selected(_) | MenuResult::Cancel => return cancel(draft),
                        MenuResult::Back => unreachable!(),
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
                let checked = with_pending(&pending_credential, || {
                    capabilities::validate(&draft.config, live)
                })?;
                print_report(&checked);
                report = Some(checked);
                advance(&mut draft)?;
            }
            Step::Review => {
                print_review(
                    &baseline,
                    &draft.config,
                    report.as_ref(),
                    pending_credential.as_ref(),
                )?;
                let Some(apply) = promptui::confirm("Apply this configuration", true)? else {
                    return cancel(draft);
                };
                if !apply {
                    draft.step = Step::Service;
                    save_draft(&draft)?;
                    continue;
                }
                validate_config(&draft.config)?;
                let backup = save_with_pending(&draft.config, pending_credential.take())?;
                discard_draft()?;
                promptui::success("Setup complete");
                println!("  config: {}", Config::path().display());
                println!("  credentials: {}", crate::credentials::path().display());
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
    if let Some(profile) = &options.credential_profile {
        provider.credential = crate::credentials::normalize_profile(profile)?;
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

fn prompt_manual_model(
    draft: &mut Draft,
    catalog: Option<&[String]>,
    pending_credential: &Option<(String, String)>,
) -> Result<PromptResult<()>> {
    let mut current = active_provider(&draft.config).model.clone();
    loop {
        match promptui::text("Model ID", &current, |value| {
            if value.trim().is_empty() {
                anyhow::bail!("model cannot be empty");
            }
            if value.len() > 256 || value.chars().any(char::is_control) {
                anyhow::bail!("model must be at most 256 characters with no control characters");
            }
            Ok(())
        })? {
            Some(value) if value == ":back" => return Ok(PromptResult::Back),
            Some(value) => {
                let model = value.trim().to_string();
                if catalog.is_some_and(|models| models.iter().any(|item| item == &model)) {
                    active_provider_mut(&mut draft.config).model = model.clone();
                    promptui::success(&format!(
                        "Model '{model}' is present in the current endpoint catalog"
                    ));
                    return Ok(PromptResult::Value(()));
                }

                if catalog.is_some() {
                    let Some(validate) = promptui::confirm(
                        "Model was not listed. Validate it with one minimal request? (may use tokens)",
                        true,
                    )?
                    else {
                        return Ok(PromptResult::Cancel);
                    };
                    if !validate {
                        current = model;
                        continue;
                    }
                }
                promptui::warning(&format!(
                    "Model '{model}' was not returned by /v1/models; making one minimal request to validate the exact ID"
                ));
                let mut candidate = draft.config.clone();
                active_provider_mut(&mut candidate).model = model.clone();
                let checked = with_pending(pending_credential, || {
                    capabilities::validate_model_request(&candidate)
                })?;
                match checked {
                    Ok(()) => {
                        active_provider_mut(&mut draft.config).model = model.clone();
                        promptui::success(&format!(
                            "Model '{model}' accepted a generation request"
                        ));
                        return Ok(PromptResult::Value(()));
                    }
                    Err(error) => {
                        promptui::error(&format!(
                            "Model validation failed ({:?}): {}",
                            error.kind(),
                            crate::redact::redact(&error.to_string())
                        ));
                        current = model;
                    }
                }
            }
            None => return Ok(PromptResult::Cancel),
        }
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
        "Compact — model, mode, scope, session cost, requests".into(),
        "Detailed — model, mode, scope, last/session tokens and costs, requests".into(),
        "Identity — model, mode, and scope only".into(),
        "Custom ordered fields…".into(),
    ];
    match promptui::menu(
        "Status-line contents",
        &choices,
        0,
        true,
        "Fields: model, mode, scope, last_tokens, last_cost, session_tokens, session_cost, requests.",
    )? {
        MenuResult::Selected(0) => {
            draft.config.aishe.status_line_items =
                ["model", "mode", "scope", "session_cost", "requests"]
                .into_iter()
                .map(str::to_string)
                .collect();
        }
        MenuResult::Selected(1) => {
            draft.config.aishe.status_line_items = [
                "model",
                "mode",
                "scope",
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
                ["model", "mode", "scope"]
                    .into_iter()
                    .map(str::to_string)
                    .collect();
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
        "scope",
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
            "scope" => config.backend.default_scope.clone(),
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
    crate::credentials::normalize_profile(&provider.credential_profile())?;
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
    if !(1..=600).contains(&config.aishe.hook_timeout_secs) {
        anyhow::bail!("hook_timeout_secs must be between 1 and 600");
    }
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
    promptui::section(&format!(
        "Provider check: {} · {} · {}",
        report.provider, report.model, report.transport
    ));
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
        let detail = format!("{label}: {}", crate::commands::display_safe(&check.detail));
        match check.state {
            State::Pass => promptui::success(&detail),
            State::Warn => promptui::warning(&detail),
            State::Fail => promptui::error(&detail),
            State::Skipped => println!("  · {detail}"),
        }
    }
}

fn print_review(
    baseline: &Config,
    configured: &Config,
    report: Option<&Report>,
    pending_credential: Option<&(String, String)>,
) -> Result<()> {
    let before = toml::to_string_pretty(baseline)?;
    let after = toml::to_string_pretty(configured)?;
    promptui::section("Review");
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
        "    credential: {} ({})",
        crate::commands::display_safe(&active_provider(configured).credential_profile()),
        if pending_credential.is_some() {
            "will save locally on Apply".to_string()
        } else if active_provider(configured).requires_auth() {
            crate::credentials::resolve(active_provider(configured))
                .map(|resolved| resolved.source.label())
                .unwrap_or_else(|_| "unavailable".into())
        } else {
            "not required".to_string()
        }
    );
    println!(
        "    environment override: ${}",
        crate::commands::display_safe(&active_provider(configured).api_key_env)
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

fn with_pending<T>(pending: &Option<(String, String)>, operation: impl FnOnce() -> T) -> Result<T> {
    match pending {
        Some((profile, secret)) => {
            crate::credentials::with_staged(profile, secret.clone(), operation)
        }
        None => Ok(operation()),
    }
}

fn save_with_pending(
    config: &Config,
    pending: Option<(String, String)>,
) -> Result<Option<PathBuf>> {
    let Some((profile, secret)) = pending else {
        return save_applied(config);
    };
    let previous = crate::credentials::Store::load()?;
    let mut updated = previous.clone().unwrap_or_default();
    updated.set(&profile, secret)?;
    updated.save()?;
    match save_applied(config) {
        Ok(backup) => Ok(backup),
        Err(error) => {
            let rollback = match previous {
                Some(store) => store.save(),
                None => match std::fs::remove_file(crate::credentials::path()) {
                    Ok(()) => Ok(()),
                    Err(remove_error) if remove_error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(())
                    }
                    Err(remove_error) => Err(remove_error.into()),
                },
            };
            if let Err(rollback_error) = rollback {
                anyhow::bail!(
                    "{error}; additionally failed to roll back the credential write: \
                     {rollback_error}"
                );
            }
            Err(error)
        }
    }
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
