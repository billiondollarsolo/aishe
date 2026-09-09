# aishe lean PTY hook (.zshrc) — generated
# Isolated ZDOTDIR. NEVER source ~/.zshrc, $AISHE_REAL_ZDOTDIR, or plugin stacks.
# Known commands stay in this zsh. NL goes to the parent over a FIFO (no `aishe` spawn).

unsetopt GLOBAL_RCS 2>/dev/null || true

if [[ -n "${AISHE_HISTFILE:-}" ]]; then
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

: ${AISHE_MODE:=ask}
export AISHE_MODE
export AISHE_LEAN=1

_aishe_lean_glyph() {
  if [[ "${AISHE_UNICODE:-unicode}" == ascii ]]; then
    case "${AISHE_MODE:-ask}" in
      agent|yolo) print -r -- '*' ;;
      allow|auto) print -r -- '>>' ;;
      *)          print -r -- '>' ;;
    esac
  else
    case "${AISHE_MODE:-ask}" in
      agent|yolo) print -r -- '*' ;;
      allow|auto) print -r -- '»' ;;
      *)          print -r -- '❯' ;;
    esac
  fi
}

aishe_set_prompt() {
  local glyph
  glyph="$(_aishe_lean_glyph)"
  PROMPT="%1~ ${glyph} "
  RPROMPT="${AISHE_MODE:-ask}"
}

# Tiny owned prompt. No theme compatibility matrix.
if [[ -o interactive ]]; then
  aishe_set_prompt
fi

# Optional tiny rc the user chose for this shell. NEVER ~/.zshrc by default.
if [[ -n "${AISHE_LEANRC:-}" && -r "${AISHE_LEANRC}" ]]; then
  source "${AISHE_LEANRC}"
elif [[ -r "${HOME}/.aishe/leanrc" ]]; then
  source "${HOME}/.aishe/leanrc"
fi

# __AISHE_GENERATED_QUESTION_GRAMMAR__

_aishe_has_assignment_head() {
  emulate -L zsh
  setopt extendedglob
  local line="$1" prefix base
  [[ "$line" == *'='* ]] || return 1
  prefix="${line%%=*}"
  [[ -n "$prefix" && "$prefix" != *[[:space:]]* ]] || return 1
  base="${prefix%%\[*}"
  base="${base%+}"
  [[ "$base" == [[:alpha:]_][[:alnum:]_]# ]]
}

# Local classifier: ? / ! / PATH-known / NL. Never starts AIShe, a provider, or OpenCode.
_aishe_routes_to_agent() {
  emulate -L zsh
  setopt extendedglob
  local line="${1##[[:space:]]#}"
  line="${line%%[[:space:]]#}"
  [[ -n "$line" ]] || return 1
  [[ "${CONTEXT:-start}" == start ]] || return 1
  [[ "$line" == '?'* ]] && return 0
  [[ "$line" == *$'\n'* ]] && return 1
  [[ "$line" == '!'* ]] && return 1
  [[ "$line" == /* && "$line" != //* ]] && {
    local slash="${line%%[[:space:]]*}"
    case "$slash" in
      /help|/mode|/status|/reset|/undo|/usage|/details|/model) return 1 ;;
    esac
  }

  if [[ "$line" == ./* || "$line" == ../* || "$line" == /* ||
        "$line" == '~/'* || "$line" == '$('* || "$line" == '('* ]]; then
    return 1
  fi
  if [[ "$line" == [[:alpha:]_][[:alnum:]_-]#'()'* ||
        "$line" == [[:alpha:]_][[:alnum:]_-]#' ()'* ||
        "$line" == 'function '[[:alpha:]_]* ]]; then
    return 1
  fi
  local head="${line%%[[:space:]]*}"
  case "$head" in
    if|for|while|until|case|select|function|time|repeat|'[['|'(('|'{' ) return 1 ;;
  esac
  _aishe_has_assignment_head "$line" && return 1
  _aishe_looks_like_question "$line" && return 0

  local -a words
  words=(${(z)line}) 2>/dev/null || return 1
  local word
  local expect_head=1
  for word in "${words[@]}"; do
    if (( expect_head )); then
      if [[ "$word" == [[:alpha:]_][[:alnum:]_]#=* ]]; then
        continue
      fi
      case "$word" in
        '('|'{'|'[['|'((') return 1 ;;
      esac
      whence -w -- "$word" > /dev/null 2>&1 || return 0
      expect_head=0
      continue
    fi
    case "$word" in
      '|'|'||'|'&&'|';') expect_head=1 ;;
    esac
  done
  return 1
}

_aishe_lean_flatten() {
  local s="$1"
  s="${s//$'\t'/ }"
  s="${s//$'\n'/ }"
  print -r -- "$s"
}

_aishe_lean_send() {
  emulate -L zsh
  local payload="$1" reply
  [[ -n "${AISHE_LEAN_REQ:-}" && -p "${AISHE_LEAN_REQ}" &&
     -n "${AISHE_LEAN_REP:-}" && -p "${AISHE_LEAN_REP}" ]] || {
    print -u2 -- 'aishe: lean IPC is not connected'
    return 1
  }
  print -r -- "$payload" > "$AISHE_LEAN_REQ" || return 1
  IFS= read -r -t 120 reply < "$AISHE_LEAN_REP" || {
    print -u2 -- 'aishe: lean NL timed out'
    return 1
  }
  print -r -- "$reply"
}

_aishe_lean_grant_needed() {
  case "${AISHE_MODE:-ask}" in
    allow|auto)
      [[ -n "${AISHE_GRANT:-}" ]] && return 1
      [[ -n "${AISHE_ACCEPTANCE_FILE:-}" && -r "${AISHE_ACCEPTANCE_FILE}" ]] &&
        grep -qx 'allow' "$AISHE_ACCEPTANCE_FILE" 2>/dev/null && return 1
      return 0
      ;;
    agent|yolo)
      [[ "${AISHE_GRANT:-}" == agent || "${AISHE_GRANT:-}" == agent-host ]] && return 1
      [[ -n "${AISHE_ACCEPTANCE_FILE:-}" && -r "${AISHE_ACCEPTANCE_FILE}" ]] &&
        grep -Eqx 'agent|agent-host|workspace|host' "$AISHE_ACCEPTANCE_FILE" 2>/dev/null && return 1
      return 0
      ;;
  esac
  return 1
}

_aishe_lean_take_grant() {
  emulate -L zsh
  local want="$1"
  print -r -- ""
  case "$want" in
    allow)
      print -r -- "Enter allow · tools for this shell?"
      print -r -- "Safe commands run. Dangerous / unknown require typing yes."
      ;;
    agent)
      print -r -- "Enter agent · workspace?"
      print -r -- "The agent may run commands and change files in this workspace without asking again."
      print -r -- "Linux workspace requires functional bubblewrap."
      ;;
    agent-host)
      print -r -- "Enter agent-host · host?"
      print -r -- "The agent may execute any command available to your user without asking again."
      ;;
  esac
  print -n -- "Type ${want} to continue: "
  local ans
  if [[ -n "${_AISHE_INPUT_FD:-}" && "_AISHE_INPUT_FD" -ge 0 ]]; then
    IFS= read -r ans <&$_AISHE_INPUT_FD
  else
    IFS= read -r ans
  fi
  [[ "${ans}" == "$want" ]] || {
    print -r -- "grant declined · mode stays ${AISHE_MODE:-ask}"
    return 1
  }
  AISHE_GRANT="$want"
  export AISHE_GRANT
  if [[ -n "${AISHE_ACCEPTANCE_FILE:-}" ]]; then
    print -r -- "$want" > "$AISHE_ACCEPTANCE_FILE"
  fi
  return 0
}

_aishe_lean_handle_reply() {
  emulate -L zsh
  local reply="$1"
  local kind="${reply%%	*}"
  local rest="${reply#*$'\t'}"
  [[ "$kind" == "$reply" ]] && rest=""
  case "$kind" in
    ANSWER)
      [[ -n "$rest" ]] && print -r -- "$rest"
      ;;
    FILL)
      typeset -g _AISHE_STAGED_SUGGESTION=1
      print -z -- "$rest"
      ;;
    RAN)
      [[ -n "$rest" ]] && print -r -- "$rest"
      ;;
    ERROR)
      print -u2 -- "aishe: $rest"
      ;;
    CONFIRM)
      print -r -- "Dangerous / unknown: $rest"
      print -n -- "Type yes to run: "
      local ans
      if [[ -n "${_AISHE_INPUT_FD:-}" && "_AISHE_INPUT_FD" -ge 0 ]]; then
        IFS= read -r ans <&$_AISHE_INPUT_FD
      else
        IFS= read -r ans
      fi
      if [[ "$ans" == yes ]]; then
        local again
        again="$(_aishe_lean_send "CONFIRM_YES	$(_aishe_lean_flatten "$rest")")"
        _aishe_lean_handle_reply "$again"
      else
        print -r -- "cancelled"
      fi
      ;;
    *)
      [[ -n "$reply" ]] && print -r -- "$reply"
      ;;
  esac
}

_aishe_lean_nl() {
  emulate -L zsh
  local line="$1"
  [[ -z "$line" ]] && return
  if _aishe_lean_grant_needed; then
    case "${AISHE_MODE:-ask}" in
      allow|auto) _aishe_lean_take_grant allow || return ;;
      agent|yolo) _aishe_lean_take_grant agent || return ;;
    esac
  fi
  local payload reply
  payload="NL	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$line")"
  reply="$(_aishe_lean_send "$payload")" || return
  _aishe_lean_handle_reply "$reply"
}

_aishe_lean_slash() {
  emulate -L zsh
  local line="$1"
  local name="${line%%[[:space:]]*}"
  local arg="${line#"$name"}"
  arg="${arg##[[:space:]]#}"
  case "$name" in
    /help)
      print -r -- 'aishe lean: typed commands run in zsh -f. English goes to the model.'
      print -r -- '  ? force NL   ! force shell   Ctrl-X ? show route'
      print -r -- '  /mode ask|allow|agent   Shift-Tab cycles mode'
      print -r -- '  Default mode is ask. allow/agent need one typed grant per shell.'
      ;;
    /mode)
      if [[ -z "$arg" ]]; then
        print -r -- "mode: ${AISHE_MODE:-ask}"
        return
      fi
      case "$arg" in
        ask|suggest) AISHE_MODE=ask ;;
        allow|auto)
          _aishe_lean_take_grant allow || return
          AISHE_MODE=allow
          ;;
        agent|yolo)
          _aishe_lean_take_grant agent || return
          AISHE_MODE=agent
          ;;
        agent-host)
          _aishe_lean_take_grant agent-host || return
          AISHE_MODE=agent
          AISHE_GRANT=agent-host
          export AISHE_GRANT
          ;;
        *)
          print -u2 -- "aishe: unknown mode '$arg' (ask|allow|agent)"
          return
          ;;
      esac
      export AISHE_MODE
      aishe_set_prompt
      print -r -- "mode: ${AISHE_MODE}"
      ;;
    /status|/reset|/undo|/usage|/details|/model)
      local reply
      reply="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$line")")" || return
      _aishe_lean_handle_reply "$reply"
      ;;
    *)
      return 1
      ;;
  esac
  return 0
}

# Unknown command: do not spawn aishe. Route the accepted line as NL.
command_not_found_handler() {
  local line="${(j: :)@}"
  [[ -n "${_AISHE_ACCEPTED_LINE:-}" && "$line" == "$_AISHE_ACCEPTED_LINE" ]] || return 127
  _aishe_lean_nl "$line"
  return 0
}

_aishe_capture_exit() {
  AISHE_LAST_EXIT=$?
  typeset -g _AISHE_ACCEPTED_LINE=""
}
_aishe_capture_cmd() {
  typeset -g _AISHE_STAGED_SUGGESTION=""
  AISHE_LAST_CMD="$1"
  typeset -g _AISHE_ACCEPTED_LINE="$1"
}

aishe-show-route() {
  emulate -L zsh
  if [[ -z "$BUFFER" ]]; then
    zle -M "aishe route: empty · type a line first"
  elif _aishe_routes_to_agent "$BUFFER"; then
    zle -M "aishe route: agent · ! forces this line to shell"
  else
    zle -M "aishe route: shell · ? forces this line to agent"
  fi
}

aishe-nl-widget() {
  emulate -L zsh
  [[ -z "$BUFFER" ]] && return
  local submitted="$BUFFER"
  print -s -- "$submitted"
  zle -I
  _aishe_lean_nl "$submitted"
  BUFFER=""
  POSTDISPLAY="$submitted"
  zle .accept-line
}

aishe-cycle-mode() {
  emulate -L zsh
  if [[ -n "$BUFFER" ]]; then
    zle "${_AISHE_ORIG_MODE_WIDGET:-reverse-menu-complete}" 2>/dev/null || true
    return
  fi
  zle -I
  case "${AISHE_MODE:-ask}" in
    ask|suggest)
      _aishe_lean_take_grant allow || { zle reset-prompt; return }
      AISHE_MODE=allow
      ;;
    allow|auto)
      _aishe_lean_take_grant agent || { zle reset-prompt; return }
      AISHE_MODE=agent
      ;;
    *)
      AISHE_MODE=ask
      ;;
  esac
  export AISHE_MODE
  aishe_set_prompt
  zle reset-prompt
}

aishe-accept-line() {
  emulate -L zsh
  local line="$BUFFER"
  local trimmed="${line##[[:space:]]#}"
  trimmed="${trimmed%%[[:space:]]#}"

  if [[ "$trimmed" == '!'* ]]; then
    BUFFER="${trimmed#\!}"
    BUFFER="${BUFFER##[[:space:]]#}"
    zle .accept-line
    return
  fi

  if [[ "$trimmed" == /* && "$trimmed" != //* ]]; then
    if _aishe_lean_slash "$trimmed"; then
      print -s -- "$trimmed"
      BUFFER=""
      POSTDISPLAY="$trimmed"
      zle .accept-line
      return
    fi
  fi

  if _aishe_routes_to_agent "$trimmed"; then
    local body="$trimmed"
    [[ "${body[1]}" == '?' ]] && body="${body#?}"
    body="${body##[[:space:]]#}"
    print -s -- "$trimmed"
    if [[ -n "$body" ]]; then
      zle -I
      _aishe_lean_nl "$body"
    fi
    BUFFER=""
    POSTDISPLAY="$trimmed"
    zle .accept-line
    return
  fi

  zle .accept-line
}

_aishe_highlight_command() {
  emulate -L zsh
  setopt extendedglob
  local -a kept
  local spec
  for spec in "${region_highlight[@]}"; do
    case "$spec" in
      <->\ <->\ fg=green,bold|<->\ <->\ fg=magenta,bold|<->\ <->\ fg=cyan,bold) ;;
      *) kept+=("$spec") ;;
    esac
  done
  region_highlight=("${kept[@]}")
  [[ "${AISHE_COMMAND_HIGHLIGHT:-1}" != 0 && -n "$BUFFER" ]] || return 0
  if _aishe_routes_to_agent "$BUFFER"; then
    region_highlight+=("0 ${#BUFFER} fg=magenta,bold")
    return 0
  fi
  local leading="${BUFFER%%[^[:space:]]*}"
  local rest="${BUFFER#$leading}"
  local head="${rest%%[[:space:]]*}"
  if [[ "$head" == /help || "$head" == /mode || "$head" == /status ||
        "$head" == /reset || "$head" == /undo || "$head" == /usage ||
        "$head" == /details || "$head" == /model ]]; then
    local slash_start=${#leading}
    local slash_end=$(( slash_start + ${#head} ))
    region_highlight+=("$slash_start $slash_end fg=cyan,bold")
    return 0
  fi
  [[ "$head" == [[:alnum:]_./+-]## ]] || return 0
  whence -w -- "$head" >/dev/null 2>&1 || return 0
  local start=${#leading}
  local end=$(( start + ${#head} ))
  region_highlight+=("$start $end fg=green,bold")
}

if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  if [[ -z "${_AISHE_INPUT_FD:-}" ]]; then
    typeset -gi _AISHE_INPUT_FD=-1
    exec {_AISHE_INPUT_FD}<&0
  fi
  add-zsh-hook precmd aishe_set_prompt
  add-zsh-hook precmd _aishe_capture_exit
  precmd_functions=(_aishe_capture_exit ${precmd_functions:#_aishe_capture_exit})
  add-zsh-hook preexec _aishe_capture_cmd
  zle -N aishe-accept-line
  zle -N aishe-nl-widget
  zle -N aishe-show-route
  zle -N aishe-cycle-mode
  zle -N _aishe_highlight_command
  if (( ${+widgets[accept-line]} )); then
    typeset -g _aishe_orig_accept_line="${widgets[accept-line]#-}"
  fi
  zle -A aishe-accept-line accept-line
  autoload -Uz add-zle-hook-widget 2>/dev/null
  add-zle-hook-widget zle-line-pre-redraw _aishe_highlight_command 2>/dev/null || true
  bindkey "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget
  bindkey "${AISHE_ROUTE_KEY:-^X?}" aishe-show-route
  if (( ${+widgets[reverse-menu-complete]} )); then
    typeset -g _AISHE_ORIG_MODE_WIDGET=reverse-menu-complete
  fi
  bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode
fi
