//! Small terminal UI primitives used by setup, settings, and the tour. This is
//! deliberately not a full-screen application: it remains readable over SSH,
//! supports arrow keys and simple letter shortcuts, and restores terminal mode
//! on every exit path.

use std::io::{IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

const ACCENT: &str = "1;36";
const MUTED: &str = "2";
const FOCUS: &str = "1;36;7";
const WARNING: &str = "1;33";
const SUCCESS: &str = "1;32";
const ERROR: &str = "1;31";

/// Monochrome terminal approximation of the AISHE robot mark (antennae + chassis).
/// ASCII only: terminals never need an emoji font or colored pictograph.
/// Branding: **AISHE** = **AI Shell**.
pub const ASCII_LOGO: &str = r#"  .-----. .-----.
 /  o--| |--o  \
|  /---+ +---\  |
|  \---+ +---/  |
 \__o--| |--o__/
       AISHE
      AI Shell"#;

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
///
/// The interactive body runs in raw mode. Every redraw must use `\r\n` (not
/// bare `\n`) and move the cursor back to the top of the previous frame;
/// otherwise multi-row lists staircase to the right on each line.
pub fn filter_picker(title: &str, options: &[String], default: usize) -> Result<PickerResult> {
    if options.is_empty() {
        anyhow::bail!("picker '{title}' has no options");
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("interactive picker requires a terminal");
    }
    // Title/help print in cooked mode so their newlines behave normally.
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
    println!(
        "  ↑/↓ or j/k move · type to filter · Enter use in this shell · d save as default · Esc cancel"
    );
    // Read keys from an unbuffered /dev/tty (not StdinLock). A buffered stdin
    // read of ESC leaves the rest of an arrow CSI in the user-space buffer
    // while poll(STDIN) sees an empty kernel buffer and treats ↑ as Esc cancel.
    let mut keys = PickerInput::open().context("opening picker input")?;
    let guard = RawGuard::enter()?;
    let mut filter = String::new();
    let mut selected = default.min(options.len() - 1);
    let mut drawn_rows = 0_usize;
    loop {
        let matches = picker_matches(options, &filter);
        if selected >= matches.len() {
            selected = 0;
        }
        let lines = picker_frame_lines(options, &matches, &filter, selected);
        draw_raw_frame(&lines, &mut drawn_rows);
        std::io::stdout().flush().ok();
        match keys.read_key().context("reading picker input")? {
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
                // 'd' with an active filter string is typing, not "save default".
                filter.push('d');
                selected = 0;
            }
            PickerKey::Backspace => {
                filter.pop();
                selected = 0;
            }
            // j/k navigate when not filtering (vim-style + no accidental filter noise).
            PickerKey::Character('k') if filter.is_empty() => {
                selected = selected
                    .checked_sub(1)
                    .unwrap_or(matches.len().saturating_sub(1))
            }
            PickerKey::Character('j') if filter.is_empty() => {
                if !matches.is_empty() {
                    selected = (selected + 1) % matches.len();
                }
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

fn picker_matches(options: &[String], filter: &str) -> Vec<usize> {
    let needle = filter.to_ascii_lowercase();
    options
        .iter()
        .enumerate()
        .filter(|(_, option)| option.to_ascii_lowercase().contains(&needle))
        .map(|(index, _)| index)
        .collect()
}

/// Build the filter line plus up to 20 option rows for one picker frame.
fn picker_frame_lines(
    options: &[String],
    matches: &[usize],
    filter: &str,
    selected: usize,
) -> Vec<String> {
    let mut lines = vec![format!(
        "  filter: {}",
        crate::commands::display_safe(filter)
    )];
    for (row, index) in matches.iter().take(20).enumerate() {
        let marker = if row == selected { ">" } else { " " };
        lines.push(format!(
            "  {marker} {}",
            crate::commands::display_safe(&options[*index])
        ));
    }
    if matches.is_empty() {
        lines.push("    no matches".into());
    }
    lines
}

/// Redraw a multi-line frame under raw mode without staircasing columns.
///
/// `drawn_rows` is the number of content lines written on the previous frame.
/// After each frame the cursor sits on the blank line immediately below the
/// last content row, so the next redraw moves up exactly `drawn_rows` lines.
fn draw_raw_frame(lines: &[String], drawn_rows: &mut usize) {
    let width = columns().max(1);
    if *drawn_rows > 0 {
        // Cursor is on the blank line under the previous frame.
        print!("\r\x1b[{}A", *drawn_rows);
    }
    for line in lines {
        let content = truncate_to_width(line, width);
        // Clear the full row, write content, then CRLF so the next row starts
        // at column 0 even when the terminal is in raw mode.
        print!("\r\x1b[2K{content}\r\n");
    }
    // Drop any leftover rows from a taller previous frame.
    print!("\x1b[J");
    *drawn_rows = lines.len();
}

/// Unbuffered controlling-terminal input for the filter picker.
///
/// Important: do **not** read arrow keys through `std::io::stdin().lock()`
/// (a `BufReader`). One `read_exact` of ESC can pull the entire CSI sequence
/// into the user-space buffer; a subsequent `poll(STDIN)` then sees no kernel
/// data and times out as bare Esc → cancel. That is why ↑ looked like cancel
/// in `/model` and `/connection` over SSH and local PTYs.
struct PickerInput {
    #[cfg(unix)]
    file: std::fs::File,
    #[cfg(not(unix))]
    // Fallback: unbuffered stdin (best-effort).
    _stdin: std::io::Stdin,
    /// Bytes already taken from a multi-byte read but not yet consumed as keys.
    pending: Vec<u8>,
}

impl PickerInput {
    fn open() -> Result<Self> {
        #[cfg(unix)]
        {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .context("opening /dev/tty for interactive picker keys")?;
            Ok(Self {
                file,
                pending: Vec::new(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                _stdin: std::io::stdin(),
                pending: Vec::new(),
            })
        }
    }

    fn read_byte(&mut self) -> std::io::Result<u8> {
        if let Some(byte) = self.pending.first().copied() {
            self.pending.remove(0);
            return Ok(byte);
        }
        let mut buf = [0_u8; 1];
        #[cfg(unix)]
        {
            self.file.read_exact(&mut buf)?;
        }
        #[cfg(not(unix))]
        {
            std::io::stdin().read_exact(&mut buf)?;
        }
        Ok(buf[0])
    }

    /// Non-blocking peek: return next byte if already pending or readable soon.
    fn poll_byte(&mut self, timeout_ms: i32) -> std::io::Result<Option<u8>> {
        if let Some(byte) = self.pending.first().copied() {
            self.pending.remove(0);
            return Ok(Some(byte));
        }
        if !self.poll_ready(timeout_ms) {
            return Ok(None);
        }
        Ok(Some(self.read_byte()?))
    }

    fn poll_ready(&self, timeout_ms: i32) -> bool {
        #[cfg(unix)]
        {
            let mut pollfd = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one initialized pollfd for the duration of the call.
            let available = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
            available > 0 && pollfd.revents & libc::POLLIN != 0
        }
        #[cfg(not(unix))]
        {
            let _ = timeout_ms;
            true
        }
    }

    fn read_key(&mut self) -> std::io::Result<PickerKey> {
        let first = self.read_byte()?;
        Ok(match first {
            b'\r' | b'\n' => PickerKey::Enter,
            3 => PickerKey::Cancel,
            8 | 127 => PickerKey::Backspace,
            27 => self.read_escape_sequence()?,
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
                let mut bytes = vec![byte];
                for _ in 1..width {
                    bytes.push(self.read_byte()?);
                }
                std::str::from_utf8(&bytes)
                    .ok()
                    .and_then(|value| value.chars().next())
                    .map(PickerKey::Character)
                    .unwrap_or(PickerKey::Other)
            }
        })
    }

    /// Parse ESC + CSI (`\x1b[…A`) or SS3 (`\x1bOA`) arrow sequences.
    fn read_escape_sequence(&mut self) -> std::io::Result<PickerKey> {
        // Prefer bytes already available; only then wait (SSH lag).
        let Some(second) = self.poll_byte(300)? else {
            return Ok(PickerKey::Cancel);
        };
        match second {
            // SS3 cursor keys (application cursor mode): ESC O A/B
            b'O' => {
                let Some(third) = self.poll_byte(150)? else {
                    return Ok(PickerKey::Other);
                };
                return Ok(match third {
                    b'A' => PickerKey::Up,
                    b'B' => PickerKey::Down,
                    _ => PickerKey::Other,
                });
            }
            // CSI: ESC [ … final
            b'[' => {}
            _ => return Ok(PickerKey::Other),
        }

        // Read CSI parameter/intermediate bytes until a final byte (0x40–0x7E).
        let mut final_byte = None;
        for _ in 0..16 {
            let Some(byte) = self.poll_byte(150)? else {
                break;
            };
            if (0x40..=0x7e).contains(&byte) {
                final_byte = Some(byte);
                break;
            }
        }
        Ok(match final_byte {
            Some(b'A') => PickerKey::Up,
            Some(b'B') => PickerKey::Down,
            _ => PickerKey::Other,
        })
    }
}

#[cfg(test)]
impl PickerInput {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            #[cfg(unix)]
            file: std::fs::OpenOptions::new()
                .read(true)
                .open("/dev/null")
                .expect("open /dev/null"),
            #[cfg(not(unix))]
            _stdin: std::io::stdin(),
            pending: bytes.to_vec(),
        }
    }
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
                // Leave raw mode before emitting a cooked newline.
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
    // Single bottom focus row: rewrite the current line in place with CR, never
    // emit bare LF under raw mode (that would staircase subsequent output).
    let safe = crate::commands::display_safe(label);
    let prefix = format!("› {}) ", index + 1);
    let available = terminal_columns.saturating_sub(2).max(1);
    let content = truncate_to_width(&format!("{prefix}{safe}"), available);
    print!("\r\x1b[2K  {}", paint(&content, FOCUS));
    std::io::stdout().flush().ok();
}

fn print_help(help: &str, terminal_columns: usize) {
    // Help is printed under raw mode; use CRLF so wrapped lines stay left-aligned.
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

    #[test]
    fn picker_frame_keeps_selection_marker_left_aligned() {
        let options = vec![
            "Anthropic            Auto (legacy)            claude-sonnet-4-20250514".into(),
            "OpenAI               Auto (legacy)            gpt-5.6-luna".into(),
        ];
        let matches = picker_matches(&options, "");
        assert_eq!(matches, vec![0, 1]);
        let lines = picker_frame_lines(&options, &matches, "", 1);
        assert_eq!(lines[0], "  filter: ");
        assert_eq!(
            lines[1],
            "    Anthropic            Auto (legacy)            claude-sonnet-4-20250514"
        );
        assert_eq!(
            lines[2],
            "  > OpenAI               Auto (legacy)            gpt-5.6-luna"
        );
        // Marker column is fixed so raw-mode CRLF redraw cannot look "selected mid-row".
        assert!(lines[1].starts_with("    "));
        assert!(lines[2].starts_with("  > "));
        assert_eq!(lines[1].find("Anthropic"), Some(4));
        assert_eq!(lines[2].find("OpenAI"), Some(4));
    }

    #[test]
    fn picker_filter_narrows_matches_and_handles_empty() {
        let options = vec![
            "Anthropic · claude".into(),
            "OpenAI · gpt".into(),
            "xAI · grok".into(),
        ];
        assert_eq!(picker_matches(&options, "open"), vec![1]);
        assert_eq!(picker_matches(&options, "nope"), Vec::<usize>::new());
        let empty = picker_frame_lines(&options, &[], "nope", 0);
        assert_eq!(empty[0], "  filter: nope");
        assert_eq!(empty[1], "    no matches");
    }

    #[test]
    fn arrow_csi_and_ss3_sequences_are_not_treated_as_cancel() {
        // Complete multi-byte sequences must never look like bare Esc cancel.
        // (Buffered-stdin + poll(STDIN) used to do exactly that.)
        assert_eq!(
            PickerInput::from_bytes(b"\x1b[A").read_key().unwrap(),
            PickerKey::Up
        );
        assert_eq!(
            PickerInput::from_bytes(b"\x1b[B").read_key().unwrap(),
            PickerKey::Down
        );
        assert_eq!(
            PickerInput::from_bytes(b"\x1bOA").read_key().unwrap(),
            PickerKey::Up
        );
        assert_eq!(
            PickerInput::from_bytes(b"\x1bOB").read_key().unwrap(),
            PickerKey::Down
        );
        // Modified CSI (e.g. from some terminals): ESC [ 1 ; 5 A
        assert_eq!(
            PickerInput::from_bytes(b"\x1b[1;5A").read_key().unwrap(),
            PickerKey::Up
        );
        assert_eq!(
            PickerInput::from_bytes(b"j").read_key().unwrap(),
            PickerKey::Character('j')
        );
        assert_eq!(
            PickerInput::from_bytes(b"\r").read_key().unwrap(),
            PickerKey::Enter
        );
    }
}
