//! Shared terminal capability, semantic style, and glyph policy.
//!
//! Business logic and view code should ask this module what the active output
//! surface supports instead of reading terminal environment variables or
//! emitting literal ANSI sequences independently.

pub mod render;

use std::ffi::OsString;
use std::fmt;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{OnceLock, RwLock};

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorDepth {
    None,
    Ansi16,
    Ansi256,
    TrueColor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Background {
    Unknown,
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Theme {
    Auto,
    Dark,
    Light,
    Mono,
    None,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnicodePolicy {
    Unicode,
    Ascii,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Motion {
    Live,
    Static,
}

// User-facing names for the presentation policy. `{:?}` leaked "None" and
// "TrueColor" into `aishe doctor` and setup.
impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Auto | Self::None => "auto",
            Self::Dark => "dark",
            Self::Light => "light",
            Self::Mono => "mono",
        })
    }
}

impl std::fmt::Display for ColorDepth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "off",
            Self::Ansi16 => "16-color",
            Self::Ansi256 => "256-color",
            Self::TrueColor => "truecolor",
        })
    }
}

impl std::fmt::Display for UnicodePolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unicode => "unicode",
            Self::Ascii => "ascii",
        })
    }
}

impl std::fmt::Display for Motion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Live => "live",
            Self::Static => "static",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StyleToken {
    Accent,
    Focus,
    UserShell,
    UserAgent,
    AssistantLabel,
    Activity,
    ProposedCommand,
    Success,
    Warning,
    Danger,
    Muted,
    Policy,
    DiffAdd,
    DiffRemove,
    CodeLabel,
}

/// Compatibility styling facade for legacy presentation call sites. Unlike
/// crossterm's literal color methods, these names resolve to semantic tokens
/// and conservatively disable ANSI when either standard output stream is not a
/// terminal. New view code should call [`TerminalCapabilities::paint`]
/// directly with an explicit [`StyleToken`].
pub trait SemanticStylize {
    fn red(&self) -> StyledText;
    fn yellow(&self) -> StyledText;
    fn green(&self) -> StyledText;
    fn cyan(&self) -> StyledText;
    fn white(&self) -> StyledText;
    fn dim(&self) -> StyledText;
    fn bold(&self) -> StyledText;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyledText {
    text: String,
    token: StyleToken,
}

impl StyledText {
    /// Preserve legacy `.color().bold()` chains while the semantic token owns
    /// the actual emphasis for each palette.
    pub fn bold(self) -> Self {
        self
    }
}

impl fmt::Display for StyledText {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut capabilities = TerminalCapabilities::detect_stdout();
        if !TerminalCapabilities::detect_stderr().is_tty {
            capabilities.color_depth = ColorDepth::None;
            capabilities.theme = Theme::None;
        }
        formatter.write_str(&capabilities.paint(self.token, &self.text))
    }
}

impl<T: AsRef<str> + ?Sized> SemanticStylize for T {
    fn red(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Danger)
    }

    fn yellow(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Warning)
    }

    fn green(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Success)
    }

    fn cyan(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Activity)
    }

    fn white(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::ProposedCommand)
    }

    fn dim(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Muted)
    }

    fn bold(&self) -> StyledText {
        styled_text(self.as_ref(), StyleToken::Accent)
    }
}

fn styled_text(text: &str, token: StyleToken) -> StyledText {
    StyledText {
        text: text.to_string(),
        token,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalCapabilities {
    pub is_tty: bool,
    pub term: Option<String>,
    pub color_depth: ColorDepth,
    pub background: Background,
    pub theme: Theme,
    pub unicode: UnicodePolicy,
    pub motion: Motion,
    pub columns: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CapabilityInputs {
    pub is_tty: bool,
    pub term: Option<String>,
    pub no_color: bool,
    pub colorterm: Option<String>,
    pub colorfgbg: Option<String>,
    pub locale: Option<String>,
    pub theme: Option<String>,
    pub color_depth: Option<String>,
    pub unicode: Option<String>,
    pub motion: Option<String>,
    pub size: Option<(u16, u16)>,
}

#[derive(Clone, Debug, Default)]
struct UiPreferences {
    theme: Option<String>,
    color_depth: Option<String>,
    unicode: Option<String>,
    motion: Option<String>,
}

static UI_PREFERENCES: OnceLock<RwLock<UiPreferences>> = OnceLock::new();
static MACHINE_OUTPUT: AtomicBool = AtomicBool::new(false);

/// Mark this process as owning a machine-readable output contract. This is set
/// immediately after CLI parsing, before config migration or diagnostics can
/// emit a human notice. It also makes semantic styling plain as a defense in
/// depth for JSON/JSONL paths.
pub fn set_machine_output(enabled: bool) {
    MACHINE_OUTPUT.store(enabled, Ordering::Relaxed);
}

pub fn machine_output() -> bool {
    MACHINE_OUTPUT.load(Ordering::Relaxed)
}

/// Install the effective user configuration for later terminal interactions.
/// Per-process environment variables have higher precedence and `NO_COLOR`
/// remains an unconditional styling veto.
pub fn configure(config: &crate::config::UiConfig) {
    let preferences = UiPreferences {
        theme: preference(&config.theme),
        color_depth: preference(&config.color_depth),
        unicode: preference(&config.unicode),
        motion: preference(&config.motion),
    };
    let lock = UI_PREFERENCES.get_or_init(|| RwLock::new(UiPreferences::default()));
    if let Ok(mut active) = lock.write() {
        *active = preferences;
    }
}

fn configured_preferences() -> UiPreferences {
    UI_PREFERENCES
        .get()
        .and_then(|lock| lock.read().ok().map(|value| value.clone()))
        .unwrap_or_default()
}

fn preference(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && !value.eq_ignore_ascii_case("auto")).then(|| value.to_string())
}

impl CapabilityInputs {
    pub fn stdout() -> Self {
        Self::for_stream(std::io::stdout().is_terminal())
    }

    pub fn stderr() -> Self {
        Self::for_stream(std::io::stderr().is_terminal())
    }

    fn for_stream(is_tty: bool) -> Self {
        let configured = configured_preferences();
        Self {
            is_tty,
            term: env_string("TERM"),
            no_color: std::env::var_os("NO_COLOR").is_some(),
            colorterm: env_string("COLORTERM"),
            colorfgbg: env_string("COLORFGBG"),
            locale: env_string("LC_ALL")
                .or_else(|| env_string("LC_CTYPE"))
                .or_else(|| env_string("LANG")),
            theme: env_string("AISHE_THEME").or(configured.theme),
            color_depth: env_string("AISHE_COLOR_DEPTH").or(configured.color_depth),
            unicode: env_string("AISHE_UNICODE").or(configured.unicode),
            motion: env_string("AISHE_MOTION").or(configured.motion),
            size: crossterm::terminal::size().ok(),
        }
    }
}

impl TerminalCapabilities {
    pub fn detect_stdout() -> Self {
        let capabilities = Self::resolve(&CapabilityInputs::stdout());
        if machine_output() {
            capabilities.for_json()
        } else {
            capabilities
        }
    }

    pub fn detect_stderr() -> Self {
        let capabilities = Self::resolve(&CapabilityInputs::stderr());
        if machine_output() {
            capabilities.for_json()
        } else {
            capabilities
        }
    }

    pub fn resolve(inputs: &CapabilityInputs) -> Self {
        let term_is_dumb = inputs
            .term
            .as_deref()
            .is_some_and(|value| value.eq_ignore_ascii_case("dumb"));
        let requested_theme = parse_theme(inputs.theme.as_deref());
        let background = detect_background(inputs.colorfgbg.as_deref());
        let theme = match requested_theme {
            Theme::Auto => match background {
                Background::Light => Theme::Light,
                Background::Dark | Background::Unknown => Theme::Dark,
            },
            explicit => explicit,
        };
        let styling_disabled =
            !inputs.is_tty || inputs.no_color || term_is_dumb || requested_theme == Theme::None;
        let color_depth = if styling_disabled {
            ColorDepth::None
        } else if requested_theme == Theme::Mono {
            ColorDepth::Ansi16
        } else {
            detect_color_depth(
                inputs.color_depth.as_deref(),
                inputs.colorterm.as_deref(),
                inputs.term.as_deref(),
            )
        };
        let unicode = detect_unicode(
            inputs.unicode.as_deref(),
            inputs.locale.as_deref(),
            term_is_dumb,
        );
        let motion = detect_motion(inputs.motion.as_deref(), inputs.is_tty && !term_is_dumb);
        let (columns, rows) = inputs
            .size
            .filter(|(columns, rows)| *columns > 0 && *rows > 0)
            .unwrap_or((80, 24));
        Self {
            is_tty: inputs.is_tty,
            term: inputs.term.clone(),
            color_depth,
            background,
            theme: if styling_disabled { Theme::None } else { theme },
            unicode,
            motion,
            columns,
            rows,
        }
    }

    pub fn styled(&self) -> bool {
        self.color_depth != ColorDepth::None && self.theme != Theme::None
    }

    pub fn paint(&self, token: StyleToken, text: &str) -> String {
        let Some(code) = self.ansi_code(token) else {
            return text.to_string();
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }

    pub fn glyphs(&self) -> Glyphs {
        Glyphs {
            unicode: self.unicode == UnicodePolicy::Unicode,
        }
    }

    /// Persistent authorship boundary shared by every prose-answer surface.
    pub fn assistant_answer_header(&self) -> String {
        render::answer_header(self)
    }

    /// Return an output policy suitable for a machine-readable document.
    /// JSON must never depend on the terminal attached to the process.
    pub fn for_json(&self) -> Self {
        let mut plain = self.clone();
        plain.color_depth = ColorDepth::None;
        plain.theme = Theme::None;
        plain.motion = Motion::Static;
        plain
    }

    fn ansi_code(&self, token: StyleToken) -> Option<String> {
        if !self.styled() {
            return None;
        }
        if self.theme == Theme::Mono {
            return Some(
                match token {
                    StyleToken::Focus | StyleToken::ProposedCommand => "1;7",
                    StyleToken::Danger => "1;4",
                    StyleToken::Accent
                    | StyleToken::AssistantLabel
                    | StyleToken::Success
                    | StyleToken::Warning
                    | StyleToken::Policy => "1",
                    _ => "2",
                }
                .into(),
            );
        }
        if token == StyleToken::Focus {
            return Some("1;7".into());
        }
        let (ansi16, ansi256, rgb) = palette_entry(self.theme, token);
        Some(match self.color_depth {
            ColorDepth::Ansi16 => ansi16.to_string(),
            ColorDepth::Ansi256 => format!("38;5;{ansi256}"),
            ColorDepth::TrueColor => format!("38;2;{};{};{}", rgb.0, rgb.1, rgb.2),
            ColorDepth::None => return None,
        })
    }
}

/// The statusline colors, as zsh prompt escapes, resolved from the same
/// palette the Rust renderers use. zsh used to carry its own literal indices,
/// so scope was green in the prompt and magenta in an approval panel, and
/// NO_COLOR never reached the prompt at all.
pub fn zsh_color_map(capabilities: &TerminalCapabilities) -> Vec<(&'static str, String)> {
    let entries: &[(&'static str, StyleToken, bool)] = &[
        ("AISHE_COLOR_CONNECTION", StyleToken::Accent, false),
        ("AISHE_COLOR_AUTH", StyleToken::Accent, false),
        ("AISHE_COLOR_PROVIDER", StyleToken::Muted, false),
        ("AISHE_COLOR_ENDPOINT", StyleToken::Muted, false),
        ("AISHE_COLOR_SELECTION", StyleToken::Muted, false),
        ("AISHE_COLOR_BACKEND", StyleToken::Muted, false),
        ("AISHE_COLOR_MUTED", StyleToken::Muted, false),
        ("AISHE_COLOR_MODEL", StyleToken::CodeLabel, false),
        ("AISHE_COLOR_REASONING", StyleToken::Activity, false),
        ("AISHE_COLOR_SCOPE", StyleToken::Policy, false),
        ("AISHE_COLOR_BRANCH", StyleToken::Policy, false),
        ("AISHE_COLOR_ENVIRONMENT", StyleToken::Danger, true),
        ("AISHE_COLOR_METRIC", StyleToken::Activity, false),
        ("AISHE_COLOR_PLAN", StyleToken::ProposedCommand, false),
        ("AISHE_COLOR_PATH", StyleToken::Accent, true),
        ("AISHE_COLOR_SUCCESS", StyleToken::Success, false),
        ("AISHE_COLOR_DANGER", StyleToken::Danger, false),
        ("AISHE_COLOR_MODE_SUGGEST", StyleToken::Warning, true),
        ("AISHE_COLOR_MODE_AUTO", StyleToken::Accent, true),
        ("AISHE_COLOR_MODE_YOLO", StyleToken::Danger, true),
    ];
    entries
        .iter()
        .map(|(key, token, bold)| {
            if !capabilities.styled() {
                return (*key, String::new());
            }
            let (_, ansi256, (r, g, b)) = palette_entry(capabilities.theme, *token);
            let color = match capabilities.color_depth {
                ColorDepth::None => return (*key, String::new()),
                ColorDepth::TrueColor => format!("%F{{#{r:02x}{g:02x}{b:02x}}}"),
                _ => format!("%F{{{ansi256}}}"),
            };
            (*key, if *bold { format!("%B{color}") } else { color })
        })
        .collect()
}

fn palette_entry(theme: Theme, token: StyleToken) -> (&'static str, u8, (u8, u8, u8)) {
    let light = theme == Theme::Light;
    match token {
        StyleToken::Accent | StyleToken::AssistantLabel => {
            if light {
                ("1;34", 25, (0, 83, 159))
            } else {
                ("1;36", 45, (89, 221, 255))
            }
        }
        StyleToken::UserShell | StyleToken::Success | StyleToken::DiffAdd => {
            if light {
                ("32", 28, (0, 115, 57))
            } else {
                ("32", 41, (63, 185, 80))
            }
        }
        StyleToken::UserAgent | StyleToken::Policy => {
            if light {
                ("35", 90, (119, 50, 141))
            } else {
                ("35", 177, (214, 137, 255))
            }
        }
        StyleToken::Activity | StyleToken::Muted => {
            if light {
                ("2;30", 242, (89, 96, 105))
            } else {
                ("2;37", 250, (168, 177, 188))
            }
        }
        StyleToken::ProposedCommand => {
            if light {
                ("1;30", 234, (31, 35, 40))
            } else {
                ("1;37", 255, (245, 247, 250))
            }
        }
        StyleToken::Warning => {
            if light {
                ("1;33", 130, (142, 88, 0))
            } else {
                ("1;33", 220, (255, 196, 61))
            }
        }
        StyleToken::Danger | StyleToken::DiffRemove => {
            if light {
                ("1;31", 124, (174, 31, 44))
            } else {
                ("1;31", 203, (255, 92, 92))
            }
        }
        StyleToken::CodeLabel => {
            if light {
                ("2;34", 25, (0, 83, 159))
            } else {
                ("2;36", 44, (70, 190, 220))
            }
        }
        StyleToken::Focus => ("1;7", 15, (255, 255, 255)),
    }
}

/// Measure visible terminal cells, ignoring ANSI CSI styling sequences.
///
/// Callers should still escape untrusted control characters before rendering;
/// this helper only makes layout independent of styling bytes.
pub fn cell_width(value: &str) -> usize {
    let plain = strip_ansi(value);
    UnicodeWidthStr::width(plain.as_str())
}

/// Truncate to terminal cells without splitting an extended grapheme cluster.
/// ANSI styling is intentionally removed: layout should happen before paint.
pub fn truncate_cells(value: &str, width: usize) -> String {
    let plain = strip_ansi(value);
    if cell_width(&plain) <= width {
        return plain;
    }
    if width == 0 {
        return String::new();
    }
    let ellipsis = if width >= 1 { "…" } else { "" };
    let target = width.saturating_sub(cell_width(ellipsis));
    let mut output = String::new();
    let mut used = 0;
    for grapheme in plain.graphemes(true) {
        let cells = UnicodeWidthStr::width(grapheme);
        if used + cells > target {
            break;
        }
        output.push_str(grapheme);
        used += cells;
    }
    output.push_str(ellipsis);
    output
}

/// Wrap text at terminal-cell boundaries while preserving graphemes. This is
/// deliberately deterministic and does not retain ANSI styling.
pub fn wrap_cells(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let plain = strip_ansi(value);
    let mut lines = Vec::new();
    for source_line in plain.split('\n') {
        if source_line.trim().is_empty() {
            lines.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut used = 0;
        for word in source_line.split_whitespace() {
            let word_width = cell_width(word);
            let separator = usize::from(!line.is_empty());
            if !line.is_empty() && used + separator + word_width <= width {
                line.push(' ');
                line.push_str(word);
                used += separator + word_width;
                continue;
            }
            if !line.is_empty() {
                lines.push(line);
                line = String::new();
                used = 0;
            }
            if word_width <= width {
                line.push_str(word);
                used = word_width;
                continue;
            }
            for grapheme in word.graphemes(true) {
                let cells = UnicodeWidthStr::width(grapheme);
                if !line.is_empty() && used + cells > width {
                    lines.push(line);
                    line = String::new();
                    used = 0;
                }
                // A single wide grapheme may exceed an extremely narrow
                // target. Preserve it rather than splitting its code points.
                line.push_str(grapheme);
                used += cells;
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn strip_ansi(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(character) = chars.next() {
        if character != '\u{1b}' {
            output.push(character);
            continue;
        }
        if chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if ('@'..='~').contains(&next) {
                    break;
                }
            }
        }
    }
    output
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Glyphs {
    unicode: bool,
}

impl Glyphs {
    pub fn success(self) -> &'static str {
        if self.unicode {
            "✓"
        } else {
            "OK"
        }
    }

    pub fn error(self) -> &'static str {
        if self.unicode {
            "✗"
        } else {
            "X"
        }
    }

    pub fn warning(self) -> &'static str {
        "!"
    }

    pub fn focus(self) -> &'static str {
        if self.unicode {
            "›"
        } else {
            ">"
        }
    }

    pub fn pending(self) -> &'static str {
        if self.unicode {
            "○"
        } else {
            "o"
        }
    }

    pub fn active(self) -> &'static str {
        if self.unicode {
            "●"
        } else {
            "*"
        }
    }

    pub fn branch(self) -> &'static str {
        if self.unicode {
            "↳"
        } else {
            "->"
        }
    }

    pub fn changed(self) -> &'static str {
        if self.unicode {
            "Δ"
        } else {
            "~"
        }
    }
}

fn env_string(name: &str) -> Option<String> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .and_then(os_string)
}

fn os_string(value: OsString) -> Option<String> {
    value.into_string().ok()
}

fn parse_theme(value: Option<&str>) -> Theme {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("dark") => Theme::Dark,
        Some("light") => Theme::Light,
        Some("mono" | "monochrome") => Theme::Mono,
        Some("none" | "plain" | "no-color") => Theme::None,
        _ => Theme::Auto,
    }
}

fn detect_color_depth(
    requested: Option<&str>,
    colorterm: Option<&str>,
    term: Option<&str>,
) -> ColorDepth {
    match requested
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("none" | "0") => return ColorDepth::None,
        Some("16" | "ansi16") => return ColorDepth::Ansi16,
        Some("256" | "ansi256") => return ColorDepth::Ansi256,
        Some("24" | "truecolor" | "24bit") => return ColorDepth::TrueColor,
        _ => {}
    }
    if colorterm.is_some_and(|value| {
        value.eq_ignore_ascii_case("truecolor") || value.eq_ignore_ascii_case("24bit")
    }) {
        ColorDepth::TrueColor
    } else if term.is_some_and(|value| value.to_ascii_lowercase().contains("256color")) {
        ColorDepth::Ansi256
    } else {
        ColorDepth::Ansi16
    }
}

fn detect_background(colorfgbg: Option<&str>) -> Background {
    let Some(value) = colorfgbg
        .and_then(|value| value.rsplit([';', ':']).next())
        .and_then(|value| value.trim().parse::<u8>().ok())
    else {
        return Background::Unknown;
    };
    if value <= 6 || value == 8 {
        Background::Dark
    } else {
        Background::Light
    }
}

fn detect_unicode(
    requested: Option<&str>,
    locale: Option<&str>,
    term_is_dumb: bool,
) -> UnicodePolicy {
    match requested
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ascii" | "never" | "off") => return UnicodePolicy::Ascii,
        Some("unicode" | "always" | "on") => return UnicodePolicy::Unicode,
        _ => {}
    }
    let locale_supports_utf8 = locale.is_some_and(|value| {
        let lower = value.to_ascii_lowercase();
        lower.contains("utf-8") || lower.contains("utf8")
    });
    if term_is_dumb || !locale_supports_utf8 {
        UnicodePolicy::Ascii
    } else {
        UnicodePolicy::Unicode
    }
}

fn detect_motion(requested: Option<&str>, live_capable: bool) -> Motion {
    match requested
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("static" | "reduced" | "off") => Motion::Static,
        Some("live" | "on") if live_capable => Motion::Live,
        _ if live_capable => Motion::Live,
        _ => Motion::Static,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal() -> CapabilityInputs {
        CapabilityInputs {
            is_tty: true,
            term: Some("xterm-256color".into()),
            no_color: false,
            colorterm: None,
            colorfgbg: Some("15;0".into()),
            locale: Some("en_US.UTF-8".into()),
            theme: None,
            color_depth: None,
            unicode: None,
            motion: None,
            size: Some((120, 40)),
        }
    }

    #[test]
    fn zsh_color_map_matches_the_renderer_palette_and_obeys_no_color() {
        let off = TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            no_color: true,
            term: Some("xterm-256color".into()),
            ..CapabilityInputs::default()
        });
        assert!(zsh_color_map(&off)
            .iter()
            .all(|(_, value)| value.is_empty()));

        let dark = TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            term: Some("xterm-256color".into()),
            ..CapabilityInputs::default()
        });
        let map = zsh_color_map(&dark);
        let scope = &map
            .iter()
            .find(|(key, _)| *key == "AISHE_COLOR_SCOPE")
            .unwrap()
            .1;
        // Scope is Policy in the renderers; the prompt used to paint it green.
        let (_, ansi256, _) = palette_entry(dark.theme, StyleToken::Policy);
        assert!(
            scope.contains(&ansi256.to_string()) || scope.starts_with("%F{#"),
            "scope color {scope} is not the Policy token"
        );
        assert!(map.iter().all(|(_, value)| !value.is_empty()));
    }

    #[test]
    fn no_color_dumb_and_redirected_output_are_plain_and_static() {
        for inputs in [
            CapabilityInputs {
                no_color: true,
                ..terminal()
            },
            CapabilityInputs {
                term: Some("dumb".into()),
                ..terminal()
            },
            CapabilityInputs {
                is_tty: false,
                ..terminal()
            },
        ] {
            let capabilities = TerminalCapabilities::resolve(&inputs);
            assert_eq!(capabilities.color_depth, ColorDepth::None);
            assert_eq!(capabilities.theme, Theme::None);
            assert!(!capabilities
                .paint(StyleToken::Danger, "error")
                .contains('\x1b'));
            if !inputs.is_tty || inputs.term.as_deref() == Some("dumb") {
                assert_eq!(capabilities.motion, Motion::Static);
            }
        }
    }

    #[test]
    fn resolves_depth_background_size_and_unicode() {
        let capabilities = TerminalCapabilities::resolve(&terminal());
        assert_eq!(capabilities.color_depth, ColorDepth::Ansi256);
        assert_eq!(capabilities.background, Background::Dark);
        assert_eq!(capabilities.theme, Theme::Dark);
        assert_eq!(capabilities.unicode, UnicodePolicy::Unicode);
        assert_eq!(capabilities.motion, Motion::Live);
        assert_eq!((capabilities.columns, capabilities.rows), (120, 40));

        let truecolor = TerminalCapabilities::resolve(&CapabilityInputs {
            colorterm: Some("truecolor".into()),
            colorfgbg: Some("0;15".into()),
            ..terminal()
        });
        assert_eq!(truecolor.color_depth, ColorDepth::TrueColor);
        assert_eq!(truecolor.background, Background::Light);
        assert_eq!(truecolor.theme, Theme::Light);
    }

    #[test]
    fn explicit_preferences_override_detection_safely() {
        let capabilities = TerminalCapabilities::resolve(&CapabilityInputs {
            theme: Some("mono".into()),
            color_depth: Some("truecolor".into()),
            unicode: Some("ascii".into()),
            motion: Some("static".into()),
            ..terminal()
        });
        assert_eq!(capabilities.theme, Theme::Mono);
        assert_eq!(capabilities.color_depth, ColorDepth::Ansi16);
        assert_eq!(capabilities.unicode, UnicodePolicy::Ascii);
        assert_eq!(capabilities.motion, Motion::Static);
        assert!(capabilities
            .paint(StyleToken::Focus, "selected")
            .contains("\x1b[1;7m"));
    }

    #[test]
    fn semantic_tokens_remain_text_when_styling_is_off() {
        let capabilities = TerminalCapabilities::resolve(&CapabilityInputs {
            no_color: true,
            ..terminal()
        });
        for token in [
            StyleToken::Accent,
            StyleToken::Focus,
            StyleToken::UserShell,
            StyleToken::UserAgent,
            StyleToken::AssistantLabel,
            StyleToken::Activity,
            StyleToken::ProposedCommand,
            StyleToken::Success,
            StyleToken::Warning,
            StyleToken::Danger,
            StyleToken::Muted,
            StyleToken::Policy,
            StyleToken::DiffAdd,
            StyleToken::DiffRemove,
            StyleToken::CodeLabel,
        ] {
            assert_eq!(capabilities.paint(token, "state"), "state");
        }
    }

    #[test]
    fn legacy_style_facade_is_semantic_and_plain_in_test_pipes() {
        let danger = "failure".red().bold().to_string();
        assert_eq!(danger, "failure");
        assert!(!danger.contains('\u{1b}'));
        assert_eq!("detail".dim().to_string(), "detail");
    }

    #[test]
    fn palettes_are_explicit_for_each_supported_depth_and_theme() {
        let cases = [
            (Theme::Dark, ColorDepth::Ansi16, "\u{1b}[1;36m"),
            (Theme::Dark, ColorDepth::Ansi256, "\u{1b}[38;5;45m"),
            (
                Theme::Dark,
                ColorDepth::TrueColor,
                "\u{1b}[38;2;89;221;255m",
            ),
            (Theme::Light, ColorDepth::Ansi16, "\u{1b}[1;34m"),
            (Theme::Light, ColorDepth::Ansi256, "\u{1b}[38;5;25m"),
            (Theme::Light, ColorDepth::TrueColor, "\u{1b}[38;2;0;83;159m"),
        ];
        for (theme, color_depth, prefix) in cases {
            let capabilities = TerminalCapabilities {
                theme,
                color_depth,
                ..TerminalCapabilities::resolve(&terminal())
            };
            assert_eq!(
                capabilities.paint(StyleToken::Accent, "AIShe"),
                format!("{prefix}AIShe\u{1b}[0m")
            );
        }
    }

    #[test]
    fn maintained_truecolor_palettes_meet_text_contrast_floor() {
        // Reference backgrounds are part of the recorded palette review in
        // docs/accessibility.md. Terminals may override ANSI16/256 palettes,
        // so only our owned truecolor values can be asserted numerically.
        let backgrounds = [(Theme::Light, (255, 255, 255)), (Theme::Dark, (18, 22, 28))];
        let tokens = [
            StyleToken::Accent,
            StyleToken::UserShell,
            StyleToken::UserAgent,
            StyleToken::Activity,
            StyleToken::ProposedCommand,
            StyleToken::Success,
            StyleToken::Warning,
            StyleToken::Danger,
            StyleToken::Muted,
            StyleToken::Policy,
            StyleToken::DiffAdd,
            StyleToken::DiffRemove,
            StyleToken::CodeLabel,
        ];
        for (theme, background) in backgrounds {
            for token in tokens {
                let (_, _, foreground) = palette_entry(theme, token);
                let ratio = contrast_ratio(foreground, background);
                assert!(
                    ratio >= 4.5,
                    "{theme:?}/{token:?} contrast {ratio:.2}:1 is below 4.5:1"
                );
            }
        }
    }

    fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
        let (a, b) = (relative_luminance(a), relative_luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn relative_luminance((red, green, blue): (u8, u8, u8)) -> f64 {
        fn channel(value: u8) -> f64 {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue)
    }

    #[test]
    fn ascii_glyphs_keep_every_state_distinct() {
        let glyphs = Glyphs { unicode: false };
        assert_eq!(glyphs.success(), "OK");
        assert_eq!(glyphs.error(), "X");
        assert_eq!(glyphs.warning(), "!");
        assert_eq!(glyphs.focus(), ">");
        assert_eq!(glyphs.pending(), "o");
        assert_eq!(glyphs.active(), "*");
        assert_eq!(glyphs.branch(), "->");
        assert_eq!(glyphs.changed(), "~");
    }

    #[test]
    fn missing_or_non_utf8_locale_uses_ascii() {
        assert_eq!(
            TerminalCapabilities::resolve(&CapabilityInputs {
                locale: None,
                ..terminal()
            })
            .unicode,
            UnicodePolicy::Ascii
        );
        assert_eq!(
            TerminalCapabilities::resolve(&CapabilityInputs {
                locale: Some("C".into()),
                ..terminal()
            })
            .unicode,
            UnicodePolicy::Ascii
        );
    }

    #[test]
    fn terminal_layout_matrix_bounds_wide_combining_and_hostile_text() {
        let samples = [
            "plain text with a very/long/project/path/that/keeps/going/src/provider/client.rs",
            "模型 claude-超长模型-2026-07-31 🚀 status",
            "combining: e\u{301} a\u{308} Z\u{20dd}",
            "emoji family: 👩‍👩‍👧‍👦 flags: 🇺🇸🇯🇵 keycap: 1️⃣",
            "\u{1b}[31mred\u{1b}[0m\rforged\nline\u{202e}unsafe",
        ];
        for width in [20, 21, 40, 79, 80, 120, 160, 200] {
            for sample in samples {
                let safe = crate::commands::display_safe_multiline(sample);
                assert!(!safe.contains('\u{1b}'));
                assert!(!safe.contains('\r'));
                assert!(!safe.contains('\u{202e}'));
                for line in wrap_cells(&safe, width) {
                    assert!(
                        cell_width(&line) <= width,
                        "width {width}: {:?} is {} cells",
                        line,
                        cell_width(&line)
                    );
                }
                let truncated = truncate_cells(&safe.replace('\n', " "), width);
                assert!(cell_width(&truncated) <= width);
            }
        }
    }

    #[test]
    fn json_policy_is_always_plain_and_static() {
        let capabilities = TerminalCapabilities::resolve(&terminal()).for_json();
        assert_eq!(capabilities.theme, Theme::None);
        assert_eq!(capabilities.color_depth, ColorDepth::None);
        assert_eq!(capabilities.motion, Motion::Static);
        assert!(!capabilities
            .paint(StyleToken::Accent, "value")
            .contains('\u{1b}'));
    }

    #[test]
    fn cell_layout_handles_wide_combining_emoji_and_ansi() {
        assert_eq!(cell_width("ASCII"), 5);
        assert_eq!(cell_width("界"), 2);
        assert_eq!(cell_width("e\u{301}"), 1);
        assert_eq!(cell_width("👩‍💻"), 2);
        assert_eq!(cell_width("\u{1b}[31mred\u{1b}[0m"), 3);

        let value = "A界e\u{301}ZQ";
        let truncated = truncate_cells(value, 5);
        assert_eq!(truncated, "A界e\u{301}…");
        assert_eq!(cell_width(&truncated), 5);
        assert!(!truncate_cells("👩‍💻xy", 3).contains('x'));
        assert!(truncate_cells("👩‍💻xy", 3).starts_with("👩‍💻"));
    }

    #[test]
    fn wrapping_never_splits_graphemes_or_exceeds_normal_widths() {
        let lines = wrap_cells("ab界cde\u{301}f", 4);
        assert_eq!(lines, vec!["ab界", "cde\u{301}f"]);
        assert!(lines.iter().all(|line| cell_width(line) <= 4));
    }
}
