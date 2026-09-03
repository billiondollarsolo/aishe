//! Small terminal UI primitives used by setup, settings, and the tour. This is
//! deliberately not a full-screen application: it remains readable over SSH,
//! supports arrow keys and simple letter shortcuts, and restores terminal mode
//! on every exit path.

use std::io::{BufRead, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;

use anyhow::{Context, Result};

#[cfg(test)]
use crate::ui::CapabilityInputs;
use crate::ui::{Motion, StyleToken, TerminalCapabilities};

/// Compatibility façade for setup/settings callers while reusable view models
/// and pure renderers live under `ui`.
pub use crate::ui::render::{ApprovalView, ProposalView, SafetyCue};

const ACCENT: StyleToken = StyleToken::Accent;
const MUTED: StyleToken = StyleToken::Muted;
const FOCUS: StyleToken = StyleToken::Focus;
const WARNING: StyleToken = StyleToken::Warning;

/// Monochrome terminal mark: glasses silhouette + wordmark.
/// Uses Unicode half-blocks (`█▀▄`) so the silhouette tracks the real logo;
/// still plain text (no images/emoji fonts). Branding: **AIShe** = **AI Shell**
/// (CLI package name remains `aishe`).
pub const ASCII_LOGO: &str = r#"   ▄▄▄        ▄▄▄
  ████        ████
   ▀██        ▄█▀
    ██▄      ▄██
 ▄████████████████▄
 ██      ██      ██
 ██      ██      ██
  ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
        AIShe
       AI Shell"#;

/// Brand fallback for ASCII-only, dumb, or assistive terminal policies.
pub const PLAIN_LOGO: &str = "AIShe\nAI Shell";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuResult {
    Selected(usize),
    Back,
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerResult {
    Use(usize),
    // Retained for compatibility with callers while picker default promotion
    // is consolidated into the post-Enter confirmation flow. The picker no
    // longer produces this result directly.
    SaveDefault(usize),
    Cancel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PickerKey {
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Enter,
    Backspace,
    Cancel,
    Character(char),
    Other,
}

const PICKER_MAX_VISIBLE_ROWS: usize = 20;
const PICKER_FRAME_OVERHEAD_ROWS: usize = 4;
const PICKER_HELP: &str = "type to search · ↑/↓ move · Enter select · Esc close";
const PICKER_HELP_ASCII: &str = "type to search | Up/Down move | Enter select | Esc close";

/// One footer for every prompt. Menus, pickers, text prompts, and the hidden
/// secret prompt used four different vocabularies, and the menu footer ignored
/// the ASCII glyph policy.
pub fn prompt_footer(capabilities: &TerminalCapabilities, back: bool) -> String {
    let ascii = capabilities.glyphs().focus() == ">";
    let (up, sep) = if ascii {
        ("Up/Down", " | ")
    } else {
        ("↑/↓", " · ")
    };
    let mut parts = vec![format!("{up} or number"), "Enter accept".to_string()];
    if back {
        parts.push("b back".into());
    }
    parts.push("? help".into());
    parts.push("Esc cancel".into());
    parts.join(sep)
}

/// The one word every cancelled prompt prints.
pub const CANCELLED: &str = "cancelled";

/// Filterable single-column picker shared by `/model` and `/connection`. It
/// intentionally uses plain text and ASCII focus marks so terminal font/theme
/// choices never turn status into colored pictographs.
///
/// The interactive body runs in raw mode. Every redraw must use `\r\n` (not
/// bare `\n`) and move the cursor back to the top of the previous frame;
/// otherwise multi-row lists staircase to the right on each line.
pub fn filter_picker(title: &str, options: &[String], default: usize) -> Result<PickerResult> {
    if options.is_empty() {
        anyhow::bail!("picker '{title}' has no options");
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "interactive picker requires a terminal; pass the model or connection name directly"
        );
    }
    // Title/help print in cooked mode so their newlines behave normally.
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
    let capabilities = TerminalCapabilities::detect_stdout();
    let help = if capabilities.glyphs().focus() == ">" {
        PICKER_HELP_ASCII
    } else {
        PICKER_HELP
    };
    println!("  {}", capabilities.paint(MUTED, help));
    if capabilities.motion == Motion::Static {
        return static_filter_picker(options, default);
    }
    // Read keys from an unbuffered stdin fd (not StdinLock). A buffered stdin
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
        let visible_rows = picker_visible_rows();
        let lines = picker_frame_lines(options, &matches, &filter, selected, visible_rows);
        draw_raw_frame(&lines, &mut drawn_rows, &capabilities);
        std::io::stdout().flush().ok();
        match keys.read_key().context("reading picker input")? {
            key @ (PickerKey::Up
            | PickerKey::Down
            | PickerKey::Home
            | PickerKey::End
            | PickerKey::PageUp
            | PickerKey::PageDown) => {
                selected = move_picker_selection(selected, matches.len(), key, visible_rows);
            }
            PickerKey::Enter if !matches.is_empty() => {
                drop(guard);
                println!();
                return Ok(PickerResult::Use(matches[selected]));
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

fn static_menu(
    options: &[String],
    default: usize,
    allow_back: bool,
    help: &str,
) -> Result<MenuResult> {
    let terminal_columns = columns();
    for (index, option) in options.iter().enumerate() {
        print_option(index, option, terminal_columns);
    }
    println!(
        "  Enter accepts {} | number selects{} | ? help | :cancel",
        default.min(options.len() - 1) + 1,
        if allow_back { " | b back" } else { "" }
    );
    loop {
        print!("  > ");
        std::io::stdout().flush().ok();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input)? == 0 {
            return Ok(MenuResult::Cancel);
        }
        let input = input.trim();
        if input.is_empty() {
            return Ok(MenuResult::Selected(default.min(options.len() - 1)));
        }
        if input == "?" {
            print_help_static(help, terminal_columns);
            continue;
        }
        if allow_back && input.eq_ignore_ascii_case("b") {
            return Ok(MenuResult::Back);
        }
        if matches!(input, ":cancel" | "q" | "Q") {
            return Ok(MenuResult::Cancel);
        }
        if let Ok(number) = input.parse::<usize>() {
            if (1..=options.len()).contains(&number) {
                return Ok(MenuResult::Selected(number - 1));
            }
        }
        println!("  ! enter a displayed number");
    }
}

fn print_help_static(help: &str, terminal_columns: usize) {
    let safe = crate::commands::display_safe(help);
    let available = terminal_columns.saturating_sub(2).max(1);
    for line in wrap_text(&safe, available) {
        println!("  {}", paint(&line, MUTED));
    }
}

fn static_filter_picker(options: &[String], default: usize) -> Result<PickerResult> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    static_filter_picker_io(options, default, &mut stdin.lock(), &mut stdout.lock())
}

/// Stdio implementation kept separate from TTY admission so the complete
/// static interaction can be exercised deterministically without raw mode.
/// This is also the screen-reader/uncertain-terminal contract: durable rows,
/// line input, safe EOF/cancel, and no cursor restoration sequences.
fn static_filter_picker_io(
    options: &[String],
    default: usize,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<PickerResult> {
    let mut filter = String::new();
    let mut page = 0_usize;
    loop {
        let matches = picker_matches(options, &filter);
        let page_count = matches.len().max(1).div_ceil(PICKER_MAX_VISIBLE_ROWS);
        page = page.min(page_count.saturating_sub(1));
        let start = page * PICKER_MAX_VISIBLE_ROWS;
        let end = (start + PICKER_MAX_VISIBLE_ROWS).min(matches.len());
        writeln!(
            output,
            "  filter: {}",
            crate::commands::display_safe(&filter)
        )?;
        if matches.is_empty() {
            writeln!(output, "  0 matches")?;
        } else {
            writeln!(
                output,
                "  {} match{} | page {}/{} | showing {}-{}",
                matches.len(),
                if matches.len() == 1 { "" } else { "es" },
                page + 1,
                page_count,
                start + 1,
                end
            )?;
            for (visible, index) in matches[start..end].iter().enumerate() {
                writeln!(
                    output,
                    "    {}) {}",
                    visible + 1,
                    crate::commands::display_safe(&options[*index])
                )?;
            }
        }
        writeln!(
            output,
            "  Enter uses current default/first | number selects | text filters | :next | :prev | :cancel"
        )?;
        write!(output, "  > ")?;
        output.flush()?;
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return Ok(PickerResult::Cancel);
        }
        let response = line.trim();
        match response {
            ":cancel" | ":back" => return Ok(PickerResult::Cancel),
            ":next" if page + 1 < page_count => page += 1,
            ":prev" if page > 0 => page -= 1,
            "" if !matches.is_empty() => {
                let original_default = default.min(options.len() - 1);
                let selected = matches
                    .iter()
                    .position(|index| *index == original_default)
                    .filter(|position| (start..end).contains(position))
                    .unwrap_or(start.min(matches.len() - 1));
                return Ok(PickerResult::Use(matches[selected]));
            }
            value => {
                if let Ok(number) = value.parse::<usize>() {
                    if (1..=end.saturating_sub(start)).contains(&number) {
                        return Ok(PickerResult::Use(matches[start + number - 1]));
                    }
                    writeln!(output, "  ! choose a displayed number")?;
                } else {
                    filter = value.to_string();
                    page = 0;
                }
            }
        }
    }
}

fn picker_matches(options: &[String], filter: &str) -> Vec<usize> {
    let needle = filter.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return (0..options.len()).collect();
    }
    let mut matches = options
        .iter()
        .enumerate()
        .filter_map(|(index, option)| {
            picker_match_rank(&option.to_ascii_lowercase(), &needle).map(|rank| (rank, index))
        })
        .collect::<Vec<_>>();
    // Stable provider order is the tie breaker, so filtering never changes the
    // identity represented by a row and equal-ranked results do not jump.
    matches.sort_by_key(|(rank, index)| (*rank, *index));
    matches.into_iter().map(|(_, index)| index).collect()
}

fn picker_match_rank(haystack: &str, needle: &str) -> Option<u8> {
    if haystack.starts_with(needle) {
        return Some(0);
    }
    if haystack
        .split(|character: char| !character.is_alphanumeric())
        .any(|token| token.starts_with(needle))
    {
        return Some(1);
    }
    if haystack.contains(needle) {
        return Some(2);
    }
    fuzzy_subsequence(haystack, needle).then_some(3)
}

fn fuzzy_subsequence(haystack: &str, needle: &str) -> bool {
    let mut wanted = needle.chars();
    let Some(mut next) = wanted.next() else {
        return true;
    };
    for character in haystack.chars() {
        if character == next {
            let Some(character) = wanted.next() else {
                return true;
            };
            next = character;
        }
    }
    false
}

fn picker_visible_rows() -> usize {
    let terminal_rows = crossterm::terminal::size()
        .ok()
        .map(|(_, rows)| usize::from(rows))
        .unwrap_or(PICKER_MAX_VISIBLE_ROWS + PICKER_FRAME_OVERHEAD_ROWS);
    picker_visible_rows_for_terminal(terminal_rows)
}

fn picker_visible_rows_for_terminal(terminal_rows: usize) -> usize {
    terminal_rows
        .saturating_sub(PICKER_FRAME_OVERHEAD_ROWS)
        .clamp(1, PICKER_MAX_VISIBLE_ROWS)
}

fn picker_viewport(match_count: usize, selected: usize, visible_rows: usize) -> (usize, usize) {
    if match_count == 0 {
        return (0, 0);
    }
    let visible_rows = visible_rows.max(1).min(match_count);
    let selected = selected.min(match_count - 1);
    let start = selected
        .saturating_sub(visible_rows / 2)
        .min(match_count - visible_rows);
    (start, start + visible_rows)
}

fn move_picker_selection(
    selected: usize,
    match_count: usize,
    key: PickerKey,
    page_size: usize,
) -> usize {
    if match_count == 0 {
        return 0;
    }
    let selected = selected.min(match_count - 1);
    let page_size = page_size.max(1);
    match key {
        PickerKey::Up => selected.checked_sub(1).unwrap_or(match_count - 1),
        PickerKey::Down => (selected + 1) % match_count,
        PickerKey::Home => 0,
        PickerKey::End => match_count - 1,
        PickerKey::PageUp => selected.saturating_sub(page_size),
        PickerKey::PageDown => selected.saturating_add(page_size).min(match_count - 1),
        _ => selected,
    }
}

/// Build the filter, position summary, and selection-following option viewport
/// for one picker frame.
fn picker_frame_lines(
    options: &[String],
    matches: &[usize],
    filter: &str,
    selected: usize,
    visible_rows: usize,
) -> Vec<String> {
    let mut lines = vec![format!(
        "  search: {}",
        crate::commands::display_safe(filter)
    )];
    if matches.is_empty() {
        lines.push("  no matches".into());
        return lines;
    }

    let selected = selected.min(matches.len() - 1);
    let (start, end) = picker_viewport(matches.len(), selected, visible_rows);
    lines.push(format!("  {} of {}", selected + 1, matches.len()));
    for (match_position, index) in matches.iter().enumerate().take(end).skip(start) {
        lines.push(crate::ui::render::picker_row(
            &crate::commands::display_safe(&options[*index]),
            match_position == selected,
            usize::MAX,
        ));
    }
    lines
}

/// Exercise the exact picker ranking implementation without entering raw mode.
///
/// This is intentionally a narrow, hidden surface for the repository's
/// performance probe. Keeping the probe on the production implementation avoids
/// a synthetic benchmark drifting away from picker behavior.
#[doc(hidden)]
pub fn performance_picker_matches(options: &[String], filter: &str) -> Vec<usize> {
    picker_matches(options, filter)
}

/// Exercise the exact pure frame construction used before a picker redraw.
/// Terminal writes are deliberately excluded so host/PTY latency can be tracked
/// separately from ranking and layout work.
#[doc(hidden)]
pub fn performance_picker_frame(
    options: &[String],
    matches: &[usize],
    selected: usize,
    visible_rows: usize,
) -> Vec<String> {
    picker_frame_lines(options, matches, "", selected, visible_rows)
}

/// Redraw a multi-line frame under raw mode without staircasing columns.
///
/// `drawn_rows` is the number of content lines written on the previous frame.
/// After each frame the cursor sits on the blank line immediately below the
/// last content row, so the next redraw moves up exactly `drawn_rows` lines.
fn draw_raw_frame(lines: &[String], drawn_rows: &mut usize, capabilities: &TerminalCapabilities) {
    let width = columns().max(1);
    if *drawn_rows > 0 {
        // Cursor is on the blank line under the previous frame.
        print!("\r\x1b[{}A", *drawn_rows);
    }
    for (index, line) in lines.iter().enumerate() {
        let content = truncate_to_width(line, width);
        let content = style_picker_line(&content, index, capabilities);
        // Clear the full row, write content, then CRLF so the next row starts
        // at column 0 even when the terminal is in raw mode.
        print!("\r\x1b[2K{content}\r\n");
    }
    // Drop any leftover rows from a taller previous frame.
    print!("\x1b[J");
    *drawn_rows = lines.len();
}

fn style_picker_line(line: &str, index: usize, capabilities: &TerminalCapabilities) -> String {
    if let Some(query) = line.strip_prefix("  search: ") {
        return format!(
            "  {} {}",
            capabilities.paint(MUTED, "search:"),
            capabilities.paint(StyleToken::ProposedCommand, query)
        );
    }
    if index == 1 {
        return capabilities.paint(MUTED, line);
    }
    if line.starts_with("  > ") {
        return capabilities.paint(FOCUS, line);
    }
    if let Some((command, summary)) = line.split_once(" — ") {
        return format!(
            "{}{}{}",
            capabilities.paint(StyleToken::ProposedCommand, command),
            capabilities.paint(MUTED, " — "),
            capabilities.paint(MUTED, summary)
        );
    }
    line.to_string()
}

/// Unbuffered terminal input for the filter picker.
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
            // The inherited fd is essential under the zsh-PTY front-end:
            // /dev/tty names the outer proxy terminal, while stdin names the
            // inner zsh PTY that must own the complete key sequence.
            let file = std::fs::OpenOptions::new()
                .read(true)
                .open("/dev/stdin")
                .context("opening terminal stdin for interactive keys")?;
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
        match self.read_byte() {
            Ok(byte) => Ok(Some(byte)),
            // A descriptor can report readable and then return EOF. Linux does
            // this for /dev/null (used by the deterministic byte fixture), and
            // a detached terminal can do the same during shutdown. Treat that
            // exactly like "no continuation byte": bare Esc cancels safely.
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(error) => Err(error),
        }
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
            14 => PickerKey::Down,
            16 => PickerKey::Up,
            8 | 127 => PickerKey::Backspace,
            27 => self.read_escape_sequence()?,
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

    /// Parse navigation keys encoded as ESC + CSI or SS3 sequences.
    fn read_escape_sequence(&mut self) -> std::io::Result<PickerKey> {
        // Prefer bytes already available; only then wait (SSH lag).
        let Some(second) = self.poll_byte(300)? else {
            return Ok(PickerKey::Cancel);
        };
        match second {
            // SS3 cursor keys (application cursor mode): ESC O A/B/H/F
            b'O' => {
                let Some(third) = self.poll_byte(150)? else {
                    return Ok(PickerKey::Other);
                };
                return Ok(match third {
                    b'A' => PickerKey::Up,
                    b'B' => PickerKey::Down,
                    b'H' => PickerKey::Home,
                    b'F' => PickerKey::End,
                    _ => PickerKey::Other,
                });
            }
            // CSI: ESC [ … final
            b'[' => {}
            _ => return Ok(PickerKey::Other),
        }

        // Read CSI parameter/intermediate bytes until a final byte (0x40–0x7E).
        let mut parameters = Vec::new();
        let mut final_byte = None;
        for _ in 0..16 {
            let Some(byte) = self.poll_byte(150)? else {
                break;
            };
            if (0x40..=0x7e).contains(&byte) {
                final_byte = Some(byte);
                break;
            }
            parameters.push(byte);
        }
        Ok(match final_byte {
            Some(b'A') => PickerKey::Up,
            Some(b'B') => PickerKey::Down,
            Some(b'H') => PickerKey::Home,
            Some(b'F') => PickerKey::End,
            Some(b'~') => match parameters.split(|byte| *byte == b';').next() {
                Some(b"1" | b"7") => PickerKey::Home,
                Some(b"4" | b"8") => PickerKey::End,
                Some(b"5") => PickerKey::PageUp,
                Some(b"6") => PickerKey::PageDown,
                _ => PickerKey::Other,
            },
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

#[cfg(test)]
fn color_enabled_for(is_terminal: bool, no_color: bool, term: Option<&str>) -> bool {
    TerminalCapabilities::resolve(&CapabilityInputs {
        is_tty: is_terminal,
        no_color,
        term: term.map(str::to_string),
        locale: Some("en_US.UTF-8".into()),
        ..CapabilityInputs::default()
    })
    .styled()
}

fn paint(text: &str, style: StyleToken) -> String {
    TerminalCapabilities::detect_stdout().paint(style, text)
}

fn columns() -> usize {
    crossterm::terminal::size()
        .ok()
        .map(|(columns, _)| usize::from(columns))
        .filter(|columns| *columns > 0)
        .unwrap_or(80)
}

fn display_width(value: &str) -> usize {
    crate::ui::cell_width(value)
}

fn truncate_to_width(value: &str, width: usize) -> String {
    crate::ui::truncate_cells(value, width)
}

fn wrap_text(value: &str, width: usize) -> Vec<String> {
    crate::ui::wrap_cells(value, width)
}

fn print_wrapped(indent: &str, value: &str, style: Option<StyleToken>) {
    let safe = crate::commands::display_safe(value);
    let available = columns().saturating_sub(display_width(indent)).max(1);
    for line in wrap_text(&safe, available) {
        let line = style.map_or(line.clone(), |token| paint(&line, token));
        println!("{indent}{line}");
    }
}

/// Print a compact, width-aware header shared by the interactive setup-like
/// experiences. Styling is deliberately ANSI-only and opt-out via NO_COLOR so
/// it remains readable over SSH and in basic terminals.
pub fn header(title: &str, description: &str, note: &str) {
    let capabilities = TerminalCapabilities::detect_stdout();
    let safe_title = crate::commands::display_safe(title);
    println!("\n  {}", paint(&safe_title, ACCENT));
    let rule = if capabilities.glyphs().focus() == ">" {
        "-"
    } else {
        "─"
    };
    println!(
        "  {}",
        paint(&rule.repeat(display_width(&safe_title)), ACCENT)
    );
    print_wrapped("  ", description, None);
    print_wrapped("  ", note, Some(MUTED));
}

pub fn brand() {
    let capabilities = TerminalCapabilities::detect_stdout();
    let logo = if capabilities.glyphs().focus() == ">" {
        PLAIN_LOGO
    } else {
        ASCII_LOGO
    };
    println!("\n{logo}");
}

pub fn section(title: &str) {
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
}

pub fn success(message: &str) {
    print_notice(crate::ui::render::NoticeKind::Success, message);
}

pub fn warning(message: &str) {
    print_notice(crate::ui::render::NoticeKind::Warning, message);
}

/// A step that did not run because it does not apply here.
pub fn skipped(message: &str) {
    print_notice(crate::ui::render::NoticeKind::Skipped, message);
}

pub fn error(message: &str) {
    print_notice(crate::ui::render::NoticeKind::Error, message);
}

fn print_notice(kind: crate::ui::render::NoticeKind, message: &str) {
    let capabilities = TerminalCapabilities::detect_stdout();
    let available = columns().saturating_sub(2).max(1);
    let safe = crate::commands::display_safe(message);
    for line in crate::ui::render::notice_lines(kind, &safe, &capabilities, available) {
        println!("  {line}");
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
    println!(
        "\n  {}",
        paint(&crate::commands::display_safe(title), ACCENT)
    );
    if TerminalCapabilities::detect_stdout().motion == Motion::Static {
        return static_menu(options, default, allow_back, help);
    }
    let terminal_columns = columns();
    for (index, option) in options.iter().enumerate() {
        print_option(index, option, terminal_columns);
    }
    let instructions = prompt_footer(&TerminalCapabilities::detect_stdout(), allow_back);
    print_wrapped("  ", &instructions, Some(MUTED));
    let selected = default.min(options.len() - 1);
    // Read keys through the picker's unbuffered stdin reader. Under the
    // zsh-PTY front-end /dev/tty is the *outer* proxy terminal, so a
    // crossterm reader cannot initialize and `/settings`, `aishe setup`, and
    // `aishe tour` died after painting their first screen.
    let mut keys = PickerInput::open().context("opening menu input")?;
    let guard = RawGuard::enter()?;
    print_selection(selected, &options[selected], terminal_columns);
    let result = menu_select(
        &mut keys,
        options,
        selected,
        allow_back,
        help,
        terminal_columns,
    );
    // Leave raw mode before the cooked newline; unwinding also drops the guard.
    drop(guard);
    println!();
    result
}

fn menu_select(
    keys: &mut PickerInput,
    options: &[String],
    mut selected: usize,
    allow_back: bool,
    help: &str,
    terminal_columns: usize,
) -> Result<MenuResult> {
    let mut number_buffer = String::new();
    loop {
        let key = match keys.read_key() {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(MenuResult::Cancel)
            }
            Err(error) => return Err(error).context("reading menu input"),
        };
        match key {
            PickerKey::Up | PickerKey::Character('k') => {
                number_buffer.clear();
                selected = selected.checked_sub(1).unwrap_or(options.len() - 1);
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Down | PickerKey::Character('j') => {
                number_buffer.clear();
                selected = (selected + 1) % options.len();
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Enter => return Ok(MenuResult::Selected(selected)),
            PickerKey::Character(c) if c.is_ascii_digit() => {
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
            PickerKey::Character('b' | 'B') if allow_back => return Ok(MenuResult::Back),
            PickerKey::Character('?') => {
                number_buffer.clear();
                print_help(help, terminal_columns);
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Cancel | PickerKey::Character('q' | 'Q') => return Ok(MenuResult::Cancel),
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
    let available = terminal_columns.saturating_sub(2).max(1);
    let capabilities = TerminalCapabilities::detect_stdout();
    let content = crate::ui::render::focus_row(index, &safe, &capabilities, available);
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
        let focus = TerminalCapabilities::detect_stdout().glyphs().focus();
        print!("  {} ", paint(focus, ACCENT));
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
    let mut keys = PickerInput::open().context("opening secret input")?;
    let guard = RawGuard::enter()?;
    let result = read_secret(&mut keys, max_bytes);
    drop(guard);
    println!();
    result
}

fn read_secret(keys: &mut PickerInput, max_bytes: usize) -> Result<Option<String>> {
    let mut value = String::new();
    loop {
        let key = match keys.read_key() {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error).context("reading hidden input"),
        };
        match key {
            PickerKey::Enter => return Ok(Some(value)),
            PickerKey::Backspace => {
                value.pop();
            }
            PickerKey::Cancel | PickerKey::Character('\u{4}') => return Ok(None),
            PickerKey::Character(character)
                if !character.is_control() && value.len() + character.len_utf8() <= max_bytes =>
            {
                value.push(character);
            }
            _ => {}
        }
    }
}

pub fn confirm(label: &str, default: bool) -> Result<Option<bool>> {
    let suffix = crate::ui::render::approval_suffix(default);
    loop {
        print!(
            "  {} {} ",
            paint(&crate::commands::display_safe(label), ACCENT),
            paint(&format!("{suffix}:"), MUTED)
        );
        std::io::stdout().flush().ok();
        let Some(line) = read_terminal_line(true)? else {
            return Ok(None);
        };
        match parse_confirmation(&line, default) {
            ConfirmationResponse::Answer(answer) => return Ok(Some(answer)),
            ConfirmationResponse::Cancel => return Ok(None),
            ConfirmationResponse::Invalid => {
                println!("  {}", paint("! enter y, n, or cancel", WARNING));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfirmationResponse {
    Answer(bool),
    Cancel,
    Invalid,
}

fn parse_confirmation(line: &str, default: bool) -> ConfirmationResponse {
    match line.trim().to_ascii_lowercase().as_str() {
        "" => ConfirmationResponse::Answer(default),
        "y" | "yes" => ConfirmationResponse::Answer(true),
        "n" | "no" => ConfirmationResponse::Answer(false),
        "q" | "cancel" => ConfirmationResponse::Cancel,
        _ => ConfirmationResponse::Invalid,
    }
}

/// Read a confirmation response from the controlling terminal when one is
/// available. A picker immediately followed by a cooked `stdin().read_line()`
/// can lose the first response byte inside a wrapped/nested PTY while terminal
/// mode is being restored. Reusing the picker's unbuffered stdin input
/// keeps the transition deterministic. Piped/non-terminal setup input retains
/// the existing line-oriented contract.
pub fn read_terminal_line(echo: bool) -> Result<Option<String>> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        let mut line = String::new();
        return if std::io::stdin().read_line(&mut line)? == 0 {
            Ok(None)
        } else {
            Ok(Some(line))
        };
    }

    let mut keys = PickerInput::open().context("opening confirmation input")?;
    let guard = RawGuard::enter()?;
    let result = read_terminal_confirmation_line(&mut keys, echo);
    // Drop explicitly before the newline so even the normal completion path
    // restores canonical mode first. Error unwinding also drops the guard.
    drop(guard);
    println!();
    result
}

fn read_terminal_confirmation_line(keys: &mut PickerInput, echo: bool) -> Result<Option<String>> {
    let mut line = String::new();
    loop {
        let key = match keys.read_key() {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error).context("reading confirmation input"),
        };
        match key {
            PickerKey::Enter => return Ok(Some(line)),
            PickerKey::Cancel | PickerKey::Character('\u{4}') => return Ok(None),
            PickerKey::Backspace if !line.is_empty() => {
                line.pop();
                if echo {
                    print!("\x08 \x08");
                    std::io::stdout().flush().ok();
                }
            }
            PickerKey::Character(character)
                if !character.is_control() && line.len() + character.len_utf8() <= 16 =>
            {
                line.push(character);
                if echo {
                    print!("{character}");
                    std::io::stdout().flush().ok();
                }
            }
            _ => {}
        }
    }
}

/// Policy for post-picker “make this the default?”: default answer is **No**.
pub const PROMOTE_DEFAULT_CONFIRM_DEFAULT: bool = false;

/// Offer the promote-to-default prompt only when the pick is still shell-local
/// and differs from durable configuration.
pub fn should_offer_promote_to_default(
    already_save_default: bool,
    differs_from_durable: bool,
) -> bool {
    !already_save_default && differs_from_durable
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_footer_respects_the_glyph_policy() {
        let unicode = TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            locale: Some("en_US.UTF-8".into()),
            ..CapabilityInputs::default()
        });
        assert_eq!(
            prompt_footer(&unicode, true),
            "↑/↓ or number · Enter accept · b back · ? help · Esc cancel"
        );
        let ascii = TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            locale: Some("C".into()),
            ..CapabilityInputs::default()
        });
        assert_eq!(
            prompt_footer(&ascii, false),
            "Up/Down or number | Enter accept | ? help | Esc cancel"
        );
    }

    #[test]
    fn menu_select_reads_arrows_digits_back_and_escape() {
        let options: Vec<String> = ["one", "two", "three"].map(String::from).to_vec();
        let mut down_enter = PickerInput::from_bytes(b"\x1b[B\r");
        assert_eq!(
            menu_select(&mut down_enter, &options, 0, true, "", 80).unwrap(),
            MenuResult::Selected(1)
        );
        let mut digit = PickerInput::from_bytes(b"3\r");
        assert_eq!(
            menu_select(&mut digit, &options, 0, true, "", 80).unwrap(),
            MenuResult::Selected(2)
        );
        let mut back = PickerInput::from_bytes(b"b");
        assert_eq!(
            menu_select(&mut back, &options, 0, true, "", 80).unwrap(),
            MenuResult::Back
        );
        // Bare Esc (no continuation byte) cancels instead of crashing.
        let mut escape = PickerInput::from_bytes(b"\x1b");
        assert_eq!(
            menu_select(&mut escape, &options, 0, false, "", 80).unwrap(),
            MenuResult::Cancel
        );
    }

    #[test]
    fn read_secret_handles_backspace_cancel_and_the_byte_cap() {
        let mut typed = PickerInput::from_bytes(b"ab\x7fc\r");
        assert_eq!(read_secret(&mut typed, 64).unwrap().as_deref(), Some("ac"));
        let mut cancelled = PickerInput::from_bytes(b"secret\x03");
        assert_eq!(read_secret(&mut cancelled, 64).unwrap(), None);
        let mut bounded = PickerInput::from_bytes(b"abcdef\r");
        assert_eq!(
            read_secret(&mut bounded, 3).unwrap().as_deref(),
            Some("abc")
        );
    }

    #[test]
    fn promote_default_policy_is_no_and_gate_works() {
        // Policy constant must stay default-No ([y/N]) for shell-local Enter.
        const {
            assert!(!PROMOTE_DEFAULT_CONFIRM_DEFAULT);
        }
        assert_eq!(
            parse_confirmation("", PROMOTE_DEFAULT_CONFIRM_DEFAULT),
            ConfirmationResponse::Answer(false)
        );
        assert!(!should_offer_promote_to_default(true, true));
        assert!(!should_offer_promote_to_default(false, false));
        assert!(should_offer_promote_to_default(false, true));
    }

    #[test]
    fn picker_to_confirmation_keeps_following_tty_bytes() {
        // One controlling-terminal stream can contain the picker's Enter and
        // the immediately following confirmation response. Consuming the
        // picker key must not strand or discard the response bytes.
        let mut input = PickerInput::from_bytes(b"\rn\r");
        assert_eq!(input.read_key().unwrap(), PickerKey::Enter);
        let line = read_terminal_confirmation_line(&mut input, false).unwrap();
        assert_eq!(line.as_deref(), Some("n"));
        assert_eq!(
            parse_confirmation(line.as_deref().unwrap(), false),
            ConfirmationResponse::Answer(false)
        );
    }

    #[test]
    fn confirmation_eof_ctrl_c_and_escape_never_promote_default() {
        for bytes in [&b""[..], &b"\x03"[..], &b"\x1b"[..]] {
            let mut input = PickerInput::from_bytes(bytes);
            assert_eq!(
                read_terminal_confirmation_line(&mut input, false).unwrap(),
                None
            );
        }
        assert_eq!(
            parse_confirmation("", false),
            ConfirmationResponse::Answer(false)
        );
        assert_eq!(
            parse_confirmation("cancel", false),
            ConfirmationResponse::Cancel
        );
    }

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
        let lines = picker_frame_lines(&options, &matches, "", 1, 20);
        assert_eq!(lines[0], "  search: ");
        assert_eq!(lines[1], "  2 of 2");
        assert_eq!(
            lines[2],
            "    Anthropic            Auto (legacy)            claude-sonnet-4-20250514"
        );
        assert_eq!(
            lines[3],
            "  > OpenAI               Auto (legacy)            gpt-5.6-luna"
        );
        // Marker column is fixed so raw-mode CRLF redraw cannot look "selected mid-row".
        assert!(lines[2].starts_with("    "));
        assert!(lines[3].starts_with("  > "));
        assert_eq!(lines[2].find("Anthropic"), Some(4));
        assert_eq!(lines[3].find("OpenAI"), Some(4));
    }

    #[test]
    fn picker_filter_narrows_matches_and_handles_empty() {
        let options = vec![
            "Anthropic · claude".into(),
            "OpenAI · gpt".into(),
            "xAI · grok".into(),
        ];
        assert_eq!(picker_matches(&options, "open"), vec![1]);
        assert_eq!(picker_matches(&options, "zzzz"), Vec::<usize>::new());
        let empty = picker_frame_lines(&options, &[], "zzzz", 0, 20);
        assert_eq!(empty, ["  search: zzzz", "  no matches"]);
    }

    #[test]
    fn picker_ranking_is_deterministic_and_preserves_exact_identities() {
        let options = vec![
            "alpha-gpt · provider/a-model".into(),
            "gpt-pro · provider/gpt-pro".into(),
            "provider · alphaogptmodel".into(),
            "great practical tool · provider/fuzzy".into(),
            "gpt-next · provider/gpt-next".into(),
        ];

        // Whole-row prefix, token prefix, substring, then fuzzy subsequence.
        assert_eq!(picker_matches(&options, "gpt"), vec![1, 4, 0, 2, 3]);
        // Equal-ranked prefix rows keep their original, stable ordering.
        assert_eq!(&picker_matches(&options, "gpt-")[..2], &[1, 4]);
        // The returned index is always the original provider identity, never a
        // synthetic display/ranking index.
        let selected = picker_matches(&options, "gpt-pro")[0];
        assert_eq!(selected, 1);
        assert_eq!(options[selected], "gpt-pro · provider/gpt-pro");
    }

    #[test]
    fn performance_probe_uses_production_picker_paths_at_1000_rows() {
        let options = (0..1000)
            .map(|index| format!("model-{index:04} · provider/model-{index:04}"))
            .collect::<Vec<_>>();
        let matches = performance_picker_matches(&options, "model");
        assert_eq!(matches.len(), 1000);
        let lines = performance_picker_frame(&options, &matches, 999, 20);
        assert_eq!(lines.len(), 22);
        assert!(lines[1].contains("1000 of 1000"));
        assert!(lines.iter().any(|line| line.starts_with("  > model-0999")));
    }

    #[test]
    fn picker_fuzzy_matching_requires_an_ordered_subsequence() {
        assert!(fuzzy_subsequence("provider/gpt-fuzzy", "gpfz"));
        assert!(!fuzzy_subsequence("provider/gpt-fuzzy", "zfgp"));
        assert_eq!(picker_match_rank("provider/gpt-fuzzy", "gpfz"), Some(3));
        assert_eq!(picker_match_rank("provider/gpt-fuzzy", "zfgp"), None);
    }

    fn numbered_options(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("option-{index:04}"))
            .collect()
    }

    fn run_static_picker(
        options: &[String],
        default: usize,
        script: &str,
    ) -> (PickerResult, String) {
        let mut input = std::io::Cursor::new(script.as_bytes());
        let mut output = Vec::new();
        let result = static_filter_picker_io(options, default, &mut input, &mut output).unwrap();
        (result, String::from_utf8(output).unwrap())
    }

    #[test]
    fn static_picker_stdio_pages_filters_selects_and_cancels_without_cursor_control() {
        let options = numbered_options(45);

        let (paged, transcript) = run_static_picker(&options, 0, ":next\n2\n");
        assert_eq!(paged, PickerResult::Use(21));
        assert!(transcript.contains("page 2/3 | showing 21-40"));
        assert!(!transcript.contains('\u{1b}'));

        let (filtered, transcript) = run_static_picker(&options, 0, "option-0033\n1\n");
        assert_eq!(filtered, PickerResult::Use(33));
        assert!(transcript.contains("1 match | page 1/1"));

        // A default outside the visible page must never be accepted invisibly;
        // bare Enter chooses the first row the static user can actually see.
        assert_eq!(
            run_static_picker(&options, 25, "\n").0,
            PickerResult::Use(0)
        );
        assert_eq!(
            run_static_picker(&options, 0, ":cancel\n").0,
            PickerResult::Cancel
        );
        assert_eq!(run_static_picker(&options, 0, "").0, PickerResult::Cancel);
    }

    #[test]
    fn picker_viewport_keeps_selection_visible_at_required_match_counts() {
        for count in [0, 1, 20, 21, 100, 1_000] {
            let options = numbered_options(count);
            let matches = picker_matches(&options, "");
            let selected = count.saturating_sub(1);
            let lines = picker_frame_lines(&options, &matches, "", selected, 20);

            assert_eq!(lines[0], "  search: ");
            if count == 0 {
                assert_eq!(lines, ["  search: ", "  no matches"]);
                continue;
            }

            let visible_count = count.min(20);
            assert_eq!(lines[1], format!("  {count} of {count}"));
            assert_eq!(lines.len(), visible_count + 2);
            assert_eq!(
                lines
                    .iter()
                    .skip(2)
                    .filter(|line| line.starts_with("  > "))
                    .count(),
                1,
                "exactly one visible option must be selected for {count} matches"
            );
            assert_eq!(
                lines.last().unwrap(),
                &format!("  > option-{:04}", count - 1),
                "the item Enter will use must be visibly marked"
            );
        }
    }

    #[test]
    fn picker_viewport_tracks_middle_selections_and_terminal_budget() {
        assert_eq!(picker_viewport(0, 0, 20), (0, 0));
        assert_eq!(picker_viewport(20, 19, 20), (0, 20));
        assert_eq!(picker_viewport(21, 20, 20), (1, 21));
        assert_eq!(picker_viewport(100, 50, 20), (40, 60));
        assert_eq!(picker_viewport(1_000, 999, 20), (980, 1_000));
        assert_eq!(picker_viewport(100, usize::MAX, 20), (80, 100));

        assert_eq!(picker_visible_rows_for_terminal(0), 1);
        assert_eq!(picker_visible_rows_for_terminal(4), 1);
        assert_eq!(picker_visible_rows_for_terminal(5), 1);
        assert_eq!(picker_visible_rows_for_terminal(12), 8);
        assert_eq!(picker_visible_rows_for_terminal(24), 20);
        assert_eq!(picker_visible_rows_for_terminal(200), 20);
    }

    #[test]
    fn picker_navigation_wraps_and_pages_without_leaving_match_range() {
        assert_eq!(move_picker_selection(0, 0, PickerKey::Down, 20), 0);
        assert_eq!(move_picker_selection(0, 100, PickerKey::Up, 20), 99);
        assert_eq!(move_picker_selection(99, 100, PickerKey::Down, 20), 0);
        assert_eq!(move_picker_selection(52, 100, PickerKey::Home, 20), 0);
        assert_eq!(move_picker_selection(52, 100, PickerKey::End, 20), 99);
        assert_eq!(move_picker_selection(52, 100, PickerKey::PageUp, 20), 32);
        assert_eq!(move_picker_selection(52, 100, PickerKey::PageDown, 20), 72);
        assert_eq!(move_picker_selection(3, 100, PickerKey::PageUp, 20), 0);
        assert_eq!(move_picker_selection(95, 100, PickerKey::PageDown, 20), 99);
        assert_eq!(
            move_picker_selection(usize::MAX, 21, PickerKey::PageDown, 20),
            20
        );
    }

    #[test]
    fn picker_printable_shortcuts_are_filter_characters() {
        for character in ['d', 'j', 'k'] {
            let encoded = [character as u8];
            assert_eq!(
                PickerInput::from_bytes(&encoded).read_key().unwrap(),
                PickerKey::Character(character)
            );
        }
        assert!(!PICKER_HELP.contains("j/k"));
        assert!(!PICKER_HELP.contains("save as default"));
        assert!(PICKER_HELP.contains("type to search"));
    }

    #[test]
    fn picker_navigation_key_sequences_are_parsed() {
        // Complete multi-byte sequences must never look like bare Esc cancel.
        // (Buffered-stdin + poll(STDIN) used to do exactly that.)
        for (encoded, expected) in [
            (&b"\x1b[A"[..], PickerKey::Up),
            (&b"\x1b[B"[..], PickerKey::Down),
            (&b"\x1bOA"[..], PickerKey::Up),
            (&b"\x1bOB"[..], PickerKey::Down),
            (&b"\x1b[1;5A"[..], PickerKey::Up),
            (&b"\x10"[..], PickerKey::Up),
            (&b"\x0e"[..], PickerKey::Down),
            (&b"\x1b[H"[..], PickerKey::Home),
            (&b"\x1bOH"[..], PickerKey::Home),
            (&b"\x1b[1~"[..], PickerKey::Home),
            (&b"\x1b[7~"[..], PickerKey::Home),
            (&b"\x1b[F"[..], PickerKey::End),
            (&b"\x1bOF"[..], PickerKey::End),
            (&b"\x1b[4~"[..], PickerKey::End),
            (&b"\x1b[8~"[..], PickerKey::End),
            (&b"\x1b[5~"[..], PickerKey::PageUp),
            (&b"\x1b[5;2~"[..], PickerKey::PageUp),
            (&b"\x1b[6~"[..], PickerKey::PageDown),
        ] {
            assert_eq!(
                PickerInput::from_bytes(encoded).read_key().unwrap(),
                expected,
                "failed to parse {encoded:?}"
            );
        }
        assert_eq!(
            PickerInput::from_bytes(b"\r").read_key().unwrap(),
            PickerKey::Enter
        );
        assert_eq!(
            PickerInput::from_bytes(b"\x03").read_key().unwrap(),
            PickerKey::Cancel
        );
        assert_eq!(
            PickerInput::from_bytes(b"\x1b").read_key().unwrap(),
            PickerKey::Cancel
        );
        assert!(PickerInput::from_bytes(b"").read_key().is_err());
    }
}
