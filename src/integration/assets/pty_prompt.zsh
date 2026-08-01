# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
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
      [[ -n "$value" ]] && status_text="${status_text:+${status_text} · }${value}"
    done
    # Never interpolate provider/model text directly into PROMPT/RPROMPT.
    # Themes commonly enable PROMPT_SUBST, which would otherwise evaluate a
    # model name containing `$()` or backticks. zsh's `%v` prompt escape reads
    # psvar without recursively expanding its contents. Slot 99 is reserved for
    # AIShe's rendered status text.
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
