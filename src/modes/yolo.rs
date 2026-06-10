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
use crate::skills::SkillRegistry;

/// Run the yolo agentic loop for one user request.
pub fn run(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
    skills: &SkillRegistry,
) -> Result<()> {
    let ctx = context::build(executor);
    // Offer the use_skill tool (and advertise the catalog) only when skills exist.
    let tools: Vec<_> = if skills.is_empty() {
        vec![run_command_tool()]
    } else {
        vec![run_command_tool(), use_skill_tool()]
    };
    let system = if skills.is_empty() {
        YOLO_SYSTEM.to_string()
    } else {
        format!(
            "{YOLO_SYSTEM}\n\nAvailable skills (call use_skill to load one's \
             instructions when relevant):\n{}",
            skills.catalog()
        )
    };
    let mut messages: Vec<Msg> = vec![Msg::User(format!("{ctx}\nUser request: {input}"))];

    interrupt.store(false, Ordering::SeqCst);

    for iteration in 0..config.aishe.max_yolo_iterations {
        if interrupt.load(Ordering::SeqCst) {
            println!("  {}", "aborted".dim());
            super::report_usage(provider, config);
            return Ok(());
        }
        // Stop before the next model call if the session budget is spent.
        if super::budget_reached(provider, config) {
            super::report_usage(provider, config);
            return Ok(());
        }

        // Stream the assistant's prose live when streaming is on; otherwise wait
        // for the whole turn. `streamed` tracks whether any text was printed.
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
                eprintln!("{}", format!("aishe: {e}").red());
                return Ok(());
            }
        };

        // No tool calls → final answer.
        if completion.tool_calls.is_empty() {
            match (&completion.text, streamed) {
                // Re-render the streamed raw text as proper markdown in place.
                (Some(text), true) => super::rerender_streamed_markdown(text),
                // Not streamed: render markdown directly.
                (Some(text), false) => render_markdown(text),
                _ => {}
            }
            super::report_usage(provider, config);
            return Ok(());
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
                return Ok(());
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

    super::report_usage(provider, config);
    Ok(())
}

const YOLO_SYSTEM: &str = super::YOLO_SYSTEM_PROMPT;
