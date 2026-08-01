use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::agent::controller::INTERRUPTED;
use crate::config::Config;
use crate::executor::Executor;
use crate::session::Session;
use crate::skills::SkillRegistry;
use crate::{context, modes, providers};

/// Parsed durable-session action transferred from the binary's Clap surface.
#[derive(Clone, Debug)]
pub enum Action {
    Show { id: String, json: bool },
    Rename { id: String, name: String },
    Delete { id: String },
}

/// Persistent conversation-memory file for shell-hook front ends.
pub fn hook_session_path(config: &Config) -> Option<std::path::PathBuf> {
    if !config.aishe.memory {
        return None;
    }
    std::env::var("AISHE_SESSION_FILE")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(std::path::PathBuf::from)
}

pub fn list(json_output: bool) -> u8 {
    let records = crate::tasks::list();
    let managed = crate::backend::opencode::session::SessionStore::from_default_root()
        .and_then(|store| store.records(None))
        .unwrap_or_default();
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "managed": managed,
                "legacy": records,
            }))
            .expect("serializing a serde_json::Value to String cannot fail")
        );
        return 0;
    }
    if records.is_empty() && managed.is_empty() {
        println!("no AI sessions");
        return 0;
    }
    if !managed.is_empty() {
        println!("managed OpenCode sessions (newest mapping last):");
        for record in managed {
            println!(
                "  {}  {:?} · {:?}  {}",
                crate::commands::display_safe(&record.backend_session_id),
                record.mode,
                record.scope,
                crate::commands::display_safe(&record.workspace.display().to_string())
            );
        }
    }
    if records.is_empty() {
        return 0;
    }
    println!("legacy durable task sessions (oldest first, retained):");
    for record in records {
        println!(
            "  {}  {:?}  {} / {}  {}",
            crate::commands::display_safe(&record.id),
            record.status,
            crate::commands::display_safe(&record.provider),
            crate::commands::display_safe(&record.model),
            crate::commands::display_safe(
                &record
                    .name
                    .as_deref()
                    .unwrap_or(record.objective.as_str())
                    .chars()
                    .take(72)
                    .collect::<String>()
            )
        );
    }
    0
}

pub fn reset(config: &Config) -> Result<u8> {
    if std::env::var_os("AISHE_SHELL_ID").is_none() {
        anyhow::bail!("`aishe reset` must run inside an active AIShe shell");
    }
    let shell_id = crate::agent::controller::current_shell_id()?;
    let workspace = std::env::current_dir().context("resolving the current workspace")?;
    let detached = crate::backend::opencode::session::SessionStore::from_default_root()?.reset(
        &shell_id,
        &workspace,
        config.active_connection_id(),
        config.active_model(),
    )?;

    // Keep the temporary native-fallback transcript aligned with the managed
    // reset. Saving an empty transcript uses the same atomic bounded path as
    // ordinary hook memory and never touches durable shell history.
    if let Some(path) = hook_session_path(config) {
        Session::new(true).save_persisted(&path);
    }

    match detached {
        Some(mapping) => {
            println!("conversation reset; the next AI turn starts fresh");
            println!(
                "previous session retained: {}",
                crate::commands::display_safe(&mapping.backend_session_id)
            );
            println!(
                "resume it with: aishe resume {}",
                crate::commands::display_safe(&mapping.backend_session_id)
            );
            crate::audit::action(
                "agent:reset",
                &format!("retained_session={}", mapping.backend_session_id),
                Some(0),
            );
        }
        None => println!("conversation already fresh; the next AI turn starts a new session"),
    }
    Ok(0)
}

pub fn command(command: &Action) -> Result<u8> {
    match command {
        Action::Show { id, json } => {
            let record = crate::tasks::load(id)?;
            if *json {
                println!("{}", serde_json::to_string_pretty(&record)?);
            } else {
                println!("task: {}", record.id);
                println!("status: {:?}", record.status);
                println!(
                    "name: {}",
                    crate::commands::display_safe(record.name.as_deref().unwrap_or("(none)"))
                );
                println!(
                    "objective: {}",
                    crate::commands::display_safe(&record.objective)
                );
                println!(
                    "provider: {} · {}",
                    crate::commands::display_safe(&record.provider),
                    crate::commands::display_safe(&record.model)
                );
                println!(
                    "cwd: {}",
                    crate::commands::display_safe(&record.cwd.display().to_string())
                );
                println!(
                    "usage: {} in · {} out · {} reqs",
                    record.usage.input, record.usage.output, record.usage.requests
                );
                println!("messages: {}", record.messages.len());
                println!("completed tools: {}", record.completed_tools.len());
                if let Some(pending) = record.pending_tool {
                    println!(
                        "pending tool: {} ({}, may_have_started={})",
                        crate::commands::display_safe(&pending.call.name),
                        crate::commands::display_safe(&pending.call.id),
                        pending.may_have_started
                    );
                }
                if let Some(error) = record.last_error {
                    println!("last error: {}", crate::commands::display_safe(&error));
                }
            }
            Ok(0)
        }
        Action::Rename { id, name } => {
            crate::tasks::rename(id, name)?;
            println!("renamed task {id} to {name}");
            Ok(0)
        }
        Action::Delete { id } => {
            crate::tasks::delete(id)?;
            println!("deleted task {id} (the task record cannot be recovered)");
            Ok(0)
        }
    }
}

pub fn resume(
    config: &Config,
    id: Option<&str>,
    replacement_cwd: Option<&std::path::Path>,
) -> Result<u8> {
    if let Some(id) = id.filter(|id| id.starts_with("ses_")) {
        return resume_managed(config, id, replacement_cwd);
    }
    let record = match id {
        Some(id) => crate::tasks::load(id)?,
        None => crate::tasks::most_recent_resumable()
            .context("no interrupted, failed, or active task is available to resume")?,
    };
    let cwd = if record.cwd.is_dir() {
        record.cwd.clone()
    } else if let Some(path) = replacement_cwd {
        if !path.is_dir() {
            anyhow::bail!("replacement cwd {} is not a directory", path.display());
        }
        path.to_path_buf()
    } else if std::io::stdin().is_terminal() {
        let current = std::env::current_dir()?;
        let Some(value) = crate::promptui::text(
            &format!(
                "Original cwd {} is missing; replacement",
                record.cwd.display()
            ),
            &current.display().to_string(),
            |value| {
                if std::path::Path::new(value).is_dir() {
                    Ok(())
                } else {
                    anyhow::bail!("path must be an existing directory")
                }
            },
        )?
        else {
            anyhow::bail!("resume cancelled");
        };
        if value == ":back" {
            anyhow::bail!("resume cancelled");
        }
        std::path::PathBuf::from(value)
    } else {
        anyhow::bail!(
            "original cwd {} no longer exists; pass `aishe resume {} --cwd PATH`",
            record.cwd.display(),
            record.id
        );
    };
    let provider = providers::make(config).map_err(|error| {
        anyhow::anyhow!("cannot resume without an LLM provider: {error}; run `aishe doctor --live`")
    })?;
    let mut executor = Executor::new()?;
    executor.redirect_cwd(cwd);
    executor.set_history_log(crate::cli::history::history_paths(config).1);
    context::init(executor.shell());
    crate::cli::history::init_audit(config);
    let skills = SkillRegistry::load();
    let mcp = crate::mcp::McpRegistry::connect(&config.mcp_servers);
    modes::yolo::resume(
        record,
        provider.as_ref(),
        &mut executor,
        config,
        &INTERRUPTED,
        &skills,
        &mcp,
    )?;
    crate::cli::status::record_session_usage(Some(provider.as_ref()), config);
    Ok(0)
}

fn resume_managed(
    config: &Config,
    id: &str,
    replacement_cwd: Option<&std::path::Path>,
) -> Result<u8> {
    use crate::agent::{BackendSession, ExecutionScope, Mode, NetworkPolicy};

    let already_in_aishe_shell = std::env::var_os("AISHE_SHELL_ID").is_some();
    if !already_in_aishe_shell
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
    {
        anyhow::bail!(
            "`aishe resume {id}` needs an interactive terminal when run outside an active AIShe shell"
        );
    }
    let state = crate::backend::supervisor::ensure_running(config)?;
    let control = crate::backend::control::SupervisorClient::new(state)?;
    let client = crate::backend::opencode::OpenCodeClient::new(
        control.opencode_connection(),
        control.provider_id(),
        control.model_id(),
    )?;
    let sessions = client.list_sessions(None)?;
    let summary = sessions
        .into_iter()
        .find(|session| session.id == id)
        .with_context(|| format!("managed session '{id}' was not found"))?;
    let workspace = match replacement_cwd {
        Some(path) if path.is_dir() => path.to_path_buf(),
        Some(path) => anyhow::bail!("replacement cwd {} is not a directory", path.display()),
        None if summary.workspace.is_dir() => summary.workspace,
        None => anyhow::bail!(
            "managed session workspace {} is missing; pass --cwd PATH",
            summary.workspace.display()
        ),
    };
    let session = BackendSession {
        id: id.to_string(),
        workspace,
        backend: "opencode".into(),
    };
    let mode = Mode::parse(&config.aishe.mode).context("active mode is invalid")?;
    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("active backend scope is invalid")?;
    let network = if scope == ExecutionScope::Host {
        NetworkPolicy::Allow
    } else {
        NetworkPolicy::parse(&config.backend.workspace_network)
            .context("active backend network policy is invalid")?
    };
    let shell_id = crate::agent::controller::current_shell_id()?;
    crate::backend::opencode::session::SessionStore::from_default_root()?.bind(
        &shell_id,
        &session,
        crate::backend::opencode::session::SessionBinding::new(
            config.active_connection_id(),
            config.active_model(),
            mode,
            scope,
            network,
        ),
    )?;
    println!("resumed managed session {id}");
    println!("workspace: {}", session.workspace.display());
    if already_in_aishe_shell {
        println!("The next natural-language turn in this shell continues that conversation.");
        return Ok(0);
    }
    println!("Opening AIShe; natural-language turns continue that conversation.");
    std::env::set_current_dir(&session.workspace).with_context(|| {
        format!(
            "entering managed session workspace {}",
            session.workspace.display()
        )
    })?;
    crate::pty::run_zsh_with_shell_id(
        config,
        &crate::cli::history::history_paths(config).1,
        &shell_id,
    )
}
