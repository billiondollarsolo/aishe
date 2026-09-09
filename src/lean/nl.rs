//! In-process NL turn: warm provider HTTP, never OpenCode.
//!
//! Lean IPC replies are **control** (`OK` / `FILL_B64` / `CONFIRM_B64` / `RAN` /
//! `ERROR`). Multi-line answers and agent tool transcripts are written by the
//! parent onto the PTY master via [`PtyOut`] so newlines/markdown survive.

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
use super::pty_out::PtyOut;
use super::stdout_redirect::StdoutRedirect;

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

/// FIFO request from the PTY child. Returns a single-line **control** reply.
pub fn handle_ipc_line(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    pty: &PtyOut,
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
                handle_nl(config, provider, executor, session, pty, mode, cwd, line)
            } else {
                handle_slash(config, session, pty, mode, line)
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
    pty: &PtyOut,
    mode: LeanMode,
    cwd: &str,
    line: &str,
) -> String {
    if line.is_empty() {
        return "OK".into();
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
        LeanMode::Ask => suggest_reply(line, provider, executor, config, session, pty, false),
        LeanMode::Allow => suggest_reply(line, provider, executor, config, session, pty, true),
        LeanMode::Agent => agent_reply(line, provider, executor, config, session, pty),
    }
}

fn suggest_reply(
    line: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    session: &mut Session,
    pty: &PtyOut,
    auto_run_safe: bool,
) -> String {
    match modes::suggest::request(line, provider, executor, config, session.history()) {
        Ok(suggestion) => match suggestion {
            Suggestion::Answer { explanation } => {
                session.record_user(line);
                session.record_assistant(&explanation);
                emit_text(pty, &explanation);
                "OK".into()
            }
            Suggestion::Command {
                command,
                explanation,
            } => {
                session.record_user(line);
                session.record_assistant(&command);
                if !explanation.trim().is_empty() {
                    emit_text(pty, &explanation);
                }
                if auto_run_safe {
                    match safety::assess(&command) {
                        Risk::Safe => run_now(executor, &command),
                        Risk::Dangerous(reason) | Risk::Unknown(reason) => {
                            format!("CONFIRM_B64\t{}", b64(&format!("{command} ({reason})")))
                        }
                    }
                } else {
                    format!("FILL_B64\t{}", b64(&command))
                }
            }
        },
        Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
    }
}

/// Complex NL: warm in-process ReAct/tool loop (`modes::yolo`), not OpenCode.
/// Transcript goes to the interactive PTY; FIFO returns `RAN` only.
fn agent_reply(
    line: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    session: &mut Session,
    pty: &PtyOut,
) -> String {
    let skills = SkillRegistry::load();
    let mcp = crate::mcp::McpRegistry::connect(&config.mcp_servers);
    let _redirect = StdoutRedirect::to_pty(pty.clone());
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
        Ok(()) => "RAN".into(),
        Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
    }
}

fn handle_slash(
    config: &Config,
    session: &mut Session,
    pty: &PtyOut,
    mode: LeanMode,
    line: &str,
) -> String {
    let name = line.split_whitespace().next().unwrap_or("");
    match name {
        "/status" => {
            emit_text(
                pty,
                &format!(
                    "lean {} · mode {} · model {}",
                    env!("CARGO_PKG_VERSION"),
                    mode.as_str(),
                    crate::commands::display_safe(config.active_model())
                ),
            );
            "OK".into()
        }
        "/reset" => {
            session.clear();
            emit_text(pty, "session cleared");
            "OK".into()
        }
        "/usage" => {
            emit_text(pty, "/usage is a stub on the lean W1 path");
            "OK".into()
        }
        "/details" => {
            emit_text(pty, "focus renderer is the lean default");
            "OK".into()
        }
        "/undo" => match crate::undo::undo_last() {
            Ok(Some(undone)) => {
                let mut msg = format!(
                    "undone batch {} · {} file(s) restored",
                    undone.batch,
                    undone.restored.len()
                );
                for path in undone.restored.iter().take(8) {
                    msg.push('\n');
                    msg.push_str("  ");
                    msg.push_str(path);
                }
                if !undone.errors.is_empty() {
                    msg.push('\n');
                    msg.push_str(&format!("{} error(s)", undone.errors.len()));
                }
                emit_text(pty, &msg);
                "OK".into()
            }
            Ok(None) => {
                emit_text(pty, "nothing to undo");
                "OK".into()
            }
            Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
        },
        "/model" => {
            emit_text(
                pty,
                &format!(
                    "model {}",
                    crate::commands::display_safe(config.active_model())
                ),
            );
            "OK".into()
        }
        _ => format!("ERROR\tunknown slash {name}"),
    }
}

fn run_confirmed(executor: &mut Executor, command: &str) -> String {
    let decoded = decode_confirm_payload(command);
    let command = decoded.split(" (").next().unwrap_or(decoded.as_str()).trim();
    if command.is_empty() {
        return "ERROR\tnothing to run".into();
    }
    run_now(executor, command)
}

fn decode_confirm_payload(raw: &str) -> String {
    // CONFIRM_YES may carry plain text or the CONFIRM_B64 body.
    if let Ok(bytes) =
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, raw.trim())
    {
        if let Ok(text) = String::from_utf8(bytes) {
            if text.contains('(') || text.contains(' ') {
                return text;
            }
        }
    }
    raw.to_string()
}

fn run_now(executor: &mut Executor, command: &str) -> String {
    let code = executor.run(command);
    format!("RAN\texit {code}")
}

fn emit_text(pty: &PtyOut, text: &str) {
    pty.write_user_line(text);
}

fn b64(text: &str) -> String {
    base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        text.as_bytes(),
    )
}

fn one_line(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config::default()
    }

    #[test]
    fn answer_preserves_newlines_on_pty_and_returns_ok() {
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"answer\",\"command\":null,\"explanation\":\"line1\\nline2\\n\\n```\\ncode\\n```\"}",
        );
        let config = test_config();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        let reply = handle_ipc_line(
            &config,
            &mut provider,
            &mut executor,
            &mut session,
            &pty,
            "NL\task\t/tmp\twhat is lean",
        );
        assert_eq!(reply, "OK");
        let shown = pty.take_capture();
        assert!(
            shown.contains("line1") && shown.contains("line2"),
            "pty lost multi-line answer: {shown:?}"
        );
        assert!(
            shown.contains('\n') || shown.contains("\r\n"),
            "newlines flattened away: {shown:?}"
        );
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn reset_clears_session_history() {
        let config = test_config();
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        session.record_user("prior");
        session.record_assistant("prior-answer");
        assert!(!session.history().is_empty());
        let pty = PtyOut::capture();
        let reply = handle_ipc_line(
            &config,
            &mut provider,
            &mut executor,
            &mut session,
            &pty,
            "SLASH\task\t/tmp\t/reset",
        );
        assert_eq!(reply, "OK");
        assert!(session.history().is_empty(), "session not cleared");
        assert!(pty.take_capture().contains("session cleared"));
    }

    #[test]
    fn fill_uses_b64_not_flatten() {
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"command\",\"command\":\"printf 'hi\\nthere'\",\"explanation\":\"say hi\"}",
        );
        let config = test_config();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        let reply = handle_ipc_line(
            &config,
            &mut provider,
            &mut executor,
            &mut session,
            &pty,
            "NL\task\t/tmp\tplease print hi",
        );
        assert!(
            reply.starts_with("FILL_B64\t"),
            "expected FILL_B64, got {reply}"
        );
        let payload = reply.trim_start_matches("FILL_B64\t");
        let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
            .expect("b64");
        let cmd = String::from_utf8(bytes).unwrap();
        assert!(cmd.contains("printf"), "{cmd}");
        std::env::remove_var("AISHE_FAKE_LLM");
    }
}
