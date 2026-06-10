//! LLM interaction modes: `suggest` (confirm-before-run) and `yolo` (agentic
//! tool loop), plus the helpers they share.

pub mod suggest;
pub mod yolo;

use std::io::Write;

use crossterm::style::Stylize;
use serde_json::json;

use crate::providers::ToolDef;
use crate::safety::{self, Risk};

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
clearly required."
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

/// Render markdown text to the terminal via termimad.
pub fn render_markdown(text: &str) {
    let skin = termimad::MadSkin::default();
    skin.print_text(text);
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
/// `yes`. Returns whether to proceed.
pub fn safety_gate(command: &str) -> GateOutcome {
    match safety::assess(command) {
        Risk::Safe => GateOutcome::Proceed,
        Risk::Dangerous(reason) => confirm_dangerous(command, reason),
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
}
