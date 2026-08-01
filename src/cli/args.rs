use clap::{Parser, Subcommand};

/// Version string shown by `aishe --version`: crate version plus the build's git
/// SHA and date (captured by `build.rs`).
pub(crate) const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AISHE_GIT_SHA"),
    ", ",
    env!("AISHE_BUILD_DATE"),
    ")"
);

#[derive(Parser, Debug)]
#[command(
    name = "aishe",
    version = VERSION,
    about = "AIShe (AI Shell): natural-language-aware shell",
    after_help = "AIShe is AI Shell; the CLI package is aishe.\n\
In the interactive shell: /connection switches account; /model lists models for the *active* account only.\n\
See also: aishe help · in-shell /help accounts|models|session|config"
)]
pub(crate) struct Args {
    /// Override the interaction mode for this session.
    #[arg(long, value_parser = ["suggest", "auto", "yolo"])]
    pub(crate) mode: Option<String>,
    /// Override the model for this session.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// Override the provider for this session.
    #[arg(long)]
    pub(crate) provider: Option<String>,
    /// Override the named connection for this process.
    #[arg(long)]
    pub(crate) connection: Option<String>,
    /// Run a single input non-interactively and exit.
    #[arg(short = 'c')]
    pub(crate) command: Option<String>,
    /// (shell hook) Suggest a command for a natural-language line: prints the
    /// command to stdout and the explanation/answer to stderr.
    #[arg(long, hide = true)]
    pub(crate) suggest_line: Option<String>,
    /// (shell hook) Run the yolo loop for a natural-language line.
    #[arg(long, hide = true)]
    pub(crate) yolo_line: Option<String>,
    /// (shell hook) Auto mode: print a suggested command and exit 0 if the
    /// safety gate deems it safe (caller runs it), or a non-zero code if
    /// dangerous (caller pre-fills it for review).
    #[arg(long, hide = true)]
    pub(crate) auto_line: Option<String>,
    /// (shell hook) Fix-the-last-command: given the failed command, print a
    /// corrected one. Reads the exit status from `$AISHE_LAST_EXIT`.
    #[arg(long, hide = true)]
    pub(crate) fix_line: Option<String>,
    /// (shell hook) Accept the configured yolo scope for this live shell.
    #[arg(long, hide = true)]
    pub(crate) accept_yolo: bool,
    #[command(subcommand)]
    pub(crate) cmd: Option<Cmd>,
}

#[derive(clap::Args, Debug)]
pub(crate) struct SetupArgs {
    /// Resume the last interrupted setup draft.
    #[arg(long)]
    pub(crate) resume: bool,
    /// Discard only the setup draft and start from the active config.
    #[arg(long)]
    pub(crate) restart: bool,
    /// Verify the active config without changing it.
    #[arg(long)]
    pub(crate) verify: bool,
    /// Configure from flags without opening a terminal UI.
    #[arg(long)]
    pub(crate) non_interactive: bool,
    /// Service preset: anthropic, openai, xai, groq, openrouter, together, ollama, custom.
    #[arg(long)]
    pub(crate) service: Option<String>,
    /// Provider base URL (host root, without /v1).
    #[arg(long)]
    pub(crate) base_url: Option<String>,
    /// Name of the environment variable containing the API key.
    #[arg(long)]
    pub(crate) key_env: Option<String>,
    /// Saved credential profile name.
    #[arg(long)]
    pub(crate) credential_profile: Option<String>,
    /// Model identifier.
    #[arg(long)]
    pub(crate) model: Option<String>,
    /// API transport: auto, responses, or chat.
    #[arg(long, value_parser = ["auto", "responses", "chat"])]
    pub(crate) transport: Option<String>,
    /// Safety profile.
    #[arg(long, value_parser = ["conservative", "balanced", "autonomous", "custom"])]
    pub(crate) profile: Option<String>,
    /// Input price in USD per million tokens.
    #[arg(long)]
    pub(crate) input_price: Option<f64>,
    /// Output price in USD per million tokens.
    #[arg(long)]
    pub(crate) output_price: Option<f64>,
    /// Make minimal live generation requests while validating.
    #[arg(long)]
    pub(crate) live: bool,
    /// Agent backend (enterprise setup currently supports opencode).
    #[arg(long, value_parser = ["opencode"])]
    pub(crate) backend: Option<String>,
    /// Install or repair the pinned managed OpenCode runtime.
    #[arg(long)]
    pub(crate) install_backend: bool,
    /// Install the runtime from an approved local archive.
    #[arg(long, value_name = "PATH", conflicts_with = "runtime_base_url")]
    pub(crate) runtime_file: Option<std::path::PathBuf>,
    /// Download the pinned runtime asset from this mirror base URL.
    #[arg(long, value_name = "URL", conflicts_with = "runtime_file")]
    pub(crate) runtime_base_url: Option<String>,
    /// Linux sandbox requirement.
    #[arg(long, value_parser = ["bwrap", "policy"])]
    pub(crate) sandbox: Option<String>,
    /// Explicitly authorize supported system package installation.
    #[arg(long)]
    pub(crate) install_system_deps: bool,
    /// Default agent execution scope.
    #[arg(long, value_parser = ["workspace", "host"])]
    pub(crate) default_scope: Option<String>,
    /// Workspace-agent network policy.
    #[arg(long, value_parser = ["allow", "deny"])]
    pub(crate) network: Option<String>,
    /// Native agent transcript density.
    #[arg(long, value_parser = ["focus", "compact", "detailed"])]
    pub(crate) output: Option<String>,
    /// Emit a stable machine-readable setup result.
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Cmd {
    /// Internal managed backend supervisor.
    #[command(name = "__backend-supervisor", hide = true)]
    BackendSupervisor,
    /// Configure and verify AIShe interactively, or provision it with flags.
    Setup(Box<SetupArgs>),
    /// Edit the current configuration through an interactive section hub.
    Settings {
        /// Print effective fields and their provenance as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage provider API keys and OAuth subscriptions in AIShe's private stores.
    Auth {
        #[command(subcommand)]
        cmd: aishe::auth::AuthCommand,
    },
    /// Run the resumable guided first-session tour.
    Tour {
        /// Discard only tour progress/workspace and begin at lesson one.
        #[arg(long)]
        restart: bool,
        /// Run every lesson without terminal prompts.
        #[arg(long)]
        non_interactive: bool,
    },
    /// Print a shell integration snippet: `eval "$(aishe init zsh)"`.
    Init {
        /// Shell to emit integration for (zsh or bash).
        shell: String,
    },
    /// Launch your real interactive zsh (with all native plugins) under aishe.
    Zsh,
    /// Check your environment: shell, config, front-end, provider, API key.
    Doctor {
        /// Also probe each provider in the chain for reachability (a short
        /// network request per endpoint; costs no tokens).
        #[arg(long)]
        probe: bool,
        /// Run minimal live text, structured-output, tools, and streaming checks.
        #[arg(long)]
        live: bool,
        /// Emit the structured report as JSON.
        #[arg(long)]
        json: bool,
        /// Apply safe, local, idempotent repairs (never installs packages).
        #[arg(long)]
        fix: bool,
        /// Write a redacted JSON support bundle to this path.
        #[arg(long, value_name = "PATH")]
        bundle: Option<std::path::PathBuf>,
    },
    /// Manage AIShe's private, compatibility-pinned agent runtime.
    Backend {
        #[command(subcommand)]
        cmd: BackendCmd,
    },
    /// Print a shell completion script for `aishe` itself (bash/zsh/fish/...).
    Completions {
        /// Shell to generate completions for.
        shell: clap_complete::Shell,
    },
    /// Print a roff man page for `aishe` (e.g. `aishe man > /usr/share/man/man1/aishe.1`).
    Man,
    /// Remove AIShe components by category; user state is preserved by default.
    Uninstall {
        /// Remove the running AIShe binary plus known completion/man artifacts.
        #[arg(long)]
        binary: bool,
        /// Remove managed OpenCode runtimes and disposable runtime caches.
        #[arg(long)]
        runtime: bool,
        /// Permanently remove AI sessions and tool journals.
        #[arg(long)]
        sessions: bool,
        /// Permanently remove config, credentials, commands, and skills.
        #[arg(long)]
        config: bool,
        /// Permanently remove shell and semantic history.
        #[arg(long)]
        history: bool,
        /// Permanently remove audit and undo journals.
        #[arg(long)]
        audit_undo: bool,
        /// Select every category, including all user state.
        #[arg(long)]
        all: bool,
        /// Show the exact removal plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Confirm the displayed plan non-interactively.
        #[arg(long)]
        yes: bool,
    },
    /// Trust the current project's `.aishe/config.toml` so its sensitive keys
    /// (provider/endpoint, MCP servers, audit logging, safety toggles, `yolo`)
    /// apply. Safe cosmetic keys apply without trust.
    Trust {
        /// List every trusted file instead of trusting one.
        #[arg(long)]
        list: bool,
        /// A specific project file to trust — a skill
        /// (`.aishe/skills/<name>/SKILL.md`) or a command
        /// (`.aishe/commands/<name>.md`). Defaults to this project's
        /// `.aishe/config.toml`.
        path: Option<std::path::PathBuf>,
    },
    /// Drop trust for the current project's `.aishe/config.toml`, or for a
    /// specific project file.
    Untrust {
        /// Drop trust for every trusted file, not just this one.
        #[arg(long)]
        all: bool,
        /// A specific project file to untrust. Defaults to this project's
        /// `.aishe/config.toml`.
        path: Option<std::path::PathBuf>,
    },
    /// Show or set the interaction mode (with a value, saves it to your config).
    Mode {
        #[arg(value_parser = ["suggest", "auto", "yolo"])]
        value: Option<String>,
    },
    /// Show or set the agent execution scope for future turns.
    Scope {
        #[arg(value_parser = ["workspace", "host"])]
        value: Option<String>,
    },
    /// Show or set network access for workspace-scoped agent turns.
    Network {
        #[arg(value_parser = ["allow", "deny"])]
        value: Option<String>,
    },
    /// Show or set persistent agent transcript density.
    Output {
        #[arg(value_parser = ["focus", "compact", "detailed"])]
        value: Option<String>,
    },
    /// Show or set reasoning effort for this shell; `auto` uses the model default.
    Reasoning {
        #[arg(value_parser = ["auto", "none", "low", "medium", "high", "xhigh", "max"])]
        value: Option<String>,
        /// Make the effort the default for new shells instead of this shell only.
        #[arg(long)]
        default: bool,
    },
    /// Select a model for the active connection (this shell, or default for new shells).
    Model {
        value: Option<String>,
        /// Apply the model on a named connection (scripting; prefer `/connection` interactively).
        #[arg(long)]
        connection: Option<String>,
        /// Make the selection the default for new shells.
        #[arg(long)]
        default: bool,
    },
    /// Manage named provider/authentication connections.
    Connection {
        #[command(subcommand)]
        cmd: ConnectionCmd,
    },
    /// Show or set the provider (with a value, saves it to your config).
    Provider {
        /// Set anthropic/openai, or use `test` to validate the active provider.
        #[arg()]
        value: Option<String>,
        /// With `provider test`, make minimal text/structured/tool/stream requests.
        #[arg(long)]
        live: bool,
        /// With `provider test`, emit the capability report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List models returned by the configured endpoint.
    Models {
        /// Unique provider family or connection to query (defaults to active).
        #[arg(long)]
        provider: Option<String>,
        /// Named connection to query (supports duplicate provider accounts).
        #[arg(long, conflicts_with = "provider")]
        connection: Option<String>,
        /// Ignore a cached capability record and request the endpoint again.
        #[arg(long)]
        refresh: bool,
        /// Emit a schema-versioned JSON model-list document.
        #[arg(long)]
        json: bool,
    },
    /// Show or apply a transparent safety profile.
    Profile {
        #[arg(value_parser = ["conservative", "balanced", "autonomous", "custom"])]
        value: Option<String>,
    },
    /// Check whether autonomous mode is ready for real work.
    Readiness {
        #[arg(long)]
        json: bool,
    },
    /// Manage per-model token prices used for estimates and budgets.
    Price {
        #[command(subcommand)]
        cmd: PriceCmd,
    },
    /// Print the active configuration.
    Config {
        /// Include effective-value provenance after project overlays.
        #[arg(long)]
        effective: bool,
        /// Emit JSON instead of TOML/text.
        #[arg(long)]
        json: bool,
    },
    /// List the MCP tools offered to yolo.
    Mcp,
    /// List primary slash-commands, or show task-oriented help.
    Commands {
        /// Optional topic: accounts, models, session, config.
        topic: Option<String>,
    },
    /// Explain whether a line will run in the shell, reach the agent, or invoke a builtin.
    Route {
        /// Emit the schema-versioned route decision as JSON.
        #[arg(long)]
        json: bool,
        /// The line to classify. Use `--` before lines that start with punctuation or options.
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        line: Vec<String>,
    },
    /// Show model, mode, scope, output, live spend, and audit-log state.
    Status {
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Inspect or reset local one-time discovery-hint seen-state.
    Hints {
        #[command(subcommand)]
        cmd: HintsCmd,
    },
    /// List model-invoked skills.
    Skills,
    /// Undo the most recent AI file change (from the built-in file tools).
    Undo {
        /// List recorded change sets instead of reverting.
        #[arg(long)]
        list: bool,
    },
    /// Show the audit log of AI calls and actions (needs logging enabled).
    Log {
        /// Only this session id.
        #[arg(long)]
        session: Option<String>,
        /// Only this event kind (for example ai_request, tool_call, tool_result, action).
        #[arg(long)]
        action: Option<String>,
        /// Only entries whose model name contains this substring.
        #[arg(long)]
        model: Option<String>,
        /// Only entries newer than this, e.g. 30m, 2h, 3d, 1w.
        #[arg(long)]
        since: Option<String>,
        /// Show at most the last N entries.
        #[arg(short = 'n', long)]
        limit: Option<usize>,
        /// Emit schema-versioned JSONL instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Summarize token usage and estimated cost from the audit log.
    Usage {
        /// Group by: model (default), connection, day, or session.
        #[arg(long, value_parser = ["model", "connection", "day", "session"])]
        by: Option<String>,
        /// Only entries newer than this, e.g. 30m, 2h, 3d, 1w.
        #[arg(long)]
        since: Option<String>,
        /// Include only this connection ID or unique label.
        #[arg(long)]
        connection: Option<String>,
    },
    /// Turn a natural-language request into a shell command (for scripting).
    /// Prints the command to stdout; exit 0 = safe/answer, 20 = flagged (either
    /// `dangerous`, or `unknown` when the gate cannot tell what the command runs
    /// — the command is still printed for review), 1 = no provider or no query.
    /// Use `--json` for structured output.
    Suggest {
        /// The natural-language request (any number of words).
        query: Vec<String>,
        /// Emit schema-versioned JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Semantic search over your shell history (opt-in; needs an embedder).
    History {
        #[command(subcommand)]
        cmd: HistoryCmd,
    },
    /// List durable AI task sessions.
    Sessions {
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start a fresh conversation in this AIShe shell without deleting the
    /// previous managed session.
    Reset,
    /// Inspect or manage one durable AI task session.
    Session {
        #[command(subcommand)]
        cmd: TaskSessionCmd,
    },
    /// Resume the most recent interrupted task, or a specific task ID.
    Resume {
        id: Option<String>,
        /// Replacement working directory when the original no longer exists.
        #[arg(long, value_name = "PATH")]
        cwd: Option<std::path::PathBuf>,
    },
    /// Preview a command's file changes against a throwaway copy of the working
    /// tree (read-only system, no network via bubblewrap), then keep or discard.
    DryRun {
        /// The shell command to preview.
        command: String,
        /// Apply the previewed changes to the real working tree (default: discard).
        #[arg(long)]
        apply: bool,
    },
    /// Inspect or configure the environment context sent to the model.
    Context {
        /// Explain included/excluded sections, sources, and token estimates.
        #[arg(long)]
        explain: bool,
        /// Include a proposed request in the token/cost estimate (text is not echoed).
        #[arg(long, value_name = "TEXT")]
        preview: Option<String>,
        /// Emit stable metadata JSON; section contents are intentionally omitted.
        #[arg(long)]
        json: bool,
        /// Persistently exclude an optional section (repeatable).
        #[arg(long, value_name = "SECTION")]
        exclude: Vec<String>,
        /// Persistently include an optional section (repeatable).
        #[arg(long, value_name = "SECTION")]
        include: Vec<String>,
    },
    /// Generate a runnable script + markdown runbook from a recorded session.
    Runbook {
        /// The audit session id to export (default: the most recent session).
        #[arg(long)]
        session: Option<String>,
        /// Output directory for the runbook files (default: current directory).
        #[arg(short = 'o', long)]
        out: Option<String>,
        /// Re-run the recorded commands through the safety gate (not the model).
        #[arg(long)]
        replay: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum ConnectionCmd {
    /// List configured connections without exposing secrets.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one connection and its authentication state.
    Show {
        id: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Interactively pick a connection for this shell (or default for new shells).
    Pick {
        /// Optional connection id/label filter or exact id.
        value: Option<String>,
        /// Make the selection the default for new shells.
        #[arg(long)]
        default: bool,
    },
    /// Add a connection.
    Add {
        id: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "auto", value_parser = ["auto", "responses", "chat"])]
        transport: String,
        #[arg(long, default_value = "api-key", value_parser = ["api-key", "oauth", "none", "auto"])]
        auth: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        credential: Option<String>,
        #[arg(long)]
        key_env: Option<String>,
        #[arg(long, value_parser = ["auto", "none", "low", "medium", "high", "xhigh", "max"])]
        reasoning: Option<String>,
    },
    /// Edit connection metadata, endpoint, model, or authentication binding.
    Edit {
        id: String,
        #[arg(long)]
        label: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, value_parser = ["auto", "responses", "chat"])]
        transport: Option<String>,
        #[arg(long, value_parser = ["api-key", "oauth", "none", "auto"])]
        auth: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        credential: Option<String>,
        #[arg(long)]
        key_env: Option<String>,
        #[arg(long, value_parser = ["auto", "none", "low", "medium", "high", "xhigh", "max"])]
        reasoning: Option<String>,
    },
    /// Remove exactly one connection. Credentials remain in their stores.
    Remove {
        id: String,
        #[arg(long)]
        yes: bool,
    },
    /// Select a connection in this shell or make it the default for new shells.
    Use {
        id: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        default: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum HistoryCmd {
    /// Find past commands by meaning, e.g. "the docker run with the volume mount".
    Search {
        /// The natural-language query (any number of words).
        query: Vec<String>,
        /// How many results to show.
        #[arg(short = 'n', long, default_value_t = 5)]
        limit: usize,
        /// Print only the matching command(s), no score column — for the recall
        /// key binding to pre-fill the line. Notices go to stderr; stdout stays
        /// empty when there's no match (or the feature is off).
        #[arg(long)]
        bare: bool,
    },
    /// (Re)build the semantic index from your shell-history log.
    Index {
        /// Re-embed everything from scratch instead of only new commands.
        #[arg(long)]
        rebuild: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum HintsCmd {
    /// Show whether discovery hints are enabled and the launch hint was seen.
    Status {
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Clear discovery seen-state only, so the launch hint can appear again.
    Reset,
}

#[derive(Subcommand, Debug)]
pub(crate) enum BackendCmd {
    /// Show the managed runtime and supervisor state.
    Status {
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Install the exact OpenCode runtime supported by this AIShe build.
    Install {
        /// Use a previously downloaded archive instead of the configured mirror.
        #[arg(long, value_name = "PATH")]
        from: Option<std::path::PathBuf>,
        /// Replace an already verified runtime.
        #[arg(long)]
        force: bool,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify runtime identity, checksum metadata, and executable version.
    Verify {
        /// Also start the authenticated server and run a health probe.
        #[arg(long)]
        live: bool,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Reinstall a missing or invalid managed runtime.
    Repair {
        /// Use a previously downloaded archive.
        #[arg(long, value_name = "PATH")]
        from: Option<std::path::PathBuf>,
        /// Emit stable JSON.
        #[arg(long)]
        json: bool,
    },
    /// Select the immediately previous verified runtime when compatible.
    Rollback,
    /// Gracefully stop the private backend supervisor.
    Stop,
    /// Print the private backend log tail.
    Logs {
        #[arg(long, default_value_t = 100)]
        tail: usize,
    },
    /// Remove inactive runtime staging/cache entries.
    Gc {
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
pub(crate) enum TaskSessionCmd {
    /// Show one task record.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Set a human-readable task name.
    Rename { id: String, name: String },
    /// Delete exactly one task record.
    Delete { id: String },
}

#[derive(Subcommand, Debug)]
pub(crate) enum PriceCmd {
    /// List built-in matches and exact user price overrides.
    List,
    /// Set input/output USD per million tokens for an exact model ID.
    Set {
        model: String,
        #[arg(long)]
        input: f64,
        #[arg(long)]
        output: f64,
    },
    /// Remove an exact user price override.
    Remove { model: String },
}

pub(crate) fn connection_action(command: &ConnectionCmd) -> aishe::cli::connection::Action {
    use aishe::cli::connection::Action;

    match command {
        ConnectionCmd::List { json } => Action::List { json: *json },
        ConnectionCmd::Show { id, json } => Action::Show {
            id: id.clone(),
            json: *json,
        },
        ConnectionCmd::Pick { value, default } => Action::Pick {
            value: value.clone(),
            default: *default,
        },
        ConnectionCmd::Add {
            id,
            provider,
            label,
            base_url,
            model,
            transport,
            auth,
            profile,
            credential,
            key_env,
            reasoning,
        } => Action::Add {
            id: id.clone(),
            provider: provider.clone(),
            label: label.clone(),
            base_url: base_url.clone(),
            model: model.clone(),
            transport: transport.clone(),
            auth: auth.clone(),
            profile: profile.clone(),
            credential: credential.clone(),
            key_env: key_env.clone(),
            reasoning: reasoning.clone(),
        },
        ConnectionCmd::Edit {
            id,
            label,
            base_url,
            model,
            transport,
            auth,
            profile,
            credential,
            key_env,
            reasoning,
        } => Action::Edit {
            id: id.clone(),
            label: label.clone(),
            base_url: base_url.clone(),
            model: model.clone(),
            transport: transport.clone(),
            auth: auth.clone(),
            profile: profile.clone(),
            credential: credential.clone(),
            key_env: key_env.clone(),
            reasoning: reasoning.clone(),
        },
        ConnectionCmd::Remove { id, yes } => Action::Remove {
            id: id.clone(),
            yes: *yes,
        },
        ConnectionCmd::Use { id, model, default } => Action::Use {
            id: id.clone(),
            model: model.clone(),
            default: *default,
        },
    }
}

pub(crate) fn history_action(command: &HistoryCmd) -> aishe::cli::history::Action {
    match command {
        HistoryCmd::Search { query, limit, bare } => aishe::cli::history::Action::Search {
            query: query.clone(),
            limit: *limit,
            bare: *bare,
        },
        HistoryCmd::Index { rebuild } => aishe::cli::history::Action::Index { rebuild: *rebuild },
    }
}

pub(crate) fn session_action(command: &TaskSessionCmd) -> aishe::cli::session::Action {
    match command {
        TaskSessionCmd::Show { id, json } => aishe::cli::session::Action::Show {
            id: id.clone(),
            json: *json,
        },
        TaskSessionCmd::Rename { id, name } => aishe::cli::session::Action::Rename {
            id: id.clone(),
            name: name.clone(),
        },
        TaskSessionCmd::Delete { id } => aishe::cli::session::Action::Delete { id: id.clone() },
    }
}

pub(crate) fn backend_action(command: &BackendCmd) -> aishe::cli::backend::Action {
    use aishe::cli::backend::Action;

    match command {
        BackendCmd::Status { json } => Action::Status { json: *json },
        BackendCmd::Install { from, force, json } => Action::Install {
            from: from.clone(),
            force: *force,
            json: *json,
        },
        BackendCmd::Verify { live, json } => Action::Verify {
            live: *live,
            json: *json,
        },
        BackendCmd::Repair { from, json } => Action::Repair {
            from: from.clone(),
            json: *json,
        },
        BackendCmd::Rollback => Action::Rollback,
        BackendCmd::Stop => Action::Stop,
        BackendCmd::Logs { tail } => Action::Logs { tail: *tail },
        BackendCmd::Gc { dry_run } => Action::Gc { dry_run: *dry_run },
    }
}

pub(crate) fn price_action(command: &PriceCmd) -> aishe::cli::settings::PriceAction {
    match command {
        PriceCmd::List => aishe::cli::settings::PriceAction::List,
        PriceCmd::Set {
            model,
            input,
            output,
        } => aishe::cli::settings::PriceAction::Set {
            model: model.clone(),
            input: *input,
            output: *output,
        },
        PriceCmd::Remove { model } => aishe::cli::settings::PriceAction::Remove {
            model: model.clone(),
        },
    }
}

impl Args {
    /// Whether the selected command promises JSON/JSONL stream ownership.
    /// Resolve this before any config migration can print a human notice.
    pub(crate) fn machine_output(&self) -> bool {
        match self.cmd.as_ref() {
            Some(Cmd::Setup(setup)) => setup.json,
            Some(
                Cmd::Settings { json }
                | Cmd::Doctor { json, .. }
                | Cmd::Provider { json, .. }
                | Cmd::Models { json, .. }
                | Cmd::Readiness { json }
                | Cmd::Config { json, .. }
                | Cmd::Route { json, .. }
                | Cmd::Status { json }
                | Cmd::Log { json, .. }
                | Cmd::Suggest { json, .. }
                | Cmd::Sessions { json }
                | Cmd::Context { json, .. },
            ) => *json,
            Some(Cmd::Connection {
                cmd: ConnectionCmd::List { json } | ConnectionCmd::Show { json, .. },
            }) => *json,
            Some(Cmd::Auth {
                cmd:
                    aishe::auth::AuthCommand::Status { json, .. }
                    | aishe::auth::AuthCommand::List { json },
            }) => *json,
            Some(Cmd::Backend {
                cmd:
                    BackendCmd::Status { json }
                    | BackendCmd::Install { json, .. }
                    | BackendCmd::Verify { json, .. }
                    | BackendCmd::Repair { json, .. },
            }) => *json,
            Some(Cmd::Session {
                cmd: TaskSessionCmd::Show { json, .. },
            }) => *json,
            Some(Cmd::Hints {
                cmd: HintsCmd::Status { json },
            }) => *json,
            _ => false,
        }
    }
}
