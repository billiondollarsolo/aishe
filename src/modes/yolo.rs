//! Yolo mode: an agentic loop where the model drives `run_command` until it has
//! accomplished the task, then summarizes.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::agent::native::{NativeBudget, NativeLimits};
use crate::agent::renderer::AgentRenderer;
use crate::agent::{AgentEvent, ToolCallView, ToolResultView, UserFacingError};
use crate::agent::{NativeTurnOutcome, NativeTurnState};
use crate::ui::SemanticStylize;
use crate::ui::{StyleToken, TerminalCapabilities};
use anyhow::Result;

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
) -> Result<NativeTurnOutcome> {
    run_with_terminal(
        input,
        provider,
        executor,
        config,
        interrupt,
        skills,
        mcp,
        session,
        TerminalCapabilities::detect_stdout(),
    )
}

/// The lean shell redirects stdout while this loop runs. Preserve the real
/// terminal's capabilities before that redirect so density, colors, and motion
/// describe the user-facing output surface rather than the forwarding pipe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_with_terminal(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    mcp: &McpRegistry,
    session: &mut Session,
    capabilities: TerminalCapabilities,
) -> Result<NativeTurnOutcome> {
    // Optional reversible session: run the whole loop against a throwaway copy of
    // the working tree, then preview + confirm/apply at the end.
    let history = session.history();
    let mut task = crate::tasks::Active::start(config, executor.cwd(), input);
    task.set_usage_baseline(provider.meter().snapshot());
    task.checkpoint_admission(executor);
    if let Err(error) = task.ensure_persisted() {
        return Ok(fail_internal(
            &mut task,
            provider,
            interrupt.load(Ordering::SeqCst) || executor.is_cancelled(),
            error,
        ));
    }
    let dry = match DryRun::setup(executor, config) {
        Ok(dry) => dry,
        Err(error) => {
            return Ok(fail_internal(
                &mut task,
                provider,
                interrupt.load(Ordering::SeqCst) || executor.is_cancelled(),
                error,
            ))
        }
    };
    println!("  {}", format!("task {}", task.id()).dim());
    let density = effective_density(config);
    let mut renderer = AgentRenderer::with_capabilities(density, capabilities);
    let outcome = run_loop(
        input,
        provider,
        executor,
        config,
        interrupt,
        skills,
        mcp,
        history,
        &mut task,
        false,
        &mut renderer,
    );
    renderer.clear_status();
    super::report_usage(provider, config);
    let mut outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => fail_internal(
            &mut task,
            provider,
            interrupt.load(Ordering::SeqCst) || executor.is_cancelled(),
            error,
        ),
    };
    if let Some(d) = dry {
        if let Some((state, detail)) =
            d.finish(executor, outcome.state == NativeTurnState::Completed)
        {
            outcome.state = state;
            outcome.detail = Some(detail);
            outcome.final_text = None;
            let messages = task.record().messages.clone();
            task.finish_native(&outcome, &messages, provider.meter().snapshot());
        }
    }
    session.record_user(input);
    session.record_assistant(
        outcome
            .final_text
            .as_deref()
            .or(outcome.detail.as_deref())
            .unwrap_or("(agent turn ended without a final summary)"),
    );
    Ok(outcome)
}

/// A reversible yolo session: the loop runs against `staging` (a copy of the
/// working tree, bind-mounted at the real cwd under bubblewrap with a read-only
/// root and no network), and the changes are previewed + applied/discarded at the
/// end. Created by [`DryRun::setup`] when `yolo_dry_run` is on and bwrap is present.
struct DryRun {
    real_cwd: std::path::PathBuf,
    staging: std::path::PathBuf,
    real_scope: Option<(
        crate::agent::ExecutionScope,
        std::path::PathBuf,
        crate::agent::NetworkPolicy,
    )>,
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
        let real_scope = executor.lean_scope().cloned();
        if real_scope.is_some() {
            executor.set_lean_scope(Some((
                crate::agent::ExecutionScope::Workspace,
                staging.canonicalize()?,
                crate::agent::NetworkPolicy::Deny,
            )));
        }
        executor.redirect_cwd(staging.clone());
        executor.set_sandbox_wrap(crate::overlay::dry_run_argv(&staging, &staging));
        println!(
            "{}",
            "dry-run: this session runs in an isolated copy; changes are previewed at the end."
                .dim()
        );
        Ok(Some(DryRun {
            real_cwd,
            staging,
            real_scope,
        }))
    }

    /// Restore the executor, then preview the session's file changes and
    /// apply (interactive: prompt; non-interactive: auto-apply, journaled) or
    /// discard them. Always cleans up the staging copy.
    fn finish(self, executor: &mut Executor, completed: bool) -> Option<(NativeTurnState, String)> {
        executor.set_sandbox_wrap(Vec::new());
        executor.redirect_cwd(self.real_cwd.clone());
        executor.set_lean_scope(self.real_scope.clone());

        let changes = crate::overlay::changes(&self.real_cwd, &self.staging);
        if changes.is_empty() {
            println!("{} no file changes this session.", "dry-run:".bold());
            let _ = std::fs::remove_dir_all(&self.staging);
            return None;
        }
        println!(
            "\n{} {} file change(s) from this session:",
            "dry-run:".bold(),
            changes.len()
        );
        crate::overlay::print_changes(&changes);

        let apply = if !completed || executor.is_cancelled() {
            false
        } else if std::io::stdin().is_terminal() {
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

        let mut outcome = None;
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
                outcome = Some((
                    NativeTurnState::Failed,
                    format!(
                        "Preview changes could not all be applied: {}",
                        failed.join(", ")
                    ),
                ));
            }
        } else {
            println!("{} changes discarded.", "✗".red());
            if completed {
                outcome = Some((
                    NativeTurnState::Declined,
                    "Preview changes were discarded.".into(),
                ));
            }
        }
        let _ = std::fs::remove_dir_all(&self.staging);
        outcome
    }
}

/// The agentic loop itself. Every stop carries an explicit terminal state;
/// provider failures, cancellation and limits can never look like completion.
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
    renderer: &mut AgentRenderer,
) -> Result<NativeTurnOutcome> {
    let current_limits = NativeLimits::from_environment()?;
    let limits = task
        .record()
        .execution_limits
        .as_ref()
        .map_or(current_limits.clone(), |original| {
            original.constrain(&current_limits)
        });
    limits.validate()?;
    task.checkpoint_limits(limits.clone());
    let mut budget = NativeBudget::new(limits, task.record().execution);
    let _http_deadline = crate::providers::HttpDeadlineGuard::new(budget.remaining())?;
    if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
        return finish_turn(
            task,
            NativeTurnState::Cancelled,
            Some("Interrupted by user.".into()),
            &history,
            provider,
            &budget,
        );
    }
    if let Err(reason) = budget.check_provider() {
        return finish_turn(
            task,
            NativeTurnState::BudgetExhausted,
            Some(reason),
            &history,
            provider,
            &budget,
        );
    }
    if budget.requires_cost_accounting()
        && crate::usage::budget_price_for(config.active_model(), &config.pricing).is_none_or(
            |price| {
                !price.input.is_finite()
                    || !price.output.is_finite()
                    || price.input < 0.0
                    || price.output < 0.0
            },
        )
    {
        return finish_turn(task, NativeTurnState::Failed, Some("An explicit task cost limit requires an exact model price in [pricing]; no provider work was started.".into()), &history, provider, &budget);
    }
    let ctx = context::build(executor, config);
    // Effective confirmation tier (resolves `yolo_confirm` and the legacy
    // `yolo_confirm_dangerous` boolean). Writes outside the tree by the file
    // tools are confirmed whenever the tier is not "never".
    // A validated lean session grant authorizes autonomous actions within its
    // explicit scope. Legacy turns keep their configured confirmation tier.
    let tier = if executor.lean_scope().is_some() {
        Tier::Never
    } else {
        sandbox::confirm_tier(config)
    };
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
    let mcp_allowed = !executor
        .lean_scope()
        .is_some_and(|(_, _, network)| *network == crate::agent::NetworkPolicy::Deny);
    if mcp_allowed && !mcp.is_empty() {
        tools.extend(mcp.tool_defs());
    }
    if !skills.is_empty() {
        tools.push(use_skill_tool());
    }
    let mut system = YOLO_SYSTEM.to_string();
    if executor.lean_scope().is_some() {
        system.push_str("\n\nCommands run in a fresh restricted POSIX shell (dash, or zsh without startup files). They inherit filtered exports, PATH and environment from the live shell, but do not import interactive aliases, functions, plugins or startup code. Use POSIX syntax for run_command.");
    }
    system.push_str("\n\n");
    system.push_str(crate::product_help::product_brief());
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
             relevant; use aishe-product for how-to questions about AIShe itself):\n{}",
            skills.catalog()
        ));
    }
    let mut messages: Vec<Msg> = history;
    let mut user_msg = format!("{ctx}\nUser request: {input}");

    // Plan-first (dry run): show the intended steps and require approval before
    // the loop touches anything. Interactive only — there is no one to approve a
    // piped/`-c` run, so it proceeds as normal there.
    if !resumed && config.aishe.yolo_plan && std::io::stdin().is_terminal() {
        if let Err(reason) = budget.admit_provider() {
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some(reason),
                &messages,
                provider,
                &budget,
            );
        }
        task.checkpoint_execution(budget.counters());
        task.ensure_persisted()?;
        let before = provider.meter().snapshot();
        let plan = plan_first(input, &ctx, provider, config);
        record_provider_cost(&mut budget, provider, config, before);
        task.checkpoint_execution(budget.counters());
        if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
            return finish_turn(
                task,
                NativeTurnState::Cancelled,
                Some("Interrupted by user.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        if let Some(reason) = budget.exhausted() {
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some(reason),
                &messages,
                provider,
                &budget,
            );
        }
        match plan {
            PlanOutcome::Declined => {
                println!("  {}", "aborted".dim());
                return finish_turn(
                    task,
                    NativeTurnState::Declined,
                    Some("User declined the proposed plan.".into()),
                    &messages,
                    provider,
                    &budget,
                );
            }
            PlanOutcome::Approved(plan) => {
                user_msg.push_str(&format!("\n\nApproved plan to follow:\n{plan}"));
            }
            PlanOutcome::Failed(detail) => {
                return finish_turn(
                    task,
                    NativeTurnState::Failed,
                    Some(detail),
                    &messages,
                    provider,
                    &budget,
                );
            }
            // Empty plan: proceed without one.
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

    crate::audit::ai_request("yolo", config.active_model(), input);

    for iteration in 0..config.aishe.max_yolo_iterations {
        if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
            renderer.render(&AgentEvent::Aborted);
            return finish_turn(
                task,
                NativeTurnState::Cancelled,
                Some("Interrupted by user.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        // Stop before the next model call if the session budget is spent.
        if super::budget_reached(provider, config) {
            renderer.clear_status();
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some("Session cost budget is exhausted.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        if let Err(reason) = budget.admit_provider() {
            renderer.clear_status();
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some(reason),
                &messages,
                provider,
                &budget,
            );
        }
        task.checkpoint_execution(budget.counters());
        task.ensure_persisted()?;

        // Stream the assistant's prose live when streaming is on; otherwise wait
        // for the whole turn. `streamed` tracks whether any text was printed.
        let before = provider.meter().snapshot();
        let mut streamed = false;
        renderer.render(&AgentEvent::ReasoningStarted);
        let result = if config.aishe.stream && effective_density(config) == "detailed" {
            provider.complete_with_tools_stream(&system, &messages, &tools, &mut |delta| {
                if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
                    return;
                }
                streamed = true;
                renderer.render(&AgentEvent::TextDelta { text: delta.into() });
            })
        } else {
            provider.complete_with_tools(&system, &messages, &tools)
        };
        record_provider_cost(&mut budget, provider, config, before);
        task.checkpoint_execution(budget.counters());
        // A blocked provider call may finish after Ctrl-C. Never print its
        // result or admit a tool after that turn has been cancelled.
        if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
            renderer.render(&AgentEvent::Aborted);
            return finish_turn(
                task,
                NativeTurnState::Cancelled,
                Some("Interrupted by user.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        if let Some(reason) = budget.exhausted() {
            renderer.clear_status();
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some(reason),
                &messages,
                provider,
                &budget,
            );
        }
        if super::budget_reached(provider, config) {
            renderer.clear_status();
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some("Session cost budget is exhausted.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        let completion: Completion = match result {
            Ok(c) => c,
            Err(e) => {
                if streamed {
                    println!();
                }
                crate::audit::ai_error("yolo", config.active_model(), &e.to_string());
                renderer.render(&AgentEvent::Failed {
                    error: UserFacingError {
                        code: "provider.error".into(),
                        message: crate::providers::actionable_error(&e),
                        retryable: false,
                    },
                });
                task.failed(
                    &messages,
                    provider.meter().snapshot(),
                    e.kind(),
                    &e.to_string(),
                );
                return finish_turn(
                    task,
                    NativeTurnState::Failed,
                    Some(crate::providers::actionable_error(&e)),
                    &messages,
                    provider,
                    &budget,
                );
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
            if task.background_cancelled() {
                return finish_turn(
                    task,
                    NativeTurnState::Cancelled,
                    Some("Interrupted by user.".into()),
                    &messages,
                    provider,
                    &budget,
                );
            }
            if completion
                .text
                .as_deref()
                .is_none_or(|text| text.trim().is_empty())
            {
                return finish_turn(
                    task,
                    NativeTurnState::Failed,
                    Some("The provider returned neither a final answer nor any tool calls.".into()),
                    &messages,
                    provider,
                    &budget,
                );
            }
            let final_text = completion.text.clone().unwrap_or_default();
            renderer.render(&AgentEvent::TextCompleted {
                text: final_text.clone(),
            });
            renderer.render(&AgentEvent::Completed {
                // TextCompleted already carries the answer; Completed closes
                // activity without printing the same prose a second time.
                summary: String::new(),
            });
            messages.push(Msg::Assistant(AssistantMsg {
                text: completion.text.clone(),
                tool_calls: Vec::new(),
            }));
            let outcome = NativeTurnOutcome::completed(task.id(), completion.text);
            task.checkpoint_execution(budget.counters());
            task.finish_native(&outcome, &messages, provider.meter().snapshot());
            task.ensure_persisted()?;
            return Ok(outcome);
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
            if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: "Interrupted by user.".to_string(),
                });
                renderer.render(&AgentEvent::Aborted);
                return finish_turn(
                    task,
                    NativeTurnState::Cancelled,
                    Some("Interrupted by user.".into()),
                    &messages,
                    provider,
                    &budget,
                );
            }
            if let Err(reason) = budget.admit_tool(call) {
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: format!("Not executed: {reason}."),
                });
                task.tool_completed(
                    call,
                    &format!("Not executed: {reason}."),
                    &messages,
                    provider.meter().snapshot(),
                );
                renderer.clear_status();
                return finish_turn(
                    task,
                    NativeTurnState::BudgetExhausted,
                    Some(reason),
                    &messages,
                    provider,
                    &budget,
                );
            }
            task.checkpoint_execution(budget.counters());
            task.ensure_persisted()?;
            renderer.render(&AgentEvent::ToolStarted {
                call: ToolCallView {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: call.arguments.clone(),
                    title: String::new(),
                },
            });

            if !tools.iter().any(|tool| tool.name == call.name) {
                let content = format!(
                    "Error: tool '{}' was not offered for this turn and was not executed.",
                    call.name
                );
                render_tool_result(renderer, &call.id, None, &content);
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: content.clone(),
                });
                task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                continue;
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
                    Some(s) => s.body.clone(),
                    None => format!("No skill named '{name}'."),
                };
                render_tool_result(renderer, &call.id, None, &content);
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
                // File previews and approvals temporarily own the cursor.
                renderer.clear_status();
                let (label, content) = super::yolo_workspace::execute_rendered(
                    executor.lean_scope(),
                    &call.name,
                    &call.arguments,
                    executor.cwd(),
                    confirm_writes,
                    config.aishe.yolo_preview,
                    effective_density(config) == "detailed",
                );
                render_tool_result(renderer, &call.id, None, &content);
                if (content.starts_with("Wrote ") || content.starts_with("Replaced "))
                    && matches!(call.name.as_str(), "write_file" | "edit_file")
                {
                    if let Some(path) = call
                        .arguments
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                    {
                        renderer.render(&AgentEvent::Diff {
                            diff: crate::agent::DiffView {
                                path: path.into(),
                                patch: String::new(),
                            },
                        });
                    }
                }
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
                if !mcp_allowed {
                    let content = "Error: opaque MCP calls are unavailable when workspace network access is denied.".to_string();
                    render_tool_result(renderer, &call.id, None, &content);
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: content.clone(),
                    });
                    task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                    continue;
                }
                task.mark_pending_started();
                let (label, content) = mcp.call(&call.name, &call.arguments);
                render_tool_result(renderer, &call.id, None, &content);
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

            if effective_density(config) == "detailed" {
                renderer.clear_status();
                renderer.line(
                    &format!("  {}", crate::commands::display_safe(&command)),
                    StyleToken::ProposedCommand,
                );
                if !reason.trim().is_empty() {
                    renderer.line(
                        &format!("  {}", crate::commands::display_safe(&reason)),
                        StyleToken::Muted,
                    );
                }
            }

            if command.trim().is_empty() {
                let content = "Error: no command provided.".to_string();
                render_tool_result(renderer, &call.id, None, &content);
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
            if executor.lean_scope().is_none() && sandbox_backend == sandbox::Backend::Policy {
                if let Some(reason) = sandbox::sandbox_refusal(&command) {
                    render_tool_result(
                        renderer,
                        &call.id,
                        None,
                        &format!("Error: workspace policy refused this command: {reason}"),
                    );
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
                renderer.clear_status();
                let declined = if dangerous {
                    matches!(safety_gate(&command), GateOutcome::Declined)
                } else {
                    !confirm_run(&command)
                };
                if declined {
                    let content = "User declined to run this command.".to_string();
                    render_tool_result(renderer, &call.id, None, &content);
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: content.clone(),
                    });
                    task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                    continue;
                }
            }

            // Lean authority retains its accepted root while cwd changes. The
            // compatibility sandbox must never replace that stronger wrapper.
            if let Some((scope, workspace, network)) = executor.lean_scope().cloned() {
                let wrap = match scope {
                    crate::agent::ExecutionScope::Host => Ok(Vec::new()),
                    crate::agent::ExecutionScope::Workspace => {
                        #[cfg(target_os = "linux")]
                        {
                            sandbox::agent_bwrap_argv(&workspace, executor.cwd(), network)
                        }
                        #[cfg(not(target_os = "linux"))]
                        {
                            if let Some(reason) = sandbox::sandbox_refusal(&command) {
                                Err(anyhow::anyhow!(reason))
                            } else {
                                // Native file tools still enforce the canonical
                                // accepted root on policy-only platforms.
                                Ok(Vec::new())
                            }
                        }
                    }
                };
                match wrap {
                    Ok(wrap) => executor.set_sandbox_wrap(wrap),
                    Err(error) => {
                        let content =
                            format!("Error: workspace scope refused this command: {error}");
                        render_tool_result(renderer, &call.id, None, &content);
                        messages.push(Msg::ToolResult {
                            call_id: call.id.clone(),
                            content: content.clone(),
                        });
                        task.tool_completed(call, &content, &messages, provider.meter().snapshot());
                        continue;
                    }
                }
            } else if sandbox_backend == sandbox::Backend::Bwrap {
                let wrap = sandbox::bwrap_wrap_argv(executor.cwd());
                executor.set_sandbox_wrap(wrap);
            }
            let verbose = effective_density(config) == "detailed";
            if verbose {
                renderer.clear_status();
            }
            task.mark_pending_started();
            let (code, output) = executor.run_captured(
                &command,
                budget.command_timeout(DEFAULT_CAPTURE_TIMEOUT),
                verbose,
            );
            task.checkpoint_cwd(executor.cwd());
            if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
                renderer.render(&AgentEvent::Aborted);
                return finish_turn(
                    task,
                    NativeTurnState::Cancelled,
                    Some("Interrupted by user.".into()),
                    &messages,
                    provider,
                    &budget,
                );
            }
            render_tool_result(renderer, &call.id, Some(code), &output);
            crate::audit::action("yolo", &command, Some(code));
            let content = format!("exit code {code}\n{output}");
            messages.push(Msg::ToolResult {
                call_id: call.id.clone(),
                content: content.clone(),
            });
            task.tool_completed(call, &content, &messages, provider.meter().snapshot());
        }

        if interrupt.load(Ordering::SeqCst) || executor.is_cancelled() {
            renderer.render(&AgentEvent::Aborted);
            return finish_turn(
                task,
                NativeTurnState::Cancelled,
                Some("Interrupted by user.".into()),
                &messages,
                provider,
                &budget,
            );
        }
        if let Some(reason) = budget.exhausted() {
            renderer.clear_status();
            return finish_turn(
                task,
                NativeTurnState::BudgetExhausted,
                Some(reason),
                &messages,
                provider,
                &budget,
            );
        }

        if iteration + 1 == config.aishe.max_yolo_iterations {
            renderer.clear_status();
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

    finish_turn(
        task,
        NativeTurnState::IterationLimit,
        Some(format!(
            "Reached the iteration limit ({}).",
            config.aishe.max_yolo_iterations
        )),
        &messages,
        provider,
        &budget,
    )
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
) -> Result<NativeTurnOutcome> {
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
    task.set_usage_baseline(provider.meter().snapshot());
    task.checkpoint_admission(executor);
    if let Err(error) = task.ensure_persisted() {
        return Ok(fail_internal(
            &mut task,
            provider,
            interrupt.load(Ordering::SeqCst) || executor.is_cancelled(),
            error,
        ));
    }
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
            let outcome = NativeTurnOutcome::new(
                task.id(),
                NativeTurnState::Declined,
                Some("Resume was declined; the pending tool was not repeated.".into()),
            );
            task.finish_native(&outcome, &messages, provider.meter().snapshot());
            return Ok(outcome);
        }
        if let Some(result) = task.clear_pending_with_result(
            "Skipped on resume because the prior process may have started this tool. \
             Inspect current state before proposing another action.",
        ) {
            messages.push(result);
        }
    }
    // A provider can return several calls in one turn. Cancellation or a
    // budget can stop before later calls have even acquired a pending record.
    // Resolve those unattempted calls explicitly rather than replaying them or
    // sending an incomplete tool transcript to the provider on resume.
    messages = resolve_unanswered_tools(&messages);
    task.checkpoint_messages(&messages, provider.meter().snapshot());
    println!("  {}", format!("resuming task {}", task.id()).dim());
    let mut renderer = AgentRenderer::new(effective_density(config));
    let outcome = run_loop(
        &objective,
        provider,
        executor,
        config,
        interrupt,
        skills,
        mcp,
        messages,
        &mut task,
        true,
        &mut renderer,
    );
    renderer.clear_status();
    super::report_usage(provider, config);
    Ok(match outcome {
        Ok(outcome) => outcome,
        Err(error) => fail_internal(
            &mut task,
            provider,
            interrupt.load(Ordering::SeqCst) || executor.is_cancelled(),
            error,
        ),
    })
}

fn fail_internal(
    task: &mut crate::tasks::Active,
    provider: &dyn Provider,
    cancelled: bool,
    error: anyhow::Error,
) -> NativeTurnOutcome {
    let cancelled = cancelled || task.background_cancelled();
    let outcome = NativeTurnOutcome::new(
        task.id(),
        if cancelled {
            NativeTurnState::Cancelled
        } else {
            NativeTurnState::Failed
        },
        Some(if cancelled {
            "Interrupted by user.".into()
        } else {
            crate::redact::redact(&error.to_string())
        }),
    );
    let messages = task.record().messages.clone();
    task.finish_native(&outcome, &messages, provider.meter().snapshot());
    eprintln!(
        "aishe: {}",
        outcome.detail.as_deref().unwrap_or("native turn failed")
    );
    outcome
}

fn record_provider_cost(
    budget: &mut NativeBudget,
    provider: &dyn Provider,
    config: &Config,
    before: crate::usage::Usage,
) {
    let after = provider.meter().snapshot();
    if let Some(price) = crate::usage::budget_price_for(config.active_model(), &config.pricing) {
        budget.record_cost(crate::usage::cost(
            crate::usage::Usage {
                input: after.input.saturating_sub(before.input),
                output: after.output.saturating_sub(before.output),
                requests: after.requests.saturating_sub(before.requests),
            },
            price,
        ));
    }
}

fn finish_turn(
    task: &mut crate::tasks::Active,
    state: NativeTurnState,
    detail: Option<String>,
    messages: &[Msg],
    provider: &dyn Provider,
    budget: &NativeBudget,
) -> Result<NativeTurnOutcome> {
    let (state, detail) = if task.background_cancelled() {
        (
            NativeTurnState::Cancelled,
            Some("Interrupted by user.".into()),
        )
    } else {
        (state, detail)
    };
    let outcome = NativeTurnOutcome::new(task.id(), state, detail);
    task.checkpoint_execution(budget.counters());
    task.finish_native(&outcome, messages, provider.meter().snapshot());
    task.ensure_persisted()?;
    Ok(outcome)
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

fn resolve_unanswered_tools(messages: &[Msg]) -> Vec<Msg> {
    let mut result = Vec::new();
    let mut pending = Vec::<String>::new();
    for message in messages {
        if !matches!(message, Msg::ToolResult { .. }) {
            for call_id in pending.drain(..) {
                result.push(Msg::ToolResult {
                    call_id,
                    content: "Not executed before the previous turn stopped. Inspect current state before proposing further work.".into(),
                });
            }
        }
        match message {
            Msg::Assistant(assistant) | Msg::ProviderItems { assistant, .. } => {
                pending.extend(assistant.tool_calls.iter().map(|call| call.id.clone()));
            }
            Msg::ToolResult { call_id, .. } => pending.retain(|id| id != call_id),
            Msg::User(_) => {}
        }
        result.push(message.clone());
    }
    for call_id in pending {
        result.push(Msg::ToolResult {
            call_id,
            content: "Not executed before the previous turn stopped. Inspect current state before proposing further work.".into(),
        });
    }
    result
}

fn effective_density(config: &Config) -> &str {
    if config.aishe.yolo_verbose {
        "detailed"
    } else {
        &config.backend.output
    }
}

fn render_tool_result(
    renderer: &mut AgentRenderer,
    call_id: &str,
    code: Option<i32>,
    output: &str,
) {
    let failed = code.is_some_and(|value| value != 0)
        || output.trim_start().starts_with("Error")
        || output.trim_start().starts_with("No skill named ")
        || output.trim_start().starts_with("User declined ");
    if failed {
        let first_line = output
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        let message = code
            .map(|value| format!("exit {value}: {first_line}"))
            .unwrap_or_else(|| first_line.to_string());
        renderer.render(&AgentEvent::ToolFailed {
            call_id: call_id.into(),
            error: UserFacingError {
                code: "tool.failed".into(),
                message,
                retryable: true,
            },
        });
    } else {
        let summary = if let Some(code) = code {
            let lines = output
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count();
            format!(
                "exit {code} · {lines} line{}",
                if lines == 1 { "" } else { "s" }
            )
        } else {
            output.to_string()
        };
        renderer.render(&AgentEvent::ToolCompleted {
            call_id: call_id.into(),
            result: ToolResultView {
                success: true,
                exit_code: code,
                output: summary,
                metadata: serde_json::Value::Null,
            },
        });
    }
}

/// Result of the plan-first pre-pass.
enum PlanOutcome {
    /// The user approved the printed plan; thread it into the loop.
    Approved(String),
    /// The user declined; abort the run.
    Declined,
    Failed(String),
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
            return PlanOutcome::Failed(crate::providers::actionable_error(&e));
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
