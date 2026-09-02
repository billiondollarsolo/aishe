//! Registry-driven shell fragments shared by the static hook assets.

use crate::command_surface::{
    ArgumentPolicy, Lifecycle, ShellHookAction, Surface, SurfaceSupport, COMMANDS,
};
use crate::dispatcher::{QUESTION_PAIR_RULES, QUESTION_SHELL_EVIDENCE, TRAILING_QUESTION_HEADS};

#[derive(Clone, Copy)]
pub(super) enum HookShell {
    Zsh,
    Bash,
}

impl HookShell {
    const fn surface(self) -> Surface {
        match self {
            Self::Zsh => Surface::ZshHook,
            Self::Bash => Surface::BashHook,
        }
    }
}

pub(super) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Render the conservative two-word grammar shared with Rust routing. The
/// generated predicate is pure zsh: highlighting never starts AIShe or a model.
pub(super) fn render_question_grammar() -> String {
    let evidence_guards = QUESTION_SHELL_EVIDENCE
        .iter()
        .map(|character| {
            format!(
                "\"$line\" != *{}*",
                shell_single_quote(&character.to_string())
            )
        })
        .collect::<Vec<_>>()
        .join(" && ");
    let pairs = QUESTION_PAIR_RULES
        .iter()
        .flat_map(|rule| {
            rule.seconds
                .iter()
                .map(move |second| format!("{}:{second}", rule.first))
        })
        .collect::<Vec<_>>()
        .join("|");
    let trailing_heads = TRAILING_QUESTION_HEADS.join("|");
    format!(
        r#"_aishe_looks_like_question() {{
  emulate -L zsh
  setopt extendedglob
  local line="${{1##[[:space:]]#}}"
  line="${{line%%[[:space:]]#}}"
  [[ -n "$line" ]] || return 1
  [[ "$line" == [#?]* ]] && return 0
  [[ "$line" == '!'* ]] && return 1

  # Operators, redirections, expansions, assignments, and explicit paths are
  # stronger shell signals than the question-word heuristic.
  [[ {evidence_guards} ]] || return 1
  local -a words
  words=(${{(z)line}}) 2>/dev/null || return 1
  (( ${{#words}} >= 2 )) || return 1
  local first="${{words[1]:l}}"
  local second="${{words[2]:l}}"
  second="${{second%%[^[:alnum:]_]#}}"

  case "${{first}}:${{second}}" in
    {pairs}) return 0 ;;
  esac

  # A trailing question mark is sufficient only for a question-word lead. This
  # avoids stealing legitimate commands such as `find . -name foo?`.
  if [[ "$line" == *'?' ]]; then
    case "$first" in
      {trailing_heads}) return 0 ;;
    esac
  fi
  return 1
}}
"#
    )
}

fn cli_words(spec: &crate::command_surface::CommandSpec) -> Option<String> {
    let invocation = spec.cli?;
    let mut words = format!("command aishe {}", invocation.command);
    for arg in invocation.prefix_args {
        words.push(' ');
        words.push_str(arg);
    }
    Some(words)
}

fn no_argument_guard() -> &'static str {
    r#"      if [[ -n "$_aishe_arg" ]]; then
        printf 'aishe: /%s does not accept arguments\n' "${_aishe_name#/}" >&2
      else
"#
}

fn close_no_argument_guard() -> &'static str {
    "      fi\n"
}

fn render_cli_hook(spec: &crate::command_surface::CommandSpec, shell: HookShell) -> String {
    let command = cli_words(spec).expect("validated CLI-backed hook command");
    let redirect = " < /dev/tty > /dev/tty 2>&1";
    match spec.arguments {
        ArgumentPolicy::None => format!(
            "{}        {command}{redirect}\n{}",
            no_argument_guard(),
            close_no_argument_guard()
        ),
        ArgumentPolicy::OptionalValue(_) => format!(
            "      if [[ -n \"$_aishe_arg\" ]]; then\n        {command} \"$_aishe_arg\"{redirect}\n      else\n        {command}{redirect}\n      fi\n"
        ),
        ArgumentPolicy::PassThrough(_) => match shell {
            HookShell::Zsh => format!(
                "      if [[ -n \"$_aishe_arg\" ]]; then\n        local -a _aishe_args\n        _aishe_args=(\"${{(z)_aishe_arg}}\") 2>/dev/null || {{ printf 'aishe: invalid slash-command arguments\\n' >&2; return 2; }}\n        {command} \"${{_aishe_args[@]}}\"{redirect}\n      else\n        {command}{redirect}\n      fi\n"
            ),
            HookShell::Bash => format!(
                "      command aishe --hook-cli {} \"$_aishe_arg\"{redirect}\n",
                shell_single_quote(spec.id)
            ),
        },
    }
}

fn render_hook_action(spec: &crate::command_surface::CommandSpec, shell: HookShell) -> String {
    match spec.hook_action() {
        ShellHookAction::Cli => render_cli_hook(spec, shell),
        ShellHookAction::OneShot => format!(
            "{}        command aishe -c \"$_aishe_line\" < /dev/tty > /dev/tty 2>&1\n{}",
            no_argument_guard(),
            close_no_argument_guard()
        ),
        ShellHookAction::AuthStatus => format!(
            "{}        _aishe_show_auth\n{}",
            no_argument_guard(),
            close_no_argument_guard()
        ),
        ShellHookAction::ToggleDetails => {
            let action = match shell {
                HookShell::Zsh => {
                    r#"        if (( ${ZSH_SUBSHELL:-0} > 0 )); then
          printf 'details\n\n' > "$AISHE_PENDING_FILE"
        else
          aishe-toggle-agent-details
        fi
"#
                }
                HookShell::Bash => {
                    r#"        printf 'details\n\n' > "$AISHE_PENDING_FILE"
"#
                }
            };
            format!(
                "{}{action}{}",
                no_argument_guard(),
                close_no_argument_guard()
            )
        }
        ShellHookAction::SessionMode => {
            let action = match shell {
                HookShell::Zsh => {
                    r#"      if [[ -z "$_aishe_arg" ]]; then
        printf 'mode: %s (this shell)\n' "${AISHE_MODE:-suggest}"
      elif (( ${ZSH_SUBSHELL:-0} > 0 )); then
        printf 'mode\n%s\n' "$_aishe_arg" > "$AISHE_PENDING_FILE"
      else
        _aishe_apply_session_mode "$_aishe_arg"
      fi
"#
                }
                HookShell::Bash => {
                    r#"      if [[ -z "$_aishe_arg" ]]; then
        printf 'mode: %s (this shell)\n' "${AISHE_MODE:-suggest}"
      else
        printf 'mode\n%s\n' "$_aishe_arg" > "$AISHE_PENDING_FILE"
      fi
"#
                }
            };
            action.to_string()
        }
        ShellHookAction::CompatibilityDiagnostic => {
            let Lifecycle::Tombstone { guidance, .. } = spec.lifecycle else {
                unreachable!("compatibility hook action on active command")
            };
            let message = shell_single_quote(&format!(
                "aishe: /{} is no longer available; {guidance}",
                spec.slash_aliases[0]
            ));
            format!("      printf '%s\\n' {message} >&2\n")
        }
    }
}

/// Render the only slash-name lookup and every implementation case from the
/// registry. Templates contain lifecycle plumbing but no slash command names.
pub(super) fn render_slash_dispatch(shell: HookShell) -> String {
    let surface = shell.surface();
    let specs: Vec<_> = COMMANDS
        .iter()
        .filter(|spec| !matches!(spec.support(surface), SurfaceSupport::Unavailable(_)))
        .collect();
    let mut out = String::from(
        r#"_aishe_slash_id() {
  local _aishe_name="${1%%[[:space:]]*}"
  case "$_aishe_name" in
"#,
    );
    for spec in &specs {
        let aliases = spec
            .slash_aliases
            .iter()
            .map(|alias| format!("/{alias}"))
            .collect::<Vec<_>>()
            .join("|");
        out.push_str(&format!(
            "    {aliases}) printf '%s\\n' {} ;;\n",
            shell_single_quote(spec.id)
        ));
    }
    out.push_str(
        r#"    *) return 1 ;;
  esac
}

_aishe_apply_session_mode() {
  local _aishe_mode="$1"
  case "$_aishe_mode" in
    suggest|auto) ;;
    yolo)
      if ! command aishe --accept-yolo < /dev/tty > /dev/tty 2>&1; then
        return 1
      fi
      ;;
    *)
      printf 'aishe: mode must be suggest, auto, or yolo\n' >&2
      return 2
      ;;
  esac
  AISHE_MODE="$_aishe_mode"
  export AISHE_MODE
  printf 'mode = %s  (this shell)\n' "$AISHE_MODE"
}

_aishe_dispatch_slash() {
  local _aishe_line="$1"
  local _aishe_name="${_aishe_line%%[[:space:]]*}"
  local _aishe_arg="${_aishe_line#"$_aishe_name"}"
  local _aishe_id
  _aishe_arg="${_aishe_arg#"${_aishe_arg%%[![:space:]]*}"}"
  _aishe_id="$(_aishe_slash_id "$_aishe_line")" || return 1
  case "$_aishe_id" in
"#,
    );
    for spec in specs {
        out.push_str(&format!("    {})\n", spec.id));
        out.push_str(&render_hook_action(spec, shell));
        out.push_str("      ;;\n");
    }
    out.push_str(
        r#"    *) return 1 ;;
  esac
  return 0
}
"#,
    );
    out
}
