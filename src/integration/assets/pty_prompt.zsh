# --- aishe branded prompt (PTY front-end; pty_prompt config) ---
if [[ -o interactive && "${AISHE_PTY_PROMPT:-1}" == 1 ]]; then
  autoload -Uz add-zsh-hook
  typeset -g _AISHE_STATUS_TEXT=""
  typeset -g _AISHE_STATUS_PROMPT=""
  typeset -g _AISHE_USER_RPROMPT="$RPROMPT"
  typeset -g _AISHE_COMPOSED_RPROMPT=""
  typeset -g _AISHE_PROMPT_HOST="native"
  typeset -gi _AISHE_STATUS_PSVAR_LAST=89
  typeset -g _AISHE_PROMPT_VALUE=""
  autoload -Uz vcs_info
  zstyle ':vcs_info:git:*' formats '%b'
  aishe_set_prompt() {
    local glyph connection connection_label provider endpoint auth selection model reasoning mode backend scope status_text status_prompt status_row base_prompt key value item field style branch environment max_width i duplicate seen prompt_open prompt_close prompt_index
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
    status_prompt=""
    status_row=""
    seen_values=()
    max_width=$(( ${COLUMNS:-80} - 20 ))
    (( max_width > 72 )) && max_width=72
    (( max_width < 20 )) && max_width=20
    prompt_index=90
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
        if [[ -z "${NO_COLOR:-}" && "${TERM:-}" != dumb ]]; then
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
          continue
        fi
        if [[ -n "$status_row" ]]; then
          status_text+=' · '
          status_prompt+=' %F{242}·%f '
          status_row+=' · '
        fi
        status_text+="$value"
        status_row+="$value"
        psvar[$prompt_index]="$value"
        prompt_open=''
        prompt_close=''
        case "$style" in
          fg=242)         prompt_open='%F{242}'; prompt_close='%f' ;;
          fg=cyan)        prompt_open='%F{cyan}'; prompt_close='%f' ;;
          fg=yellow)      prompt_open='%F{yellow}'; prompt_close='%f' ;;
          fg=215)         prompt_open='%F{215}'; prompt_close='%f' ;;
          fg=green)       prompt_open='%F{green}'; prompt_close='%f' ;;
          fg=209)         prompt_open='%F{209}'; prompt_close='%f' ;;
          fg=204)         prompt_open='%F{204}'; prompt_close='%f' ;;
          fg=yellow,bold) prompt_open='%B%F{yellow}'; prompt_close='%f%b' ;;
          fg=cyan,bold)   prompt_open='%B%F{cyan}'; prompt_close='%f%b' ;;
          fg=red,bold)    prompt_open='%B%F{red}'; prompt_close='%f%b' ;;
        esac
        status_prompt+="${prompt_open}%${prompt_index}v${prompt_close}"
        (( prompt_index++ ))
      done
    done
    for (( i = prompt_index; i <= _AISHE_STATUS_PSVAR_LAST; i++ )); do
      psvar[$i]=''
    done
    _AISHE_STATUS_PSVAR_LAST=$(( prompt_index - 1 ))
    _AISHE_STATUS_TEXT="$status_text"
    _AISHE_STATUS_PROMPT="$status_prompt"
    base_prompt="%B%F{cyan}%~%f%b %(?.%F{green}.%F{red})${glyph}%f "
    # A key-triggered refresh may run after Starship/Powerlevel10k has replaced
    # AIShe's prompt. Update AIShe-owned glyphs, but never seize a theme's prompt.
    if [[ "$_AISHE_PROMPT_HOST" != spaceship &&
          ( "${1:-}" != status-only || "$PROMPT" == "$_AISHE_PROMPT_VALUE" ) ]]; then
      PROMPT="${base_prompt}"
      _AISHE_PROMPT_VALUE="$PROMPT"
    fi
    if [[ "$_AISHE_PROMPT_HOST" == spaceship ]]; then
      spaceship::core::refresh_section --sync aishe
    else
      if [[ "$RPROMPT" != "$_AISHE_COMPOSED_RPROMPT" ]]; then
        _AISHE_USER_RPROMPT="$RPROMPT"
      fi
      local rprompt_separator=''
      [[ -n "$_AISHE_USER_RPROMPT" ]] && rprompt_separator=' %F{242}·%f '
      if [[ "${AISHE_STATUS_POSITION:-right}" != off && -n "$status_prompt" ]]; then
        RPROMPT="${_AISHE_USER_RPROMPT}${rprompt_separator}${status_prompt}"
      else
        RPROMPT="$_AISHE_USER_RPROMPT"
      fi
      _AISHE_COMPOSED_RPROMPT="$RPROMPT"
    fi
  }
  # Themes own their render lifecycle. Join through a supported extension point
  # when one is available instead of racing the theme for PROMPT/RPROMPT.
  if (( $+functions[spaceship::rprompt] )); then
    spaceship_aishe() {
      [[ "${AISHE_STATUS_POSITION:-right}" != off ]] &&
        spaceship::section::v4 --color white --prefix '' --suffix '' "$_AISHE_STATUS_PROMPT"
    }
    SPACESHIP_RPROMPT_ORDER=("${(@)SPACESHIP_RPROMPT_ORDER:#aishe}" aishe)
    _AISHE_PROMPT_HOST="spaceship"
  fi
  add-zsh-hook precmd aishe_set_prompt
fi
