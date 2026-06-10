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
use crate::providers::{Msg, Provider};

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
) -> Result<()> {
    let suggestion = request(input, provider, executor, config)?;
    handle_suggestion(suggestion, executor, scriptable, auto)
}

/// Ask the provider for a suggestion given the user's input + context.
pub fn request(
    input: &str,
    provider: &dyn Provider,
    executor: &Executor,
    config: &Config,
) -> Result<Suggestion> {
    let ctx = context::build(executor);
    let shell = executor
        .shell()
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "sh".to_string());
    let os = std::env::consts::OS;
    let system = super::suggest_system_prompt(&shell, os);
    let user = format!("{ctx}\nUser request: {input}");

    let _ = config; // reserved for future per-request tuning
    match provider.complete(&system, &[Msg::User(user)], true) {
        Ok(text) => Ok(parse_suggestion(&text)),
        Err(e) => {
            eprintln!("{}", format!("aishe: {e}").red());
            // Surface the failure as an empty answer so the REPL keeps going.
            Ok(Suggestion::Answer {
                explanation: String::new(),
            })
        }
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
    run_with_gate(command, executor)
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
        Choice::Run => run_with_gate(command, executor),
        Choice::Edit => {
            if let Some(edited) = edit_line(command)? {
                let edited = edited.trim();
                if !edited.is_empty() {
                    run_with_gate(edited, executor)?;
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

fn run_with_gate(command: &str, executor: &mut Executor) -> Result<()> {
    match safety_gate(command) {
        GateOutcome::Declined => Ok(()),
        GateOutcome::Proceed => {
            let code = executor.run(command);
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

/// Re-open a line editor pre-filled with the command for editing.
fn edit_line(initial: &str) -> Result<Option<String>> {
    use reedline::{DefaultPrompt, DefaultPromptSegment, Reedline, Signal};

    let mut line_editor = Reedline::create();
    let prompt = DefaultPrompt::new(
        DefaultPromptSegment::Basic("edit".to_string()),
        DefaultPromptSegment::Empty,
    );

    // Pre-fill the buffer with the command to edit.
    line_editor.run_edit_commands(&[reedline::EditCommand::InsertString(initial.to_string())]);

    match line_editor.read_line(&prompt) {
        Ok(Signal::Success(buffer)) => Ok(Some(buffer)),
        Ok(Signal::CtrlC) | Ok(Signal::CtrlD) => Ok(None),
        Err(_) => Ok(None),
    }
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
}
