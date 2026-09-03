//! Inline rendering for backend-neutral agent events.
//!
//! The renderer deliberately owns no policy or backend state. It prints only
//! normalized AIShe events, bounds every model-controlled field, and leaves
//! the user's submitted shell line untouched.

use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;

use super::AgentEvent;
use crate::ui::{Motion, StyleToken, TerminalCapabilities};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputMode {
    Focus,
    Compact,
    Detailed,
}

/// Static terminals receive each semantic phase at most once per turn. This
/// keeps long tasks informative without emitting one progress line per tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActivityPhase {
    Connecting,
    Planning,
    Acting,
    Waiting,
    Recovering,
    Finalizing,
}

impl ActivityPhase {
    fn for_status(value: &str) -> Self {
        if value.starts_with("connect") {
            Self::Connecting
        } else if value.starts_with("plan") {
            Self::Planning
        } else if value.starts_with("wait") {
            Self::Waiting
        } else if value.starts_with("reconnect")
            || value.starts_with("recover")
            || value.starts_with("attempt failed")
            || value.starts_with("compact")
        {
            Self::Recovering
        } else if value.starts_with("final") {
            Self::Finalizing
        } else {
            Self::Acting
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Planning => "planning",
            Self::Acting => "acting",
            Self::Waiting => "waiting",
            Self::Recovering => "recovering",
            Self::Finalizing => "finalizing",
        }
    }

    fn bit(self) -> u8 {
        1 << (self as u8)
    }
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
    capabilities: TerminalCapabilities,
    text_streamed: bool,
    answer_started: bool,
    status_visible: bool,
    static_phases_emitted: u8,
    final_text: String,
    completed_tools: usize,
    failed_tools: usize,
    changed_files: usize,
    changed_paths: Vec<String>,
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
        Self::with_capabilities(output, TerminalCapabilities::detect_stdout())
    }

    fn with_capabilities(output: &str, capabilities: TerminalCapabilities) -> Self {
        Self {
            mode: OutputMode::parse(output),
            capabilities,
            text_streamed: false,
            answer_started: false,
            status_visible: false,
            static_phases_emitted: 0,
            final_text: String::new(),
            completed_tools: 0,
            failed_tools: 0,
            changed_files: 0,
            changed_paths: Vec::new(),
            subagents: 0,
            reconnects: 0,
            commands: Vec::new(),
            started_at: Instant::now(),
            tools: HashMap::new(),
        }
    }

    pub fn render(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Connected => self.status("connecting"),
            AgentEvent::SessionCreated { .. }
            | AgentEvent::UserPromptAccepted { .. }
            | AgentEvent::ReasoningDelta { .. } => {}
            AgentEvent::ReasoningStarted => {
                if self.mode == OutputMode::Detailed {
                    self.line("  thinking", StyleToken::Activity);
                } else {
                    self.status("working");
                }
            }
            AgentEvent::ReasoningCompleted => {}
            AgentEvent::TextDelta { text } => {
                if self.mode == OutputMode::Detailed {
                    self.begin_assistant_answer();
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
                        self.begin_assistant_answer();
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
                let name = tool_name(&call.name);
                if name == "run command" {
                    self.commands.push(label.clone());
                }
                self.tools.insert(
                    call.call_id.clone(),
                    ActiveTool {
                        started_at: Instant::now(),
                        label: label.clone(),
                        name: name.clone(),
                    },
                );
                // ask_user owns the terminal for a real prompt panel; do not
                // leave a sticky "running ask user …" status that steals the
                // cursor line before the worker prints its box.
                if name == "ask user" {
                    self.clear_status();
                    if self.mode == OutputMode::Detailed {
                        self.line("  waiting for you: agent question", StyleToken::Warning);
                    }
                } else if self.mode == OutputMode::Detailed {
                    // run_command prints its exact command immediately before
                    // streaming child output. Other tools have no foreground
                    // stream, so render their label here.
                    if name != "run command" {
                        self.line(&format!("  running: {label}"), StyleToken::Activity);
                    }
                } else {
                    self.status(&format!("running {label}"));
                }
            }
            AgentEvent::ToolOutput { chunk, .. } if self.mode == OutputMode::Detailed => {
                for line in safe(chunk, 16 * 1024).lines().take(80) {
                    self.line(&format!("      {line}"), StyleToken::Muted);
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
                        &compact_completion(
                            label,
                            elapsed.as_deref(),
                            &result.output,
                            self.capabilities.glyphs().success(),
                        ),
                        StyleToken::Success,
                    );
                } else {
                    let name = active
                        .as_ref()
                        .map(|tool| tool.name.as_str())
                        .unwrap_or("tool action");
                    self.line(
                        &format!(
                            "  {} {name}{}",
                            self.capabilities.glyphs().success(),
                            elapsed.as_deref().unwrap_or("")
                        ),
                        StyleToken::Success,
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
                        StyleToken::Warning,
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
                        StyleToken::Warning,
                    );
                }
            }
            AgentEvent::Diff { diff } if self.mode == OutputMode::Detailed => {
                self.record_changed_file(&diff.path);
                self.line(
                    &format!("  changed: {}", safe(&diff.path, 4096)),
                    StyleToken::Activity,
                );
                for line in safe(&diff.patch, 64 * 1024).lines().take(200) {
                    let token = if line.starts_with('+') {
                        StyleToken::DiffAdd
                    } else if line.starts_with('-') {
                        StyleToken::DiffRemove
                    } else {
                        StyleToken::Muted
                    };
                    self.line(&format!("    {line}"), token);
                }
            }
            AgentEvent::Diff { diff } if self.mode == OutputMode::Focus => {
                self.record_changed_file(&diff.path);
                let status = self.live_summary("working");
                self.status(&status);
            }
            AgentEvent::Diff { diff } => {
                self.record_changed_file(&diff.path);
                self.line(
                    &format!(
                        "  {} changed  {}",
                        self.capabilities.glyphs().changed(),
                        safe(&diff.path, 4096)
                    ),
                    StyleToken::Activity,
                );
            }
            AgentEvent::TodoUpdated { items } if self.mode == OutputMode::Detailed => {
                self.line(
                    &format!("  plan      {} item(s)", items.len()),
                    StyleToken::Activity,
                );
                for item in items.iter().take(20) {
                    self.line(
                        &format!(
                            "    {} {}",
                            todo_mark(&item.status, &self.capabilities),
                            safe(&item.content, 512)
                        ),
                        StyleToken::Muted,
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
                        &format!(
                            "  {} agent    {} ({})",
                            self.capabilities.glyphs().branch(),
                            safe(agent, 128),
                            safe(child, 48)
                        ),
                        StyleToken::Activity,
                    );
                }
            }
            AgentEvent::SubagentCompleted { child, result } => {
                if self.mode != OutputMode::Detailed {
                    let status = self.live_summary("working");
                    self.status(&status);
                } else {
                    self.line(
                        &format!(
                            "  {} agent    {}  {}",
                            self.capabilities.glyphs().success(),
                            safe(child, 48),
                            first_line(result)
                        ),
                        StyleToken::Success,
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
                    StyleToken::Muted,
                );
            }
            AgentEvent::Usage { .. } => {}
            AgentEvent::Compacted if self.mode != OutputMode::Detailed => {
                let status = self.live_summary("compacting context");
                self.status(&status);
            }
            AgentEvent::Compacted => self.line("  context compacted", StyleToken::Warning),
            AgentEvent::WaitingForApproval { request } => {
                let panel = waiting_approval_panel(&self.capabilities, request);
                self.block(&panel, StyleToken::Warning);
            }
            AgentEvent::WaitingForUser { request } => {
                let panel = waiting_question_panel(
                    &self.capabilities,
                    &request.prompt,
                    request.agent.as_deref(),
                    request.task.as_deref(),
                    Some(&request.request_id),
                );
                self.block(&panel, StyleToken::Warning);
            }
            AgentEvent::Reconnecting { attempt } => {
                self.reconnects += 1;
                if self.mode == OutputMode::Focus {
                    self.status(&format!("reconnecting ({attempt}/3)"));
                } else {
                    self.line(
                        &format!("  reconnecting ({attempt}/3)"),
                        StyleToken::Warning,
                    );
                }
            }
            AgentEvent::Reconciled if self.mode == OutputMode::Focus => {
                let status = self.live_summary("working");
                self.status(&status);
            }
            AgentEvent::Reconciled => self.line(
                &format!(
                    "  {} session reconciled",
                    self.capabilities.glyphs().success()
                ),
                StyleToken::Success,
            ),
            AgentEvent::Aborted => self.line("  interrupted", StyleToken::Warning),
            AgentEvent::Completed { summary } if self.mode != OutputMode::Detailed => {
                self.status("finalizing");
                self.clear_status();
                if self.mode == OutputMode::Focus {
                    if let Some(commands) = self.command_summary() {
                        self.line(&format!("  commands: {commands}"), StyleToken::Muted);
                    }
                }
                if let Some(files) = self.changed_file_summary() {
                    self.line(&format!("  files: {files}"), StyleToken::Muted);
                }
                if let Some(activity) = self.activity_summary(true) {
                    let token = if self.failed_tools > 0 {
                        StyleToken::Warning
                    } else {
                        StyleToken::Success
                    };
                    let mark = if self.failed_tools > 0 {
                        self.capabilities.glyphs().warning()
                    } else {
                        self.capabilities.glyphs().success()
                    };
                    // begin_assistant_answer prints the separating blank line.
                    self.line(&format!("  {mark} {activity}"), token);
                }
                let final_text = if self.final_text.trim().is_empty() {
                    safe_multiline(summary, 256 * 1024)
                } else {
                    std::mem::take(&mut self.final_text)
                };
                if !final_text.trim().is_empty() {
                    self.begin_assistant_answer();
                    crate::modes::render_markdown(&final_text);
                }
            }
            AgentEvent::Completed { summary }
                if self.mode == OutputMode::Detailed && !summary.is_empty() =>
            {
                let (mark, token) = if self.failed_tools > 0 {
                    (self.capabilities.glyphs().warning(), StyleToken::Warning)
                } else {
                    (self.capabilities.glyphs().success(), StyleToken::Success)
                };
                self.line(&format!("  {mark} {}", safe(summary, 1024)), token);
                if let Some(activity) = self.activity_summary(true) {
                    self.line(&format!("  {activity}"), token);
                }
            }
            AgentEvent::Completed { .. } => {
                if let Some(activity) = self.activity_summary(true) {
                    let token = if self.failed_tools > 0 {
                        StyleToken::Warning
                    } else {
                        StyleToken::Success
                    };
                    self.line(&format!("  {activity}"), token);
                }
            }
            AgentEvent::Failed { error } => {
                self.clear_status();
                if let Some(files) = self.changed_file_summary() {
                    self.line(&format!("  files: {files}"), StyleToken::Muted);
                }
                if let Some(activity) = self.activity_summary(false) {
                    self.line(
                        &format!("  {} {activity}", self.capabilities.glyphs().warning()),
                        StyleToken::Warning,
                    );
                }
                self.line(
                    &format!("  AIShe error: {}", safe(&error.message, 4096)),
                    StyleToken::Danger,
                );
            }
        }
    }

    fn line(&mut self, value: &str, token: StyleToken) {
        self.clear_status();
        println!("{}", self.capabilities.paint(token, value));
    }

    fn block(&mut self, value: &str, token: StyleToken) {
        self.clear_status();
        print!("{}", self.capabilities.paint(token, value));
        if !value.ends_with('\n') {
            println!();
        }
        let _ = std::io::stdout().flush();
    }

    fn begin_assistant_answer(&mut self) {
        if self.answer_started {
            return;
        }
        self.clear_status();
        println!();
        println!("{}", self.capabilities.assistant_answer_header());
        self.answer_started = true;
    }

    fn status(&mut self, value: &str) {
        if self.mode == OutputMode::Detailed || !self.capabilities.is_tty {
            return;
        }
        if self.capabilities.motion == Motion::Static {
            let phase = ActivityPhase::for_status(value);
            let bit = phase.bit();
            if self.static_phases_emitted & bit != 0 {
                return;
            }
            println!(
                "{}",
                self.capabilities
                    .paint(StyleToken::Activity, &format!("  phase: {}", phase.label()))
            );
            self.static_phases_emitted |= bit;
            return;
        }
        // Some PTY hosts (notably `script` over SSH) initially report a
        // zero-column terminal. Treat that as unknown so the live command is
        // still useful instead of collapsing to a single character.
        let width = status_width(Some(self.capabilities.columns));
        // Measure cells, not chars: a wide path used to wrap and the next
        // clear only erased the last physical row.
        let line = crate::ui::truncate_cells_with(
            &safe(value, 4096),
            width.max(1),
            self.capabilities.glyphs(),
        );
        print!("\r\x1b[2K  {line}");
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

    fn record_changed_file(&mut self, path: &str) {
        let path = safe(path, 4096);
        if self.changed_paths.contains(&path) {
            return;
        }
        self.changed_files += 1;
        if self.changed_paths.len() < 8 {
            self.changed_paths.push(path);
        }
    }

    fn changed_file_summary(&self) -> Option<String> {
        if self.changed_paths.is_empty() {
            return None;
        }
        let mut summary = self
            .changed_paths
            .join(&format!("  {}  ", self.capabilities.glyphs().separator()));
        let remaining = self.changed_files.saturating_sub(self.changed_paths.len());
        if remaining > 0 {
            summary.push_str(&format!("  |  +{remaining} more"));
        }
        Some(safe(&summary, 320))
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
        if self.mode == OutputMode::Focus
            && self.capabilities.is_tty
            && crate::hints::details_hint_pending_here()
        {
            let _ = crate::hints::mark_details_hint_seen();
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
        let mut summary = visible.join(&format!("  {}  ", self.capabilities.glyphs().separator()));
        if remaining > 0 {
            summary.push_str(&format!("  |  +{remaining} more"));
        }
        Some(safe(&summary, 320))
    }
}

/// Shared, terminal-safe question panel used by normalized agent events and by
/// the foreground `ask_user` proxy. The returned text contains no styling;
/// callers apply the effective terminal policy once around the whole block.
pub(crate) fn waiting_question_panel(
    capabilities: &TerminalCapabilities,
    prompt: &str,
    agent: Option<&str>,
    task: Option<&str>,
    request_id: Option<&str>,
) -> String {
    let mut metadata = Vec::new();
    if let Some(agent) = agent.filter(|value| !value.is_empty()) {
        metadata.push(("agent", safe(agent, 128)));
    }
    if let Some(task) = task.filter(|value| !value.is_empty()) {
        metadata.push(("task", safe(task, 160)));
    }
    if let Some(request_id) = request_id.filter(|value| !value.is_empty()) {
        metadata.push(("request", safe(request_id, 96)));
    }
    metadata.push(("shell", "awaiting input".to_string()));
    waiting_panel(
        capabilities,
        "waiting for you: agent question",
        prompt,
        &metadata,
        "Type an answer and press Enter to continue.",
    )
}

fn waiting_approval_panel(
    capabilities: &TerminalCapabilities,
    request: &super::ApprovalRequest,
) -> String {
    let mut metadata = Vec::new();
    if let Some(agent) = request.agent.as_deref().filter(|value| !value.is_empty()) {
        metadata.push(("agent", safe(agent, 128)));
    }
    if let Some(task) = request.task.as_deref().filter(|value| !value.is_empty()) {
        metadata.push(("task", safe(task, 160)));
    }
    if !request.request_id.is_empty() {
        metadata.push(("request", safe(&request.request_id, 96)));
    }
    metadata.push((
        "risk",
        if request.dangerous {
            "dangerous action".to_string()
        } else {
            "approval required".to_string()
        },
    ));
    metadata.push(("default", "deny".to_string()));
    metadata.push(("shell", "awaiting approval input".to_string()));
    let detail = if request.title.trim().is_empty() {
        request.detail.clone()
    } else if request.detail.trim().is_empty() {
        request.title.clone()
    } else {
        format!("{}\n{}", request.title, request.detail)
    };
    waiting_panel(
        capabilities,
        "approval required",
        &detail,
        &metadata,
        "Approve explicitly or cancel.",
    )
}

fn waiting_panel(
    capabilities: &TerminalCapabilities,
    title: &str,
    body: &str,
    metadata: &[(&str, String)],
    footer: &str,
) -> String {
    let ascii = capabilities.glyphs().focus() == ">";
    let (left, right, vertical, lower_left, lower_right, horizontal) = if ascii {
        ("+", "+", "|", "+", "+", "-")
    } else {
        ("┌", "┐", "│", "└", "┘", "─")
    };
    let content_width = usize::from(capabilities.columns)
        .saturating_sub(6)
        .clamp(24, 100);
    let title = safe(title, content_width.saturating_sub(2));
    let title_width = crate::ui::cell_width(&title).min(content_width);
    let mut lines = Vec::new();
    lines.push(format!(
        "  {left}{horizontal} {title} {}{right}",
        horizontal.repeat(content_width.saturating_sub(title_width + 2))
    ));
    for (label, value) in metadata {
        for (index, line) in crate::ui::wrap_cells(value, content_width.saturating_sub(10))
            .into_iter()
            .enumerate()
        {
            let label = if index == 0 { *label } else { "" };
            lines.push(format!("  {vertical} {label:<8} {line}"));
        }
    }
    if !metadata.is_empty() {
        lines.push(format!("  {vertical}"));
    }
    let body = safe_multiline(body, 4096);
    for raw in body.lines() {
        for line in crate::ui::wrap_cells(raw.trim_end(), content_width) {
            lines.push(format!("  {vertical} {line}"));
        }
    }
    if body.is_empty() {
        lines.push(format!("  {vertical}"));
    }
    lines.push(format!("  {vertical}"));
    for line in crate::ui::wrap_cells(footer, content_width) {
        lines.push(format!("  {vertical} {line}"));
    }
    lines.push(format!(
        "  {lower_left}{}{lower_right}",
        horizontal.repeat(content_width + 1)
    ));
    format!("{}\n", lines.join("\n"))
}

fn tool_label(name: &str, arguments: &serde_json::Value) -> String {
    // The dependency-free OpenCode plugin presents one top-level `input`
    // object so optional JSON-Schema fields remain optional in v1.18.27. Events
    // therefore carry the provider-facing wrapper, while native/fallback tests
    // and completed bridge calls can still contain the direct argument shape.
    let arguments = arguments.get("input").unwrap_or(arguments);
    let name = tool_name(name);
    if let Some(command) = arguments.get("command").and_then(serde_json::Value::as_str) {
        // Multi-line scripts: first line + (+N lines), never \x0a walls.
        return format!(
            "{name}  {}",
            crate::commands::command_status_summary(command, 180)
        );
    }
    let detail = arguments
        .get("path")
        .or_else(|| arguments.get("url"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
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

fn compact_completion(label: &str, elapsed: Option<&str>, output: &str, mark: &str) -> String {
    let summary = first_line(output);
    let mut line = format!("  {mark} {label}{}", elapsed.unwrap_or(""));
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

fn todo_mark<'a>(status: &str, capabilities: &'a TerminalCapabilities) -> &'a str {
    let glyphs = capabilities.glyphs();
    match status {
        "completed" => glyphs.success(),
        "in_progress" => glyphs.active(),
        _ => glyphs.pending(),
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
    use crate::ui::CapabilityInputs;

    fn unicode_capabilities() -> TerminalCapabilities {
        TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            term: Some("xterm-256color".into()),
            locale: Some("en_US.UTF-8".into()),
            size: Some((80, 24)),
            ..CapabilityInputs::default()
        })
    }

    fn static_plain_capabilities(columns: u16) -> TerminalCapabilities {
        TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            term: Some("dumb".into()),
            locale: Some("C".into()),
            size: Some((columns, 24)),
            ..CapabilityInputs::default()
        })
    }

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
        assert_eq!(multiline, "run command  set -eu  (+1 lines)");
        assert!(!multiline.contains("\\x0a"));
        assert_eq!(todo_mark("completed", &unicode_capabilities()), "✓");
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
        let terminal = renderer.activity_summary(false).unwrap();
        assert!(terminal.contains("2 failed attempts"));
        assert!(!terminal.contains("recovered attempt"));

        let completion = compact_completion(
            "run command  docker ps",
            Some("  0.2s"),
            "container is running\nignored detail",
            "✓",
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
        assert!(commands.starts_with("docker ps  ·  docker inspect web"));
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

    #[test]
    fn question_and_approval_panels_are_plain_bounded_and_restore_status() {
        let capabilities = static_plain_capabilities(40);
        let long_prompt = format!(
            "second question \u{1b}[2J\n{}",
            "a very long agent question ".repeat(300)
        );
        let question = waiting_question_panel(
            &capabilities,
            &long_prompt,
            Some("planner"),
            Some("task-123"),
            Some("request-456"),
        );
        assert!(question.contains("waiting for you: agent"));
        assert!(question.contains("agent    planner"));
        assert!(question.contains("task     task-123"));
        assert!(question.contains("shell    awaiting input"));
        assert!(!question.contains('\u{1b}'));
        assert!(question.contains("\\x1b[2J"));
        assert!(question.chars().count() < 8_000);
        assert!(question
            .lines()
            .all(|line| crate::ui::cell_width(line) <= 42));
        // The top and bottom rules must be the same width: the corners used to
        // sit one column apart on every panel.
        let rules: Vec<&str> = question.lines().collect();
        assert_eq!(
            crate::ui::cell_width(rules[0]),
            crate::ui::cell_width(rules[rules.len() - 1]),
            "panel corners are misaligned:\n{question}"
        );

        let approval = waiting_approval_panel(
            &capabilities,
            &super::super::ApprovalRequest {
                request_id: "approval-1".into(),
                title: "Delete generated output?".into(),
                detail: "This may remove files.".into(),
                dangerous: true,
                agent: Some("builder".into()),
                task: Some("task-123".into()),
            },
        );
        assert!(approval.contains("approval required"));
        assert!(approval.contains("risk     dangerous action"));
        assert!(approval.contains("default  deny"));
        assert!(!approval.contains('\u{1b}'));

        let mut renderer = AgentRenderer::with_capabilities("focus", capabilities);
        renderer.status_visible = true;
        for index in 0..2 {
            renderer.render(&AgentEvent::WaitingForUser {
                request: super::super::UserQuestion {
                    request_id: format!("q-{index}"),
                    prompt: format!("question {index}"),
                    agent: Some("planner".into()),
                    task: Some("task-123".into()),
                },
            });
            assert!(!renderer.status_visible);
        }
    }

    #[test]
    fn static_progress_has_six_bounded_phases_and_truthful_completion_state() {
        let mut renderer = AgentRenderer::with_capabilities("focus", static_plain_capabilities(80));
        for value in [
            "connecting",
            "planning",
            "running command one",
            "running command two",
            "waiting for you",
            "attempt failed · continuing",
            "reconnecting (1/3)",
            "finalizing",
        ] {
            renderer.status(value);
        }
        assert_eq!(renderer.static_phases_emitted.count_ones(), 6);
        let emitted = renderer.static_phases_emitted;
        for index in 0..500 {
            renderer.status(&format!("running tool {index}"));
        }
        assert_eq!(renderer.static_phases_emitted, emitted);

        renderer.failed_tools = 1;
        renderer.reconnects = 2;
        renderer.record_changed_file("src/one.rs");
        renderer.record_changed_file("src/one.rs");
        for index in 2..=12 {
            renderer.record_changed_file(&format!("src/{index}.rs"));
        }
        let recovered = renderer.activity_summary(true).unwrap();
        assert!(recovered.contains("1 recovered attempt"));
        assert!(recovered.contains("12 files changed"));
        assert!(recovered.contains("2 reconnects"));
        let files = renderer.changed_file_summary().unwrap();
        assert!(files.contains("src/one.rs"));
        assert!(files.ends_with("+4 more"));
    }
}
