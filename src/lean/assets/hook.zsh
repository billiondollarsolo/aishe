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
      agent|yolo) REPLY='*' ;;
      allow|auto) REPLY='>>' ;;
      *)          REPLY='>' ;;
    esac
  else
    case "${AISHE_MODE:-ask}" in
      agent|yolo) REPLY='*' ;;
      allow|auto) REPLY='»' ;;
      *)          REPLY='❯' ;;
    esac
  fi
}

# Display values remain literal psvar entries, including % and shell syntax.
# Builtin substitutions bound file-backed labels and remove terminal controls.
_aishe_lean_prompt_value() {
  local value="${1[1,512]}"
  value="${value//[[:cntrl:]]/}"
  REPLY="${value//$'[\u200b\u200e\u200f\u202a-\u202e\u2066-\u2069]'/}"
}

_aishe_lean_prompt_fit() {
  local value="$1" marker='…'
  local -i budget="$2"
  [[ "${AISHE_UNICODE:-unicode}" == ascii ]] && marker='...'
  REPLY="$value"
  (( ${(m)#value} <= budget )) && return
  (( budget <= ${(m)#marker} )) && { REPLY=''; return }
  while [[ -n "$value" ]] && (( ${(m)#value} + ${(m)#marker} > budget )); do
    value="${value[1,-2]}"
  done
  REPLY="${value}${marker}"
}

aishe_set_prompt() {
  emulate -L zsh
  # A CLI child stages mode changes for this parent shell; the mode controller
  # consumes them before any mode or grant state is displayed.
  (( $+functions[_aishe_lean_apply_pending_mode] )) && _aishe_lean_apply_pending_mode
  if [[ -n "${AISHE_SELECTION_FILE:-}" && -r "$AISHE_SELECTION_FILE" ]]; then
    {
      IFS= read -r AISHE_CONNECTION
      IFS= read -r AISHE_CONNECTION_LABEL
      IFS= read -r AISHE_PROVIDER
      IFS= read -r AISHE_ENDPOINT_HOST
      IFS= read -r AISHE_AUTH_LABEL
      IFS= read -r AISHE_MODEL
      IFS= read -r AISHE_REASONING
      IFS= read -r AISHE_SELECTION_SCOPE
    } < "$AISHE_SELECTION_FILE"
    export AISHE_CONNECTION AISHE_CONNECTION_LABEL AISHE_PROVIDER AISHE_ENDPOINT_HOST
    export AISHE_AUTH_LABEL AISHE_MODEL AISHE_REASONING AISHE_SELECTION_SCOPE
  elif [[ -n "${AISHE_MODEL_FILE:-}" && -r "$AISHE_MODEL_FILE" ]]; then
    IFS= read -r AISHE_MODEL < "$AISHE_MODEL_FILE"
    export AISHE_MODEL
  fi
  if [[ -n "${AISHE_SCOPE_FILE:-}" && -r "$AISHE_SCOPE_FILE" ]]; then
    IFS= read -r AISHE_SCOPE < "$AISHE_SCOPE_FILE"
    export AISHE_SCOPE
  fi
  if [[ -n "${AISHE_OUTPUT_FILE:-}" && -r "$AISHE_OUTPUT_FILE" ]]; then
    IFS= read -r AISHE_AGENT_OUTPUT < "$AISHE_OUTPUT_FILE"
    export AISHE_AGENT_OUTPUT
  fi

  local mode="${AISHE_MODE:-ask}" scope="${AISHE_SCOPE:-workspace}"
  local mode_label mode_color glyph path value field key item separator=' · '
  local close='' path_color='' metadata_color='' separator_prompt
  local -i columns=${COLUMNS:-80} granted=1 left_cells path_budget right_budget index=90
  local -A metrics
  local -a values colors
  case "$mode" in
    allow|auto)
      mode_label=allow
      mode_color="$AISHE_COLOR_MODE_AUTO"
      [[ "${_AISHE_ALLOW_GRANTED:-0}" == 1 ]] || granted=0
      ;;
    agent|yolo)
      [[ "$scope" == host ]] || scope=workspace
      mode_label="agent:${scope}"
      mode_color="$AISHE_COLOR_MODE_YOLO"
      if [[ "$scope" == host ]]; then
        [[ "${_AISHE_AGENT_HOST_GRANTED:-0}" == 1 ]] || granted=0
      elif [[ "${_AISHE_AGENT_WORKSPACE_GRANTED:-0}" != 1 &&
              "${_AISHE_AGENT_HOST_GRANTED:-0}" != 1 ]]; then
        granted=0
      elif [[ "${_AISHE_AGENT_HOST_GRANTED:-0}" != 1 &&
              -n "${_AISHE_AGENT_WORKSPACE_ROOT:-}" &&
              "${PWD:A}" != "$_AISHE_AGENT_WORKSPACE_ROOT" &&
              "${PWD:A}" != "$_AISHE_AGENT_WORKSPACE_ROOT"/* ]]; then
        granted=0
      fi
      ;;
    *) mode_label=ask; mode_color="$AISHE_COLOR_MODE_SUGGEST" ;;
  esac
  _aishe_lean_glyph
  glyph="$REPLY"
  if (( !granted )); then
    if (( ${(m)#mode_label} + ${(m)#glyph} + 17 <= columns )); then
      mode_label+=' [grant needed]'
    elif (( ${(m)#mode_label} + ${(m)#glyph} + 10 <= columns )); then
      mode_label+=' [grant]'
    else
      mode_label+='?'
      if [[ -o interactive && "${_AISHE_GRANT_HINT_SHOWN:-0}" != 1 ]]; then
        print -r -- 'aishe: mode? awaits a grant; an AI request shows the grant prompt.'
        typeset -g _AISHE_GRANT_HINT_SHOWN=1
      fi
    fi
  fi
  [[ "${AISHE_UNICODE:-unicode}" == ascii ]] && separator=' | '
  if [[ "${AISHE_STYLE:-none}" == on ]]; then
    close='%f%b%u%s'
    path_color="$AISHE_COLOR_PATH"
    separator_prompt="${AISHE_COLOR_MUTED}${separator}${close}"
  else
    mode_color=''
    separator_prompt="$separator"
  fi

  # Mode and scope live on the left even when RPROMPT is hidden by ZLE. Give
  # them space first; shorten or omit the path before losing authority state.
  left_cells=$(( ${(m)#mode_label} + ${(m)#glyph} + 2 ))
  path_budget=$(( columns - left_cells - 2 ))
  (( path_budget > 24 )) && path_budget=24
  path="${PWD:t}"
  [[ "$PWD" == "$HOME" ]] && path='~'
  [[ -n "$path" ]] || path='/'
  _aishe_lean_prompt_value "$path"
  if (( ${(m)#REPLY} > path_budget && path_budget < 6 )); then
    REPLY=''
  else
    _aishe_lean_prompt_fit "$REPLY" "$path_budget"
  fi
  psvar[89]="$REPLY"
  PROMPT="${mode_color}${mode_label} ${glyph}${close} "
  if [[ -n "$REPLY" ]]; then
    PROMPT="${path_color}%89v${close} ${PROMPT}"
    left_cells=$(( left_cells + ${(m)#REPLY} + 1 ))
  fi

  _AISHE_STATUS_TEXT=''
  _AISHE_STATUS_PROMPT=''
  RPROMPT=''
  for (( index=90; index<=99; index++ )); do psvar[$index]=''; done
  [[ "${AISHE_STATUS_POSITION:-right}" == off ]] && return
  right_budget=$(( columns - left_cells - 2 ))
  (( right_budget >= 8 )) || return
  (( right_budget > 72 )) && right_budget=72

  if [[ -n "${AISHE_STATUS_FILE:-}" && -r "$AISHE_STATUS_FILE" ]]; then
    while IFS=$'\t' read -r key value; do
      case "$key" in
        session_tokens|session_cost|requests|elapsed|context)
          _aishe_lean_prompt_value "$value"
          metrics[$key]="$REPLY"
          ;;
      esac
    done < "$AISHE_STATUS_FILE"
  fi
  _aishe_lean_prompt_value "${AISHE_MODEL:-}"
  if [[ -n "$REPLY" ]]; then
    values+=("$REPLY")
    colors+=("$AISHE_COLOR_MODEL")
  fi
  _aishe_lean_prompt_value "${AISHE_CONNECTION_LABEL:-${AISHE_CONNECTION:-}}"
  if [[ -n "$REPLY" && "$REPLY" != "${values[1]:-}" ]]; then
    values+=("$REPLY")
    colors+=("$AISHE_COLOR_CONNECTION")
  fi
  for item in ${(s:,:)AISHE_STATUS_ITEMS}; do
    case "$item" in
      session_tokens|session_cost|requests|elapsed|context)
        value="${metrics[$item]:-}"
        [[ -n "$value" ]] || continue
        values+=("${value#session }")
        colors+=("$AISHE_COLOR_METRIC")
        ;;
    esac
  done
  index=90
  local -i i used=0 available
  for (( i=1; i<=${#values} && index<=99; i++ )); do
    value="${values[$i]}"
    available=$(( right_budget - used ))
    (( used )) && available=$(( available - ${(m)#separator} ))
    if (( i == 1 )); then
      _aishe_lean_prompt_fit "$value" "$available"
      value="$REPLY"
    fi
    [[ -n "$value" ]] || continue
    (( ${(m)#value} <= available )) || continue
    if (( used )); then
      _AISHE_STATUS_TEXT+="$separator"
      _AISHE_STATUS_PROMPT+="$separator_prompt"
      used=$(( used + ${(m)#separator} ))
    fi
    psvar[$index]="$value"
    metadata_color=''
    [[ "${AISHE_STYLE:-none}" == on ]] && metadata_color="${colors[$i]}"
    _AISHE_STATUS_TEXT+="$value"
    _AISHE_STATUS_PROMPT+="${metadata_color}%${index}v${close}"
    used=$(( used + ${(m)#value} ))
    (( index++ ))
  done
  RPROMPT="$_AISHE_STATUS_PROMPT"
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
# Always pass `-u`: CI images (esp. macOS runners) ship group-writable
# /usr/share/zsh; plain `compinit` then prompts on a TTY and deadlocks nested
# PTY tests that cannot answer the security question.
if [[ -o interactive ]]; then
  autoload -Uz compinit 2>/dev/null || true
  if (( $+functions[compinit] )); then
    typeset -g _AISHE_COMPDUMP="${ZDOTDIR:-${HOME}}/.zcompdump"
    typeset -g _AISHE_COMPCACHE="${ZDOTDIR:-${HOME}}/.zcompcache"
    mkdir -p "${_AISHE_COMPCACHE}" 2>/dev/null || true
    zstyle ':completion:*' use-cache on
    zstyle ':completion:*' cache-path "${_AISHE_COMPCACHE}"
    if [[ -s "${_AISHE_COMPDUMP}" ]]; then
      compinit -u -d "${_AISHE_COMPDUMP}" -C
    else
      compinit -u -d "${_AISHE_COMPDUMP}"
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

_aishe_lean_grant_word() {
  case "${AISHE_MODE:-ask}" in
    allow|auto) print -r -- allow ;;
    agent|yolo)
      [[ "${AISHE_SCOPE:-workspace}" == host ]] && print -r -- agent-host || print -r -- agent
      ;;
    *) print -r -- ask ;;
  esac
}

_aishe_lean_mark_mode_interaction() {
  if [[ "${_AISHE_GRANT_DIALOG:-0}" != 1 ]]; then
    typeset -g _AISHE_GRANT_PREVIOUS_EXIT="${AISHE_LAST_EXIT:-0}"
  fi
  typeset -g _AISHE_GRANT_DIALOG=1
}

_aishe_lean_read_grant() {
  emulate -L zsh
  local answer="" key input_fd="${_AISHE_INPUT_FD:-0}" tty_state
  # `read -u` does not switch an explicitly preserved descriptor out of
  # canonical mode. Read one byte immediately so Esc does not need Enter.
  tty_state="$(command stty -g <&$input_fd 2>/dev/null)" || return 1
  command stty -icanon -echo min 1 time 0 <&$input_fd 2>/dev/null || return 1
  {
    while IFS= read -r -k 1 -u $input_fd key; do
      case "$key" in
        $'\r'|$'\n') print; REPLY="$answer"; return 0 ;;
        $'\e'|$'\003'|$'\004') print; return 1 ;;
        $'\177'|$'\b')
          if [[ -n "$answer" ]]; then
            answer="${answer%?}"
            print -n -- $'\b \b'
          fi
          ;;
        [[:print:]])
          if (( ${#answer} < 32 )); then
            answer+="$key"
            print -n -- "$key"
          fi
          ;;
      esac
    done
    print
    return 1
  } always {
    command stty "$tty_state" <&$input_fd 2>/dev/null || true
  }
}

_aishe_lean_take_grant() {
  emulate -L zsh
  local want="$1" reply
  reply="$(_aishe_lean_send $'MODE_CHECK\t'"$want"$'\t'"$PWD")" || return 1
  case "$reply" in
    ACCEPTED) ;;
    GRANT_REQUIRED)
      _aishe_lean_mark_mode_interaction
      print
      case "$want" in
        allow)
          print -r -- "Enter allow · safe commands for this shell?"
          print -r -- "Dangerous / unknown commands still require typing yes."
          ;;
        agent)
          print -r -- "Enter agent · workspace ${PWD:A}?"
          print -r -- "The agent may run commands and change files inside this workspace without asking again."
          [[ "${OSTYPE:-}" == darwin* ]] && print -r -- "Warning: macOS workspace mode is policy-only."
          ;;
        agent-host)
          print -r -- "Enter agent-host · host?"
          print -r -- "The agent may execute commands and change files anywhere your user can access."
          ;;
      esac
      if [[ "$want" == agent* && "${AISHE_AGENT_PREVIEW:-0}" == 1 ]]; then
        print -r -- "File previews still ask for approval."
      fi
      if [[ "$want" == agent && "${AISHE_MCP_ENABLED:-0}" != 0 ]]; then
        print -r -- "MCP tools retain their configured server access."
      fi
      print -n -- "Type ${want} to continue (Esc cancels): "
      if ! _aishe_lean_read_grant || [[ "$REPLY" != "$want" ]]; then
        print -r -- "grant declined · mode stays ${AISHE_MODE:-ask}"
        return 1
      fi
      ;;
    ERROR$'\t'*) print -u2 -r -- "aishe: ${reply#*$'\t'}"; return 1 ;;
    *) print -u2 -r -- "aishe: mode check failed"; return 1 ;;
  esac
  reply="$(_aishe_lean_send $'MODE_ACCEPT\t'"$want"$'\t'"$PWD")" || return 1
  if [[ "$reply" != MODE_OK$'\t'* ]]; then
    print -u2 -r -- "aishe: ${reply#*$'\t'}"
    return 1
  fi
  local values="${reply#*$'\t'}"
  AISHE_MODE="${values%%$'\t'*}"
  values="${values#*$'\t'}"
  AISHE_SCOPE="${values%%$'\t'*}"
  local accepted_root="${values#*$'\t'}"
  case "$want" in
    allow) typeset -g _AISHE_ALLOW_GRANTED=1 ;;
    agent)
      typeset -g _AISHE_AGENT_WORKSPACE_GRANTED=1
      typeset -g _AISHE_AGENT_WORKSPACE_ROOT="$accepted_root"
      ;;
    agent-host) typeset -g _AISHE_AGENT_HOST_GRANTED=1 ;;
  esac
  [[ "$want" != ask ]] && AISHE_GRANT="$want"
  export AISHE_MODE AISHE_SCOPE AISHE_GRANT
  if [[ -n "${AISHE_SCOPE_FILE:-}" ]]; then
    print -r -- "$AISHE_SCOPE" > "$AISHE_SCOPE_FILE"
  fi
  # Standalone CLI calls in this shell can reuse the same authority. Workspace
  # markers retain the canonical root so changing directory cannot broaden it.
  if [[ "$want" != ask && -n "${AISHE_ACCEPTANCE_FILE:-}" ]]; then
    local marker="$want" saved exists=0
    if [[ "$want" == agent ]]; then
      local grant_root="$_AISHE_AGENT_WORKSPACE_ROOT"
      # The compatibility file is line-oriented; control characters must never
      # become extra scope markers. The live parent grant remains authoritative.
      if [[ "$grant_root" == *$'\n'* || "$grant_root" == *$'\r'* || "$grant_root" == *$'\t'* ]]; then
        return 0
      fi
      marker="agent"$'\t'"$grant_root"
    fi
    if [[ -r "$AISHE_ACCEPTANCE_FILE" ]]; then
      while IFS= read -r saved; do
        [[ "$saved" == "$marker" ]] && { exists=1; break }
      done < "$AISHE_ACCEPTANCE_FILE"
    fi
    if (( ! exists )); then
      (umask 077; print -r -- "$marker" >> "$AISHE_ACCEPTANCE_FILE")
    fi
  fi
  return 0
}

# `aishe mode ...` runs as a child process; consume its explicit shell handoff.
_aishe_lean_apply_pending_mode() {
  emulate -L zsh
  [[ -n "${AISHE_PENDING_FILE:-}" && -r "$AISHE_PENDING_FILE" ]] || return 0
  local action value
  { IFS= read -r action; IFS= read -r value; } < "$AISHE_PENDING_FILE"
  command rm -f -- "$AISHE_PENDING_FILE"
  [[ "$action" == mode ]] || return 0
  case "$value" in
    ask|suggest) value=ask ;;
    allow|auto) value=allow ;;
    agent|yolo) value=agent ;;
    agent-host) ;;
    *) print -u2 -r -- "aishe: unknown mode '$value'"; return 0 ;;
  esac
  _aishe_lean_mark_mode_interaction
  _aishe_lean_take_grant "$value" || return 0
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
      # Parent already wrote multi-line / streamed answer onto terminal output.
      ;;
    CANCELLED)
      print -r -- "cancelled"
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
  _aishe_lean_take_grant "$(_aishe_lean_grant_word)" || return 0
  local payload reply
  payload="NL	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$line")"
  reply="$(_aishe_lean_send "$payload")" || return
  _aishe_lean_handle_reply "$reply"
}

_aishe_lean_slash() {
  emulate -L zsh
  setopt extendedglob
  local line="$1"
  local name="${line%%[[:space:]]*}"
  local arg="${line#"$name"}"
  arg="${arg##[[:space:]]#}"
  arg="${arg%%[[:space:]]#}"
  case "$name" in
    /mode)
      _aishe_lean_mark_mode_interaction
      if [[ -z "$arg" ]]; then
        print -r -- "mode: ${AISHE_MODE:-ask} (${AISHE_SCOPE:-workspace})"
        return 0
      fi
      case "${arg:l}" in
        ask|suggest) arg=ask ;;
        allow|auto) arg=allow ;;
        agent|yolo) arg=agent ;;
        agent-host) ;;
        *) print -u2 -r -- "aishe: unknown mode '$arg' (ask|allow|agent|agent-host)"; return 0 ;;
      esac
      _aishe_lean_take_grant "$arg" || return 0
      aishe_set_prompt
      print -r -- "mode: ${AISHE_MODE} (${AISHE_SCOPE})"
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
  local shell_exit=$?
  typeset -g _AISHE_ACCEPTED_LINE=""
  if [[ "${_AISHE_GRANT_DIALOG:-0}" == 1 ]]; then
    # A mode dialog owns no shell command. Canceling it must preserve the last
    # real failure, rather than journal a synthetic read/widget failure.
    AISHE_LAST_EXIT="${_AISHE_GRANT_PREVIOUS_EXIT:-0}"
    unset _AISHE_GRANT_DIALOG _AISHE_GRANT_PREVIOUS_EXIT
    return 0
  fi
  AISHE_LAST_EXIT=$shell_exit
  if [[ "$AISHE_LAST_EXIT" != 0 && "$AISHE_LAST_EXIT" != 130 && -n "$AISHE_LAST_CMD" ]]; then
    local elapsed=""
    if [[ -n "${_AISHE_COMMAND_STARTED:-}" && -n "${EPOCHREALTIME:-}" ]]; then
      elapsed=$(( (EPOCHREALTIME - _AISHE_COMMAND_STARTED) * 1000 ))
      elapsed=${elapsed%.*}
    fi
    # Capsule write is local JSON only — not OpenCode. Backgrounded so prompts stay fast.
    AISHE_LAST_EXIT="$AISHE_LAST_EXIT" AISHE_LAST_DURATION_MS="$elapsed" command aishe --record-failure "$AISHE_LAST_CMD" >/dev/null 2>&1 &!
    typeset -g _AISHE_FAILURE_ACTIVE=1
    if [[ "${AISHE_FAILURE_HINTS:-1}" == 1 ]]; then
      local hint="aishe: exit ${AISHE_LAST_EXIT} | ? explain | Ctrl-X Ctrl-F fix"
      if [[ "${AISHE_UNICODE:-unicode}" != ascii ]]; then
        hint="aishe: exit ${AISHE_LAST_EXIT} — ? explain · Ctrl-X Ctrl-F fix"
      fi
      if [[ "${AISHE_STYLE:-on}" != none && -z "${NO_COLOR+x}" && "${TERM:-}" != dumb ]]; then
        print -P -- "${AISHE_COLOR_MUTED:-}${hint}%f%b%u%s"
      else
        print -r -- "$hint"
      fi
    fi
  elif [[ "${_AISHE_FAILURE_ACTIVE:-0}" == 1 ]]; then
    command aishe last clear >/dev/null 2>&1
    typeset -g _AISHE_FAILURE_ACTIVE=""
  fi
}
_aishe_capture_cmd() {
  unset _AISHE_GRANT_DIALOG _AISHE_GRANT_PREVIOUS_EXIT
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
  zle -I
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
    CANCELLED)
      zle -M "aishe: fix cancelled"
      ;;
    *)
      zle -M "aishe: no fix available"
      ;;
  esac
  aishe_set_prompt
  zle reset-prompt
}


# Density toggle (default Ctrl-O; override with AISHE_DETAILS_KEY). Parent owns
# config.backend.output + PtyOut message; child syncs AISHE_AGENT_OUTPUT.
aishe-toggle-agent-details() {
  emulate -L zsh
  local reply
  # The parent prints a persistent notice. Give it a clear output line, then
  # redraw the same input buffer instead of writing into the active ZLE row.
  zle -I
  reply="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	/details")" || {
    zle -M "aishe: details toggle failed"
    zle reset-prompt
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
  aishe_set_prompt
  zle reset-prompt
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
  POSTDISPLAY=""
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
  _aishe_lean_mark_mode_interaction
  zle -I
  local want
  case "${AISHE_MODE:-ask}" in
    ask|suggest) want=allow ;;
    allow|auto)
      [[ "${AISHE_SCOPE:-workspace}" == host ]] && want=agent-host || want=agent
      ;;
    *) want=ask ;;
  esac
  _aishe_lean_take_grant "$want" || { zle reset-prompt; return 0 }
  aishe_set_prompt
  zle reset-prompt
}

aishe-accept-line() {
  emulate -L zsh
  setopt extendedglob
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
    zle -I
    if _aishe_lean_slash "$trimmed"; then
      print -s -- "$trimmed"
      BUFFER=""
      POSTDISPLAY=""
      # The slash completed inside this widget. Refresh its existing input
      # row rather than accepting a second empty shell line and prompt.
      aishe_set_prompt
      zle reset-prompt
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
    POSTDISPLAY=""
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
      *memo=aishe) ;;
      *) kept+=("$spec") ;;
    esac
  done
  region_highlight=("${kept[@]}")
  [[ "${AISHE_COMMAND_HIGHLIGHT:-1}" != 0 && -n "$BUFFER" ]] || return 0
  [[ "${AISHE_STYLE:-on}" != none && -z "${NO_COLOR+x}" && "${TERM:-}" != dumb ]] || return 0
  if _aishe_routes_to_agent "$BUFFER"; then
    region_highlight+=("0 ${#BUFFER} ${AISHE_HIGHLIGHT_AGENT:-bold},memo=aishe")
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
    region_highlight+=("$slash_start $slash_end ${AISHE_HIGHLIGHT_SLASH:-bold},memo=aishe")
    return 0
  fi
  [[ "$head" == [[:alnum:]_./+-]## ]] || return 0
  whence -w -- "$head" >/dev/null 2>&1 || return 0
  local start=${#leading}
  local end=$(( start + ${#head} ))
  region_highlight+=("$start $end ${AISHE_HIGHLIGHT_SHELL:-bold},memo=aishe")
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
