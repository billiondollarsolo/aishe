//! Task-first product help for `/help` and the built-in `aishe-product` skill.
//!
//! Keep this module the single source of truth for “how do I use Aishe?” so the
//! shell help surface and the model skill cannot drift apart.

use crate::skills::Skill;

/// Print task-oriented help. `topic` is optional (`accounts`, `models`, …).
pub fn print_help(topic: Option<&str>) {
    match topic.map(|t| t.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("help") | Some("all") => print_overview(),
        Some("accounts" | "account" | "connection" | "connections" | "auth" | "login") => {
            print_accounts()
        }
        Some("models" | "model") => print_models(),
        Some("session" | "status" | "keys") => print_session(),
        Some("config" | "setup" | "settings" | "doctor") => print_config(),
        Some(other) => {
            println!("unknown help topic '{other}'");
            println!("topics: accounts · models · session · config  (or bare /help)");
        }
    }
}

fn print_overview() {
    println!("aishe help — what do you want to do?\n");
    println!("Accounts & models");
    println!("  /connection              switch account for this shell");
    println!("  /model                   change model on the *current* account");
    println!("  Add a new account:       aishe setup");
    println!("                           aishe settings          (provider section)");
    println!("                           aishe connection add ID --provider openai \\");
    println!("                             --auth oauth --profile work");
    println!("  Sign in (Codex OAuth):   aishe auth login openai --profile work");
    println!("  Sign in (Grok OAuth):    aishe auth login xai --profile work");
    println!("  Sign in (API key):       aishe auth set openai");
    println!("  More:                    /help accounts\n");
    println!("Session");
    println!("  /status  /usage  /reset  /reasoning");
    println!("  Shift-Tab                suggest → auto → yolo");
    println!("  Ctrl-O                   focus ↔ detailed agent output");
    println!("  More:                    /help session\n");
    println!("Config & health");
    println!("  /settings                interactive settings hub");
    println!("  aishe doctor             diagnose environment");
    println!("  aishe tour               guided first session");
    println!("  More:                    /help config\n");
    println!("Ask Aishe");
    println!("  Type naturally, e.g.  how do I add a Codex OAuth account?");
    println!("  (product answers use the built-in aishe-product skill)\n");
    println!("Slash index: /commands   ·   Full CLI: aishe --help");
}

fn print_accounts() {
    println!("Accounts & authentication\n");
    println!("Labels");
    println!("  Codex - API              OpenAI API key");
    println!("  Codex - OAuth · work     ChatGPT/Codex subscription (profile work)");
    println!("  Grok - API               xAI API key");
    println!("  Grok - OAuth · work      SuperGrok subscription\n");
    println!("Switch account (this shell)");
    println!("  /connection              interactive picker");
    println!("  /connection ID           select by id or label");
    println!("  d in the picker          save as durable default\n");
    println!("Add a new account");
    println!("  aishe setup              guided setup (includes OAuth shortcuts)");
    println!("  aishe settings           Provider section → new connection");
    println!("  aishe connection add my-codex --provider openai \\");
    println!("      --auth oauth --profile work --label \"Codex - OAuth · work\"");
    println!("  aishe connection add my-api --provider openai --auth api-key\n");
    println!("Sign in");
    println!("  aishe auth login openai --profile work     # Codex OAuth");
    println!("  aishe auth login xai --profile work        # Grok OAuth");
    println!("  aishe auth set openai                      # API key (hidden prompt)");
    println!("  aishe auth status                          # what is selected");
    println!("  After login, a connection is created if missing so /connection lists it.\n");
    println!("Inspect");
    println!("  aishe connection list|show");
    println!("  /auth                    active connection auth state");
    println!("  /status                  connection + model + health summary");
}

fn print_models() {
    println!("Models\n");
    println!("  /model                   list models for the *active* connection");
    println!("  /model NAME              set model for this shell");
    println!("  d in the picker          save model as default on this connection\n");
    println!("  Codex/Grok OAuth: models come from managed OpenCode");
    println!("  (subscription catalog), not public GET /v1/models.");
    println!("  API-key connections: endpoint GET /v1/models.\n");
    println!("  List without picker:     aishe models");
    println!("  Scripting:               aishe model gpt-5.5");
    println!("                           aishe model --connection ID NAME");
}

fn print_session() {
    println!("Session controls\n");
    println!("  /status                  connection, model, mode, spend, audit");
    println!("  /usage                   live token/cost totals for this shell");
    println!("  /log                     recent audit events");
    println!("  /reset                   fresh conversation (same account/model)");
    println!("  /reasoning [LEVEL]       auto|none|low|medium|high|xhigh|max");
    println!("  /details                 transcript density for agent turns");
    println!("  Shift-Tab                cycle suggest / auto / yolo");
    println!("  Ctrl-O                   focus ↔ detailed agent output");
}

fn print_config() {
    println!("Config & health\n");
    println!("  /settings                interactive section hub (draft until apply)");
    println!("  aishe setup              resumable first-run / reconfigure");
    println!("  aishe setup --verify     check current config only");
    println!("  aishe doctor [--live]    environment + backend + provider checks");
    println!("  aishe tour               guided first-session tour");
    println!("  aishe config             print active configuration");
    println!("  aishe backend status     managed OpenCode runtime");
}

/// Condensed product truth always safe to inject into suggest/yolo system prompts.
pub fn product_brief() -> &'static str {
    "\
Aishe product rules (authoritative — prefer these over guessing):\n\
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

/// Full skill body for progressive disclosure (yolo `use_skill`).
pub fn product_skill_body() -> &'static str {
    r#"# Aishe product help (authoritative)

Answer user questions about **using Aishe itself** with the recipes below.
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
/connection          # pick existing account (this shell; d = default)
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
            "How to use Aishe: accounts, OAuth/API login, /connection vs /model, setup, doctor"
                .into(),
        body: product_skill_body().into(),
        source: None,
    }
}

/// Heuristic: user is asking how to operate Aishe, not a general shell question.
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
}
