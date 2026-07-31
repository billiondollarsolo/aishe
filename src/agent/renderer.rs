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
    status_visible: bool,
    final_text: String,
    completed_tools: usize,
    failed_tools: usize,
    changed_files: usize,
    subagents: usize,
    reconnects: usize,
    commands: Vec<String>,
    started_at: Instant,
    tools: HashMap<String, ActiveTool>,
}

struct ActiveTool {
    started_at: Instant,
    label: String,
    name: String,
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
            status_visible: false,
            final_text: String::new(),
            completed_tools: 0,
            failed_tools: 0,
            changed_files: 0,
            subagents: 0,
            reconnects: 0,
            commands: Vec::new(),
            started_at: Instant::now(),
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
                } else {
                    self.status("working");
                }
            }
            AgentEvent::ReasoningCompleted => {}
            AgentEvent::TextDelta { text } => {
                if self.mode == OutputMode::Detailed {
                    print!("{}", safe_multiline(text, 64 * 1024));
                    let _ = std::io::stdout().flush();
                    self.text_streamed = true;
                } else {
                    self.final_text.push_str(&safe_multiline(text, 64 * 1024));
                }
            }
            AgentEvent::TextCompleted { text } => {
                if self.mode != OutputMode::Detailed {
                    // TextCompleted is authoritative for one assistant message.
                    // Keep only the latest completed message so intermediate
                    // narration never pollutes the native shell transcript.
                    self.final_text = safe_multiline(text, 256 * 1024);
                } else {
                    if !self.text_streamed && !text.is_empty() {
                        crate::modes::render_markdown(&safe_multiline(text, 256 * 1024));
                    } else if self.text_streamed {
                        crate::modes::rerender_streamed_markdown(&safe_multiline(text, 256 * 1024));
                    }
                    self.text_streamed = false;
                }
            }
            AgentEvent::ToolQueued { call } => {
                if self.mode != OutputMode::Detailed {
                    self.status(&format!(
                        "queued {}",
                        tool_label(&call.name, &call.arguments)
                    ));
                }
            }
            AgentEvent::ToolStarted { call } => {
                let label = tool_label(&call.name, &call.arguments);
                if tool_name(&call.name) == "run command" {
                    self.commands.push(label.clone());
                }
                self.tools.insert(
                    call.call_id.clone(),
                    ActiveTool {
                        started_at: Instant::now(),
                        label: label.clone(),
                        name: tool_name(&call.name),
                    },
                );
                if self.mode == OutputMode::Detailed {
                    // run_command prints its exact command immediately before
                    // streaming child output. Other tools have no foreground
                    // stream, so render their label here.
                    if tool_name(&call.name) != "run command" {
                        self.line(&format!("  ▶ {label}"), "36");
                    }
                } else {
                    self.status(&format!("running {label}"));
                }
            }
            AgentEvent::ToolOutput { chunk, .. } if self.mode == OutputMode::Detailed => {
                for line in safe(chunk, 16 * 1024).lines().take(80) {
                    self.line(&format!("      {line}"), "2");
                }
            }
            AgentEvent::ToolOutput { .. } => {}
            AgentEvent::ToolCompleted { call_id, result } => {
                let active = self.tools.remove(call_id);
                let elapsed = active.as_ref().map(|tool| elapsed_suffix(tool.started_at));
                self.completed_tools += 1;
                if self.mode == OutputMode::Focus {
                    let status = self.live_summary("working");
                    self.status(&status);
                } else if self.mode == OutputMode::Compact {
                    let label = active
                        .as_ref()
                        .map(|tool| tool.label.as_str())
                        .unwrap_or("tool action");
                    self.line(
                        &compact_completion(label, elapsed.as_deref(), &result.output),
                        "32",
                    );
                } else {
                    let name = active
                        .as_ref()
                        .map(|tool| tool.name.as_str())
                        .unwrap_or("tool action");
                    self.line(
                        &format!("  ✓ {name}{}", elapsed.as_deref().unwrap_or("")),
                        "32",
                    );
                }
            }
            AgentEvent::ToolFailed { call_id, error } => {
                let active = self.tools.remove(call_id);
                self.failed_tools += 1;
                if self.mode == OutputMode::Focus {
                    let status = self.live_summary("attempt failed · continuing");
                    self.status(&status);
                } else if self.mode == OutputMode::Compact {
                    let label = active
                        .as_ref()
                        .map(|tool| tool.label.as_str())
                        .unwrap_or("tool action");
                    let elapsed = active
                        .as_ref()
                        .map(|tool| elapsed_suffix(tool.started_at))
                        .unwrap_or_default();
                    self.line(
                        &format!("  ! {label}{elapsed}  {}", first_line(&error.message)),
                        "33",
                    );
                } else {
                    let name = active
                        .as_ref()
                        .map(|tool| tool.name.as_str())
                        .unwrap_or("tool action");
                    let elapsed = active
                        .as_ref()
                        .map(|tool| elapsed_suffix(tool.started_at))
                        .unwrap_or_default();
                    self.line(
                        &format!("  ! {name}{elapsed}  {}", first_line(&error.message)),
                        "33",
                    );
                }
            }
            AgentEvent::Diff { diff } if self.mode == OutputMode::Detailed => {
                self.changed_files += 1;
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
                let status = self.live_summary("working");
                self.status(&status);
            }
            AgentEvent::Diff { diff } => {
                self.changed_files += 1;
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
            AgentEvent::TodoUpdated { .. } if self.mode != OutputMode::Detailed => {
                let status = self.live_summary("planning");
                self.status(&status);
            }
            AgentEvent::TodoUpdated { .. } => {}
            AgentEvent::SubagentStarted { child, agent, .. } => {
                self.subagents += 1;
                if self.mode != OutputMode::Detailed {
                    self.status(&format!("delegating to {}", safe(agent, 128)));
                } else {
                    self.line(
                        &format!("  ↳ agent    {} ({})", safe(agent, 128), safe(child, 48)),
                        "36",
                    );
                }
            }
            AgentEvent::SubagentCompleted { child, result } => {
                if self.mode != OutputMode::Detailed {
                    let status = self.live_summary("working");
                    self.status(&status);
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
            AgentEvent::Compacted if self.mode != OutputMode::Detailed => {
                let status = self.live_summary("compacting context");
                self.status(&status);
            }
            AgentEvent::Compacted => self.line("  context compacted", "33"),
            AgentEvent::WaitingForUser { request } => {
                self.line(&format!("  ? {}", safe(&request.prompt, 4096)), "33");
            }
            AgentEvent::Reconnecting { attempt } => {
                self.reconnects += 1;
                if self.mode == OutputMode::Focus {
                    self.status(&format!("reconnecting ({attempt}/3)"));
                } else {
                    self.line(&format!("  ↻ reconnecting ({attempt}/3)"), "33");
                }
            }
            AgentEvent::Reconciled if self.mode == OutputMode::Focus => {
                let status = self.live_summary("working");
                self.status(&status);
            }
            AgentEvent::Reconciled => self.line("  ✓ session reconciled", "32"),
            AgentEvent::Aborted => self.line("  interrupted", "33"),
            AgentEvent::Completed { summary } if self.mode != OutputMode::Detailed => {
                self.clear_status();
                if self.mode == OutputMode::Focus {
                    if let Some(commands) = self.command_summary() {
                        self.line(&format!("  commands: {commands}"), "2");
                    }
                }
                if let Some(activity) = self.activity_summary(true) {
                    self.line(&format!("  ✓ {activity}"), "32");
                    println!();
                }
                let final_text = if self.final_text.trim().is_empty() {
                    safe_multiline(summary, 256 * 1024)
                } else {
                    std::mem::take(&mut self.final_text)
                };
                if !final_text.trim().is_empty() {
                    crate::modes::render_markdown(&final_text);
                }
            }
            AgentEvent::Completed { summary }
                if self.mode == OutputMode::Detailed && !summary.is_empty() =>
            {
                self.line(&format!("  ✓ {}", safe(summary, 1024)), "32");
            }
            AgentEvent::Completed { .. } => {
                if let Some(activity) = self.activity_summary(true) {
                    self.line(&format!("  ✓ {activity}"), "32");
                }
            }
            AgentEvent::Failed { error } => {
                self.line(&format!("  aishe: {}", safe(&error.message, 4096)), "31");
            }
        }
    }

    fn line(&mut self, value: &str, code: &str) {
        self.clear_status();
        if self.color {
            println!("\x1b[{code}m{value}\x1b[0m");
        } else {
            println!("{value}");
        }
    }

    fn status(&mut self, value: &str) {
        if self.mode == OutputMode::Detailed || !self.terminal {
            return;
        }
        // Some PTY hosts (notably `script` over SSH) initially report a
        // zero-column terminal. Treat that as unknown so the live command is
        // still useful instead of collapsing to a single character.
        let width = status_width(crossterm::terminal::size().ok().map(|(columns, _)| columns));
        print!("\r\x1b[2K  {}", safe(value, width.max(1)));
        let _ = std::io::stdout().flush();
        self.status_visible = true;
    }

    fn clear_status(&mut self) {
        if !self.status_visible {
            return;
        }
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
        self.status_visible = false;
    }

    fn live_summary(&self, prefix: &str) -> String {
        let mut parts = vec![prefix.to_string()];
        let actions = self.completed_tools + self.failed_tools;
        if actions > 0 {
            parts.push(counted(actions, "action", "actions"));
        }
        if self.changed_files > 0 {
            parts.push(counted(self.changed_files, "file", "files"));
        }
        parts.join(" · ")
    }

    fn activity_summary(&self, recovered: bool) -> Option<String> {
        let actions = self.completed_tools + self.failed_tools;
        if actions == 0 && self.changed_files == 0 && self.subagents == 0 && self.reconnects == 0 {
            return None;
        }
        let mut parts = Vec::new();
        if actions > 0 {
            parts.push(counted(actions, "action", "actions"));
        }
        if self.failed_tools > 0 {
            let label = if recovered {
                counted(self.failed_tools, "recovered attempt", "recovered attempts")
            } else {
                counted(self.failed_tools, "failed attempt", "failed attempts")
            };
            parts.push(label);
        }
        if self.changed_files > 0 {
            parts.push(counted(self.changed_files, "file changed", "files changed"));
        }
        if self.subagents > 0 {
            parts.push(counted(self.subagents, "subagent", "subagents"));
        }
        if self.reconnects > 0 {
            parts.push(counted(self.reconnects, "reconnect", "reconnects"));
        }
        parts.push(format_elapsed(self.started_at));
        if self.mode == OutputMode::Focus && self.terminal {
            parts.push("Ctrl-O details next turn".into());
        }
        Some(parts.join(" · "))
    }

    fn command_summary(&self) -> Option<String> {
        if self.commands.is_empty() {
            return None;
        }
        let mut commands = Vec::new();
        for label in &self.commands {
            let command = label
                .strip_prefix("run command  ")
                .unwrap_or(label.as_str());
            let command = safe(command, 96);
            if commands.last() != Some(&command) {
                commands.push(command);
            }
        }
        let visible = commands.iter().take(3).cloned().collect::<Vec<_>>();
        let remaining = commands.len().saturating_sub(visible.len());
        let mut summary = visible.join("  |  ");
        if remaining > 0 {
            summary.push_str(&format!("  |  +{remaining} more"));
        }
        Some(safe(&summary, 320))
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
    let name = tool_name(name);
    if detail.is_empty() {
        name
    } else {
        let detail = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        format!("{name}  {}", safe(&detail, 180))
    }
}

fn tool_name(name: &str) -> String {
    name.trim_start_matches("aishe_").replace('_', " ")
}

fn compact_completion(label: &str, elapsed: Option<&str>, output: &str) -> String {
    let summary = first_line(output);
    let mut line = format!("  ✓ {label}{}", elapsed.unwrap_or(""));
    if !summary.is_empty() {
        line.push_str("  ");
        line.push_str(&summary);
    }
    safe(&line, 280)
}

fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

fn elapsed_suffix(started_at: Instant) -> String {
    format!("  {}", format_elapsed(started_at))
}

fn format_elapsed(started_at: Instant) -> String {
    let seconds = started_at.elapsed().as_secs_f64();
    if seconds < 10.0 {
        format!("{seconds:.1}s")
    } else {
        format!("{seconds:.0}s")
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

fn safe_multiline(value: &str, limit: usize) -> String {
    let mut output = crate::commands::display_safe_multiline(value);
    if output.chars().count() > limit {
        output = output.chars().take(limit).collect::<String>();
        output.push('…');
    }
    output
}

fn status_width(columns: Option<u16>) -> usize {
    usize::from(columns.filter(|columns| *columns > 0).unwrap_or(80))
        .saturating_sub(2)
        .max(1)
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
        let multiline = tool_label(
            "aishe_run_command",
            &serde_json::json!({"command":"set -eu\necho ready"}),
        );
        assert_eq!(multiline, "run command  set -eu echo ready");
        assert!(!multiline.contains("\\x0a"));
        assert_eq!(todo_mark("completed"), "✓");
    }

    #[test]
    fn zero_width_ptys_fall_back_to_a_readable_status_line() {
        assert_eq!(status_width(None), 78);
        assert_eq!(status_width(Some(0)), 78);
        assert_eq!(status_width(Some(100)), 98);
    }

    #[test]
    fn summaries_are_compact_and_reclassify_recovered_attempts() {
        let mut renderer = AgentRenderer::new("focus");
        renderer.completed_tools = 5;
        renderer.failed_tools = 2;
        renderer.changed_files = 1;
        renderer.subagents = 1;
        let summary = renderer.activity_summary(true).unwrap();
        assert!(summary.contains("7 actions"));
        assert!(summary.contains("2 recovered attempts"));
        assert!(summary.contains("1 file changed"));
        assert!(summary.contains("1 subagent"));
        assert!(!summary.contains("failed attempt"));

        let completion = compact_completion(
            "run command  docker ps",
            Some("  0.2s"),
            "container is running\nignored detail",
        );
        assert_eq!(
            completion,
            "  ✓ run command  docker ps  0.2s  container is running"
        );

        renderer.commands = vec![
            "run command  docker ps".into(),
            "run command  docker inspect web".into(),
            "run command  curl -fsS http://127.0.0.1:8080".into(),
            "run command  docker logs web".into(),
        ];
        let commands = renderer.command_summary().unwrap();
        assert!(commands.starts_with("docker ps  |  docker inspect web"));
        assert!(commands.ends_with("+1 more"));
    }

    #[test]
    fn final_markdown_keeps_newlines_but_escapes_terminal_controls() {
        let text = safe_multiline(
            "# Current capacity\n\n- **2 vCPUs**\n```bash\necho ok\n```\u{1b}[2J",
            1024,
        );
        assert!(text.contains("# Current capacity\n\n- **2 vCPUs**"));
        assert!(text.contains("```bash\necho ok\n```"));
        assert!(!text.contains('\u{1b}'));
        assert!(text.ends_with("\\x1b[2J"));
    }
}
