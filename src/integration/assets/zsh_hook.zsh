: ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}
export AISHE_PENDING_FILE
: ${AISHE_FORCE_FILE:=${TMPDIR:-/tmp}/aishe-force-$$}
# Conversation memory shared across the per-call NL invocations in this shell, so
# follow-ups keep context. Exported so `command aishe` inherits it.
: ${AISHE_SESSION_FILE:=${TMPDIR:-/tmp}/aishe-session-mem-$$}
export AISHE_SESSION_FILE
# Stable only for this live shell. Hook subprocesses inherit it so backend
# sessions and foreground tool leases cannot cross between terminals.
if [[ -z "${AISHE_SHELL_ID:-}" ]]; then
  AISHE_SHELL_ID="$(command od -An -N24 -tx1 /dev/urandom 2>/dev/null | command tr -d ' \n')"
  [[ -n "$AISHE_SHELL_ID" ]] || AISHE_SHELL_ID="shell-${$}-${EPOCHREALTIME:-$RANDOM$RANDOM}"
fi
export AISHE_SHELL_ID
# Yolo acceptance is deliberately scoped to this random live-shell identity
# and removed at exit.
: ${AISHE_ACCEPTANCE_FILE:=${TMPDIR:-/tmp}/aishe-yolo-accept-${AISHE_SHELL_ID}}
export AISHE_ACCEPTANCE_FILE

# Route one natural-language line according to AISHE_MODE. Suggest stages one
# command for review. Auto and yolo run their managed agent loops inline.
_aishe_handle_nl() {
  local line="$1"
  [[ -z "$line" ]] && return
  case "${AISHE_MODE:-suggest}" in
    yolo)
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --yolo-line "$line" <&$_AISHE_INPUT_FD
      ;;
    auto)
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line "$line" <&$_AISHE_INPUT_FD
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line")"
      [[ -n "$cmd" ]] && printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
      ;;
  esac
}

_aishe_show_auth() {
  if [[ -n "${AISHE_CONNECTION:-}" ]]; then
    command aishe auth status --connection "$AISHE_CONNECTION" <&$_AISHE_INPUT_FD
  else
    command aishe auth status <&$_AISHE_INPUT_FD
  fi
}

# Rendered from command_surface::COMMANDS. Do not add slash names by hand here.
# __AISHE_GENERATED_SLASH_DISPATCH__

# Unknown command: zsh forks a SUBSHELL for this, so it stages via the temp file.
command_not_found_handler() {
  local line="${(j: :)@}"
  # Prompt themes, plugins, functions, substitutions, and startup files also
  # trigger this global hook. Only the exact line most recently accepted at
  # the interactive top level is eligible; nested misses keep native status 127.
  [[ -n "${_AISHE_ACCEPTED_LINE:-}" && "$line" == "$_AISHE_ACCEPTED_LINE" ]] || return 127
  _aishe_dispatch_slash "$line" && return 0
  local head="${line%%[[:space:]]*}"
  if [[ "$head" == */* ]]; then
    # Preserve absolute/relative path and custom/MCP slash precedence through
    # the canonical one-shot dispatcher. A missing path must not become model
    # input merely because zsh called command_not_found_handler for it.
    command aishe -c "$line" <&$_AISHE_INPUT_FD
    return $?
  fi
  _aishe_handle_nl "$line"
  return 0
}

# Stage a line for the AI; the next aishe_precmd (MAIN shell) routes it.
_aishe_force_nl() { printf '%s' "$1" > "$AISHE_FORCE_FILE"; }

# Put a proposal into zsh's native editing buffer. The marker distinguishes a
# later Ctrl-C/empty accept (review canceled) from an executed command; preexec
# clears it before any accepted/edited proposal runs.
_aishe_stage_command() {
  typeset -g _AISHE_STAGED_SUGGESTION=1
  print -z -- "$1"
}

# Runs in the MAIN shell before each prompt: route a forced-NL line (from the
# sigil or key), then act on a staged command.
aishe_precmd() {
  # Apply the connection/model handoff in the main shell. This is independent
  # of AIShe's optional branded prompt so `/model` still changes the runtime
  # identity when the operator keeps their own prompt theme.
  if [[ -n "${AISHE_SELECTION_FILE:-}" && -r "${AISHE_SELECTION_FILE}" ]]; then
    {
      IFS= read -r AISHE_CONNECTION
      IFS= read -r AISHE_CONNECTION_LABEL
      IFS= read -r AISHE_PROVIDER
      IFS= read -r AISHE_ENDPOINT_HOST
      IFS= read -r AISHE_AUTH_LABEL
      IFS= read -r AISHE_MODEL
      IFS= read -r AISHE_REASONING
      IFS= read -r AISHE_SELECTION_SCOPE
    } < "${AISHE_SELECTION_FILE}"
    export AISHE_CONNECTION AISHE_CONNECTION_LABEL AISHE_PROVIDER AISHE_ENDPOINT_HOST
    export AISHE_AUTH_LABEL AISHE_MODEL AISHE_REASONING AISHE_SELECTION_SCOPE
  fi
  # A durable scope change writes a one-shot handoff under the PTY wrapper so
  # child AIShe commands and /status observe the new value immediately.
  if [[ -n "${AISHE_SCOPE_FILE:-}" && -r "${AISHE_SCOPE_FILE}" ]]; then
    IFS= read -r AISHE_SCOPE < "${AISHE_SCOPE_FILE}"
    export AISHE_SCOPE
  fi
  # A persistent `aishe output ...` runs in a child process under the PTY.
  # Consume its one-shot handoff without overriding a later Ctrl-O session
  # toggle on every prompt.
  local _aishe_output_root="${TMPDIR:-/tmp}"
  local _aishe_expected_output_file="${_aishe_output_root%/}/aishe-output-${AISHE_SHELL_ID}"
  if [[ -n "${AISHE_OUTPUT_FILE:-}" &&
        "$AISHE_OUTPUT_FILE" == "$_aishe_expected_output_file" &&
        -r "$AISHE_OUTPUT_FILE" ]]; then
    IFS= read -r AISHE_AGENT_OUTPUT < "$AISHE_OUTPUT_FILE"
    command rm -f "$AISHE_OUTPUT_FILE"
    export AISHE_AGENT_OUTPUT
  fi
  # One concise, configurable hint after an ordinary failure. A signature keeps
  # prompt redraws from repeating it; Ctrl-C (130) is intentionally quiet.
  local _aishe_hint_sig="${AISHE_LAST_EXIT:-0}:${AISHE_LAST_CMD:-}"
  if [[ "${_AISHE_STAGED_SUGGESTION:-0}" == 1 ]]; then
    # Ctrl-C while reviewing a staged proposal is a normal cancel action, not a
    # failed command which warrants the generic recovery hint.
    AISHE_LAST_EXIT=0
    typeset -g _AISHE_STAGED_SUGGESTION=""
    _aishe_hint_sig="0:${AISHE_LAST_CMD:-}"
  fi
  if [[ "${AISHE_FAILURE_HINTS:-${AISHE_AUTODIAGNOSE:-0}}" == 1 &&
        "${AISHE_LAST_EXIT:-0}" != 0 && "${AISHE_LAST_EXIT:-0}" != 130 &&
        -n "$AISHE_LAST_CMD" && "$_aishe_hint_sig" != "$_AISHE_LAST_HINT_SIGNATURE" ]]; then
    print -P "%F{244}aishe: exit ${AISHE_LAST_EXIT} — ? explain · Ctrl-X Ctrl-F suggest a fix · !<cmd> force shell%f"
    typeset -g _AISHE_LAST_HINT_SIGNATURE="$_aishe_hint_sig"
  fi
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
  case "$action" in
    mode) _aishe_apply_session_mode "$cmd"; return ;;
    details) aishe-toggle-agent-details; return ;;
  esac
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
        _aishe_stage_command "$cmd"
      fi
      ;;
    *)  _aishe_stage_command "$cmd" ;;         # native confirm/edit buffer
  esac
}

# Clean up this shell's per-shell temp files when the interactive shell exits, so
# they don't accumulate in $TMPDIR. Registered as a zshexit hook below. Also caps
# the aishe history log so it can't grow without bound across many sessions.
aishe_zshexit() {
  command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE" "$AISHE_ACCEPTANCE_FILE"
  if [[ -n "$AISHE_HISTFILE" && -f "$AISHE_HISTFILE" ]]; then
    local _n
    _n=$(command wc -l < "$AISHE_HISTFILE" 2>/dev/null) || _n=0
    if (( _n > 20000 )); then
      command tail -n 10000 "$AISHE_HISTFILE" > "$AISHE_HISTFILE.tmp" 2>/dev/null \
        && command mv -f "$AISHE_HISTFILE.tmp" "$AISHE_HISTFILE" 2>/dev/null
    fi
  fi
}

# Force-NL key: send the current line to the AI even if it starts with a real
# command. Default key Alt-Enter; override with AISHE_NL_KEY (a zsh bindkey seq).
aishe-nl-widget() {
  emulate -L zsh
  [[ -z "$BUFFER" ]] && return
  local submitted="$BUFFER"
  print -s -- "$submitted"   # keep the NL line in history (up-arrow recall)
  _aishe_force_nl "$submitted"
  # The shell must accept an empty buffer so it never executes the request.
  # Keep a non-editable copy on screen: without POSTDISPLAY, ZLE redraws the
  # newly empty BUFFER before accepting it and erases the submitted request
  # from terminal scrollback even though history and the LLM still receive it.
  BUFFER=""
  POSTDISPLAY="$submitted"
  zle .accept-line
}

# Capture the exit status and command line of the last command, so the fix-it key
# can ask the model to correct a command that just failed. _aishe_capture_exit is
# moved to the FRONT of precmd_functions (below) so it sees $? before a prompt
# theme resets it; _aishe_capture_cmd records the command via preexec.
_aishe_capture_exit() {
  AISHE_LAST_EXIT=$?
  # This runs before every prompt theme. The accepted-line proof has already
  # served any top-level command and must not authorize prompt/plugin probes.
  typeset -g _AISHE_ACCEPTED_LINE=""
  if [[ "$AISHE_LAST_EXIT" != 0 && "$AISHE_LAST_EXIT" != 130 && -n "$AISHE_LAST_CMD" ]]; then
    local elapsed=""
    if [[ -n "${_AISHE_COMMAND_STARTED:-}" && -n "${EPOCHREALTIME:-}" ]]; then
      elapsed=$(( (EPOCHREALTIME - _AISHE_COMMAND_STARTED) * 1000 ))
      elapsed=${elapsed%.*}
    fi
    AISHE_LAST_DURATION_MS="$elapsed" command aishe --record-failure "$AISHE_LAST_CMD" >/dev/null 2>&1
    typeset -g _AISHE_FAILURE_ACTIVE=1
  elif [[ "${_AISHE_FAILURE_ACTIVE:-0}" == 1 ]]; then
    command aishe last clear >/dev/null 2>&1
    typeset -g _AISHE_FAILURE_ACTIVE=""
  fi
}
_aishe_capture_cmd() {
  # Any preexec proves the user accepted the staged buffer (possibly edited),
  # so a real failure should retain the ordinary recovery hint.
  typeset -g _AISHE_STAGED_SUGGESTION=""
  AISHE_LAST_CMD="$1"
  typeset -g _AISHE_ACCEPTED_LINE="$1"
  typeset -g _AISHE_COMMAND_STARTED="${EPOCHREALTIME:-}"
  # Persist each interactive command to aishe's timestamped history log (zsh
  # EXTENDED_HISTORY format) so `aishe history` and semantic search have data;
  # the PTY's commands run in real zsh, not through aishe's executor. Newlines
  # are flattened so each entry stays on one line. Best-effort. When the wrapper
  # adopted AISHE_HISTFILE as zsh's native HISTFILE, zsh writes the entry itself;
  # do not append a second copy here.
  if [[ -n "$AISHE_HISTFILE" && -z "$AISHE_MANAGED_HISTFILE" ]]; then
    # Don't record history-management commands (they only read the log).
    case "${1%%[ 	]*}" in
      history|fc) ;;
      *) print -r -- ": ${EPOCHSECONDS:-0}:0;${1//$'\n'/ }" >> "$AISHE_HISTFILE" 2>/dev/null ;;
    esac
  fi
}

# Fix-the-last-command (default Ctrl-X Ctrl-F; override AISHE_FIX_KEY). When the
# previous command failed, ask the model for a corrected command and pre-fill it
# on the line for review — it never auto-runs. The call is synchronous but bounded
# by aishe's own hook timeout, so it can't hang the editor.
aishe-fix-command() {
  emulate -L zsh
  if [[ "${AISHE_LAST_EXIT:-0}" == 0 || -z "$AISHE_LAST_CMD" ]]; then
    zle -M "aishe: no failed command to fix"
    return
  fi
  zle -M "aishe: asking for a fix…"
  local fix
  # Prefer the durable capsule. If it was unavailable (for example a transient
  # state-write failure), retain the established in-memory hook fallback.
  if ! fix="$(command aishe last fix 2>/dev/null)"; then
    fix="$(AISHE_LAST_EXIT="$AISHE_LAST_EXIT" command aishe --fix-line "$AISHE_LAST_CMD" 2>/dev/null)"
  fi
  if [[ -n "$fix" ]]; then
    BUFFER="$fix"
    CURSOR=${#BUFFER}
  else
    zle -M "aishe: no fix available"
  fi
}

# Current-buffer copilot (default Ctrl-X Ctrl-A; override AISHE_EDIT_KEY).
# Rewrites the current command in place for review and never executes it.
aishe-edit-command() {
  emulate -L zsh
  if [[ -z "$BUFFER" ]]; then
    zle -M "aishe: type a command to improve"
    return
  fi
  local original="$BUFFER" edited
  zle -M "aishe: improving command…"
  edited="$(command aishe --edit-line "$original")"
  if [[ -n "$edited" ]]; then
    BUFFER="$edited"
    CURSOR=${#BUFFER}
  else
    BUFFER="$original"
    zle -M "aishe: command unchanged"
  fi
}

# Generated command palette (default Ctrl-X Space). Selection only fills the
# current ZLE buffer; Enter remains a separate user decision.
aishe-command-palette() {
  emulate -L zsh
  local handoff="${TMPDIR:-/tmp}/aishe-palette-${AISHE_SHELL_ID}"
  command rm -f "$handoff"
  # The picker paints a frame below the prompt. Tell ZLE its display is gone,
  # or the buffer is redrawn against a stale cursor with no prompt in sight.
  zle -I
  AISHE_PALETTE_FILE="$handoff" command aishe palette <&$_AISHE_INPUT_FD
  if [[ -r "$handoff" ]]; then
    BUFFER="$(<"$handoff")"
    CURSOR=${#BUFFER}
    command rm -f "$handoff"
  else
    BUFFER=""
    CURSOR=0
    zle -M "aishe: palette cancelled"
  fi
  # The frame overwrote the prompt line; ZLE would otherwise repaint only the
  # right-prompt region and leave the left prompt missing.
  zle reset-prompt
}

# `/` + Tab opens AIShe's palette; every other Tab delegates to the widget the
# user's theme or completion plugin installed before AIShe.
aishe-slash-tab() {
  emulate -L zsh
  if [[ "$BUFFER" == "/" ]]; then
    aishe-command-palette
  else
    zle "${_AISHE_ORIG_TAB_WIDGET:-expand-or-complete}"
  fi
}

# Semantic history recall (default Ctrl-X Ctrl-R; override AISHE_RECALL_KEY).
# Takes the current line as a natural-language query ("the docker run with the
# prometheus volume"), asks aishe for the closest past command by meaning, and
# pre-fills it on the line for review — it never auto-runs. Needs
# `semantic_history = true` and a built index (`aishe history index`); with the
# feature off or no match it just shows a message and leaves the line untouched.
# Bounded by aishe's hook timeout, so it can't hang the editor.
aishe-recall() {
  emulate -L zsh
  if [[ -z "$BUFFER" ]]; then
    zle -M "aishe: type a few words first, then press recall"
    return
  fi
  zle -M "aishe: recalling…"
  local hit
  hit="$(command aishe history search "$BUFFER" -n 1 --bare 2>/dev/null)"
  if [[ -n "$hit" ]]; then
    BUFFER="$hit"
    CURSOR=${#BUFFER}
  else
    zle -M "aishe: no recall match"
  fi
}

# Route-aware highlighting. A recognized command is green on minimal accounts;
# input AIShe will treat as natural language is magenta. The natural-language
# overlay also corrects the common zsh-syntax-highlighting ambiguity where a
# question such as `what is ...` stays green merely because macOS ships a real
# `what` binary. Full shell grammar highlighting remains the external plugin's
# job. Exact prior regions are removed on every redraw so edits never leave
# stale color behind, while regions owned by other widgets are preserved.
# zsh 5.9 added `memo=token`, which is the collision-free way for plugins to
# remove only their own regions. On 5.8, use an AIShe-specific bold color
# combination as a compatibility marker.
autoload -Uz is-at-least
typeset -g _AISHE_HIGHLIGHT_MEMO=""
is-at-least 5.9 && _AISHE_HIGHLIGHT_MEMO="memo=aishe"
# Conservative question grammar for command-name collisions. Generated from
# dispatcher metadata so Rust routing, zsh highlighting, and Enter submission
# change together. It runs entirely in-process and never calls AIShe/model code.
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

# Return success only when the local zsh contract will submit the full buffer
# to AIShe as natural language. This is deliberately shell-local: it uses zsh
# lexical splitting and whence, never a process/network/backend call.
_aishe_routes_to_agent() {
  emulate -L zsh
  setopt extendedglob
  local line="${1##[[:space:]]#}"
  line="${line%%[[:space:]]#}"
  [[ -n "$line" ]] || return 1
  # Once zsh has entered a continuation prompt (heredoc, quote, loop, function,
  # and so on), native parsing owns every following line. CONTEXT is a ZLE
  # special parameter; it is unset in non-widget conformance probes.
  [[ "${CONTEXT:-start}" == start ]] || return 1
  [[ "$line" == [#?]* ]] && return 0
  # Also defer a complete multiline buffer, including pasted shell constructs.
  [[ "$line" == *$'\n'* ]] && return 1
  [[ "$line" == '!'* ]] && return 1
  _aishe_slash_id "$line" > /dev/null 2>&1 && return 1

  # Exact shell-shape precedence shared with the Rust classifier.
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

  # For compound lines, every top-level segment head must resolve locally.
  # ${(z)} respects quotes, so `grep 'a|b'` does not create a false segment.
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

_aishe_highlight_command() {
  emulate -L zsh
  setopt extendedglob
  # `region_highlight` is a widget-scoped special array. Cleanup must happen in
  # this hook itself (a nested helper would mutate its own scoped copy and leave
  # stale spans behind as the buffer grows).
  local -a kept
  local spec
  for spec in "${region_highlight[@]}"; do
    if [[ -n "$_AISHE_HIGHLIGHT_MEMO" ]]; then
      [[ "$spec" == *" memo=aishe" ]] || kept+=("$spec")
    else
      case "$spec" in
        <->\ <->\ fg=green,bold|<->\ <->\ fg=magenta,bold) ;;
        *) kept+=("$spec") ;;
      esac
    fi
  done
  region_highlight=("${kept[@]}")
  [[ "${AISHE_COMMAND_HIGHLIGHT:-1}" != 0 && -n "$BUFFER" ]] || return 0

  if _aishe_routes_to_agent "$BUFFER"; then
    local owned_spec
    if [[ -n "$_AISHE_HIGHLIGHT_MEMO" ]]; then
      owned_spec="0 ${#BUFFER} fg=magenta memo=aishe"
    else
      owned_spec="0 ${#BUFFER} fg=magenta,bold"
    fi
    region_highlight+=("$owned_spec")
    return 0
  fi

  local leading="${BUFFER%%[^[:space:]]*}"
  local rest="${BUFFER#$leading}"
  local head="${rest%%[[:space:]]*}"
  [[ "$head" == [[:alnum:]_./+-]## ]] || return 0

  # Explicit paths and other shell-shaped buffers may have an unresolved head;
  # leave those to the native highlighter instead of falsely coloring them as
  # agent input.
  whence -w -- "$head" >/dev/null 2>&1 || return 0

  # A real syntax plugin owns valid shell grammar and command colors. AIShe only
  # overlays the natural-language route above.
  if (( $+functions[_zsh_highlight] || $+functions[_zsh_highlight_main] ||
        $+functions[_fast_highlight] || $+functions[_fast_main] )); then
    return 0
  fi

  local start=${#leading}
  local end=$(( start + ${#head} ))
  local owned_spec
  if [[ -n "$_AISHE_HIGHLIGHT_MEMO" ]]; then
    owned_spec="$start $end fg=green memo=aishe"
  else
    owned_spec="$start $end fg=green,bold"
  fi
  region_highlight+=("$owned_spec")
  return 0
}

# On-demand, non-color route cue. It is a widget instead of an always-on
# POSTDISPLAY/RPROMPT mutation so autosuggestions, syntax plugins, and the
# user's prompt remain authoritative. Default Ctrl-X ?; configurable.
aishe-show-route() {
  emulate -L zsh
  if [[ -z "$BUFFER" ]]; then
    zle -M "aishe route: empty · type a line first"
  elif _aishe_routes_to_agent "$BUFFER"; then
    zle -M "aishe route: agent · ! forces this line to shell"
  else
    zle -M "aishe route: shell/local · ? forces this line to agent"
  fi
}

# Accept-line and highlighting call the same local predicate. Agent input is
# staged before zsh parses it; native shell syntax and real commands continue
# through the previously installed accept-line widget.
aishe-accept-line() {
  emulate -L zsh
  if [[ "$BUFFER" == "/" || "$BUFFER" == "/palette" ]]; then
    aishe-command-palette
    return
  elif _aishe_slash_id "$BUFFER" > /dev/null; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    _aishe_dispatch_slash "$submitted"
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif _aishe_routes_to_agent "$BUFFER"; then
    local submitted="$BUFFER"
    print -s -- "$submitted"   # keep the NL line in history (up-arrow recall)
    if [[ "${submitted[1]}" == '#' && -z "${AISHE_HASH_DEPRECATION_SHOWN:-}" ]]; then
      AISHE_HASH_DEPRECATION_SHOWN=1
      zle -I
      print -u2 -- 'aishe: `#` agent prefix is deprecated; use `?` (removed in 0.9)'
    fi
    local body="${submitted#[#?]}"
    body="${body# }"
    if [[ -n "$body" ]]; then
      _aishe_force_nl "$body"
      BUFFER=""
      POSTDISPLAY="$submitted"
    fi
  fi
  zle "${_aishe_orig_accept_line:-.accept-line}"
}

# Detail toggle (default Ctrl-O; override with AISHE_DETAILS_KEY). Focus is the
# quiet default: it leaves only the final response in scrollback. Detailed
# preserves reasoning summaries, tool calls, tool output, diffs, and usage.
aishe-toggle-agent-details() {
  emulate -L zsh
  case "${AISHE_AGENT_OUTPUT:-focus}" in
    detailed) AISHE_AGENT_OUTPUT=focus ;;
    *)        AISHE_AGENT_OUTPUT=detailed ;;
  esac
  export AISHE_AGENT_OUTPUT
  zle -M "aishe agent details: ${AISHE_AGENT_OUTPUT}"
}

# Mode-cycle key (default Shift-Tab; override with AISHE_MODE_KEY). Rotates
# AISHE_MODE suggest -> auto -> yolo -> suggest for the rest of the session, like
# Claude Code's Shift-Tab. The safety gate and yolo_confirm tier still apply, so
# this never bypasses confirmation; it only changes how the next NL line routes.
aishe-cycle-mode() {
  emulate -L zsh
  case "${AISHE_MODE:-suggest}" in
    suggest) AISHE_MODE=auto ;;
    auto)
      # An interactive child owns the terminal until acceptance completes.
      zle -I
      if command aishe --accept-yolo <&$_AISHE_INPUT_FD; then
        AISHE_MODE=yolo
      else
        AISHE_MODE=auto
      fi
      ;;
    *)       AISHE_MODE=suggest ;;
  esac
  export AISHE_MODE
  if (( $+functions[aishe_set_prompt] )); then
    # Refresh zsh's native right prompt without touching the theme's left prompt.
    aishe_set_prompt status-only
    zle reset-prompt
  else
    zle -M "aishe mode: ${AISHE_MODE}"
  fi
}
if [[ -o interactive ]]; then
  autoload -Uz add-zsh-hook
  zmodload zsh/datetime 2>/dev/null   # $EPOCHSECONDS for history timestamps
  # ZLE gives external commands launched inside widgets a non-terminal stdin.
  # Preserve the real inner zsh PTY before entering any widget; /dev/tty is the
  # outer proxy terminal under `aishe`, where competing readers split keys.
  if [[ -z "${_AISHE_INPUT_FD:-}" ]]; then
    typeset -gi _AISHE_INPUT_FD=-1
    exec {_AISHE_INPUT_FD}<&0
  fi
  add-zsh-hook precmd aishe_precmd
  add-zsh-hook zshexit aishe_zshexit   # remove per-shell temp files on exit
  # Last-command capture for the fix-it key. The exit capture must run before any
  # prompt theme's precmd (which resets $?), so pull it to the front.
  add-zsh-hook precmd _aishe_capture_exit
  precmd_functions=(_aishe_capture_exit ${precmd_functions:#_aishe_capture_exit})
  add-zsh-hook preexec _aishe_capture_cmd
  zle -N aishe-nl-widget
  bindkey "${AISHE_NL_KEY:-^[^M}" aishe-nl-widget
  zle -N aishe-cycle-mode
  bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode
  zle -N aishe-toggle-agent-details
  bindkey "${AISHE_DETAILS_KEY:-^O}" aishe-toggle-agent-details
  zle -N aishe-fix-command
  bindkey "${AISHE_FIX_KEY:-^X^F}" aishe-fix-command
  zle -N aishe-edit-command
  bindkey "${AISHE_EDIT_KEY:-^X^A}" aishe-edit-command
  zle -N aishe-command-palette
  bindkey "${AISHE_PALETTE_KEY:-^X }" aishe-command-palette
  typeset -ga _aishe_tab_binding
  _aishe_tab_binding=("${(@z)$(bindkey '^I')}")
  if [[ "${_aishe_tab_binding[-1]:-}" != aishe-slash-tab ]]; then
    typeset -g _AISHE_ORIG_TAB_WIDGET="${_aishe_tab_binding[-1]:-expand-or-complete}"
  fi
  unset _aishe_tab_binding
  zle -N aishe-slash-tab
  bindkey '^I' aishe-slash-tab
  zle -N aishe-recall
  bindkey "${AISHE_RECALL_KEY:-^X^R}" aishe-recall
  zle -N aishe-show-route
  bindkey "${AISHE_ROUTE_KEY:-^X?}" aishe-show-route
  # Supply a small green valid-command cue on minimal accounts. A full syntax
  # highlighter, when present, takes precedence dynamically.
  autoload -Uz add-zle-hook-widget
  if [[ -z "${zle_line_pre_redraw_functions[(r)_aishe_highlight_command]}" ]]; then
    add-zle-hook-widget line-pre-redraw _aishe_highlight_command
  fi
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
