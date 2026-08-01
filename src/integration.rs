//! Native zsh and Bash integration generation.
//!
//! Reviewable shell source lives under `integration/assets`; this module only
//! orchestrates the registry-derived fragments and the few typed substitutions
//! needed by standalone hooks and the PTY wrapper. The shared zsh hook remains
//! the single behavior source for both `aishe init zsh` and `aishe zsh`.

use std::borrow::Cow;

mod registry;
mod templates;

#[cfg(test)]
mod tests;

use registry::{render_question_grammar, render_slash_dispatch, HookShell};

#[cfg(test)]
use crate::command_surface::{Surface, SurfaceSupport, COMMANDS};
#[cfg(test)]
use crate::dispatcher::{QUESTION_PAIR_RULES, TRAILING_QUESTION_HEADS};
#[cfg(test)]
use registry::shell_single_quote;
#[cfg(test)]
use templates::{
    BASH_SCRIPT_TEMPLATE, QUESTION_GRAMMAR_MARKER, SLASH_DISPATCH_MARKER, ZSH_HOOK_TEMPLATE,
};

/// Return the integration script for the named shell, or `None` if unsupported.
pub fn script(shell: &str) -> Option<Cow<'static, str>> {
    match shell {
        "zsh" => Some(Cow::Owned(zsh_script())),
        "bash" => Some(Cow::Owned(bash_script())),
        _ => None,
    }
}

/// Shells we can emit an integration for.
pub const SUPPORTED: &[&str] = &["zsh", "bash"];

/// Shared, fully rendered zsh hook used by both `init zsh` and the PTY wrapper.
pub fn zsh_hook() -> String {
    templates::zsh_hook(
        &render_slash_dispatch(HookShell::Zsh),
        &render_question_grammar(),
    )
}

/// The full `init zsh` snippet: static header plus the rendered shared hook.
pub fn zsh_script() -> String {
    templates::zsh_script(&zsh_hook())
}

/// `.zshenv` for the PTY wrapper's isolated `ZDOTDIR`.
pub const WRAPPER_ZSHENV: &str = templates::WRAPPER_ZSHENV;

/// `.zshrc` for the PTY wrapper: user config, shared hook, prompt, and hint.
pub fn wrapper_zshrc() -> String {
    templates::wrapper_zshrc(
        &zsh_hook(),
        templates::PTY_PROMPT,
        crate::promptui::ASCII_LOGO,
    )
}

/// Fully rendered Bash hook with registry-derived slash dispatch.
pub fn bash_script() -> String {
    templates::bash_script(&render_slash_dispatch(HookShell::Bash))
}
