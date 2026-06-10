//! Yolo mode: an agentic loop where the model drives `run_command` until it has
//! accomplished the task, then summarizes.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::style::Stylize;

use super::{render_markdown, run_command_tool, safety_gate, use_skill_tool, GateOutcome};
use crate::config::Config;
use crate::context;
use crate::executor::{Executor, DEFAULT_CAPTURE_TIMEOUT};
use crate::providers::{AssistantMsg, Completion, Msg, Provider};
use crate::safety::{self, Risk};
use crate::session::Session;
use crate::skills::SkillRegistry;

/// Run the yolo agentic loop for one user request, priming and recording session
/// memory so follow-up requests have context.
pub fn run(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    session: &mut Session,
) -> Result<()> {
    let history = session.history();
    let outcome = run_loop(
        input, provider, executor, config, interrupt, skills, history,
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
fn run_loop(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
    history: Vec<Msg>,
) -> Result<Option<String>> {
    let ctx = context::build(executor, config.aishe.redact_secrets);
    // Tools: always run_command; the built-in file tools when enabled; use_skill
    // when skills exist.
    let mut tools = vec![run_command_tool()];
    if config.aishe.file_tools {
        tools.extend(crate::tools::file_tool_defs());
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
    if !skills.is_empty() {
        system.push_str(&format!(
            "\n\nAvailable skills (call use_skill to load one's instructions when \
             relevant):\n{}",
            skills.catalog()
        ));
    }
    let mut messages: Vec<Msg> = history;
    messages.push(Msg::User(format!("{ctx}\nUser request: {input}")));

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

            // Built-in file tools (read/write/edit/list) operate directly on the
            // filesystem, relative to the cwd.
            if crate::tools::is_file_tool(&call.name) {
                let (label, content) = crate::tools::execute(
                    &call.name,
                    &call.arguments,
                    executor.cwd(),
                    config.aishe.yolo_confirm_dangerous,
                );
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

            // Safety gate: confirm dangerous commands when configured.
            if config.aishe.yolo_confirm_dangerous {
                if let Risk::Dangerous(_) = safety::assess(&command) {
                    if let GateOutcome::Declined = safety_gate(&command) {
                        messages.push(Msg::ToolResult {
                            call_id: call.id.clone(),
                            content: "User declined to run this command.".to_string(),
                        });
                        continue;
                    }
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
