//! Small terminal UI primitives used by setup, settings, and the tour. This is
//! deliberately not a full-screen application: it remains readable over SSH,
//! supports arrow keys and simple letter shortcuts, and restores terminal mode
//! on every exit path.

use std::io::{IsTerminal, Read, Write};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

const ACCENT: &str = "1;36";
const MUTED: &str = "2";
const FOCUS: &str = "1;36;7";
const WARNING: &str = "1;33";
const SUCCESS: &str = "1;32";
const ERROR: &str = "1;31";

/// Monochrome terminal approximation of the README's circuit-brain mark.
/// ASCII only: terminals never need an emoji font or colored pictograph.
pub const ASCII_LOGO: &str = r#"  .-----. .-----.
 /  o--| |--o  \
|  /---+ +---\  |
|  \---+ +---/  |
 \__o--| |--o__/
       AISHE"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuResult {
    Selected(usize),
    Back,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerResult {
    Use(usize),
    SaveDefault(usize),
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerKey {
    Up,
    Down,
    Enter,
    SaveDefault,
    Backspace,
    Cancel,
    Character(char),
    Other,
}

/// Filterable single-column picker used by `/model`. It intentionally uses
/// plain text and ASCII focus marks so terminal font/theme choices never turn
/// status into colored pictographs.
pub fn filter_picker(title: &str, options: &[String], default: usize) -> Result<PickerResult> {
    if options.is_empty() {
        anyhow::bail!("picker '{title}' has no options");
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive picker requires a terminal");
    }
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
    println!("  Type to filter · Enter use in this shell · d save as default · Esc cancel");
    let guard = RawGuard::enter()?;
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut filter = String::new();
    let mut selected = default.min(options.len() - 1);
    loop {
        let matches: Vec<usize> = options
            .iter()
            .enumerate()
            .filter(|(_, option)| {
                option
                    .to_ascii_lowercase()
                    .contains(&filter.to_ascii_lowercase())
            })
            .map(|(index, _)| index)
            .collect();
        if selected >= matches.len() {
            selected = 0;
        }
        print!(
            "\r\x1b[2K  filter: {}\r\n\x1b[J",
            crate::commands::display_safe(&filter)
        );
        for (row, index) in matches.iter().take(20).enumerate() {
            let marker = if row == selected { ">" } else { " " };
            println!(
                "  {marker} {}",
                crate::commands::display_safe(&options[*index])
            );
        }
        if matches.is_empty() {
            println!("    no matches");
        }
        std::io::stdout().flush().ok();
        match read_picker_key(&mut input).context("reading picker input")? {
            PickerKey::Up => {
                selected = selected
                    .checked_sub(1)
                    .unwrap_or(matches.len().saturating_sub(1))
            }
            PickerKey::Down => {
                if !matches.is_empty() {
                    selected = (selected + 1) % matches.len();
                }
            }
            PickerKey::Enter if !matches.is_empty() => {
                drop(guard);
                println!();
                return Ok(PickerResult::Use(matches[selected]));
            }
            PickerKey::SaveDefault
                if (filter.is_empty() || matches.len() == 1) && !matches.is_empty() =>
            {
                drop(guard);
                println!();
                return Ok(PickerResult::SaveDefault(matches[selected]));
            }
            PickerKey::SaveDefault => {
                filter.push('d');
                selected = 0;
            }
            PickerKey::Backspace => {
                filter.pop();
                selected = 0;
            }
            PickerKey::Character(c) => {
                if !c.is_control() {
                    filter.push(c);
                    selected = 0;
                }
            }
            PickerKey::Cancel => {
                drop(guard);
                println!();
                return Ok(PickerResult::Cancel);
            }
            _ => {}
        }
    }
}

/// Crossterm's global Unix input reader cannot initialize while this process is
/// launched synchronously from a live zsh ZLE widget. Read this picker's small
/// key vocabulary directly from the already-verified terminal instead. This
/// keeps `/model` usable both as a standalone command and inside Aishe's shell.
fn read_picker_key(input: &mut impl Read) -> std::io::Result<PickerKey> {
    let mut first = [0_u8; 1];
    input.read_exact(&mut first)?;
    Ok(match first[0] {
        b'\r' | b'\n' => PickerKey::Enter,
        3 => PickerKey::Cancel,
        8 | 127 => PickerKey::Backspace,
        27 => {
            let mut pollfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: `pollfd` points to one initialized entry and the call
            // does not outlive it. A short timeout distinguishes Esc from an
            // arrow-key escape sequence without making cancellation feel slow.
            let available = unsafe { libc::poll(&mut pollfd, 1, 25) };
            if available <= 0 || pollfd.revents & libc::POLLIN == 0 {
                PickerKey::Cancel
            } else {
                let mut sequence = [0_u8; 2];
                input.read_exact(&mut sequence)?;
                match sequence {
                    [b'[', b'A'] => PickerKey::Up,
                    [b'[', b'B'] => PickerKey::Down,
                    _ => PickerKey::Other,
                }
            }
        }
        b'd' => PickerKey::SaveDefault,
        byte if byte.is_ascii() => PickerKey::Character(char::from(byte)),
        byte => {
            let width = if byte & 0b1110_0000 == 0b1100_0000 {
                2
            } else if byte & 0b1111_0000 == 0b1110_0000 {
                3
            } else if byte & 0b1111_1000 == 0b1111_0000 {
                4
            } else {
                1
            };
            let mut bytes = [0_u8; 4];
            bytes[0] = byte;
            if width > 1 {
                input.read_exact(&mut bytes[1..width])?;
            }
            std::str::from_utf8(&bytes[..width])
                .ok()
                .and_then(|value| value.chars().next())
                .map(PickerKey::Character)
                .unwrap_or(PickerKey::Other)
        }
    })
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

fn color_enabled() -> bool {
    color_enabled_for(
        std::io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
        std::env::var("TERM").ok().as_deref(),
    )
}

fn color_enabled_for(is_terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    is_terminal && !no_color && !term.is_some_and(|value| value.eq_ignore_ascii_case("dumb"))
}

fn paint(text: &str, style: &str) -> String {
    if color_enabled() {
        format!("\x1b[{style}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn columns() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
}

fn display_width(value: &str) -> usize {
    value.chars().count()
}

fn truncate_to_width(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.to_string();
    }
    match width {
        0 => String::new(),
        1 => "…".to_string(),
        _ => value
            .chars()
            .take(width - 1)
            .chain(std::iter::once('…'))
            .collect(),
    }
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let characters: Vec<char> = value.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < characters.len() {
        while start < characters.len() && characters[start].is_whitespace() {
            start += 1;
        }
        if start == characters.len() {
            break;
        }
        if characters.len() - start <= width {
            lines.push(characters[start..].iter().collect());
            break;
        }
        let hard_end = start + width;
        let end = (start + 1..hard_end)
            .rev()
            .find(|index| characters[*index].is_whitespace())
            .unwrap_or(hard_end);
        lines.push(characters[start..end].iter().collect());
        start = end;
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn print_wrapped(indent: &str, value: &str, style: Option<&str>) {
    let safe = crate::commands::display_safe(value);
    let available = columns().saturating_sub(display_width(indent)).max(1);
    for line in wrap_text(&safe, available) {
        let line = style.map_or(line.clone(), |code| paint(&line, code));
        println!("{indent}{line}");
    }
}

/// Print a compact, width-aware header shared by the interactive setup-like
/// experiences. Styling is deliberately ANSI-only and opt-out via NO_COLOR so
/// it remains readable over SSH and in basic terminals.
pub fn header(title: &str, description: &str, note: &str) {
    let safe_title = crate::commands::display_safe(title);
    println!("\n  {}", paint(&safe_title, ACCENT));
    println!(
        "  {}",
        paint(&"─".repeat(display_width(&safe_title)), ACCENT)
    );
    print_wrapped("  ", description, None);
    print_wrapped("  ", note, Some(MUTED));
}

pub fn brand() {
    println!("\n{ASCII_LOGO}");
}

pub fn section(title: &str) {
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
}

pub fn success(message: &str) {
    print_wrapped("  ", &format!("✓ {message}"), Some(SUCCESS));
}

pub fn warning(message: &str) {
    print_wrapped("  ", &format!("! {message}"), Some(WARNING));
}

pub fn error(message: &str) {
    print_wrapped("  ", &format!("✗ {message}"), Some(ERROR));
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
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
    let terminal_columns = columns();
    for (index, option) in options.iter().enumerate() {
        print_option(index, option, terminal_columns);
    }
    let instructions = format!(
        "↑/↓ or number select · Enter accept{} · ? help · Esc cancel",
        if allow_back { " · b back" } else { "" }
    );
    print_wrapped("  ", &instructions, Some(MUTED));
    let mut selected = default.min(options.len() - 1);
    // Enter raw mode before advertising an active focus row. Otherwise a fast
    // typist (or PTY automation) can submit a numeric choice while the terminal
    // is still in canonical mode; the buffered digit/newline can then be
    // observed out of order when crossterm starts reading events.
    let guard = RawGuard::enter()?;
    print_selection(selected, &options[selected], terminal_columns);
    let mut number_buffer = String::new();
    loop {
        let Event::Key(KeyEvent {
            code, modifiers, ..
        }) = event::read().context("reading terminal input")?
        else {
            continue;
        };
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                number_buffer.clear();
                selected = selected.checked_sub(1).unwrap_or(options.len() - 1);
                print_selection(selected, &options[selected], terminal_columns);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                number_buffer.clear();
                selected = (selected + 1) % options.len();
                print_selection(selected, &options[selected], terminal_columns);
            }
            KeyCode::Enter => {
                drop(guard);
                println!();
                return Ok(MenuResult::Selected(selected));
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                number_buffer.push(c);
                if !(1..=options.len()).any(|number| number.to_string().starts_with(&number_buffer))
                {
                    number_buffer.clear();
                    number_buffer.push(c);
                }
                let index = number_buffer.parse::<usize>().unwrap_or(0);
                if index >= 1 && index <= options.len() {
                    selected = index - 1;
                    print_selection(selected, &options[selected], terminal_columns);
                }
            }
            KeyCode::Char('b') | KeyCode::Char('B') if allow_back => {
                drop(guard);
                println!();
                return Ok(MenuResult::Back);
            }
            KeyCode::Char('?') => {
                number_buffer.clear();
                print_help(help, terminal_columns);
                print_selection(selected, &options[selected], terminal_columns);
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                drop(guard);
                println!();
                return Ok(MenuResult::Cancel);
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                drop(guard);
                println!();
                return Ok(MenuResult::Cancel);
            }
            _ => {}
        }
    }
}

fn print_option(index: usize, option: &str, terminal_columns: usize) {
    let number = format!("{})", index + 1);
    let prefix = format!("    {number} ");
    let safe = crate::commands::display_safe(option);
    let available = terminal_columns
        .saturating_sub(display_width(&prefix))
        .max(1);
    let lines = wrap_text(&safe, available);
    println!("    {} {}", paint(&number, ACCENT), lines[0]);
    let continuation = " ".repeat(display_width(&prefix));
    for line in lines.iter().skip(1) {
        println!("{continuation}{line}");
    }
}

fn print_selection(index: usize, label: &str, terminal_columns: usize) {
    let safe = crate::commands::display_safe(label);
    let prefix = format!("› {}) ", index + 1);
    let available = terminal_columns.saturating_sub(2).max(1);
    let content = truncate_to_width(&format!("{prefix}{safe}"), available);
    print!("\r\x1b[2K  {}", paint(&content, FOCUS));
    std::io::stdout().flush().ok();
}

fn print_help(help: &str, terminal_columns: usize) {
    let safe = crate::commands::display_safe(help);
    let available = terminal_columns.saturating_sub(2).max(1);
    print!("\r\x1b[2K");
    for line in wrap_text(&safe, available) {
        print!("  {}\r\n", paint(&line, MUTED));
    }
}

pub fn text(
    label: &str,
    default: &str,
    validate: impl Fn(&str) -> Result<()>,
) -> Result<Option<String>> {
    loop {
        print_text_prompt(label, default);
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
            Err(error) => println!(
                "  {}",
                paint(
                    &format!("! {}", crate::commands::display_safe(&error.to_string())),
                    WARNING
                )
            ),
        }
    }
}

fn print_text_prompt(label: &str, default: &str) {
    let safe_label = crate::commands::display_safe(label);
    let safe_default = crate::commands::display_safe(default);
    let compact_width = 2
        + display_width(&safe_label)
        + display_width(&safe_default)
        + display_width(" [] (or :back/:cancel): ");
    if compact_width <= columns() {
        print!(
            "  {} [{}] {} ",
            paint(&safe_label, ACCENT),
            paint(&safe_default, MUTED),
            paint("(or :back/:cancel):", MUTED)
        );
    } else {
        println!("  {}", paint(&safe_label, ACCENT));
        let default_prefix = "    default: ";
        let available = columns()
            .saturating_sub(display_width(default_prefix))
            .max(1);
        let lines = wrap_text(&safe_default, available);
        println!(
            "{}{}",
            paint(default_prefix, MUTED),
            paint(&lines[0], MUTED)
        );
        let continuation = " ".repeat(display_width(default_prefix));
        for line in lines.iter().skip(1) {
            println!("{}{}", paint(&continuation, MUTED), paint(line, MUTED));
        }
        println!(
            "  {}",
            paint("Enter keeps the default · :back · :cancel", MUTED)
        );
        print!("  {} ", paint("›", ACCENT));
    }
}

/// Read a secret from an interactive terminal without echoing its characters.
/// Returns `None` for Esc/Ctrl-C/Ctrl-D. The buffer is bounded before any
/// validation so an accidental paste cannot grow memory without limit.
pub fn secret(label: &str, max_bytes: usize) -> Result<Option<String>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("hidden secret input requires a terminal");
    }
    print!(
        "  {} {} ",
        paint(&crate::commands::display_safe(label), ACCENT),
        paint("(hidden; Esc cancels):", MUTED)
    );
    std::io::stdout().flush().ok();
    let guard = RawGuard::enter()?;
    let mut value = String::new();
    loop {
        match event::read().context("reading hidden terminal input")? {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                drop(guard);
                println!();
                return Ok(Some(value));
            }
            Event::Key(KeyEvent {
                code: KeyCode::Backspace,
                ..
            }) => {
                value.pop();
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            })
            | Event::Key(KeyEvent {
                code: KeyCode::Char('c' | 'd'),
                modifiers: KeyModifiers::CONTROL,
                ..
            }) => {
                drop(guard);
                println!();
                return Ok(None);
            }
            Event::Key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers,
                ..
            }) if !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                if value.len() + character.len_utf8() <= max_bytes {
                    value.push(character);
                }
            }
            Event::Paste(pasted) if value.len() + pasted.len() <= max_bytes => {
                value.push_str(&pasted);
            }
            _ => {}
        }
    }
}

pub fn confirm(label: &str, default: bool) -> Result<Option<bool>> {
    let suffix = if default { "[Y/n]" } else { "[y/N]" };
    loop {
        print!(
            "  {} {} ",
            paint(&crate::commands::display_safe(label), ACCENT),
            paint(&format!("{suffix}:"), MUTED)
        );
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
            _ => println!("  {}", paint("! enter y, n, or cancel", WARNING)),
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

    #[test]
    fn wrapping_prefers_words_and_hard_wraps_long_tokens() {
        assert_eq!(
            wrap_text("Official OpenAI Responses API", 16),
            ["Official OpenAI", "Responses API"]
        );
        assert_eq!(wrap_text("abcdefghijkl", 5), ["abcde", "fghij", "kl"]);
        assert!(wrap_text("", 20).iter().all(String::is_empty));
    }

    #[test]
    fn truncation_keeps_the_focus_row_on_one_terminal_line() {
        assert_eq!(truncate_to_width("short", 10), "short");
        assert_eq!(truncate_to_width("a long selection", 8), "a long …");
        assert_eq!(truncate_to_width("anything", 1), "…");
    }

    #[test]
    fn color_policy_respects_no_color_and_dumb_terminals() {
        assert!(color_enabled_for(true, false, Some("xterm-256color")));
        assert!(!color_enabled_for(true, true, Some("xterm-256color")));
        assert!(!color_enabled_for(true, false, Some("dumb")));
        assert!(!color_enabled_for(false, false, Some("xterm-256color")));
    }
}
