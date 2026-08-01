//! Compile-time shell assets and their small, typed substitution surface.
//!
//! The assets are valid, reviewable shell files. Rendering is deliberately
//! limited to the registry-derived blocks and wrapper branding that cannot be
//! expressed statically. Every requested slot must occur exactly once, and no
//! template marker may survive generation.

pub(super) const SLASH_DISPATCH_MARKER: &str = "# __AISHE_GENERATED_SLASH_DISPATCH__";
pub(super) const QUESTION_GRAMMAR_MARKER: &str = "# __AISHE_GENERATED_QUESTION_GRAMMAR__";

pub(super) const ZSH_HOOK_TEMPLATE: &str = include_str!("assets/zsh_hook.zsh");
pub(super) const ZSH_INIT_TEMPLATE: &str = include_str!("assets/zsh_init.zsh");
pub(super) const WRAPPER_ZSHENV: &str = include_str!("assets/wrapper.zshenv");
pub(super) const WRAPPER_ZSHRC_TEMPLATE: &str = include_str!("assets/wrapper.zshrc");
pub(super) const PTY_PROMPT: &str = include_str!("assets/pty_prompt.zsh");
pub(super) const BASH_SCRIPT_TEMPLATE: &str = include_str!("assets/bash_hook.bash");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Slot {
    SlashDispatch,
    QuestionGrammar,
    ZshHook,
    ZshHookFinal,
    PtyPrompt,
    AsciiLogo,
}

impl Slot {
    const fn marker(self) -> &'static str {
        match self {
            Self::SlashDispatch => SLASH_DISPATCH_MARKER,
            Self::QuestionGrammar => QUESTION_GRAMMAR_MARKER,
            Self::ZshHook => "# __AISHE_TEMPLATE_ZSH_HOOK__",
            // The init template ends at this slot. Including its newline avoids
            // adding a second newline after the already newline-terminated hook.
            Self::ZshHookFinal => "# __AISHE_TEMPLATE_ZSH_HOOK_FINAL__\n",
            Self::PtyPrompt => "# __AISHE_TEMPLATE_PTY_PROMPT__",
            Self::AsciiLogo => "__AISHE_TEMPLATE_ASCII_LOGO__",
        }
    }
}

#[derive(Clone, Copy)]
struct Substitution<'a> {
    slot: Slot,
    value: &'a str,
}

impl<'a> Substitution<'a> {
    const fn new(slot: Slot, value: &'a str) -> Self {
        Self { slot, value }
    }
}

fn render(template_name: &str, template: &str, substitutions: &[Substitution<'_>]) -> String {
    let mut rendered = template.to_owned();
    for (index, substitution) in substitutions.iter().enumerate() {
        assert!(
            !substitutions[..index]
                .iter()
                .any(|prior| prior.slot == substitution.slot),
            "{template_name} received duplicate {:?} substitution",
            substitution.slot
        );
        let marker = substitution.slot.marker();
        assert_eq!(
            rendered.matches(marker).count(),
            1,
            "{template_name} must contain exactly one {marker:?} marker"
        );
        rendered = rendered.replacen(marker, substitution.value, 1);
    }
    assert!(
        !rendered.contains("__AISHE_TEMPLATE_"),
        "{template_name} has an unresolved typed template marker"
    );
    rendered
}

pub(super) fn zsh_hook(slash_dispatch: &str, question_grammar: &str) -> String {
    let rendered = render(
        "zsh hook",
        ZSH_HOOK_TEMPLATE,
        &[
            Substitution::new(Slot::SlashDispatch, slash_dispatch),
            Substitution::new(Slot::QuestionGrammar, question_grammar),
        ],
    );
    assert!(!rendered.contains("__AISHE_GENERATED_"));
    rendered
}

pub(super) fn zsh_script(zsh_hook: &str) -> String {
    render(
        "zsh init",
        ZSH_INIT_TEMPLATE,
        &[Substitution::new(Slot::ZshHookFinal, zsh_hook)],
    )
}

pub(super) fn wrapper_zshrc(zsh_hook: &str, pty_prompt: &str, ascii_logo: &str) -> String {
    let mut rendered = render(
        "PTY wrapper zshrc",
        WRAPPER_ZSHRC_TEMPLATE,
        &[
            Substitution::new(Slot::ZshHook, zsh_hook),
            Substitution::new(Slot::PtyPrompt, pty_prompt),
            Substitution::new(Slot::AsciiLogo, ascii_logo),
        ],
    );
    // The historical formatter ended immediately after `fi`; retain those
    // bytes even though the source asset itself follows POSIX text-file style.
    assert!(rendered.ends_with('\n'));
    rendered.pop();
    rendered
}

pub(super) fn bash_script(slash_dispatch: &str) -> String {
    let rendered = render(
        "bash hook",
        BASH_SCRIPT_TEMPLATE,
        &[Substitution::new(Slot::SlashDispatch, slash_dispatch)],
    );
    assert!(!rendered.contains("__AISHE_GENERATED_"));
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_rejects_missing_and_duplicate_slots() {
        let missing = std::panic::catch_unwind(|| render("missing", "before", &[]));
        assert!(missing.is_ok(), "marker-free static templates are valid");

        let duplicate = std::panic::catch_unwind(|| {
            render(
                "duplicate",
                "# __AISHE_TEMPLATE_ZSH_HOOK__",
                &[
                    Substitution::new(Slot::ZshHook, "one"),
                    Substitution::new(Slot::ZshHook, "two"),
                ],
            )
        });
        assert!(duplicate.is_err());

        let unresolved =
            std::panic::catch_unwind(|| render("unresolved", "__AISHE_TEMPLATE_ASCII_LOGO__", &[]));
        assert!(unresolved.is_err());
    }

    #[test]
    fn repeated_generation_is_byte_stable() {
        let first = wrapper_zshrc("hook\n", "prompt\n", "logo");
        let second = wrapper_zshrc("hook\n", "prompt\n", "logo");
        assert_eq!(first.as_bytes(), second.as_bytes());
    }
}
