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
    auto)
      local cmd
      cmd="$(command llmsh --suggest-line "$line" 2> /dev/tty)"
      if [[ -n "$cmd" ]]; then
        print -z -- "$cmd"
      fi
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
}
