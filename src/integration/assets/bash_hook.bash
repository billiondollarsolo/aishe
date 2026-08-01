# aishe bash integration — add to ~/.bashrc:  eval "$(aishe init bash)"
# Routes unknown input to aishe. Set AISHE_MODE=suggest|auto|yolo (default
# suggest). bash runs command_not_found_handle in a SUBSHELL, so it can't touch
# shell state directly — it writes a temp file that a PROMPT_COMMAND hook acts
# on in the main shell.
: ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}
: ${AISHE_FORCE_FILE:=${TMPDIR:-/tmp}/aishe-force-$$}
# Conversation memory shared across the per-call NL invocations in this shell.
: ${AISHE_SESSION_FILE:=${TMPDIR:-/tmp}/aishe-session-mem-$$}
export AISHE_SESSION_FILE
if [[ -z "${AISHE_SHELL_ID:-}" ]]; then
  AISHE_SHELL_ID="$(command od -An -N24 -tx1 /dev/urandom 2>/dev/null | command tr -d ' \n')"
  [[ -n "$AISHE_SHELL_ID" ]] || AISHE_SHELL_ID="shell-${$}-${RANDOM}${RANDOM}"
fi
export AISHE_SHELL_ID
: ${AISHE_ACCEPTANCE_FILE:=${TMPDIR:-/tmp}/aishe-yolo-accept-${AISHE_SHELL_ID}}
export AISHE_ACCEPTANCE_FILE
__aishe_show_auth() {
  if [[ -n "${AISHE_CONNECTION:-}" ]]; then
    command aishe auth status --connection "$AISHE_CONNECTION" < /dev/tty > /dev/tty 2>&1
  else
    command aishe auth status < /dev/tty > /dev/tty 2>&1
  fi
}

# Capture a suggestion without running AIShe inside `$(...)`. Bash 5.3/Linux
# can leave Readline outside a usable foreground process group when a monitored
# command substitution launches AIShe and its shell-context probes. This helper
# disables monitor mode only around the synchronous child, restores it before
# returning, and reads the private per-shell file with Bash builtins.
__aishe_capture_suggestion() {
  local request="$1" stderr_mode="${2:-visible}" had_monitor=0 status line
  AISHE_CAPTURED_SUGGESTION=""
  case "$-" in *m*) had_monitor=1; set +m ;; esac
  if [[ "$stderr_mode" == quiet ]]; then
    command aishe --suggest-line "$request" > "$AISHE_FORCE_FILE" 2>/dev/null
  else
    command aishe --suggest-line "$request" > "$AISHE_FORCE_FILE" 2>/dev/tty
  fi
  status=$?
  [[ "$had_monitor" -eq 1 ]] && set -m
  if [[ "$status" -eq 0 && -r "$AISHE_FORCE_FILE" ]]; then
    while IFS= read -r line || [[ -n "$line" ]]; do
      [[ -n "$AISHE_CAPTURED_SUGGESTION" ]] && AISHE_CAPTURED_SUGGESTION+=$'\n'
      AISHE_CAPTURED_SUGGESTION+="$line"
    done < "$AISHE_FORCE_FILE"
  fi
  command rm -f "$AISHE_FORCE_FILE"
  return "$status"
}

# Rendered from command_surface::COMMANDS. Do not add slash names by hand here.
# __AISHE_GENERATED_SLASH_DISPATCH__

command_not_found_handle() {
  local line="$*"
  # Bash 5.3 on Linux can leave the parent Readline loop in a broken foreground
  # state when a monitored command-not-found/bind-x child launches AIShe. Run
  # every child path with monitor mode disabled inside its existing subshell;
  # the interactive parent shell's job-control setting is unchanged.
  if (set +m; _aishe_dispatch_slash "$line"); then
    return 0
  fi
  case "$line" in
    details)
      printf 'details\n\n' > "$AISHE_PENDING_FILE"
      return 0
      ;;
  esac
  case "${AISHE_MODE:-suggest}" in
    yolo)
      (set +m; AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1)
      return 0
      ;;
    auto)
      (set +m; AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line "$line" < /dev/tty > /dev/tty 2>&1)
      return 0
      ;;
    *)
      # Keep provider/backend work out of Bash's command-not-found subshell.
      # The parent PROMPT_COMMAND consumes this request before Readline starts,
      # avoiding foreground-process-group races on Bash 5.3/Linux.
      printf 'suggest\n%s\n' "$line" > "$AISHE_PENDING_FILE"
      return 0
      ;;
  esac
}

# Bash 3.2 does not consistently invoke command_not_found_handle, and newer
# Bash parses `/help`-shaped input as an absolute path. ERR is the portable
# fallback for those two cases. Gate on 127 and re-check the first word so an
# existing executable which deliberately returns 127, or a missing path, is
# never reclassified as natural language. Normal Bash 4/5 command-not-found
# dispatch returns success above and therefore never reaches this trap.
__aishe_err_fallback() {
  local status="$1" line="$2" first handled
  handled=0
  if [[ "$status" -eq 127 && "${__AISHE_ERR_ACTIVE:-0}" != 1 ]]; then
    first="${line%%[[:space:]]*}"
    if [[ "$first" == /* ]]; then
      # Only registered slash commands qualify; ordinary absolute paths retain
      # Bash's native not-found behavior.
      if _aishe_slash_id "$line" > /dev/null 2>&1; then
        handled=1
      fi
    elif [[ -n "$first" && "$first" != */* ]] && ! command -v "$first" > /dev/null 2>&1; then
      handled=1
    fi
    if [[ "$handled" -eq 1 ]]; then
      __AISHE_ERR_ACTIVE=1
      command_not_found_handle "$line"
      unset __AISHE_ERR_ACTIVE
      AISHE_ERR_HANDLED=1
    fi
  fi
  # Keep existing ERR instrumentation observable. Its status does not decide
  # whether AIShe routes the original command. The conditional reconstructs
  # the original `$?` without recursively firing ERR, so the prior body sees
  # the same status it saw before AIShe was installed.
  if [[ -n "${__AISHE_PREV_ERR_TRAP:-}" ]]; then
    if __aishe_return_status "$status"; then
      eval "$__AISHE_PREV_ERR_TRAP"
    else
      eval "$__AISHE_PREV_ERR_TRAP"
    fi
  fi
  return "$status"
}

__aishe_return_status() { return "$1"; }

# Chain the pre-existing ERR trap and keep sourcing idempotent. `trap -p`
# returns `trap -- '<body>' ERR`; preserve only the body for evaluation above.
__aishe_existing_err_trap="$(trap -p ERR)"
case "$__aishe_existing_err_trap" in
  *__aishe_err_fallback*) ;;
  '') __AISHE_PREV_ERR_TRAP=""
      trap '__aishe_err_fallback "$?" "$BASH_COMMAND"' ERR ;;
  *)  __AISHE_PREV_ERR_TRAP="${__aishe_existing_err_trap#trap -- \'}"
      __AISHE_PREV_ERR_TRAP="${__AISHE_PREV_ERR_TRAP%\' ERR}"
      trap '__aishe_err_fallback "$?" "$BASH_COMMAND"' ERR ;;
esac
unset __aishe_existing_err_trap

# Main-shell hook: run a safe auto command (state persists), or offer a
# suggestion. (readline can't be reliably pre-filled from PROMPT_COMMAND, so a
# suggestion is printed and stashed; recall it with Ctrl-X Ctrl-R.)
__aishe_prompt() {
  # Capture before reading any handoff file so the user's last exit status is
  # not replaced by the selection refresh.
  AISHE_LAST_EXIT=$?
  if [[ "${AISHE_ERR_HANDLED:-0}" == 1 ]]; then
    AISHE_LAST_EXIT=0
    unset AISHE_ERR_HANDLED
  fi
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
  if [[ -n "${AISHE_SCOPE_FILE:-}" && -r "${AISHE_SCOPE_FILE}" ]]; then
    IFS= read -r AISHE_SCOPE < "${AISHE_SCOPE_FILE}"
    export AISHE_SCOPE
  fi
  if [[ -n "${AISHE_OUTPUT_FILE:-}" && -r "${AISHE_OUTPUT_FILE}" ]]; then
    IFS= read -r AISHE_AGENT_OUTPUT < "${AISHE_OUTPUT_FILE}"
    command rm -f "$AISHE_OUTPUT_FILE"
    export AISHE_AGENT_OUTPUT
  fi
  # Capture the last command's exit status and text first (this hook is prepended
  # to PROMPT_COMMAND, so it runs before anything resets $?), for the fix-it key.
  AISHE_LAST_CMD="$(HISTTIMEFORMAT='' builtin history 1 2>/dev/null | sed 's/^ *[0-9][0-9]* *//')"
  # One concise hint after an ordinary failure (never after Ctrl-C).
  local __aishe_hint_sig="${AISHE_LAST_EXIT:-0}:${AISHE_LAST_CMD:-}"
  if [ "${AISHE_FAILURE_HINTS:-${AISHE_AUTODIAGNOSE:-0}}" = 1 ] \
     && [ "${AISHE_LAST_EXIT:-0}" -ne 0 ] \
     && [ "${AISHE_LAST_EXIT:-0}" -ne 130 ] \
     && [ -n "$AISHE_LAST_CMD" ] \
     && [ "$__aishe_hint_sig" != "${_AISHE_LAST_HINT_SIGNATURE:-}" ]; then
    printf '\033[2maishe: exit %s — ? explain · Ctrl-X Ctrl-F suggest a fix · !<cmd> force shell\033[0m\n' "$AISHE_LAST_EXIT"
    _AISHE_LAST_HINT_SIGNATURE="$__aishe_hint_sig"
  fi
  [ -f "$AISHE_PENDING_FILE" ] || return
  local action cmd
  action="$(head -n 1 "$AISHE_PENDING_FILE")"
  cmd="$(tail -n +2 "$AISHE_PENDING_FILE")"
  command rm -f "$AISHE_PENDING_FILE"
  case "$action" in
    mode) _aishe_apply_session_mode "$cmd"; return ;;
    details) __aishe_toggle_details; return ;;
    suggest)
      if __aishe_capture_suggestion "$cmd"; then
        cmd="$AISHE_CAPTURED_SUGGESTION"
      else
        cmd=""
      fi
      action=fill
      ;;
  esac
  [ -z "$cmd" ] && return
  if [ "$action" = run ]; then
    # Only eval a syntactically valid command (a question answered as prose, or a
    # malformed command, would otherwise print a parse error and pollute history).
    if command bash -nc "$cmd" 2>/dev/null; then
      history -s "$cmd"; eval "$cmd"
    else
      printf 'aishe suggests: %s  (Ctrl-X Ctrl-R to recall)\n' "$cmd"
      export AISHE_PENDING="$cmd"
    fi
  else
    printf 'aishe suggests: %s  (Ctrl-X Ctrl-R to recall)\n' "$cmd"
    export AISHE_PENDING="$cmd"
  fi
}

# Recall must be a bind-x function: readline macros do not expand shell
# variables and would insert the literal text `$AISHE_PENDING`.
__aishe_recall_pending() {
  [[ -n "${AISHE_PENDING:-}" ]] || return
  READLINE_LINE="$AISHE_PENDING"
  READLINE_POINT=${#READLINE_LINE}
}
bind -x '"\C-x\C-r": __aishe_recall_pending' 2>/dev/null
case ":${PROMPT_COMMAND}:" in
  *__aishe_prompt*) ;;
  *) PROMPT_COMMAND="__aishe_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}" ;;
esac

# Remove this shell's per-shell temp files when it exits, so they don't pile up in
# $TMPDIR. Chain onto any existing EXIT trap (don't clobber it), and only install
# once so sourcing twice is safe.
__aishe_cleanup() {
  command rm -f "$AISHE_PENDING_FILE" "$AISHE_FORCE_FILE" "$AISHE_SESSION_FILE" "$AISHE_ACCEPTANCE_FILE"
}
__aishe_existing_exit_trap="$(trap -p EXIT)"
case "$__aishe_existing_exit_trap" in
  *__aishe_cleanup*) ;;
  '') trap '__aishe_cleanup' EXIT ;;
  *)  # Chain onto the existing EXIT trap. `trap -p` prints it wrapped as
      # `trap -- '<cmds>' EXIT`; strip both the leading wrapper and the trailing
      # `' EXIT` so we re-arm just `<cmds>` after our cleanup (a malformed trap
      # results if the suffix is left on).
      __aishe_prev="${__aishe_existing_exit_trap#trap -- \'}"
      __aishe_prev="${__aishe_prev%\' EXIT}"
      trap "__aishe_cleanup; ${__aishe_prev}" EXIT
      unset __aishe_prev ;;
esac
unset __aishe_existing_exit_trap

# Force-NL: Ctrl-G turns the current line into an aishe suggestion even if it is
# a valid command. `bind -x` runs in the current shell, so READLINE_LINE sticks.
__aishe_nl() {
  local line="$READLINE_LINE"
  [ -z "$line" ] && return
  local cmd
  if __aishe_capture_suggestion "$line"; then
    cmd="$AISHE_CAPTURED_SUGGESTION"
  else
    cmd=""
  fi
  if [ -n "$cmd" ]; then
    READLINE_LINE="$cmd"
    READLINE_POINT=${#cmd}
  fi
}
bind -x '"\C-g": __aishe_nl' 2>/dev/null

# Fix-the-last-command (Ctrl-X Ctrl-F): when the previous command failed, ask the
# model for a corrected command and pre-fill it on the line for review (never
# auto-run). `bind -x` runs in the current shell so READLINE_LINE sticks.
__aishe_fix() {
  if [ "${AISHE_LAST_EXIT:-0}" -eq 0 ] || [ -z "$AISHE_LAST_CMD" ]; then
    return
  fi
  local fix fix_request
  fix_request="The previous shell command failed with exit status ${AISHE_LAST_EXIT}. Command: ${AISHE_LAST_CMD}. Reply with a corrected shell command."
  if __aishe_capture_suggestion "$fix_request" quiet; then
    fix="$AISHE_CAPTURED_SUGGESTION"
  else
    fix=""
  fi
  if [ -n "$fix" ]; then
    READLINE_LINE="$fix"
    READLINE_POINT=${#fix}
  fi
}
bind -x '"\C-x\C-f": __aishe_fix' 2>/dev/null

# Mode-cycle: Shift-Tab rotates AISHE_MODE suggest -> auto -> yolo -> suggest for
# the session (override the key by re-binding "\e[Z"). The next prompt reflects
# it; the safety gate and yolo_confirm tier still apply.
__aishe_cycle_mode() {
  case "${AISHE_MODE:-suggest}" in
    suggest) export AISHE_MODE=auto ;;
    auto)
      if (set +m; command aishe --accept-yolo < /dev/tty > /dev/tty 2>&1); then
        export AISHE_MODE=yolo
      else
        export AISHE_MODE=auto
      fi
      ;;
    *)       export AISHE_MODE=suggest ;;
  esac
  printf '\naishe mode: %s\n' "$AISHE_MODE"
}
bind -x '"\e[Z": __aishe_cycle_mode' 2>/dev/null

# Focus/detailed transcript toggle. Ctrl-O mirrors the native AIShe zsh hook;
# AISHE_AGENT_OUTPUT is inherited by each per-prompt AIShe process.
__aishe_toggle_details() {
  case "${AISHE_AGENT_OUTPUT:-focus}" in
    detailed) export AISHE_AGENT_OUTPUT=focus ;;
    *)        export AISHE_AGENT_OUTPUT=detailed ;;
  esac
  printf '\naishe agent details: %s\n' "$AISHE_AGENT_OUTPUT"
}
bind -x '"\C-o": __aishe_toggle_details' 2>/dev/null
