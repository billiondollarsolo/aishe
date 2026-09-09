//! In-process NL turn: warm provider HTTP, never OpenCode.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use crate::config::Config;
use crate::executor::Executor;
use crate::modes;
use crate::modes::suggest::Suggestion;
use crate::providers::{self, Provider};
use crate::safety::{self, Risk};
use crate::session::Session;
use crate::skills::SkillRegistry;

use super::grant::{ensure_session_grant, LeanGrant, LeanMode};

/// `-c` / hook NL entry used when lean is on. Skips `backend::supervisor`.
pub fn run_nl(
    nl: &str,
    mode: &str,
    provider: Option<&dyn Provider>,
    executor: &mut Executor,
    config: &Config,
    skills: &SkillRegistry,
    mcp: &crate::mcp::McpRegistry,
    session: &mut Session,
) -> Result<()> {
    super::mark_nl_turn_start();
    let lean_mode = LeanMode::parse(mode);
    if lean_mode != LeanMode::Ask {
        match ensure_session_grant(config, lean_mode)? {
            LeanGrant::Accepted => {}
            LeanGrant::Declined => return Ok(()),
        }
        prepare_agent_executor(executor, config, lean_mode)?;
    }
    let Some(provider) = provider else {
        eprintln!(
            "aishe: no provider configured for connection '{}'",
            crate::commands::display_safe(config.active_connection_id())
        );
        return Ok(());
    };
    match lean_mode {
        LeanMode::Agent => modes::yolo::run(
            nl,
            provider,
            executor,
            config,
            &crate::agent::controller::INTERRUPTED,
            skills,
            mcp,
            session,
        )?,
        LeanMode::Allow => {
            modes::suggest::run(nl, provider, executor, config, false, true, session)?
        }
        LeanMode::Ask => {
            modes::suggest::run(nl, provider, executor, config, false, false, session)?
        }
    }
    Ok(())
}

pub fn prepare_agent_executor(
    executor: &mut Executor,
    config: &Config,
    mode: LeanMode,
) -> Result<()> {
    executor.prefer_posix_capture();
    if mode == LeanMode::Agent && config.backend.default_scope != "host" {
        #[cfg(target_os = "linux")]
        {
            if crate::sandbox::bwrap_available() {
                executor.set_sandbox_wrap(crate::sandbox::bwrap_wrap_argv(executor.cwd()));
            }
        }
    }
    Ok(())
}

/// FIFO request from the PTY child. Returns a single-line reply.
pub fn handle_ipc_line(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    raw: &str,
) -> String {
    super::mark_nl_turn_start();
    let (op, rest) = raw.split_once('\t').unwrap_or((raw, ""));
    match op {
        "NL" | "SLASH" => {
            let mut parts = rest.splitn(3, '\t');
            let mode = LeanMode::parse(parts.next().unwrap_or("ask"));
            let cwd = parts.next().unwrap_or("");
            let line = parts.next().unwrap_or("").trim();
            if op == "NL" {
                handle_nl(config, provider, executor, session, mode, cwd, line)
            } else {
                handle_slash(config, mode, line)
            }
        }
        "CONFIRM_YES" => run_confirmed(executor, rest.trim()),
        "STOP" => "OK".into(),
        _ => format!("ERROR\tunknown lean op {op}"),
    }
}

fn handle_nl(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    mode: LeanMode,
    cwd: &str,
    line: &str,
) -> String {
    if line.is_empty() {
        return "ANSWER\t".into();
    }
    if !cwd.is_empty() {
        let path = PathBuf::from(cwd);
        if path.is_dir() {
            executor.redirect_cwd(path);
        }
    }
    if provider.is_none() {
        *provider = providers::make(config).ok();
    }
    let Some(provider) = provider.as_deref() else {
        return "ERROR\tno provider configured (set an API key, or AISHE_FAKE_LLM for tests)"
            .into();
    };
    if mode != LeanMode::Ask {
        let _ = prepare_agent_executor(executor, config, mode);
    }
    match mode {
        LeanMode::Ask => suggest_reply(line, provider, executor, config, session, false),
        LeanMode::Allow => suggest_reply(line, provider, executor, config, session, true),
        LeanMode::Agent => agent_reply(line, provider, executor, config, session),
    }
}

fn suggest_reply(
    line: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    session: &mut Session,
    auto_run_safe: bool,
) -> String {
    match modes::suggest::request(line, provider, executor, config, session.history()) {
        Ok(suggestion) => match suggestion {
            Suggestion::Answer { explanation } => {
                session.record_user(line);
                session.record_assistant(&explanation);
                format!("ANSWER\t{}", flatten(&explanation))
            }
            Suggestion::Command {
                command,
                explanation,
            } => {
                session.record_user(line);
                session.record_assistant(&command);
                if auto_run_safe {
                    match safety::assess(&command) {
                        Risk::Safe => run_now(executor, &command),
                        Risk::Dangerous(reason) | Risk::Unknown(reason) => {
                            format!("CONFIRM\t{}", flatten(&format!("{command} ({reason})")))
                        }
                    }
                } else {
                    let _ = explanation;
                    format!("FILL\t{}", flatten(&command))
                }
            }
        },
        Err(error) => format!("ERROR\t{}", flatten(&error.to_string())),
    }
}

fn agent_reply(
    line: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    session: &mut Session,
) -> String {
    let skills = SkillRegistry::load();
    let mcp = crate::mcp::McpRegistry::connect(&config.mcp_servers);
    match modes::yolo::run(
        line,
        provider,
        executor,
        config,
        &crate::agent::controller::INTERRUPTED,
        &skills,
        &mcp,
        session,
    ) {
        Ok(()) => "RAN\t".into(),
        Err(error) => format!("ERROR\t{}", flatten(&error.to_string())),
    }
}

fn handle_slash(config: &Config, mode: LeanMode, line: &str) -> String {
    let name = line.split_whitespace().next().unwrap_or("");
    match name {
        "/status" => format!(
            "ANSWER\tlean {} · mode {} · model {}",
            env!("CARGO_PKG_VERSION"),
            mode.as_str(),
            crate::commands::display_safe(config.active_model())
        ),
        "/reset" => "ANSWER\tsession reset (next NL starts clean)".into(),
        "/usage" => "ANSWER\t/usage is a stub on the lean W1 path".into(),
        "/details" => "ANSWER\tfocus renderer is the lean default".into(),
        "/undo" => "ANSWER\t/undo is unchanged: run `aishe undo`".into(),
        "/model" => format!(
            "ANSWER\tmodel {}",
            crate::commands::display_safe(config.active_model())
        ),
        _ => format!("ERROR\tunknown slash {name}"),
    }
}

fn run_confirmed(executor: &mut Executor, command: &str) -> String {
    let command = command.split(" (").next().unwrap_or(command).trim();
    if command.is_empty() {
        return "ERROR\tnothing to run".into();
    }
    run_now(executor, command)
}

fn run_now(executor: &mut Executor, command: &str) -> String {
    let code = executor.run(command);
    format!("RAN\texit {code}")
}

fn flatten(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}
