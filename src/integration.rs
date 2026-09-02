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

/// Bash 3.2 has no quote-aware argv splitter. Keep slash arguments as data,
/// parse their shell-style quotes here, then re-enter the canonical CLI by argv.
pub fn dispatch_hook_cli(id: &str, raw: &str) -> anyhow::Result<u8> {
    use crate::command_surface::{ArgumentPolicy, Lifecycle, ShellHookAction, SurfaceSupport};

    let spec = crate::command_surface::by_id(id).filter(|spec| {
        matches!(spec.lifecycle, Lifecycle::Active)
            && matches!(spec.hook_action(), ShellHookAction::Cli)
            && matches!(spec.arguments, ArgumentPolicy::PassThrough(_))
            && matches!(
                spec.support(crate::command_surface::Surface::BashHook),
                SurfaceSupport::Supported
            )
    });
    let invocation = spec
        .and_then(|spec| spec.cli)
        .ok_or_else(|| anyhow::anyhow!("invalid slash-command dispatch identity"))?;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command.arg(invocation.command).args(invocation.prefix_args);
    command.args(split_hook_words(raw)?);
    let status = command.status()?;
    Ok(status.code().unwrap_or(1).clamp(0, u8::MAX as i32) as u8)
}

fn split_hook_words(input: &str) -> anyhow::Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaped = false;
    let mut started = false;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if escaped {
            word.push(character);
            escaped = false;
            started = true;
        } else if character == '\\' && quote != Some('\'') {
            if quote == Some('"')
                && !characters
                    .peek()
                    .is_some_and(|next| matches!(*next, '$' | '`' | '"' | '\\' | '\n'))
            {
                word.push(character);
            } else {
                escaped = true;
            }
            started = true;
        } else if matches!(character, '\'' | '"') {
            if quote == Some(character) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(character);
                started = true;
            } else {
                word.push(character);
            }
        } else if character.is_whitespace() && quote.is_none() {
            if started {
                words.push(std::mem::take(&mut word));
                started = false;
            }
        } else {
            word.push(character);
            started = true;
        }
    }
    if escaped || quote.is_some() {
        anyhow::bail!("invalid slash-command arguments: unterminated quote or escape");
    }
    if started {
        words.push(word);
    }
    Ok(words)
}
