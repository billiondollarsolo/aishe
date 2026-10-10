# aishe lean PTY hook (.zshrc) — generated
# The clean profile starts isolated; the explicitly selected personal wrapper
# has already sourced user configuration before this shared native hook.
# Known commands stay in this zsh. NL goes to the parent over a FIFO (no `aishe` spawn).

[[ "${AISHE_ZSH_PROFILE:-clean}" == clean ]] && unsetopt GLOBAL_RCS 2>/dev/null

if [[ -n "${AISHE_HISTFILE:-}" &&
      ( "${AISHE_ZSH_PROFILE:-clean}" == clean || "${AISHE_MANAGE_HISTORY:-0}" == 1 ) ]]; then
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
# The parent selected these private controls. Later environment edits must not
# redirect the producer or weaken credential filtering for an agent request.
(( ${+_AISHE_STATE_CONTROL_FILE} )) || readonly _AISHE_STATE_CONTROL_FILE="${AISHE_EXECUTION_STATE_FILE:-}"
(( ${+_AISHE_STATE_CONTROL_DENY} )) || readonly _AISHE_STATE_CONTROL_DENY="${AISHE_EXECUTION_STATE_DENY:-}"
(( ${+_AISHE_BACKGROUND_CONTROL_FILE} )) || readonly _AISHE_BACKGROUND_CONTROL_FILE="${AISHE_BACKGROUND_FILE:-}"
(( ${+_AISHE_BACKGROUND_CONTROL_EVENTS} )) || readonly _AISHE_BACKGROUND_CONTROL_EVENTS="${AISHE_BACKGROUND_EVENTS:-}"
(( ${+_AISHE_LAUNCH_HINT_ACK} )) || readonly _AISHE_LAUNCH_HINT_ACK="${AISHE_LAUNCH_HINT_ACK:-}"

# The parent writes counts, never task output. Read the small private cache only
# before a prompt or after a native fd notification, never on each keystroke.
_aishe_background_refresh() {
  emulate -L zsh
  setopt extendedglob
  local _AISHE_BACKGROUND_NAME _AISHE_BACKGROUND_COUNT
  local -i _AISHE_BACKGROUND_FD _AISHE_BACKGROUND_LINES=0
  local -A _AISHE_BACKGROUND_COUNTS
  local -a _AISHE_BACKGROUND_PARTS
  typeset -gx AISHE_BACKGROUND_INDICATOR=''
  [[ "${AISHE_BACKGROUND_INDICATOR_ENABLED:-1}" != 0 &&
      -n "$_AISHE_BACKGROUND_CONTROL_FILE" && -f "$_AISHE_BACKGROUND_CONTROL_FILE" &&
      ! -L "$_AISHE_BACKGROUND_CONTROL_FILE" ]] || return 0
  zmodload zsh/system 2>/dev/null || return 0
  sysopen -r -o nonblock,nofollow -u _AISHE_BACKGROUND_FD "$_AISHE_BACKGROUND_CONTROL_FILE" 2>/dev/null || return 0
  while IFS=$'\t' read -r -u $_AISHE_BACKGROUND_FD _AISHE_BACKGROUND_NAME _AISHE_BACKGROUND_COUNT; do
    (( ++_AISHE_BACKGROUND_LINES <= 5 )) || break
    [[ "$_AISHE_BACKGROUND_COUNT" == [0-9]## && ${#_AISHE_BACKGROUND_COUNT} -le 9 ]] || continue
    case "$_AISHE_BACKGROUND_NAME" in
      running|ready|attention|needs_you|queued) _AISHE_BACKGROUND_COUNTS[$_AISHE_BACKGROUND_NAME]="$_AISHE_BACKGROUND_COUNT" ;;
    esac
  done
  exec {_AISHE_BACKGROUND_FD}<&-
  for _AISHE_BACKGROUND_NAME in needs_you attention running queued ready; do
    _AISHE_BACKGROUND_COUNT="${_AISHE_BACKGROUND_COUNTS[$_AISHE_BACKGROUND_NAME]:-0}"
    [[ "$_AISHE_BACKGROUND_COUNT" == 0 ]] && continue
    _AISHE_BACKGROUND_PARTS+=("$_AISHE_BACKGROUND_COUNT ${_AISHE_BACKGROUND_NAME//_/ }")
  done
  if [[ "${AISHE_UNICODE:-unicode}" == ascii ]]; then
    AISHE_BACKGROUND_INDICATOR="${(j: | :)_AISHE_BACKGROUND_PARTS}"
  else
    AISHE_BACKGROUND_INDICATOR="${(j: · :)_AISHE_BACKGROUND_PARTS}"
  fi
}

aishe-background-event() {
  emulate -L zsh
  local _AISHE_BACKGROUND_EVENT _AISHE_BACKGROUND_PREVIOUS="$AISHE_BACKGROUND_INDICATOR"
  # The opened descriptor is nonblocking. Drain a burst into one redraw.
  sysread -i "$1" -s 256 _AISHE_BACKGROUND_EVENT 2>/dev/null || true
  if [[ -n "${2:-}" ]]; then
    zle -F "$1" 2>/dev/null || true
    return 0
  fi
  _aishe_background_refresh
  [[ "$AISHE_BACKGROUND_INDICATOR" != "$_AISHE_BACKGROUND_PREVIOUS" ]] || return 0
  local _AISHE_BACKGROUND_BUFFER="$BUFFER" _AISHE_BACKGROUND_CURSOR="$CURSOR"
  local _AISHE_BACKGROUND_MARK="$MARK" _AISHE_BACKGROUND_REGION="$REGION_ACTIVE"
  local _AISHE_BACKGROUND_REDRAW=1
  aishe_set_prompt
  BUFFER="$_AISHE_BACKGROUND_BUFFER"
  CURSOR="$_AISHE_BACKGROUND_CURSOR"
  MARK="$_AISHE_BACKGROUND_MARK"
  REGION_ACTIVE="$_AISHE_BACKGROUND_REGION"
  zle reset-prompt
}

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
  _aishe_background_refresh
  # A CLI child stages mode changes for this parent shell; the mode controller
  # consumes them before any mode or grant state is displayed.
  if [[ "${_AISHE_BACKGROUND_REDRAW:-0}" != 1 ]]; then
    (( $+functions[_aishe_lean_apply_pending_mode] )) && _aishe_lean_apply_pending_mode
  fi
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
      if [[ -o interactive && "${_AISHE_GRANT_HINT_SHOWN:-0}" != 1 && "${_AISHE_BACKGROUND_REDRAW:-0}" != 1 ]]; then
        print -r -- 'aishe: mode? awaits a grant; an AI request shows the grant prompt.'
        typeset -g _AISHE_GRANT_HINT_SHOWN=1
      fi
    fi
  fi
  [[ "${AISHE_UNICODE:-unicode}" == ascii ]] && separator=' | '
  # Keep a theme's prompt intact unless the person explicitly requests ours.
  # Mode state is still refreshed above and available to themes as this value.
  typeset -gx AISHE_MODE_INDICATOR="${mode_label} ${glyph}"
  if [[ "${AISHE_PTY_PROMPT:-1}" == 0 ||
        ( "${AISHE_ZSH_PROFILE:-clean}" == personal && "${AISHE_PTY_PROMPT:-1}" != force ) ]]; then
    if [[ -n "${_AISHE_COMPOSED_RPROMPT:-}" && "$RPROMPT" == "$_AISHE_COMPOSED_RPROMPT" ]]; then
      RPROMPT="${_AISHE_USER_RPROMPT:-}"
    fi
    if [[ "${AISHE_PERSONAL_INDICATOR:-0}" == 1 ]]; then
      typeset -g _AISHE_USER_RPROMPT="$RPROMPT"
      psvar[88]="$AISHE_MODE_INDICATOR"
      typeset -g _AISHE_COMPOSED_RPROMPT="${RPROMPT}${RPROMPT:+$separator}%88v"
      if [[ -n "$AISHE_BACKGROUND_INDICATOR" ]]; then
        psvar[87]="$AISHE_BACKGROUND_INDICATOR"
        _AISHE_COMPOSED_RPROMPT+="${separator}%87v"
      fi
      RPROMPT="$_AISHE_COMPOSED_RPROMPT"
    fi
    return 0
  fi
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
        task|last_tokens|last_cost|session_tokens|session_cost|requests|elapsed|context)
          _aishe_lean_prompt_value "$value"
          metrics[$key]="$REPLY"
          ;;
      esac
    done < "$AISHE_STATUS_FILE"
  fi
  # Activity comes first so a narrow terminal still exposes work awaiting the
  # person. Existing model/connection metrics fill the remaining right margin.
  if [[ -n "$AISHE_BACKGROUND_INDICATOR" ]]; then
    values+=("$AISHE_BACKGROUND_INDICATOR")
    colors+=("$AISHE_COLOR_METRIC")
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
      task|last_tokens|last_cost|session_tokens|session_cost|requests|elapsed|context)
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

# __AISHE_GENERATED_SLASH_CATALOGUE__

# Use zsh's native completion list: descriptions, grouped rows, and repeated Tab.
# Only the command head belongs to AIShe; absolute paths and arguments keep
# their normal completion behavior.
_aishe_complete_slashes() {
  setopt localoptions extendedglob
  if [[ "${_AISHE_SLASH_DISCOVERED:-0}" != 1 ]]; then
    local reply
    reply="$(_aishe_lean_send COMMANDS)" || return 1
    [[ "$reply" == OK ]] || return 1
    typeset -g _AISHE_SLASH_DISCOVERED=1
  fi
  local group name description
  local -a entries customs
  for group in "${_AISHE_SLASH_CATEGORIES[@]}"; do
    entries=()
    for name in "${_AISHE_SLASH_NAMES[@]}"; do
      [[ "${_AISHE_SLASH_GROUPS[$name]}" == "$group" ]] || continue
      description="${_AISHE_SLASH_DESCRIPTIONS[$name]}"
      entries+=("/$name:${description//:/\\:}")
    done
    _describe -t "aishe-${group:l}" "$group" entries -Q -S ' '
  done
  if [[ -n "${AISHE_LEAN_CMDS_FILE:-}" && -r "$AISHE_LEAN_CMDS_FILE" ]]; then
    while IFS=$'\t' read -r name description; do
      [[ "$name" == [[:alnum:]_-]## ]] || continue
      [[ -n "${_AISHE_SLASH_DESCRIPTIONS[$name]:-}" ]] && continue
      description="${description//[[:cntrl:]]/}"
      [[ -n "$description" ]] || description='Custom command'
      customs+=("/$name:${description//:/\\:}")
    done < "$AISHE_LEAN_CMDS_FILE"
  fi
  (( ${#customs} )) && _describe -t aishe-custom 'Custom commands' customs -Q -S ' '
  # A single slash also starts an absolute path. Keep native file completion
  # when the head does not match a built-in or a custom command.
  (( compstate[nmatches] )) || _files
  return 0
}

_aishe_complete_slash_arguments() {
  local -a entries
  case "${words[1]}" in
    /help)
      local name
      entries=('keys:Keyboard shortcuts' 'all:Full command list')
      for name in "${_AISHE_SLASH_NAMES[@]}"; do
        entries+=("$name:${_AISHE_SLASH_DESCRIPTIONS[$name]//:/\\:}")
      done
      _describe -t aishe-help 'Command help' entries -Q -S ' '
      ;;
    /mode)
      entries=('ask:Propose commands for review' 'allow:Run safe suggestions after a shell grant'
               'agent:Act within a granted workspace' 'agent-host:Act with a host grant')
      _describe -t aishe-modes 'Mode' entries -Q -S ' '
      ;;
    /context)
      entries=('--explain:Explain included sections' '--json:Print section metadata')
      _describe -t aishe-context 'Context preview' entries -Q -S ' '
      ;;
    /doctor)
      entries=('--json:Print diagnostic metadata' '--probe:Check provider reachability'
               '--live:Check model capabilities' '--fix:Apply safe local repairs')
      _describe -t aishe-doctor 'Diagnostics' entries -Q -S ' '
      ;;
    *) _default ;;
  esac
}

_aishe_slash_completion() {
  # zle calls from another widget retain the caller's WIDGET. Supply an explicit
  # context and completer so nested dispatch cannot fall back to root files.
  local curcontext='aishe-slashes:::'
  _main_complete _aishe_complete_slashes
}

# Bare-slash discovery is a bounded view, independent of the user's native
# completion list. A nested edit captures search/navigation; Enter only stages
# a command back into the original edit, where a second Enter can execute it.
_aishe_slash_picker_render() {
  emulate -L zsh
  local name description row value
  local -i width=${COLUMNS:-80} height=${LINES:-24} limit first last index count
  local -a lines
  _AISHE_SLASH_PICKER_MATCHES=()
  for name in "${_AISHE_SLASH_PICKER_NAMES[@]}"; do
    description="${_AISHE_SLASH_PICKER_DESCRIPTIONS[$name]}"
    if [[ -z "$_AISHE_SLASH_PICKER_QUERY" ||
          "${name:l} ${description:l}" == *"${_AISHE_SLASH_PICKER_QUERY:l}"* ]]; then
      _AISHE_SLASH_PICKER_MATCHES+=("$name")
    fi
  done
  count=${#_AISHE_SLASH_PICKER_MATCHES}
  (( _AISHE_SLASH_PICKER_INDEX > count )) && _AISHE_SLASH_PICKER_INDEX=$count
  (( _AISHE_SLASH_PICKER_INDEX < 1 )) && _AISHE_SLASH_PICKER_INDEX=1
  limit=$(( height - 7 ))
  (( limit > 6 )) && limit=6
  (( limit < 1 )) && limit=1
  first=$(( ((_AISHE_SLASH_PICKER_INDEX - 1) / limit) * limit + 1 ))
  last=$(( first + limit - 1 ))
  (( last > count )) && last=$count
  if (( count )); then
    lines+=("Commands ${_AISHE_SLASH_PICKER_INDEX}/${count}")
  else
    lines+=('Commands - no matches')
  fi
  _aishe_lean_prompt_fit "Search: $_AISHE_SLASH_PICKER_QUERY" $(( width - 1 ))
  lines+=("$REPLY")
  for (( index=first; index<=last; index++ )); do
    name="${_AISHE_SLASH_PICKER_MATCHES[$index]}"
    description="${_AISHE_SLASH_PICKER_DESCRIPTIONS[$name]}"
    row='  '
    (( index == _AISHE_SLASH_PICKER_INDEX )) && row='> '
    row+="/${name}  ${description}"
    _aishe_lean_prompt_fit "$row" $(( width - 1 ))
    lines+=("$REPLY")
  done
  _aishe_lean_prompt_fit 'Tab/arrows | Enter stages | Esc' $(( width - 1 ))
  lines+=("$REPLY")
  POSTDISPLAY=$'\n'"${(F)lines}"
  zle -R
}

aishe-slash-picker-insert() {
  (( ${#_AISHE_SLASH_PICKER_QUERY} >= 128 )) && { zle beep; return; }
  _AISHE_SLASH_PICKER_QUERY+="$KEYS"
  _AISHE_SLASH_PICKER_INDEX=1
  _aishe_slash_picker_render
}

aishe-slash-picker-paste() {
  local pasted
  zle .bracketed-paste pasted || return
  pasted="${pasted//[[:cntrl:]]/}"
  local -i available=$(( 128 - ${#_AISHE_SLASH_PICKER_QUERY} ))
  (( available > 0 )) && _AISHE_SLASH_PICKER_QUERY+="${pasted[1,$available]}"
  _AISHE_SLASH_PICKER_INDEX=1
  _aishe_slash_picker_render
}

aishe-slash-picker-delete() {
  _AISHE_SLASH_PICKER_QUERY="${_AISHE_SLASH_PICKER_QUERY[1,-2]}"
  _AISHE_SLASH_PICKER_INDEX=1
  _aishe_slash_picker_render
}

aishe-slash-picker-clear() {
  _AISHE_SLASH_PICKER_QUERY=''
  _AISHE_SLASH_PICKER_INDEX=1
  _aishe_slash_picker_render
}

aishe-slash-picker-next() {
  (( _AISHE_SLASH_PICKER_INDEX++ ))
  (( _AISHE_SLASH_PICKER_INDEX > ${#_AISHE_SLASH_PICKER_MATCHES} )) && _AISHE_SLASH_PICKER_INDEX=1
  _aishe_slash_picker_render
}

aishe-slash-picker-previous() {
  (( _AISHE_SLASH_PICKER_INDEX-- ))
  (( _AISHE_SLASH_PICKER_INDEX < 1 )) && _AISHE_SLASH_PICKER_INDEX=${#_AISHE_SLASH_PICKER_MATCHES}
  _aishe_slash_picker_render
}

aishe-slash-picker-accept() {
  (( ${#_AISHE_SLASH_PICKER_MATCHES} )) || { zle beep; return; }
  zle .accept-line
}

_aishe_slash_picker() {
  emulate -L zsh
  setopt extendedglob
  if [[ "${_AISHE_SLASH_DISCOVERED:-0}" != 1 ]]; then
    local reply
    reply="$(_aishe_lean_send COMMANDS)" || return 1
    [[ "$reply" == OK ]] || return 1
    typeset -g _AISHE_SLASH_DISCOVERED=1
  fi
  local _AISHE_SLASH_PICKER_QUERY=''
  local -i _AISHE_SLASH_PICKER_INDEX=1 accepted=0
  local -a _AISHE_SLASH_PICKER_NAMES=("${_AISHE_SLASH_NAMES[@]}") _AISHE_SLASH_PICKER_MATCHES
  local -A _AISHE_SLASH_PICKER_DESCRIPTIONS=("${(@kv)_AISHE_SLASH_DESCRIPTIONS}")
  local name description old_buffer="$BUFFER" old_cursor="$CURSOR"
  local old_postdisplay="$POSTDISPLAY" old_keymap="$KEYMAP"
  if [[ -n "${AISHE_LEAN_CMDS_FILE:-}" && -r "$AISHE_LEAN_CMDS_FILE" ]]; then
    while IFS=$'\t' read -r name description; do
      [[ "$name" == [[:alnum:]_-]## ]] || continue
      [[ -n "${_AISHE_SLASH_PICKER_DESCRIPTIONS[$name]:-}" ]] && continue
      _aishe_lean_prompt_value "$description"
      description="${REPLY:-Custom command}"
      _AISHE_SLASH_PICKER_NAMES+=("$name")
      _AISHE_SLASH_PICKER_DESCRIPTIONS[$name]="$description"
    done < "$AISHE_LEAN_CMDS_FILE"
  fi
  zle -K aishe-slash-picker
  _aishe_slash_picker_render
  zle recursive-edit && accepted=1
  POSTDISPLAY="$old_postdisplay"
  BUFFER="$old_buffer"
  CURSOR="$old_cursor"
  zle -K "$old_keymap"
  if (( accepted )); then
    local leading="${old_buffer%%[^[:space:]]*}"
    BUFFER="${leading}/${_AISHE_SLASH_PICKER_MATCHES[$_AISHE_SLASH_PICKER_INDEX]} "
    CURSOR=${#BUFFER}
  fi
  zle -R
}

aishe-slash-tab() {
  emulate -L zsh
  setopt extendedglob
  local leading="${BUFFER%%[^[:space:]]*}"
  local trimmed="${BUFFER#$leading}" head="${BUFFER#$leading}"
  head="${head%%[[:space:]]*}"
  if [[ "$head" != /* || "$head" == //* || "$head" == /*/* ||
        "$trimmed" == *$'\n'* ]] || (( CURSOR > ${#leading} + ${#head} )); then
    local REPLY
    _aishe_effective_keymap
    zle "${_AISHE_USER_TAB_NAMES[$REPLY]:-expand-or-complete}" -w
    return
  fi
  if [[ "$head" == / && "$trimmed" == / ]]; then
    _aishe_slash_picker
    return
  fi
  zle aishe-complete-slashes
}

# Early native configuration: aliases, environment, completion paths and widgets.
if [[ -n "${AISHE_LEANRC:-}" && -r "${AISHE_LEANRC}" ]]; then
  source "${AISHE_LEANRC}"
elif [[ -r "${HOME}/.aishe/leanrc" ]]; then
  source "${HOME}/.aishe/leanrc"
fi


# Bounded clean-profile compsys. The private cache survives shell launches.
# Personal profiles retain their existing completion setup and cache policy.
# Always pass `-u`: CI images (esp. macOS runners) ship group-writable
# /usr/share/zsh; plain `compinit` then prompts on a TTY and deadlocks nested
# PTY tests that cannot answer the security question.
if [[ -o interactive ]]; then
  if (( ! $+functions[compdef] )); then
    autoload -Uz compinit 2>/dev/null || true
    if (( $+functions[compinit] )); then
      typeset -g _AISHE_COMPLETION_ROOT="${AISHE_COMPLETION_CACHE:-${ZDOTDIR:-${HOME}}}"
      # A different completion search path must not reuse an incompatible dump.
      typeset -gi _AISHE_FPATH_HASH=5381
      typeset -g _AISHE_FPATH_TEXT="${(j.:.)fpath}"
      typeset -gi _AISHE_FPATH_MTIME=0 _AISHE_COMPDUMP_FAST=0
      typeset -gA _AISHE_FPATH_STAT
      typeset -g _AISHE_FPATH_DIR
      if zmodload zsh/stat 2>/dev/null; then
        for _AISHE_FPATH_DIR in "${fpath[@]}"; do
          if zstat -H _AISHE_FPATH_STAT +mtime "$_AISHE_FPATH_DIR" 2>/dev/null; then
            _AISHE_FPATH_TEXT+=":${_AISHE_FPATH_STAT[mtime]}"
            (( _AISHE_FPATH_STAT[mtime] > _AISHE_FPATH_MTIME )) && _AISHE_FPATH_MTIME=${_AISHE_FPATH_STAT[mtime]}
          else
            _AISHE_FPATH_TEXT+=':missing'
          fi
        done
      fi
      typeset -gi _AISHE_FPATH_INDEX
      for (( _AISHE_FPATH_INDEX=1; _AISHE_FPATH_INDEX<=${#_AISHE_FPATH_TEXT}; _AISHE_FPATH_INDEX++ )); do
        (( _AISHE_FPATH_HASH = ((_AISHE_FPATH_HASH * 33) ^ #_AISHE_FPATH_TEXT[$_AISHE_FPATH_INDEX]) & 0x7fffffff ))
      done
      typeset -g _AISHE_COMPDUMP="${_AISHE_COMPLETION_ROOT}/.zcompdump-${ZSH_VERSION}-${_AISHE_FPATH_HASH}"
      if [[ -s "$_AISHE_COMPDUMP" ]] && zstat -H _AISHE_FPATH_STAT +mtime "$_AISHE_COMPDUMP" 2>/dev/null; then
        # If a directory changed in the same second as this dump, let compinit
        # check its function count. This closes coarse-mtime installation races.
        (( _AISHE_FPATH_MTIME < _AISHE_FPATH_STAT[mtime] )) && _AISHE_COMPDUMP_FAST=1
      fi
      typeset -g _AISHE_COMPCACHE="${_AISHE_COMPLETION_ROOT}/.zcompcache"
      (umask 077; mkdir -p "${_AISHE_COMPCACHE}") 2>/dev/null || true
      if [[ "${AISHE_ZSH_PROFILE:-clean}" == clean ]]; then
        zstyle ':completion:*' use-cache on
        zstyle ':completion:*' cache-path "${_AISHE_COMPCACHE}"
      fi
      if [[ -s "${_AISHE_COMPDUMP}" && "$_AISHE_COMPDUMP_FAST" == 1 ]]; then
        compinit -u -d "${_AISHE_COMPDUMP}" -C
      else
        _aishe_compinit_private() {
          local saved_umask="$(umask)" result
          umask 077
          compinit -u -d "${_AISHE_COMPDUMP}"
          result=$?
          umask "$saved_umask"
          return "$result"
        }
        _aishe_compinit_private
      fi
    fi
  fi
  if (( $+functions[compdef] )); then
    compdef _aishe_complete_slash_arguments /help /mode /context /doctor
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
      if _aishe_has_assignment_head "$word"; then
        # The tokenizer splits array values after `name=(` into further words.
        # Once a command starts with shell assignment syntax, let zsh parse the
        # whole compound statement instead of routing an array item as NL.
        return 1
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

_aishe_capture_execution_state() {
  emulate -L zsh
  setopt extendedglob
  [[ -n "${_AISHE_STATE_CONTROL_FILE:-}" ]] || return 0
  zmodload zsh/system 2>/dev/null && zmodload zsh/files 2>/dev/null || return 1
  local _AISHE_STATE_FILE="$_AISHE_STATE_CONTROL_FILE"
  local _AISHE_STATE_TEMPORARY="${_AISHE_STATE_FILE}.tmp.${sysparams[pid]}.${RANDOM}${RANDOM}"
  local _AISHE_STATE_NAME _AISHE_STATE_UPPER _AISHE_STATE_VALUE _AISHE_STATE_DENIED_NAME
  local -a _AISHE_STATE_CONFIGURED_DENY
  local -A _AISHE_STATE_DENIED
  _AISHE_STATE_CONFIGURED_DENY=("${(@f)_AISHE_STATE_CONTROL_DENY}")
  for _AISHE_STATE_DENIED_NAME in "${_AISHE_STATE_CONFIGURED_DENY[@]}"; do
    [[ -n "$_AISHE_STATE_DENIED_NAME" ]] && _AISHE_STATE_DENIED[${(U)_AISHE_STATE_DENIED_NAME}]=1
  done
  local -i _AISHE_STATE_STATE_FD _AISHE_STATE_BYTES=13 _AISHE_STATE_COUNT=0 _AISHE_STATE_FAILED=0 _AISHE_STATE_VALUE_BYTES
  # The parent owns this private directory. Exclusive/no-follow creation keeps
  # this handoff separate from user files; a partial snapshot is never sent.
  sysopen -w -o creat,excl,nofollow -m 0600 -u _AISHE_STATE_STATE_FD "$_AISHE_STATE_TEMPORARY" 2>/dev/null || return 1
  print -rn -u "$_AISHE_STATE_STATE_FD" -- $'AISHE_ENV_V1\0' || _AISHE_STATE_FAILED=1
  for _AISHE_STATE_NAME in ${(ok)parameters}; do
    [[ "${parameters[$_AISHE_STATE_NAME]}" == *export* ]] || continue
    case "${parameters[$_AISHE_STATE_NAME]}" in
      array-*|association-*) continue ;;
    esac
    [[ "$_AISHE_STATE_NAME" == [A-Za-z_][A-Za-z0-9_]# ]] || continue
    _AISHE_STATE_UPPER="${(U)_AISHE_STATE_NAME}"
    case "$_AISHE_STATE_UPPER" in
      AISHE_*|_AISHE_*|OPENCODE_*|LD_*|DYLD_*|ENV|BASH_ENV|SHELLOPTS|BASHOPTS|ZDOTDIR|*TOKEN*|*SECRET*|*PASSWORD*|*PASSWD*|*API_KEY*|*APIKEY*|*AUTHORIZATION*|*CREDENTIAL*|*PRIVATE_KEY*|*ACCESS_KEY*) continue ;;
    esac
    [[ "${_AISHE_STATE_DENIED[$_AISHE_STATE_UPPER]:-0}" == 1 ]] && continue
    _AISHE_STATE_VALUE="${(P)_AISHE_STATE_NAME}"
    # OS environment values cannot contain NUL. Reject a zsh-only value rather
    # than interpreting its bytes as another record in this framing protocol.
    if [[ "$_AISHE_STATE_VALUE" == *$'\0'* ]]; then
      _AISHE_STATE_FAILED=1
      break
    fi
    # Measure bytes under a function-local locale; exported locale values in
    # the snapshot remain exactly what the person configured in their shell.
    () { local LC_ALL=C; _AISHE_STATE_VALUE_BYTES=${#_AISHE_STATE_VALUE}; }
    (( _AISHE_STATE_COUNT++, _AISHE_STATE_BYTES += ${#_AISHE_STATE_NAME} + _AISHE_STATE_VALUE_BYTES + 2 ))
    if (( _AISHE_STATE_COUNT > 512 || _AISHE_STATE_BYTES > 262144 || _AISHE_STATE_VALUE_BYTES > 65536 || ${#_AISHE_STATE_NAME} > 128 )); then
      _AISHE_STATE_FAILED=1
      break
    fi
    print -rn -u "$_AISHE_STATE_STATE_FD" -- "$_AISHE_STATE_NAME"$'\0'"$_AISHE_STATE_VALUE"$'\0' || { _AISHE_STATE_FAILED=1; break; }
  done
  exec {_AISHE_STATE_STATE_FD}>&-
  if (( _AISHE_STATE_FAILED )); then
    zf_rm -f -- "$_AISHE_STATE_TEMPORARY" 2>/dev/null
    return 1
  fi
  zf_mv -f -- "$_AISHE_STATE_TEMPORARY" "$_AISHE_STATE_FILE" 2>/dev/null || {
    zf_rm -f -- "$_AISHE_STATE_TEMPORARY" 2>/dev/null
    return 1
  }
}

_aishe_lean_send() {
  emulate -L zsh
  local _AISHE_STATE_PAYLOAD="$1" _AISHE_STATE_REPLY
  [[ -n "${AISHE_LEAN_REQ:-}" && -p "${AISHE_LEAN_REQ}" &&
     -n "${AISHE_LEAN_REP:-}" && -p "${AISHE_LEAN_REP}" ]] || {
    print -u2 -- 'aishe: lean IPC is not connected'
    return 1
  }
  case "${_AISHE_STATE_PAYLOAD%%$'\t'*}" in
    NL|FIX|CONFIRM_YES|SLASH)
      _aishe_capture_execution_state || {
        print -u2 -- 'aishe: live shell state unavailable; request was not submitted'
        return 1
      }
      ;;
  esac
  print -r -- "$_AISHE_STATE_PAYLOAD" > "$AISHE_LEAN_REQ" || return 1
  IFS= read -r -t 120 _AISHE_STATE_REPLY < "$AISHE_LEAN_REP" || {
    print -u2 -- 'aishe: lean NL timed out'
    return 1
  }
  print -r -- "$_AISHE_STATE_REPLY"
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
  # Publish readiness only after stty finishes: on macOS its transition can
  # flush a reply that arrived while the terminal was still changing modes.
  [[ -n "${1:-}" ]] && print -n -- "$1"
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
      if ! _aishe_lean_read_grant "Type ${want} to continue (Esc cancels): " || [[ "$REPLY" != "$want" ]]; then
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
  local _AISHE_LOCAL_REPLY="$1"
  local _AISHE_LOCAL_KIND="${_AISHE_LOCAL_REPLY%%	*}"
  local _AISHE_LOCAL_REST="${_AISHE_LOCAL_REPLY#*$'\t'}"
  [[ "$_AISHE_LOCAL_KIND" == "$_AISHE_LOCAL_REPLY" ]] && _AISHE_LOCAL_REST=""
  case "$_AISHE_LOCAL_KIND" in
    OK|STREAM_END)
      # Parent already wrote multi-line / streamed answer onto terminal output.
      ;;
    CANCELLED)
      print -r -- "cancelled"
      ;;
    ANSWER)
      # Legacy one-liner fallback.
      [[ -n "$_AISHE_LOCAL_REST" ]] && print -r -- "$_AISHE_LOCAL_REST"
      ;;
    ANSWER_B64)
      local _AISHE_LOCAL_TEXT
      _AISHE_LOCAL_TEXT="$(_aishe_lean_b64_decode "$_AISHE_LOCAL_REST")"
      [[ -n "$_AISHE_LOCAL_TEXT" ]] && print -r -- "$_AISHE_LOCAL_TEXT"
      ;;
    FILL|FILL_B64)
      local _AISHE_LOCAL_CMD="$_AISHE_LOCAL_REST"
      if [[ "$_AISHE_LOCAL_KIND" == FILL_B64 ]]; then
        _AISHE_LOCAL_CMD="$(_aishe_lean_b64_decode "$_AISHE_LOCAL_REST")"
      fi
      typeset -g _AISHE_STAGED_SUGGESTION=1
      print -z -- "$_AISHE_LOCAL_CMD"
      ;;
    RAN)
      [[ -n "$_AISHE_LOCAL_REST" ]] && print -r -- "$_AISHE_LOCAL_REST"
      ;;
    ERROR)
      print -u2 -- "aishe: $_AISHE_LOCAL_REST"
      ;;
    CONFIRM|CONFIRM_B64)
      local _AISHE_LOCAL_BODY="$_AISHE_LOCAL_REST"
      if [[ "$_AISHE_LOCAL_KIND" == CONFIRM_B64 ]]; then
        _AISHE_LOCAL_BODY="$(_aishe_lean_b64_decode "$_AISHE_LOCAL_REST")"
      fi
      print -r -- "Dangerous / unknown: $_AISHE_LOCAL_BODY"
      print -n -- "Type yes to run: "
      local _AISHE_LOCAL_ANS
      if [[ -n "${_AISHE_INPUT_FD:-}" && "_AISHE_INPUT_FD" -ge 0 ]]; then
        IFS= read -r _AISHE_LOCAL_ANS <&$_AISHE_INPUT_FD
      else
        IFS= read -r _AISHE_LOCAL_ANS
      fi
      if [[ "$_AISHE_LOCAL_ANS" == yes ]]; then
        local _AISHE_LOCAL_AGAIN _AISHE_LOCAL_PAYLOAD
        if [[ "$_AISHE_LOCAL_KIND" == CONFIRM_B64 ]]; then
          _AISHE_LOCAL_PAYLOAD="$_AISHE_LOCAL_REST"
        else
          _AISHE_LOCAL_PAYLOAD="$(_aishe_lean_flatten "$_AISHE_LOCAL_BODY")"
        fi
        _AISHE_LOCAL_AGAIN="$(_aishe_lean_send "CONFIRM_YES	$_AISHE_LOCAL_PAYLOAD")"
        _aishe_lean_handle_reply "$_AISHE_LOCAL_AGAIN"
      else
        print -r -- "cancelled"
      fi
      ;;
    *)
      [[ -n "$_AISHE_LOCAL_REPLY" ]] && print -r -- "$_AISHE_LOCAL_REPLY"
      ;;
  esac
}

_aishe_lean_nl() {
  emulate -L zsh
  local _AISHE_LOCAL_LINE="$1"
  [[ -z "$_AISHE_LOCAL_LINE" ]] && return
  _aishe_lean_take_grant "$(_aishe_lean_grant_word)" || return 0
  local _AISHE_LOCAL_PAYLOAD _AISHE_LOCAL_REPLY
  _AISHE_LOCAL_PAYLOAD="NL	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$_AISHE_LOCAL_LINE")"
  _AISHE_LOCAL_REPLY="$(_aishe_lean_send "$_AISHE_LOCAL_PAYLOAD")" || return
  _aishe_lean_handle_reply "$_AISHE_LOCAL_REPLY"
}

_aishe_lean_slash() {
  emulate -L zsh
  setopt extendedglob
  local _AISHE_LOCAL_LINE="$1"
  local _AISHE_LOCAL_NAME="${_AISHE_LOCAL_LINE%%[[:space:]]*}"
  local _AISHE_LOCAL_ARG="${_AISHE_LOCAL_LINE#"$_AISHE_LOCAL_NAME"}"
  _AISHE_LOCAL_ARG="${_AISHE_LOCAL_ARG##[[:space:]]#}"
  _AISHE_LOCAL_ARG="${_AISHE_LOCAL_ARG%%[[:space:]]#}"
  local -a _AISHE_LOCAL_CLI_ARGS
  # Tokenize quoted CLI arguments without evaluating substitutions or globs.
  _AISHE_LOCAL_CLI_ARGS=()
  [[ -n "$_AISHE_LOCAL_ARG" ]] && _AISHE_LOCAL_CLI_ARGS=("${(@Q)${(z)_AISHE_LOCAL_ARG}}")
  case "$_AISHE_LOCAL_NAME" in
    /)
      local _AISHE_LOCAL_REPLY
      _AISHE_LOCAL_REPLY="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	/help")" || return
      _aishe_lean_handle_reply "$_AISHE_LOCAL_REPLY"
      ;;
    /mode)
      _aishe_lean_mark_mode_interaction
      if [[ -z "$_AISHE_LOCAL_ARG" ]]; then
        print -r -- "mode: ${AISHE_MODE:-ask} (${AISHE_SCOPE:-workspace})"
        return 0
      fi
      case "${_AISHE_LOCAL_ARG:l}" in
        ask|suggest) _AISHE_LOCAL_ARG=ask ;;
        allow|auto) _AISHE_LOCAL_ARG=allow ;;
        agent|yolo) _AISHE_LOCAL_ARG=agent ;;
        agent-host) ;;
        *) print -u2 -r -- "aishe: unknown mode '$_AISHE_LOCAL_ARG' (ask|allow|agent|agent-host)"; return 0 ;;
      esac
      _aishe_lean_take_grant "$_AISHE_LOCAL_ARG" || return 0
      aishe_set_prompt
      print -r -- "mode: ${AISHE_MODE} (${AISHE_SCOPE})"
      ;;
    /model|/connection)
      if [[ -z "$_AISHE_LOCAL_ARG" || "$_AISHE_LOCAL_ARG" == default ]]; then
        # Interactive prompts belong to the inner shell's real terminal, not
        # the parent FIFO worker. The selection handoff updates the next turn.
        if [[ "$_AISHE_LOCAL_NAME" == /model ]]; then
          if [[ -n "$_AISHE_LOCAL_ARG" ]]; then command aishe model "$_AISHE_LOCAL_ARG" <&$_AISHE_INPUT_FD
          else command aishe model <&$_AISHE_INPUT_FD; fi
        else
          if [[ -n "$_AISHE_LOCAL_ARG" ]]; then command aishe connection pick "$_AISHE_LOCAL_ARG" <&$_AISHE_INPUT_FD
          else command aishe connection pick <&$_AISHE_INPUT_FD; fi
        fi
      else
        local _AISHE_LOCAL_REPLY
        _AISHE_LOCAL_REPLY="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$_AISHE_LOCAL_LINE")")" || return
        _aishe_lean_handle_reply "$_AISHE_LOCAL_REPLY"
      fi
      ;;
    /settings) command aishe settings "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /setup) command aishe setup "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /tour) command aishe tour "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /context) command aishe context "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /doctor) command aishe doctor "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /inbox) command aishe inbox "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD ;;
    /workflow)
      if (( ${#_AISHE_LOCAL_CLI_ARGS} )); then
        command aishe workflow "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD
      else
        command aishe workflow browse <&$_AISHE_INPUT_FD
      fi
      ;;
    /tasks)
      # The browser and any explicit action own the child terminal. Forward
      # tokenized arguments literally; model text never becomes shell syntax.
      if (( ${#_AISHE_LOCAL_CLI_ARGS} == 1 )) && [[ "${_AISHE_LOCAL_CLI_ARGS[1]}" == [[:xdigit:]]##-[[:xdigit:]]## ]]; then
        command aishe task browse "${_AISHE_LOCAL_CLI_ARGS[1]}" <&$_AISHE_INPUT_FD
      elif (( ${#_AISHE_LOCAL_CLI_ARGS} )); then
        command aishe task "${(@)_AISHE_LOCAL_CLI_ARGS}" <&$_AISHE_INPUT_FD
      else
        command aishe task browse <&$_AISHE_INPUT_FD
      fi
      ;;
    /help|/commands|/status|/reset|/undo|/usage|/details|/mcp|/skills|/sessions|/backend)
      local _AISHE_LOCAL_REPLY
      _AISHE_LOCAL_REPLY="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$_AISHE_LOCAL_LINE")")" || return
      _aishe_lean_handle_reply "$_AISHE_LOCAL_REPLY"
      ;;
    /*/*)
      # Absolute path with extra segments — leave to shell.
      return 1
      ;;
    /[[:alnum:]_-]##)
      # Custom markdown slash-command → FIFO (F40).
      local _AISHE_LOCAL_REPLY
      _AISHE_LOCAL_REPLY="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	$(_aishe_lean_flatten "$_AISHE_LOCAL_LINE")")" || return
      _aishe_lean_handle_reply "$_AISHE_LOCAL_REPLY"
      ;;
    *)
      return 1
      ;;
  esac
  return 0
}


# Unknown command: do not spawn aishe. Route the accepted line as NL.
command_not_found_handler() {
  local _AISHE_LOCAL_LINE="${(j: :)@}"
  [[ -n "${_AISHE_ACCEPTED_LINE:-}" && "$_AISHE_LOCAL_LINE" == "$_AISHE_ACCEPTED_LINE" ]] || return 127
  _aishe_lean_nl "$_AISHE_LOCAL_LINE"
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
  local _AISHE_LOCAL_REPLY
  _AISHE_LOCAL_REPLY="$(_aishe_lean_send "FIX	${AISHE_MODE:-ask}	$PWD	fix")" || {
    zle -M "aishe: fix request failed"
    return
  }
  local _AISHE_LOCAL_KIND="${_AISHE_LOCAL_REPLY%%	*}"
  local _AISHE_LOCAL_REST="${_AISHE_LOCAL_REPLY#*$'	'}"
  [[ "$_AISHE_LOCAL_KIND" == "$_AISHE_LOCAL_REPLY" ]] && _AISHE_LOCAL_REST=""
  case "$_AISHE_LOCAL_KIND" in
    FILL_B64)
      local _AISHE_LOCAL_DECODED
      _AISHE_LOCAL_DECODED="$(print -r -- "$_AISHE_LOCAL_REST" | base64 -d 2>/dev/null)" || _AISHE_LOCAL_DECODED=""
      if [[ -n "$_AISHE_LOCAL_DECODED" ]]; then
        BUFFER="$_AISHE_LOCAL_DECODED"
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
      zle -M "aishe: ${_AISHE_LOCAL_REST:-fix failed}"
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


# Open background work without submitting, saving, or replacing the draft. The
# child browser owns terminal input; the parent's IPC thread never reads it.
aishe-background-tasks() {
  emulate -L zsh
  local _AISHE_BACKGROUND_BUFFER="$BUFFER" _AISHE_BACKGROUND_CURSOR="$CURSOR"
  local _AISHE_BACKGROUND_MARK="$MARK" _AISHE_BACKGROUND_REGION="$REGION_ACTIVE"
  local _AISHE_BACKGROUND_LAST_EXIT="${AISHE_LAST_EXIT:-0}"
  zle -I
  {
    command aishe task browse <&$_AISHE_INPUT_FD
  } always {
    BUFFER="$_AISHE_BACKGROUND_BUFFER"
    CURSOR="$_AISHE_BACKGROUND_CURSOR"
    MARK="$_AISHE_BACKGROUND_MARK"
    REGION_ACTIVE="$_AISHE_BACKGROUND_REGION"
    AISHE_LAST_EXIT="$_AISHE_BACKGROUND_LAST_EXIT"
    aishe_set_prompt
    zle reset-prompt
  }
  return 0
}

# Density toggle (default Ctrl-O; override with AISHE_DETAILS_KEY). Parent owns
# config.backend.output + PtyOut message; child syncs AISHE_AGENT_OUTPUT.
aishe-toggle-agent-details() {
  emulate -L zsh
  local _AISHE_LOCAL_REPLY
  # The parent prints a persistent notice. Give it a clear output line, then
  # redraw the same input buffer instead of writing into the active ZLE row.
  zle -I
  _AISHE_LOCAL_REPLY="$(_aishe_lean_send "SLASH	${AISHE_MODE:-ask}	$PWD	/details")" || {
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
  local _AISHE_LOCAL_SUBMITTED="$BUFFER"
  print -s -- "$_AISHE_LOCAL_SUBMITTED"
  zle -I
  _aishe_lean_nl "$_AISHE_LOCAL_SUBMITTED"
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
  # User widgets often call accept-line themselves. End their chain at zsh's
  # built-in instead of recursively routing the same line back through AIShe.
  if [[ "${_AISHE_CHAINING_ACCEPT:-0}" == 1 ]]; then
    zle .accept-line
    return
  fi
  setopt extendedglob
  local _AISHE_LOCAL_LINE="$BUFFER"
  local _AISHE_LOCAL_TRIMMED="${_AISHE_LOCAL_LINE##[[:space:]]#}"
  _AISHE_LOCAL_TRIMMED="${_AISHE_LOCAL_TRIMMED%%[[:space:]]#}"

  # A spaced `! command` is AIShe's explicit shell route. Keep native zsh
  # history forms (!!, !$, !word, !?word?, !-n, !n) byte-for-byte intact.
  if [[ "$_AISHE_LOCAL_TRIMMED" == '!'[[:space:]]* ]]; then
    BUFFER="${_AISHE_LOCAL_TRIMMED#\!}"
    BUFFER="${BUFFER##[[:space:]]#}"
    _aishe_accept_shell_line
    return
  fi

  if [[ "$_AISHE_LOCAL_TRIMMED" == /* && "$_AISHE_LOCAL_TRIMMED" != //* ]]; then
    zle -I
    if _aishe_lean_slash "$_AISHE_LOCAL_TRIMMED"; then
      print -s -- "$_AISHE_LOCAL_TRIMMED"
      BUFFER=""
      POSTDISPLAY=""
      # The slash completed inside this widget. Refresh its existing input
      # row rather than accepting a second empty shell line and prompt.
      aishe_set_prompt
      zle reset-prompt
      return
    fi
  fi

  if _aishe_routes_to_agent "$_AISHE_LOCAL_TRIMMED"; then
    local _AISHE_LOCAL_BODY="$_AISHE_LOCAL_TRIMMED"
    local _AISHE_LOCAL_WAS_Q=0
    [[ "${_AISHE_LOCAL_BODY[1]}" == '?' ]] && { _AISHE_LOCAL_BODY="${_AISHE_LOCAL_BODY#?}"; _AISHE_LOCAL_WAS_Q=1 }
    _AISHE_LOCAL_BODY="${_AISHE_LOCAL_BODY##[[:space:]]#}"
    print -s -- "$_AISHE_LOCAL_TRIMMED"
    zle -I
    if [[ -n "$_AISHE_LOCAL_BODY" ]]; then
      _aishe_lean_nl "$_AISHE_LOCAL_BODY"
    elif (( _AISHE_LOCAL_WAS_Q )); then
      # Empty `?` → explain last failure capsule (lean-native, no OpenCode).
      _aishe_lean_nl "?"
    fi
    BUFFER=""
    POSTDISPLAY=""
    zle .accept-line
    return
  fi

  _aishe_accept_shell_line
}

_aishe_accept_shell_line() {
  local _AISHE_CHAINING_ACCEPT=1
  local REPLY
  _aishe_effective_keymap
  local _AISHE_ENTER_TARGET="${_AISHE_USER_ACCEPT_NAMES[$REPLY]:-accept-line}"
  if [[ "$_AISHE_ENTER_TARGET" == accept-line ]]; then
    # Give a plugin its original WIDGET=accept-line while it runs. Restore the
    # actual live wrapper afterward, including a plugin installed in leanrc.post.
    zle -A accept-line _aishe-live-accept-line
    zle -A _aishe-user-accept-line accept-line
    { zle accept-line -w } always {
      zle -A _aishe-live-accept-line accept-line
    }
  else
    zle "$_AISHE_ENTER_TARGET" -w
  fi
}

_aishe_effective_keymap() {
  local keymap="${KEYMAP:-main}"
  local -a mapping
  mapping=(${(z)$(bindkey -l -L "$keymap" 2>/dev/null)})
  # main is an alias for emacs or viins; resolve it at use time so `bindkey -v`
  # after launch chooses the correct saved user widgets as well.
  [[ "${mapping[2]:-}" == -A ]] && keymap="${mapping[3]}"
  REPLY="$keymap"
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
  zle -N aishe-background-tasks
  zle -N aishe-background-event
  zle -N aishe-slash-tab
  zle -N aishe-slash-picker-insert
  zle -N aishe-slash-picker-delete
  zle -N aishe-slash-picker-paste
  zle -N aishe-slash-picker-clear
  zle -N aishe-slash-picker-next
  zle -N aishe-slash-picker-previous
  zle -N aishe-slash-picker-accept
  bindkey -N aishe-slash-picker
  bindkey -M aishe-slash-picker -R ' '-'\M-^?' aishe-slash-picker-insert
  bindkey -M aishe-slash-picker '^?' aishe-slash-picker-delete
  bindkey -M aishe-slash-picker '^[[200~' aishe-slash-picker-paste
  bindkey -M aishe-slash-picker '^H' aishe-slash-picker-delete
  bindkey -M aishe-slash-picker '^U' aishe-slash-picker-clear
  bindkey -M aishe-slash-picker '^I' aishe-slash-picker-next
  bindkey -M aishe-slash-picker '^N' aishe-slash-picker-next
  bindkey -M aishe-slash-picker '^P' aishe-slash-picker-previous
  bindkey -M aishe-slash-picker '^[[B' aishe-slash-picker-next
  bindkey -M aishe-slash-picker '^[[A' aishe-slash-picker-previous
  bindkey -M aishe-slash-picker '^[[Z' aishe-slash-picker-previous
  bindkey -M aishe-slash-picker '^M' aishe-slash-picker-accept
  bindkey -M aishe-slash-picker '^J' aishe-slash-picker-accept
  bindkey -M aishe-slash-picker '^[' send-break
  bindkey -M aishe-slash-picker '^C' send-break
  bindkey -M aishe-slash-picker '^D' send-break
  zle -C aishe-complete-slashes complete-word _aishe_slash_completion
  zstyle ':completion:aishe-slashes:*' group-name ''
  zstyle ':completion:aishe-slashes:*' list-grouped true
  zstyle ':completion:aishe-slashes:*' format '%d'
  zstyle ':completion:aishe-slashes:*' menu auto select
  zle -N _aishe_highlight_command
  zle -A accept-line _aishe-user-accept-line
  typeset -gA _AISHE_USER_ACCEPT_NAMES _AISHE_USER_TAB_NAMES
  typeset -g _aishe_keymap _aishe_old_widget
  typeset -ga _aishe_binding
  for _aishe_keymap in emacs viins vicmd; do
    # Capture the actual Enter/Tab binding in each keymap, not only the named
    # accept-line widget. Plugins can bind Enter directly to their own widget.
    _aishe_binding=(${(z)$(bindkey -M "$_aishe_keymap" '^M' 2>/dev/null)})
    _aishe_old_widget="${_aishe_binding[2]:-accept-line}"
    _AISHE_USER_ACCEPT_NAMES[$_aishe_keymap]="$_aishe_old_widget"
    _aishe_binding=(${(z)$(bindkey -M "$_aishe_keymap" '^I' 2>/dev/null)})
    _aishe_old_widget="${_aishe_binding[2]:-expand-or-complete}"
    _AISHE_USER_TAB_NAMES[$_aishe_keymap]="$_aishe_old_widget"
    bindkey -M "$_aishe_keymap" '^M' aishe-accept-line
    bindkey -M "$_aishe_keymap" '^I' aishe-slash-tab
  done
  zle -A aishe-accept-line accept-line
  autoload -Uz add-zle-hook-widget 2>/dev/null
  add-zle-hook-widget zle-line-pre-redraw _aishe_highlight_command 2>/dev/null || true
  if (( ${+widgets[reverse-menu-complete]} )); then
    typeset -g _AISHE_ORIG_MODE_WIDGET=reverse-menu-complete
  fi
  _aishe_bind_optional() {
    local keymap="$1" key="$2" widget="$3" explicit="$4"
    local -a binding
    binding=(${(z)$(bindkey -M "$keymap" "$key" 2>/dev/null)})
    # Personal custom shortcuts win unless an AISHE_*_KEY explicitly requests
    # that shortcut. Clean profile keeps the established AIShe defaults.
    if [[ "${AISHE_ZSH_PROFILE:-clean}" == clean || "$explicit" == 1 ||
          "${binding[2]:-undefined-key}" == undefined-key ]]; then
      bindkey -M "$keymap" "$key" "$widget"
    fi
  }
  for _aishe_keymap in emacs viins; do
    _aishe_bind_optional "$_aishe_keymap" "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget "${+AISHE_NL_KEY}"
    _aishe_bind_optional "$_aishe_keymap" "${AISHE_FIX_KEY:-^X^F}" aishe-fix-command "${+AISHE_FIX_KEY}"
    _aishe_bind_optional "$_aishe_keymap" "${AISHE_ROUTE_KEY:-^X?}" aishe-show-route "${+AISHE_ROUTE_KEY}"
    _aishe_bind_optional "$_aishe_keymap" "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode "${+AISHE_MODE_KEY}"
    _aishe_bind_optional "$_aishe_keymap" "${AISHE_DETAILS_KEY:-^O}" aishe-toggle-agent-details "${+AISHE_DETAILS_KEY}"
    # A new shortcut is opt-in when already bound, even in the clean profile.
    # Ctrl-X followed by the letter b leaves conventional Ctrl-B untouched.
    _aishe_binding=(${(z)$(bindkey -M "$_aishe_keymap" "${AISHE_BACKGROUND_KEY:-^Xb}" 2>/dev/null)})
    if [[ "${+AISHE_BACKGROUND_KEY}" == 1 || "${_aishe_binding[2]:-undefined-key}" == undefined-key ]]; then
      bindkey -M "$_aishe_keymap" "${AISHE_BACKGROUND_KEY:-^Xb}" aishe-background-tasks
    fi
  done
  if [[ -n "$_AISHE_BACKGROUND_CONTROL_EVENTS" && -p "$_AISHE_BACKGROUND_CONTROL_EVENTS" &&
        ! -L "$_AISHE_BACKGROUND_CONTROL_EVENTS" ]] && zmodload zsh/system 2>/dev/null; then
    typeset -gi _AISHE_BACKGROUND_EVENT_FD
    if sysopen -r -w -o nonblock,nofollow -u _AISHE_BACKGROUND_EVENT_FD "$_AISHE_BACKGROUND_CONTROL_EVENTS" 2>/dev/null; then
      zle -F -w "$_AISHE_BACKGROUND_EVENT_FD" aishe-background-event
    fi
  fi
  # Late native customization deliberately sees the installed AIShe widgets.
  # It can override a binding or extend a widget without being replaced below.
  if [[ -n "${AISHE_LEANRC_POST:-}" && -r "$AISHE_LEANRC_POST" ]]; then
    source "$AISHE_LEANRC_POST"
  elif [[ -r "$HOME/.aishe/leanrc.post" ]]; then
    source "$HOME/.aishe/leanrc.post"
  fi
  # This is the actual presentation boundary. Merely spawning zsh must not
  # consume the persisted first-launch hint, because user startup can abort.
  if [[ "${AISHE_COMMAND_HINT_SHOWN:-0}" != 1 ]]; then
    local _aishe_launch_hint='AIShe: /help | ? ask | Shift-Tab mode'
    (( ${COLUMNS:-80} < 40 )) && _aishe_launch_hint='/help | ? ask | Shift-Tab mode'
    if print -r -- "$_aishe_launch_hint"; then
      typeset -gx AISHE_COMMAND_HINT_SHOWN=1
      if [[ -n "$_AISHE_LAUNCH_HINT_ACK" ]]; then
        print -r -- shown >| "$_AISHE_LAUNCH_HINT_ACK" 2>/dev/null || true
      fi
    fi
  fi
  aishe_set_prompt
fi
