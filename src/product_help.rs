//! Task-first product help for `/help` and the built-in `aishe-product` skill.
//!
//! Keep this module the single source of truth for “how do I use AIShe?” so the
//! shell help surface and the model skill cannot drift apart.

use std::fmt::Write as _;

use crate::command_surface::{
    ArgumentPolicy, CommandSpec, Lifecycle, ShellLocalRequirement, SideEffectClass, Surface,
    SurfaceSupport, COMMANDS,
};
use crate::skills::Skill;

pub const COMMAND_REFERENCE_BEGIN: &str = "<!-- BEGIN GENERATED COMMAND SURFACE -->";
pub const COMMAND_REFERENCE_END: &str = "<!-- END GENERATED COMMAND SURFACE -->";

/// Print task-oriented help. `topic` is optional (`accounts`, `models`, …).
pub fn print_help(topic: Option<&str>) {
    print!("{}", render_help(topic));
}

/// Build task-oriented help. The prose remains task-first; command rows come
/// from the authoritative registry so aliases cannot silently drift.
pub fn render_help(topic: Option<&str>) -> String {
    let normalized = topic.map(|value| value.trim().to_ascii_lowercase());
    match normalized.as_deref() {
        None | Some("") | Some("help") | Some("all") => render_overview(),
        Some("accounts" | "account" | "connection" | "connections" | "auth" | "login") => {
            render_accounts()
        }
        Some("models" | "model") => render_models(),
        Some("session" | "status" | "keys") => render_session(),
        Some("agent" | "agents" | "task" | "tasks") => render_agent(),
        Some("config" | "setup" | "settings" | "doctor") => render_config(),
        Some("routing" | "route" | "input") => render_routing(),
        Some("migration" | "removed" | "legacy") => render_migration(),
        Some(other) => format!(
            "unknown help topic '{other}'\n\
             topics: accounts · models · agent · session · config · routing · migration  (or bare /help)\n"
        ),
    }
}

fn render_overview() -> String {
    let mut out = String::from(
        "AIShe (AI Shell) — what do you want to do?\n\n\
           Switch account/model:  /connection · /model\n\
           Inspect this session:  /status · /usage · /log\n\
           Change this session:   /mode · /reasoning · /details · /reset\n\
           Configure AIShe:       /settings · /scope · /network · /output\n\
           Add an account:        aishe setup\n\
           Diagnose health:       aishe doctor --live\n\
           Explain input routing: /help routing\n\n\
         Keys: Shift-Tab mode · Ctrl-O details · Alt-Enter natural language\n\n\
         Topics: /help accounts · models · agent · session · config · routing · migration\n\
         Ask:   how do I add a Codex OAuth account?\n\
         CLI:   aishe --help\n\n\
         Slash commands\n",
    );
    append_terminal_commands(&mut out, None);
    out
}

fn render_agent() -> String {
    let mut out = String::from(
        "Agent work\n\n\
           /                         searchable action palette\n\
           /agent                    guided foreground/background launcher\n\
           /inbox                    active and reviewable background work\n\
           /sessions                 resume, inspect, or fork conversations\n\
           /plan · /replan           durable task checklists and evidence\n\
           /context --show           exact redacted local model context\n\
           /test --live              paid text/structured/tool/stream validation\n\n\
         Related commands\n",
    );
    append_terminal_commands(&mut out, Some("agent"));
    out
}

fn render_accounts() -> String {
    let mut out = String::from(
        "Accounts & authentication\n\n\
         Labels\n\
           Codex - API              OpenAI API key\n\
           Codex - OAuth · work     ChatGPT/Codex subscription (profile work)\n\
           Grok - API               xAI API key\n\
           Grok - OAuth · work      SuperGrok subscription\n\n\
         Switch account\n\
           /connection              interactive picker for this shell\n\
           /connection ID           select by id or label\n\
           Enter, then y            make default for new shells\n\n\
         Add or sign in\n\
           aishe setup\n\
           aishe connection add my-codex --provider openai --auth oauth --profile work\n\
           aishe auth login openai --profile work     # Codex OAuth\n\
           aishe auth login xai --profile work        # Grok OAuth\n\
           aishe auth set openai                      # hidden API-key prompt\n\
           Successful OAuth login creates a selectable connection when needed.\n\n\
         Inspect\n\
           aishe connection list|show\n\n\
         Related commands\n",
    );
    append_terminal_commands(&mut out, Some("accounts"));
    out
}

fn render_models() -> String {
    let mut out = String::from(
        "Models\n\n\
           /model                   list models for the active connection\n\
           /model NAME              set model for this shell\n\
           Enter, then y            make default for new shells\n\n\
           OAuth catalogs come from managed OpenCode, not public GET /v1/models.\n\
           API-key catalogs come from the configured endpoint.\n\n\
           aishe models\n\
           aishe model gpt-5.5\n\
           aishe model --connection ID NAME\n\n\
         Related commands\n",
    );
    append_terminal_commands(&mut out, Some("models"));
    out
}

fn render_session() -> String {
    let mut out = String::from(
        "Session controls\n\n\
           Shift-Tab                cycle suggest / auto / yolo\n\
           Ctrl-O                   focus ↔ detailed agent output\n\
           Alt-Enter                force this buffer to the agent (zsh)\n\
           Ctrl-X ?                 show a non-color route cue for the zsh buffer\n\
           Ctrl-X Ctrl-F            stage a reviewed fix for the last failure\n\
           Ctrl-X Ctrl-R            semantic recall; Bash recalls last suggestion\n\
           suggest mode             first Enter stages; edit; second Enter runs\n\
           Ctrl-C                   cancel a staged suggestion without running it\n\
           picker                   arrows/Ctrl-P/N · Page Up/Down · Home/End\n\
                                    Enter accepts · Esc/Ctrl-C cancels\n\
           /connection and /model   this shell unless promoted for new shells\n\
           /scope                   durable; the hook refreshes this shell\n\
           /network                 durable workspace-agent policy\n\
           /reset                   new retained conversation\n\n\
         Key conflicts\n\
           Option/Alt must send Meta/Esc; `?` is the portable force-agent path.\n\
           Rebind with AISHE_NL_KEY, AISHE_MODE_KEY, AISHE_FIX_KEY, or\n\
           AISHE_RECALL_KEY. `aishe doctor` reports duplicate configured keys.\n\n\
         Related commands\n",
    );
    append_terminal_commands(&mut out, Some("session"));
    out
}

fn render_config() -> String {
    let mut out = String::from(
        "Config & health\n\n\
           /settings                transactional section hub\n\
           aishe setup              resumable first-run / reconfigure\n\
           aishe setup --verify     check current config only\n\
           aishe doctor --live      environment, backend, provider checks\n\
           aishe tour               guided first-session tour\n\
           aishe backend status     managed OpenCode runtime\n\n\
         Related commands\n",
    );
    append_terminal_commands(&mut out, Some("config"));
    out
}

fn render_routing() -> String {
    String::from(
        "Input routing\n\n\
           executable or path       runs as a shell command\n\
           ! line                   force shell-command routing\n\
           ? question               force natural-language routing\n\
           Alt-Enter                force the current line to natural language\n\
           Ctrl-X ?                 show shell/agent route as text in zsh\n\
           /name                    local built-in command (listed by /help)\n\
           other text               classified for the active interaction mode\n\n\
         Explain without executing\n\
           aishe route -- 'git status'\n\
           aishe route --json -- 'summarize this repository'\n\n\
         Paths and executable names win over natural-language heuristics. Use ! or ?\n\
         whenever you want an explicit route.\n",
    )
}

fn render_migration() -> String {
    let mut out = String::from(
        "Removed slash commands\n\n\
         These names are reserved tombstones. They fail locally and are never sent to a model.\n",
    );
    for spec in COMMANDS
        .iter()
        .filter(|spec| matches!(spec.lifecycle, Lifecycle::Tombstone { .. }))
    {
        let Lifecycle::Tombstone {
            recognized_since,
            guidance,
        } = spec.lifecycle
        else {
            unreachable!()
        };
        let _ = writeln!(
            out,
            "  {:<18} removed in {recognized_since}; {guidance}",
            slash_names(spec)
        );
    }
    out
}

fn argument_suffix(policy: ArgumentPolicy) -> String {
    match policy {
        ArgumentPolicy::None => String::new(),
        ArgumentPolicy::OptionalValue(label) => format!(" [{label}]"),
        ArgumentPolicy::PassThrough(label) => format!(" [{label}…]"),
    }
}

fn slash_names(spec: &CommandSpec) -> String {
    spec.slash_aliases
        .iter()
        .map(|alias| format!("/{alias}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn slash_usage(spec: &CommandSpec) -> String {
    let mut usage = slash_names(spec);
    usage.push_str(&argument_suffix(spec.arguments));
    usage
}

fn cli_usage(spec: &CommandSpec) -> String {
    let Some(invocation) = spec.cli else {
        return String::new();
    };
    let mut usage = format!("aishe {}", invocation.command);
    for argument in invocation.prefix_args {
        usage.push(' ');
        usage.push_str(argument);
    }
    usage.push_str(&argument_suffix(spec.arguments));
    usage
}

fn command_usage(spec: &CommandSpec) -> String {
    if spec.slash_aliases.is_empty() {
        cli_usage(spec)
    } else {
        slash_usage(spec)
    }
}

pub fn effect_label(spec: &CommandSpec) -> &'static str {
    match (spec.side_effects, spec.shell_local) {
        (SideEffectClass::ReadOnly, _) => "read-only",
        (SideEffectClass::ShellState, _) => "this shell",
        (SideEffectClass::ConversationState, _) => "session",
        (SideEffectClass::Credentials, _) => "credentials",
        (SideEffectClass::PersistentConfig, ShellLocalRequirement::OptionalHandoff) => {
            "durable; refreshes this shell"
        }
        (SideEffectClass::PersistentConfig, _) => "durable setting",
        (SideEffectClass::Mixed, ShellLocalRequirement::OptionalHandoff) => "this shell by default",
        (SideEffectClass::Mixed, _) => "may change state",
        (SideEffectClass::None, _) => "no effect",
    }
}

fn append_terminal_commands(out: &mut String, topic: Option<&str>) {
    for spec in COMMANDS.iter().filter(|spec| {
        spec.is_active()
            && topic.is_none_or(|topic| spec.help_topic == topic)
            && (!matches!(
                spec.support(Surface::ZshHook),
                SurfaceSupport::Unavailable(_)
            ) || (spec.slash_aliases.is_empty() && spec.support(Surface::Cli).is_supported()))
    }) {
        let _ = writeln!(
            out,
            "  {:<30} {} [{}]",
            command_usage(spec),
            spec.summary,
            effect_label(spec)
        );
    }
}

fn markdown_escape(value: &str) -> String {
    value.replace('|', "\\|")
}

/// Generate the documentation block for one interactive surface. The checked
/// in Markdown is guarded by an exact-conformance test.
pub fn markdown_command_reference(surface: Surface) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{COMMAND_REFERENCE_BEGIN}");
    out.push_str("| Slash command | Purpose | State/effect |\n");
    out.push_str("|---|---|---|\n");
    for spec in COMMANDS.iter().filter(|spec| {
        spec.is_active()
            && !spec.slash_aliases.is_empty()
            && !matches!(spec.support(surface), SurfaceSupport::Unavailable(_))
    }) {
        let _ = writeln!(
            out,
            "| `{}` | {} | {} |",
            markdown_escape(&slash_usage(spec)),
            markdown_escape(spec.summary),
            effect_label(spec)
        );
    }
    let cli_only = COMMANDS
        .iter()
        .filter(|spec| {
            spec.is_active()
                && spec.slash_aliases.is_empty()
                && spec.support(Surface::Cli).is_supported()
        })
        .collect::<Vec<_>>();
    if !cli_only.is_empty() {
        out.push_str("\nTop-level CLI-only commands (no slash or hook form):\n\n");
        out.push_str("| CLI command | Purpose | State/effect |\n");
        out.push_str("|---|---|---|\n");
        for spec in cli_only {
            let _ = writeln!(
                out,
                "| `{}` | {} | {} |",
                markdown_escape(&cli_usage(spec)),
                markdown_escape(spec.summary),
                effect_label(spec)
            );
        }
    }
    out.push_str("\nRemoved names remain reserved for one compatibility window:\n\n");
    out.push_str("| Removed slash command | Local guidance |\n");
    out.push_str("|---|---|\n");
    for spec in COMMANDS.iter().filter(|spec| {
        matches!(spec.lifecycle, Lifecycle::Tombstone { .. })
            && !matches!(spec.support(surface), SurfaceSupport::Unavailable(_))
    }) {
        let Lifecycle::Tombstone { guidance, .. } = spec.lifecycle else {
            unreachable!()
        };
        let _ = writeln!(
            out,
            "| `{}` | {} |",
            markdown_escape(&slash_usage(spec)),
            markdown_escape(guidance)
        );
    }
    let _ = writeln!(out, "{COMMAND_REFERENCE_END}");
    out
}

/// Condensed product truth always safe to inject into suggest/yolo system prompts.
pub fn product_brief() -> &'static str {
    "\
AIShe product rules (authoritative — prefer these over guessing):\n\
- /connection switches account; /model changes model on the *current* account only.\n\
- Add accounts with `aishe setup`, `aishe settings`, or `aishe connection add …`.\n\
- Codex OAuth: `aishe auth login openai --profile work` then pick via /connection.\n\
- Grok OAuth: `aishe auth login xai --profile work`.\n\
- API keys: `aishe auth set openai` (or xai/anthropic).\n\
- Labels: \"Codex - API\" vs \"Codex - OAuth · profile\"; \"Grok - API\" vs \"Grok - OAuth · profile\".\n\
- OAuth models are listed by managed OpenCode; do not invent dashboard-only steps.\n\
- For product how-to questions, answer with the exact commands above (type answer, not shell echo).\n\
- Full skill: use_skill name=aishe-product when available (yolo)."
}

/// Build the product-reference block injected into **suggest** user messages.
///
/// Uses the compact [`product_brief`] only — never the full skill body — so
/// product how-to turns stay token-cheap. Yolo keeps progressive disclosure via
/// `use_skill name=aishe-product` ([`product_skill_body`]).
pub fn suggest_product_reference(answer_hint: &str) -> String {
    format!(
        "\n\n--- AIShe product reference (authoritative) ---\n\
         {}\n\
         --- end product reference ---\n\
         {answer_hint}",
        product_brief()
    )
}

/// Full skill body for progressive disclosure (yolo `use_skill`).
pub fn product_skill_body() -> &'static str {
    r#"# AIShe product help (authoritative)

Answer user questions about **using AIShe itself** with the recipes below.
Prefer exact commands. Do not invent OpenAI/xAI website-only steps.

## Mental model
- **Connection** = account (provider + endpoint + auth + default model).
- **Model** = which model on the *active* connection.
- `/connection` switches accounts; `/model` does not change login.
- Statusline/status show brands: `Codex - API`, `Codex - OAuth · work`, `Grok - API`, `Grok - OAuth · work`.

## Add a new account
Interactive:
- `aishe setup` — guided; ChatGPT/Codex OAuth and Grok OAuth are top shortcuts.
- `aishe settings` — Provider section; review/apply at the end.

CLI examples:
```sh
# Codex subscription (ChatGPT Plus/Pro via OpenCode OAuth)
aishe connection add codex-work --provider openai --auth oauth --profile work
aishe auth login openai --profile work
aishe connection use codex-work --default

# Grok subscription
aishe connection add grok-work --provider xai --auth oauth --profile work
aishe auth login xai --profile work

# API key OpenAI / xAI
aishe connection add openai-api --provider openai --auth api-key
aishe auth set openai
```

## Switch account vs model
```sh
/connection          # pick existing account (Enter: this shell; then y: new-shell default)
/model               # models for *current* account only
aishe models         # list without picker
aishe model gpt-5.5  # set model on current connection
```

OAuth model lists come from **managed OpenCode** (`/config/providers`), not public `GET /v1/models`.

## Sign-in
```sh
aishe auth login openai --profile work    # Codex OAuth
aishe auth login xai --profile work       # Grok OAuth
aishe auth set anthropic                  # API key hidden prompt
aishe auth status
aishe auth status openai --profile work
```

A successful `auth login` (without `--connection`) creates a selectable
connection when none exists for that provider/profile (e.g. `xai-work` /
`Grok - OAuth · work`) and may switch to it. Then `/connection` lists it.

## Session controls
- `/status`, `/usage`, `/log`, `/reset`, `/reasoning`
- Shift-Tab: suggest → auto → yolo
- Ctrl-O: focus ↔ detailed agent output
- `/settings`, `aishe doctor`, `aishe tour`

## When something fails
```sh
aishe doctor --live
aishe backend status
aishe backend logs
aishe connection show
```

## Help surfaces
- In shell: `/help`, `/help accounts`, `/help models`, `/help session`, `/help config`
- CLI: `aishe --help`, `aishe connection --help`, `aishe auth --help`
"#
}

/// Built-in model-invoked skill (always registered unless the user overrides the name).
pub fn product_skill() -> Skill {
    Skill {
        name: "aishe-product".into(),
        description:
            "How to use AIShe: accounts, OAuth/API login, /connection vs /model, setup, doctor"
                .into(),
        body: product_skill_body().into(),
        source: None,
    }
}

/// Heuristic: user is asking how to operate AIShe, not a general shell question.
pub fn looks_like_product_question(input: &str) -> bool {
    let t = input.to_ascii_lowercase();
    let mentions_product = t.contains("aishe")
        || t.contains("/connection")
        || t.contains("/model")
        || t.contains("/help")
        || t.contains("/status")
        || t.contains("/settings")
        || t.contains("codex")
        || t.contains("supergrok");
    let how_to = t.contains("how do i")
        || t.contains("how do you")
        || t.contains("how to")
        || t.contains("where do i")
        || t.contains("what's the command")
        || t.contains("what is the command")
        || t.contains("help me")
        || t.contains("can i ");
    let topics = t.contains("connection")
        || t.contains("oauth")
        || t.contains("login")
        || t.contains("log in")
        || t.contains("sign in")
        || t.contains("api key")
        || t.contains("add account")
        || t.contains("new account")
        || t.contains("switch model")
        || t.contains("change model")
        || t.contains("setup")
        || t.contains("statusline")
        || t.contains("status line");
    // "how to switch model…" is product help even without the word "aishe".
    (mentions_product && (how_to || topics))
        || (how_to && topics)
        || (t.starts_with("how ") && topics)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_skill_is_loadable() {
        let skill = product_skill();
        assert_eq!(skill.name, "aishe-product");
        assert!(skill.body.contains("/connection"));
        assert!(skill.body.contains("aishe auth login openai"));
        assert!(!skill.needs_trust(false));
    }

    #[test]
    fn detects_product_questions() {
        assert!(looks_like_product_question(
            "how do I add a new Codex OAuth account in aishe?"
        ));
        assert!(looks_like_product_question(
            "how to switch model without changing connection"
        ));
        assert!(!looks_like_product_question("list files in /tmp"));
        assert!(!looks_like_product_question(
            "what is the capital of france"
        ));
    }

    #[test]
    fn suggest_reference_uses_brief_not_full_skill() {
        let block = suggest_product_reference("Answer in prose with exact commands (no CMD:).");
        assert!(block.contains("AIShe product reference"));
        assert!(block.contains("/connection"));
        assert!(block.contains("aishe auth login openai"));
        assert!(
            !block.contains("# AIShe product help"),
            "suggest must not inject the full skill markdown heading"
        );
        assert!(
            !block.contains("## Mental model"),
            "suggest must not inject full skill sections"
        );
        // Brief is much smaller than the yolo skill body.
        assert!(product_brief().len() < product_skill_body().len() / 2);
        // The real skill still carries the long recipes for yolo.
        assert!(product_skill_body().contains("# AIShe product help"));
        assert!(product_skill_body().contains("## Mental model"));
    }

    #[test]
    fn overview_and_topics_cover_the_active_registry_exactly() {
        crate::command_surface::validate_registry().unwrap();
        let overview = render_help(None);
        for spec in COMMANDS.iter().filter(|spec| spec.is_active()) {
            for alias in spec.slash_aliases {
                assert!(
                    overview.contains(&format!("/{alias}")),
                    "overview omitted /{alias} ({})",
                    spec.id
                );
            }
            let topic = render_help(Some(spec.help_topic));
            assert!(
                topic.contains(&command_usage(spec)),
                "topic {} omitted exact usage for {}",
                spec.help_topic,
                spec.id
            );
        }
    }

    #[test]
    fn migration_help_contains_every_tombstone_and_exact_guidance() {
        let migration = render_help(Some("migration"));
        assert!(migration.contains("never sent to a model"));
        for spec in COMMANDS
            .iter()
            .filter(|spec| matches!(spec.lifecycle, Lifecycle::Tombstone { .. }))
        {
            let Lifecycle::Tombstone { guidance, .. } = spec.lifecycle else {
                unreachable!()
            };
            assert!(migration.contains(&slash_names(spec)));
            assert!(migration.contains(guidance));
        }
    }

    #[test]
    fn commands_markdown_matches_the_generated_registry_block() {
        let docs = include_str!("../docs/commands.md");
        let start = docs
            .find(COMMAND_REFERENCE_BEGIN)
            .expect("commands docs have generated block start");
        let from_start = &docs[start..];
        let end = from_start
            .find(COMMAND_REFERENCE_END)
            .expect("commands docs have generated block end")
            + COMMAND_REFERENCE_END.len();
        let checked_in = &from_start[..end];
        let generated = markdown_command_reference(Surface::ZshHook);
        assert_eq!(checked_in.trim(), generated.trim());
    }

    #[test]
    fn routing_help_documents_explicit_and_diagnostic_routes() {
        let routing = render_help(Some("routing"));
        assert!(routing.contains("! line"));
        assert!(routing.contains("? question"));
        assert!(routing.contains("Ctrl-X ?"));
        assert!(routing.contains("route as text"));
        assert!(routing.contains("aishe route -- 'git status'"));
        assert!(routing.contains("without executing"));
    }

    #[test]
    fn session_help_explains_native_suggest_staging_and_cancel() {
        let session = render_help(Some("session"));
        assert!(session.contains("first Enter stages; edit; second Enter runs"));
        assert!(session.contains("cancel a staged suggestion without running it"));
        for key in [
            "Alt-Enter",
            "Ctrl-X ?",
            "Ctrl-X Ctrl-F",
            "Ctrl-X Ctrl-R",
            "Page Up/Down",
            "Home/End",
            "Esc/Ctrl-C",
        ] {
            assert!(session.contains(key), "session help omitted {key}");
        }
        assert!(session.contains("AISHE_NL_KEY"));
        assert!(session.contains("aishe doctor"));
    }

    #[test]
    fn terminal_mark_is_half_block_glasses() {
        let mark = crate::promptui::ASCII_LOGO;
        assert!(mark.contains('█') || mark.contains('▄') || mark.contains('▀'));
        assert!(mark.contains("AIShe"));
        assert!(mark.contains("AI Shell"));
        assert!(!mark.contains("| o   o |"), "old face logo must be gone");
    }
}
