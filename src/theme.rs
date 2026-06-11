//! Color theming for the prompt and the syntax highlighter.
//!
//! The prompt uses `crossterm::style::Color` (re-exported by reedline) while the
//! highlighter emits `nu_ansi_term::Style`. To keep a single source of truth we
//! represent every themeable color as an ANSI index / RGB triple (`Col`) and
//! convert to whichever library type is needed.

use nu_ansi_term::{Color as NuColor, Style as NuStyle};
use serde::{Deserialize, Serialize};

/// Built-in theme preset names (for `aishe theme` and validation).
pub const PRESETS: &[&str] = &["default", "vivid", "mono", "nord", "gruvbox"];

/// A library-neutral color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Col {
    /// 256-color / 16-color palette index.
    Ansi(u8),
    /// True color.
    Rgb(u8, u8, u8),
    /// Terminal default.
    Default,
}

impl Col {
    pub fn to_crossterm(self) -> crossterm::style::Color {
        use crossterm::style::Color;
        match self {
            Col::Ansi(n) => Color::AnsiValue(n),
            Col::Rgb(r, g, b) => Color::Rgb { r, g, b },
            Col::Default => Color::Reset,
        }
    }

    pub fn to_nu(self) -> NuColor {
        match self {
            Col::Ansi(n) => NuColor::Fixed(n),
            Col::Rgb(r, g, b) => NuColor::Rgb(r, g, b),
            Col::Default => NuColor::Default,
        }
    }

    pub fn to_nu_style(self) -> NuStyle {
        match self {
            Col::Default => NuStyle::new(),
            other => NuStyle::new().fg(other.to_nu()),
        }
    }

    /// Parse a color from a string: a named color (optionally `bright-`/`dark-`
    /// prefixed), `#rrggbb`, or a palette index (`0`–`255`).
    pub fn parse(s: &str) -> Option<Col> {
        let s = s.trim().to_lowercase();
        if s.is_empty() || s == "default" || s == "none" {
            return Some(Col::Default);
        }
        if let Some(hex) = s.strip_prefix('#') {
            if hex.len() == 6 {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                return Some(Col::Rgb(r, g, b));
            }
            return None;
        }
        if let Ok(n) = s.parse::<u8>() {
            return Some(Col::Ansi(n));
        }
        named_to_ansi(&s).map(Col::Ansi)
    }
}

/// Map a named color (with optional `bright-`/`light-`/`dark-` prefix) to an
/// ANSI palette index (0–15).
fn named_to_ansi(name: &str) -> Option<u8> {
    let (bright, base) = if let Some(rest) = name
        .strip_prefix("bright-")
        .or_else(|| name.strip_prefix("bright"))
        .or_else(|| name.strip_prefix("light-"))
        .or_else(|| name.strip_prefix("light"))
    {
        (true, rest)
    } else if let Some(rest) = name
        .strip_prefix("dark-")
        .or_else(|| name.strip_prefix("dark"))
    {
        (false, rest)
    } else {
        (false, name)
    };

    let idx = match base.trim_matches('-') {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" | "purple" => 5,
        "cyan" => 6,
        "white" | "grey" | "gray" => 7,
        _ => return None,
    };
    // "grey"/"gray" with the bright prefix maps to bright black (8), and
    // "darkgrey" (base white→7, dark) to bright black too; normalize sensibly.
    let idx = if (base == "grey" || base == "gray") && !bright {
        8 // a bare "grey" reads as bright black, the usual dim grey
    } else {
        idx
    };
    Some(if bright { idx + 8 } else { idx })
}

/// Serializable theme: an optional preset plus per-role overrides (color names).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default)]
    pub preset: Option<String>,
    // Prompt roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph_ok: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glyph_err: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right_prompt: Option<String>,
    // Highlighter roles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unknown_cmd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub string: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigil_nl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigil_shell: Option<String>,
}

/// Resolved theme with concrete colors for every role.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub cwd: Col,
    pub glyph_ok: Col,
    pub glyph_err: Col,
    pub right_prompt: Col,
    pub known_cmd: Col,
    pub unknown_cmd: Col,
    pub flag: Col,
    pub string: Col,
    pub operator: Col,
    pub path: Col,
    pub assignment: Col,
    pub sigil_nl: Col,
    pub sigil_shell: Col,
}

impl Theme {
    /// The built-in default palette.
    pub fn preset_default() -> Theme {
        Theme {
            cwd: Col::Ansi(14),         // bright cyan
            glyph_ok: Col::Ansi(10),    // bright green
            glyph_err: Col::Ansi(9),    // bright red
            right_prompt: Col::Ansi(8), // grey
            known_cmd: Col::Ansi(10),   // bright green
            unknown_cmd: Col::Ansi(9),  // bright red
            flag: Col::Ansi(11),        // bright yellow
            string: Col::Ansi(2),       // green
            operator: Col::Ansi(13),    // bright magenta
            path: Col::Ansi(12),        // bright blue
            assignment: Col::Ansi(6),   // cyan
            sigil_nl: Col::Ansi(13),    // bright magenta
            sigil_shell: Col::Ansi(11), // bright yellow
        }
    }

    /// A higher-contrast preset.
    pub fn preset_vivid() -> Theme {
        Theme {
            cwd: Col::Rgb(0x56, 0xb6, 0xc2),
            glyph_ok: Col::Rgb(0x98, 0xc3, 0x79),
            glyph_err: Col::Rgb(0xe0, 0x6c, 0x75),
            right_prompt: Col::Rgb(0x5c, 0x63, 0x70),
            known_cmd: Col::Rgb(0x98, 0xc3, 0x79),
            unknown_cmd: Col::Rgb(0xe0, 0x6c, 0x75),
            flag: Col::Rgb(0xe5, 0xc0, 0x7b),
            string: Col::Rgb(0x98, 0xc3, 0x79),
            operator: Col::Rgb(0xc6, 0x78, 0xdd),
            path: Col::Rgb(0x61, 0xaf, 0xef),
            assignment: Col::Rgb(0x56, 0xb6, 0xc2),
            sigil_nl: Col::Rgb(0xc6, 0x78, 0xdd),
            sigil_shell: Col::Rgb(0xe5, 0xc0, 0x7b),
        }
    }

    /// A monochrome preset (only known/unknown and structure differ subtly).
    pub fn preset_mono() -> Theme {
        let fg = Col::Default;
        Theme {
            cwd: Col::Ansi(7),
            glyph_ok: fg,
            glyph_err: Col::Ansi(9),
            right_prompt: Col::Ansi(8),
            known_cmd: fg,
            unknown_cmd: Col::Ansi(9),
            flag: Col::Ansi(8),
            string: Col::Ansi(8),
            operator: Col::Ansi(8),
            path: fg,
            assignment: Col::Ansi(8),
            sigil_nl: Col::Ansi(8),
            sigil_shell: Col::Ansi(8),
        }
    }

    /// Nord palette (arctic, bluish).
    pub fn preset_nord() -> Theme {
        Theme {
            cwd: Col::Rgb(0x88, 0xc0, 0xd0),
            glyph_ok: Col::Rgb(0xa3, 0xbe, 0x8c),
            glyph_err: Col::Rgb(0xbf, 0x61, 0x6a),
            right_prompt: Col::Rgb(0x4c, 0x56, 0x6a),
            known_cmd: Col::Rgb(0xa3, 0xbe, 0x8c),
            unknown_cmd: Col::Rgb(0xbf, 0x61, 0x6a),
            flag: Col::Rgb(0xeb, 0xcb, 0x8b),
            string: Col::Rgb(0xa3, 0xbe, 0x8c),
            operator: Col::Rgb(0xb4, 0x8e, 0xad),
            path: Col::Rgb(0x81, 0xa1, 0xc1),
            assignment: Col::Rgb(0x88, 0xc0, 0xd0),
            sigil_nl: Col::Rgb(0xb4, 0x8e, 0xad),
            sigil_shell: Col::Rgb(0xeb, 0xcb, 0x8b),
        }
    }

    /// Gruvbox (dark) palette (warm, retro).
    pub fn preset_gruvbox() -> Theme {
        Theme {
            cwd: Col::Rgb(0x8e, 0xc0, 0x7c),
            glyph_ok: Col::Rgb(0xb8, 0xbb, 0x26),
            glyph_err: Col::Rgb(0xfb, 0x49, 0x34),
            right_prompt: Col::Rgb(0x92, 0x83, 0x74),
            known_cmd: Col::Rgb(0xb8, 0xbb, 0x26),
            unknown_cmd: Col::Rgb(0xfb, 0x49, 0x34),
            flag: Col::Rgb(0xfa, 0xbd, 0x2f),
            string: Col::Rgb(0xb8, 0xbb, 0x26),
            operator: Col::Rgb(0xd3, 0x86, 0x9b),
            path: Col::Rgb(0x83, 0xa5, 0x98),
            assignment: Col::Rgb(0x8e, 0xc0, 0x7c),
            sigil_nl: Col::Rgb(0xd3, 0x86, 0x9b),
            sigil_shell: Col::Rgb(0xfa, 0xbd, 0x2f),
        }
    }

    fn preset_by_name(name: &str) -> Theme {
        match name {
            "vivid" => Theme::preset_vivid(),
            "mono" => Theme::preset_mono(),
            "nord" => Theme::preset_nord(),
            "gruvbox" => Theme::preset_gruvbox(),
            _ => Theme::preset_default(),
        }
    }

    /// Resolve a `ThemeConfig` into a concrete `Theme`: start from the named
    /// preset (default if none/unknown), then apply any per-role overrides.
    pub fn from_config(cfg: &ThemeConfig) -> Theme {
        let mut t = Theme::preset_by_name(cfg.preset.as_deref().unwrap_or("default"));
        let apply = |slot: &mut Col, value: &Option<String>| {
            if let Some(s) = value {
                if let Some(c) = Col::parse(s) {
                    *slot = c;
                }
            }
        };
        apply(&mut t.cwd, &cfg.cwd);
        apply(&mut t.glyph_ok, &cfg.glyph_ok);
        apply(&mut t.glyph_err, &cfg.glyph_err);
        apply(&mut t.right_prompt, &cfg.right_prompt);
        apply(&mut t.known_cmd, &cfg.known_cmd);
        apply(&mut t.unknown_cmd, &cfg.unknown_cmd);
        apply(&mut t.flag, &cfg.flag);
        apply(&mut t.string, &cfg.string);
        apply(&mut t.operator, &cfg.operator);
        apply(&mut t.path, &cfg.path);
        apply(&mut t.assignment, &cfg.assignment);
        apply(&mut t.sigil_nl, &cfg.sigil_nl);
        apply(&mut t.sigil_shell, &cfg.sigil_shell);
        t
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::preset_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_named_colors() {
        assert_eq!(Col::parse("red"), Some(Col::Ansi(1)));
        assert_eq!(Col::parse("green"), Some(Col::Ansi(2)));
        assert_eq!(Col::parse("bright-green"), Some(Col::Ansi(10)));
        assert_eq!(Col::parse("brightred"), Some(Col::Ansi(9)));
        assert_eq!(Col::parse("cyan"), Some(Col::Ansi(6)));
        assert_eq!(Col::parse("magenta"), Some(Col::Ansi(5)));
        assert_eq!(Col::parse("purple"), Some(Col::Ansi(5)));
    }

    #[test]
    fn parse_hex_and_index() {
        assert_eq!(Col::parse("#ff8800"), Some(Col::Rgb(0xff, 0x88, 0x00)));
        assert_eq!(Col::parse("42"), Some(Col::Ansi(42)));
        assert_eq!(Col::parse("default"), Some(Col::Default));
        assert_eq!(Col::parse(""), Some(Col::Default));
        assert_eq!(Col::parse("notacolor"), None);
        assert_eq!(Col::parse("#zzz"), None);
    }

    #[test]
    fn preset_and_overrides() {
        let cfg = ThemeConfig {
            preset: Some("mono".into()),
            cwd: Some("#102030".into()),
            ..Default::default()
        };
        let t = Theme::from_config(&cfg);
        assert_eq!(t.cwd, Col::Rgb(0x10, 0x20, 0x30));
        // unspecified roles come from the mono preset
        assert_eq!(t.unknown_cmd, Col::Ansi(9));
    }

    #[test]
    fn named_presets_resolve() {
        // Every advertised preset name resolves to a distinct, parseable theme.
        for name in PRESETS {
            let cfg = ThemeConfig {
                preset: Some((*name).to_string()),
                ..Default::default()
            };
            let _ = Theme::from_config(&cfg);
        }
        assert_eq!(
            Theme::from_config(&ThemeConfig {
                preset: Some("nord".into()),
                ..Default::default()
            })
            .cwd,
            Col::Rgb(0x88, 0xc0, 0xd0)
        );
    }

    #[test]
    fn unknown_preset_falls_back_to_default() {
        let cfg = ThemeConfig {
            preset: Some("nonsense".into()),
            ..Default::default()
        };
        let t = Theme::from_config(&cfg);
        assert_eq!(t.cwd, Theme::preset_default().cwd);
    }

    #[test]
    fn conversions_do_not_panic() {
        let t = Theme::preset_vivid();
        let _ = t.cwd.to_crossterm();
        let _ = t.known_cmd.to_nu_style();
        let _ = Col::Default.to_crossterm();
    }
}
