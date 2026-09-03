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
    assert!(s.contains("${AISHE_EDIT_KEY:-^X^A}"));
    assert!(s.contains("zle -N aishe-command-palette"));
    assert!(s.contains(r#"if [[ "$BUFFER" == "/" ]]; then"#));
    assert!(s.contains("${AISHE_PALETTE_KEY:-^X }"));
    assert!(s.contains(r#"_aishe_slash_id "$BUFFER" > /dev/null"#));
    assert!(s.contains("/help|/commands"));
    assert!(s.contains("command aishe status"));
    assert!(s.contains("command aishe settings"));
}

#[test]
fn generated_dispatch_covers_every_declared_hook_identity_and_alias() {
    crate::command_surface::validate_registry().unwrap();
    for (surface, rendered) in [
        (Surface::ZshHook, zsh_hook()),
        (Surface::BashHook, bash_script()),
    ] {
        assert!(!rendered.contains(SLASH_DISPATCH_MARKER));
        for spec in COMMANDS {
            let support = spec.support(surface);
            let aliases = spec
                .slash_aliases
                .iter()
                .map(|alias| format!("/{alias}"))
                .collect::<Vec<_>>()
                .join("|");
            let lookup = format!(
                "    {aliases}) printf '%s\\n' {} ;;",
                shell_single_quote(spec.id)
            );
            match support {
                SurfaceSupport::Supported | SurfaceSupport::Recognized(_) => {
                    assert!(
                        rendered.contains(&lookup),
                        "{surface:?} omitted {} ({aliases})",
                        spec.id
                    );
                    assert!(
                        rendered.contains(&format!("    {})", spec.id)),
                        "{surface:?} has no implementation for {}",
                        spec.id
                    );
                }
                SurfaceSupport::Unavailable(_) => {
                    assert!(
                        !rendered.contains(&lookup),
                        "{surface:?} emitted unavailable command {}",
                        spec.id
                    );
                }
            }
        }
    }
}

#[test]
fn shell_templates_contain_no_hand_maintained_slash_case_table() {
    assert_eq!(ZSH_HOOK_TEMPLATE.matches(SLASH_DISPATCH_MARKER).count(), 1);
    assert_eq!(
        BASH_SCRIPT_TEMPLATE.matches(SLASH_DISPATCH_MARKER).count(),
        1
    );
    assert!(!ZSH_HOOK_TEMPLATE.contains("/help|/commands"));
    assert!(!BASH_SCRIPT_TEMPLATE.contains("/help|/commands"));
    assert_eq!(
        ZSH_HOOK_TEMPLATE.matches(QUESTION_GRAMMAR_MARKER).count(),
        1
    );
    assert!(!zsh_hook().contains(QUESTION_GRAMMAR_MARKER));
}

#[test]
fn generated_shell_artifacts_match_the_reviewed_byte_snapshots() {
    use sha2::{Digest, Sha256};

    fn digest(value: &str) -> String {
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }

    // Update these only alongside an intentional review of the rendered shell
    // diff. They pin the pre-extraction bytes for ARCH-003.
    for (name, rendered, expected) in [
        (
            "zsh init",
            zsh_script(),
            "2ae62ce2d916c6d414b9df0e3946622b1e11e37be8b9f523d0e380e05f2e029f",
        ),
        (
            "bash init",
            bash_script(),
            "116f2d3f3e16bbd753cb0611f75bf87e4bb945d24c3750ea8d9f494e5a0962e0",
        ),
        (
            "wrapper zshenv",
            WRAPPER_ZSHENV.to_owned(),
            "d09acb3da656663fafd8c13ff8bc1cd2678983ebb866ee581ed70957372bb423",
        ),
        (
            "wrapper zshrc",
            wrapper_zshrc(),
            "ff5510ed148d21a922ead03aa375b25653b1c2b033103f83fd9c763ccefead3f",
        ),
    ] {
        assert_eq!(digest(&rendered), expected, "unexpected {name} byte drift");
    }
}

#[test]
fn generated_hooks_parse_in_their_declared_shells() {
    for (shell, rendered) in [("zsh", zsh_script()), ("bash", bash_script())] {
        if std::process::Command::new(shell)
            .arg("--version")
            .output()
            .is_err()
        {
            continue;
        }
        let output = std::process::Command::new(shell)
            .args(["-n", "-c", &rendered])
            .output()
            .expect("syntax-check generated shell hook");
        assert!(
            output.status.success(),
            "{shell} rejected generated integration: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn generated_bash_hook_has_no_new_shellcheck_findings() {
    use std::io::Write as _;
    use std::process::Stdio;

    if std::process::Command::new("shellcheck")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let mut child = std::process::Command::new("shellcheck")
        .args([
            "-s",
            "bash",
            // Existing template findings: safe default assignments,
            // intentionally literal backticks/readline text, and EXIT
            // trap chaining captured at source time.
            "-e",
            "SC2016,SC2064,SC2223",
            "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start shellcheck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(bash_script().as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait for shellcheck");
    assert!(
        output.status.success(),
        "shellcheck rejected generated bash hook:\n{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn generated_zsh_dispatch_applies_shell_mode_and_keeps_tombstones_local() {
    let program = format!(
            "{}\n_aishe_dispatch_slash '/mode auto'\nprintf 'effective=%s\\n' \"$AISHE_MODE\"\n_aishe_dispatch_slash '/ghost'",
            zsh_hook()
        );
    let output = std::process::Command::new("zsh")
        .args(["-f", "-c", &program])
        .output()
        .expect("exercise zsh generated slash dispatcher");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("mode = auto  (this shell)"));
    assert!(stdout.contains("effective=auto"));
    assert!(stderr.contains("/ghost is no longer available"));
    assert!(!stderr.contains("command not found"));
}

#[test]
fn generated_bash_dispatch_hands_shell_state_back_to_the_prompt() {
    let program = format!(
            "{}\n_aishe_dispatch_slash '/mode auto'\n__aishe_prompt\nprintf 'effective=%s\\n' \"$AISHE_MODE\"",
            bash_script()
        );
    let output = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-c", &program])
        .output()
        .expect("exercise bash generated slash dispatcher");
    assert!(
        output.status.success(),
        "bash state handoff failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mode = auto  (this shell)"));
    assert!(stdout.contains("effective=auto"));
}

#[test]
fn bash_err_fallback_reaches_registered_slashes_and_chains_prior_trap() {
    let program = format!(
        "trap 'printf \"prior-err=%s\\\\n\" \"$?\"' ERR\n{}\n\
             command_not_found_handle() {{ printf 'fallback=%s\\n' \"$*\"; }}\n\
             /help\n\
             printf 'continued=yes\\n'",
        bash_script()
    );
    let output = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-c", &program])
        .output()
        .expect("exercise bash ERR fallback");
    assert!(
        output.status.success(),
        "bash ERR fallback failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("fallback=/help"),
        "stdout={stdout:?} stderr={stderr:?}"
    );
    assert!(stdout.contains("prior-err=127"));
    assert!(stdout.contains("continued=yes"));
}

#[test]
fn bash_err_fallback_does_not_reclassify_real_commands_or_paths() {
    let program = format!(
        "{}\n\
             command_not_found_handle() {{ printf 'unexpected-route=%s\\n' \"$*\"; }}\n\
             known_command() {{ return 127; }}\n\
             known_command\n\
             /definitely/not/a/registered/slash\n\
             printf 'continued=yes\\n'",
        bash_script()
    );
    let output = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-c", &program])
        .output()
        .expect("exercise bash ERR exclusions");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("unexpected-route="));
    assert!(stdout.contains("continued=yes"));
}

#[test]
fn pty_wrapper_advertises_the_primary_command_surface_once() {
    let s = wrapper_zshrc();
    assert!(s.contains("aishe: /help · /connection · /model"));
    assert!(s.contains("AIShe"));
    assert!(s.contains("AI Shell"));
    // Half-block glasses mark (Unicode, not the old ASCII face).
    assert!(s.contains('█'));
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
    assert!(s.contains("_aishe_capture_exit()"));
    assert!(s.contains("--record-failure"));
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
    // The fix widget delegates to the durable capsule helper.
    assert!(s.contains("aishe last fix"));
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
fn zsh_hash_agent_alias_warns_once_with_bounded_migration_guidance() {
    let hook = zsh_hook();
    let guidance = "aishe: `#` agent prefix is deprecated; use `?` (removed in 0.9)";
    assert_eq!(hook.matches(guidance).count(), 1);
    assert!(hook.contains("-z \"${AISHE_HASH_DEPRECATION_SHOWN:-}\""));
    assert!(hook.contains("AISHE_HASH_DEPRECATION_SHOWN=1"));
}

#[test]
fn zsh_highlight_and_submit_share_one_local_route_predicate() {
    let s = script("zsh").unwrap();
    assert!(!s.contains("_aishe_should_route_question"));
    assert!(s.contains(r#"if _aishe_routes_to_agent "$BUFFER"; then"#));
    assert!(s.contains(r#"elif _aishe_routes_to_agent "$BUFFER"; then"#));
    assert_eq!(
        s.matches("_aishe_routes_to_agent \"$BUFFER\"").count(),
        3,
        "highlight, submit, and the on-demand text cue share one predicate"
    );

    let predicate = s
        .split("_aishe_routes_to_agent() {")
        .nth(1)
        .unwrap()
        .split("\n}\n\n_aishe_highlight_command")
        .next()
        .unwrap();
    assert!(!predicate.contains("command aishe"));
    assert!(!predicate.contains("--suggest-line"));
    assert!(predicate.contains(r#"[[ "${CONTEXT:-start}" == start ]] || return 1"#));
    assert!(predicate.contains(r#"[[ "$line" == *$'\n'* ]] && return 1"#));
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
fn zsh_route_cue_is_textual_on_demand_and_uses_the_shared_predicate() {
    let s = zsh_hook();
    assert!(s.contains("aishe-show-route"));
    assert!(s.contains("aishe route: agent · ! forces this line to shell"));
    assert!(s.contains("aishe route: shell/local · ? forces this line to agent"));
    assert!(s.contains("${AISHE_ROUTE_KEY:-^X?}"));
    assert!(!s.contains("POSTDISPLAY=\"aishe route:"));
}

#[test]
fn suggest_handoff_uses_native_zsh_staging_and_cancel_marker() {
    let s = zsh_hook();
    assert!(s.contains("_aishe_stage_command"));
    assert!(s.contains(r#"print -z -- "$1""#));
    assert!(s.contains("_AISHE_STAGED_SUGGESTION=1"));
    assert!(s.contains("typeset -g _AISHE_STAGED_SUGGESTION=\"\""));
    assert!(s.contains("AISHE_LAST_EXIT=0"));
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
    let hook = zsh_hook();
    for rule in QUESTION_PAIR_RULES {
        for second in rule.seconds {
            assert!(
                hook.contains(&format!("{}:{second}", rule.first)),
                "generated zsh grammar omitted {}:{second}",
                rule.first
            );
        }
    }
    for head in TRAILING_QUESTION_HEADS {
        assert!(
            hook.contains(head),
            "generated zsh trailing-question grammar omitted {head}"
        );
    }
    for (line, expected) in cases {
        let quoted = line.replace('\'', "'\\''");
        let program = format!(
            "{hook}\nif _aishe_looks_like_question '{quoted}'; then print yes; else print no; fi"
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
fn generated_zsh_route_matches_every_versioned_corpus_case() {
    #[derive(serde::Deserialize)]
    struct Corpus {
        normative: Vec<Case>,
        research: Vec<Case>,
    }
    #[derive(serde::Deserialize)]
    struct Case {
        id: String,
        input: String,
        expected: String,
        known_commands: Vec<String>,
        aliases_functions: Vec<String>,
    }

    if std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let fixture: Corpus =
        serde_json::from_str(include_str!("../../tests/fixtures/routing/v1.json")).unwrap();
    let hook = zsh_hook();
    for case in fixture.normative.into_iter().chain(fixture.research) {
        let mut program = hook.clone();
        for name in case.known_commands.iter().chain(&case.aliases_functions) {
            assert!(
                name.chars()
                    .all(|character| character.is_ascii_alphanumeric()
                        || matches!(character, '_' | '-' | '.')),
                "unsafe fixture command name {name:?}"
            );
            program.push_str(&format!("\nfunctions[{name}]='return 0'"));
        }
        program.push_str(&format!(
                "\nif _aishe_routes_to_agent {}; then builtin print agent; else builtin print shell; fi",
                shell_single_quote(&case.input)
            ));
        let output = std::process::Command::new("zsh")
            .args(["-fc", &program])
            .output()
            .expect("run generated zsh route predicate");
        assert!(
            output.status.success(),
            "zsh route failed for {}: {}",
            case.id,
            String::from_utf8_lossy(&output.stderr)
        );
        let expected = if case.expected == "natural_language" {
            "agent"
        } else {
            "shell"
        };
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            expected,
            "generated zsh route mismatch for {} ({:?})",
            case.id,
            case.input
        );
    }
}

#[test]
fn nested_zsh_missing_commands_never_reach_the_agent() {
    if std::process::Command::new("zsh")
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let program = format!(
        "{}\n_aishe_handle_nl() {{ return 0 }}\n\
         _AISHE_ACCEPTED_LINE=aishe-definitely-missing\n\
         aishe-definitely-missing >/dev/null 2>&1; print $?\n\
         _AISHE_ACCEPTED_LINE=previous-user-command\n\
         _aishe_prompt_probe() {{ aishe-definitely-missing >/dev/null 2>&1; print $? }}\n\
         _aishe_prompt_probe",
        zsh_hook()
    );
    let output = std::process::Command::new("zsh")
        .args(["-fc", &program])
        .output()
        .expect("run nested missing-command probe");
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "0\n127");
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
    // AIShe children must not inherit Bash monitor mode: Bash 5.3 on Linux can
    // otherwise leave the parent Readline loop outside a usable foreground
    // process group after command-not-found or bind-x returns.
    assert!(s.contains("if (set +m; _aishe_dispatch_slash"));
    assert!(s.contains("printf 'suggest\\n%s\\n' \"$line\""));
    assert!(s.contains("__aishe_capture_suggestion"));
    assert!(s.contains("suggest)\n      if __aishe_capture_suggestion"));
    assert!(!s.contains("$(set +m; command aishe --suggest-line"));
    assert!(s.matches("set +m").count() >= 5);
}

#[test]
fn bash_pass_through_arguments_are_quote_aware_without_eval() {
    let words =
        split_hook_words(r#"--role build 'two words' plain\ value "" $(inert) "C:\temp""#).unwrap();
    assert_eq!(
        words,
        [
            "--role",
            "build",
            "two words",
            "plain value",
            "",
            "$(inert)",
            r"C:\temp"
        ]
    );
    assert!(split_hook_words("'unterminated").is_err());
    let hook = bash_script();
    assert!(hook.contains("--hook-cli 'agent' \"$_aishe_arg\""));
    assert!(!hook.contains("eval \"$_aishe_arg\""));
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
    // The wrapper gets the force-NL widget too (shared rendered zsh hook).
    assert!(rc.contains("aishe-nl-widget"));
}

#[test]
fn managed_zsh_history_is_not_double_appended_by_the_hook() {
    let s = wrapper_zshrc();
    assert!(s.contains(r#"[[ -n "$AISHE_HISTFILE" && -z "$AISHE_MANAGED_HISTFILE" ]]"#));
}
