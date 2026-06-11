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
    let history = session.history();
    let outcome = run_loop(
        input, provider, executor, config, interrupt, skills, mcp, history,
    );
    super::report_usage(provider, config);
    let final_text = outcome?;
    session.record_user(input);
    session.record_assistant(
        final_text
            .as_deref()
            .unwrap_or("(yolo task ended without a final summary)"),
    );
    Ok(())
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
) -> Result<Option<String>> {
    let ctx = context::build(
        executor,
        config.aishe.redact_secrets,
        config.aishe.project_context,
    );
    // Effective confirmation tier (resolves `yolo_confirm` and the legacy
    // `yolo_confirm_dangerous` boolean). Writes outside the tree by the file
    // tools are confirmed whenever the tier is not "never".
    let tier = sandbox::confirm_tier(config);
    let confirm_writes = tier != Tier::Never;
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
    if config.aishe.yolo_plan && std::io::stdin().is_terminal() {
        match plan_first(input, &ctx, provider, config) {
            PlanOutcome::Declined => {
                println!("  {}", "aborted".dim());
                return Ok(None);
            }
            PlanOutcome::Approved(plan) => {
                user_msg.push_str(&format!("\n\nApproved plan to follow:\n{plan}"));
            }
            // No plan produced (empty or error): proceed without one.
            PlanOutcome::Skip => {}
        }
    }

    messages.push(Msg::User(user_msg));

    interrupt.store(false, Ordering::SeqCst);
    crate::audit::ai_request("yolo", config.active_model(), input);

    for iteration in 0..config.aishe.max_yolo_iterations {
        if interrupt.load(Ordering::SeqCst) {
            println!("  {}", "aborted".dim());
            return Ok(None);
        }
        // Stop before the next model call if the session budget is spent.
        if super::budget_reached(provider, config) {
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
                eprintln!("{}", format!("aishe: {e}").red());
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
            return Ok(completion.text);
        }

        // Interim turn that emitted prose before its tool calls: end the line so
        // the upcoming tool-call lines start fresh.
        if streamed {
            println!();
        }

        // Record the assistant turn (text + tool calls) before tool results.
        messages.push(Msg::Assistant(AssistantMsg {
            text: completion.text.clone(),
            tool_calls: completion.tool_calls.clone(),
        }));

        for call in &completion.tool_calls {
            if interrupt.load(Ordering::SeqCst) {
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: "Interrupted by user.".to_string(),
                });
                println!("  {}", "aborted".dim());
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
                    content,
                });
                continue;
            }

            // Built-in tools (file read/write/edit/list, web fetch_url) run
            // directly here, relative to the cwd where applicable.
            if crate::tools::is_builtin_tool(&call.name) {
                let (label, content) = crate::tools::execute(
                    &call.name,
                    &call.arguments,
                    executor.cwd(),
                    confirm_writes,
                );
                crate::audit::action(&format!("yolo:{}", call.name), &label, None);
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content,
                });
                continue;
            }

            // MCP tools (namespaced mcp__server__tool) are proxied to the server.
            if crate::mcp::is_mcp_tool(&call.name) {
                println!("  🔌 {}", format!("mcp: {}", call.name).dim());
                let (label, content) = mcp.call(&call.name, &call.arguments);
                crate::audit::action(&format!("yolo:{}", call.name), &label, None);
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content,
                });
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
                messages.push(Msg::ToolResult {
                    call_id: call.id.clone(),
                    content: "No command provided.".to_string(),
                });
                continue;
            }

            // Sandbox: refuse network access / out-of-tree writes before running,
            // feeding the reason back to the model so it can adapt.
            if config.aishe.yolo_sandbox {
                if let Some(reason) = sandbox::sandbox_refusal(&command) {
                    println!("  {} {}", "⛔".red(), reason.as_str().yellow());
                    crate::audit::action("yolo:sandbox-refused", &command, None);
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: sandbox::refusal_message(&reason),
                    });
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
                    messages.push(Msg::ToolResult {
                        call_id: call.id.clone(),
                        content: "User declined to run this command.".to_string(),
                    });
                    continue;
                }
            }

            let (code, output) = executor.run_captured(&command, DEFAULT_CAPTURE_TIMEOUT);
            crate::audit::action("yolo", &command, Some(code));
            messages.push(Msg::ToolResult {
                call_id: call.id.clone(),
                content: format!("exit code {code}\n{output}"),
            });
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

    Ok(None)
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
            eprintln!("{}", format!("aishe: planning failed: {e}").red());
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
