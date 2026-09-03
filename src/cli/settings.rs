//! Configuration, context, trust, profile, pricing, and undo commands.

use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::context;
use crate::executor::Executor;
use crate::ui::SemanticStylize;

/// Parsed price action transferred from the binary's Clap surface.
#[derive(Clone, Debug)]
pub enum PriceAction {
    List,
    Set {
        model: String,
        input: f64,
        output: f64,
    },
    Remove {
        model: String,
    },
}

pub fn models(config: &Config, provider: &str, json: bool) -> u8 {
    match crate::capabilities::list_models(config, provider) {
        Ok(models) => {
            if json {
                if let Err(error) = crate::cli::json_contract::print_envelope("models", &models) {
                    crate::cli::error_contract::emit_from(error.as_ref());
                    return 1;
                }
            } else {
                println!("{provider}: {} model(s):", models.len());
                for model in models {
                    let active = if config.resolve_connection_id(provider).ok().as_deref()
                        == Some(config.active_connection_id())
                        && model == config.active_model()
                    {
                        " (active)"
                    } else {
                        ""
                    };
                    println!("  {}{active}", crate::commands::display_safe(&model));
                }
            }
            0
        }
        Err(error) => {
            // The Debug-formatted kind leaked an internal enum name here.
            crate::cli::error_contract::emit_classified(
                crate::user_error::ErrorNamespace::Provider,
                "model_list_failed",
                format!(
                    "Could not list models for '{provider}': {}",
                    crate::redact::redact(&error.to_string())
                ),
                "Run `aishe doctor --probe`, then check the endpoint and credential.",
                None,
            );
            crate::user_error::ErrorNamespace::Provider.exit_code()
        }
    }
}

pub fn print_capability_report(report: &crate::capabilities::Report) {
    println!(
        "provider validation: {} · {} · {}",
        crate::commands::display_safe(&report.provider),
        crate::commands::display_safe(&report.model),
        crate::commands::display_safe(&report.transport)
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
            crate::capabilities::State::Pass => "✓",
            crate::capabilities::State::Warn => "!",
            crate::capabilities::State::Fail => "✗",
            crate::capabilities::State::Skipped => "·",
        };
        println!(
            "  {marker} {label}: {}",
            crate::commands::display_safe(&check.detail)
        );
    }
}

pub fn context(
    mut effective: Config,
    explain: bool,
    request: Option<&str>,
    json_output: bool,
    excludes: &[String],
    includes: &[String],
    show: bool,
) -> Result<u8> {
    const OPTIONAL: &[&str] = &[
        "history",
        "project_context",
        "project_tasks",
        "host_profile",
    ];
    for section in excludes.iter().chain(includes.iter()) {
        if !OPTIONAL.contains(&section.as_str()) {
            eprintln!(
                "aishe: unknown context section '{section}' (expected {})",
                OPTIONAL.join(", ")
            );
            return Ok(1);
        }
    }
    if let Some(section) = excludes
        .iter()
        .find(|section| includes.iter().any(|included| included == *section))
    {
        eprintln!("aishe: context section '{section}' cannot be included and excluded together");
        return Ok(1);
    }

    if !excludes.is_empty() || !includes.is_empty() {
        let mut persisted =
            Config::load_quiet()?.context("no config exists; run `aishe setup` first")?;
        for section in excludes {
            if !persisted
                .aishe
                .context_exclude
                .iter()
                .any(|item| item == section)
            {
                persisted.aishe.context_exclude.push(section.clone());
            }
        }
        for section in includes {
            persisted
                .aishe
                .context_exclude
                .retain(|item| item != section);
            match section.as_str() {
                "project_context" => persisted.aishe.project_context = true,
                "project_tasks" => persisted.aishe.project_tasks = true,
                "host_profile" => persisted.aishe.host_profile = true,
                _ => {}
            }
        }
        persisted.save()?;
        effective.aishe.context_exclude = persisted.aishe.context_exclude.clone();
        effective.aishe.project_context = persisted.aishe.project_context;
        effective.aishe.project_tasks = persisted.aishe.project_tasks;
        effective.aishe.host_profile = persisted.aishe.host_profile;
        if !json_output {
            for section in excludes {
                println!("context.{section} = excluded");
            }
            for section in includes {
                println!("context.{section} = included");
            }
        }
    }

    let mut executor = Executor::new()?;
    executor.set_history_log(crate::cli::history::history_paths(&effective).1);
    context::init(executor.shell());
    if show {
        let request = request.unwrap_or("");
        let expanded = crate::attachments::expand(request, executor.cwd(), &effective)?;
        println!("--- model-visible local context (redacted) ---");
        print!("{}", context::build(&executor, &effective));
        if !request.is_empty() {
            println!(
                "\nUser request: {}",
                crate::commands::display_safe_multiline(&expanded.prompt)
            );
        }
        println!("--- end model-visible local context ---");
        if !expanded.sources.is_empty() {
            eprintln!(
                "attachments: {} · {} bytes",
                expanded.sources.join(", "),
                expanded.bytes
            );
        }
        return Ok(0);
    }
    if !explain
        && request.is_none()
        && !json_output
        && excludes.is_empty()
        && includes.is_empty()
        && std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
    {
        return context_cockpit(effective, &executor);
    }
    if !explain && request.is_none() && !json_output && excludes.is_empty() && includes.is_empty() {
        print!("{}", context::build(&executor, &effective));
        return Ok(0);
    }
    let report = context::preview(&executor, &effective, request);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(0);
    }
    println!(
        "context preview: {} · {} · ~{} tokens{}",
        crate::commands::display_safe(&report.provider),
        crate::commands::display_safe(&report.model),
        report.total_estimated_tokens,
        report
            .estimated_input_cost_usd
            .map(|cost| format!(" · ~${cost:.6} input"))
            .unwrap_or_else(|| " · cost n/a".into())
    );
    for section in &report.sections {
        println!(
            "  {} {:16} ~{:5} tok · {} · {}{}",
            if section.included { "✓" } else { "–" },
            crate::commands::display_safe(&section.id),
            section.estimated_tokens,
            if section.required {
                "required"
            } else if section.included {
                "included"
            } else {
                "excluded"
            },
            crate::commands::display_safe(&section.source),
            if section.redactions > 0 {
                format!(" · {} redacted", section.redactions)
            } else {
                String::new()
            }
        );
    }
    if let Some(text) = request {
        println!(
            "  request: {} chars · ~{} tokens (text intentionally not echoed)",
            text.chars().count(),
            report.request_estimated_tokens
        );
    }
    Ok(0)
}

pub fn capabilities(config: &Config, json: bool) -> Result<u8> {
    let report = crate::capabilities::load(config);
    if json {
        crate::cli::json_contract::print_object(&serde_json::json!({
            "schema_version": 1,
            "available": report.is_some(),
            "report": report,
        }))?;
    } else if let Some(report) = report {
        print_capability_report(&report);
        println!(
            "  agent: {}",
            if report.live_verified() {
                "ready"
            } else {
                "needs `aishe test --live`"
            }
        );
    } else {
        println!("no capability evidence for the active connection/model");
        println!(
            "run `aishe test --live` to validate text, structured output, tools, and streaming"
        );
    }
    Ok(0)
}

pub fn self_test(config: &Config, live: bool, json: bool) -> Result<u8> {
    let started = std::time::Instant::now();
    let report = if live {
        crate::capabilities::validate(config, true)
    } else {
        crate::capabilities::load(config)
            .unwrap_or_else(|| crate::capabilities::validate(config, false))
    };
    let sandbox_backend = crate::sandbox::backend(config);
    let sandbox = format!("{sandbox_backend:?}").to_ascii_lowercase();
    let local_ok = config.aishe.redact_secrets;
    let passed = local_ok && (!live || report.verified());
    let elapsed_ms = started.elapsed().as_millis();
    if json {
        crate::cli::json_contract::print_object(&serde_json::json!({
            "schema_version": 1,
            "passed": passed,
            "live": live,
            "elapsed_ms": elapsed_ms,
            "local": {
                "config": "pass",
                "redaction": if config.aishe.redact_secrets { "pass" } else { "fail" },
                "statusline_position": if config.aishe.status_line { "right" } else { "off" },
                "sandbox": {
                    "backend": sandbox,
                    "enabled": !matches!(sandbox_backend, crate::sandbox::Backend::Off),
                },
                "unicode": format!("{:?}", crate::ui::TerminalCapabilities::detect_stdout().unicode).to_ascii_lowercase(),
            },
            "provider": report,
        }))?;
    } else {
        println!("AIShe self-test · {} ms", elapsed_ms);
        println!("  ✓ config parsed");
        println!(
            "  {} secret redaction",
            if config.aishe.redact_secrets {
                "✓"
            } else {
                "✗"
            }
        );
        println!(
            "  {} statusline",
            if config.aishe.status_line {
                "✓ right prompt"
            } else {
                "· off"
            }
        );
        println!(
            "  {} {sandbox} sandbox policy",
            if matches!(sandbox_backend, crate::sandbox::Backend::Off) {
                "·"
            } else {
                "✓"
            }
        );
        print_capability_report(&report);
        if !live {
            println!("  · cached/provider metadata only; add --live for paid end-to-end checks");
        }
    }
    Ok(if passed { 0 } else { 1 })
}

fn context_cockpit(mut config: Config, executor: &Executor) -> Result<u8> {
    loop {
        let report = context::preview(executor, &config, None);
        println!(
            "context · ~{} tokens · {} redaction{}",
            report.total_estimated_tokens,
            report.total_redactions,
            if report.total_redactions == 1 {
                ""
            } else {
                "s"
            }
        );
        let optional = report
            .sections
            .iter()
            .filter(|section| !section.required)
            .collect::<Vec<_>>();
        let mut options = optional
            .iter()
            .map(|section| {
                format!(
                    "{} {} · ~{} tok · {}",
                    if section.included { "[on] " } else { "[off]" },
                    section.id,
                    section.estimated_tokens,
                    section.source
                )
            })
            .collect::<Vec<_>>();
        options.push("Done".into());
        let crate::promptui::PickerResult::Use(index) =
            crate::promptui::filter_picker("Context cockpit", &options, options.len() - 1)?
        else {
            return Ok(0);
        };
        if index >= optional.len() {
            return Ok(0);
        }
        let section = optional[index].id.as_str();
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
        let mut persisted = Config::load_quiet()?.context("no config exists; run `aishe setup`")?;
        persisted.aishe.context_exclude = config.aishe.context_exclude.clone();
        persisted.save()?;
    }
}

pub fn profile(effective: &Config, value: Option<&str>) -> u8 {
    let Some(value) = value else {
        println!("profile: {}", effective.aishe.safety_profile);
        return 0;
    };
    let Some(profile) = crate::profiles::Profile::parse(value) else {
        crate::cli::error_contract::emit_classified(
            crate::user_error::ErrorNamespace::Cli,
            "unknown_profile",
            format!("Unknown safety profile '{value}'."),
            "Run `aishe profile` to see conservative, balanced, autonomous, and custom.",
            None,
        );
        return 1;
    };
    let mut config = match Config::load_quiet() {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!("aishe: no config; run `aishe setup`");
            return 1;
        }
        Err(error) => {
            eprintln!("aishe: {error}");
            return 1;
        }
    };
    let changes = crate::profiles::apply(&mut config, profile);
    if let Err(error) = config.save() {
        eprintln!("aishe: {error}");
        return 1;
    }
    println!("profile = {}", profile.key());
    if changes.is_empty() {
        println!("  no setting changes");
    } else {
        for change in changes {
            println!("  {}: {} → {}", change.field, change.before, change.after);
        }
    }
    0
}

pub fn price(_effective: &Config, command: &PriceAction) -> u8 {
    let mut config = match Config::load_quiet() {
        Ok(Some(config)) => config,
        Ok(None) => {
            eprintln!("aishe: no config; run `aishe setup`");
            return 1;
        }
        Err(error) => {
            eprintln!("aishe: {error}");
            return 1;
        }
    };
    match command {
        PriceAction::List => {
            if config.pricing.is_empty() {
                println!("no user price overrides");
            } else {
                println!("user prices (USD per 1M tokens):");
                for (model, price) in &config.pricing {
                    println!(
                        "  {}: input ${:.6} · output ${:.6}",
                        crate::commands::display_safe(model),
                        price.input,
                        price.output
                    );
                }
            }
            let model = config.active_model();
            match crate::usage::price_for(model, &config.pricing) {
                Some(price) => println!(
                    "active {}: input ${:.6} · output ${:.6}",
                    crate::commands::display_safe(model),
                    price.input,
                    price.output
                ),
                None => {
                    let model = crate::commands::display_safe(model);
                    println!(
                        "active {model}: unknown; run `aishe price set {model} --input USD --output USD`"
                    )
                }
            }
            return 0;
        }
        PriceAction::Set {
            model,
            input,
            output,
        } => {
            if !input.is_finite() || *input < 0.0 || !output.is_finite() || *output < 0.0 {
                eprintln!("aishe: prices must be finite non-negative numbers");
                return 1;
            }
            config.pricing.insert(
                model.clone(),
                crate::usage::Price {
                    input: *input,
                    output: *output,
                },
            );
            if let Err(error) = config.save() {
                eprintln!("aishe: {error}");
                return 1;
            }
            println!(
                "price {} = input ${input:.6} · output ${output:.6} per 1M tokens",
                crate::commands::display_safe(model)
            );
        }
        PriceAction::Remove { model } => {
            if config.pricing.remove(model).is_none() {
                eprintln!(
                    "aishe: no exact user price override for '{}'",
                    crate::commands::display_safe(model)
                );
                return 1;
            }
            if let Err(error) = config.save() {
                eprintln!("aishe: {error}");
                return 1;
            }
            println!(
                "removed price override for {}",
                crate::commands::display_safe(model)
            );
        }
    }
    0
}

pub fn notify_project_overlay(outcome: &Option<crate::config::OverlayOutcome>) {
    let Some(o) = outcome else { return };
    if let Some(err) = &o.error {
        eprintln!(
            "{}",
            format!(
                "aishe: ignoring malformed project config {}: {err}",
                o.path.display()
            )
            .yellow()
        );
        return;
    }
    if !o.applied.is_empty() {
        let how = if o.trusted { ", trusted" } else { "" };
        eprintln!(
            "{}",
            format!(
                "aishe: applied project config {} ({} key(s){how})",
                o.path.display(),
                o.applied.len()
            )
            .dim()
        );
    }
    if !o.deferred.is_empty() {
        eprintln!(
            "{}",
            format!(
                "aishe: {} sensitive key(s) in {} need trust to apply ({}). Run `aishe trust`.",
                o.deferred.len(),
                o.path.display(),
                o.deferred.join(", ")
            )
            .yellow()
        );
    }
}

/// Resolve the nearest project config from cwd, or print why there is none.
fn current_project_config() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    match Config::find_project_config(&cwd) {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "aishe: no .aishe/config.toml found at or above {}",
                cwd.display()
            );
            None
        }
    }
}

/// `aishe trust [--list]`: trust the current project's config, or list trusted.
pub fn trust(list: bool, explicit: Option<&std::path::Path>) -> u8 {
    if list {
        let items = crate::trust::list();
        if items.is_empty() {
            println!("No trusted project files.");
        } else {
            println!("Trusted project files:");
            for (path, _) in items {
                println!("  {path}");
            }
        }
        return 0;
    }
    // With no argument this trusts the project config; with one it trusts that
    // exact file, which is how a project skill or command is enabled (the gate
    // that rejects them prints the very command to run).
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => match current_project_config() {
            Some(p) => p,
            None => return 1,
        },
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("aishe: {}: {e}", path.display());
            return 1;
        }
    };
    // Only a `config.toml` has sensitive keys to report; a skill or command file
    // is markdown, so parsing it as TOML would be meaningless.
    let is_config = path.file_name().and_then(|n| n.to_str()) == Some("config.toml");
    let deferred = if is_config {
        match toml::from_str::<toml::Table>(&text) {
            Ok(table) => Config::default().merge_project_table(&table, false).1,
            Err(e) => {
                eprintln!("aishe: malformed project config {}: {e}", path.display());
                return 1;
            }
        }
    } else {
        Vec::new()
    };
    match crate::trust::trust(&path, &text) {
        Ok(_) => {
            println!("Trusted {}", path.display());
            if !deferred.is_empty() {
                println!("  now applies: {}", deferred.join(", "));
            }
            0
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            1
        }
    }
}

/// `aishe untrust [--all]`: drop trust for the current project, or all of them.
pub fn untrust(all: bool, explicit: Option<&std::path::Path>) -> u8 {
    if all {
        return match crate::trust::untrust_all() {
            Ok(n) => {
                println!("Dropped trust for {n} project file(s).");
                0
            }
            Err(e) => {
                eprintln!("aishe: {e}");
                1
            }
        };
    }
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => match current_project_config() {
            Some(p) => p,
            None => return 1,
        },
    };
    match crate::trust::untrust(&path) {
        Ok(true) => {
            println!("Dropped trust for {}", path.display());
            0
        }
        Ok(false) => {
            println!("{} was not trusted", path.display());
            0
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            1
        }
    }
}

/// `aishe undo` / `aishe undo --list`: revert the most recent AI file change (or
/// list recorded change sets). Reads the reversible-edits journal written by the
/// built-in file tools.
pub fn undo(list: bool) -> u8 {
    if list {
        let batches = crate::undo::list();
        if batches.is_empty() {
            println!("no recorded AI file changes");
            return 0;
        }
        println!("recorded AI file changes (most recent last):");
        for b in &batches {
            let state = if b.reverted {
                "reverted".dim().to_string()
            } else {
                "active".green().to_string()
            };
            println!(
                "  {}  {} file(s)  [{}]  {}",
                b.id,
                b.files.len(),
                state,
                b.summary.as_str().dim()
            );
        }
        return 0;
    }
    match crate::undo::undo_last() {
        Ok(Some(u)) => {
            for f in &u.restored {
                println!("{} {}", "restored".green(), f);
            }
            for e in &u.errors {
                eprintln!("{} {}", "aishe undo:".red(), e);
            }
            if u.restored.is_empty() && u.errors.is_empty() {
                println!("nothing to restore in the last change set");
            }
            if u.errors.is_empty() {
                0
            } else {
                1
            }
        }
        Ok(None) => {
            println!("nothing to undo");
            0
        }
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").red());
            1
        }
    }
}
