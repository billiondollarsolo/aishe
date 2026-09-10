//! In-process NL turn: warm provider HTTP, never OpenCode.
//!
//! Lean IPC replies are **control** (`OK` / `STREAM_END` / `FILL_B64` /
//! `CONFIRM_B64` / `RAN` / `ERROR`). Token streams and multi-line answers are
//! written by the parent onto the PTY master via [`PtyOut`] as they arrive;
//! the FIFO stays control-only (never carries answer body).

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
use crate::commands::CommandRegistry;
use crate::skills::SkillRegistry;

use super::grant::{ensure_session_grant, LeanGrant, LeanMode};
use super::pty_out::PtyOut;
use super::sessions::LeanSessionStore;
use super::stdout_redirect::StdoutRedirect;

/// Warm MCP + skills + custom slash-commands once per live lean shell
/// (lazy on first agent/`/status`/`/help`/custom slash).
#[derive(Default)]
pub struct LeanWarm {
    pub skills: Option<SkillRegistry>,
    pub mcp: Option<crate::mcp::McpRegistry>,
    pub commands: Option<CommandRegistry>,
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
        if self.commands.is_none() {
            self.commands = Some(CommandRegistry::load());
            refresh_custom_cmds_file(self.commands.as_ref());
        }
    }

    /// Custom slash-command names for `/help` / `/commands` / tab file.
    pub fn command_names(&self) -> Vec<String> {
        self.commands
            .as_ref()
            .map(|c| c.list().into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default()
    }

    pub fn command_list(&self) -> Vec<(String, String)> {
        self.commands
            .as_ref()
            .map(|c| c.list())
            .unwrap_or_default()
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

    /// Skill names for `/skills` (empty when none / not warmed).
    pub fn skill_names(&self) -> Vec<String> {
        self.skills
            .as_ref()
            .map(|s| s.list().into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default()
    }

    /// Enabled MCP server ids from config (names, not just counts).
    pub fn mcp_server_names(config: &Config) -> Vec<String> {
        config
            .mcp_servers
            .iter()
            .filter(|(_, c)| c.enabled)
            .map(|(n, _)| n.clone())
            .collect()
    }

    /// Connected MCP tool names after warm (may be empty if servers down).
    pub fn mcp_tool_names(&self) -> Vec<String> {
        self.mcp
            .as_ref()
            .map(|m| m.list().into_iter().map(|(n, _)| n).collect())
            .unwrap_or_default()
    }
}

/// Publish custom slash names for lean tab completion (`AISHE_LEAN_CMDS_FILE`).
fn refresh_custom_cmds_file(commands: Option<&CommandRegistry>) {
    let Ok(path) = std::env::var("AISHE_LEAN_CMDS_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let body = commands
        .map(|c| {
            c.list()
                .into_iter()
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    let _ = std::fs::write(path, body);
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
                _ => handle_slash(
                    config, provider, executor, session, store, warm, pty, mode, cwd, line,
                ),
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
    if reply == "OK"
        || reply == "STREAM_END"
        || reply == "RAN"
        || reply.starts_with("FILL_B64\t")
        || reply.starts_with("CONFIRM_B64\t")
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
    // Lean default: stream ask answers via complete_stream into PtyOut.
    // Allow streams when config.aishe.stream is on. Commands stay FILL/CONFIRM.
    let stream = should_stream_lean_ask(config, auto_run_safe);
    if stream {
        return suggest_reply_streamed(
            line,
            provider,
            executor,
            config,
            session,
            pty,
            auto_run_safe,
        );
    }
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

/// Lean ask (and allow-when-stream) path: provider SSE / `complete_stream` into
/// the PTY master; FIFO returns `STREAM_END` (answer) or FILL/CONFIRM (command).
fn suggest_reply_streamed(
    line: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    session: &mut Session,
    pty: &PtyOut,
    auto_run_safe: bool,
) -> String {
    let mut out = super::PtyWrite::new(pty);
    match modes::suggest::request_streamed(
        line,
        provider,
        executor,
        config,
        session.history(),
        &mut out,
    ) {
        Ok(suggestion) => match suggestion {
            Suggestion::Answer { explanation } => {
                session.record_user(line);
                session.record_assistant(&explanation);
                // Deltas already on PTY; control only.
                "STREAM_END".into()
            }
            Suggestion::Command {
                command,
                explanation,
            } => {
                session.record_user(line);
                session.record_assistant(&command);
                // Command path withheld stream body; optional WHY on PTY.
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

/// Lean streams ask answers by default (capability: live tokens). `config.stream`
/// additionally enables streaming on allow. Never involves OpenCode.
fn should_stream_lean_ask(config: &Config, auto_run_safe: bool) -> bool {
    if config.aishe.stream {
        return true;
    }
    // Lean default for ask answers.
    !auto_run_safe
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
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &mut LeanWarm,
    pty: &PtyOut,
    mode: LeanMode,
    cwd: &str,
    line: &str,
) -> String {
    let mut parts = line.split_whitespace();
    let name = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");
    // Remaining args after the slash name (for custom commands).
    let rest_args: Vec<&str> = line.split_whitespace().skip(1).collect();
    match name {
        "/help" | "/commands" => {
            warm.ensure(config);
            emit_lean_help(warm, pty, name == "/commands");
            "OK".into()
        }
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
                    "skills: {} loaded · mcp: {} · custom: {}",
                    warm.skills_len(),
                    warm.mcp_server_hint(config),
                    warm.command_names().len()
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
            let next = cycle_details_density(&mut config.backend.output);
            config.aishe.yolo_verbose = next == "detailed";
            std::env::set_var("AISHE_AGENT_OUTPUT", next);
            if let Ok(path) = std::env::var("AISHE_OUTPUT_FILE") {
                if !path.is_empty() {
                    let _ = std::fs::write(&path, next);
                }
            }
            emit_text(pty, &format!("details: {next} (this shell)"));
            "OK".into()
        }
        "/skills" => {
            warm.ensure(config);
            let names = warm.skill_names();
            if names.is_empty() {
                emit_text(pty, "skills: (none loaded)");
            } else {
                emit_text(pty, &format!("skills ({}):", names.len()));
                for skill in names.iter().take(64) {
                    emit_text(pty, &format!("  {skill}"));
                }
                if names.len() > 64 {
                    emit_text(pty, &format!("  … +{} more", names.len() - 64));
                }
            }
            "OK".into()
        }
        "/mcp" => {
            warm.ensure(config);
            let servers = LeanWarm::mcp_server_names(config);
            let tools = warm.mcp_tool_names();
            if servers.is_empty() {
                emit_text(pty, "mcp: none configured");
            } else {
                emit_text(pty, &format!("mcp servers ({}):", servers.len()));
                for sname in &servers {
                    emit_text(pty, &format!("  {sname}"));
                }
            }
            if tools.is_empty() {
                emit_text(pty, "mcp tools: (none connected)");
            } else {
                emit_text(pty, &format!("mcp tools ({}):", tools.len()));
                for tname in tools.iter().take(64) {
                    emit_text(pty, &format!("  {tname}"));
                }
                if tools.len() > 64 {
                    emit_text(pty, &format!("  … +{} more", tools.len() - 64));
                }
            }
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
        "/backend" => {
            emit_text(
                pty,
                "heavy specialist backends are opt-in only — never auto on known-cmd or default NL",
            );
            emit_text(
                pty,
                "escape hatch: AISHE_LEGACY_OPENCODE=1 (or AISHE_LEAN=0) · see src/lean/heavy.rs",
            );
            emit_text(
                pty,
                "named backends (not default controller): opencode | codex | claude-code",
            );
            "OK".into()
        }
        _ => handle_custom_or_unknown(
            config,
            provider,
            executor,
            session,
            store,
            warm,
            pty,
            mode,
            cwd,
            name,
            &rest_args,
        ),
    }
}

fn emit_lean_help(warm: &LeanWarm, pty: &PtyOut, commands_only: bool) {
    if !commands_only {
        emit_text(
            pty,
            "aishe lean: typed commands run in zsh -f. English goes to the model.",
        );
        emit_text(pty, "  ? force NL   ! force shell   Ctrl-X ? show route");
        emit_text(pty, "  empty ? explains last failure · Ctrl-X Ctrl-F suggests a fix");
        emit_text(
            pty,
            "  /mode ask|allow|agent   Shift-Tab cycles (aliases suggest|auto|yolo)",
        );
        emit_text(pty, "  /connection /model   list/pick for this shell");
        emit_text(
            pty,
            "  /sessions list|clear|resume:<id>   /usage /status /reset /undo",
        );
        emit_text(
            pty,
            "  /details or Ctrl-O   cycle focus|compact|detailed (this shell)",
        );
        emit_text(pty, "  /mcp /skills   list names (not just /status counts)");
        emit_text(pty, "  /commands   list custom markdown slash-commands");
        emit_text(
            pty,
            "  /backend   heavy specialist opt-in note (no auto OpenCode)",
        );
        emit_text(
            pty,
            "  Default mode is ask. allow/agent need one typed grant per shell.",
        );
        emit_text(
            pty,
            "  Auth: Grok CLI OAuth (~/.grok/auth.json) · API-key fallback · OpenAI OAuth is LEGACY",
        );
    }
    let list = warm.command_list();
    if list.is_empty() {
        let hint = crate::commands::user_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "~/.config/aishe/commands".into());
        emit_text(
            pty,
            &format!(
                "custom slash-commands: none (add *.md under {})",
                crate::commands::display_safe(&hint)
            ),
        );
    } else {
        emit_text(pty, &format!("custom slash-commands ({}):", list.len()));
        for (cname, desc) in list.iter().take(64) {
            let d = if desc.is_empty() {
                String::new()
            } else {
                format!(" — {}", crate::commands::display_safe(desc))
            };
            emit_text(pty, &format!("  /{cname}{d}"));
        }
        if list.len() > 64 {
            emit_text(pty, &format!("  … +{} more", list.len() - 64));
        }
    }
}

fn custom_cmd_trusted(cmd: &crate::commands::CustomCommand) -> bool {
    match cmd.source.as_deref() {
        None => true,
        Some(src) => {
            let contents = std::fs::read_to_string(src).unwrap_or_default();
            crate::trust::is_trusted(src, &contents)
        }
    }
}

fn handle_custom_or_unknown(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &mut LeanWarm,
    pty: &PtyOut,
    mode: LeanMode,
    cwd: &str,
    name: &str,
    args: &[&str],
) -> String {
    // Only single-segment /name (reuse discovery; not a path like /usr/bin/x).
    let bare = name.strip_prefix('/').unwrap_or("");
    if bare.is_empty()
        || bare.contains('/')
        || !bare
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return format!("ERROR\tunknown slash {name}");
    }
    warm.ensure(config);
    let Some(cmd) = warm
        .commands
        .as_ref()
        .and_then(|reg| reg.get(bare))
        .cloned()
    else {
        return format!("ERROR\tunknown slash {name}");
    };
    let ex = cmd.expand(args);
    if ex.text.is_empty() {
        emit_text(pty, &format!("/{bare}: empty expansion"));
        return "OK".into();
    }
    let trusted = custom_cmd_trusted(&cmd);
    if ex.shell {
        if cmd.needs_trust_confirm(trusted) {
            let shown = cmd
                .source
                .as_ref()
                .map(|p| crate::commands::display_safe(&p.display().to_string()))
                .unwrap_or_else(|| "(project)".into());
            emit_text(
                pty,
                &format!("untrusted project command /{bare} — run: aishe trust {shown}"),
            );
            emit_text(
                pty,
                &format!("would run: {}", crate::commands::display_safe(&ex.text)),
            );
            return "OK".into();
        }
        match safety::assess(&ex.text) {
            Risk::Safe => run_now(executor, &ex.text),
            Risk::Dangerous(reason) | Risk::Unknown(reason) => {
                format!("CONFIRM_B64\t{}", b64(&format!("{} ({reason})", ex.text)))
            }
        }
    } else {
        // Frontmatter still uses suggest|auto|yolo; map current lean mode for
        // escalation checks, then parse back to LeanMode for the NL turn.
        let configured_legacy = match mode {
            LeanMode::Ask => "suggest",
            LeanMode::Allow => "auto",
            LeanMode::Agent => "yolo",
        };
        let effective = cmd.effective_mode(configured_legacy, trusted);
        if let Some(want) = ex.mode.as_deref().filter(|w| *w != effective) {
            emit_text(
                pty,
                &format!(
                    "aishe: ignoring untrusted project command mode '{}' — running in '{}'",
                    crate::commands::display_safe(want),
                    crate::commands::display_safe(effective)
                ),
            );
        }
        let run_mode = LeanMode::parse(effective);
        handle_nl(
            config, provider, executor, session, store, warm, pty, run_mode, cwd, &ex.text,
        )
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

fn cycle_details_density(current: &mut String) -> &'static str {
    let next = match current.trim().to_ascii_lowercase().as_str() {
        "focus" => "compact",
        "compact" => "detailed",
        _ => "focus",
    };
    *current = next.to_string();
    next
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
            assert!(
                reply == "OK" || reply == "STREAM_END",
                "expected OK/STREAM_END, got {reply}"
            );
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
            assert!(reply == "OK" || reply == "STREAM_END", "expected OK/STREAM_END, got {reply}");
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
            assert!(reply == "OK" || reply == "STREAM_END", "expected OK/STREAM_END, got {reply}");
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
            assert!(reply == "OK" || reply == "STREAM_END", "expected OK/STREAM_END, got {reply}");
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


    #[test]
    fn ask_streams_answer_chunks_into_pty_and_returns_stream_end() {
        let _guard = fake_llm_lock();
        std::env::set_var(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","command":null,"explanation":"alpha beta gamma delta"}"#,
        );
        std::env::set_var("AISHE_FAKE_STREAM_CHUNK", "5");
        let chunk_spy = std::env::temp_dir().join(format!(
            "aishe-stream-chunks-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("AISHE_SPY_STREAM_CHUNKS", &chunk_spy);
        let mut config = test_config();
        config.aishe.stream = false; // lean ask still streams by default
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
                "NL\task\t/tmp\tstream please",
            );
            assert_eq!(reply, "STREAM_END");
            let shown = pty.take_capture();
            assert!(
                shown.contains("alpha") && shown.contains("delta"),
                "pty missing streamed answer: {shown:?}"
            );
        });
        let n: u64 = std::fs::read_to_string(&chunk_spy)
            .unwrap_or_default()
            .trim()
            .parse()
            .unwrap_or(0);
        assert!(n > 1, "fake complete_stream should chunk, got {n}");
        std::env::remove_var("AISHE_FAKE_LLM");
        std::env::remove_var("AISHE_FAKE_STREAM_CHUNK");
        std::env::remove_var("AISHE_SPY_STREAM_CHUNKS");
        let _ = std::fs::remove_file(&chunk_spy);
    }

    #[test]
    fn details_cycles_density_into_pty_and_config() {
        let mut config = test_config();
        config.backend.output = "focus".into();
        config.aishe.yolo_verbose = false;
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            for expect in ["compact", "detailed", "focus"] {
                let reply = handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    "SLASH\task\t/tmp\t/details",
                );
                assert_eq!(reply, "OK");
                assert_eq!(config.backend.output, expect);
                assert_eq!(config.aishe.yolo_verbose, expect == "detailed");
                let shown = pty.take_capture();
                assert!(
                    shown.contains(&format!("details: {expect}")),
                    "pty missing density: {shown:?}"
                );
            }
        });
    }

    #[test]
    fn skills_and_mcp_slashes_list_names_not_just_counts() {
        let mut config = test_config();
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
                "SLASH\task\t/tmp\t/skills",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("skills"),
                "skills slash must list names header: {shown:?}"
            );
            assert!(
                shown.contains("aishe-product") || shown.contains("(none loaded)"),
                "expected skill names or empty marker: {shown:?}"
            );

            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/mcp",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("mcp"),
                "mcp slash must list servers/tools: {shown:?}"
            );
            assert!(
                shown.contains("none configured") || shown.contains("mcp servers"),
                "mcp must name servers or say none: {shown:?}"
            );
        });
    }

    #[test]
    fn cycle_details_density_order_is_focus_compact_detailed() {
        let mut cur = "focus".to_string();
        assert_eq!(cycle_details_density(&mut cur), "compact");
        assert_eq!(cycle_details_density(&mut cur), "detailed");
        assert_eq!(cycle_details_density(&mut cur), "focus");
    }

    #[test]
    fn backend_slash_documents_opt_in_heavy_no_auto() {
        let mut config = test_config();
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
                "SLASH\task\t/tmp\t/backend",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture().to_ascii_lowercase();
            assert!(
                shown.contains("opt-in") && shown.contains("legacy"),
                "expected heavy opt-in docs, got {shown:?}"
            );
            assert!(
                shown.contains("opencode") || shown.contains("heavy"),
                "expected backend names, got {shown:?}"
            );
        });
    }

    #[test]
    fn custom_markdown_slash_runs_shell_true_via_fifo() {
        let home = std::env::temp_dir().join(format!(
            "aishe-lean-custom-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let cmds = home.join("aishe").join("commands");
        std::fs::create_dir_all(&cmds).unwrap();
        std::fs::write(
            cmds.join("leanping.md"),
            "---\ndescription: lean custom\nshell: true\n---\ntrue\n",
        )
        .unwrap();
        let prev_cfg = std::env::var_os("AISHE_CONFIG_DIR");
        let prev_xdg = std::env::var_os("XDG_CONFIG_HOME");
        std::env::set_var("AISHE_CONFIG_DIR", &home);
        std::env::set_var("XDG_CONFIG_HOME", &home);

        let mut config = test_config();
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
                "SLASH\task\t/tmp\t/commands",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("/leanping"),
                "commands list must include /leanping: {shown:?}"
            );

            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/leanping",
            );
            assert!(
                reply.starts_with("RAN") || reply.starts_with("CONFIRM_B64"),
                "expected RAN/CONFIRM for shell custom, got {reply}"
            );
        });

        match prev_cfg {
            Some(v) => std::env::set_var("AISHE_CONFIG_DIR", v),
            None => std::env::remove_var("AISHE_CONFIG_DIR"),
        }
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&home);
    }

}
