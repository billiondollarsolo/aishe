//! Inline rendering for backend-neutral agent events.
//!
//! The renderer deliberately owns no policy or backend state. It prints only
//! normalized Aishe events, bounds every model-controlled field, and leaves
//! the user's submitted shell line untouched.

use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::time::Instant;

use super::AgentEvent;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Focus,
    Compact,
    Detailed,
}

impl OutputMode {
    fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "detailed" => Self::Detailed,
            "compact" => Self::Compact,
            _ => Self::Focus,
        }
    }
}

pub struct AgentRenderer {
    mode: OutputMode,
    color: bool,
    terminal: bool,
    text_streamed: bool,
    focus_status_visible: bool,
    focus_text: String,
    completed_tools: usize,
    changed_files: usize,
    tools: HashMap<String, Instant>,
}

impl AgentRenderer {
    pub fn new(output: &str) -> Self {
        let terminal = std::io::stdout().is_terminal()
            && std::env::var("TERM").ok().as_deref() != Some("dumb");
        Self {
            mode: OutputMode::parse(output),
            color: terminal && std::env::var_os("NO_COLOR").is_none(),
            terminal,
            text_streamed: false,
            focus_status_visible: false,
            focus_text: String::new(),
            completed_tools: 0,
            changed_files: 0,
            tools: HashMap::new(),
        }
    }

    pub fn render(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Connected
            | AgentEvent::SessionCreated { .. }
            | AgentEvent::UserPromptAccepted { .. }
            | AgentEvent::ReasoningDelta { .. }
            | AgentEvent::WaitingForApproval { .. } => {}
            AgentEvent::ReasoningStarted => {
                if self.mode == OutputMode::Detailed {
                    self.line("  • thinking", "2");
                } else if self.mode == OutputMode::Focus {
                    self.focus_status("working");
                }
            }
            AgentEvent::ReasoningCompleted => {}
            AgentEvent::TextDelta { text } => {
                if self.mode == OutputMode::Focus {
                    self.focus_text.push_str(&safe(text, 64 * 1024));
                } else {
                    print!("{}", safe(text, 64 * 1024));
                    let _ = std::io::stdout().flush();
                    self.text_streamed = true;
                }
            }
            AgentEvent::TextCompleted { text } => {
                if self.mode == OutputMode::Focus {
                    // TextCompleted is authoritative for one assistant message.
                    // Keep only the latest completed message so intermediate
                    // narration never pollutes the native shell transcript.
                    self.focus_text = safe(text, 256 * 1024);
                } else {
                    if !self.text_streamed && !text.is_empty() {
                        println!("{}", safe(text, 256 * 1024));
                    } else if self.text_streamed {
                        println!();
                    }
                    self.text_streamed = false;
                }
            }
            AgentEvent::ToolQueued { call } => {
                if self.mode == OutputMode::Focus {
                    self.focus_status(&format!(
                        "queued {}",
                        tool_label(&call.name, &call.arguments)
                    ));
                } else {
                    self.line(
                        &format!("  ○ queued   {}", tool_label(&call.name, &call.arguments)),
                        "2",
                    );
                }
            }
            AgentEvent::ToolStarted { call } => {
                self.tools.insert(call.call_id.clone(), Instant::now());
                if self.mode == OutputMode::Focus {
                    self.focus_status(&format!(
                        "running {}",
                        tool_label(&call.name, &call.arguments)
                    ));
                } else {
                    self.line(
                        &format!("  ● running  {}", tool_label(&call.name, &call.arguments)),
                        "36",
                    );
                }
            }
            AgentEvent::ToolOutput { chunk, .. } if self.mode == OutputMode::Detailed => {
                for line in safe(chunk, 16 * 1024).lines().take(80) {
                    self.line(&format!("      {line}"), "2");
                }
            }
            AgentEvent::ToolOutput { .. } => {}
            AgentEvent::ToolCompleted { call_id, result } => {
                let elapsed = self
                    .tools
                    .remove(call_id)
                    .map(|start| format!("  {:.1}s", start.elapsed().as_secs_f64()))
                    .unwrap_or_default();
                self.completed_tools += 1;
                if self.mode == OutputMode::Focus {
                    let status = self.focus_summary("working");
                    self.focus_status(&status);
                } else {
                    let summary = first_line(&result.output);
                    self.line(
                        &format!(
                            "  ✓ ran      {}{}{}",
                            safe(call_id, 48),
                            elapsed,
                            if summary.is_empty() {
                                String::new()
                            } else {
                                format!("  {summary}")
                            }
                        ),
                        "32",
                    );
                }
            }
            AgentEvent::ToolFailed { call_id, error } => {
                self.tools.remove(call_id);
                self.line(
                    &format!(
                        "  ✗ failed   {}  {}",
                        safe(call_id, 48),
                        safe(&error.message, 512)
                    ),
                    "31",
                );
            }
            AgentEvent::Diff { diff } if self.mode == OutputMode::Detailed => {
                self.line(&format!("  diff {}", safe(&diff.path, 4096)), "36");
                for line in safe(&diff.patch, 64 * 1024).lines().take(200) {
                    let color = if line.starts_with('+') {
                        "32"
                    } else if line.starts_with('-') {
                        "31"
                    } else {
                        "2"
                    };
                    self.line(&format!("    {line}"), color);
                }
            }
            AgentEvent::Diff { .. } if self.mode == OutputMode::Focus => {
                self.changed_files += 1;
                let status = self.focus_summary("working");
                self.focus_status(&status);
            }
            AgentEvent::Diff { diff } => {
                self.line(&format!("  Δ changed  {}", safe(&diff.path, 4096)), "36");
            }
            AgentEvent::TodoUpdated { items } if self.mode == OutputMode::Detailed => {
                self.line(&format!("  plan      {} item(s)", items.len()), "36");
                for item in items.iter().take(20) {
                    self.line(
                        &format!(
                            "    {} {}",
                            todo_mark(&item.status),
                            safe(&item.content, 512)
                        ),
                        "2",
                    );
                }
            }
            AgentEvent::TodoUpdated { .. } if self.mode == OutputMode::Focus => {
                let status = self.focus_summary("planning");
                self.focus_status(&status);
            }
            AgentEvent::TodoUpdated { .. } => {}
            AgentEvent::SubagentStarted { child, agent, .. } => {
                if self.mode == OutputMode::Focus {
                    self.focus_status(&format!("delegating to {}", safe(agent, 128)));
                } else {
                    self.line(
                        &format!("  ↳ agent    {} ({})", safe(agent, 128), safe(child, 48)),
                        "36",
                    );
                }
            }
            AgentEvent::SubagentCompleted { child, result } => {
                if self.mode == OutputMode::Focus {
                    let status = self.focus_summary("working");
                    self.focus_status(&status);
                } else {
                    self.line(
                        &format!("  ✓ agent    {}  {}", safe(child, 48), first_line(result)),
                        "32",
                    );
                }
            }
            AgentEvent::Usage { usage } if self.mode == OutputMode::Detailed => {
                let cost = usage
                    .cost_usd
                    .map(|value| format!(" · ${value:.4}"))
                    .unwrap_or_default();
                self.line(
                    &format!(
                        "  usage     {} in · {} out{}",
                        usage.input_tokens, usage.output_tokens, cost
                    ),
                    "2",
                );
            }
            AgentEvent::Usage { .. } => {}
            AgentEvent::Compacted if self.mode == OutputMode::Focus => {
                let status = self.focus_summary("compacting context");
                self.focus_status(&status);
            }
            AgentEvent::Compacted => self.line("  context compacted", "33"),
            AgentEvent::WaitingForUser { request } => {
                self.line(&format!("  ? {}", safe(&request.prompt, 4096)), "33");
            }
            AgentEvent::Reconnecting { attempt } => {
                self.line(&format!("  ↻ reconnecting ({attempt}/3)"), "33");
            }
            AgentEvent::Reconciled => self.line("  ✓ session reconciled", "32"),
            AgentEvent::Aborted => self.line("  interrupted", "33"),
            AgentEvent::Completed { summary } if self.mode == OutputMode::Focus => {
                self.clear_focus_status();
                let final_text = if self.focus_text.trim().is_empty() {
                    safe(summary, 256 * 1024)
                } else {
                    std::mem::take(&mut self.focus_text)
                };
                if !final_text.trim().is_empty() {
                    println!("{final_text}");
                }
            }
            AgentEvent::Completed { summary }
                if self.mode == OutputMode::Detailed && !summary.is_empty() =>
            {
                self.line(&format!("  ✓ {}", safe(summary, 1024)), "32");
            }
            AgentEvent::Completed { .. } => {}
            AgentEvent::Failed { error } => {
                self.line(&format!("  aishe: {}", safe(&error.message, 4096)), "31");
            }
        }
    }

    fn line(&mut self, value: &str, code: &str) {
        self.clear_focus_status();
        if self.color {
            println!("\x1b[{code}m{value}\x1b[0m");
        } else {
            println!("{value}");
        }
    }

    fn focus_status(&mut self, value: &str) {
        if self.mode != OutputMode::Focus || !self.terminal {
            return;
        }
        print!("\r\x1b[2K  {}", safe(value, 512));
        let _ = std::io::stdout().flush();
        self.focus_status_visible = true;
    }

    fn clear_focus_status(&mut self) {
        if !self.focus_status_visible {
            return;
        }
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
        self.focus_status_visible = false;
    }

    fn focus_summary(&self, prefix: &str) -> String {
        let mut parts = vec![prefix.to_string()];
        if self.completed_tools > 0 {
            parts.push(format!("{} tool(s)", self.completed_tools));
        }
        if self.changed_files > 0 {
            parts.push(format!("{} file(s)", self.changed_files));
        }
        parts.join(" · ")
    }
}

fn tool_label(name: &str, arguments: &serde_json::Value) -> String {
    // The dependency-free OpenCode plugin presents one top-level `input`
    // object so optional JSON-Schema fields remain optional in v1.18.9. Events
    // therefore carry the provider-facing wrapper, while native/fallback tests
    // and completed bridge calls can still contain the direct argument shape.
    let arguments = arguments.get("input").unwrap_or(arguments);
    let detail = arguments
        .get("command")
        .or_else(|| arguments.get("path"))
        .or_else(|| arguments.get("url"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    if detail.is_empty() {
        safe(name, 128)
    } else {
        format!("{}  {}", safe(name, 128), safe(detail, 512))
    }
}

fn first_line(value: &str) -> String {
    safe(value.lines().next().unwrap_or(""), 160)
}

fn todo_mark(status: &str) -> &'static str {
    match status {
        "completed" => "✓",
        "in_progress" => "●",
        _ => "○",
    }
}

fn safe(value: &str, limit: usize) -> String {
    let mut output = crate::commands::display_safe(value);
    if output.chars().count() > limit {
        output = output.chars().take(limit).collect::<String>();
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_bounded_and_escape_terminal_controls() {
        let label = tool_label(
            "aishe_run_command",
            &serde_json::json!({"command":"echo hi\u{1b}[2J"}),
        );
        assert!(!label.contains('\u{1b}'));
        assert!(label.contains("\\x1b"));
        assert!(tool_label(
            "aishe_run_command",
            &serde_json::json!({"input":{"command":"echo nested"}})
        )
        .contains("echo nested"));
        assert_eq!(todo_mark("completed"), "✓");
    }
}
