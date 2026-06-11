//! Native shell integration.
//!
//! Instead of wrapping the user's shell in a PTY (fragile, and it would fight
//! the shell's own line editor), `aishe` can run *inside* the user's real zsh or
//! bash as a `command_not_found` hook. That gives 100% native line editing — so
//! zsh-autosuggestions, zsh-syntax-highlighting, and oh-my-zsh all work for
//! real — while still routing anything that isn't a command to the LLM.
//!
//! The user adds `eval "$(aishe init zsh)"` to their `~/.zshrc`. When they type
//! something that isn't a command, the hook asks `aishe` for a command.
//!
//! **Subshell handoff (important).** zsh (like bash) runs
//! `command_not_found_handler` in a *subshell* (`$ZSH_SUBSHELL > 0`), so it
//! cannot touch the line editor (`print -z`) or shell state (`cd`/`export`)
//! directly — those changes are discarded. Instead the handler writes the
//! intended action + command to a per-shell temp file (`$AISHE_PENDING_FILE`,
//! which survives the subshell), and a `precmd` hook — running in the *main*
//! shell before the next prompt — acts on it:
//! - **suggest**: `print -z` the command onto the editing buffer (confirm/edit).
//! - **auto**: `eval` a safe command in the main shell (so `cd`/`export` persist
//!   and it's recorded in history), or `print -z` a dangerous one for review.
//! - **yolo**: the handler runs `aishe --yolo-line` inline (its side effects and
//!   tty output survive the subshell; it manages its own commands).
//!
//! - **force-NL keybinding.** A ZLE widget (default Alt-Enter, override with
//!   `AISHE_NL_KEY`) runs in a real widget (so `BUFFER` works) and replaces the
//!   line with an LLM suggestion. bash binds the same to `Ctrl-G`.

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
pub const ZSH_HOOK: &str = r#": ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}

# Runs in a SUBSHELL (zsh forks for command-not-found), so it cannot use
# `print -z`/`cd`/`export` directly — it writes the action+command to a temp
# file that survives the subshell; aishe_precmd (main shell) acts on it.
command_not_found_handler() {
  local line="${(j: :)@}"
  case "${AISHE_MODE:-suggest}" in
    yolo)
      command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    auto)
      local cmd rc
      cmd="$(command aishe --auto-line "$line" 2> /dev/tty)"
      rc=$?
      if [[ -n "$cmd" ]]; then
        # exit 0 => safe (run it); non-zero => dangerous (pre-fill for review)
        if (( rc == 0 )); then
          printf 'run\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
        else
          printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
        fi
      fi
      return 0
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      [[ -n "$cmd" ]] && printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
      return 0
      ;;
  esac
}

# Runs in the MAIN shell before each prompt: act on a pending command.
aishe_precmd() {
  [[ -f "$AISHE_PENDING_FILE" ]] || return
  local action cmd
  action="$(head -n 1 "$AISHE_PENDING_FILE")"
  cmd="$(tail -n +2 "$AISHE_PENDING_FILE")"
  command rm -f "$AISHE_PENDING_FILE"
  [[ -z "$cmd" ]] && return
  case "$action" in
    run)  print -s -- "$cmd"; eval "$cmd" ;;  # main shell: cd/export persist
    *)    print -z -- "$cmd" ;;               # pre-fill for confirm/edit
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
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd aishe_precmd
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
# Routes unknown input to aishe. Set AISHE_MODE=suggest|auto|yolo (default
# suggest). bash runs command_not_found_handle in a SUBSHELL, so it can't touch
# shell state directly — it writes a temp file that a PROMPT_COMMAND hook acts
# on in the main shell.
: ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}
command_not_found_handle() {
  local line="$*"
  case "${AISHE_MODE:-suggest}" in
    yolo)
      command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    auto)
      local cmd rc
      cmd="$(command aishe --auto-line "$line" 2> /dev/tty)"
      rc=$?
      if [ -n "$cmd" ]; then
        if [ "$rc" -eq 0 ]; then printf 'run\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
        else printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"; fi
      fi
      return 0
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      [ -n "$cmd" ] && printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
      return 0
      ;;
  esac
}

# Main-shell hook: run a safe auto command (state persists), or offer a
# suggestion. (readline can't be reliably pre-filled from PROMPT_COMMAND, so a
# suggestion is printed and stashed; recall it with Ctrl-X Ctrl-R.)
__aishe_prompt() {
  [ -f "$AISHE_PENDING_FILE" ] || return
  local action cmd
  action="$(head -n 1 "$AISHE_PENDING_FILE")"
  cmd="$(tail -n +2 "$AISHE_PENDING_FILE")"
  command rm -f "$AISHE_PENDING_FILE"
  [ -z "$cmd" ] && return
  if [ "$action" = run ]; then
    history -s "$cmd"; eval "$cmd"
  else
    printf 'aishe suggests: %s  (Ctrl-X Ctrl-R to recall)\n' "$cmd"
    export AISHE_PENDING="$cmd"
    bind '"\C-x\C-r": "\C-a\C-k$AISHE_PENDING"' 2>/dev/null
  fi
}
case ":${PROMPT_COMMAND}:" in
  *__aishe_prompt*) ;;
  *) PROMPT_COMMAND="__aishe_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac

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
    fn zsh_script_uses_precmd_handoff() {
        // The handler runs in a subshell, so it must hand off via a temp file to
        // a precmd hook (which runs in the main shell where print -z/eval work).
        let s = script("zsh").unwrap();
        assert!(s.contains("AISHE_PENDING_FILE"));
        assert!(s.contains("aishe_precmd"));
        assert!(s.contains("add-zsh-hook precmd aishe_precmd"));
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
        // subshell handoff: handler writes a file, PROMPT_COMMAND acts on it.
        assert!(s.contains("AISHE_PENDING_FILE"));
        assert!(s.contains("__aishe_prompt"));
        assert!(s.contains("PROMPT_COMMAND"));
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
