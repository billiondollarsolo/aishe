//! LLM interaction modes: `suggest` (confirm-before-run) and `yolo` (agentic
//! tool loop), plus the helpers they share.

pub mod suggest;
pub mod yolo;

use std::io::{IsTerminal, Write};

use crossterm::style::Stylize;
use serde_json::json;

use crate::config::Config;
use crate::providers::{Provider, ToolDef};
use crate::safety::{self, Risk};
use crate::usage;

/// `true` if the session has reached the configured `budget_usd`. When it has,
/// prints a one-line notice (so the caller can simply stop).
pub fn budget_reached(provider: &dyn Provider, config: &Config) -> bool {
    let snap = provider.meter().snapshot();
    if usage::over_budget(
        snap,
        config.active_model(),
        &config.pricing,
        config.aishe.budget_usd,
    ) {
        eprintln!(
            "  {}",
            format!(
                "budget reached (~${:.2} ≥ ${:.2}); raise `budget_usd` to continue",
                usage::price_for(config.active_model(), &config.pricing)
                    .map(|p| usage::cost(snap, p))
                    .unwrap_or(0.0),
                config.aishe.budget_usd,
            )
            .red()
        );
        return true;
    }
    false
}

/// Print a dim per-session token/cost line, when `show_usage` is on and at least
/// one request has been made.
pub fn report_usage(provider: &dyn Provider, config: &Config) {
    if !config.aishe.show_usage {
        return;
    }
    let snap = provider.meter().snapshot();
    if snap.is_empty() {
        return;
    }
    eprintln!(
        "  {}",
        usage::summary(snap, config.active_model(), &config.pricing).dim()
    );
}

/// System prompt for suggest mode (PRD Appendix A.1).
pub fn suggest_system_prompt(shell: &str, os: &str) -> String {
    format!(
        "You are aishe, an expert command-line assistant embedded in the user's shell.\n\
The user typed natural language instead of a command. Using the provided\n\
environment context, respond with ONLY a JSON object, no markdown fences:\n\n\
{{\"type\": \"command\", \"command\": \"<single shell line for {shell} on {os}>\",\n\
 \"explanation\": \"<one short sentence>\"}}\n\n\
or, if the input is an informational question not best answered by running\n\
a command:\n\n\
{{\"type\": \"answer\", \"command\": null, \"explanation\": \"<concise answer, markdown ok>\"}}\n\n\
Rules: prefer safe, idiomatic, non-interactive flags; never invent paths not\n\
implied by the context; one line only (use && or ; if needed); no sudo unless\n\
clearly required.\n\
\n\
Classify by the user's intent, not by whether an answer could be wrapped in a\n\
shell command. Informational questions (for example who/what/why/when,\n\
define/explain, or what a command does) MUST use type \"answer\". Do not turn a\n\
fact into echo/printf/python, and do not substitute man for an explanation.\n\
Use type \"command\" only when the user wants to inspect or change the current\n\
machine, filesystem, processes, network, or other local state.\n\n\
{}\n",
        crate::product_help::product_brief()
    )
}

/// System prompt for streaming suggest mode. Uses a line-oriented sentinel
/// protocol (instead of a JSON object) so prose answers can be streamed to the
/// terminal token-by-token while command suggestions stay cleanly detectable.
pub fn suggest_stream_system_prompt(shell: &str, os: &str) -> String {
    format!(
        "You are aishe, an expert command-line assistant embedded in the user's shell.\n\
The user typed natural language instead of a command. Using the provided\n\
environment context, respond in ONE of two ways:\n\n\
1. If the request is best satisfied by running a shell command, reply with\n\
   exactly this and nothing else:\n\
   CMD: <single shell line for {shell} on {os}>\n\
   WHY: <one short sentence>\n\n\
2. Otherwise (an informational question), answer directly in plain prose\n\
   (markdown allowed). Do NOT use the CMD: prefix in that case.\n\n\
Rules: prefer safe, idiomatic, non-interactive flags; never invent paths not\n\
implied by the context; one command line only (use && or ; if needed); no sudo\n\
unless clearly required.\n\
\n\
Classify by the user's intent, not by whether an answer could be wrapped in a\n\
shell command. Informational questions (for example who/what/why/when,\n\
define/explain, or what a command does) must be answered directly in prose.\n\
Never turn a fact into echo/printf/python, and do not substitute man for an\n\
explanation. Use CMD only when the user wants to inspect or change the current\n\
machine, filesystem, processes, network, or other local state.\n\n\
{}\n",
        crate::product_help::product_brief()
    )
}

/// System prompt for yolo mode (PRD Appendix A.2).
pub const YOLO_SYSTEM_PROMPT: &str =
    "You are aishe in autonomous mode. Accomplish the user's request by calling the \
run_command tool. Inspect output, adapt, and iterate. Environment context is \
provided. Rules: act rather than ask, unless the request is destructive or \
genuinely ambiguous; commands run with stdin closed, so always use \
non-interactive flags (-y, --no-input); when finished, reply with a brief \
plain-text summary of what you did and the result. Keep total commands minimal.";

/// Tool that loads a named skill's full instructions into context (progressive
/// disclosure). Only offered to yolo when skills are available.
pub fn use_skill_tool() -> ToolDef {
    ToolDef {
        name: "use_skill".to_string(),
        description: "Load the full instructions for a named skill before acting, \
            when the user's request matches one of the available skills."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "the skill name"}
            },
            "required": ["name"]
        }),
    }
}

/// The single tool exposed to yolo mode.
pub fn run_command_tool() -> ToolDef {
    ToolDef {
        name: "run_command".to_string(),
        description: "Run a shell command non-interactively and get its output.".to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "command": {"type": "string"},
                "reason": {"type": "string", "description": "one short phrase"}
            },
            "required": ["command", "reason"]
        }),
    }
}

/// Render markdown text to the terminal. Prose is rendered with termimad; fenced
/// code blocks are syntax-highlighted (when the `highlight` feature is on).
pub fn render_markdown(text: &str) {
    let text = crate::commands::display_safe_multiline(text);
    let styled = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none()
        && std::env::var("TERM").ok().as_deref() != Some("dumb");
    if !styled {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
        let _ = std::io::stdout().flush();
        return;
    }

    #[cfg(not(feature = "highlight"))]
    {
        markdown_skin().print_text(&text);
    }
    #[cfg(feature = "highlight")]
    {
        render_markdown_highlighted(&text);
    }
}

fn markdown_skin() -> termimad::MadSkin {
    use termimad::crossterm::style::{Attribute, Color};

    let mut skin = termimad::MadSkin::default();
    for (depth, header) in skin.headers.iter_mut().enumerate() {
        header.align = termimad::Alignment::Left;
        header.compound_style.remove_attr(Attribute::Underlined);
        header.set_fg(if depth == 0 {
            Color::Cyan
        } else {
            Color::DarkCyan
        });
    }
    skin.bullet.set_fg(Color::Cyan);
    skin.quote_mark.set_fg(Color::DarkCyan);
    skin.horizontal_rule.set_fg(Color::DarkGrey);
    skin
}

/// Split `text` into prose segments (rendered by termimad) and fenced code
/// blocks (syntax-highlighted), preserving order. A trailing unterminated fence
/// is still rendered as code.
#[cfg(feature = "highlight")]
fn render_markdown_highlighted(text: &str) {
    let skin = markdown_skin();
    let mut prose = String::new();
    let mut code = String::new();
    let mut lang = String::new();
    let mut in_code = false;

    for line in text.split('\n') {
        let head = line.trim_start();
        if in_code {
            if head.starts_with("```") {
                highlight::print_code_block(&code, &lang);
                code.clear();
                lang.clear();
                in_code = false;
            } else {
                code.push_str(line);
                code.push('\n');
            }
        } else if head.starts_with("```") {
            if !prose.trim().is_empty() {
                skin.print_text(&prose);
            }
            prose.clear();
            lang = head.trim_start_matches('`').trim().to_string();
            in_code = true;
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    if in_code && !code.is_empty() {
        highlight::print_code_block(&code, &lang);
    }
    if !prose.trim().is_empty() {
        skin.print_text(&prose);
    }
}

/// Syntax highlighting for fenced code blocks via syntect.
#[cfg(feature = "highlight")]
mod highlight {
    use std::io::Write;
    use std::sync::OnceLock;

    use syntect::easy::HighlightLines;
    use syntect::highlighting::{Theme, ThemeSet};
    use syntect::parsing::SyntaxSet;
    use syntect::util::{as_24_bit_terminal_escaped, LinesWithEndings};

    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEME: OnceLock<Theme> = OnceLock::new();

    fn syntaxes() -> &'static SyntaxSet {
        SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines)
    }

    fn theme() -> &'static Theme {
        THEME.get_or_init(|| {
            let mut ts = ThemeSet::load_defaults();
            ts.themes
                .remove("base16-ocean.dark")
                .unwrap_or_else(|| ThemeSet::load_defaults().themes["InspiredGitHub"].clone())
        })
    }

    /// Print one fenced code block, syntax-highlighted by `lang` (falling back to
    /// plain text for unknown or empty languages). A subdued label and rule make
    /// the block distinct without prefixing copied code with border characters.
    pub fn print_code_block(code: &str, lang: &str) {
        print!("{}", render_code_block(code, lang));
        let _ = std::io::stdout().flush();
    }

    pub(super) fn render_code_block(code: &str, lang: &str) -> String {
        let ss = syntaxes();
        let syntax = (!lang.is_empty())
            .then(|| {
                ss.find_syntax_by_token(lang)
                    .or_else(|| ss.find_syntax_by_extension(lang))
            })
            .flatten()
            .unwrap_or_else(|| ss.find_syntax_plain_text());

        let mut h = HighlightLines::new(syntax, theme());
        let mut out = String::new();
        let label = if lang.is_empty() { "code" } else { lang };
        out.push_str(&format!("\x1b[2;36m  {label}\x1b[0m\n"));
        for line in LinesWithEndings::from(code) {
            match h.highlight_line(line, ss) {
                Ok(ranges) => {
                    out.push_str(&as_24_bit_terminal_escaped(&ranges[..], false));
                }
                Err(_) => {
                    out.push_str(line);
                }
            }
        }
        out.push_str("\x1b[0m");
        if !code.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("\x1b[2;36m  ────────────────────\x1b[0m\n");
        out
    }
}

/// After a final answer has been streamed to the screen as raw text, re-render it
/// as markdown in place so code fences, lists, and emphasis look right. We move
/// the cursor back up over the streamed block (a relative move, robust to the
/// terminal having scrolled) and clear it before rendering. If the streamed block
/// was taller than the screen, the top scrolled off and cannot be erased, so we
/// just terminate the line and keep the raw text.
pub fn rerender_streamed_markdown(text: &str) {
    use crossterm::{cursor, terminal, ExecutableCommand};
    let text = crate::commands::display_safe_multiline(text);
    // In a pipe/file there is no cursor to move; the raw markdown already streamed
    // out, so just end the line and leave it.
    if !std::io::stdout().is_terminal() {
        println!();
        return;
    }
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let used = streamed_rows(&text, cols);
    if used + 1 < rows as usize {
        let mut out = std::io::stdout();
        let _ = out.execute(cursor::MoveToColumn(0));
        if used > 1 {
            let _ = out.execute(cursor::MoveUp((used - 1) as u16));
        }
        let _ = out.execute(terminal::Clear(terminal::ClearType::FromCursorDown));
        render_markdown(&text);
    } else {
        println!();
    }
}

#[cfg(test)]
mod markdown_tests {
    #[cfg(feature = "highlight")]
    #[test]
    fn fenced_code_highlighting_is_visually_delimited_and_preserves_code() {
        let rendered = super::highlight::render_code_block("echo \"$HOME\"\n", "bash");
        assert!(rendered.contains("bash"));
        assert!(rendered.contains("echo"));
        assert!(rendered.contains("HOME"));
        assert!(rendered.contains("\x1b["));
        assert!(rendered.contains("────────────────────"));
    }
}

/// Estimate how many terminal rows a raw streamed string occupied, accounting for
/// line wrapping at `cols`. Used to reposition the cursor for re-rendering.
fn streamed_rows(text: &str, cols: u16) -> usize {
    let cols = cols.max(1) as usize;
    text.split('\n')
        .map(|line| {
            let w = line.chars().count();
            if w == 0 {
                1
            } else {
                w.div_ceil(cols)
            }
        })
        .sum()
}

/// Strip code fences and slice from the first `{` to the last `}` so we can
/// parse JSON even when the model wraps it in prose or fences.
pub fn extract_json(raw: &str) -> Option<String> {
    let s = raw.trim();
    // Remove surrounding ``` fences if present.
    let s = if let Some(rest) = s.strip_prefix("```") {
        // drop an optional language tag on the first line
        let rest = rest.split_once('\n').map(|x| x.1).unwrap_or(rest);
        rest.trim_end_matches("```").trim()
    } else {
        s
    };
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end >= start {
        Some(s[start..=end].to_string())
    } else {
        None
    }
}

/// Result of the safety confirmation step.
pub enum GateOutcome {
    Proceed,
    Declined,
}

/// Apply the safety gate to a command. Safe commands pass silently. Dangerous
/// commands print a red panel and require the user to type the literal word
/// `yes`. A command the gate could not *resolve* (an unparseable head — see
/// [`Risk::Unknown`]) fails closed too, but with a milder yellow panel and a
/// plain `y/N`: it is a "I can't tell what this runs" warning, not an accusation,
/// and making it as loud as a real `rm -rf /` would train users to confirm
/// reflexively. Returns whether to proceed.
pub fn safety_gate(command: &str) -> GateOutcome {
    match safety::assess(command) {
        Risk::Safe => GateOutcome::Proceed,
        Risk::Dangerous(reason) => confirm_dangerous(command, reason),
        Risk::Unknown(reason) => confirm_unresolved(command, reason),
    }
}

fn confirm_dangerous(command: &str, reason: &str) -> GateOutcome {
    println!();
    println!(
        "{}",
        "  ┌─ DANGEROUS COMMAND ─────────────────".red().bold()
    );
    println!("  {} {}", "│".red(), command.white().bold());
    println!("  {} reason: {}", "│".red(), reason.yellow());
    println!(
        "{}",
        "  └─────────────────────────────────────".red().bold()
    );
    print!(
        "  Type {} to proceed (anything else cancels): ",
        "yes".red().bold()
    );
    std::io::stdout().flush().ok();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return GateOutcome::Declined;
    }
    if line.trim() == "yes" {
        GateOutcome::Proceed
    } else {
        println!("  {}", "cancelled".dim());
        GateOutcome::Declined
    }
}

fn confirm_unresolved(command: &str, reason: &str) -> GateOutcome {
    println!();
    println!(
        "{}",
        "  ┌─ COULD NOT VERIFY ──────────────────".yellow().bold()
    );
    println!("  {} {}", "│".yellow(), command.white().bold());
    println!("  {} {}", "│".yellow(), reason.dim());
    println!(
        "  {} the safety gate could not tell what this runs",
        "│".yellow()
    );
    println!(
        "{}",
        "  └─────────────────────────────────────".yellow().bold()
    );
    print!("  Run it anyway? [y/N]: ");
    std::io::stdout().flush().ok();

    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return GateOutcome::Declined;
    }
    if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        GateOutcome::Proceed
    } else {
        println!("  {}", "cancelled".dim());
        GateOutcome::Declined
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_plain_json() {
        let j = extract_json(r#"{"type":"answer","command":null,"explanation":"hi"}"#).unwrap();
        assert!(j.starts_with('{') && j.ends_with('}'));
    }

    #[test]
    fn extract_fenced_json() {
        let raw =
            "```json\n{\"type\":\"command\",\"command\":\"ls\",\"explanation\":\"list\"}\n```";
        let j = extract_json(raw).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["command"], "ls");
    }

    #[test]
    fn extract_json_with_prose() {
        let raw = "Sure! Here is the command:\n{\"type\":\"command\",\"command\":\"pwd\",\"explanation\":\"x\"} Hope that helps";
        let j = extract_json(raw).unwrap();
        let v: serde_json::Value = serde_json::from_str(&j).unwrap();
        assert_eq!(v["command"], "pwd");
    }

    #[test]
    fn extract_json_garbage_returns_none() {
        assert!(extract_json("no json here at all").is_none());
    }

    #[test]
    fn suggest_prompts_keep_informational_answers_out_of_shell_commands() {
        let structured = suggest_system_prompt("zsh", "linux");
        assert!(structured.contains("Classify by the user's intent"));
        assert!(structured.contains("MUST use type \"answer\""));
        assert!(structured.contains("Do not turn a\nfact into echo/printf/python"));

        let streaming = suggest_stream_system_prompt("zsh", "linux");
        assert!(streaming.contains("answered directly in prose"));
        assert!(streaming.contains("Never turn a fact into echo/printf/python"));
    }
}
