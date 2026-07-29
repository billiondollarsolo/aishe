//! Small terminal UI primitives used by setup, settings, and the tour. This is
//! deliberately not a full-screen application: it remains readable over SSH,
//! supports arrow keys and simple letter shortcuts, and restores terminal mode
//! on every exit path.

use std::io::{IsTerminal, Write};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuResult {
    Selected(usize),
    Back,
    Cancel,
}

struct RawGuard;

impl RawGuard {
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("enabling terminal raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

pub fn menu(
    title: &str,
    options: &[String],
    default: usize,
    allow_back: bool,
    help: &str,
) -> Result<MenuResult> {
    if options.is_empty() {
        anyhow::bail!("menu '{title}' has no options");
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive menu requires a terminal");
    }
    println!("\n  {}", crate::commands::display_safe(title));
    for (index, option) in options.iter().enumerate() {
        println!(
            "    {}) {}",
            index + 1,
            crate::commands::display_safe(option)
        );
    }
    println!(
        "  ↑/↓ select · Enter accept{} · ? help · Esc cancel",
        if allow_back { " · b back" } else { "" }
    );
    let mut selected = default.min(options.len() - 1);
    print_selection(&options[selected]);
    let _guard = RawGuard::enter()?;
    loop {
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read().context("reading terminal input")?
        else {
            continue;
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                selected = selected.checked_sub(1).unwrap_or(options.len() - 1);
                print_selection(&options[selected]);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                selected = (selected + 1) % options.len();
                print_selection(&options[selected]);
            }
            KeyCode::Enter => {
                println!();
                return Ok(MenuResult::Selected(selected));
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                let index = c.to_digit(10).unwrap_or(0) as usize;
                if index >= 1 && index <= options.len() {
                    selected = index - 1;
                    print_selection(&options[selected]);
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B') if allow_back => {
                println!();
                return Ok(MenuResult::Back);
            }
            KeyCode::Char('?') => {
                println!("\r\x1b[2K  {}", crate::commands::display_safe(help));
                print_selection(&options[selected]);
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                println!();
                return Ok(MenuResult::Cancel);
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                println!();
                return Ok(MenuResult::Cancel);
            }
            _ => {}
        }
    }
}

fn print_selection(label: &str) {
    print!(
        "\r\x1b[2K  selected: {}",
        crate::commands::display_safe(label)
    );
    std::io::stdout().flush().ok();
}

pub fn text(
    label: &str,
    default: &str,
    validate: impl Fn(&str) -> Result<()>,
) -> Result<Option<String>> {
    loop {
        print!(
            "  {} [{}] (or :back/:cancel): ",
            crate::commands::display_safe(label),
            crate::commands::display_safe(default)
        );
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Ok(None);
        }
        let value = line.trim();
        if value.eq_ignore_ascii_case(":cancel") {
            return Ok(None);
        }
        if value.eq_ignore_ascii_case(":back") {
            return Ok(Some(":back".into()));
        }
        let value = if value.is_empty() { default } else { value };
        match validate(value) {
            Ok(()) => return Ok(Some(value.to_string())),
            Err(error) => println!("  ! {}", crate::commands::display_safe(&error.to_string())),
        }
    }
}

pub fn confirm(label: &str, default: bool) -> Result<Option<bool>> {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    loop {
        print!("  {} {suffix}: ", crate::commands::display_safe(label));
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "" => return Ok(Some(default)),
            "y" | "yes" => return Ok(Some(true)),
            "n" | "no" => return Ok(Some(false)),
            "q" | "cancel" => return Ok(None),
            _ => println!("  ! enter y, n, or cancel"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_result_is_stable_for_state_machines() {
        assert_eq!(MenuResult::Selected(2), MenuResult::Selected(2));
        assert_ne!(MenuResult::Back, MenuResult::Cancel);
    }
}
