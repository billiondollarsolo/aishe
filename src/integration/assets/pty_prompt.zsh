# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
if [[ -o interactive && "${AISHE_PTY_PROMPT:-1}" == 1 ]]; then
  autoload -Uz add-zsh-hook
  typeset -g _AISHE_STATUS_TEXT=""
  autoload -Uz vcs_info
  zstyle ':vcs_info:git:*' formats '%b'
  aishe_set_prompt() {
    local glyph connection connection_label provider endpoint auth selection identity model reasoning mode backend scope status_text status_row base_prompt key value item branch environment max_width
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
    identity="${connection_label} (${connection}) · ${provider}@${endpoint} · ${auth} · ${model}/${reasoning} · ${selection}"
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
    max_width=$(( ${COLUMNS:-80} - 2 ))
    status_items=("${(@s:,:)${AISHE_STATUS_ITEMS:-identity,mode,scope,session_cost,requests}}")
    for item in "${status_items[@]}"; do
      value=""
      case "$item" in
        identity) value="$identity" ;;
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
          elif [[ "${AISHE_AUTH_KIND:-}" == oauth ]]; then
            value="plan"
          else
            value="${metrics[plan]:-}"
          fi
          ;;
        *) value="${metrics[$item]:-}" ;;
      esac
      if [[ -n "$value" ]]; then
        if (( max_width > 12 && ${#value} > max_width )); then
          if [[ "${AISHE_UNICODE:-unicode}" == ascii ]]; then
            value="${value[1,$((max_width - 3))]}..."
          else
            value="${value[1,$((max_width - 1))]}…"
          fi
        fi
        if [[ -n "$status_row" ]] && (( ${#status_row} + ${#value} + 3 > max_width )); then
          status_text+=$'\n'"$value"
          status_row="$value"
        else
          status_text="${status_text:+${status_text} · }${value}"
          status_row="${status_row:+${status_row} · }${value}"
        fi
      fi
    done
    # Keep provider/model text out of PROMPT/RPROMPT. Themes commonly enable
    # PROMPT_SUBST, which would otherwise evaluate `$()` or backticks.
    _AISHE_STATUS_TEXT="$status_text"
    base_prompt="%B%F{cyan}%~%f%b %(?.%F{green}.%F{red})${glyph}%f "
    PROMPT="${base_prompt}"
    RPROMPT=""
  }
  add-zsh-hook precmd aishe_set_prompt
  autoload -Uz add-zle-hook-widget
  _aishe_status_below() {
    emulate -L zsh
    if [[ "${AISHE_STATUS_POSITION:-below}" != off && -n "$_AISHE_STATUS_TEXT" && -z "${POSTDISPLAY:-}" ]]; then
      POSTDISPLAY=$'\n'"$_AISHE_STATUS_TEXT"
    fi
  }
  add-zle-hook-widget line-init _aishe_status_below
  add-zle-hook-widget line-pre-redraw _aishe_status_below
fi
