//! Native shell integration.
//!
//! Instead of wrapping the user's shell in a PTY (fragile, and it would fight
//! the shell's own line editor), `aishe` can run *inside* the user's real zsh or
//! bash as a `command_not_found` hook. That gives 100% native line editing — so
//! zsh-autosuggestions, zsh-syntax-highlighting, and oh-my-zsh all work for
//! real — while still routing anything that isn't a command to the LLM.
//!
//! The user adds `eval "$(aishe init zsh)"` to their `~/.zshrc`. When they type
//! something that isn't a command, the hook asks `aishe --suggest-line` for a
//! command and pushes it onto the editing buffer (`print -z`) for confirm/edit;
//! in `auto` mode it runs safe commands directly via `eval` (zsh only — see
//! below); in `yolo` mode it runs the agentic loop directly.
//!
//! Two ergonomic extras (zsh):
//! - **auto-run safe via `eval`.** In `auto` mode the hook calls
//!   `aishe --auto-line`, which prints the command and exits `0` if the safety
//!   gate deems it safe (the hook `eval`s it in your real shell, so `cd`/`export`
//!   persist) or exits non-zero if dangerous (the hook pre-fills it for review).
//!   bash runs `command_not_found_handle` in a *subshell*, so eval'd state would
//!   not persist there — bash keeps the pre-fill path in auto mode.
//! - **force-NL keybinding.** A ZLE widget (default Alt-Enter, override with
//!   `AISHE_NL_KEY`) sends the current line to the LLM as natural language even
//!   when it is also a valid command. bash binds the same to `Ctrl-G`.

use std::borrow::Cow;

/// Return the integration script for the named shell, or `None` if unsupported.
pub fn script(shell: &str) -> Option<Cow<'static, str>> {
    match shell {
        "zsh" => Some(Cow::Owned(zsh_script())),
        "bash" => Some(Cow::Borrowed(BASH_SCRIPT)),
        _ => None,
    }
}

/// Shells we can emit an integration for.
pub const SUPPORTED: &[&str] = &["zsh", "bash"];

/// The zsh hook itself (handler + force-NL widget), reused by both `init zsh`
/// and the PTY wrapper's generated `.zshrc`. This is the single source of truth
/// for zsh behavior so the standalone `init` snippet and the PTY front-end never
/// drift apart.
pub const ZSH_HOOK: &str = r#"command_not_found_handler() {
  local line="${(j: :)@}"
  case "${AISHE_MODE:-suggest}" in
    yolo)
      command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    auto)
      # Ask aishe for a command. Exit 0 => safe: run it directly in this shell
      # (cd/export persist). Non-zero => dangerous: pre-fill for review instead.
      local cmd rc
      cmd="$(command aishe --auto-line "$line" 2> /dev/tty)"
      rc=$?
      if [[ -n "$cmd" ]]; then
        if (( rc == 0 )); then
          print -s -- "$cmd"
          eval "$cmd"
        else
          print -z -- "$cmd"
        fi
      fi
      return 0
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      if [[ -n "$cmd" ]]; then
        print -z -- "$cmd"
      fi
      return 0
      ;;
  esac
}

# Force-NL: treat the current line as natural language even if it's a valid
# command. Default key Alt-Enter; override with AISHE_NL_KEY (a zsh bindkey seq).
aishe-nl-widget() {
  emulate -L zsh
  local line="$BUFFER"
  [[ -z "$line" ]] && return
  local cmd
  cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
  if [[ -n "$cmd" ]]; then
    BUFFER="$cmd"
    CURSOR=${#BUFFER}
  fi
}
if [[ -o interactive ]]; then
  zle -N aishe-nl-widget
  bindkey "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget
fi
"#;

/// The full `init zsh` snippet: header comment + the shared hook.
pub fn zsh_script() -> String {
    format!(
        r#"# aishe zsh integration — add to ~/.zshrc:  eval "$(aishe init zsh)"
# Routes unknown input to aishe. Native ZLE (autosuggestions, syntax
# highlighting, oh-my-zsh) is untouched and works as usual.
# Set AISHE_MODE=suggest|auto|yolo to control behavior (default: suggest).
# In auto mode, safe commands run directly (cd/export persist); dangerous ones
# are pre-filled for review. Press Alt-Enter (or $AISHE_NL_KEY) to force a line
# to be treated as natural language.
{ZSH_HOOK}"#
    )
}

/// `.zshenv` for the PTY wrapper's isolated ZDOTDIR: source the user's real
/// zshenv, then force ZDOTDIR back to ours so our `.zshrc` loads next.
pub const WRAPPER_ZSHENV: &str = r#"# aishe PTY wrapper (.zshenv) — generated
[ -f "${AISHE_REAL_ZDOTDIR}/.zshenv" ] && source "${AISHE_REAL_ZDOTDIR}/.zshenv"
export ZDOTDIR="${AISHE_OUR_ZDOTDIR}"
"#;

/// `.zshrc` for the PTY wrapper: source the user's real interactive config (all
/// plugins), restore the real ZDOTDIR for the session, then append the AI hook.
pub fn wrapper_zshrc() -> String {
    format!(
        r#"# aishe PTY wrapper (.zshrc) — generated
[ -f "${{AISHE_REAL_ZDOTDIR}}/.zshrc" ] && source "${{AISHE_REAL_ZDOTDIR}}/.zshrc"
export ZDOTDIR="${{AISHE_REAL_ZDOTDIR}}"
# --- aishe AI hook (added last) ---
{ZSH_HOOK}"#
    )
}

const BASH_SCRIPT: &str = r#"# aishe bash integration — add to ~/.bashrc:  eval "$(aishe init bash)"
# Routes unknown input to aishe. Bash has no `print -z`, so the suggested
# command is placed in the readline buffer via the PROMPT hook.
# Set AISHE_MODE=suggest|auto|yolo to control behavior (default: suggest).
command_not_found_handle() {
  local line="$*"
  case "${AISHE_MODE:-suggest}" in
    yolo)
      command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    *)
      # suggest and auto both pre-fill here. bash runs command_not_found_handle
      # in a subshell, so eval'd state (cd/export) would not persist — pre-fill
      # is the honest path. Use Ctrl-X Ctrl-R to recall the suggestion.
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      if [ -n "$cmd" ]; then
        # Pre-fill the next prompt with the suggested command.
        READLINE_LINE="$cmd"
        READLINE_POINT=${#cmd}
        export AISHE_PENDING="$cmd"
        bind '"\C-x\C-r": "\C-a\C-k$AISHE_PENDING"' 2>/dev/null
      fi
      return 0
      ;;
  esac
}

# Force-NL: Ctrl-G turns the current line into an aishe suggestion even if it is
# a valid command. `bind -x` runs in the current shell, so READLINE_LINE sticks.
__aishe_nl() {
  local line="$READLINE_LINE"
  [ -z "$line" ] && return
  local cmd
  cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
  if [ -n "$cmd" ]; then
    READLINE_LINE="$cmd"
    READLINE_POINT=${#cmd}
  fi
}
bind -x '"\C-g": __aishe_nl' 2>/dev/null
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_script_has_handler_and_print_z() {
        let s = script("zsh").unwrap();
        assert!(s.contains("command_not_found_handler"));
        assert!(s.contains("print -z"));
        assert!(s.contains("--suggest-line"));
        assert!(s.contains("--yolo-line"));
        assert!(s.contains("AISHE_MODE"));
    }

    #[test]
    fn zsh_script_has_auto_eval_path() {
        let s = script("zsh").unwrap();
        assert!(s.contains("--auto-line"));
        assert!(s.contains("eval \"$cmd\""));
        // history record so eval'd commands show up in history.
        assert!(s.contains("print -s"));
    }

    #[test]
    fn zsh_script_has_force_nl_widget() {
        let s = script("zsh").unwrap();
        assert!(s.contains("aishe-nl-widget"));
        assert!(s.contains("zle -N aishe-nl-widget"));
        assert!(s.contains("AISHE_NL_KEY"));
        // zle/bindkey must be guarded so sourcing non-interactively is safe.
        assert!(s.contains("[[ -o interactive ]]"));
    }

    #[test]
    fn bash_script_has_handle_and_force_nl() {
        let s = script("bash").unwrap();
        assert!(s.contains("command_not_found_handle"));
        assert!(s.contains("--suggest-line"));
        assert!(s.contains("__aishe_nl"));
        assert!(s.contains("bind -x"));
    }

    #[test]
    fn unsupported_shell_is_none() {
        assert!(script("fish").is_none());
    }

    #[test]
    fn wrapper_files_source_user_config_and_add_hook() {
        assert!(WRAPPER_ZSHENV.contains("AISHE_REAL_ZDOTDIR"));
        assert!(WRAPPER_ZSHENV.contains("export ZDOTDIR=\"${AISHE_OUR_ZDOTDIR}\""));
        let rc = wrapper_zshrc();
        assert!(rc.contains("${AISHE_REAL_ZDOTDIR}/.zshrc"));
        // Restores the real ZDOTDIR and appends the command_not_found hook.
        assert!(rc.contains("export ZDOTDIR=\"${AISHE_REAL_ZDOTDIR}\""));
        assert!(rc.contains("command_not_found_handler"));
        assert!(rc.contains("print -z"));
        // The wrapper gets the force-NL widget too (shared ZSH_HOOK).
        assert!(rc.contains("aishe-nl-widget"));
    }
}
