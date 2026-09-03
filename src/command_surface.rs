//! Authoritative metadata for AIShe's built-in slash-command surface.
//!
//! This registry deliberately describes product commands, not arbitrary clap
//! subcommands.  A slash alias is reserved as soon as it appears here: custom
//! commands and MCP prompts may only use names which are absent from the
//! registry.  Removed commands remain registered as tombstones for one
//! compatibility window so they fail locally with migration guidance instead
//! of being sent to an agent as natural language.

use std::collections::HashSet;
use std::fmt;

/// A user-facing place where a command may be invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Surface {
    /// The ordinary `aishe <command>` clap interface.
    Cli,
    /// Rust's `/name` classifier (including `aishe -c '/name'`).
    RustSlash,
    /// Execution inside the single-process `aishe -c` driver.
    OneShot,
    /// The full-buffer zsh integration.
    ZshHook,
    /// The bash `command_not_found_handle` integration.
    BashHook,
}

/// Support level for one command on one [`Surface`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceSupport {
    /// The command is implemented on this surface.
    Supported,
    /// The name is intentionally recognized only to produce a local diagnostic.
    Recognized(&'static str),
    /// The surface does not implement this command.
    Unavailable(&'static str),
}

impl SurfaceSupport {
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported)
    }
}

/// Availability on every currently declared front end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceAvailability {
    pub cli: SurfaceSupport,
    pub rust_slash: SurfaceSupport,
    pub one_shot: SurfaceSupport,
    pub zsh_hook: SurfaceSupport,
    pub bash_hook: SurfaceSupport,
}

impl SurfaceAvailability {
    pub const fn support(self, surface: Surface) -> SurfaceSupport {
        match surface {
            Surface::Cli => self.cli,
            Surface::RustSlash => self.rust_slash,
            Surface::OneShot => self.one_shot,
            Surface::ZshHook => self.zsh_hook,
            Surface::BashHook => self.bash_hook,
        }
    }
}

/// How arguments following a slash alias are interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentPolicy {
    None,
    OptionalValue(&'static str),
    PassThrough(&'static str),
}

/// The command's primary output contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputType {
    HumanText,
    StructuredOptional,
    Interactive,
    ShellState,
}

/// Broad side-effect class used by help, approval, and automation consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideEffectClass {
    ReadOnly,
    ShellState,
    PersistentConfig,
    Credentials,
    ConversationState,
    Mixed,
    None,
}

/// Whether correct behavior needs state to be handed back to the parent shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellLocalRequirement {
    None,
    OptionalHandoff,
    RequiredHandoff,
}

/// How zsh/bash implement a registered slash command. Static CLI invocations
/// are rendered from [`CommandSpec::cli`]; the other variants name the small
/// set of behaviors which genuinely require parent-shell state or compatibility
/// handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellHookAction {
    Cli,
    OneShot,
    AuthStatus,
    ToggleDetails,
    SessionMode,
    CompatibilityDiagnostic,
}

/// Lifecycle of a command identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Active,
    Tombstone {
        recognized_since: &'static str,
        guidance: &'static str,
    },
}

/// A canonical CLI spelling for a slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CliInvocation {
    pub command: &'static str,
    pub prefix_args: &'static [&'static str],
}

/// Declarative definition of one built-in command identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// Stable internal identity. This is not an alias and must not be renamed.
    pub id: &'static str,
    /// Canonical direct CLI invocation, when one exists.
    pub cli: Option<CliInvocation>,
    /// Names reserved in the `/name` namespace (without the leading slash).
    pub slash_aliases: &'static [&'static str],
    pub summary: &'static str,
    pub help_topic: &'static str,
    pub arguments: ArgumentPolicy,
    pub availability: SurfaceAvailability,
    pub output: OutputType,
    pub side_effects: SideEffectClass,
    pub shell_local: ShellLocalRequirement,
    pub lifecycle: Lifecycle,
}

impl CommandSpec {
    pub const fn is_active(self) -> bool {
        matches!(self.lifecycle, Lifecycle::Active)
    }

    pub fn has_alias(&self, name: &str) -> bool {
        self.slash_aliases.contains(&name)
    }

    pub const fn support(self, surface: Surface) -> SurfaceSupport {
        self.availability.support(surface)
    }

    /// Shell implementation selected by stable identity. Alias spelling never
    /// participates in this choice, so adding an alias cannot create a second
    /// hand-maintained shell case.
    pub fn hook_action(self) -> ShellHookAction {
        match self.lifecycle {
            Lifecycle::Tombstone { .. } => ShellHookAction::CompatibilityDiagnostic,
            Lifecycle::Active => match self.id {
                "auth" => ShellHookAction::AuthStatus,
                "details" => ShellHookAction::ToggleDetails,
                "mode" => ShellHookAction::SessionMode,
                "usage" => ShellHookAction::OneShot,
                _ => ShellHookAction::Cli,
            },
        }
    }
}

const SUPPORTED: SurfaceSupport = SurfaceSupport::Supported;
const CLI_ONLY: SurfaceSupport = SurfaceSupport::Unavailable(
    "this slash form has no one-shot handler; run the direct `aishe` CLI command",
);
const TOP_LEVEL_ONLY: SurfaceSupport =
    SurfaceSupport::Unavailable("this is a top-level CLI command and has no slash form");
const SHELL_ONLY: SurfaceSupport = SurfaceSupport::Unavailable(
    "this command needs an interactive shell and cannot run through `aishe -c`",
);
const REMOVED: SurfaceSupport = SurfaceSupport::Unavailable("the command has been removed");
const TOMBSTONE: SurfaceSupport =
    SurfaceSupport::Recognized("removed command retained for migration guidance");

const fn surfaces(
    one_shot: SurfaceSupport,
    zsh_hook: SurfaceSupport,
    bash_hook: SurfaceSupport,
) -> SurfaceAvailability {
    SurfaceAvailability {
        cli: SUPPORTED,
        rust_slash: SUPPORTED,
        one_shot,
        zsh_hook,
        bash_hook,
    }
}

const fn shell_only_surfaces() -> SurfaceAvailability {
    surfaces(SHELL_ONLY, SUPPORTED, SUPPORTED)
}

const fn cli_only_surfaces() -> SurfaceAvailability {
    surfaces(CLI_ONLY, SUPPORTED, SUPPORTED)
}

const fn top_level_cli_surfaces() -> SurfaceAvailability {
    SurfaceAvailability {
        cli: SUPPORTED,
        rust_slash: TOP_LEVEL_ONLY,
        one_shot: TOP_LEVEL_ONLY,
        zsh_hook: TOP_LEVEL_ONLY,
        bash_hook: TOP_LEVEL_ONLY,
    }
}

const fn tombstone_surfaces() -> SurfaceAvailability {
    SurfaceAvailability {
        cli: REMOVED,
        rust_slash: TOMBSTONE,
        one_shot: TOMBSTONE,
        zsh_hook: TOMBSTONE,
        bash_hook: TOMBSTONE,
    }
}

const fn cli(command: &'static str) -> Option<CliInvocation> {
    Some(CliInvocation {
        command,
        prefix_args: &[],
    })
}

const fn cli_with(
    command: &'static str,
    prefix_args: &'static [&'static str],
) -> Option<CliInvocation> {
    Some(CliInvocation {
        command,
        prefix_args,
    })
}

/// The complete built-in slash namespace for the 0.6.5 compatibility line.
///
/// Keep active entries before tombstones. Registry validation and tests make
/// duplicate aliases, missing summaries/help topics, and malformed names hard
/// failures.
pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "help",
        cli: cli("commands"),
        slash_aliases: &["help", "commands"],
        summary: "Show task-oriented AIShe help",
        help_topic: "help",
        arguments: ArgumentPolicy::OptionalValue("TOPIC"),
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "connection",
        cli: cli_with("connection", &["pick"]),
        slash_aliases: &["connection", "provider"],
        summary: "Inspect or switch the active account connection",
        help_topic: "accounts",
        arguments: ArgumentPolicy::OptionalValue("ID_OR_LABEL"),
        availability: shell_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "auth",
        cli: cli_with("auth", &["status"]),
        slash_aliases: &["auth"],
        summary: "Show authentication state for the active connection",
        help_topic: "accounts",
        arguments: ArgumentPolicy::None,
        availability: shell_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "model",
        cli: cli("model"),
        slash_aliases: &["model"],
        summary: "Inspect or select a model on the active connection",
        help_topic: "models",
        arguments: ArgumentPolicy::OptionalValue("MODEL"),
        availability: shell_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "mode",
        cli: cli("mode"),
        slash_aliases: &["mode"],
        summary: "Inspect or select suggest, auto, or yolo mode",
        help_topic: "session",
        arguments: ArgumentPolicy::PassThrough("MODE [--default]"),
        availability: cli_only_surfaces(),
        output: OutputType::ShellState,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::RequiredHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "scope",
        cli: cli("scope"),
        slash_aliases: &["scope"],
        summary: "Inspect or select workspace or host agent scope",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("SCOPE"),
        availability: cli_only_surfaces(),
        output: OutputType::ShellState,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "network",
        cli: cli("network"),
        slash_aliases: &["network"],
        summary: "Inspect or select workspace-agent network policy",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("allow|deny"),
        availability: cli_only_surfaces(),
        output: OutputType::ShellState,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "reasoning",
        cli: cli("reasoning"),
        slash_aliases: &["reasoning"],
        summary: "Inspect or select reasoning effort",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("LEVEL"),
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::ShellState,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "details",
        cli: None,
        slash_aliases: &["details"],
        summary: "Toggle focused and detailed agent output for this shell",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: SurfaceAvailability {
            cli: SurfaceSupport::Unavailable("use `/details` or Ctrl-O in the interactive shell"),
            rust_slash: SUPPORTED,
            one_shot: SHELL_ONLY,
            zsh_hook: SUPPORTED,
            bash_hook: SUPPORTED,
        },
        output: OutputType::ShellState,
        side_effects: SideEffectClass::ShellState,
        shell_local: ShellLocalRequirement::RequiredHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "status",
        cli: cli("status"),
        slash_aliases: &["status"],
        summary: "Show the effective connection, model, mode, scope, and usage",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "hints",
        cli: cli("hints"),
        slash_aliases: &[],
        summary: "Inspect or reset local discovery-hint seen-state",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("status [--json] | reset"),
        availability: top_level_cli_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "usage",
        cli: cli("usage"),
        slash_aliases: &["usage"],
        summary: "Show token and estimated-cost usage",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "log",
        cli: cli_with("log", &["-n", "20"]),
        slash_aliases: &["log"],
        summary: "Show recent audit events and agent actions",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "reset",
        cli: cli("reset"),
        slash_aliases: &["reset"],
        summary: "Start a fresh conversation without deleting the prior session",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: shell_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "settings",
        cli: cli("settings"),
        slash_aliases: &["settings"],
        summary: "Open the transactional settings editor",
        help_topic: "config",
        arguments: ArgumentPolicy::None,
        availability: shell_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "output",
        cli: cli("output"),
        slash_aliases: &["output"],
        summary: "Inspect or save persistent agent transcript density",
        help_topic: "config",
        arguments: ArgumentPolicy::OptionalValue("DENSITY"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "config",
        cli: cli("config"),
        slash_aliases: &["config"],
        summary: "Print the active AIShe configuration",
        help_topic: "config",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "skills",
        cli: cli("skills"),
        slash_aliases: &["skills"],
        summary: "List model-invoked skills",
        help_topic: "config",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "mcp",
        cli: cli("mcp"),
        slash_aliases: &["mcp"],
        summary: "List configured MCP tools and prompts",
        help_topic: "config",
        arguments: ArgumentPolicy::None,
        availability: surfaces(SUPPORTED, SUPPORTED, SUPPORTED),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "palette",
        cli: cli("palette"),
        slash_aliases: &["palette"],
        summary: "Search AIShe actions from one focused menu",
        help_topic: "help",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "agent",
        cli: cli("agent"),
        slash_aliases: &["agent"],
        summary: "Launch a controlled foreground or background agent",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("OPTIONS OBJECTIVE"),
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "inbox",
        cli: cli("inbox"),
        slash_aliases: &["inbox"],
        summary: "Review agent work that needs attention",
        help_topic: "agent",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "sessions",
        cli: cli("sessions"),
        slash_aliases: &["sessions"],
        summary: "Browse, resume, inspect, or fork AI sessions",
        help_topic: "session",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "resume",
        cli: cli("resume"),
        slash_aliases: &["resume"],
        summary: "Resume the latest interrupted task or a session by ID",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("ID"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "fork",
        cli: cli_with("session", &["fork"]),
        slash_aliases: &["fork"],
        summary: "Fork a managed conversation and switch this shell to it",
        help_topic: "session",
        arguments: ArgumentPolicy::OptionalValue("SESSION_ID"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "task",
        cli: cli("task"),
        slash_aliases: &["task"],
        summary: "Start and manage isolated background agent tasks",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("ACTION OPTIONS"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "plan",
        cli: cli("plan"),
        slash_aliases: &["plan"],
        summary: "Inspect or edit a durable agent checklist",
        help_topic: "agent",
        arguments: ArgumentPolicy::OptionalValue("TASK_ID"),
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "replan",
        cli: cli("replan"),
        slash_aliases: &["replan"],
        summary: "Revise a checklist while retaining completed evidence",
        help_topic: "agent",
        arguments: ArgumentPolicy::OptionalValue("TASK_ID"),
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "context",
        cli: cli("context"),
        slash_aliases: &["context"],
        summary: "Inspect exact model-visible local context and token estimates",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("OPTIONS"),
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "last",
        cli: cli("last"),
        slash_aliases: &["last"],
        summary: "Explain, fix, retry, or clear the last shell failure",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("ACTION"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "role",
        cli: cli("role"),
        slash_aliases: &["role"],
        summary: "Inspect or configure workload model roles",
        help_topic: "models",
        arguments: ArgumentPolicy::PassThrough("ACTION OPTIONS"),
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "ask",
        cli: cli("ask"),
        slash_aliases: &["ask"],
        summary: "Ask a non-executing question with optional structured output",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("OPTIONS QUESTION"),
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::OptionalHandoff,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "index",
        cli: cli("index"),
        slash_aliases: &["index"],
        summary: "Build or search the bounded repository index",
        help_topic: "agent",
        arguments: ArgumentPolicy::PassThrough("OPTIONS"),
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "capabilities",
        cli: cli("capabilities"),
        slash_aliases: &["capabilities"],
        summary: "Show capability evidence for the active model",
        help_topic: "models",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "test",
        cli: cli("test"),
        slash_aliases: &["test"],
        summary: "Validate local UX or run paid live model/tool checks",
        help_topic: "models",
        arguments: ArgumentPolicy::OptionalValue("--live"),
        availability: cli_only_surfaces(),
        output: OutputType::StructuredOptional,
        side_effects: SideEffectClass::ReadOnly,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "demo",
        cli: cli("demo"),
        slash_aliases: &["demo"],
        summary: "Run the safe guided first-session demonstration",
        help_topic: "help",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::Interactive,
        side_effects: SideEffectClass::ConversationState,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "undo",
        cli: cli("undo"),
        slash_aliases: &["undo"],
        summary: "Undo the most recent journaled AI file change",
        help_topic: "agent",
        arguments: ArgumentPolicy::None,
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::Mixed,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "trust",
        cli: cli("trust"),
        slash_aliases: &["trust"],
        summary: "Trust a project AIShe configuration, command, or skill",
        help_topic: "config",
        arguments: ArgumentPolicy::OptionalValue("PATH"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "untrust",
        cli: cli("untrust"),
        slash_aliases: &["untrust"],
        summary: "Remove trust from a project AIShe file",
        help_topic: "config",
        arguments: ArgumentPolicy::OptionalValue("PATH"),
        availability: cli_only_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::PersistentConfig,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Active,
    },
    CommandSpec {
        id: "editor",
        cli: None,
        slash_aliases: &["editor"],
        summary: "Removed legacy prompt editor selector",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "AIShe now uses native shell line editing; configure zsh or bash directly",
        },
    },
    CommandSpec {
        id: "frontend",
        cli: None,
        slash_aliases: &["frontend"],
        summary: "Removed legacy front-end selector",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "use `aishe zsh` for the full PTY front end or `aishe init bash` for the bash hook",
        },
    },
    CommandSpec {
        id: "stream",
        cli: None,
        slash_aliases: &["stream"],
        summary: "Removed legacy streaming toggle",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "streaming is selected automatically by the active backend",
        },
    },
    CommandSpec {
        id: "structured",
        cli: None,
        slash_aliases: &["structured"],
        summary: "Removed legacy structured-output toggle",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "structured output is negotiated internally; use command-specific `--json` for automation",
        },
    },
    CommandSpec {
        id: "theme",
        cli: None,
        slash_aliases: &["theme"],
        summary: "Removed legacy prompt theme selector",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "use the terminal palette or set `NO_COLOR=1` for plain output",
        },
    },
    CommandSpec {
        id: "rehash",
        cli: None,
        slash_aliases: &["rehash"],
        summary: "Removed AIShe command-cache refresh",
        help_topic: "migration",
        arguments: ArgumentPolicy::None,
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "use bare `rehash` for zsh's executable cache; AIShe discovers commands automatically",
        },
    },
    CommandSpec {
        id: "ghost",
        cli: None,
        slash_aliases: &["ghost"],
        summary: "Removed legacy ghost-text toggle",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "use suggest mode with `aishe mode suggest`",
        },
    },
    CommandSpec {
        id: "sandbox",
        cli: None,
        slash_aliases: &["sandbox"],
        summary: "Removed legacy sandbox toggle",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "use `aishe scope workspace` and `aishe network deny`, then verify with `aishe readiness`",
        },
    },
    CommandSpec {
        id: "cache",
        cli: None,
        slash_aliases: &["cache"],
        summary: "Removed legacy response-cache toggle",
        help_topic: "migration",
        arguments: ArgumentPolicy::PassThrough("ARGS"),
        availability: tombstone_surfaces(),
        output: OutputType::HumanText,
        side_effects: SideEffectClass::None,
        shell_local: ShellLocalRequirement::None,
        lifecycle: Lifecycle::Tombstone {
            recognized_since: "0.6.5",
            guidance: "the legacy suggest-response cache was removed and has no direct replacement",
        },
    },
];

/// Look up a reserved slash name (without a leading slash).
pub fn by_slash_alias(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|spec| spec.has_alias(name))
}

/// Look up a stable command identity.
pub fn by_id(id: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|spec| spec.id == id)
}

/// Whether a name is reserved by either an active command or a tombstone.
pub fn is_reserved_slash(name: &str) -> bool {
    by_slash_alias(name).is_some()
}

/// A minimally parsed slash line. Quoting is intentionally left untouched:
/// current slash commands use whitespace arguments, and shell paths must not be
/// reinterpreted by this parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSlash<'a> {
    pub name: &'a str,
    pub args: Vec<&'a str>,
    pub spec: Option<&'static CommandSpec>,
}

pub fn parse_slash(line: &str) -> Option<ParsedSlash<'_>> {
    let rest = line.trim().strip_prefix('/')?;
    let mut parts = rest.split_whitespace();
    let name = parts.next()?;
    Some(ParsedSlash {
        name,
        args: parts.collect(),
        spec: by_slash_alias(name),
    })
}

/// Validate registry invariants. This is cheap enough for tests and generation
/// tools; runtime lookups use the immutable static table directly.
pub fn validate_registry() -> Result<(), RegistryError> {
    let mut ids = HashSet::new();
    let mut aliases = HashSet::new();
    for spec in COMMANDS {
        if spec.id.is_empty() || !ids.insert(spec.id) {
            return Err(RegistryError(format!(
                "empty or duplicate command id {:?}",
                spec.id
            )));
        }
        if spec.summary.trim().is_empty() || spec.help_topic.trim().is_empty() {
            return Err(RegistryError(format!(
                "command {} is missing summary/help metadata",
                spec.id
            )));
        }
        if spec.slash_aliases.is_empty() && spec.cli.is_none() {
            return Err(RegistryError(format!(
                "command {} has neither slash aliases nor a CLI invocation",
                spec.id
            )));
        }
        for alias in spec.slash_aliases {
            if alias.is_empty()
                || alias.starts_with('/')
                || alias.chars().any(char::is_whitespace)
                || !aliases.insert(*alias)
            {
                return Err(RegistryError(format!(
                    "command {} has an invalid or duplicate slash alias {:?}",
                    spec.id, alias
                )));
            }
        }
        if let Some(invocation) = spec.cli {
            if invocation.command.trim().is_empty() {
                return Err(RegistryError(format!(
                    "command {} has an empty CLI command",
                    spec.id
                )));
            }
        }
        match spec.lifecycle {
            Lifecycle::Active => {
                if !spec.slash_aliases.is_empty()
                    && !spec.support(Surface::RustSlash).is_supported()
                {
                    return Err(RegistryError(format!(
                        "active command {} is not implemented by Rust slash dispatch",
                        spec.id
                    )));
                }
                if spec.slash_aliases.is_empty()
                    && !matches!(
                        spec.support(Surface::RustSlash),
                        SurfaceSupport::Unavailable(_)
                    )
                {
                    return Err(RegistryError(format!(
                        "top-level-only command {} unexpectedly declares a slash surface",
                        spec.id
                    )));
                }
            }
            Lifecycle::Tombstone {
                recognized_since,
                guidance,
            } => {
                if recognized_since.trim().is_empty() || guidance.trim().is_empty() {
                    return Err(RegistryError(format!(
                        "tombstone {} is missing compatibility guidance",
                        spec.id
                    )));
                }
            }
        }
        for surface in [Surface::ZshHook, Surface::BashHook] {
            match spec.support(surface) {
                SurfaceSupport::Supported => {
                    if matches!(spec.hook_action(), ShellHookAction::Cli) && spec.cli.is_none() {
                        return Err(RegistryError(format!(
                            "command {} declares {surface:?} support without a CLI or special hook action",
                            spec.id
                        )));
                    }
                    if matches!(spec.hook_action(), ShellHookAction::CompatibilityDiagnostic) {
                        return Err(RegistryError(format!(
                            "active {surface:?} command {} uses a tombstone hook action",
                            spec.id
                        )));
                    }
                }
                SurfaceSupport::Recognized(_) => {
                    if !matches!(spec.hook_action(), ShellHookAction::CompatibilityDiagnostic) {
                        return Err(RegistryError(format!(
                            "recognized-only {surface:?} command {} has no compatibility action",
                            spec.id
                        )));
                    }
                }
                SurfaceSupport::Unavailable(_) => {}
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError(String);

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RegistryError {}

#[cfg(test)]
mod tests {
    use super::*;

    const ACTIVE_IDS: &[&str] = &[
        "help",
        "connection",
        "auth",
        "model",
        "mode",
        "scope",
        "network",
        "reasoning",
        "details",
        "status",
        "hints",
        "usage",
        "log",
        "reset",
        "settings",
        "output",
        "config",
        "skills",
        "mcp",
        "palette",
        "agent",
        "inbox",
        "sessions",
        "resume",
        "fork",
        "task",
        "plan",
        "replan",
        "context",
        "last",
        "role",
        "ask",
        "index",
        "capabilities",
        "test",
        "demo",
        "undo",
        "trust",
        "untrust",
    ];
    const TOMBSTONE_IDS: &[&str] = &[
        "editor",
        "frontend",
        "stream",
        "structured",
        "theme",
        "rehash",
        "ghost",
        "sandbox",
        "cache",
    ];

    #[test]
    fn registry_is_valid_and_complete_for_compatibility_line() {
        validate_registry().unwrap();
        let active: Vec<_> = COMMANDS
            .iter()
            .filter(|spec| spec.is_active())
            .map(|spec| spec.id)
            .collect();
        let tombstones: Vec<_> = COMMANDS
            .iter()
            .filter(|spec| !spec.is_active())
            .map(|spec| spec.id)
            .collect();
        assert_eq!(active, ACTIVE_IDS);
        assert_eq!(tombstones, TOMBSTONE_IDS);
    }

    #[test]
    fn every_alias_resolves_to_its_canonical_spec() {
        for spec in COMMANDS {
            for alias in spec.slash_aliases {
                assert_eq!(by_slash_alias(alias).map(|found| found.id), Some(spec.id));
                assert!(is_reserved_slash(alias));
            }
        }
        assert!(by_slash_alias("made-up-command").is_none());
    }

    #[test]
    fn primary_aliases_have_the_intended_identities() {
        let cases = [
            ("help", "help"),
            ("commands", "help"),
            ("connection", "connection"),
            ("provider", "connection"),
            ("auth", "auth"),
            ("scope", "scope"),
            ("network", "network"),
        ];
        for (alias, id) in cases {
            assert_eq!(by_slash_alias(alias).unwrap().id, id);
        }
    }

    #[test]
    fn hints_is_registered_as_top_level_cli_only_without_hook_or_slash_impact() {
        let hints = by_id("hints").unwrap();
        assert_eq!(hints.cli.unwrap().command, "hints");
        assert!(hints.slash_aliases.is_empty());
        assert_eq!(hints.support(Surface::Cli), SurfaceSupport::Supported);
        for surface in [
            Surface::RustSlash,
            Surface::OneShot,
            Surface::ZshHook,
            Surface::BashHook,
        ] {
            assert!(matches!(
                hints.support(surface),
                SurfaceSupport::Unavailable(_)
            ));
        }
        assert!(by_slash_alias("hints").is_none());
    }

    #[test]
    fn hook_actions_are_selected_by_stable_identity_not_alias() {
        let special = [
            ("auth", ShellHookAction::AuthStatus),
            ("details", ShellHookAction::ToggleDetails),
            ("mode", ShellHookAction::SessionMode),
            ("usage", ShellHookAction::OneShot),
        ];
        for &(id, expected) in &special {
            assert_eq!(by_id(id).unwrap().hook_action(), expected);
        }
        for spec in COMMANDS {
            let expected = match spec.lifecycle {
                Lifecycle::Tombstone { .. } => ShellHookAction::CompatibilityDiagnostic,
                Lifecycle::Active if special.iter().any(|(id, _)| *id == spec.id) => continue,
                Lifecycle::Active => ShellHookAction::Cli,
            };
            assert_eq!(spec.hook_action(), expected, "{} hook action", spec.id);
        }
    }

    #[test]
    fn slash_parser_preserves_paths_and_unknown_names_for_later_precedence() {
        let absolute = parse_slash("/usr/bin/env -i").unwrap();
        assert_eq!(absolute.name, "usr/bin/env");
        assert_eq!(absolute.args, ["-i"]);
        assert!(absolute.spec.is_none());

        let mcp = parse_slash("/server:prompt one two").unwrap();
        assert_eq!(mcp.name, "server:prompt");
        assert!(mcp.spec.is_none());
    }
}
