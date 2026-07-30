//! Yolo mode: an agentic loop where the model drives `run_command` until it has
//! accomplished the task, then summarizes.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::style::Stylize;

use super::{render_markdown, run_command_tool, safety_gate, use_skill_tool, GateOutcome};
use crate::config::Config;
use crate::context;
use crate::executor::{Executor, DEFAULT_CAPTURE_TIMEOUT};
use crate::mcp::McpRegistry;
use crate::providers::{AssistantMsg, Completion, Msg, Provider, ResponseFormat};
use crate::sandbox::{self, Tier};
use crate::session::Session;
use crate::skills::SkillRegistry;

/// Run the yolo agentic loop for one user request, priming and recording session
/// memory so follow-up requests have context.
#[allow(clippy::too_many_arguments)]
pub fn run(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    mcp: &McpRegistry,
    session: &mut Session,
) -> Result<()> {
    // Optional reversible session: run the whole loop against a throwaway copy of
    // the working tree, then preview + confirm/apply at the end.
    let dry = DryRun::setup(executor, config)?;
    let history = session.history();
    let mut task = crate::tasks::Active::start(config, executor.cwd(), input);
    println!("  {}", format!("task {}", task.id()).dim());
    let outcome = run_loop(
        input, provider, executor, config, interrupt, skills, mcp, history, &mut task, false,
    );
    super::report_usage(provider, config);
    if let Some(d) = dry {
        d.finish(executor);
    }
    let final_text = outcome?;
    session.record_user(input);
    session.record_assistant(
        final_text
            .as_deref()
            .unwrap_or("(yolo task ended without a final summary)"),
    );
    Ok(())
}

/// A reversible yolo session: the loop runs against `staging` (a copy of the
/// working tree, bind-mounted at the real cwd under bubblewrap with a read-only
/// root and no network), and the changes are previewed + applied/discarded at the
/// end. Created by [`DryRun::setup`] when `yolo_dry_run` is on and bwrap is present.
struct DryRun {
    real_cwd: std::path::PathBuf,
    staging: std::path::PathBuf,
}

impl DryRun {
    /// Set up the staging copy and point the executor at it, or `None` when the
    /// feature is off. Fail closed if the requested isolation is unavailable.
    fn setup(executor: &mut Executor, config: &Config) -> Result<Option<DryRun>> {
        if !config.aishe.yolo_dry_run {
            return Ok(None);
        }
        let state = crate::dependencies::bubblewrap_probe();
        if !matches!(state, crate::dependencies::BubblewrapState::Usable { .. }) {
            anyhow::bail!(
                "yolo_dry_run requires functional bubblewrap and will not execute \
                 without its preview sandbox; current state: {state:?}. Run `aishe doctor` \
                 or `aishe setup`"
            );
        }
        let real_cwd = executor.cwd().clone();
        let staging = std::env::temp_dir().join(format!("aishe-yolo-dry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        if let Err(error) = crate::overlay::copy_tree(&real_cwd, &staging) {
            let _ = std::fs::remove_dir_all(&staging);
            anyhow::bail!(
                "yolo_dry_run could not create its isolated preview and will not execute: {error}"
            );
        }
        executor.redirect_cwd(staging.clone());
        executor.set_sandbox_wrap(crate::overlay::dry_run_argv(&staging, &staging));
        println!(
            "{}",
            "dry-run: this session runs in an isolated copy; changes are previewed at the end."
                .dim()
        );
        Ok(Some(DryRun { real_cwd, staging }))
    }

    /// Restore the executor, then preview the session's file changes and
    /// apply (interactive: prompt; non-interactive: auto-apply, journaled) or
    /// discard them. Always cleans up the staging copy.
    fn finish(self, executor: &mut Executor) {
        executor.set_sandbox_wrap(Vec::new());
        executor.redirect_cwd(self.real_cwd.clone());

        let changes = crate::overlay::changes(&self.real_cwd, &self.staging);
        if changes.is_empty() {
            println!("{} no file changes this session.", "dry-run:".bold());
            let _ = std::fs::remove_dir_all(&self.staging);
            return;
        }
        println!(
            "\n{} {} file change(s) from this session:",
            "dry-run:".bold(),
            changes.len()
        );
        crate::overlay::print_changes(&changes);

        let apply = if std::io::stdin().is_terminal() {
            print!(
                "\napply these {} change(s) to the working tree? [Y/n]: ",
                changes.len()
            );
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).is_ok()
                && !matches!(line.trim().to_ascii_lowercase().as_str(), "n" | "no")
        } else {
            true // non-interactive (-c): auto-apply (journaled, so `aishe undo` reverts)
        };

        if apply {
            let failed = crate::overlay::apply_journaled(
                &self.real_cwd,
                &self.staging,
                &changes,
                "yolo_dry_run",
            );
            if failed.is_empty() {
                println!(
                    "{} applied {} change(s) ({} to revert).",
                    "✓".green(),
                    changes.len(),
                    "aishe undo".bold()
                );
            } else {
                println!(
                    "{} applied with {} failure(s): {}",
                    "!".yellow(),
                    failed.len(),
                    failed.join(", ")
                );
            }
        } else {
            println!("{} changes discarded.", "✗".red());
        }
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// The agentic loop itself. Returns the model's final answer text (when it
/// finished with a no-tool turn), or `None` if it was aborted, hit the budget, or
/// reached the iteration cap. Usage reporting and session recording happen in
/// [`run`].
#[allow(clippy::too_many_arguments)]
fn run_loop(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    mcp: &McpRegistry,
    history: Vec<Msg>,
    task: &mut crate::tasks::Active,
    resumed: bool,
) -> Result<Option<String>> {
    let ctx = context::build(executor, config);
    // Effective confirmation tier (resolves `yolo_confirm` and the legacy
    // `yolo_confirm_dangerous` boolean). Writes outside the tree by the file
    // tools are confirmed whenever the tier is not "never".
    let tier = sandbox::confirm_tier(config);
    let confirm_writes = tier != Tier::Never;
    // Sandbox backend (Off / Policy gate / bwrap OS isolation). A `bwrap` request
    // with bubblewrap missing degrades to the policy gate — warn once.
    let sandbox_backend = sandbox::backend(config);
    if sandbox::bwrap_requested_but_missing(config) {
        eprintln!(
            "{}",
            "aishe: sandbox_backend=\"bwrap\" but bubblewrap (bwrap) isn't installed; \
             using the best-effort policy sandbox instead."
                .yellow()
        );
    }
    // Tools: always run_command; the built-in file tools when enabled; use_skill
    // when skills exist.
    let mut tools = vec![run_command_tool()];
    if config.aishe.file_tools {
        tools.extend(crate::tools::file_tool_defs());
    }
    if config.aishe.web_tool {
        tools.extend(crate::tools::web_tool_defs());
    }
    if !mcp.is_empty() {
        tools.extend(mcp.tool_defs());
    }
    if !skills.is_empty() {
        tools.push(use_skill_tool());
    }
    let mut system = YOLO_SYSTEM.to_string();
    if config.aishe.file_tools {
        system.push_str(
            "\n\nFor files, prefer the read_file / write_file / edit_file / list_dir \
             tools over shell cat/sed/heredoc (they are exact and avoid quoting issues).",
        );
    }
    if config.aishe.web_tool {
        system.push_str(
            "\n\nTo read a web page or docs, use the fetch_url tool rather than curl/wget.",
        );
    }
    if !skills.is_empty() {
        system.push_str(&format!(
            "\n\nAvailable skills (call use_skill to load one's instructions when \
             relevant):\n{}",
            skills.catalog()
        ));
    }
    let mut messages: Vec<Msg> = history;
    let mut user_msg = format!("{ctx}\nUser request: {input}");

    // Plan-first (dry run): show the intended steps and require approval before
    // the loop touches anything. Interactive only — there is no one to approve a
    // piped/`-c` run, so it proceeds as normal there.
    if !resumed && config.aishe.yolo_plan && std::io::stdin().is_terminal() {
        match plan_first(input, &ctx, provider, config) {
            PlanOutcome::Declined => {
                println!("  {}", "aborted".dim());
                task.interrupted(&messages, provider.meter().snapshot());
                return Ok(None);
            }
            PlanOutcome::Approved(plan) => {
                user_msg.push_str(&format!("\n\nApproved plan to follow:\n{plan}"));
            }
            // No plan produced (empty or error): proceed without one.
            PlanOutcome::Skip => {}
        }
    }

    if resumed {
        messages.push(Msg::User(format!(
            "{ctx}\nResume the existing task safely. Original objective: {input}. \
             Inspect current state before making further changes."
        )));
    } else {
        messages.push(Msg::User(user_msg));
    }
    task.checkpoint_messages(&messages, provider.meter().snapshot());

    interrupt.store(false, Ordering::SeqCst);
    crate::audit::ai_request("yolo", config.active_model(), input);

    for iteration in 0..config.aishe.max_yolo_iterations {
        if interrupt.load(Ordering::SeqCst) {
            println!("  {}", "aborted".dim());
            task.interrupted(&messages, provider.meter().snapshot());
            return Ok(None);
        }
        // Stop before the next model call if the session budget is spent.
        if super::budget_reached(provider, config) {
            task.interrupted(&messages, provider.meter().snapshot());
            return Ok(None);
        }

        // Stream the assistant's prose live when streaming is on; otherwise wait
        // for the whole turn. `streamed` tracks whether any text was printed.
        let before = provider.meter().snapshot();
        let mut streamed = false;
        let result = if config.aishe.stream {
            let mut out = std::io::stdout();
            provider.complete_with_tools_stream(&system, &messages, &tools, &mut |delta| {
                streamed = true;
                print!("{delta}");
                let _ = out.flush();
            })
        } else {
            provider.complete_with_tools(&system, &messages, &tools)
        };
        let completion: Completion = match result {
            Ok(c) => c,
            Err(e) => {
                if streamed {
                    println!();
                }
                crate::audit::ai_error("yolo", config.active_model(), &e.to_string());
                eprintln!(
                    "{}",
                    format!("aishe: {}", crate::providers::actionable_error(&e)).red()
                );
                task.failed(
                    &messages,
                    provider.meter().snapshot(),
                    e.kind(),
                    &e.to_string(),
                );
                return Ok(None);
            }
        };
        let after = provider.meter().snapshot();
        crate::audit::ai_response(
            "yolo",
            config.active_model(),
            &completion_summary(&completion),
            after.input.saturating_sub(before.input),
            after.output.saturating_sub(before.output),
        );

        // No tool calls → final answer.
        if completion.tool_calls.is_empty() {
            match (&completion.text, streamed) {
                // Re-render the streamed raw text as proper markdown in place.
                (Some(text), true) => super::rerender_streamed_markdown(text),
                // Not streamed: render markdown directly.
                (Some(text), false) => render_markdown(text),
                _ => {}
            }
            messages.push(Msg::Assistant(AssistantMsg {
                text: completion.text.clone(),
                tool_calls: Vec::new(),
            }));
            task.completed(&messages, provider.meter().snapshot());
            return Ok(completion.text);
        }

        // Interim turn that emitted prose before its tool calls: end the line so
        // the upcoming tool-call lines start fresh.
        if streamed {
            println!();
        }

        // Record the assistant turn before tool results. OpenAI Responses
        // returns provider-native reasoning/function-call items that must be
        // replayed verbatim on the continuation; other providers use the
        // canonical assistant message.
        if completion.provider_items.is_empty() {
            messages.push(Msg::Assistant(AssistantMsg {
                text: completion.text.clone(),
                tool_calls: completion.tool_calls.clone(),
            }));
        } else {
            messages.push(Msg::ProviderItems {
                items: completion.provider_items.clone(),
                assistant: AssistantMsg {
                    text: completion.text.clone(),
                    tool_calls: completion.tool_calls.clone(),
                },
            });
        }
        task.checkpoint_messages(&messages, provider.meter().snapshot());

        for call in &completion.tool_calls {
            task.pending(call, &messages, provider.meter().snapshot());
            if interrupt.load(Ordering::SeqCst) {
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: "Interrupted by user.".to_string(),
                });
                println!("  {}", "aborted".dim());
                task.interrupted(&messages, provider.meter().snapshot());
                return Ok(None);
            }

            // Skill loading (progressive disclosure): return the skill body so
            // the model has its instructions in context, then continue.
            if call.name == "use_skill" {
                let name = call
                    .arguments
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let content = match skills.get(name) {
                    Some(s) => {
                        println!("  📖 {}", format!("skill: {name}").dim());
                        s.body.clone()
                    }
                    None => format!("No skill named '{name}'."),
                };
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: content.clone(),
                });
                task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                continue;
            }

            // Built-in tools (file read/write/edit/list, web fetch_url) run
            // directly here, relative to the cwd where applicable.
            if crate::tools::is_builtin_tool(&call.name) {
                task.mark_pending_started();
                let (label, content) = crate::tools::execute(
                    &call.name,
                    &call.arguments,
                    executor.cwd(),
                    confirm_writes,
                    config.aishe.yolo_preview,
                );
                crate::audit::action(&format!("yolo:{}", call.name), &label, None);
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: content.clone(),
                });
                task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                continue;
            }

            // MCP tools (namespaced mcp__server__tool) are proxied to the server.
            if crate::mcp::is_mcp_tool(&call.name) {
                task.mark_pending_started();
                println!("  🔌 {}", format!("mcp: {}", call.name).dim());
                let (label, content) = mcp.call(&call.name, &call.arguments);
                crate::audit::action(&format!("yolo:{}", call.name), &label, None);
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: content.clone(),
                });
                task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                continue;
            }

            let command = call
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let reason = call
                .arguments
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            println!(
                "  {} {}: {}",
                "⚡".yellow(),
                reason.as_str().dim(),
                command.as_str().white()
            );

            if command.trim().is_empty() {
                let content = "No command provided.".to_string();
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: content.clone(),
                });
                task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                continue;
            }

            // Policy sandbox: refuse network access / out-of-tree writes before
            // running, feeding the reason back to the model so it can adapt. The
            // bwrap backend enforces isolation at run time instead, so it does not
            // pre-refuse here.
            if sandbox_backend == sandbox::Backend::Policy {
                if let Some(reason) = sandbox::sandbox_refusal(&command) {
                    println!("  {} {}", "⛔".red(), reason.as_str().yellow());
                    crate::audit::action("yolo:sandbox-refused", &command, None);
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: sandbox::refusal_message(&reason),
                    });
                    let content = messages
                        .last()
                        .and_then(|message| match message {
                            Msg::ToolResult { content, .. } => Some(content.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                    continue;
                }
            }

            // Confirmation tier: pause for dangerous and/or state-modifying
            // commands depending on `yolo_confirm`. Dangerous commands show the
            // red panel; tier-only confirms use a plain yes/no prompt. Both
            // proceed automatically when stdin is not a terminal.
            let (need_confirm, dangerous) = sandbox::needs_confirm(tier, &command);
            if need_confirm {
                let declined = if dangerous {
                    matches!(safety_gate(&command), GateOutcome::Declined)
                } else {
                    !confirm_run(&command)
                };
                if declined {
                    let content = "User declined to run this command.".to_string();
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: content.clone(),
                    });
                    task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                    continue;
                }
            }

            // bwrap backend: run this command inside a sandbox (read-only root,
            // writable working tree). Recomputed each time so it tracks the cwd.
            if sandbox_backend == sandbox::Backend::Bwrap {
                let wrap = sandbox::bwrap_wrap_argv(executor.cwd());
                executor.set_sandbox_wrap(wrap);
            }
            let verbose = config.aishe.yolo_verbose;
            task.mark_pending_started();
            let (code, output) = executor.run_captured(&command, DEFAULT_CAPTURE_TIMEOUT, verbose);
            // Quiet by default: the model still gets the full output, but the
            // terminal shows only a compact result instead of dumping everything.
            if !verbose {
                print_run_result(code, &output);
            }
            crate::audit::action("yolo", &command, Some(code));
            let content = format!("exit code {code}\n{output}");
            messages.push(Msg::ToolResult {
                call_id: call.id.clone(),
                content: content.clone(),
            });
            task.tool_completed(call, &content, &messages, provider.meter().snapshot());
        }

        if iteration + 1 == config.aishe.max_yolo_iterations {
            println!(
                "  {}",
                format!(
                    "reached max iterations ({})",
                    config.aishe.max_yolo_iterations
                )
                .yellow()
            );
        }
    }

    task.interrupted(&messages, provider.meter().snapshot());
    Ok(None)
}

/// Continue a durable task from its last complete checkpoint. A pending tool is
/// never repeated automatically: the default records an explicit skipped result
/// so the model can inspect current state before deciding what to do next.
#[allow(clippy::too_many_arguments)]
pub fn resume(
    record: crate::tasks::Record,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    mcp: &McpRegistry,
) -> Result<()> {
    if record.status == crate::tasks::Status::Completed {
        anyhow::bail!("task {} is already completed", record.id);
    }
    let objective = record.objective.clone();
    let changed_provider =
        record.provider != config.aishe.provider || record.model != config.active_model();
    let mut messages = if changed_provider {
        eprintln!(
            "{}",
            format!(
                "aishe: resuming {} with {} / {} instead of {} / {}; \
                 using provider-neutral canonical history",
                record.id,
                config.aishe.provider,
                config.active_model(),
                record.provider,
                record.model
            )
            .yellow()
        );
        canonical_messages(&record.messages)
    } else {
        record.messages.clone()
    };
    let mut task = crate::tasks::Active::resume(record);
    if let Some(pending) = task.record().pending_tool.clone() {
        println!(
            "{}",
            format!(
                "pending tool '{}' ({}) may not have completed before interruption",
                pending.call.name, pending.call.id
            )
            .yellow()
        );
        let skip = if std::io::stdin().is_terminal() {
            crate::promptui::confirm(
                "Skip it and let the model inspect current state (never repeat automatically)",
                true,
            )?
            .unwrap_or(false)
        } else {
            true
        };
        if !skip {
            task.interrupted(&messages, provider.meter().snapshot());
            println!("  resume cancelled; task remains interrupted");
            return Ok(());
        }
        if let Some(result) = task.clear_pending_with_result(
            "Skipped on resume because the prior process may have started this tool. \
             Inspect current state before proposing another action.",
        ) {
            messages.push(result);
        }
    }
    println!("  {}", format!("resuming task {}", task.id()).dim());
    let outcome = run_loop(
        &objective, provider, executor, config, interrupt, skills, mcp, messages, &mut task, true,
    );
    super::report_usage(provider, config);
    outcome?;
    Ok(())
}

fn canonical_messages(messages: &[Msg]) -> Vec<Msg> {
    messages
        .iter()
        .map(|message| match message {
            Msg::ProviderItems { assistant, .. } => Msg::Assistant(assistant.clone()),
            other => other.clone(),
        })
        .collect()
}

/// Compact per-step result shown in non-verbose yolo: the exit code and a line
/// count, plus a short tail of the output when the command failed (so the user
/// sees what went wrong without the full dump). The model still receives the
/// complete output.
fn print_run_result(code: i32, output: &str) {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let n = lines.len();
    let plural = if n == 1 { "" } else { "s" };
    if code == 0 {
        println!(
            "  {} {}",
            "✓".green(),
            format!("exit 0 · {n} line{plural}").dim()
        );
    } else {
        println!(
            "  {} {}",
            "✗".red(),
            format!("exit {code} · {n} line{plural}").dim()
        );
        // A short tail for context (the model gets all of it).
        let tail = lines.len().saturating_sub(4);
        for l in &lines[tail..] {
            println!("    {}", l.dim());
        }
    }
}

/// Result of the plan-first pre-pass.
enum PlanOutcome {
    /// The user approved the printed plan; thread it into the loop.
    Approved(String),
    /// The user declined; abort the run.
    Declined,
    /// No usable plan (empty or the planning call failed); run without one.
    Skip,
}

const PLAN_SYSTEM: &str = "You are about to run an agentic shell task. First, \
    WITHOUT executing anything, lay out the concrete steps you intend to take as \
    a short numbered list (commands or file edits at a high level). Be specific \
    but concise; do not actually run or simulate anything, just describe the plan.";

/// Ask the model for its intended steps, print them, and get the user's approval
/// before the agentic loop runs. The planning call is a plain (no-tool)
/// completion; its tokens are metered and the turn is audit-logged.
fn plan_first(input: &str, ctx: &str, provider: &dyn Provider, config: &Config) -> PlanOutcome {
    println!("  {}", "planning…".dim());
    let messages = vec![Msg::User(format!("{ctx}\nUser request: {input}"))];
    crate::audit::ai_request("yolo-plan", config.active_model(), input);
    let before = provider.meter().snapshot();
    let plan = match provider.complete(PLAN_SYSTEM, &messages, &ResponseFormat::Text) {
        Ok(p) => p,
        Err(e) => {
            crate::audit::ai_error("yolo-plan", config.active_model(), &e.to_string());
            eprintln!(
                "{}",
                format!(
                    "aishe: planning failed: {}",
                    crate::providers::actionable_error(&e)
                )
                .red()
            );
            return PlanOutcome::Skip;
        }
    };
    let after = provider.meter().snapshot();
    crate::audit::ai_response(
        "yolo-plan",
        config.active_model(),
        &plan,
        after.input.saturating_sub(before.input),
        after.output.saturating_sub(before.output),
    );
    if plan.trim().is_empty() {
        return PlanOutcome::Skip;
    }
    println!("\n  {}", "Plan".bold());
    render_markdown(&plan);
    if confirm_plan() {
        PlanOutcome::Approved(plan)
    } else {
        PlanOutcome::Declined
    }
}

/// Confirm running a (non-dangerous) command under a "writes"/"all" tier. With an
/// interactive terminal it asks (`[Y/n]`, default yes on Enter); without one
/// (`-c`/piped, no human to answer) it proceeds, consistent with the file-tool
/// `confirm()` and the rest of the codebase's non-tty behavior.
fn confirm_run(command: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return true;
    }
    print!("  {} {} [Y/n]: ", "run".yellow().bold(), command.white());
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let a = line.trim();
    a.is_empty() || a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
}

/// Prompt `Proceed with this plan? [Y/n]`. Defaults to yes on Enter.
fn confirm_plan() -> bool {
    print!("  {} ", "Proceed with this plan? [Y/n]".yellow().bold());
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    let a = line.trim();
    a.is_empty() || a.eq_ignore_ascii_case("y") || a.eq_ignore_ascii_case("yes")
}

/// A concise summary of one assistant turn for the audit log: the final answer
/// text, or the list of tool calls it made.
fn completion_summary(c: &Completion) -> String {
    if c.tool_calls.is_empty() {
        return c.text.clone().unwrap_or_default();
    }
    let calls: Vec<String> = c
        .tool_calls
        .iter()
        .map(|call| {
            if call.name == "use_skill" {
                let name = call
                    .arguments
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                format!("use_skill({name})")
            } else {
                call.arguments
                    .get("command")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            }
        })
        .collect();
    format!("tool_calls: {}", calls.join(" | "))
}

const YOLO_SYSTEM: &str = super::YOLO_SYSTEM_PROMPT;
