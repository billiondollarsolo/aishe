//! In-process NL turn: warm provider HTTP, never OpenCode.
//!
//! Lean IPC replies are **control** (`OK` / `STREAM_END` / `FILL_B64` /
//! `CONFIRM_B64` / `RAN` / `ERROR`). Token streams and multi-line answers are
//! written by the parent onto terminal output via [`PtyOut`] as they arrive;
//! the FIFO stays control-only (never carries answer body).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use crate::commands::CommandRegistry;
use crate::config::Config;
use crate::executor::Executor;
use crate::modes;
use crate::modes::suggest::Suggestion;
use crate::providers::{self, Provider};
use crate::safety::{self, Risk};
use crate::session::Session;
use crate::skills::SkillRegistry;

use super::grant::{
    ensure_session_grant, validate_scope, workspace_grant_root, LeanGrant, LeanMode, SessionGrants,
};
use super::pty_out::PtyOut;
use super::sessions::LeanSessionStore;
use super::stdout_redirect::StdoutRedirect;

/// Load local registries once per live lean shell; connect MCP only when an
/// agent turn or explicit `/mcp` discovery needs it.
#[derive(Default)]
pub struct LeanWarm {
    pub skills: Option<SkillRegistry>,
    pub mcp: Option<crate::mcp::McpRegistry>,
    pub commands: Option<CommandRegistry>,
    pub grants: SessionGrants,
    // Per-shell accounting survives provider replacement and conversation
    // resets. Attribute each delta to the connection and model that billed it.
    usage: BTreeMap<(String, String), crate::usage::Usage>,
}

impl LeanWarm {
    /// Record a provider-meter delta after a request. Callers must supply only
    /// newly metered usage, never the provider's cumulative snapshot.
    pub fn record_usage(&mut self, usage: crate::usage::Usage, model: &str, connection_id: &str) {
        if usage.is_empty() {
            return;
        }
        let total = self
            .usage
            .entry((connection_id.to_string(), model.to_string()))
            .or_default();
        total.input = total.input.saturating_add(usage.input);
        total.output = total.output.saturating_add(usage.output);
        total.requests = total.requests.saturating_add(usage.requests);
    }

    /// Include recorded standalone CLI calls made inside this shell when usage
    /// is explicitly inspected. An unreadable tally leaves memory intact.
    pub fn replace_usage_from_log(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        self.usage.clear();
        for entry in crate::usagelog::parse_entries(&text) {
            self.record_usage(
                entry.usage,
                &entry.model,
                entry.connection_id.as_deref().unwrap_or("legacy/unknown"),
            );
        }
    }

    /// Whole-shell usage, priced with each billed model rather than the active
    /// model. Unknown prices remain visible instead of becoming a zero cost.
    pub fn usage_summary(&self, config: &Config) -> Option<String> {
        self.usage_summary_for_connection(config, None)
    }

    /// Carry known session spend across provider replacements. The native
    /// loop compares its own cumulative meter with this turn's threshold, so
    /// add that meter's baseline to the remaining shell allowance exactly once.
    fn budgeted_turn_config<'a>(
        &self,
        config: &'a Config,
        provider: Option<&dyn Provider>,
    ) -> Result<Cow<'a, Config>> {
        let budget = config.aishe.budget_usd;
        if budget <= 0.0 {
            return Ok(Cow::Borrowed(config));
        }
        let current_price = crate::usage::budget_price_for(config.active_model(), &config.pricing);
        let current_cost = current_price
            .zip(provider.map(|provider| provider.meter().snapshot()))
            .map(|(price, usage)| crate::usage::cost(usage, price))
            .unwrap_or(0.0);
        let spent = if self.usage.is_empty() {
            current_cost
        } else {
            self.usage
                .iter()
                .filter_map(|((_, model), usage)| {
                    crate::usage::budget_price_for(model, &config.pricing)
                        .map(|price| crate::usage::cost(*usage, price))
                })
                .sum()
        };
        let remaining = budget - spent;
        if remaining <= 0.0 {
            anyhow::bail!(
                "session budget reached (~${spent:.4} ≥ ${budget:.4}); raise budget_usd to continue"
            );
        }
        let mut turn = config.clone();
        if let Some(price) = current_price {
            // Give the existing native loop an exact price too, preventing its
            // display-price substring resolver from changing this threshold.
            turn.pricing.insert(config.active_model().into(), price);
            turn.aishe.budget_usd = current_cost + remaining;
        } else {
            // Unknown-price calls retain the existing unenforced behavior.
            // Their limitation is disclosed next to the configured budget.
            turn.aishe.budget_usd = 0.0;
        }
        Ok(Cow::Owned(turn))
    }

    fn budget_summary(&self, config: &Config) -> Option<String> {
        if config.aishe.budget_usd <= 0.0 {
            return None;
        }
        let unknown =
            crate::usage::budget_price_for(config.active_model(), &config.pricing).is_none()
                || self.usage.keys().any(|(_, model)| {
                    crate::usage::budget_price_for(model, &config.pricing).is_none()
                });
        Some(format!(
            "budget: ${:.2}{}",
            config.aishe.budget_usd,
            if unknown {
                " · unknown model prices cannot be enforced"
            } else {
                ""
            },
        ))
    }

    fn usage_summary_for_connection(
        &self,
        config: &Config,
        connection_id: Option<&str>,
    ) -> Option<String> {
        let mut total = crate::usage::Usage::default();
        let mut total_cost = 0.0;
        let mut unpriced = 0u64;
        for ((connection, model), usage) in &self.usage {
            if connection_id.is_some_and(|id| id != connection) {
                continue;
            }
            total.input = total.input.saturating_add(usage.input);
            total.output = total.output.saturating_add(usage.output);
            total.requests = total.requests.saturating_add(usage.requests);
            match crate::usage::price_for(model, &config.pricing) {
                Some(price) => total_cost += crate::usage::cost(*usage, price),
                None => unpriced = unpriced.saturating_add(usage.requests),
            }
        }
        if total.is_empty() {
            return None;
        }
        let unpriced_requests = format!(
            "{unpriced} unpriced req{}",
            if unpriced == 1 { "" } else { "s" },
        );
        let cost = if unpriced == 0 {
            format!("~${total_cost:.4}")
        } else if total_cost > 0.0 {
            format!("~${total_cost:.4} (+{unpriced_requests})")
        } else {
            format!("cost n/a ({unpriced_requests})")
        };
        Some(format!(
            "{} in · {} out · {} req{} · {cost}",
            crate::usage::group(total.input),
            crate::usage::group(total.output),
            total.requests,
            if total.requests == 1 { "" } else { "s" }
        ))
    }

    fn used_multiple_connections(&self) -> bool {
        let mut connections = self.usage.keys().map(|(connection, _)| connection);
        let first = connections.next();
        connections.any(|connection| Some(connection) != first)
    }

    /// Warm all registries for an agent turn. Local slash commands use the
    /// narrower helpers so inspecting this shell never starts MCP servers.
    pub fn ensure(&mut self, config: &Config) {
        self.ensure_local();
        self.ensure_mcp(config);
    }

    fn ensure_local(&mut self) {
        self.ensure_skills();
        self.ensure_commands();
    }

    fn ensure_skills(&mut self) {
        if self.skills.is_none() {
            self.skills = Some(SkillRegistry::load());
        }
    }

    fn ensure_mcp(&mut self, config: &Config) {
        if self.mcp.is_none() {
            // Empty/disabled config → empty registry; never touches OpenCode.
            self.mcp = Some(crate::mcp::McpRegistry::deferred(&config.mcp_servers));
        }
    }

    fn ensure_commands(&mut self) {
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
        self.commands.as_ref().map(|c| c.list()).unwrap_or_default()
    }

    pub fn skills_len(&self) -> usize {
        self.skills.as_ref().map(|s| s.len()).unwrap_or(0)
    }

    pub fn mcp_tool_len(&self) -> usize {
        self.mcp
            .as_ref()
            .filter(|m| !m.is_deferred())
            .map(|m| m.list().len())
            .unwrap_or(0)
    }

    pub fn mcp_server_hint(&self, config: &Config) -> String {
        let configured = config.mcp_servers.iter().filter(|(_, c)| c.enabled).count();
        if configured == 0 {
            "none configured".into()
        } else if self.mcp.as_ref().is_none_or(|m| m.is_deferred()) {
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

/// Publish safe names and descriptions for lean completion (`AISHE_LEAN_CMDS_FILE`).
fn refresh_custom_cmds_file(commands: Option<&CommandRegistry>) {
    let Ok(path) = std::env::var("AISHE_LEAN_CMDS_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let _ = std::fs::write(path, command_completion_text(commands));
}

pub(super) fn command_completion_text(commands: Option<&CommandRegistry>) -> String {
    let mut body = commands
        .map(|c| {
            c.list()
                .into_iter()
                .filter(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                })
                .map(|(name, description)| {
                    let description: String = crate::commands::display_safe(&description)
                        .chars()
                        .take(96)
                        .collect();
                    format!("{name}\t{description}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if !body.is_empty() {
        body.push('\n');
    }
    body
}

/// `-c` / hook NL entry used when lean is on. Skips `backend::supervisor`.
#[allow(clippy::too_many_arguments)]
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
        LeanMode::Agent => {
            modes::yolo::run(
                &nl,
                provider,
                executor,
                config,
                &crate::agent::controller::INTERRUPTED,
                skills,
                mcp,
                session,
            )?;
        }
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
    let root = if mode == LeanMode::Agent && config.backend.default_scope != "host" {
        workspace_grant_root(executor.cwd()).unwrap_or_else(|| executor.cwd().clone())
    } else {
        executor.cwd().clone()
    };
    prepare_scoped_executor(executor, config, mode, &root)
}

fn prepare_scoped_executor(
    executor: &mut Executor,
    config: &Config,
    mode: LeanMode,
    workspace: &Path,
) -> Result<()> {
    if mode == LeanMode::Agent {
        return crate::agent::native::prepare_executor(executor, config, workspace);
    }
    executor.prefer_posix_capture();
    executor.set_sandbox_wrap(Vec::new());
    executor.set_lean_scope(None);
    Ok(())
}

/// FIFO request from the PTY child. Returns a single-line **control** reply.
#[allow(clippy::too_many_arguments)]
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
        "COMMANDS" => {
            warm.ensure_commands();
            "OK".into()
        }
        "MODE_CHECK" | "MODE_ACCEPT" => {
            let (word, cwd) = rest.split_once('\t').unwrap_or((rest, ""));
            let (mode, host) = match word.trim().to_ascii_lowercase().as_str() {
                "ask" | "suggest" => (LeanMode::Ask, config.backend.default_scope == "host"),
                "allow" | "auto" => (LeanMode::Allow, config.backend.default_scope == "host"),
                "agent" | "yolo" => (LeanMode::Agent, false),
                "agent-host" => (LeanMode::Agent, true),
                _ => return "ERROR\tmode must be ask, allow, agent, or agent-host".into(),
            };
            let cwd = Path::new(cwd);
            if let Err(error) = validate_scope(config, mode, host, cwd) {
                return format!("ERROR\t{}", one_line(&error.to_string()));
            }
            if op == "MODE_CHECK" {
                return if warm.grants.accepted(mode, host, cwd) {
                    "ACCEPTED".into()
                } else {
                    "GRANT_REQUIRED".into()
                };
            }
            if let Err(error) = warm.grants.accept(mode, host, cwd) {
                return format!("ERROR\t{}", one_line(&error.to_string()));
            }
            if mode == LeanMode::Agent {
                config.backend.default_scope = if host { "host" } else { "workspace" }.into();
            }
            config.aishe.mode = mode.as_str().into();
            format!(
                "MODE_OK\t{}\t{}\t{}",
                mode.as_str(),
                config.backend.default_scope,
                warm.grants
                    .workspace_root()
                    .map(|root| root.display().to_string())
                    .unwrap_or_default()
            )
        }
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
                "FIX" => handle_fix(config, provider, executor, session, store, warm, pty, line),
                _ => handle_slash(
                    config, provider, executor, session, store, warm, pty, mode, cwd, line,
                ),
            }
        }
        "CONFIRM_YES" => run_confirmed(executor, rest.trim(), pty),
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

#[allow(clippy::too_many_arguments)]
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
        return explain_last_failure(config, provider, executor, session, store, warm, pty);
    }
    if mode != LeanMode::Ask {
        let host = config.backend.default_scope == "host";
        let workspace = if host {
            executor.cwd().to_path_buf()
        } else {
            warm.grants
                .workspace_root()
                .unwrap_or(executor.cwd())
                .to_path_buf()
        };
        if let Err(error) = validate_scope(config, mode, host, &workspace) {
            return format!("ERROR\t{}", one_line(&error.to_string()));
        }
        if !warm.grants.accepted(mode, host, executor.cwd()) {
            return format!(
                "ERROR\t{} requires a grant for this shell and workspace; use /mode {}",
                mode.as_str(),
                if mode == LeanMode::Agent && host {
                    "agent-host"
                } else {
                    mode.as_str()
                }
            );
        }
        if let Err(error) = prepare_scoped_executor(executor, config, mode, &workspace) {
            return format!("ERROR\t{}", one_line(&error.to_string()));
        }
    } else {
        executor.set_sandbox_wrap(Vec::new());
        executor.set_lean_scope(None);
    }
    let turn_config = match warm.budgeted_turn_config(config, provider.as_deref()) {
        Ok(config) => config,
        Err(error) => return format!("ERROR\t{}", one_line(&error.to_string())),
    };
    let config = turn_config.as_ref();
    if provider.is_none() {
        *provider = providers::make(config).ok();
    }
    let Some(provider_ref) = provider.as_deref() else {
        return "ERROR\tno provider configured (set an API key, or AISHE_FAKE_LLM for tests)"
            .into();
    };
    let cwd_path = if cwd.is_empty() {
        executor.cwd().to_path_buf()
    } else {
        PathBuf::from(cwd)
    };
    let expanded = prepare_nl_prompt(line, &cwd_path, config);
    let reply = match mode {
        LeanMode::Ask => suggest_reply(
            &expanded,
            provider_ref,
            executor,
            config,
            session,
            pty,
            false,
        ),
        LeanMode::Allow => suggest_reply(
            &expanded,
            provider_ref,
            executor,
            config,
            session,
            pty,
            true,
        ),
        LeanMode::Agent => agent_reply(
            &expanded,
            provider_ref,
            executor,
            config,
            session,
            warm,
            pty,
        ),
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

#[allow(clippy::too_many_arguments)]
fn explain_last_failure(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &LeanWarm,
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
    let turn_config = match warm.budgeted_turn_config(config, provider.as_deref()) {
        Ok(config) => config,
        Err(error) => return format!("ERROR\t{}", one_line(&error.to_string())),
    };
    let config = turn_config.as_ref();
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

#[allow(clippy::too_many_arguments)]
fn handle_fix(
    config: &Config,
    provider: &mut Option<Arc<dyn Provider>>,
    executor: &mut Executor,
    session: &mut Session,
    store: &mut Option<LeanSessionStore>,
    warm: &LeanWarm,
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
    let turn_config = match warm.budgeted_turn_config(config, provider.as_deref()) {
        Ok(config) => config,
        Err(error) => return format!("ERROR\t{}", one_line(&error.to_string())),
    };
    let config = turn_config.as_ref();
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
    let result = modes::suggest::request(&prompt, provider_ref, executor, config, Vec::new());
    if executor.is_cancelled() {
        return "CANCELLED".into();
    }
    match result {
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
            emit_answer(pty, &explanation);
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
    let result = modes::suggest::request(line, provider, executor, config, session.history());
    if executor.is_cancelled() {
        return "CANCELLED".into();
    }
    match result {
        Ok(suggestion) => match suggestion {
            Suggestion::Answer { explanation } => {
                session.record_user(line);
                session.record_assistant(&explanation);
                emit_answer(pty, &explanation);
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
                        Risk::Safe => run_now(executor, &command, pty),
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
    let mut out = CancellablePtyWrite {
        inner: super::PtyWrite::new(pty),
        executor,
    };
    let result = modes::suggest::request_streamed(
        line,
        provider,
        executor,
        config,
        session.history(),
        &mut out,
    );
    if executor.is_cancelled() {
        return "CANCELLED".into();
    }
    match result {
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
                        Risk::Safe => run_now(executor, &command, pty),
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

struct CancellablePtyWrite<'a> {
    inner: super::PtyWrite<'a>,
    executor: &'a Executor,
}

impl std::io::Write for CancellablePtyWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.executor.is_cancelled() {
            return Ok(buf.len());
        }
        std::io::Write::write(&mut self.inner, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.inner)
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
    warm.ensure_local();
    let denied_network = executor
        .lean_scope()
        .is_some_and(|(_, _, network)| *network == crate::agent::NetworkPolicy::Deny);
    let empty_mcp = crate::mcp::McpRegistry::connect(&std::collections::BTreeMap::new());
    if !denied_network {
        warm.ensure_mcp(config);
    }
    if executor.is_cancelled() {
        return "CANCELLED".into();
    }
    let skills = warm.skills.as_ref().expect("skills warmed");
    let mcp = if denied_network {
        &empty_mcp
    } else {
        warm.mcp.as_ref().expect("mcp warmed")
    };
    let capabilities = crate::ui::TerminalCapabilities::detect_stdout();
    let _redirect = StdoutRedirect::to_pty(pty.clone());
    match modes::yolo::run_with_terminal(
        line,
        provider,
        executor,
        config,
        &crate::agent::controller::INTERRUPTED,
        skills,
        mcp,
        session,
        capabilities,
    ) {
        Ok(outcome) => match outcome.state {
            crate::agent::native::NativeTurnState::Completed => "RAN".into(),
            crate::agent::native::NativeTurnState::Cancelled => "CANCELLED".into(),
            crate::agent::native::NativeTurnState::HandedOff => {
                let receipt = outcome
                    .detail
                    .as_deref()
                    .unwrap_or("Task moved to the background; open /tasks to view it.");
                emit_text(pty, &format!("\n{receipt}"));
                "RAN".into()
            }
            _ => format!(
                "ERROR\t{}",
                one_line(
                    outcome
                        .detail
                        .as_deref()
                        .unwrap_or("native task did not complete")
                )
            ),
        },
        Err(error) => format!("ERROR\t{}", one_line(&error.to_string())),
    }
}

#[allow(clippy::too_many_arguments)]
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
            warm.ensure_commands();
            emit_lean_help(warm, pty, name == "/commands" || arg == "all", arg);
            "OK".into()
        }
        "/status" => {
            warm.ensure_local();
            let sid = store
                .as_ref()
                .map(|s| s.id().to_string())
                .unwrap_or_else(|| "-".into());
            let conn = config.active_connection_id();
            let host = config.backend.default_scope == "host";
            let scope = match mode {
                LeanMode::Ask => "none (suggestions)",
                LeanMode::Allow => "host (dangerous commands require yes)",
                LeanMode::Agent if host => "host",
                LeanMode::Agent => "workspace",
            };
            let grant = if mode == LeanMode::Ask {
                "not needed"
            } else if mode == LeanMode::Agent && host && !config.sandbox.allow_host_yolo {
                "blocked by policy"
            } else if warm.grants.accepted(mode, host, executor.cwd()) {
                "accepted"
            } else {
                "required"
            };
            emit_text(
                pty,
                &format!(
                    "lean {} · mode {} · scope {} · grant {}",
                    env!("CARGO_PKG_VERSION"),
                    mode.as_str(),
                    scope,
                    grant,
                ),
            );
            emit_text(
                pty,
                &format!(
                    "connection {} · model {} · details {}",
                    crate::commands::display_safe(conn),
                    crate::commands::display_safe(config.active_model()),
                    crate::commands::display_safe(&config.backend.output),
                ),
            );
            emit_text(
                pty,
                &format!("session {}", crate::commands::display_safe(&sid)),
            );
            let usage = lean_usage_summary(warm, provider.as_deref(), config)
                .unwrap_or_else(|| "no model calls yet this session".into());
            emit_text(pty, &format!("usage: {usage}"));
            if let Some(budget) = warm.budget_summary(config) {
                emit_text(pty, &budget);
            }
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
            let msg = lean_usage_summary(warm, provider.as_deref(), config)
                .map(|summary| format!("usage: {summary}"))
                .unwrap_or_else(|| "usage: no model calls yet this session".into());
            emit_text(pty, &msg);
            if warm.used_multiple_connections() {
                let conn = config.active_connection_id();
                let summary = warm
                    .usage_summary_for_connection(config, Some(conn))
                    .unwrap_or_else(|| "no model calls yet".into());
                emit_text(
                    pty,
                    &format!(
                        "active connection {}: {summary}",
                        crate::commands::display_safe(conn)
                    ),
                );
            }
            if let Some(budget) = warm.budget_summary(config) {
                emit_text(pty, &budget);
            }
            "OK".into()
        }
        "/sessions" => handle_sessions_slash(session, store, pty, arg),
        "/details" => {
            let next = cycle_details_density(&mut config.backend.output);
            config.aishe.yolo_verbose = next == "detailed";
            emit_text(pty, &format!("details: {next} (this shell)"));
            "OK".into()
        }
        "/skills" => {
            warm.ensure_skills();
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
            warm.ensure_mcp(config);
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
            config, provider, executor, session, store, warm, pty, mode, cwd, name, &rest_args,
        ),
    }
}

fn emit_lean_help(warm: &LeanWarm, pty: &PtyOut, all_commands: bool, topic: &str) {
    if !all_commands && !topic.is_empty() {
        if topic == "keys" {
            emit_text(pty, "AIShe · keys");
            emit_text(pty, "  / then Tab    browse commands; Tab cycles matches");
            emit_text(
                pty,
                "  ?             ask the AI; empty ? explains the last failure",
            );
            emit_text(pty, "  !             run a shell line directly");
            emit_text(pty, "  Shift-Tab     cycle modes on empty input");
            emit_text(
                pty,
                "  Ctrl-O        cycle output detail without losing input",
            );
            emit_text(
                pty,
                "  Ctrl-C        cancel work or clear the current input",
            );
            emit_text(pty, "  Ctrl-X ?      explain the current route");
            emit_text(
                pty,
                "  Ctrl-X b      view background work and keep your input",
            );
            emit_text(pty, "  Ctrl-X Ctrl-F suggest a fix for the last failure");
        } else if let Some(command) = super::slash::find(topic) {
            emit_text(pty, &format!("AIShe · /{}", command.name));
            emit_text(pty, &format!("  {}", command.usage));
            emit_text(pty, &format!("  {}", command.detail));
        } else if let Some(custom) = warm
            .commands
            .as_ref()
            .and_then(|commands| commands.get(topic.trim_start_matches('/')))
        {
            emit_text(
                pty,
                &format!("AIShe · /{}", crate::commands::display_safe(&custom.name)),
            );
            emit_text(
                pty,
                &format!("  {}", crate::commands::display_safe(&custom.description)),
            );
            emit_text(
                pty,
                if custom.shell {
                    "  Custom shell command; existing safety and project trust rules apply."
                } else {
                    "  Custom AI request; uses this shell's selected mode and connection."
                },
            );
        } else {
            emit_text(
                pty,
                &format!(
                    "No help for {}. Use /commands or /help keys.",
                    crate::commands::display_safe(topic)
                ),
            );
        }
        return;
    }

    emit_text(
        pty,
        if all_commands {
            "AIShe · commands"
        } else {
            "AIShe · quick guide"
        },
    );
    emit_text(
        pty,
        "Type / then Tab to browse. /help <command> for details.",
    );
    for group in super::slash::GROUPS {
        if all_commands {
            emit_text(pty, &format!("\n{group}"));
            for command in super::slash::COMMANDS
                .iter()
                .filter(|command| command.group == *group)
            {
                emit_text(pty, &format!("  /{:<12} {}", command.name, command.summary));
            }
        } else {
            let names = super::slash::COMMANDS
                .iter()
                .filter(|command| command.group == *group && command.name != "help")
                .map(|command| format!("/{}", command.name))
                .collect::<Vec<_>>()
                .join(" ");
            emit_text(pty, &format!("  {group:<8} {names}"));
        }
    }
    if !all_commands {
        emit_text(
            pty,
            "  ? ask · ! shell · Shift-Tab mode · Ctrl-O details · Ctrl-C cancel",
        );
        emit_text(
            pty,
            "Ask proposes commands; allow and agent require a grant for this shell.",
        );
        emit_text(pty, "More: /commands · /help keys · /tour");
    }
    let list = warm.command_list();
    if list.is_empty() {
        if !all_commands {
            return;
        }
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
        for (cname, desc) in list.iter().take(if all_commands { 64 } else { 6 }) {
            let d = if desc.is_empty() {
                String::new()
            } else {
                format!(" — {}", crate::commands::display_safe(desc))
            };
            emit_text(
                pty,
                &format!("  /{}{d}", crate::commands::display_safe(cname)),
            );
        }
        let displayed = if all_commands { 64 } else { 6 };
        if list.len() > displayed {
            emit_text(
                pty,
                &format!("  … +{} more · /commands", list.len() - displayed),
            );
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

#[allow(clippy::too_many_arguments)]
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
    warm.ensure_commands();
    let Some(cmd) = warm
        .commands
        .as_ref()
        .and_then(|reg| reg.get(bare))
        .cloned()
    else {
        let suggestion = crate::fuzzy::correction(
            bare,
            super::slash::COMMANDS.iter().map(|command| command.name),
            2,
        );
        return match suggestion {
            Some(suggestion) => format!(
                "ERROR\tunknown slash {name}. Try /{suggestion}; / then Tab lists commands."
            ),
            None => format!("ERROR\tunknown slash {name}. Use /commands or / then Tab."),
        };
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
            Risk::Safe => run_now(executor, &ex.text, pty),
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
    crate::capabilities::cached_models(config, config.active_connection_id())
        .unwrap_or_else(|_| vec![config.active_model().to_string()])
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
        let mut out =
            String::from("connections (* active; Grok subscription remains default happy path):");
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
                for meta in listed
                    .iter()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                {
                    out.push('\n');
                    out.push_str(&format!(
                        "  {}  turns={}  {}",
                        crate::commands::display_safe(&meta.id),
                        meta.turns,
                        crate::commands::display_safe(if meta.title.is_empty() {
                            meta.cwd.as_str()
                        } else {
                            meta.title.as_str()
                        })
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
            let id = id
                .strip_prefix("resume")
                .unwrap_or(id)
                .trim_matches(':')
                .trim();
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
            format!(
                "{line}\n\n[attachment error: {}]",
                one_line(&error.to_string())
            )
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

fn run_confirmed(executor: &mut Executor, command: &str, pty: &PtyOut) -> String {
    let decoded = decode_confirm_payload(command);
    let command = decoded
        .split(" (")
        .next()
        .unwrap_or(decoded.as_str())
        .trim();
    if command.is_empty() {
        return "ERROR\tnothing to run".into();
    }
    run_now(executor, command, pty)
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

fn run_now(executor: &mut Executor, command: &str, pty: &PtyOut) -> String {
    // Command stdout uses the same raw-terminal newline handling as answers.
    let _redirect = StdoutRedirect::to_pty(pty.clone());
    let code = executor.run(command);
    if executor.is_cancelled() {
        "CANCELLED".into()
    } else {
        format!("RAN\texit {code}")
    }
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

fn emit_answer(pty: &PtyOut, text: &str) {
    if !text.trim().is_empty() {
        let capabilities = crate::ui::TerminalCapabilities::detect_stdout();
        pty.write_user_line(&capabilities.assistant_answer_header());
        pty.write_user_line(text);
    }
}

fn lean_usage_summary(
    warm: &LeanWarm,
    provider: Option<&dyn Provider>,
    config: &Config,
) -> Option<String> {
    warm.usage_summary(config).or_else(|| {
        // Direct callers may not have an IPC request boundary to record into
        // the shell ledger. Fall back only while the ledger is still empty.
        let usage = provider?.meter().snapshot();
        (!usage.is_empty())
            .then(|| crate::usage::summary(usage, config.active_model(), &config.pricing))
    })
}

fn b64(text: &str) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, text.as_bytes())
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
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
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
            assert!(
                reply == "OK" || reply == "STREAM_END",
                "expected OK/STREAM_END, got {reply}"
            );
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
    fn usage_survives_model_connection_and_conversation_changes() {
        let _guard = fake_llm_lock();
        std::env::set_var("AISHE_FAKE_LLM", "unused");
        let mut config = test_config();
        config.set_active_model("metered-a".into());
        config.pricing.insert(
            "metered-a".into(),
            crate::usage::Price {
                input: 1.0,
                output: 2.0,
            },
        );
        config.pricing.insert(
            "metered-b".into(),
            crate::usage::Price {
                input: 4.0,
                output: 8.0,
            },
        );
        let original_connection = config.active_connection_id().to_string();
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        session.record_user("conversation to reset");
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            warm.record_usage(
                crate::usage::Usage {
                    input: 1_000_000,
                    output: 1_000_000,
                    requests: 1,
                },
                "metered-a",
                &original_connection,
            );
            for slash in ["/model metered-b", "/connection openai", "/reset"] {
                let reply = handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    &format!("SLASH\task\t/tmp\t{slash}"),
                );
                assert_eq!(reply, "OK", "{slash}: {reply}");
            }
            warm.record_usage(
                crate::usage::Usage {
                    input: 2_000_000,
                    output: 0,
                    requests: 1,
                },
                "metered-b",
                config.active_connection_id(),
            );
            assert!(session.history().is_empty());
            let _ = pty.take_capture();
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
                shown.contains("3,000,000 in · 1,000,000 out · 2 reqs · ~$11.0000"),
                "{shown}"
            );
            assert!(
                shown.contains("active connection openai: 2,000,000 in · 0 out · 1 req · ~$8.0000"),
                "{shown}"
            );
        });
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn mixed_usage_discloses_unpriced_requests_without_repricing_old_models() {
        let mut config = test_config();
        config.pricing.insert(
            "priced".into(),
            crate::usage::Price {
                input: 2.0,
                output: 3.0,
            },
        );
        config.set_active_model("unknown".into());
        let mut warm = LeanWarm::default();
        warm.record_usage(
            crate::usage::Usage {
                input: 1_000_000,
                output: 0,
                requests: 1,
            },
            "priced",
            "work",
        );
        warm.record_usage(
            crate::usage::Usage {
                input: 50,
                output: 5,
                requests: 3,
            },
            "unknown",
            "work",
        );
        let summary = warm.usage_summary(&config).expect("usage recorded");
        assert!(
            summary.contains("4 reqs · ~$2.0000 (+3 unpriced reqs)"),
            "{summary}"
        );
        assert!(summary.contains("1,000,050 in · 5 out"), "{summary}");
    }

    #[test]
    fn status_reports_scope_grant_policy_density_and_shell_usage() {
        let mut config = test_config();
        config.backend.default_scope = "host".into();
        config.backend.output = "compact".into();
        config.aishe.budget_usd = 12.5;
        config.sandbox.allow_host_yolo = false;
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            warm.grants
                .accept(LeanMode::Agent, true, executor.cwd())
                .expect("host grant");
            warm.record_usage(
                crate::usage::Usage {
                    input: 10,
                    output: 5,
                    requests: 1,
                },
                "unknown",
                "previous-connection",
            );
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\tagent\t/tmp\t/status",
            );
            assert_eq!(reply, "OK");
            let shown = pty.take_capture();
            assert!(
                shown.contains("mode agent · scope host · grant blocked by policy"),
                "{shown}"
            );
            assert!(shown.contains("details compact"), "{shown}");
            assert!(shown.contains("usage: 10 in · 5 out · 1 req"), "{shown}");
            assert!(shown.contains("budget: $12.50"), "{shown}");
            assert!(provider.is_none(), "status initialized a provider");
            assert!(warm.mcp.is_none(), "status initialized MCP");
        });
    }

    #[test]
    fn session_budget_cannot_be_reset_by_model_or_connection_swaps() {
        let _guard = fake_llm_lock();
        std::env::set_var("AISHE_FAKE_LLM", "budget fixture answer");
        std::env::set_var("AISHE_FAKE_USAGE", "1000000,0");
        let mut config = test_config();
        config.aishe.budget_usd = 2.0;
        config.set_active_model("budget-a".into());
        for model in ["budget-a", "budget-b"] {
            config.pricing.insert(
                model.into(),
                crate::usage::Price {
                    input: 1.0,
                    output: 1.0,
                },
            );
        }
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            let first = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\tfirst call",
            );
            assert_eq!(first, "STREAM_END");
            warm.record_usage(
                provider.as_ref().unwrap().meter().snapshot(),
                config.active_model(),
                config.active_connection_id(),
            );
            for slash in ["/model budget-b", "/connection openai", "/model budget-b"] {
                assert_eq!(
                    handle_ipc_line(
                        &mut config,
                        &mut provider,
                        &mut executor,
                        &mut session,
                        store,
                        &mut warm,
                        &pty,
                        &format!("SLASH\task\t/tmp\t{slash}"),
                    ),
                    "OK"
                );
            }
            let second = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\tsecond call",
            );
            assert_eq!(second, "STREAM_END");
            let baseline = provider.as_ref().unwrap().meter().snapshot();
            warm.record_usage(
                baseline,
                config.active_model(),
                config.active_connection_id(),
            );
            let blocked = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\tthird call",
            );
            assert!(
                blocked.starts_with("ERROR\tsession budget reached"),
                "{blocked}"
            );
            assert_eq!(provider.as_ref().unwrap().meter().snapshot(), baseline);
            assert_eq!(config.aishe.budget_usd, 2.0);
            assert_eq!(
                handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    "SLASH\task\t/tmp\t/model unknown",
                ),
                "OK"
            );
            let blocked = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\task\t/tmp\tunknown model bypass",
            );
            assert!(
                blocked.starts_with("ERROR\tsession budget reached"),
                "{blocked}"
            );
            assert!(provider.as_ref().unwrap().meter().snapshot().is_empty());
        });
        std::env::remove_var("AISHE_FAKE_LLM");
        std::env::remove_var("AISHE_FAKE_USAGE");
    }

    #[test]
    fn native_agent_loop_uses_remaining_shell_budget_without_double_counting_meter() {
        let _guard = fake_llm_lock();
        std::env::set_var("AISHE_FAKE_LLM", "budget fixture final");
        std::env::set_var("AISHE_FAKE_USAGE", "1000000,0");
        std::env::set_var("AISHE_FAKE_TOOL", "true");
        let mut config = test_config();
        config.backend.default_scope = "host".into();
        config.aishe.budget_usd = 10.0;
        config.aishe.stream = false;
        config.aishe.yolo_confirm = "never".into();
        config.aishe.yolo_confirm_dangerous = false;
        config.set_active_model("budget-current".into());
        for model in ["budget-previous", "budget-current"] {
            config.pricing.insert(
                model.into(),
                crate::usage::Price {
                    input: 1.0,
                    output: 1.0,
                },
            );
        }
        let mut provider = providers::make(&config).ok();
        provider.as_ref().unwrap().meter().record(1_000_000, 0);
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            std::env::set_var(
                "AISHE_DATA_DIR",
                std::env::var_os("AISHE_LEAN_SESSIONS").unwrap(),
            );
            let mut warm = LeanWarm::default();
            warm.grants
                .accept(LeanMode::Agent, true, executor.cwd())
                .unwrap();
            warm.record_usage(
                crate::usage::Usage {
                    input: 8_000_000,
                    output: 0,
                    requests: 8,
                },
                "budget-previous",
                "previous",
            );
            warm.record_usage(
                crate::usage::Usage {
                    input: 1_000_000,
                    output: 0,
                    requests: 1,
                },
                "budget-current",
                config.active_connection_id(),
            );
            let effective = warm
                .budgeted_turn_config(&config, provider.as_deref())
                .unwrap();
            assert_eq!(effective.aishe.budget_usd, 2.0);
            drop(effective);
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "NL\tagent\t/tmp\tspend one remaining call",
            );
            assert_eq!(reply, "ERROR\tSession cost budget is exhausted.");
            let usage = provider.as_ref().unwrap().meter().snapshot();
            assert_eq!(usage.requests, 2, "agent exceeded the remaining allowance");
            assert_eq!(usage.input, 2_000_000);
            assert_eq!(config.aishe.budget_usd, 10.0);
            std::env::remove_var("AISHE_DATA_DIR");
        });
        std::env::remove_var("AISHE_FAKE_LLM");
        std::env::remove_var("AISHE_FAKE_USAGE");
        std::env::remove_var("AISHE_FAKE_TOOL");
    }

    #[test]
    fn hard_budget_does_not_substitute_display_price_patterns_for_unknown_models() {
        let mut config = test_config();
        config.aishe.budget_usd = 1.0;
        config.set_active_model("custom-claude-sonnet".into());
        assert!(crate::usage::price_for(config.active_model(), &config.pricing).is_some());
        let warm = LeanWarm::default();
        let turn = warm.budgeted_turn_config(&config, None).unwrap();
        assert_eq!(turn.aishe.budget_usd, 0.0);
        assert!(warm
            .budget_summary(&config)
            .unwrap()
            .contains("unknown model prices cannot be enforced"));
        assert_eq!(config.aishe.budget_usd, 1.0);
    }

    #[test]
    fn exhausted_budget_blocks_failure_ai_but_keeps_local_status_available() {
        let _guard = fake_llm_lock();
        std::env::set_var("AISHE_FAKE_LLM", "must not be called");
        let mut config = test_config();
        config.aishe.budget_usd = 1.0;
        config.set_active_model("failure-budget".into());
        config.pricing.insert(
            "failure-budget".into(),
            crate::usage::Price {
                input: 1.0,
                output: 1.0,
            },
        );
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            std::env::set_var(
                "AISHE_DATA_DIR",
                std::env::var_os("AISHE_LEAN_SESSIONS").unwrap(),
            );
            std::env::set_var("AISHE_SHELL_ID", "budget-failure");
            std::env::set_var("AISHE_LAST_EXIT", "1");
            crate::failure::record_from_env("false").expect("failure fixture");
            let mut warm = LeanWarm::default();
            warm.record_usage(
                crate::usage::Usage {
                    input: 1_000_000,
                    output: 0,
                    requests: 1,
                },
                config.active_model(),
                config.active_connection_id(),
            );
            for request in ["NL\task\t/tmp\t?", "FIX\task\t/tmp\tfix"] {
                let reply = handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    request,
                );
                assert!(
                    reply.starts_with("ERROR\tsession budget reached"),
                    "{reply}"
                );
                assert!(
                    provider.is_none(),
                    "budget rejection initialized a provider"
                );
            }
            assert_eq!(
                handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    "SLASH\task\t/tmp\t/status",
                ),
                "OK"
            );
            assert!(provider.is_none());
            std::env::remove_var("AISHE_DATA_DIR");
            std::env::remove_var("AISHE_SHELL_ID");
            std::env::remove_var("AISHE_LAST_EXIT");
        });
        std::env::remove_var("AISHE_FAKE_LLM");
    }

    #[test]
    fn usage_inspection_imports_standalone_and_legacy_calls_without_double_counting() {
        let config = test_config();
        let path = std::env::temp_dir().join(format!(
            "aishe-lean-usage-import-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let mut warm = LeanWarm::default();
        warm.record_usage(
            crate::usage::Usage {
                input: 12,
                output: 4,
                requests: 1,
            },
            "unknown",
            "openai",
        );
        std::fs::write(
            &path,
            "v2\t12\t4\t1\tunknown\topenai\n10\t2\t1\tlegacy-model\n",
        )
        .unwrap();
        warm.replace_usage_from_log(&path);
        warm.replace_usage_from_log(&path);
        let summary = warm.usage_summary(&config).expect("usage imported");
        assert!(summary.contains("22 in · 6 out · 2 reqs"), "{summary}");
        assert!(warm.used_multiple_connections());
        let active = warm
            .usage_summary_for_connection(&config, Some("openai"))
            .unwrap();
        assert!(active.contains("12 in · 4 out · 1 req"), "{active}");
        std::fs::remove_file(&path).unwrap();
        warm.replace_usage_from_log(&path);
        assert_eq!(warm.usage_summary(&config), Some(summary));
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
            assert!(
                reply == "OK" || reply == "STREAM_END",
                "expected OK/STREAM_END, got {reply}"
            );
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
    fn status_warms_local_registries_without_connecting_mcp() {
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
            assert!(warm.commands.is_some());
            assert!(warm.mcp.is_none());
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
    fn local_slashes_defer_mcp_until_explicit_discovery() {
        let Some(python) = ["python3", "python"].into_iter().find(|name| {
            std::process::Command::new(name)
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        }) else {
            eprintln!("skipping: Python is required for the MCP subprocess fixture");
            return;
        };
        let marker = std::env::temp_dir().join(format!(
            "aishe-lean-mcp-starts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let server = r#"
import json, sys
with open(sys.argv[1], "a") as marker:
    marker.write("started\n")
for line in sys.stdin:
    message = json.loads(line)
    request_id = message.get("id")
    if request_id is None:
        continue
    method = message.get("method")
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "marker", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "echo", "description": "Echo fixture input",
                             "inputSchema": {"type": "object"}}]}
    elif method == "tools/call":
        result = {"content": [{"type": "text", "text": "fixture echo"}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}), flush=True)
"#;
        let mut config = test_config();
        config.mcp_servers.insert(
            "marker".into(),
            crate::config::McpServerConfig {
                command: Some(python.into()),
                args: vec![
                    "-u".into(),
                    "-c".into(),
                    server.into(),
                    marker.display().to_string(),
                ],
                env: Default::default(),
                url: None,
                headers: Default::default(),
                enabled: true,
            },
        );
        let mut provider = None;
        let mut executor = Executor::new().expect("executor");
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        with_store(|store| {
            let mut warm = LeanWarm::default();
            for slash in ["/help", "/commands", "/skills", "/status"] {
                let reply = handle_ipc_line(
                    &mut config,
                    &mut provider,
                    &mut executor,
                    &mut session,
                    store,
                    &mut warm,
                    &pty,
                    &format!("SLASH\task\t/tmp\t{slash}"),
                );
                assert_eq!(reply, "OK", "local slash {slash} failed");
                assert!(!marker.exists(), "{slash} started an MCP subprocess");
                assert!(warm.mcp.is_none());
                let shown = pty.take_capture();
                if slash == "/status" {
                    assert!(shown.contains("1 configured (not warmed)"), "{shown:?}");
                }
            }
            let reply = handle_ipc_line(
                &mut config,
                &mut provider,
                &mut executor,
                &mut session,
                store,
                &mut warm,
                &pty,
                "SLASH\task\t/tmp\t/aishe_missing_mcp_regression_command",
            );
            assert!(reply.starts_with("ERROR\tunknown slash"));
            assert!(!marker.exists(), "unknown slash started an MCP subprocess");
            assert!(
                provider.is_none(),
                "local inspection constructed a provider"
            );

            for _ in 0..2 {
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
                assert!(shown.contains("mcp__marker__echo"), "{shown:?}");
            }
            // The agent's full warm-up reuses the same connected registry.
            warm.ensure(&config);
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "started\n");
            let registry = warm.mcp.as_ref().expect("MCP discovered");
            let (_, output) = registry.call("mcp__marker__echo", &serde_json::json!({}));
            assert!(output.contains("fixture echo"), "{output:?}");
        });
        let _ = std::fs::remove_file(marker);
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
            assert!(
                reply == "OK" || reply == "STREAM_END",
                "expected OK/STREAM_END, got {reply}"
            );
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
            let header = crate::ui::TerminalCapabilities::detect_stdout().assistant_answer_header();
            assert_eq!(shown.matches(&header).count(), 1, "{shown:?}");
            assert_eq!(
                shown.matches("alpha beta gamma delta").count(),
                1,
                "{shown:?}"
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
    fn nonstream_answers_have_one_authorship_header_and_command_reasons_have_none() {
        let config = test_config();
        let mut executor = Executor::new().unwrap();
        let mut session = Session::new(true);
        let pty = PtyOut::capture();
        let provider = providers::fake::FakeProvider::new(
            r#"{"type":"answer","explanation":"the nonstream answer"}"#.into(),
        );
        assert_eq!(
            suggest_reply(
                "answer",
                &provider,
                &mut executor,
                &config,
                &mut session,
                &pty,
                true
            ),
            "OK"
        );
        let shown = pty.take_capture();
        let header = crate::ui::TerminalCapabilities::detect_stdout().assistant_answer_header();
        assert_eq!(shown.matches(&header).count(), 1, "{shown:?}");
        assert_eq!(
            shown.matches("the nonstream answer").count(),
            1,
            "{shown:?}"
        );
        let provider = providers::fake::FakeProvider::new(
            r#"{"type":"command","command":"true","explanation":"the command reason"}"#.into(),
        );
        assert!(suggest_reply(
            "command",
            &provider,
            &mut executor,
            &config,
            &mut session,
            &pty,
            true
        )
        .starts_with("RAN\texit 0"));
        let shown = pty.take_capture();
        assert!(!shown.contains(&header), "{shown:?}");
        assert_eq!(shown.matches("the command reason").count(), 1, "{shown:?}");
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
            assert!(warm.mcp.is_none(), "custom shell commands must stay local");
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
