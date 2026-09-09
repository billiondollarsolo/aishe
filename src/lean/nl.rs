//! In-process NL turn: warm provider HTTP, never OpenCode.
//!
//! Lean IPC replies are **control** (`OK` / `FILL_B64` / `CONFIRM_B64` / `RAN` /
//! `ERROR`). Multi-line answers and agent tool transcripts are written by the
//! parent onto the PTY master via [`PtyOut`] so newlines/markdown survive.

use std::path::{Path, PathBuf};
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
use super::sessions::LeanSessionStore;
use super::stdout_redirect::StdoutRedirect;

/// Warm MCP + skills once per live lean shell (lazy on first agent/`/status`).
#[derive(Default)]
pub struct LeanWarm {
    pub skills: Option<SkillRegistry>,
    pub mcp: Option<crate::mcp::McpRegistry>,
}

impl LeanWarm {
    pub fn ensure(&mut self, config: &Config) {
        if self.skills.is_none() {
            self.skills = Some(SkillRegistry::load());
        }
        if self.mcp.is_none() {
            // Empty/disabled config → empty registry; never touches OpenCode.
            self.mcp = Some(crate::mcp::McpRegistry::connect(&config.mcp_servers));
        }
    }

    pub fn skills_len(&self) -> usize {
        self.skills.as_ref().map(|s| s.len()).unwrap_or(0)
    }

    pub fn mcp_tool_len(&self) -> usize {
        self.mcp.as_ref().map(|m| m.list().len()).unwrap_or(0)
    }

    pub fn mcp_server_hint(&self, config: &Config) -> String {
        let configured = config
            .mcp_servers
            .iter()
            .filter(|(_, c)| c.enabled)
            .count();
        if configured == 0 {
            "none configured".into()
        } else if self.mcp.is_none() {
            format!("{configured} configured (not warmed)")
        } else {
            let tools = self.mcp_tool_len();
            format!("{configured} configured · {tools} tool(s)")
        }
    }
}

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
    let nl = prepare_nl_prompt(nl, executor.cwd(), config);
    match lean_mode {
        LeanMode::Agent => modes::yolo::run(
            &nl,
            provider,
            executor,
            config,
            &crate::agent::controller::INTERRUPTED,
            skills,
            mcp,
            session,
        )?,
        LeanMode::Allow => {
            modes::suggest::run(&nl, provider, executor, config, false, true, session)?
        }
        LeanMode::Ask => {
            modes::suggest::run(&nl, provider, executor, config, false, false, session)?
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
    config: &mut Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &mut LeanWarm,
    pty: &PtyOut,
    raw: &str,
) -> String {
    super::mark_nl_turn_start();
    let (op, rest) = raw.split_once('\t').unwrap_or((raw, ""));
    match op {
        "NL" | "SLASH" | "FIX" => {
            let mut parts = rest.splitn(3, '\t');
            let mode = LeanMode::parse(parts.next().unwrap_or("ask"));
            let cwd = parts.next().unwrap_or("");
            let line = parts.next().unwrap_or("").trim();
            ensure_store(store, cwd, config);
            if !cwd.is_empty() {
                let path = PathBuf::from(cwd);
                if path.is_dir() {
                    executor.redirect_cwd(path);
                }
            }
            match op {
                "NL" => handle_nl(
                    config, provider, executor, session, store, warm, pty, mode, cwd, line,
                ),
                "FIX" => handle_fix(config, provider, executor, session, store, pty, line),
                _ => handle_slash(config, provider, session, store, warm, pty, mode, line),
            }
        }
        "CONFIRM_YES" => run_confirmed(executor, rest.trim()),
        "STOP" => "OK".into(),
        _ => format!("ERROR\tunknown lean op {op}"),
    }
}

fn ensure_store(store: &mut Option<LeanSessionStore>, cwd: &str, config: &Config) {
    if store.is_none() {
        *store = Some(LeanSessionStore::create(
            if cwd.is_empty() { "/" } else { cwd },
            config.active_model(),
        ));
    }
}

fn persist_store(store: &mut Option<LeanSessionStore>, session: &Session) {
    if let Some(store) = store.as_mut() {
        store.persist(session);
    }
}

fn handle_nl(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &mut LeanWarm,
    pty: &PtyOut,
    mode: LeanMode,
    cwd: &str,
    line: &str,
) -> String {
    // Empty `?` (hook sends "?" or "") → explain last failure capsule.
    if line.is_empty() || line == "?" {
        return explain_last_failure(config, provider, executor, session, store, pty);
    }
    if provider.is_none() {
        *provider = providers::make(config).ok();
    }
    let Some(provider_ref) = provider.as_deref() else {
        return "ERROR\tno provider configured (set an API key, or AISHE_FAKE_LLM for tests)"
            .into();
    };
    if mode != LeanMode::Ask {
        let _ = prepare_agent_executor(executor, config, mode);
    }
    let cwd_path = if cwd.is_empty() {
        executor.cwd().to_path_buf()
    } else {
        PathBuf::from(cwd)
    };
    let expanded = prepare_nl_prompt(line, &cwd_path, config);
    let reply = match mode {
        LeanMode::Ask => {
            suggest_reply(&expanded, provider_ref, executor, config, session, pty, false)
        }
        LeanMode::Allow => {
            suggest_reply(&expanded, provider_ref, executor, config, session, pty, true)
        }
        LeanMode::Agent => {
            agent_reply(&expanded, provider_ref, executor, config, session, warm, pty)
        }
    };
    if reply == "OK" || reply == "RAN" || reply.starts_with("FILL_B64\t") || reply.starts_with("CONFIRM_B64\t")
    {
        persist_store(store, session);
    }
    reply
}

fn explain_last_failure(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    pty: &PtyOut,
) -> String {
    let capsule = match crate::failure::current() {
        Ok(c) => c,
        Err(_) => {
            emit_text(pty, "no failed command to explain (run a command, then ?)");
            return "OK".into();
        }
    };
    let prompt = format!(
        "Explain why this shell command failed with exit status {} and suggest safe next steps. Do not execute anything.\nCommand: {}",
        capsule.exit_status, capsule.command
    );
    if provider.is_none() {
        *provider = providers::make(config).ok();
    }
    let Some(provider_ref) = provider.as_deref() else {
        return "ERROR\tno provider configured".into();
    };
    let reply = suggest_reply(&prompt, provider_ref, executor, config, session, pty, false);
    persist_store(store, session);
    reply
}

fn handle_fix(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    pty: &PtyOut,
    _line: &str,
) -> String {
    let capsule = match crate::failure::current() {
        Ok(c) => c,
        Err(_) => {
            emit_text(pty, "no failed command to fix");
            return "OK".into();
        }
    };
    if capsule.redacted {
        emit_text(pty, "fix disabled: stored command was redacted");
        return "OK".into();
    }
    if provider.is_none() {
        *provider = providers::make(config).ok();
    }
    let Some(provider_ref) = provider.as_deref() else {
        return "ERROR\tno provider configured".into();
    };
    let ctx = crate::fix::error_context(&capsule.command, config.aishe.fix_capture_stderr);
    let prompt = crate::fix::build_prompt(
        &capsule.command,
        &capsule.exit_status.to_string(),
        ctx.as_deref(),
    );
    match modes::suggest::request(&prompt, provider_ref, executor, config, Vec::new()) {
        Ok(Suggestion::Command {
            command,
            explanation,
        }) => {
            session.record_user(&format!("fix: {}", capsule.command));
            session.record_assistant(&command);
            if !explanation.trim().is_empty() {
                emit_text(pty, &explanation);
            }
            persist_store(store, session);
            format!("FILL_B64\t{}", b64(&command))
        }
        Ok(Suggestion::Answer { explanation }) => {
            session.record_user(&format!("fix: {}", capsule.command));
            session.record_assistant(&explanation);
            emit_text(pty, &explanation);
            persist_store(store, session);
            "OK".into()
        }
        Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
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
    warm: &mut LeanWarm,
    pty: &PtyOut,
) -> String {
    warm.ensure(config);
    let skills = warm.skills.as_ref().expect("skills warmed");
    let mcp = warm.mcp.as_ref().expect("mcp warmed");
    let _redirect = StdoutRedirect::to_pty(pty.clone());
    match modes::yolo::run(
        line,
        provider,
        executor,
        config,
        &crate::agent::controller::INTERRUPTED,
        skills,
        mcp,
        session,
    ) {
        Ok(()) => "RAN".into(),
        Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
    }
}

fn handle_slash(
    config: &mut Config,
    provider: &mut Option<Arc<dyn Provider>>,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &mut LeanWarm,
    pty: &PtyOut,
    mode: LeanMode,
    line: &str,
) -> String {
    let mut parts = line.split_whitespace();
    let name = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");
    match name {
        "/status" => {
            warm.ensure(config);
            let sid = store
                .as_ref()
                .map(|s| s.id().to_string())
                .unwrap_or_else(|| "-".into());
            let conn = config.active_connection_id();
            emit_text(
                pty,
                &format!(
                    "lean {} · mode {} · connection {} · model {} · session {}",
                    env!("CARGO_PKG_VERSION"),
                    mode.as_str(),
                    crate::commands::display_safe(conn),
                    crate::commands::display_safe(config.active_model()),
                    crate::commands::display_safe(&sid)
                ),
            );
            emit_text(
                pty,
                &format!(
                    "skills: {} loaded · mcp: {}",
                    warm.skills_len(),
                    warm.mcp_server_hint(config)
                ),
            );
            "OK".into()
        }
        "/reset" => {
            if let Some(store) = store.as_mut() {
                store.clear(session);
            } else {
                session.clear();
            }
            emit_text(pty, "session cleared");
            "OK".into()
        }
        "/usage" => {
            let msg = match provider.as_deref() {
                Some(p) => {
                    let snap = p.meter().snapshot();
                    if snap.is_empty() {
                        "usage: no model calls yet this session".into()
                    } else {
                        format!(
                            "usage: {}",
                            crate::usage::summary(snap, config.active_model(), &config.pricing)
                        )
                    }
                }
                None => "usage: no model calls yet this session".into(),
            };
            emit_text(pty, &msg);
            if config.aishe.budget_usd > 0.0 {
                emit_text(
                    pty,
                    &format!("budget: ${:.2}", config.aishe.budget_usd),
                );
            }
            "OK".into()
        }
        "/sessions" => handle_sessions_slash(session, store, pty, arg),
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
        "/model" => handle_model_slash(config, provider, pty, arg),
        "/connection" => handle_connection_slash(config, provider, pty, arg),
        _ => format!("ERROR\tunknown slash {name}"),
    }
}

/// Lean-safe model list: connection + provider catalog + capability cache.
/// Never starts OpenCode (unlike `capabilities::known_models` OAuth path).
fn lean_known_models(config: &Config) -> Vec<String> {
    let id = config.active_connection_id().to_string();
    let mut models = Vec::new();
    if let Some(connection) = config.active_connection() {
        if !connection.settings.model.is_empty() {
            models.push(connection.settings.model.clone());
        }
        let endpoint = crate::provider_catalog::normalize_base_url(&connection.settings.base_url);
        models.extend(
            crate::provider_catalog::SERVICES
                .iter()
                .filter(|service| {
                    !service.model.is_empty()
                        && crate::provider_catalog::normalize_base_url(service.base_url) == endpoint
                })
                .map(|service| service.model.to_string()),
        );
    }
    if let Some(report) = crate::capabilities::load(config) {
        if report.connection_id == id {
            models.extend(report.models);
            if !report.model.is_empty() {
                models.push(report.model);
            }
        }
    }
    models.retain(|model| crate::connection::validate_model_id(model).is_ok());
    models.sort();
    models.dedup();
    let active = config.active_model().to_string();
    if let Some(position) = models.iter().position(|m| m == &active) {
        models.swap(0, position);
    } else if !active.is_empty() {
        models.insert(0, active);
    }
    models
}

fn handle_model_slash(
    config: &mut Config,
    provider: &mut Option<Arc<dyn Provider>>,
    pty: &PtyOut,
    arg: &str,
) -> String {
    if arg.is_empty() {
        let models = lean_known_models(config);
        let active = config.active_model();
        let mut out = format!(
            "models for {} (active *):",
            crate::commands::display_safe(config.active_connection_id())
        );
        if models.is_empty() {
            out.push_str("\n  (none — set via /model NAME)");
        } else {
            for (i, model) in models.iter().enumerate() {
                let mark = if model == active { " *" } else { "" };
                out.push('\n');
                out.push_str(&format!(
                    "  {}. {}{}",
                    i + 1,
                    crate::commands::display_safe(model),
                    mark
                ));
            }
        }
        out.push_str("\nusage: /model <name|#>");
        emit_text(pty, &out);
        return "OK".into();
    }
    let models = lean_known_models(config);
    let chosen = if let Ok(n) = arg.parse::<usize>() {
        if n >= 1 && n <= models.len() {
            models[n - 1].clone()
        } else {
            emit_text(pty, &format!("no model #{n}"));
            return "OK".into();
        }
    } else {
        arg.to_string()
    };
    if let Err(error) = crate::connection::validate_model_id(&chosen) {
        return format!("ERROR\t{}", one_line(&error.to_string()));
    }
    config.set_active_model(chosen.clone());
    let _ = crate::connection::write_shell_selection(config, "shell");
    *provider = providers::make(config).ok();
    emit_text(
        pty,
        &format!(
            "model {} (this shell)",
            crate::commands::display_safe(config.active_model())
        ),
    );
    "OK".into()
}

fn handle_connection_slash(
    config: &mut Config,
    provider: &mut Option<Arc<dyn Provider>>,
    pty: &PtyOut,
    arg: &str,
) -> String {
    if arg.is_empty() {
        let active = config.active_connection_id();
        let mut out = String::from(
            "connections (* active; Grok subscription remains default happy path):",
        );
        if config.connections.is_empty() {
            out.push_str("\n  (none configured — aishe setup)");
        } else {
            for (i, (id, connection)) in config.connections.iter().enumerate() {
                let mark = if id.as_str() == active { " *" } else { "" };
                out.push('\n');
                out.push_str(&format!(
                    "  {}. {}  {}  model={}{}",
                    i + 1,
                    crate::commands::display_safe(id),
                    crate::commands::display_safe(&connection.label),
                    crate::commands::display_safe(&connection.settings.model),
                    mark
                ));
            }
        }
        out.push_str("\nusage: /connection <id|label|#>");
        emit_text(pty, &out);
        return "OK".into();
    }
    let ids: Vec<String> = config.connections.keys().cloned().collect();
    let id = if let Ok(n) = arg.parse::<usize>() {
        if n >= 1 && n <= ids.len() {
            ids[n - 1].clone()
        } else {
            emit_text(pty, &format!("no connection #{n}"));
            return "OK".into();
        }
    } else {
        match config.resolve_connection_id(arg) {
            Ok(id) => id,
            Err(error) => return format!("ERROR\t{}", one_line(&error.to_string())),
        }
    };
    if let Err(error) = config.select_connection(&id) {
        return format!("ERROR\t{}", one_line(&error.to_string()));
    }
    let _ = crate::connection::write_shell_selection(config, "shell");
    *provider = providers::make(config).ok();
    emit_text(
        pty,
        &format!(
            "connection {} · model {} (this shell)",
            crate::commands::display_safe(config.active_connection_id()),
            crate::commands::display_safe(config.active_model())
        ),
    );
    "OK".into()
}

fn handle_sessions_slash(
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    pty: &PtyOut,
    arg: &str,
) -> String {
    match arg {
        "" | "list" => {
            let listed = super::sessions::list();
            if listed.is_empty() {
                emit_text(pty, "no lean sessions");
            } else {
                let mut out = String::from("lean sessions (oldest first):");
                for meta in listed.iter().rev().take(20).collect::<Vec<_>>().into_iter().rev() {
                    out.push('\n');
                    out.push_str(&format!(
                        "  {}  turns={}  {}",
                        crate::commands::display_safe(&meta.id),
                        meta.turns,
                        crate::commands::display_safe(
                            if meta.title.is_empty() {
                                meta.cwd.as_str()
                            } else {
                                meta.title.as_str()
                            }
                        )
                    ));
                }
                emit_text(pty, &out);
            }
            "OK".into()
        }
        "clear" => {
            if let Some(store) = store.as_mut() {
                store.clear(session);
            } else {
                session.clear();
            }
            emit_text(pty, "current lean session cleared");
            "OK".into()
        }
        other if other.starts_with("resume") || !other.is_empty() => {
            let id = other.strip_prefix("resume:").unwrap_or(other);
            let id = id.strip_prefix("resume").unwrap_or(id).trim_matches(':').trim();
            if id.is_empty() {
                emit_text(pty, "usage: /sessions resume:<id>");
                return "OK".into();
            }
            match super::sessions::load_session(id) {
                Some(loaded) => {
                    *session = loaded;
                    if let Some(store) = store.as_mut() {
                        // Keep current store id; re-persist resumed transcript under it.
                        store.persist(session);
                    }
                    emit_text(
                        pty,
                        &format!(
                            "resumed {} ({} turns)",
                            crate::commands::display_safe(id),
                            session.turns()
                        ),
                    );
                    "OK".into()
                }
                None => {
                    emit_text(pty, &format!("unknown lean session {id}"));
                    "OK".into()
                }
            }
        }
        _ => {
            emit_text(pty, "usage: /sessions [list|clear|resume:<id>]");
            "OK".into()
        }
    }
}

fn expand_attachments(line: &str, cwd: &Path, config: &Config) -> String {
    match crate::attachments::expand(line, cwd, config) {
        Ok(expanded) => expanded.prompt,
        Err(error) => {
            // Soft-fail: keep original line so NL still runs; surface error inline.
            format!("{line}\n\n[attachment error: {}]", one_line(&error.to_string()))
        }
    }
}

/// Attachments go through `attachments::expand` (redacts file bodies when enabled).
/// Suggest/agent then pull `.aishe/context.md` via `context::build` (also redacts).
/// Extra pass here scrubs secret shapes in the raw user prompt itself.
fn prepare_nl_prompt(line: &str, cwd: &Path, config: &Config) -> String {
    let expanded = expand_attachments(line, cwd, config);
    if config.aishe.redact_secrets {
        crate::redact::redact(&expanded)
    } else {
        expanded
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
    use std::sync::{Mutex, OnceLock};

    fn fake_llm_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
    }

    fn test_config() -> Config {
        Config::default()
    }

    fn with_store<F: FnOnce(&mut Option<LeanSessionStore>) -> T, T>(f: F) -> T {
        let _sessions_guard = crate::lean::sessions::test_env_lock();
        let root = std::env::temp_dir().join(format!(
            "aishe-lean-nl-store-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("AISHE_LEAN_SESSIONS", &root);
        let mut store = Some(LeanSessionStore::create("/tmp", "test"));
        let out = f(&mut store);
        std::env::remove_var("AISHE_LEAN_SESSIONS");
        let _ = std::fs::remove_dir_all(&root);
        out
    }

    #[test]
    fn answer_preserves_newlines_on_pty_and_returns_ok() {
        let _guard = fake_llm_lock();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"answer\",\"command\":null,\"explanation\":\"line1\\nline2\\n\\n```\\ncode\\n```\"}",
        );
        let mut config = test_config();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
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
        });
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn reset_clears_session_history_and_durable() {
        let mut config = test_config();
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        session.record_user("prior");
        session.record_assistant("prior-answer");
        assert!(!session.history().is_empty());
        let pty = PtyOut::capture();
        with_store(|store| {
            store.as_mut().unwrap().persist(&session);
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/reset",
            );
            assert_eq!(reply, "OK");
            assert!(session.history().is_empty(), "session not cleared");
            assert!(pty.take_capture().contains("session cleared"));
            let loaded = Session::load_persisted(store.as_ref().unwrap().path());
            assert!(loaded.history().is_empty(), "durable not cleared");
        });
    }

    #[test]
    fn usage_reports_meter_not_stub() {
        let _guard = fake_llm_lock();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"answer\",\"command\":null,\"explanation\":\"ok\"}",
        );
        std::env::set_var("AISHE_FAKE_USAGE", "12,4");
        let mut config = test_config();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let _ = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\thello",
            );
            let _ = pty.take_capture();
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/usage",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("usage:") && !shown.contains("stub"),
                "expected real usage, got {shown:?}"
            );
            assert!(
                shown.contains("12") || shown.contains("in"),
                "meter missing from /usage: {shown:?}"
            );
        });
        std::env::remove_var("AISHE_FAKE_LLM");
        std::env::remove_var("AISHE_FAKE_USAGE");
    }

    #[test]
    fn fill_uses_b64_not_flatten() {
        let _guard = fake_llm_lock();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"command\",\"command\":\"printf 'hi\\nthere'\",\"explanation\":\"say hi\"}",
        );
        let mut config = test_config();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
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
        });
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn at_file_expands_before_nl() {
        let _guard = fake_llm_lock();
        let dir = std::env::temp_dir().join(format!(
            "aishe-lean-attach-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, "secret-payload-xyz").unwrap();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"answer\",\"command\":null,\"explanation\":\"saw-it\"}",
        );
        let mut config = test_config();
        config.backend.default_scope = "host".into();
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        let line = format!("summarize @file:{}", file.display());
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                &format!("NL\task\t{}\t{line}", dir.display()),
            );
            assert_eq!(reply, "OK");
            // User turn recorded should include attachment expansion.
            let hist = session.history();
            let user = hist
                .iter()
                .find_map(|m| match m {
                    crate::providers::Msg::User(t) => Some(t.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            assert!(
                user.contains("secret-payload-xyz") || user.contains("Explicit attachments"),
                "attachment not expanded into NL prompt: {user:?}"
            );
        });
        std::env::remove_var("AISHE_FAKE_LLM");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn connection_lists_and_model_pick() {
        let mut config = test_config();
        assert!(!config.connections.is_empty());
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/connection",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("connections") && shown.contains("usage: /connection"),
                "expected connection list, got {shown:?}"
            );

            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/model",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("models for") && shown.contains("usage: /model"),
                "expected model list, got {shown:?}"
            );

            let active = config.active_model().to_string();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                &format!("SLASH\task\t/tmp\t/model {active}"),
            );
            assert_eq!(reply, "OK");
            assert_eq!(config.active_model(), active);
        });
    }

    #[test]
    fn status_warms_skills_and_mcp_once() {
        let mut config = test_config();
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            assert!(warm.skills.is_none());
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/status",
            );
            assert_eq!(reply, "OK");
            assert!(warm.skills.is_some());
            assert!(warm.mcp.is_some());
            let shown = pty.take_capture();
            assert!(
                shown.contains("skills:") && shown.contains("mcp:"),
                "status must surface skills/mcp: {shown:?}"
            );
            assert!(
                shown.contains("connection") && shown.contains("model"),
                "status must show connection+model: {shown:?}"
            );
        });
    }

    #[test]
    fn lean_nl_redacts_secret_shapes_when_enabled() {
        let _guard = fake_llm_lock();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            "{\"type\":\"answer\",\"command\":null,\"explanation\":\"ok\"}",
        );
        let mut config = test_config();
        config.aishe.redact_secrets = true;
        let mut provider = providers::make(&config).ok();
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\texport API_TOKEN=supersecretvalue123 please explain",
            );
            assert_eq!(reply, "OK");
            let hist = session.history();
            let user = hist
                .iter()
                .find_map(|m| match m {
                    crate::providers::Msg::User(t) => Some(t.as_str()),
                    _ => None,
                })
                .unwrap_or("");
            assert!(
                user.contains("<redacted>") || !user.contains("supersecretvalue123"),
                "lean NL must redact secret shapes before record: {user:?}"
            );
        });
        std::env::remove_var("AISHE_FAKE_LLM");
    }

}
