//! Suggest mode: the LLM proposes a command (confirm before running) or answers
//! a question. Default mode.

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::style::Stylize;
use crossterm::terminal;
use serde::Deserialize;

use super::{extract_json, render_markdown, safety_gate, GateOutcome};
use crate::config::Config;
use crate::context;
use crate::executor::Executor;
use crate::providers::{Msg, Provider, ResponseFormat};
use crate::session::Session;

/// The model's structured response.
#[derive(Debug, Clone, PartialEq)]
pub enum Suggestion {
    Command {
        command: String,
        explanation: String,
    },
    Answer {
        explanation: String,
    },
}

#[derive(Deserialize)]
struct RawSuggestion {
    #[serde(rename = "type")]
    kind: String,
    command: Option<String>,
    #[serde(default)]
    explanation: String,
}

/// Parse a raw model response into a `Suggestion`, defensively. On unparseable
/// input, fall back to treating the whole text as an answer.
pub fn parse_suggestion(raw: &str) -> Suggestion {
    if let Some(json) = extract_json(raw) {
        if let Ok(parsed) = serde_json::from_str::<RawSuggestion>(&json) {
            match parsed.kind.as_str() {
                "command" => {
                    if let Some(cmd) = parsed.command.filter(|c| !c.trim().is_empty()) {
                        return Suggestion::Command {
                            command: cmd.trim().to_string(),
                            explanation: parsed.explanation,
                        };
                    }
                    // command type but no command → treat as answer.
                    return Suggestion::Answer {
                        explanation: parsed.explanation,
                    };
                }
                _ => {
                    return Suggestion::Answer {
                        explanation: parsed.explanation,
                    };
                }
            }
        }
    }
    // Garbage / no JSON → render the raw text as an answer.
    Suggestion::Answer {
        explanation: raw.trim().to_string(),
    }
}

/// Run one suggest-mode interaction.
///
/// `scriptable` (the `-c` flag) prints a suggested command to stdout and exits
/// without running it; answers are still printed.
///
/// `auto` (auto mode) runs commands the safety gate classifies as Safe without
/// a confirmation keypress; Dangerous commands still require typing `yes`.
pub fn run(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    scriptable: bool,
    auto: bool,
    session: &mut Session,
) -> Result<()> {
    if super::budget_reached(provider, config) {
        return Ok(());
    }
    // Interactive streaming: render answers token-by-token. Not used for the
    // scriptable (`-c`) path, whose stdout is consumed by the shell hook.
    let result = if config.aishe.stream && !scriptable {
        run_stream(input, provider, executor, config, auto, session)
    } else {
        match request(input, provider, executor, config, session.history()) {
            Ok(suggestion) => {
                record_turn(session, input, &suggestion);
                handle_suggestion(suggestion, executor, scriptable, auto)
            }
            Err(e) => Err(e),
        }
    };
    super::report_usage(provider, config);
    result
}

/// Record a completed suggest turn into session memory: the user's request and
/// the assistant's reply (the proposed command + explanation, or the answer).
fn record_turn(session: &mut Session, input: &str, suggestion: &Suggestion) {
    let reply = match suggestion {
        Suggestion::Command {
            command,
            explanation,
        } => {
            if explanation.is_empty() {
                command.clone()
            } else {
                format!("{command}\n{explanation}")
            }
        }
        Suggestion::Answer { explanation } => explanation.clone(),
    };
    if reply.is_empty() {
        return;
    }
    session.record_user(input);
    session.record_assistant(&reply);
}

/// Streaming variant of [`run`]: uses the sentinel-protocol system prompt and
/// streams a prose answer to the terminal as it arrives, or — once the model
/// commits to a `CMD:` line — falls back to the normal confirm/auto flow.
fn run_stream(
    input: &str,
    provider: &dyn Provider,
    executor: &mut Executor,
    config: &Config,
    auto: bool,
    session: &mut Session,
) -> Result<()> {
    let _ = config; // streaming needs no further per-request tuning yet
    let ctx = context::build(
        executor,
        config.aishe.redact_secrets,
        config.aishe.project_context,
    );
    let shell = executor
        .shell()
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "sh".to_string());
    let system = super::suggest_stream_system_prompt(&shell, std::env::consts::OS);
    let mut messages = session.history();
    messages.push(Msg::User(format!("{ctx}\nUser request: {input}")));

    let model = config.active_model();
    let mode = config.aishe.mode.as_str();
    crate::audit::ai_request(mode, model, input);
    let before = provider.meter().snapshot();
    let mut streamer = AnswerStreamer::new();
    let mut out = std::io::stdout();
    // Streaming uses the CMD:/WHY: sentinel protocol, not JSON — unconstrained.
    let result =
        provider.complete_stream(&system, &messages, &ResponseFormat::Text, &mut |delta| {
            streamer.push(delta, &mut out);
        });
    let full = match result {
        Ok(f) => f,
        Err(e) => {
            crate::audit::ai_error(mode, model, &e.to_string());
            eprintln!("{}", format!("aishe: {e}").red());
            return Ok(());
        }
    };
    let after = provider.meter().snapshot();
    crate::audit::ai_response(
        mode,
        model,
        &full,
        after.input.saturating_sub(before.input),
        after.output.saturating_sub(before.output),
    );

    if streamer.finish(&mut out) {
        // The model committed to a command; nothing was streamed to the screen.
        let (command, explanation) = parse_cmd_protocol(&full);
        if command.is_empty() {
            return Ok(());
        }
        record_turn(
            session,
            input,
            &Suggestion::Command {
                command: command.clone(),
                explanation: explanation.clone(),
            },
        );
        if auto {
            auto_run(&command, &explanation, executor)
        } else {
            present_command(&command, &explanation, executor)
        }
    } else {
        // A prose answer was streamed live as raw text; re-render it as markdown
        // in place (code blocks, lists, emphasis) now that it is complete.
        super::rerender_streamed_markdown(&full);
        record_turn(
            session,
            input,
            &Suggestion::Answer {
                explanation: full.trim().to_string(),
            },
        );
        Ok(())
    }
}

/// Routes a streamed response to either a live prose answer or a buffered
/// command. While undecided it withholds output until it can tell whether the
/// text begins with the `CMD:` sentinel, so a command is never half-printed.
struct AnswerStreamer {
    /// `None` until decided; `Some(true)` = command, `Some(false)` = prose.
    is_command: Option<bool>,
    /// Output withheld while undecided.
    pending: String,
    /// The full response, accumulated regardless of decision.
    full: String,
}

impl AnswerStreamer {
    fn new() -> Self {
        Self {
            is_command: None,
            pending: String::new(),
            full: String::new(),
        }
    }

    fn push<W: std::io::Write>(&mut self, delta: &str, out: &mut W) {
        self.full.push_str(delta);
        match self.is_command {
            Some(true) => {} // command: swallow output
            Some(false) => {
                let _ = write!(out, "{delta}");
                let _ = out.flush();
            }
            None => {
                self.pending.push_str(delta);
                let trimmed = self.pending.trim_start();
                if trimmed.is_empty() {
                    return;
                }
                if trimmed.starts_with("CMD:") {
                    self.is_command = Some(true);
                    self.pending.clear();
                } else if "CMD:".starts_with(trimmed) {
                    // Still possibly the start of "CMD:" — keep buffering.
                } else {
                    self.is_command = Some(false);
                    let _ = write!(out, "{}", self.pending);
                    let _ = out.flush();
                    self.pending.clear();
                }
            }
        }
    }

    /// Finalize and report whether the response was a command. Flushes any
    /// withheld prose for very short responses that never tripped a decision.
    fn finish<W: std::io::Write>(&mut self, out: &mut W) -> bool {
        match self.is_command {
            Some(decided) => decided,
            None => {
                let is_cmd = self.full.trim_start().starts_with("CMD:");
                if !is_cmd && !self.pending.is_empty() {
                    let _ = write!(out, "{}", self.pending);
                    let _ = out.flush();
                }
                is_cmd
            }
        }
    }
}

/// Parse the `CMD:`/`WHY:` sentinel protocol into (command, explanation).
fn parse_cmd_protocol(text: &str) -> (String, String) {
    let mut command = String::new();
    let mut explanation = String::new();
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("CMD:") {
            if command.is_empty() {
                command = rest.trim().to_string();
            }
        } else if let Some(rest) = l.strip_prefix("WHY:") {
            if explanation.is_empty() {
                explanation = rest.trim().to_string();
            }
        }
    }
    (command, explanation)
}

/// The response-format strategy for suggest mode, per config (`structured`):
/// strict `schema` (default) → `json` object → unconstrained `prompt`.
fn suggestion_format(config: &Config) -> ResponseFormat {
    match config.aishe.structured.as_str() {
        "json" => ResponseFormat::Json,
        "prompt" => ResponseFormat::Text,
        _ => ResponseFormat::JsonSchema {
            name: "aishe_suggestion".to_string(),
            schema: suggestion_schema(),
        },
    }
}

/// Strict JSON Schema for a suggestion (matches `RawSuggestion`).
fn suggestion_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "type": {"type": "string", "enum": ["command", "answer"]},
            "command": {"type": ["string", "null"]},
            "explanation": {"type": "string"}
        },
        "required": ["type", "command", "explanation"]
    })
}

/// Ask the provider for a suggestion given the user's input + context, primed
/// with any prior conversation turns in `history`.
pub fn request(
    input: &str,
    provider: &dyn Provider,
    executor: &Executor,
    config: &Config,
    history: Vec<Msg>,
) -> Result<Suggestion> {
    let ctx = context::build(
        executor,
        config.aishe.redact_secrets,
        config.aishe.project_context,
    );
    let shell = executor
        .shell()
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "sh".to_string());
    let os = std::env::consts::OS;
    let system = super::suggest_system_prompt(&shell, os);
    let mut messages = history;
    messages.push(Msg::User(format!("{ctx}\nUser request: {input}")));

    let model = config.active_model();
    let mode = config.aishe.mode.as_str();
    crate::audit::ai_request(mode, model, input);
    let before = provider.meter().snapshot();
    match provider.complete(&system, &messages, &suggestion_format(config)) {
        Ok(text) => {
            let after = provider.meter().snapshot();
            let s = parse_suggestion(&text);
            crate::audit::ai_response(
                mode,
                model,
                &suggestion_summary(&s),
                after.input.saturating_sub(before.input),
                after.output.saturating_sub(before.output),
            );
            Ok(s)
        }
        Err(e) => {
            crate::audit::ai_error(mode, model, &e.to_string());
            eprintln!("{}", format!("aishe: {e}").red());
            // Surface the failure as an empty answer so the REPL keeps going.
            Ok(Suggestion::Answer {
                explanation: String::new(),
            })
        }
    }
}

/// One-line summary of a suggestion for the audit log.
fn suggestion_summary(s: &Suggestion) -> String {
    match s {
        Suggestion::Command { command, .. } => format!("command: {command}"),
        Suggestion::Answer { explanation } => format!("answer: {explanation}"),
    }
}

fn handle_suggestion(
    suggestion: Suggestion,
    executor: &mut Executor,
    scriptable: bool,
    auto: bool,
) -> Result<()> {
    match suggestion {
        Suggestion::Answer { explanation } => {
            if !explanation.is_empty() {
                render_markdown(&explanation);
            }
            Ok(())
        }
        Suggestion::Command {
            command,
            explanation,
        } => {
            if scriptable {
                // -c with NL in suggest mode: print the command and exit 0.
                println!("{command}");
                return Ok(());
            }
            if auto {
                return auto_run(&command, &explanation, executor);
            }
            present_command(&command, &explanation, executor)
        }
    }
}

/// Auto mode: show the command, then run it immediately if Safe, or fall
/// through the safety gate (which confirms) if Dangerous.
fn auto_run(command: &str, explanation: &str, executor: &mut Executor) -> Result<()> {
    println!();
    println!("  {} {}", "»".green(), command.white().bold());
    if !explanation.is_empty() {
        println!("  {}", explanation.dim());
    }
    run_with_gate(command, executor, "auto")
}

fn present_command(command: &str, explanation: &str, executor: &mut Executor) -> Result<()> {
    println!();
    println!("  {}", command.white().bold());
    if !explanation.is_empty() {
        println!("  {}", explanation.dim());
    }
    print!(
        "  {}  {}  {}  ",
        "[Enter] run".green(),
        "[e] edit".cyan(),
        "[n/Esc] cancel".dim()
    );
    use std::io::Write;
    std::io::stdout().flush().ok();

    let choice = read_choice()?;
    println!();

    match choice {
        Choice::Run => run_with_gate(command, executor, "suggest"),
        Choice::Edit => {
            if let Some(edited) = edit_line(command)? {
                let edited = edited.trim();
                if !edited.is_empty() {
                    run_with_gate(edited, executor, "suggest")?;
                }
            }
            Ok(())
        }
        Choice::Cancel => {
            println!("  {}", "cancelled".dim());
            Ok(())
        }
    }
}

fn run_with_gate(command: &str, executor: &mut Executor, source: &str) -> Result<()> {
    match safety_gate(command) {
        GateOutcome::Declined => {
            crate::audit::action(source, command, None);
            Ok(())
        }
        GateOutcome::Proceed => {
            let code = executor.run(command);
            crate::audit::action(source, command, Some(code));
            if code != 0 {
                println!("  {}", "(type ? to ask about the error)".dim());
            }
            Ok(())
        }
    }
}

enum Choice {
    Run,
    Edit,
    Cancel,
}

/// Read a single keypress: Enter=run, e=edit, n/Esc=cancel. Ctrl-C cancels.
fn read_choice() -> Result<Choice> {
    terminal::enable_raw_mode()?;
    let result = loop {
        match event::read() {
            Ok(Event::Key(KeyEvent {
                code, modifiers, ..
            })) => match code {
                KeyCode::Enter => break Choice::Run,
                KeyCode::Char('e') | KeyCode::Char('E') => break Choice::Edit,
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => break Choice::Cancel,
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    break Choice::Cancel
                }
                _ => continue,
            },
            Ok(_) => continue,
            Err(_) => break Choice::Cancel,
        }
    };
    terminal::disable_raw_mode()?;
    Ok(result)
}

/// Prompt for an edited version of the command: show it and read a replacement
/// line from stdin (empty input keeps the original). A plain stdin read, so it
/// has no dependency on a built-in line editor.
fn edit_line(initial: &str) -> Result<Option<String>> {
    use std::io::Write;
    eprint!("  edit (Enter to keep) [{initial}]: ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line)? == 0 {
        return Ok(None); // EOF / Ctrl-D
    }
    let edited = line.trim();
    Ok(Some(if edited.is_empty() {
        initial.to_string()
    } else {
        edited.to_string()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command() {
        let raw = r#"{"type":"command","command":"ls -la","explanation":"list files"}"#;
        assert_eq!(
            parse_suggestion(raw),
            Suggestion::Command {
                command: "ls -la".into(),
                explanation: "list files".into()
            }
        );
    }

    #[test]
    fn parse_answer() {
        let raw = r#"{"type":"answer","command":null,"explanation":"42"}"#;
        assert_eq!(
            parse_suggestion(raw),
            Suggestion::Answer {
                explanation: "42".into()
            }
        );
    }

    #[test]
    fn parse_fenced() {
        let raw = "```json\n{\"type\":\"command\",\"command\":\"pwd\",\"explanation\":\"x\"}\n```";
        assert_eq!(
            parse_suggestion(raw),
            Suggestion::Command {
                command: "pwd".into(),
                explanation: "x".into()
            }
        );
    }

    #[test]
    fn parse_garbage_is_answer() {
        let raw = "I think you should just run ls";
        match parse_suggestion(raw) {
            Suggestion::Answer { explanation } => assert!(explanation.contains("ls")),
            _ => panic!("expected answer fallback"),
        }
    }

    #[test]
    fn parse_command_without_command_field_is_answer() {
        let raw = r#"{"type":"command","command":null,"explanation":"cannot do that"}"#;
        assert!(matches!(parse_suggestion(raw), Suggestion::Answer { .. }));
    }

    /// Drive a streamer with a sequence of deltas; return (is_command, printed).
    fn drive(deltas: &[&str]) -> (bool, String) {
        let mut s = AnswerStreamer::new();
        let mut out: Vec<u8> = Vec::new();
        for d in deltas {
            s.push(d, &mut out);
        }
        let is_cmd = s.finish(&mut out);
        (is_cmd, String::from_utf8(out).unwrap())
    }

    #[test]
    fn streams_prose_live_and_withholds_nothing() {
        let (is_cmd, printed) = drive(&["The ", "answer ", "is 42."]);
        assert!(!is_cmd);
        assert_eq!(printed, "The answer is 42.");
    }

    #[test]
    fn command_is_never_printed() {
        // Even when the sentinel is split across deltas, no command text leaks.
        let (is_cmd, printed) = drive(&["CM", "D: rm -rf build\nWHY: clean"]);
        assert!(is_cmd);
        assert_eq!(printed, "");
    }

    #[test]
    fn leading_whitespace_before_sentinel_still_detected() {
        let (is_cmd, printed) = drive(&["\n  CMD: ls -la"]);
        assert!(is_cmd);
        assert_eq!(printed, "");
    }

    #[test]
    fn short_prose_flushed_on_finish() {
        // "Hi" never disambiguates from "CMD:" by prefix, so it's held until end.
        let (is_cmd, printed) = drive(&["Hi"]);
        assert!(!is_cmd);
        assert_eq!(printed, "Hi");
    }

    #[test]
    fn parses_cmd_and_why_protocol() {
        let (cmd, why) = parse_cmd_protocol("CMD: du -sh *\nWHY: show sizes");
        assert_eq!(cmd, "du -sh *");
        assert_eq!(why, "show sizes");
    }

    #[test]
    fn parses_cmd_without_why() {
        let (cmd, why) = parse_cmd_protocol("CMD: pwd");
        assert_eq!(cmd, "pwd");
        assert_eq!(why, "");
    }
}
