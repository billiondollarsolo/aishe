//! Local workflow inspection and deliberate execution controls.

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::background::workflows::{self, WorkflowRun, WorkflowTemplate};
use crate::config::Config;
use crate::promptui::{self, PickerResult};

pub enum Action {
    Browse,
    List {
        json: bool,
    },
    Show {
        name: String,
        json: bool,
    },
    Save {
        name: String,
        file: PathBuf,
    },
    Run {
        name: String,
        parameters: Vec<String>,
        no_isolation: bool,
    },
    Runs {
        json: bool,
    },
    Cancel {
        id: String,
    },
    Resume {
        id: String,
    },
    Remove {
        name: String,
    },
}

pub fn command(config: &Config, action: Action) -> Result<u8> {
    match action {
        Action::Browse => browse(config)?,
        Action::List { json } => {
            let templates = workflows::list_templates()?;
            if json {
                crate::cli::json_contract::print_envelope("workflows", &templates)?;
            } else if templates.is_empty() {
                println!("No saved workflows. Use aishe workflow save NAME --file PATH.");
            } else {
                for template in templates {
                    println!(
                        "{} · {} stages · {} parallel",
                        safe(&template.name),
                        template.stages.len(),
                        template.max_parallel
                    );
                }
            }
        }
        Action::Show { name, json } => {
            if let Ok(template) = workflows::load_template(&name) {
                if json {
                    crate::cli::json_contract::print_object(&template)?;
                } else {
                    for line in template_lines(&template) {
                        println!("{line}");
                    }
                }
            } else {
                let run = workflows::run_details(&name)?;
                if json {
                    crate::cli::json_contract::print_object(&run)?;
                } else {
                    for line in run_lines(&run) {
                        println!("{line}");
                    }
                }
            }
        }
        Action::Save { name, file } => {
            workflows::save_template_file(&name, &file)?;
            println!("Saved workflow {}.", safe(&name));
        }
        Action::Run {
            name,
            parameters,
            no_isolation,
        } => {
            let run = workflows::run_template(
                config,
                &name,
                parse_parameters(&parameters)?,
                no_isolation,
            )?;
            println!(
                "Started workflow {} · {} stages · task browser: aishe task browse",
                run.id,
                run.stages.len()
            );
        }
        Action::Runs { json } => {
            let runs = workflows::list_runs()?;
            if json {
                crate::cli::json_contract::print_envelope("runs", &runs)?;
            } else {
                for run in runs {
                    println!(
                        "{} · {} · {:?} · {} stages",
                        run.id,
                        safe(&run.name),
                        run.state,
                        run.stages.len()
                    );
                }
            }
        }
        Action::Cancel { id } => {
            workflows::cancel_run(&id)?;
            println!("Stopped workflow {id}; stage work is retained.");
        }
        Action::Resume { id } => {
            workflows::resume_run(&id)?;
            println!("Resumed workflow scheduler {id}.");
        }
        Action::Remove { name } => {
            workflows::remove_template(&name)?;
            println!("Removed workflow {}.", safe(&name));
        }
    }
    Ok(0)
}

fn parse_parameters(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for value in values {
        let (name, value) = value
            .split_once('=')
            .context("workflow parameters use KEY=VALUE")?;
        if result.insert(name.to_string(), value.to_string()).is_some() {
            anyhow::bail!("duplicate workflow parameter {name}");
        }
    }
    Ok(result)
}

pub fn template_lines(template: &WorkflowTemplate) -> Vec<String> {
    let mut lines = vec![format!(
        "{} · {} stages · at most {} parallel",
        safe(&template.name),
        template.stages.len(),
        template.max_parallel
    )];
    if !template.description.is_empty() {
        lines.push(safe(&template.description));
    }
    for stage in &template.stages {
        lines.push(format!(
            "  {} · {} · {} / {}",
            safe(&stage.key),
            safe(&stage.name),
            stage.scope,
            stage.network
        ));
        if !stage.depends_on.is_empty() {
            lines.push(format!(
                "    after {}",
                stage
                    .depends_on
                    .iter()
                    .map(|v| safe(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines.push(format!("    {}", safe(&stage.objective)));
        lines.push(format!(
            "    budget: {} min · {} turns · {} tools · {} files",
            stage.budget.max_minutes,
            stage.budget.max_provider_turns,
            stage.budget.max_tool_calls,
            stage.budget.max_changed_files
        ));
        lines.push(format!(
            "    changed size: {} MiB · {} · cost: {}",
            stage.budget.max_changed_bytes / 1_048_576,
            if stage.network == "deny" {
                "network denied".into()
            } else {
                format!("{} network calls", stage.budget.max_network_calls)
            },
            if stage.budget.max_cost_usd > 0.0 {
                format!("${:.2} task cap", stage.budget.max_cost_usd)
            } else {
                "session cap".into()
            }
        ));
        for check in &stage.required_checks {
            lines.push(format!("    required check: {}", safe(check)));
        }
    }
    lines
}

pub fn run_lines(run: &WorkflowRun) -> Vec<String> {
    let complete = run
        .stages
        .iter()
        .filter(|s| s.snapshot_head.is_some())
        .count();
    let mut lines = vec![format!(
        "{} · {} · {:?} · {complete}/{} stages",
        safe(&run.name),
        run.id,
        run.state,
        run.stages.len()
    )];
    for stage in &run.stages {
        lines.push(format!(
            "  {} · {} · {}",
            safe(&stage.key),
            safe(&stage.name),
            stage
                .state
                .map(|s| format!("{s:?}"))
                .unwrap_or_else(|| "queued".into())
        ));
        if !stage.depends_on.is_empty() {
            lines.push(format!("    after {}", stage.depends_on.join(", ")));
        }
        lines.push(format!("    task {}", stage.task_id));
    }
    if let Some(error) = &run.error {
        lines.push(format!("Needs attention: {}", safe(error)));
    }
    lines
}

fn browse(config: &Config) -> Result<()> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        command(config, Action::List { json: false })?;
        return Ok(());
    }
    loop {
        let templates = workflows::list_templates()?;
        let runs = workflows::list_runs()?;
        let mut labels = templates
            .iter()
            .map(|t| format!("{} · {} stages", safe(&t.name), t.stages.len()))
            .collect::<Vec<_>>();
        labels.extend(
            runs.iter()
                .take(32)
                .map(|r| format!("{} · {:?} · {}", safe(&r.name), r.state, r.id)),
        );
        labels.push("Back to shell".into());
        let PickerResult::Use(index) =
            promptui::filter_picker("Workflows", &labels, labels.len() - 1)?
        else {
            return Ok(());
        };
        if let Some(template) = templates.get(index) {
            start_template(config, template)?;
        } else if let Some(run) = runs.get(index.saturating_sub(templates.len())) {
            for line in run_lines(run) {
                println!("{line}");
            }
            let mut choices = run
                .stages
                .iter()
                .map(|s| format!("View {}", safe(&s.name)))
                .collect::<Vec<_>>();
            choices.push("Back to workflows".into());
            if let PickerResult::Use(stage) =
                promptui::filter_picker("Workflow stages", &choices, choices.len() - 1)?
            {
                if let Some(stage) = run.stages.get(stage) {
                    crate::cli::taskui::browse(config, Some(&stage.task_id), true)?;
                }
            }
        } else {
            return Ok(());
        }
    }
}

fn start_template(config: &Config, template: &WorkflowTemplate) -> Result<()> {
    for line in template_lines(template) {
        println!("{line}");
    }
    let mut parameters = BTreeMap::new();
    for parameter in &template.parameters {
        let default = parameter.default.as_deref().unwrap_or("");
        print!(
            "{}{}: ",
            safe(&parameter.name),
            if default.is_empty() {
                String::new()
            } else {
                format!(" [{}]", safe(default))
            }
        );
        std::io::stdout().flush()?;
        let mut value = String::new();
        if std::io::stdin().read_line(&mut value)? == 0 {
            return Ok(());
        }
        let value = value.trim_end_matches(['\r', '\n']);
        parameters.insert(
            parameter.name.clone(),
            if value.is_empty() {
                default.into()
            } else {
                value.into()
            },
        );
    }
    let choices = vec![
        "Start workflow in isolated worktrees".into(),
        "Back to workflows".into(),
    ];
    if matches!(
        promptui::filter_picker("Start workflow", &choices, 1)?,
        PickerResult::Use(0)
    ) {
        let run = workflows::run_template(config, &template.name, parameters, false)?;
        println!("Started workflow {}.", run.id);
    }
    Ok(())
}

fn safe(value: &str) -> String {
    crate::commands::display_safe(&crate::redact::redact(value))
}
