//! Pure, width-bounded terminal views shared by interactive front ends.
//!
//! Inputs must describe presentation state only. This module deliberately has
//! no dependency on providers, agents, policy engines, or prompt readers, so a
//! caller can snapshot the exact view before performing any side effect.

use super::{truncate_cells, wrap_cells, StyleToken, TerminalCapabilities};

const MODEL_TEXT_LIMIT: usize = 4_096;
const MODEL_LINE_LIMIT: usize = 24;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyCue {
    Safe,
    Review,
    Dangerous,
    Unknown,
}

impl SafetyCue {
    fn label(self) -> &'static str {
        match self {
            Self::Safe => "[safe]",
            Self::Review => "[review]",
            Self::Dangerous => "[danger]",
            Self::Unknown => "[unknown]",
        }
    }

    fn token(self) -> StyleToken {
        match self {
            Self::Safe => StyleToken::Success,
            Self::Review | Self::Unknown => StyleToken::Warning,
            Self::Dangerous => StyleToken::Danger,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ProposalView<'a> {
    pub title: &'a str,
    pub command: &'a str,
    pub effect: &'a str,
    pub reason: &'a str,
    pub safety: &'a str,
    pub safety_cue: SafetyCue,
    pub scope: &'a str,
    pub network: &'a str,
    pub sandbox: &'a str,
    pub default_action: &'a str,
}

#[derive(Clone, Copy, Debug)]
pub struct ApprovalView<'a> {
    pub proposal: ProposalView<'a>,
    pub tool: &'a str,
    pub mode: &'a str,
    pub choices: &'a str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    Success,
    Warning,
    Error,
    /// Not done and not wrong: a check the platform cannot run.
    Skipped,
}

/// Persistent authorship boundary shared by prose-answer surfaces.
pub fn answer_header(capabilities: &TerminalCapabilities) -> String {
    capabilities.paint(StyleToken::AssistantLabel, "AIShe · answer")
}

/// Render a proposal as plain semantic rows. Every safety state carries a text
/// cue, so meaning never depends on color or Unicode support.
pub fn proposal_lines(
    view: &ProposalView<'_>,
    capabilities: &TerminalCapabilities,
    width: usize,
) -> Vec<String> {
    let width = width.max(20);
    let mut lines =
        vec![capabilities.paint(StyleToken::AssistantLabel, &bounded_model_text(view.title))];
    lines.push(format!(
        "  {} {}",
        capabilities.paint(view.safety_cue.token(), view.safety_cue.label()),
        bounded_model_text(view.safety)
    ));
    push_field(
        &mut lines,
        "command",
        view.command,
        StyleToken::ProposedCommand,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "effect",
        view.effect,
        StyleToken::Accent,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "reason",
        view.reason,
        StyleToken::Muted,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "scope",
        view.scope,
        StyleToken::Policy,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "network",
        view.network,
        StyleToken::Policy,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "sandbox",
        view.sandbox,
        StyleToken::Policy,
        capabilities,
        width,
    );
    push_field(
        &mut lines,
        "default",
        view.default_action,
        StyleToken::Warning,
        capabilities,
        width,
    );
    lines
}

/// Compact compatibility proposal used by suggest/auto command surfaces. It
/// preserves their established spacing while centralizing bounded model-text
/// escaping and semantic authorship/command styling.
pub fn command_proposal_lines(
    title: &str,
    command: &str,
    explanation: &str,
    capabilities: &TerminalCapabilities,
    spacious: bool,
) -> Vec<String> {
    let mut lines =
        vec![capabilities.paint(StyleToken::AssistantLabel, &bounded_model_text(title))];
    if spacious {
        lines.push(String::new());
    }
    lines.push(format!(
        "  {}",
        capabilities.paint(StyleToken::ProposedCommand, &bounded_model_text(command))
    ));
    if !explanation.is_empty() {
        if spacious {
            lines.push(String::new());
        }
        lines.push(capabilities.paint(StyleToken::Muted, &bounded_model_text(explanation)));
    }
    lines
}

/// Render the reusable bordered approval card. The card is static and safe for
/// redirected, monochrome, ASCII, SSH, and screen-reader-oriented terminals.
pub fn approval_panel(
    view: &ApprovalView<'_>,
    capabilities: &TerminalCapabilities,
    width: usize,
) -> String {
    let ascii = capabilities.glyphs().focus() == ">";
    let side = if ascii { "|" } else { "│" };
    let tool = bounded_model_text(view.tool);
    let top = if ascii {
        format!("  +-- approval required: agent action - {tool} --")
    } else {
        format!("  ┌─ approval required: agent action · {tool} ──")
    };
    let bottom = if ascii {
        "  +--------------------------------------------------"
    } else {
        "  └──────────────────────────────────────────────────"
    };
    let mut lines = vec![top, format!("  {side} mode      {}", safe(view.mode))];
    let fields = [
        ("scope", view.proposal.scope),
        ("network", view.proposal.network),
        ("sandbox", view.proposal.sandbox),
        ("effect", view.proposal.effect),
        ("reason", view.proposal.reason),
    ];
    for (label, value) in fields {
        append_panel_field(&mut lines, side, label, value, width);
    }
    let safety = format!(
        "{} {}",
        view.proposal.safety_cue.label(),
        bounded_model_text(view.proposal.safety)
    );
    append_panel_field(&mut lines, side, "safety", &safety, width);
    append_panel_field(&mut lines, side, "command", view.proposal.command, width);
    append_panel_field(
        &mut lines,
        side,
        "default",
        view.proposal.default_action,
        width,
    );
    if !view.choices.is_empty() {
        append_panel_field(&mut lines, side, "keys", view.choices, width);
    }
    lines.push(bottom.to_string());
    format!("{}\n", lines.join("\n"))
}

/// Compatibility row for raw-mode pickers. It deliberately keeps an ASCII
/// marker even under Unicode policy so redraw alignment and copied logs remain
/// stable across terminal fonts.
pub fn picker_row(label: &str, selected: bool, width: usize) -> String {
    let marker = if selected { ">" } else { " " };
    truncate_cells(&format!("  {marker} {}", safe(label)), width.max(1))
}

pub fn focus_row(
    index: usize,
    label: &str,
    capabilities: &TerminalCapabilities,
    width: usize,
) -> String {
    truncate_cells(
        &format!(
            "{} {}) {}",
            capabilities.glyphs().focus(),
            index + 1,
            safe(label)
        ),
        width.max(1),
    )
}

pub fn notice_text(kind: NoticeKind, message: &str, capabilities: &TerminalCapabilities) -> String {
    let (glyph, token) = match kind {
        NoticeKind::Success => (capabilities.glyphs().success(), StyleToken::Success),
        NoticeKind::Warning => (capabilities.glyphs().warning(), StyleToken::Warning),
        NoticeKind::Error => (capabilities.glyphs().error(), StyleToken::Danger),
        NoticeKind::Skipped => (capabilities.glyphs().pending(), StyleToken::Muted),
    };
    capabilities.paint(token, &format!("{glyph} {}", safe(message)))
}

pub fn notice_lines(
    kind: NoticeKind,
    message: &str,
    capabilities: &TerminalCapabilities,
    width: usize,
) -> Vec<String> {
    let (glyph, token) = match kind {
        NoticeKind::Success => (capabilities.glyphs().success(), StyleToken::Success),
        NoticeKind::Warning => (capabilities.glyphs().warning(), StyleToken::Warning),
        NoticeKind::Error => (capabilities.glyphs().error(), StyleToken::Danger),
        NoticeKind::Skipped => (capabilities.glyphs().pending(), StyleToken::Muted),
    };
    wrap_cells(&format!("{glyph} {}", safe(message)), width.max(1))
        .into_iter()
        .map(|line| capabilities.paint(token, &line))
        .collect()
}

pub fn error_lines(
    message: &str,
    capabilities: &TerminalCapabilities,
    width: usize,
) -> Vec<String> {
    notice_lines(NoticeKind::Error, message, capabilities, width)
}

pub fn approval_suffix(default: bool) -> &'static str {
    if default {
        "[Y/n]"
    } else {
        "[y/N]"
    }
}

fn push_field(
    output: &mut Vec<String>,
    label: &str,
    value: &str,
    token: StyleToken,
    capabilities: &TerminalCapabilities,
    width: usize,
) {
    let prefix = format!("  {label:<8} ");
    let available = width.saturating_sub(super::cell_width(&prefix)).max(1);
    let value = bounded_model_text(value);
    for (index, line) in wrap_cells(&value, available).into_iter().enumerate() {
        let prefix = if index == 0 {
            prefix.clone()
        } else {
            " ".repeat(super::cell_width(&prefix))
        };
        output.push(format!("{prefix}{}", capabilities.paint(token, &line)));
    }
}

fn append_panel_field(
    output: &mut Vec<String>,
    side: &str,
    label: &str,
    value: &str,
    width: usize,
) {
    let prefix = format!("  {side} {label:<9} ");
    let available = width
        .max(40)
        .saturating_sub(super::cell_width(&prefix))
        .max(1);
    let value = bounded_model_text(value);
    for (index, line) in wrap_cells(&value, available)
        .into_iter()
        .take(MODEL_LINE_LIMIT)
        .enumerate()
    {
        let prefix = if index == 0 {
            prefix.clone()
        } else {
            format!("  {side} {}", " ".repeat(10))
        };
        output.push(format!("{prefix}{line}"));
    }
}

fn bounded_model_text(value: &str) -> String {
    let safe = safe(value);
    if safe.chars().count() <= MODEL_TEXT_LIMIT {
        return safe;
    }
    let mut bounded = safe.chars().take(MODEL_TEXT_LIMIT - 1).collect::<String>();
    bounded.push('…');
    bounded
}

fn safe(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\n' => output.push('\n'),
            '\t' => output.push_str("  "),
            '\u{1b}' => output.push_str("\\x1b"),
            '\r' => output.push_str("\\r"),
            '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' => {
                output.push_str(&format!("\\u{{{:04x}}}", character as u32));
            }
            control if control.is_control() => {
                output.push_str(&format!("\\x{:02x}", control as u32));
            }
            printable => output.push(printable),
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{CapabilityInputs, UnicodePolicy};

    fn plain(ascii: bool) -> TerminalCapabilities {
        TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: false,
            locale: Some(if ascii { "C" } else { "en_US.UTF-8" }.into()),
            unicode: Some(if ascii { "ascii" } else { "unicode" }.into()),
            size: Some((100, 30)),
            ..CapabilityInputs::default()
        })
    }

    fn proposal<'a>() -> ProposalView<'a> {
        ProposalView {
            title: "AIShe · proposal",
            command: "rm -rf /tmp/example",
            effect: "deletes files",
            reason: "clean generated output",
            safety: "destructive command requires review",
            safety_cue: SafetyCue::Dangerous,
            scope: "workspace only",
            network: "deny",
            sandbox: "bubblewrap",
            default_action: "deny; Enter, Esc, Ctrl-C, and EOF run nothing",
        }
    }

    #[test]
    fn proposal_snapshot_has_complete_non_color_semantics() {
        assert_eq!(
            proposal_lines(&proposal(), &plain(true), 100).join("\n"),
            "AIShe · proposal\n  [danger] destructive command requires review\n  command  rm -rf /tmp/example\n  effect   deletes files\n  reason   clean generated output\n  scope    workspace only\n  network  deny\n  sandbox  bubblewrap\n  default  deny; Enter, Esc, Ctrl-C, and EOF run nothing"
        );
    }

    #[test]
    fn approval_snapshot_is_ascii_bounded_and_escapes_model_text() {
        let mut proposal = proposal();
        proposal.command = "printf '\u{1b}[31munsafe\u{1b}[0m'";
        let rendered = approval_panel(
            &ApprovalView {
                proposal,
                tool: "run command",
                mode: "auto",
                choices: "o once; s session; e edit; d/Esc deny",
            },
            &plain(true),
            100,
        );
        assert!(rendered.starts_with("  +-- approval required: agent action - run command --\n"));
        assert!(rendered.contains("| safety    [danger] destructive command requires review"));
        assert!(rendered.contains("\\x1b[31munsafe\\x1b[0m"));
        assert!(rendered.contains("| default   deny; Enter, Esc, Ctrl-C, and EOF run nothing"));
        assert!(!rendered.contains('\u{1b}'));
        assert_eq!(plain(true).unicode, UnicodePolicy::Ascii);
    }

    #[test]
    fn picker_notice_answer_and_defaults_have_stable_plain_snapshots() {
        let capabilities = plain(true);
        assert_eq!(answer_header(&capabilities), "AIShe · answer");
        assert_eq!(picker_row("OpenAI · gpt", true, 80), "  > OpenAI · gpt");
        assert_eq!(focus_row(2, "choice", &capabilities, 80), "> 3) choice");
        assert_eq!(
            notice_text(NoticeKind::Error, "failed", &capabilities),
            "X failed"
        );
        assert_eq!(approval_suffix(false), "[y/N]");
        assert_eq!(approval_suffix(true), "[Y/n]");
    }
}
