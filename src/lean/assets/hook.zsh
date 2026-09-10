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
  local glyph mode
  glyph="$(_aishe_lean_glyph)"
  mode="${AISHE_MODE:-ask}"
  # Mode stays in PROMPT so narrow PTYs (no usable RPROMPT) still show it.
  PROMPT="%1~ ${mode} ${glyph} "
  RPROMPT="${mode}"
}

# Tiny owned prompt. No theme compatibility matrix.

# Tab completion for lean builtins + custom cmds (AISHE_LEAN_CMDS_FILE).
aishe-slash-tab() {
  emulate -L zsh
  setopt extendedglob
  local trimmed="${BUFFER##[[:space:]]#}"
  if [[ "$trimmed" != /* || "$trimmed" == *$'\n'* ]]; then
    zle expand-or-complete
    return
  fi
  local -a cmds
  cmds=(help mode status reset undo usage details mcp skills model connection sessions backend commands)
  if [[ -n "${AISHE_LEAN_CMDS_FILE:-}" && -r "$AISHE_LEAN_CMDS_FILE" ]]; then
    local custom
    while IFS= read -r custom; do
      [[ -n "$custom" ]] && cmds+=("$custom")
    done < "$AISHE_LEAN_CMDS_FILE"
  fi
  local prefix="${trimmed#/}"
  prefix="${prefix%%[[:space:]]*}"
  local -a matches
  local c
  for c in "${cmds[@]}"; do
    [[ "$c" == ${prefix}* ]] && matches+=("/$c")
  done
  if (( ${#matches} == 0 )); then
    zle -M "aishe: no slash matches for /$prefix"
    return
  fi
  if (( ${#matches} == 1 )); then
    BUFFER="${matches[1]} "
    CURSOR=${#BUFFER}
    return
  fi
  zle -M "aishe slashes: ${(j: :)matches}"
}

if [[ -o interactive ]]; then
  aishe_set_prompt
fi

# Optional tiny rc the user chose for this shell. NEVER ~/.zshrc by default.
if [[ -n "${AISHE_LEANRC:-}" && -r "${AISHE_LEANRC}" ]]; then
  source "${AISHE_LEANRC}"
elif [[ -r "${HOME}/.aishe/leanrc" ]]; then
  source "${HOME}/.aishe/leanrc"
fi


# Bounded compsys (F18). Dump + cache under private ZDOTDIR only — no user
# plugins, no ~/.zshrc. First interactive start may rebuild .zcompdump (tens to
# low hundreds of ms); subsequent prompts use `compinit -C` and stay fast.
if [[ -o interactive ]]; then
  autoload -Uz compinit 2>/dev/null || true
  if (( $+functions[compinit] )); then
    typeset -g _AISHE_COMPDUMP="${ZDOTDIR:-${HOME}}/.zcompdump"
    typeset -g _AISHE_COMPCACHE="${ZDOTDIR:-${HOME}}/.zcompcache"
    mkdir -p "${_AISHE_COMPCACHE}" 2>/dev/null || true
    zstyle ':completion:*' use-cache on
    zstyle ':completion:*' cache-path "${_AISHE_COMPCACHE}"
    if [[ -s "${_AISHE_COMPDUMP}" ]]; then
      compinit -d "${_AISHE_COMPDUMP}" -C
    else
      compinit -d "${_AISHE_COMPDUMP}"
    fi
  fi
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
      /help|/mode|/status|/reset|/undo|/usage|/details|/mcp|/skills|/model|/connection|/sessions|/backend|/commands) return 1 ;;
      # Single-segment /name -> custom markdown slash (FIFO), not PATH/NL.
      /*/*) ;;
      /[[:alnum:]_-]##) return 1 ;;
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

_aishe_lean_b64_decode() {
  # Decode STANDARD base64 from parent FILL_B64 / CONFIRM_B64 payloads.
  print -r -- "$1" | base64 -d 2>/dev/null
}

_aishe_lean_handle_reply() {
  emulate -L zsh
  local reply="$1"
  local kind="${reply%%	*}"
  local rest="${reply#*$'\t'}"
  [[ "$kind" == "$reply" ]] && rest=""
  case "$kind" in
    OK|STREAM_END)
      # Parent already wrote multi-line / streamed answer onto the PTY master.
      ;;
    ANSWER)
      # Legacy one-liner fallback.
      [[ -n "$rest" ]] && print -r -- "$rest"
      ;;
    ANSWER_B64)
      local text
      text="$(_aishe_lean_b64_decode "$rest")"
      [[ -n "$text" ]] && print -r -- "$text"
      ;;
    FILL|FILL_B64)
      local cmd="$rest"
      if [[ "$kind" == FILL_B64 ]]; then
        cmd="$(_aishe_lean_b64_decode "$rest")"
      fi
      typeset -g _AISHE_STAGED_SUGGESTION=1
      print -z -- "$cmd"
      ;;
    RAN)
      [[ -n "$rest" ]] && print -r -- "$rest"
      ;;
    ERROR)
      print -u2 -- "aishe: $rest"
      ;;
    CONFIRM|CONFIRM_B64)
      local body="$rest"
      if [[ "$kind" == CONFIRM_B64 ]]; then
        body="$(_aishe_lean_b64_decode "$rest")"
      fi
      print -r -- "Dangerous / unknown: $body"
      print -n -- "Type yes to run: "
      local ans
      if [[ -n "${_AISHE_INPUT_FD:-}" && "_AISHE_INPUT_FD" -ge 0 ]]; then
        IFS= read -r ans <&$_AISHE_INPUT_FD
      else
        IFS= read -r ans
      fi
      if [[ "$ans" == yes ]]; then
        local again payload
        if [[ "$kind" == CONFIRM_B64 ]]; then
          payload="$rest"
        else
          payload="$(_aishe_lean_flatten "$body")"
        fi
        again="$(_aishe_lean_send "CONFIRM_YES	$payload")"
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
    /help|/commands|/status|/reset|/undo|/usage|/details|/mcp|/skills|/model|/connection|/sessions|/backend)
      local reply
      reply="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$line")")" || return
      _aishe_lean_handle_reply "$reply"
      ;;
    /*/*)
      # Absolute path with extra segments — leave to shell.
      return 1
      ;;
    /[[:alnum:]_-]##)
      # Custom markdown slash-command → FIFO (F40).
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
  if [[ "$AISHE_LAST_EXIT" != 0 && "$AISHE_LAST_EXIT" != 130 && -n "$AISHE_LAST_CMD" ]]; then
    local elapsed=""
    if [[ -n "${_AISHE_COMMAND_STARTED:-}" && -n "${EPOCHREALTIME:-}" ]]; then
      elapsed=$(( (EPOCHREALTIME - _AISHE_COMMAND_STARTED) * 1000 ))
      elapsed=${elapsed%.*}
    fi
    # Capsule write is local JSON only — not OpenCode. Backgrounded so prompts stay fast.
    AISHE_LAST_DURATION_MS="$elapsed" command aishe --record-failure "$AISHE_LAST_CMD" >/dev/null 2>&1 &!
    typeset -g _AISHE_FAILURE_ACTIVE=1
    if [[ "${AISHE_FAILURE_HINTS:-1}" == 1 ]]; then
      print -P "%F{244}aishe: exit ${AISHE_LAST_EXIT} — ? explain · Ctrl-X Ctrl-F fix%f"
    fi
  elif [[ "${_AISHE_FAILURE_ACTIVE:-0}" == 1 ]]; then
    command aishe last clear >/dev/null 2>&1
    typeset -g _AISHE_FAILURE_ACTIVE=""
  fi
}
_aishe_capture_cmd() {
  typeset -g _AISHE_STAGED_SUGGESTION=""
  AISHE_LAST_CMD="$1"
  typeset -g _AISHE_ACCEPTED_LINE="$1"
  typeset -g _AISHE_COMMAND_STARTED="${EPOCHREALTIME:-}"
}


# Fix-the-last-command (Ctrl-X Ctrl-F). Prefills a corrected command; never auto-runs.
aishe-fix-command() {
  emulate -L zsh
  if [[ "${AISHE_LAST_EXIT:-0}" == 0 || -z "${AISHE_LAST_CMD:-}" ]]; then
    zle -M "aishe: no failed command to fix"
    return
  fi
  zle -M "aishe: asking for a fix…"
  local reply
  reply="$(_aishe_lean_send "FIX	${AISHE_MODE:-ask}	$PWD	fix")" || {
    zle -M "aishe: fix request failed"
    return
  }
  local kind="${reply%%	*}"
  local rest="${reply#*$'	'}"
  [[ "$kind" == "$reply" ]] && rest=""
  case "$kind" in
    FILL_B64)
      local decoded
      decoded="$(print -r -- "$rest" | base64 -d 2>/dev/null)" || decoded=""
      if [[ -n "$decoded" ]]; then
        BUFFER="$decoded"
        CURSOR=${#BUFFER}
        zle -M "aishe: fix ready — review before Enter"
      else
        zle -M "aishe: no fix available"
      fi
      ;;
    OK)
      zle -M "aishe: see explanation above"
      ;;
    ERROR)
      zle -M "aishe: ${rest:-fix failed}"
      ;;
    *)
      zle -M "aishe: no fix available"
      ;;
  esac
}


# Density toggle (default Ctrl-O; override with AISHE_DETAILS_KEY). Parent owns
# config.backend.output + PtyOut message; child syncs AISHE_AGENT_OUTPUT.
aishe-toggle-agent-details() {
  emulate -L zsh
  local reply
  reply="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	/details")" || {
    zle -M "aishe: details toggle failed"
    return
  }
  if [[ -n "${AISHE_OUTPUT_FILE:-}" && -r "$AISHE_OUTPUT_FILE" ]]; then
    IFS= read -r AISHE_AGENT_OUTPUT < "$AISHE_OUTPUT_FILE" || true
    export AISHE_AGENT_OUTPUT
  else
    case "${AISHE_AGENT_OUTPUT:-focus}" in
      focus)   AISHE_AGENT_OUTPUT=compact ;;
      compact) AISHE_AGENT_OUTPUT=detailed ;;
      *)       AISHE_AGENT_OUTPUT=focus ;;
    esac
    export AISHE_AGENT_OUTPUT
  fi
  zle -M "details: ${AISHE_AGENT_OUTPUT:-focus} (this shell)"
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
  # With text on the line, Shift-Tab delegates to completion (F18 compsys).
  # Mode cycling is empty-buffer only — matches keys_pty contract without
  # stealing completion.
  if [[ -n "$BUFFER" ]]; then
    if (( ${+widgets[reverse-menu-complete]} )); then
      zle reverse-menu-complete
    elif (( ${+widgets[menu-complete]} )); then
      zle menu-complete
    else
      zle expand-or-complete 2>/dev/null || true
    fi
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
    local was_q=0
    [[ "${body[1]}" == '?' ]] && { body="${body#?}"; was_q=1 }
    body="${body##[[:space:]]#}"
    print -s -- "$trimmed"
    zle -I
    if [[ -n "$body" ]]; then
      _aishe_lean_nl "$body"
    elif (( was_q )); then
      # Empty `?` → explain last failure capsule (lean-native, no OpenCode).
      _aishe_lean_nl "?"
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
  if [[ "$head" == /help || "$head" == /mode || "$head" == /status || "$head" == /backend ||
        "$head" == /reset || "$head" == /undo || "$head" == /usage || "$head" == /sessions ||
        "$head" == /details || "$head" == /mcp || "$head" == /skills ||
        "$head" == /model || "$head" == /connection || "$head" == /commands ||
        ( "$head" == /[[:alnum:]_-]## && "$head" != /*/* ) ]]; then
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
  zle -N aishe-fix-command
zle -N aishe-show-route
  zle -N aishe-cycle-mode
  zle -N aishe-toggle-agent-details
  zle -N aishe-slash-tab
  zle -N _aishe_highlight_command
  if (( ${+widgets[accept-line]} )); then
    typeset -g _aishe_orig_accept_line="${widgets[accept-line]#-}"
  fi
  zle -A aishe-accept-line accept-line
  autoload -Uz add-zle-hook-widget 2>/dev/null
  add-zle-hook-widget zle-line-pre-redraw _aishe_highlight_command 2>/dev/null || true
  bindkey "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget
  bindkey "${AISHE_FIX_KEY:-^X^F}" aishe-fix-command
bindkey "${AISHE_ROUTE_KEY:-^X?}" aishe-show-route
  if (( ${+widgets[reverse-menu-complete]} )); then
    typeset -g _AISHE_ORIG_MODE_WIDGET=reverse-menu-complete
  fi
  bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode
  bindkey "${AISHE_DETAILS_KEY:-^O}" aishe-toggle-agent-details
  bindkey "^I" aishe-slash-tab
fi
