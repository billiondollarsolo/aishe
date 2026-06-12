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
: ${AISHE_FORCE_FILE:=${TMPDIR:-/tmp}/aishe-force-$$}
# Conversation memory shared across the per-call NL invocations in this shell, so
# follow-ups keep context. Exported so `command aishe` inherits it.
: ${AISHE_SESSION_FILE:=${TMPDIR:-/tmp}/aishe-session-mem-$$}
export AISHE_SESSION_FILE

# Route one natural-language line according to AISHE_MODE. suggest/auto stage a
# command in AISHE_PENDING_FILE (acted on by aishe_precmd in the MAIN shell,
# where print -z / cd / export work); yolo runs its loop inline.
_aishe_handle_nl() {
  local line="$1"
  [[ -z "$line" ]] && return
  case "${AISHE_MODE:-suggest}" in
    yolo)
      command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
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
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      [[ -n "$cmd" ]] && printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
      ;;
  esac
}

# Unknown command: zsh forks a SUBSHELL for this, so it stages via the temp file.
command_not_found_handler() {
  _aishe_handle_nl "${(j: :)@}"
  return 0
}

# Stage a line for the AI; the next aishe_precmd (MAIN shell) routes it.
_aishe_force_nl() { printf '%s' "$1" > "$AISHE_FORCE_FILE"; }

# Runs in the MAIN shell before each prompt: route a forced-NL line (from the
# sigil or key), then act on a staged command.
aishe_precmd() {
  if [[ -f "$AISHE_FORCE_FILE" ]]; then
    local fline
    fline="$(cat "$AISHE_FORCE_FILE")"
    command rm -f "$AISHE_FORCE_FILE"
    _aishe_handle_nl "$fline"
  fi
  [[ -f "$AISHE_PENDING_FILE" ]] || return
  local action cmd
  action="$(head -n 1 "$AISHE_PENDING_FILE")"
  cmd="$(tail -n +2 "$AISHE_PENDING_FILE")"
  command rm -f "$AISHE_PENDING_FILE"
  [[ -z "$cmd" ]] && return
  case "$action" in
    run)
      # Only eval a syntactically valid command. If the model answered a question
      # with prose (or returned a malformed command), eval would print an ugly
      # parse error and pollute history; pre-fill it for review instead.
      if command zsh -nc -- "$cmd" 2>/dev/null; then
        print -s -- "$cmd"   # main shell: cd/export persist; record in history
        eval "$cmd"
      else
        print -z -- "$cmd"
      fi
      ;;
    *)  print -z -- "$cmd" ;;                 # pre-fill for confirm/edit
  esac
}

# Clean up this shell's per-shell temp files when the interactive shell exits, so
# they don't accumulate in $TMPDIR. Registered as a zshexit hook below.
aishe_zshexit() {
  command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE"
}

# Force-NL key: send the current line to the AI even if it starts with a real
# command. Default key Alt-Enter; override with AISHE_NL_KEY (a zsh bindkey seq).
aishe-nl-widget() {
  emulate -L zsh
  [[ -z "$BUFFER" ]] && return
  print -s -- "$BUFFER"   # keep the NL line in history (up-arrow recall)
  _aishe_force_nl "$BUFFER"
  BUFFER=""
  zle .accept-line
}

# accept-line wrapper: a line starting with `?` or `#` is natural language. Strip
# the sigil here (before zsh parses it, so the shell's comment/glob rules never
# apply) and force it to the AI; otherwise behave as before, chaining whatever
# accept-line widget was already installed so plugins keep working.
aishe-accept-line() {
  emulate -L zsh
  if [[ "$BUFFER" == [#?]* ]]; then
    print -s -- "$BUFFER"   # keep the NL line in history (up-arrow recall)
    local body="${BUFFER#[#?]}"
    body="${body# }"
    if [[ -n "$body" ]]; then
      _aishe_force_nl "$body"
      BUFFER=""
    fi
  fi
  zle "${_aishe_orig_accept_line:-.accept-line}"
}

# Mode-cycle key (default Shift-Tab; override with AISHE_MODE_KEY). Rotates
# AISHE_MODE suggest -> auto -> yolo -> suggest for the rest of the session, like
# Claude Code's Shift-Tab. The safety gate and yolo_confirm tier still apply, so
# this never bypasses confirmation; it only changes how the next NL line routes.
aishe-cycle-mode() {
  emulate -L zsh
  case "${AISHE_MODE:-suggest}" in
    suggest) AISHE_MODE=auto ;;
    auto)    AISHE_MODE=yolo ;;
    *)       AISHE_MODE=suggest ;;
  esac
  export AISHE_MODE
  # Repaint the branded prompt glyph if the PTY prompt function is loaded.
  (( $+functions[aishe_set_prompt] )) && aishe_set_prompt
  zle reset-prompt
  zle -M "aishe mode: ${AISHE_MODE}"
}
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  add-zsh-hook precmd aishe_precmd
  add-zsh-hook zshexit aishe_zshexit   # remove per-shell temp files on exit
  zle -N aishe-nl-widget
  bindkey "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget
  zle -N aishe-cycle-mode
  bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode
  # Wrap accept-line once, chaining any existing widget (plugin-friendly).
  if [[ "${widgets[accept-line]}" != "user:aishe-accept-line" ]]; then
    case "${widgets[accept-line]}" in
      user:*) zle -A accept-line aishe-orig-accept-line
              _aishe_orig_accept_line="aishe-orig-accept-line" ;;
      *)      _aishe_orig_accept_line=".accept-line" ;;
    esac
    zle -N accept-line aishe-accept-line
  fi
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
# to be treated as natural language, and Shift-Tab (or $AISHE_MODE_KEY) to cycle
# the mode for the session.
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
{ZSH_HOOK}
{PTY_PROMPT}"#
    )
}

/// Branded prompt for the PTY front-end only (never `init zsh`, which must leave
/// the user's prompt alone). Mirrors the reedline prompt: `<cwd> <glyph>`, where
/// the glyph reflects the mode and is green/red by the last exit code, with a
/// dim `model · mode` right prompt. Applied via a precmd hook added last so it
/// wins over a prompt that the user's config rebuilds each prompt. Honors
/// `AISHE_PTY_PROMPT` (set from the `pty_prompt` config option).
const PTY_PROMPT: &str = r#"# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
if [[ -o interactive && "${AISHE_PTY_PROMPT:-1}" == 1 ]]; then
  autoload -Uz add-zsh-hook
  aishe_set_prompt() {
    local glyph
    case "${AISHE_MODE:-suggest}" in
      yolo) glyph='⚡' ;;
      auto) glyph='»' ;;
      *)    glyph='❯' ;;
    esac
    PROMPT="%B%F{cyan}%~%f%b %(?.%F{green}.%F{red})${glyph}%f "
    RPROMPT="%F{244}${AISHE_MODEL:-} · ${AISHE_MODE:-suggest}%f"
  }
  add-zsh-hook precmd aishe_set_prompt
fi
"#;

const BASH_SCRIPT: &str = r#"# aishe bash integration — add to ~/.bashrc:  eval "$(aishe init bash)"
# Routes unknown input to aishe. Set AISHE_MODE=suggest|auto|yolo (default
# suggest). bash runs command_not_found_handle in a SUBSHELL, so it can't touch
# shell state directly — it writes a temp file that a PROMPT_COMMAND hook acts
# on in the main shell.
: ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}
# Conversation memory shared across the per-call NL invocations in this shell.
: ${AISHE_SESSION_FILE:=${TMPDIR:-/tmp}/aishe-session-mem-$$}
export AISHE_SESSION_FILE
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
    # Only eval a syntactically valid command (a question answered as prose, or a
    # malformed command, would otherwise print a parse error and pollute history).
    if command bash -nc "$cmd" 2>/dev/null; then
      history -s "$cmd"; eval "$cmd"
    else
      printf 'aishe suggests: %s  (Ctrl-X Ctrl-R to recall)\n' "$cmd"
      export AISHE_PENDING="$cmd"
      bind '"\C-x\C-r": "\C-a\C-k$AISHE_PENDING"' 2>/dev/null
    fi
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

# Remove this shell's per-shell temp files when it exits, so they don't pile up in
# $TMPDIR. Chain onto any existing EXIT trap (don't clobber it), and only install
# once so sourcing twice is safe.
__aishe_cleanup() {
  command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE"
}
__aishe_existing_exit_trap="$(trap -p EXIT)"
case "$__aishe_existing_exit_trap" in
  *__aishe_cleanup*) ;;
  '') trap '__aishe_cleanup' EXIT ;;
  *)  trap "__aishe_cleanup; ${__aishe_existing_exit_trap#trap -- \'}" EXIT ;;
esac
unset __aishe_existing_exit_trap

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

# Mode-cycle: Shift-Tab rotates AISHE_MODE suggest -> auto -> yolo -> suggest for
# the session (override the key by re-binding "\e[Z"). The next prompt reflects
# it; the safety gate and yolo_confirm tier still apply.
__aishe_cycle_mode() {
  case "${AISHE_MODE:-suggest}" in
    suggest) export AISHE_MODE=auto ;;
    auto)    export AISHE_MODE=yolo ;;
    *)       export AISHE_MODE=suggest ;;
  esac
  printf '\naishe mode: %s\n' "$AISHE_MODE"
}
bind -x '"\e[Z": __aishe_cycle_mode' 2>/dev/null
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
    fn zsh_script_has_mode_cycle_widget() {
        let s = script("zsh").unwrap();
        assert!(s.contains("aishe-cycle-mode"));
        assert!(s.contains("zle -N aishe-cycle-mode"));
        // Default key is Shift-Tab, overridable via AISHE_MODE_KEY.
        assert!(s.contains("${AISHE_MODE_KEY:-^[[Z}"));
        // It repaints and reports the new mode.
        assert!(s.contains("reset-prompt"));
        assert!(s.contains("aishe mode: "));
    }

    #[test]
    fn bash_script_has_mode_cycle_binding() {
        let s = script("bash").unwrap();
        assert!(s.contains("__aishe_cycle_mode"));
        assert!(s.contains(r#"bind -x '"\e[Z": __aishe_cycle_mode'"#));
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
    fn zsh_script_cleans_up_temp_files_on_exit() {
        // A zshexit hook removes this shell's per-shell temp files so they don't
        // pile up in $TMPDIR. It's registered alongside the precmd hook, under
        // the same interactive guard.
        let s = script("zsh").unwrap();
        assert!(s.contains("aishe_zshexit"));
        assert!(s.contains("add-zsh-hook zshexit aishe_zshexit"));
        assert!(s.contains(
            r#"command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE""#
        ));
    }

    #[test]
    fn bash_script_cleans_up_temp_files_on_exit() {
        // An EXIT trap removes this shell's per-shell temp files. It chains onto
        // any existing EXIT trap (so it doesn't clobber it) and only installs once.
        let s = script("bash").unwrap();
        assert!(s.contains("__aishe_cleanup"));
        assert!(s.contains("trap '__aishe_cleanup' EXIT"));
        assert!(s.contains(
            r#"command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE""#
        ));
    }

    #[test]
    fn zsh_script_has_nl_sigil() {
        // A leading `?` or `#` forces a line to the AI via an accept-line wrapper
        // that strips the sigil before zsh parses it, staged through the force
        // file and routed in the main shell.
        let s = script("zsh").unwrap();
        assert!(s.contains("aishe-accept-line"));
        assert!(s.contains("[#?]*")); // sigil match on the buffer
        assert!(s.contains("AISHE_FORCE_FILE"));
        assert!(s.contains("_aishe_handle_nl"));
        // accept-line is wrapped plugin-friendly (chains the prior widget).
        assert!(s.contains("zle -N accept-line aishe-accept-line"));
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
