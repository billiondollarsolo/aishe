# aishe PTY wrapper (.zshrc) — generated
[ -f "${AISHE_REAL_ZDOTDIR}/.zshrc" ] && source "${AISHE_REAL_ZDOTDIR}/.zshrc"
export ZDOTDIR="${AISHE_REAL_ZDOTDIR}"

# Preserve the user's zsh/Oh My Zsh history configuration when it exists. On a
# minimal account zsh defaults to HISTFILE unset and SAVEHIST=0, which otherwise
# makes Up-arrow/Ctrl-R history disappear whenever the aishe session exits. In
# that case, use aishe's existing timestamped log as zsh's native history file.
# It lives in the user data directory, so replacing the aishe binary never
# removes it. SHARE_HISTORY makes concurrent sessions exchange entries.
if [[ -z "${HISTFILE:-}" && -n "${AISHE_HISTFILE:-}" ]]; then
  HISTFILE="${AISHE_HISTFILE}"
  HISTSIZE=20000
  SAVEHIST=10000
  setopt EXTENDED_HISTORY APPEND_HISTORY
  if [[ "${AISHE_SHARE_HISTORY:-1}" == 1 ]]; then
    setopt SHARE_HISTORY
  else
    unsetopt SHARE_HISTORY
  fi
  AISHE_MANAGED_HISTFILE=1
fi

# --- aishe AI hook (added last) ---
# __AISHE_TEMPLATE_ZSH_HOOK__
# __AISHE_TEMPLATE_PTY_PROMPT__
if [[ -z "${AISHE_COMMAND_HINT_SHOWN:-}" ]]; then
  print -r -- '__AISHE_TEMPLATE_ASCII_LOGO__'
  print -P "%F{244}aishe: /help · /connection · /model · Shift-Tab mode · Ctrl-O details · ask \"how do I…\"%f"
  export AISHE_COMMAND_HINT_SHOWN=1
fi
