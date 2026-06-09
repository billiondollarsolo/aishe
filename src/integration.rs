//! Native shell integration.
//!
//! Instead of wrapping the user's shell in a PTY (fragile, and it would fight
//! the shell's own line editor), `llmsh` can run *inside* the user's real zsh or
//! bash as a `command_not_found` hook. That gives 100% native line editing — so
//! zsh-autosuggestions, zsh-syntax-highlighting, and oh-my-zsh all work for
//! real — while still routing anything that isn't a command to the LLM.
//!
//! The user adds `eval "$(llmsh init zsh)"` to their `~/.zshrc`. When they type
//! something that isn't a command, the hook asks `llmsh --suggest-line` for a
//! command and pushes it onto the editing buffer (`print -z`) for confirm/edit,
//! or, in yolo mode, runs the agentic loop directly.

/// Return the integration script for the named shell, or `None` if unsupported.
pub fn script(shell: &str) -> Option<&'static str> {
    match shell {
        "zsh" => Some(ZSH_SCRIPT),
        "bash" => Some(BASH_SCRIPT),
        _ => None,
    }
}

/// Shells we can emit an integration for.
pub const SUPPORTED: &[&str] = &["zsh", "bash"];

/// The zsh `command_not_found_handler` itself, reused by both `init zsh` and the
/// PTY wrapper's generated `.zshrc`.
pub const ZSH_HOOK: &str = r#"command_not_found_handler() {
  local line="${(j: :)@}"
  case "${LLMSH_MODE:-suggest}" in
    yolo)
      command llmsh --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    *)
      local cmd
      cmd="$(command llmsh --suggest-line "$line" 2> /dev/tty)"
      if [[ -n "$cmd" ]]; then
        print -z -- "$cmd"
      fi
      return 0
      ;;
  esac
}
"#;

const ZSH_SCRIPT: &str = r#"# llmsh zsh integration — add to ~/.zshrc:  eval "$(llmsh init zsh)"
# Routes unknown input to llmsh. Native ZLE (autosuggestions, syntax
# highlighting, oh-my-zsh) is untouched and works as usual.
# Set LLMSH_MODE=suggest|auto|yolo to control behavior (default: suggest).
command_not_found_handler() {
  local line="${(j: :)@}"
  case "${LLMSH_MODE:-suggest}" in
    yolo)
      command llmsh --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    *)
      local cmd
      cmd="$(command llmsh --suggest-line "$line" 2> /dev/tty)"
      if [[ -n "$cmd" ]]; then
        print -z -- "$cmd"
      fi
      return 0
      ;;
  esac
}
"#;

/// `.zshenv` for the PTY wrapper's isolated ZDOTDIR: source the user's real
/// zshenv, then force ZDOTDIR back to ours so our `.zshrc` loads next.
pub const WRAPPER_ZSHENV: &str = r#"# llmsh PTY wrapper (.zshenv) — generated
[ -f "${LLMSH_REAL_ZDOTDIR}/.zshenv" ] && source "${LLMSH_REAL_ZDOTDIR}/.zshenv"
export ZDOTDIR="${LLMSH_OUR_ZDOTDIR}"
"#;

/// `.zshrc` for the PTY wrapper: source the user's real interactive config (all
/// plugins), restore the real ZDOTDIR for the session, then append the AI hook.
pub fn wrapper_zshrc() -> String {
    format!(
        r#"# llmsh PTY wrapper (.zshrc) — generated
[ -f "${{LLMSH_REAL_ZDOTDIR}}/.zshrc" ] && source "${{LLMSH_REAL_ZDOTDIR}}/.zshrc"
export ZDOTDIR="${{LLMSH_REAL_ZDOTDIR}}"
# --- llmsh AI hook (added last) ---
{ZSH_HOOK}"#
    )
}

const BASH_SCRIPT: &str = r#"# llmsh bash integration — add to ~/.bashrc:  eval "$(llmsh init bash)"
# Routes unknown input to llmsh. Bash has no `print -z`, so the suggested
# command is placed in the readline buffer via the PROMPT hook.
# Set LLMSH_MODE=suggest|auto|yolo to control behavior (default: suggest).
command_not_found_handle() {
  local line="$*"
  case "${LLMSH_MODE:-suggest}" in
    yolo)
      command llmsh --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    *)
      local cmd
      cmd="$(command llmsh --suggest-line "$line" 2> /dev/tty)"
      if [ -n "$cmd" ]; then
        # Pre-fill the next prompt with the suggested command.
        READLINE_LINE="$cmd"
        READLINE_POINT=${#cmd}
        export LLMSH_PENDING="$cmd"
        bind '"\C-x\C-r": "\C-a\C-k$LLMSH_PENDING"' 2>/dev/null
      fi
      return 0
      ;;
  esac
}
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
        assert!(s.contains("LLMSH_MODE"));
    }

    #[test]
    fn bash_script_has_handle() {
        let s = script("bash").unwrap();
        assert!(s.contains("command_not_found_handle"));
        assert!(s.contains("--suggest-line"));
    }

    #[test]
    fn unsupported_shell_is_none() {
        assert!(script("fish").is_none());
    }

    #[test]
    fn wrapper_files_source_user_config_and_add_hook() {
        assert!(WRAPPER_ZSHENV.contains("LLMSH_REAL_ZDOTDIR"));
        assert!(WRAPPER_ZSHENV.contains("export ZDOTDIR=\"${LLMSH_OUR_ZDOTDIR}\""));
        let rc = wrapper_zshrc();
        assert!(rc.contains("${LLMSH_REAL_ZDOTDIR}/.zshrc"));
        // Restores the real ZDOTDIR and appends the command_not_found hook.
        assert!(rc.contains("export ZDOTDIR=\"${LLMSH_REAL_ZDOTDIR}\""));
        assert!(rc.contains("command_not_found_handler"));
        assert!(rc.contains("print -z"));
    }
}
