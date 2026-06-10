//! Yolo mode: an agentic loop where the model drives `run_command` until it has
//! accomplished the task, then summarizes.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::style::Stylize;

use super::{render_markdown, run_command_tool, safety_gate, GateOutcome};
use crate::config::Config;
use crate::context;
use crate::executor::{Executor, DEFAULT_CAPTURE_TIMEOUT};
use crate::providers::{AssistantMsg, Completion, Msg, Provider};
use crate::safety::{self, Risk};

/// Run the yolo agentic loop for one user request.
pub fn run(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    interrupt: &AtomicBool,
) -> Result<()> {
    let ctx = context::build(executor);
    let tools = [run_command_tool()];
    let mut messages: Vec<Msg> = vec![Msg::User(format!("{ctx}\nUser request: {input}"))];

    interrupt.store(false, Ordering::SeqCst);

    for iteration in 0..config.aishe.max_yolo_iterations {
        if interrupt.load(Ordering::SeqCst) {
            println!("  {}", "aborted".dim());
            return Ok(());
        }

        let completion: Completion =
            match provider.complete_with_tools(YOLO_SYSTEM, &messages, &tools) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{}", format!("aishe: {e}").red());
                    return Ok(());
                }
            };

        // No tool calls → final answer.
        if completion.tool_calls.is_empty() {
            if let Some(text) = &completion.text {
                render_markdown(text);
            }
            return Ok(());
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

    Ok(())
}

const YOLO_SYSTEM: &str = super::YOLO_SYSTEM_PROMPT;
