//! The commands supported by the lean shell, shared by help and ZLE completion.

pub struct Command {
    pub name: &'static str,
    pub group: &'static str,
    pub usage: &'static str,
    pub summary: &'static str,
    pub detail: &'static str,
}

pub const GROUPS: &[&str] = &["Shell", "Sessions", "Tools", "Setup"];

pub const COMMANDS: &[Command] = &[
    Command { name: "help", group: "Shell", usage: "/help [command|keys|all]", summary: "Quick guide and command help", detail: "Type / then Tab to browse commands. Use /help model for one command or /commands for the full list." },
    Command { name: "mode", group: "Shell", usage: "/mode ask|allow|agent|agent-host", summary: "Choose how the AI may act", detail: "Ask proposes commands. Allow runs safe suggestions and holds dangerous commands for yes. Agent requires a grant for this shell; workspace and host are separate scopes. Shift-Tab cycles modes on empty input." },
    Command { name: "model", group: "Shell", usage: "/model [name]", summary: "Choose a model for this shell", detail: "Open the model picker, or select a model by name. The selection applies to this shell; use the picker's default action to save it." },
    Command { name: "connection", group: "Shell", usage: "/connection [id]", summary: "Choose a connection for this shell", detail: "Open the connection picker, or select a saved connection by id. The selection applies to this shell; use the picker's default action to save it." },
    Command { name: "details", group: "Shell", usage: "/details", summary: "Cycle output detail", detail: "Cycle focus, compact, and detailed output for this shell. Ctrl-O performs the same action and keeps your editable input." },
    Command { name: "status", group: "Sessions", usage: "/status", summary: "Show mode, scope, model, and usage", detail: "Inspect this shell's mode, grant, connection, model, session, usage, and budget. MCP servers remain disconnected until /mcp or an agent turn needs them." },
    Command { name: "tasks", group: "Sessions", usage: "/tasks [id|subcommand]", summary: "Browse background work and results", detail: "Open a live task browser without starting an AI connection. Inspect progress, recorded checks, results, and changes. Answer questions, approve specific actions, and steer live work. Names, pins, archived results, and reviewed status persist across shells. Ctrl-X b opens it while keeping your editable input. Tab switches project/all; Ctrl-V selects current work, Needs you, or archived history. /tasks list and other task subcommands are also available." },
    Command { name: "inbox", group: "Sessions", usage: "/inbox [--json]", summary: "Answer background questions and action approvals", detail: "Open Needs you across projects. Answer a question or approve the exact requested action to continue its saved task with the original scope and remaining budget. Leaving the inbox keeps the request pending." },
    Command { name: "usage", group: "Sessions", usage: "/usage", summary: "Show this shell's tokens and cost", detail: "Usage includes calls across model and connection changes. Unpriced calls remain visible; the budget applies across this shell." },
    Command { name: "sessions", group: "Sessions", usage: "/sessions [list|clear|resume:<id>]", summary: "List or resume conversations", detail: "List saved conversations, resume one with /sessions resume:<id>, or clear this conversation with /sessions clear." },
    Command { name: "reset", group: "Sessions", usage: "/reset", summary: "Start a fresh conversation", detail: "Clear the current conversation and its saved transcript. This preserves the shell's usage and budget." },
    Command { name: "undo", group: "Sessions", usage: "/undo", summary: "Restore the last recorded file changes", detail: "Restore the last recorded AIShe file-change batch. Commands and external side effects cannot be undone this way." },
    Command { name: "commands", group: "Tools", usage: "/commands", summary: "Browse all slash commands", detail: "Show supported builtins and custom Markdown commands, grouped by purpose. User commands are discovered from the AIShe config directory." },
    Command { name: "skills", group: "Tools", usage: "/skills", summary: "List available skills", detail: "Load and list local skills without starting MCP servers or making model calls." },
    Command { name: "mcp", group: "Tools", usage: "/mcp", summary: "Discover connected tools", detail: "Connect configured MCP servers and list their tools. Connections are reused by later agent turns." },
    Command { name: "context", group: "Tools", usage: "/context [--explain|--json]", summary: "Preview the model's context", detail: "Inspect local context and its estimated size before sending a request. --explain shows the included sections; --json emits their metadata." },
    Command { name: "settings", group: "Setup", usage: "/settings", summary: "Edit saved defaults", detail: "Open settings for saved connection, model, mode, and other defaults. This shell keeps its current selection until you change it." },
    Command { name: "doctor", group: "Setup", usage: "/doctor [--json|--probe|--live]", summary: "Check setup and dependencies", detail: "Inspect configuration and local dependencies. --probe checks provider reachability; --live makes model calls to check capabilities." },
    Command { name: "setup", group: "Setup", usage: "/setup", summary: "Connect an AI provider", detail: "Open guided setup to create or repair saved defaults. This shell keeps its current connection until you explicitly change it." },
    Command { name: "tour", group: "Setup", usage: "/tour", summary: "Try the guided walkthrough", detail: "Learn AIShe in an isolated walkthrough workspace. Existing progress can be resumed." },
    Command { name: "backend", group: "Setup", usage: "/backend", summary: "Explain optional specialist backends", detail: "The lean shell uses the native agent. Specialist backends are opt-in; OpenCode requires AISHE_LEGACY_OPENCODE=1 in a new shell. Codex and Claude Code are reserved specialist names, not active controllers." },
];

pub fn find(name: &str) -> Option<&'static Command> {
    COMMANDS
        .iter()
        .find(|command| command.name == name.trim_start_matches('/'))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn hook_catalogue() -> String {
    let names = COMMANDS
        .iter()
        .map(|command| quote(command.name))
        .collect::<Vec<_>>()
        .join(" ");
    let descriptions = COMMANDS
        .iter()
        .map(|command| format!("{} {}", quote(command.name), quote(command.summary)))
        .collect::<Vec<_>>()
        .join(" ");
    let groups = COMMANDS
        .iter()
        .map(|command| format!("{} {}", quote(command.name), quote(command.group)))
        .collect::<Vec<_>>()
        .join(" ");
    let categories = GROUPS
        .iter()
        .map(|group| quote(group))
        .collect::<Vec<_>>()
        .join(" ");
    format!("typeset -ga _AISHE_SLASH_NAMES=({names})\ntypeset -gA _AISHE_SLASH_DESCRIPTIONS=({descriptions})\ntypeset -gA _AISHE_SLASH_GROUPS=({groups})\ntypeset -ga _AISHE_SLASH_CATEGORIES=({categories})")
}
