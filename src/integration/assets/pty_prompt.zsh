# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
if [[ -o interactive && "${AISHE_PTY_PROMPT:-1}" == 1 ]]; then
  autoload -Uz add-zsh-hook
  typeset -g _AISHE_STATUS_TEXT=""
  typeset -g _AISHE_STATUS_POSTDISPLAY=""
  typeset -g _AISHE_PROMPT_VALUE=""
  typeset -g _AISHE_RPROMPT_VALUE=""
  typeset -ga _AISHE_STATUS_HIGHLIGHTS=()
  autoload -Uz vcs_info
  zstyle ':vcs_info:git:*' formats '%b'
  aishe_set_prompt() {
    local glyph connection connection_label provider endpoint auth selection model reasoning mode backend scope status_text status_row base_prompt key value item field style branch environment max_width i duplicate seen start end
    local -A metrics
    local -a status_items item_values item_keys seen_values
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
    if [[ "${AISHE_UNICODE:-unicode}" == ascii ]]; then
      case "${AISHE_MODE:-suggest}" in
        yolo) glyph='*' ;;
        auto) glyph='>>' ;;
        *)    glyph='>' ;;
      esac
    else
      case "${AISHE_MODE:-suggest}" in
        yolo) glyph='*' ;;
        auto) glyph='»' ;;
        *)    glyph='❯' ;;
      esac
    fi
    model="${AISHE_MODEL}"
    connection="${AISHE_CONNECTION}"
    connection_label="${AISHE_CONNECTION_LABEL:-$connection}"
    provider="${AISHE_PROVIDER:-unknown}"
    endpoint="${AISHE_ENDPOINT_HOST:-unknown}"
    auth="${AISHE_AUTH_LABEL:-Auto (legacy)}"
    selection="${AISHE_SELECTION_SCOPE:-default}"
    [[ "$selection" == shell ]] && selection="this shell"
    reasoning="${AISHE_REASONING:-auto}"
    mode="${AISHE_MODE:-suggest}"
    backend="${AISHE_BACKEND:-opencode}"
    if [[ -n "${AISHE_SCOPE_FILE:-}" && -r "${AISHE_SCOPE_FILE}" ]]; then
      scope="$(<"$AISHE_SCOPE_FILE")"
      [[ -n "$scope" ]] && AISHE_SCOPE="$scope"
    fi
    scope="${AISHE_SCOPE:-workspace}"
    vcs_info
    branch="${vcs_info_msg_0_:-}"
    environment="${AISHE_ENVIRONMENT:-}"
    if [[ -n "$branch" && -n "${AISHE_PROTECTED_PATTERNS:-}" ]]; then
      local pattern
      for pattern in "${(@s/:/)AISHE_PROTECTED_PATTERNS}"; do
        if [[ "${branch:l}" == *"${pattern:l}"* ]]; then
          [[ "$environment" == *PROD* ]] || environment="PROD${environment:+/$environment}"
          break
        fi
      done
    fi
    metrics=()
    if [[ -n "${AISHE_STATUS_FILE:-}" && -r "${AISHE_STATUS_FILE}" ]]; then
      while IFS=$'\t' read -r key value; do
        [[ -n "$key" ]] && metrics[$key]="$value"
      done < "${AISHE_STATUS_FILE}"
    fi
    status_text=""
    status_row=""
    seen_values=()
    _AISHE_STATUS_HIGHLIGHTS=()
    max_width=$(( ${COLUMNS:-80} - 2 ))
    status_items=("${(@s:,:)${AISHE_STATUS_ITEMS:-identity,mode,scope,session_cost,requests}}")
    for item in "${status_items[@]}"; do
      value=""
      case "$item" in
        identity)
          item_values=("$connection_label" "${provider}@${endpoint}" "$model" "$reasoning")
          item_keys=(connection endpoint model reasoning)
          if [[ "$selection" != default ]]; then
            item_values+=("$selection")
            item_keys+=(selection)
          fi
          ;;
        connection) value="${connection_label}" ;;
        provider) value="$provider" ;;
        endpoint) value="$endpoint" ;;
        auth) value="$auth" ;;
        selection) value="$selection" ;;
        model) value="$model" ;;
        reasoning) value="$reasoning" ;;
        mode) value="$mode" ;;
        backend) value="$backend" ;;
        scope) value="$scope" ;;
        branch) [[ -n "$branch" ]] && value="git:$branch" ;;
        environment) value="$environment" ;;
        plan)
          if [[ -n "${AISHE_PLAN_LABEL:-}" ]]; then
            value="${AISHE_PLAN_LABEL}"
          else
            value="${metrics[plan]:-}"
          fi
          ;;
        *) value="${metrics[$item]:-}" ;;
      esac
      if [[ "$item" != identity ]]; then
        item_values=("$value")
        item_keys=("$item")
      fi
      for (( i = 1; i <= ${#item_values}; i++ )); do
        value="${item_values[$i]}"
        field="${item_keys[$i]}"
        if [[ "${AISHE_AUTH_KIND:-}" == oauth && -n "${metrics[plan]:-}" &&
              "$field" == (last_tokens|last_cost|session_tokens|session_cost|requests) ]]; then
          continue
        fi
        [[ -n "$value" ]] || continue
        case "$field" in
          reasoning) value="reason:$value" ;;
          mode)
            case "$value" in
              suggest) value='REVIEW' ;;
              auto)    value='AUTO' ;;
              yolo)    value='AGENT' ;;
              *)       value="mode:$value" ;;
            esac
            ;;
        esac
        duplicate=0
        # ponytail: the field list is tiny; linear comparison keeps hostile
        # display values out of associative-array subscript evaluation.
        for seen in "${seen_values[@]}"; do
          [[ "$seen" == "$value" ]] && duplicate=1 && break
        done
        (( duplicate )) && continue
        seen_values+=("$value")
        if (( max_width > 12 && ${#value} > max_width )); then
          if [[ "${AISHE_UNICODE:-unicode}" == ascii ]]; then
            value="${value[1,$((max_width - 3))]}..."
          else
            value="${value[1,$((max_width - 1))]}…"
          fi
        fi
        style=''
        if [[ -z "${NO_COLOR:-}" && "${TERM:-}" != dumb && -n "${_AISHE_HIGHLIGHT_MEMO:-}" ]]; then
          case "$field" in
            connection|auth) style='fg=cyan' ;;
            provider|endpoint|selection|backend) style='fg=242' ;;
            model) style='fg=yellow' ;;
            reasoning) style='fg=215' ;;
            mode)
              case "$mode" in
                suggest) style='fg=yellow,bold' ;;
                auto)    style='fg=cyan,bold' ;;
                yolo)    style='fg=red,bold' ;;
              esac
              ;;
            scope|branch) style='fg=green' ;;
            environment) style='fg=red,bold' ;;
            last_tokens|last_cost|session_tokens|session_cost|requests|elapsed|context) style='fg=209' ;;
            plan|task|tasks) style='fg=204' ;;
          esac
        fi
        if [[ -n "$status_row" ]] && (( ${#status_row} + ${#value} + 3 > max_width )); then
          status_text+=$'\n'
          status_row=""
        else
          if [[ -n "$status_row" ]]; then
            start=${#status_text}
            status_text+=' · '
            end=${#status_text}
            [[ -n "$style" ]] && _AISHE_STATUS_HIGHLIGHTS+=("$start $end fg=242")
            status_row+=' · '
          fi
        fi
        start=${#status_text}
        status_text+="$value"
        end=${#status_text}
        [[ -n "$style" ]] && _AISHE_STATUS_HIGHLIGHTS+=("$start $end $style")
        status_row+="$value"
      done
    done
    # Keep provider/model text out of PROMPT/RPROMPT. Themes commonly enable
    # PROMPT_SUBST, which would otherwise evaluate `$()` or backticks.
    _AISHE_STATUS_TEXT="$status_text"
    base_prompt="%B%F{cyan}%~%f%b %(?.%F{green}.%F{red})${glyph}%f "
    # A key-triggered refresh may run after Starship/Powerlevel10k has replaced
    # AIShe's prompt. Update AIShe-owned glyphs, but never seize a theme's prompt.
    if [[ "${1:-}" != status-only ||
          ( "$PROMPT" == "$_AISHE_PROMPT_VALUE" && "$RPROMPT" == "$_AISHE_RPROMPT_VALUE" ) ]]; then
      PROMPT="${base_prompt}"
      RPROMPT=""
      _AISHE_PROMPT_VALUE="$PROMPT"
      _AISHE_RPROMPT_VALUE="$RPROMPT"
    fi
  }
  add-zsh-hook precmd aishe_set_prompt
  autoload -Uz add-zle-hook-widget
  _aishe_status_below() {
    emulate -L zsh
    region_highlight=("${region_highlight[@]:#*memo=aishe-status}")
    if [[ -n "$_AISHE_STATUS_POSTDISPLAY" && "$POSTDISPLAY" == "$_AISHE_STATUS_POSTDISPLAY" ]]; then
      POSTDISPLAY=""
    fi
    _AISHE_STATUS_POSTDISPLAY=""
    if [[ "${AISHE_STATUS_POSITION:-below}" != off && -n "$_AISHE_STATUS_TEXT" && -z "${POSTDISPLAY:-}" ]]; then
      _AISHE_STATUS_POSTDISPLAY=$'\n'"$_AISHE_STATUS_TEXT"
      POSTDISPLAY="$_AISHE_STATUS_POSTDISPLAY"
      local base=$(( ${#BUFFER} + 1 )) spec
      local -a parts
      for spec in "${_AISHE_STATUS_HIGHLIGHTS[@]}"; do
        parts=(${=spec})
        region_highlight+=("$((base + parts[1])) $((base + parts[2])) ${parts[3]} memo=aishe-status")
      done
    fi
  }
  add-zle-hook-widget line-init _aishe_status_below
  add-zle-hook-widget line-pre-redraw _aishe_status_below
fi
