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
//!
//! - **semantic-recall keybinding.** A ZLE widget (default `Ctrl-X Ctrl-R`,
//!   override with `AISHE_RECALL_KEY`) takes the current line as a
//!   natural-language query and pre-fills the closest past command by meaning
//!   (opt-in `semantic_history`; never auto-runs).

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
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      ;;
    auto)
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line "$line" < /dev/tty > /dev/tty 2>&1
      ;;
    *)
      local cmd
      cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
      [[ -n "$cmd" ]] && printf 'fill\n%s\n' "$cmd" > "$AISHE_PENDING_FILE"
      ;;
  esac
}

_aishe_show_auth() {
  if [[ -n "${AISHE_CONNECTION:-}" ]]; then
    command aishe auth status --connection "$AISHE_CONNECTION" < /dev/tty > /dev/tty 2>&1
  else
    command aishe auth status < /dev/tty > /dev/tty 2>&1
  fi
}

# Unknown command: zsh forks a SUBSHELL for this, so it stages via the temp file.
command_not_found_handler() {
  local line="${(j: :)@}"
  case "$line" in
    /help|/commands)
      command aishe commands < /dev/tty > /dev/tty 2>&1
      ;;
    /status)
      command aishe status < /dev/tty > /dev/tty 2>&1
      ;;
    /reasoning)
      command aishe reasoning < /dev/tty > /dev/tty 2>&1
      ;;
    /reasoning\ *)
      command aishe reasoning "${line#/reasoning }" < /dev/tty > /dev/tty 2>&1
      ;;
    /model)
      command aishe model < /dev/tty > /dev/tty 2>&1
      ;;
    /model\ *)
      command aishe model "${line#/model }" < /dev/tty > /dev/tty 2>&1
      ;;
    /provider)
      command aishe model < /dev/tty > /dev/tty 2>&1
      ;;
    /auth)
      _aishe_show_auth
      ;;
    /log)
      command aishe log -n 20 < /dev/tty > /dev/tty 2>&1
      ;;
    /usage)
      command aishe -c /usage < /dev/tty > /dev/tty 2>&1
      ;;
    /settings)
      command aishe settings < /dev/tty > /dev/tty 2>&1
      ;;
    *)
      _aishe_handle_nl "$line"
      ;;
  esac
  return 0
}

# Stage a line for the AI; the next aishe_precmd (MAIN shell) routes it.
_aishe_force_nl() { printf '%s' "$1" > "$AISHE_FORCE_FILE"; }

# Runs in the MAIN shell before each prompt: route a forced-NL line (from the
# sigil or key), then act on a staged command.
aishe_precmd() {
  # Apply the connection/model handoff in the main shell. This is independent
  # of Aishe's optional branded prompt so `/model` still changes the runtime
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
_aishe_capture_exit() { AISHE_LAST_EXIT=$?; }
_aishe_capture_cmd() {
  AISHE_LAST_CMD="$1"
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
  # --fix-line builds the correction prompt (and, with fix_capture_stderr, re-runs
  # a read-only failed command to capture its real error output). Pass the exit
  # status through the environment.
  fix="$(AISHE_LAST_EXIT="$AISHE_LAST_EXIT" command aishe --fix-line "$AISHE_LAST_CMD" 2>/dev/null)"
  if [[ -n "$fix" ]]; then
    BUFFER="$fix"
    CURSOR=${#BUFFER}
  else
    zle -M "aishe: no fix available"
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
# input Aishe will treat as natural language is magenta. The natural-language
# overlay also corrects the common zsh-syntax-highlighting ambiguity where a
# question such as `what is ...` stays green merely because macOS ships a real
# `what` binary. Full shell grammar highlighting remains the external plugin's
# job. Exact prior regions are removed on every redraw so edits never leave
# stale color behind, while regions owned by other widgets are preserved.
# zsh 5.9 added `memo=token`, which is the collision-free way for plugins to
# remove only their own regions. On 5.8, use an Aishe-specific bold color
# combination as a compatibility marker.
autoload -Uz is-at-least
typeset -g _AISHE_HIGHLIGHT_MEMO=""
is-at-least 5.9 && _AISHE_HIGHLIGHT_MEMO="memo=aishe"
# Conservative question grammar for command-name collisions. It deliberately
# requires a recognizable question phrase instead of treating every valid
# command with arguments as prose. Prefix `!` always forces the shell; `?` or
# `#` always forces Aishe.
_aishe_looks_like_question() {
  emulate -L zsh
  setopt extendedglob
  local line="${1##[[:space:]]#}"
  line="${line%%[[:space:]]#}"
  [[ -n "$line" ]] || return 1
  [[ "$line" == [#?]* ]] && return 0
  [[ "$line" == '!'* ]] && return 1

  # Operators, redirections, expansions, assignments, and explicit paths are
  # stronger shell signals than the question-word heuristic.
  [[ "$line" != *[\|\;\&\<\>\$\`\(\)\{\}]* ]] || return 1
  local -a words
  words=(${(z)line}) 2>/dev/null || return 1
  (( ${#words} >= 2 )) || return 1
  local first="${words[1]:l}"
  local second="${words[2]:l}"
  second="${second%%[^[:alnum:]_]#}"

  case "${first}:${second}" in
    what:is|what:are|what:was|what:were|what:do|what:does|what:did|what:can|what:could|what:would|what:should|what:will)
      return 0 ;;
    where:is|where:are|where:was|where:were|where:do|where:does|where:did|where:can|where:could|where:would|where:should|where:will)
      return 0 ;;
    who:is|who:are|who:was|who:were|who:am|who:do|who:does|who:did|who:can|who:could|who:would|who:should|who:will)
      return 0 ;;
    when:is|when:are|when:was|when:were|when:do|when:does|when:did|when:can|when:could|when:would|when:should|when:will)
      return 0 ;;
    why:is|why:are|why:was|why:were|why:do|why:does|why:did|why:can|why:could|why:would|why:should|why:will)
      return 0 ;;
    how:is|how:are|how:was|how:were|how:do|how:does|how:did|how:can|how:could|how:would|how:should|how:will|how:many|how:much|how:long|how:far|how:old|how:often)
      return 0 ;;
    can:you|could:you|would:you|will:you|should:i|should:we|is:there|are:there|do:you|does:the|did:the)
      return 0 ;;
  esac

  # A trailing question mark is sufficient only for a question-word lead. This
  # avoids stealing legitimate commands such as `find . -name foo?`.
  if [[ "$line" == *'?' ]]; then
    case "$first" in
      what|where|who|when|why|how|which|whose|whom|can|could|would|will|should|is|are|do|does|did)
        return 0 ;;
    esac
  fi
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
  [[ "${AISHE_COMMAND_HIGHLIGHT:-1}" != 0 && -n "$BUFFER" ]] || return

  if _aishe_looks_like_question "$BUFFER"; then
    local owned_spec
    if [[ -n "$_AISHE_HIGHLIGHT_MEMO" ]]; then
      owned_spec="0 ${#BUFFER} fg=magenta memo=aishe"
    else
      owned_spec="0 ${#BUFFER} fg=magenta,bold"
    fi
    region_highlight+=("$owned_spec")
    return
  fi

  local leading="${BUFFER%%[^[:space:]]*}"
  local rest="${BUFFER#$leading}"
  local head="${rest%%[[:space:]]*}"
  [[ "$head" == [[:alnum:]_./+-]## ]] || return

  if ! whence -w -- "$head" >/dev/null 2>&1; then
    # Unknown command heads route through Aishe's command-not-found handler.
    local owned_spec
    if [[ -n "$_AISHE_HIGHLIGHT_MEMO" ]]; then
      owned_spec="${#leading} ${#BUFFER} fg=magenta memo=aishe"
    else
      owned_spec="${#leading} ${#BUFFER} fg=magenta,bold"
    fi
    region_highlight+=("$owned_spec")
    return
  fi

  # A real syntax plugin owns valid shell grammar and command colors. Aishe only
  # overlays the natural-language route above.
  if (( $+functions[_zsh_highlight] || $+functions[_zsh_highlight_main] ||
        $+functions[_fast_highlight] || $+functions[_fast_main] )); then
    return
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
}

# A plain-language question ending in `?` never reaches
# command_not_found_handler when NOMATCH is enabled: zsh rejects the unmatched
# `?` glob first. Question-grammar collisions (`what is`, `where is`, `who am`)
# also need pre-routing because their first word can be a real command. Other
# real commands and explicit paths keep zsh's native behavior.
_aishe_should_route_question() {
  emulate -L zsh
  setopt extendedglob
  local line="${1##[[:space:]]#}"
  line="${line%%[[:space:]]#}"
  _aishe_looks_like_question "$line" && return 0
  [[ "$line" == *'?' ]] || return 1
  local head="${line%%[[:space:]]*}"
  # A shell operator, quote, expansion, assignment, or path in the first token
  # makes this shell syntax, not the bare unknown-command shape we can pre-route
  # safely. In particular, this keeps `false; echo rc=$?` in zsh.
  [[ "$head" == [[:alnum:]_.+-]## ]] || return 1
  [[ "$head" != *'?'* && "$head" != *'*'* && "$head" != *'['* ]] || return 1
  ! whence -w -- "$head" >/dev/null 2>&1
}

# accept-line wrapper: a line starting with `?` or `#`, or a plain-language
# question identified above, is natural language. Route it before zsh parses it,
# then chain whatever accept-line widget was already installed so plugins keep
# working.
aishe-accept-line() {
  emulate -L zsh
  if [[ "$BUFFER" == "reset" || "$BUFFER" == "/reset" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    command aishe reset < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "details" || "$BUFFER" == "/details" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    aishe-toggle-agent-details
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/help" || "$BUFFER" == "/commands" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe commands < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/status" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe status < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/reasoning" || "$BUFFER" == "/reasoning "* ]]; then
    local submitted="$BUFFER"
    local reasoning_arg="${BUFFER#/reasoning}"
    reasoning_arg="${reasoning_arg# }"
    print -s -- "$submitted"
    zle -I
    if [[ -n "$reasoning_arg" ]]; then
      command aishe reasoning "$reasoning_arg" < /dev/tty > /dev/tty 2>&1
    else
      command aishe reasoning < /dev/tty > /dev/tty 2>&1
    fi
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/model" || "$BUFFER" == "/model "* ]]; then
    local submitted="$BUFFER"
    local model_arg="${BUFFER#/model}"
    model_arg="${model_arg# }"
    print -s -- "$submitted"
    zle -I
    if [[ -n "$model_arg" ]]; then
      command aishe model "$model_arg" < /dev/tty > /dev/tty 2>&1
    else
      command aishe model < /dev/tty > /dev/tty 2>&1
    fi
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/provider" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe model < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/auth" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    _aishe_show_auth
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/log" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe log -n 20 < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/usage" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe -c /usage < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == "/settings" ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    zle -I
    command aishe settings < /dev/tty > /dev/tty 2>&1
    BUFFER=""
    POSTDISPLAY="$submitted"
  elif [[ "$BUFFER" == [#?]* ]]; then
    local submitted="$BUFFER"
    print -s -- "$submitted"   # keep the NL line in history (up-arrow recall)
    local body="${submitted#[#?]}"
    body="${body# }"
    if [[ -n "$body" ]]; then
      _aishe_force_nl "$body"
      BUFFER=""
      POSTDISPLAY="$submitted"
    fi
  elif _aishe_should_route_question "$BUFFER"; then
    local submitted="$BUFFER"
    print -s -- "$submitted"
    _aishe_force_nl "$submitted"
    BUFFER=""
    POSTDISPLAY="$submitted"
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
      if command aishe --accept-yolo < /dev/tty > /dev/tty 2>&1; then
        AISHE_MODE=yolo
      else
        AISHE_MODE=auto
      fi
      ;;
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
  zmodload zsh/datetime 2>/dev/null   # $EPOCHSECONDS for history timestamps
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
  zle -N aishe-recall
  bindkey "${AISHE_RECALL_KEY:-^X^R}" aishe-recall
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

# Preserve the user's zsh/Oh My Zsh history configuration when it exists. On a
# minimal account zsh defaults to HISTFILE unset and SAVEHIST=0, which otherwise
# makes Up-arrow/Ctrl-R history disappear whenever the aishe session exits. In
# that case, use aishe's existing timestamped log as zsh's native history file.
# It lives in the user data directory, so replacing the aishe binary never
# removes it. SHARE_HISTORY makes concurrent sessions exchange entries.
if [[ -z "${{HISTFILE:-}}" && -n "${{AISHE_HISTFILE:-}}" ]]; then
  HISTFILE="${{AISHE_HISTFILE}}"
  HISTSIZE=20000
  SAVEHIST=10000
  setopt EXTENDED_HISTORY APPEND_HISTORY
  if [[ "${{AISHE_SHARE_HISTORY:-1}}" == 1 ]]; then
    setopt SHARE_HISTORY
  else
    unsetopt SHARE_HISTORY
  fi
  AISHE_MANAGED_HISTFILE=1
fi

# --- aishe AI hook (added last) ---
{ZSH_HOOK}
{PTY_PROMPT}
if [[ -z "${{AISHE_COMMAND_HINT_SHOWN:-}}" ]]; then
  print -r -- '{ascii_logo}'
  print -P "%F{{244}}aishe: /help commands · Shift-Tab mode · Ctrl-O details · /model switch%f"
  export AISHE_COMMAND_HINT_SHOWN=1
fi"#,
        ascii_logo = crate::promptui::ASCII_LOGO,
    )
}

/// Branded prompt for the PTY front-end only (never `init zsh`, which must leave
/// the user's prompt alone). Mirrors the reedline prompt: `<cwd> <glyph>`, where
/// the glyph reflects the mode and is green/red by the last exit code, with a
/// configurable status line. `right` uses RPROMPT; `below` renders a secondary
/// line with the next prompt; `off` hides it. Applied via a precmd hook added
/// last so it wins over a prompt that the user's config rebuilds each prompt.
const PTY_PROMPT: &str = r#"# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
if [[ -o interactive && "${AISHE_PTY_PROMPT:-1}" == 1 ]]; then
  autoload -Uz add-zsh-hook
  aishe_set_prompt() {
    local glyph connection connection_label provider endpoint auth selection identity model reasoning mode backend scope status_text status_prompt base_prompt key value item
    local -A metrics
    local -a status_items
    if [[ -n "${AISHE_MODEL_FILE:-}" && -r "${AISHE_MODEL_FILE}" ]]; then
      IFS= read -r AISHE_MODEL < "${AISHE_MODEL_FILE}"
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
    case "${AISHE_MODE:-suggest}" in
      yolo) glyph='*' ;;
      auto) glyph='»' ;;
      *)    glyph='❯' ;;
    esac
    model="${AISHE_MODEL}"
    connection="${AISHE_CONNECTION}"
    connection_label="${AISHE_CONNECTION_LABEL:-$connection}"
    provider="${AISHE_PROVIDER:-unknown}"
    endpoint="${AISHE_ENDPOINT_HOST:-unknown}"
    auth="${AISHE_AUTH_LABEL:-Auto (legacy)}"
    selection="${AISHE_SELECTION_SCOPE:-default}"
    [[ "$selection" == shell ]] && selection="this shell"
    reasoning="${AISHE_REASONING:-auto}"
    identity="${connection_label} (${connection}) · ${provider}@${endpoint} · ${auth} · ${model}/${reasoning} · ${selection}"
    mode="${AISHE_MODE:-suggest}"
    backend="${AISHE_BACKEND:-opencode}"
    if [[ -n "${AISHE_SCOPE_FILE:-}" && -r "${AISHE_SCOPE_FILE}" ]]; then
      scope="$(<"$AISHE_SCOPE_FILE")"
      [[ -n "$scope" ]] && AISHE_SCOPE="$scope"
    fi
    scope="${AISHE_SCOPE:-workspace}"
    metrics=()
    if [[ -n "${AISHE_STATUS_FILE:-}" && -r "${AISHE_STATUS_FILE}" ]]; then
      while IFS=$'\t' read -r key value; do
        [[ -n "$key" ]] && metrics[$key]="$value"
      done < "${AISHE_STATUS_FILE}"
    fi
    status_text=""
    status_items=("${(@s:,:)${AISHE_STATUS_ITEMS:-identity,mode,scope,session_cost,requests}}")
    for item in "${status_items[@]}"; do
      value=""
      case "$item" in
        identity) value="$identity" ;;
        connection) value="${connection_label} (${connection})" ;;
        provider) value="$provider" ;;
        endpoint) value="$endpoint" ;;
        auth) value="$auth" ;;
        selection) value="$selection" ;;
        model) value="$model" ;;
        reasoning) value="$reasoning" ;;
        mode) value="$mode" ;;
        backend) value="$backend" ;;
        scope) value="$scope" ;;
        *) value="${metrics[$item]:-}" ;;
      esac
      [[ -n "$value" ]] && status_text="${status_text:+${status_text} · }${value}"
    done
    # Never interpolate provider/model text directly into PROMPT/RPROMPT.
    # Themes commonly enable PROMPT_SUBST, which would otherwise evaluate a
    # model name containing `$()` or backticks. zsh's `%v` prompt escape reads
    # psvar without recursively expanding its contents. Slot 99 is reserved for
    # Aishe's rendered status text.
    psvar[99]="$status_text"
    if [[ -n "$status_text" && -z "${NO_COLOR:-}" ]]; then
      status_prompt="%F{244}%99v%f"
    else
      status_prompt="%99v"
    fi
    base_prompt="%B%F{cyan}%~%f%b %(?.%F{green}.%F{red})${glyph}%f "
    case "${AISHE_STATUS_POSITION:-right}" in
      off)
        PROMPT="${base_prompt}"
        RPROMPT=""
        ;;
      below)
        PROMPT="${status_prompt:+${status_prompt}
}${base_prompt}"
        RPROMPT=""
        ;;
      *)
        PROMPT="${base_prompt}"
        RPROMPT="${status_prompt}"
        ;;
    esac
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
command_not_found_handle() {
  local line="$*"
  case "$line" in
    /reset)
      command aishe reset < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    details|/details)
      __aishe_toggle_details
      return 0
      ;;
    /help|/commands)
      command aishe commands < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /status)
      command aishe status < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /reasoning)
      command aishe reasoning < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /reasoning\ *)
      command aishe reasoning "${line#/reasoning }" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /model)
      command aishe model < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /model\ *)
      command aishe model "${line#/model }" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /provider)
      command aishe model < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /auth)
      __aishe_show_auth
      return 0
      ;;
    /log)
      command aishe log -n 20 < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /usage)
      command aishe -c /usage < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    /settings)
      command aishe settings < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
  esac
  case "${AISHE_MODE:-suggest}" in
    yolo)
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --yolo-line "$line" < /dev/tty > /dev/tty 2>&1
      return 0
      ;;
    auto)
      AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line "$line" < /dev/tty > /dev/tty 2>&1
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
  # Capture before reading any handoff file so the user's last exit status is
  # not replaced by the selection refresh.
  AISHE_LAST_EXIT=$?
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
  cmd="$(command aishe --suggest-line "$line" 2> /dev/tty)"
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
  local fix
  fix="$(command aishe --suggest-line "The previous shell command failed with exit status ${AISHE_LAST_EXIT}. Command: ${AISHE_LAST_CMD}. Reply with a corrected shell command." 2>/dev/null)"
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
      if command aishe --accept-yolo < /dev/tty > /dev/tty 2>&1; then
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

# Focus/detailed transcript toggle. Ctrl-O mirrors the native Aishe zsh hook;
# AISHE_AGENT_OUTPUT is inherited by each per-prompt Aishe process.
__aishe_toggle_details() {
  case "${AISHE_AGENT_OUTPUT:-focus}" in
    detailed) export AISHE_AGENT_OUTPUT=focus ;;
    *)        export AISHE_AGENT_OUTPUT=detailed ;;
  esac
  printf '\naishe agent details: %s\n' "$AISHE_AGENT_OUTPUT"
}
bind -x '"\C-o": __aishe_toggle_details' 2>/dev/null
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
        assert!(s.contains("AISHE_SHELL_ID"));
        assert!(s.contains("/dev/urandom"));
        assert!(s.contains("aishe-toggle-agent-details"));
        assert!(s.contains("${AISHE_DETAILS_KEY:-^O}"));
        assert!(s.contains(r#""$BUFFER" == "reset" || "$BUFFER" == "/reset""#));
        assert!(s.contains("/help|/commands"));
        assert!(s.contains("command aishe status"));
        assert!(s.contains("command aishe settings"));
    }

    #[test]
    fn pty_wrapper_advertises_the_primary_command_surface_once() {
        let s = wrapper_zshrc();
        assert!(s.contains("aishe: /help commands · Shift-Tab mode · Ctrl-O details"));
        assert!(s.contains(".-----. .-----."));
        assert!(s.contains("AISHE"));
        assert!(s.contains("AISHE_COMMAND_HINT_SHOWN"));
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
    fn zsh_script_has_fix_command_key() {
        let s = script("zsh").unwrap();
        // Capture the last command + exit status, with the exit capture pulled to
        // the front of precmd_functions (so a prompt theme can't reset $? first).
        assert!(s.contains("_aishe_capture_exit() { AISHE_LAST_EXIT=$?; }"));
        assert!(s.contains("_aishe_capture_cmd()"));
        assert!(s.contains("AISHE_LAST_CMD=\"$1\""));
        // It also persists each command to the aishe history log when set.
        assert!(s.contains("AISHE_HISTFILE"));
        assert!(s.contains(
            "precmd_functions=(_aishe_capture_exit ${precmd_functions:#_aishe_capture_exit})"
        ));
        assert!(s.contains("add-zsh-hook preexec _aishe_capture_cmd"));
        // The fix widget asks for a corrected command and pre-fills the buffer.
        assert!(s.contains("aishe-fix-command"));
        assert!(s.contains("zle -N aishe-fix-command"));
        assert!(s.contains("${AISHE_FIX_KEY:-^X^F}"));
        // The fix widget delegates to the `--fix-line` hook helper.
        assert!(s.contains("--fix-line"));
        // Opt-in ambient hint after a failure.
        assert!(s.contains("AISHE_AUTODIAGNOSE"));
        assert!(s.contains("AISHE_FAILURE_HINTS"));
        assert!(s.contains(r#""${AISHE_LAST_EXIT:-0}" != 130"#));
        assert!(s.contains("_AISHE_LAST_HINT_SIGNATURE"));
        assert!(s.contains("Ctrl-X Ctrl-F suggest a fix"));
    }

    #[test]
    fn bash_script_has_fix_command_key() {
        let s = script("bash").unwrap();
        assert!(s.contains("AISHE_SHELL_ID"));
        assert!(s.contains("AISHE_LAST_EXIT=$?"));
        assert!(s.contains("AISHE_LAST_CMD="));
        assert!(s.contains("__aishe_fix"));
        assert!(s.contains("__aishe_toggle_details"));
        assert!(s.contains(r#"bind -x '"\C-o": __aishe_toggle_details'"#));
        assert!(s.contains("command aishe reset"));
        assert!(s.contains(r#"bind -x '"\C-x\C-f": __aishe_fix'"#));
        assert!(s.contains("AISHE_AUTODIAGNOSE"));
        assert!(s.contains("AISHE_FAILURE_HINTS"));
        assert!(s.contains(r#"[ "${AISHE_LAST_EXIT:-0}" -ne 130 ]"#));
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
        assert!(s.contains(r#"AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line"#));
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
        // When chaining onto an existing EXIT trap, both the leading `trap -- '`
        // wrapper and the trailing `' EXIT` that `trap -p` prints must be stripped,
        // or the re-armed trap is malformed.
        assert!(s.contains(r#"${__aishe_existing_exit_trap#trap -- \'}"#));
        assert!(s.contains(r#"${__aishe_prev%\' EXIT}"#));
    }

    #[test]
    fn bash_auto_fallback_uses_main_shell_handoff() {
        let s = script("bash").unwrap();
        assert!(s.contains(r#"AISHE_PENDING_FILE="$AISHE_PENDING_FILE" command aishe --auto-line"#));
        assert!(s.contains(r#"[ "$action" = run ]"#));
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
    fn zsh_script_pre_routes_unknown_questions_before_nomatch() {
        let s = script("zsh").unwrap();
        assert!(s.contains("_aishe_should_route_question"));
        assert!(s.contains(r#"[[ "$line" == *'?' ]]"#));
        assert!(s.contains(r#"[[ "$head" == [[:alnum:]_.+-]## ]]"#));
        assert!(s.contains(r#"! whence -w -- "$head""#));
        assert!(s.contains(r#"elif _aishe_should_route_question "$BUFFER"; then"#));
    }

    #[test]
    fn zsh_script_has_fallback_command_highlighting() {
        let s = script("zsh").unwrap();
        assert!(s.contains("_aishe_highlight_command"));
        assert!(s.contains("_aishe_looks_like_question"));
        assert!(s.contains("fg=magenta"));
        assert!(s.contains(r#"whence -w -- "$head""#));
        assert!(s.contains(r#"region_highlight+=("$owned_spec")"#));
        assert!(s.contains("memo=aishe"));
        assert!(s.contains("fg=green,bold"));
        assert!(s.contains("add-zle-hook-widget line-pre-redraw _aishe_highlight_command"));
        assert!(s.contains("$+functions[_zsh_highlight]"));
        assert!(s.contains("AISHE_COMMAND_HIGHLIGHT"));
    }

    #[test]
    fn zsh_question_grammar_disambiguates_command_name_collisions() {
        if std::process::Command::new("zsh")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        let cases = [
            ("what is the capital of France", true),
            ("where is the config", true),
            ("who am i", true),
            ("how many files are here", true),
            ("can you list large files", true),
            ("who", false),
            ("where ls", false),
            ("what /bin/ls", false),
            ("find . -name foo?", false),
            ("!who am i", false),
        ];
        for (line, expected) in cases {
            let quoted = line.replace('\'', "'\\''");
            let program = format!(
                "{ZSH_HOOK}\nif _aishe_looks_like_question '{quoted}'; then print yes; else print no; fi"
            );
            let output = std::process::Command::new("zsh")
                .args(["-fc", &program])
                .output()
                .expect("run zsh question grammar");
            assert!(
                output.status.success(),
                "zsh script failed for {line:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout).trim(),
                if expected { "yes" } else { "no" },
                "route for {line:?}"
            );
        }
    }

    #[test]
    fn zsh_script_has_force_nl_widget() {
        let s = script("zsh").unwrap();
        assert!(s.contains("aishe-nl-widget"));
        assert!(s.contains("zle -N aishe-nl-widget"));
        assert!(s.contains("AISHE_NL_KEY"));
        assert!(s.contains(r#"POSTDISPLAY="$submitted""#));
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
        assert!(rc.contains("AISHE_MODEL_FILE"));
        assert!(rc.contains("read -r AISHE_MODEL"));
        // A user-configured HISTFILE wins. Minimal zsh accounts get aishe's
        // persistent log as their native Up-arrow/Ctrl-R history, with sharing
        // controlled by the existing config flag.
        assert!(rc.contains(r#"if [[ -z "${HISTFILE:-}" && -n "${AISHE_HISTFILE:-}" ]]"#));
        assert!(rc.contains(r#"HISTFILE="${AISHE_HISTFILE}""#));
        assert!(rc.contains("HISTSIZE=20000"));
        assert!(rc.contains("SAVEHIST=10000"));
        assert!(rc.contains("setopt EXTENDED_HISTORY APPEND_HISTORY"));
        assert!(rc.contains("setopt SHARE_HISTORY"));
        assert!(rc.contains("AISHE_MANAGED_HISTFILE=1"));
        // The wrapper gets the force-NL widget too (shared ZSH_HOOK).
        assert!(rc.contains("aishe-nl-widget"));
    }

    #[test]
    fn managed_zsh_history_is_not_double_appended_by_the_hook() {
        let s = wrapper_zshrc();
        assert!(s.contains(r#"[[ -n "$AISHE_HISTFILE" && -z "$AISHE_MANAGED_HISTFILE" ]]"#));
    }
}
