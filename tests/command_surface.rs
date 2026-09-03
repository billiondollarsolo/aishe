//! Command-surface contract tests through the built binary.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use aishe::command_surface::{by_id, by_slash_alias, Surface, SurfaceSupport, COMMANDS};
use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;

fn temp_home(label: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "aishe-command-surface-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let config = root.join("aishe");
    std::fs::create_dir_all(&config).unwrap();
    let mut file = std::fs::File::create(config.join("config.toml")).unwrap();
    writeln!(
        file,
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[backend]
engine = "native"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "test-model"
"#
    )
    .unwrap();
    root
}

fn aishe(home: &std::path::Path) -> Command {
    let mut command = Command::cargo_bin("aishe").unwrap();
    command
        .env("XDG_CONFIG_HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", home)
        .env("AISHE_DATA_DIR", home.join("data"))
        // If a reserved slash command ever falls through to the model, this
        // distinctive response makes the regression visible.
        .env("AISHE_FAKE_LLM", "SHADOW_EXECUTED_BY_MODEL");
    command
}

#[test]
fn every_registered_alias_is_reserved_from_custom_commands() {
    let home = temp_home("reserved");
    let command_dir = home.join("aishe").join("commands");
    std::fs::create_dir_all(&command_dir).unwrap();
    for spec in COMMANDS {
        for alias in spec.slash_aliases {
            std::fs::write(
                command_dir.join(format!("{alias}.md")),
                "---\ndescription: collision fixture\nshell: true\n---\nprintf SHADOW_EXECUTED_BY_CUSTOM\\n\n",
            )
            .unwrap();
        }
    }

    for spec in COMMANDS {
        for alias in spec.slash_aliases {
            let output = aishe(&home)
                .args(["-c", &format!("/{alias}")])
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let expected = match spec.support(Surface::OneShot) {
                SurfaceSupport::Supported => 0,
                SurfaceSupport::Recognized(_) | SurfaceSupport::Unavailable(_) => 2,
            };
            assert_eq!(
                output.status.code(),
                Some(expected),
                "/{alias} (id {}) produced stdout={stdout:?}, stderr={stderr:?}",
                spec.id
            );
            assert!(
                !stdout.contains("SHADOW_EXECUTED") && !stderr.contains("SHADOW_EXECUTED"),
                "/{alias} escaped the built-in reservation: stdout={stdout:?}, stderr={stderr:?}"
            );
        }
    }
    std::fs::remove_dir_all(home).ok();
}

#[test]
fn newly_registered_primary_commands_fail_locally_with_direct_cli_guidance() {
    let home = temp_home("primary");
    let cases = [
        ("connection", "next: aishe connection pick [ID_OR_LABEL]"),
        ("auth", "next: aishe auth status"),
        ("scope", "next: aishe scope [SCOPE]"),
        ("network", "next: aishe network [allow|deny]"),
    ];
    for (alias, next) in cases {
        aishe(&home)
            .args(["-c", &format!("/{alias}")])
            .assert()
            .code(2)
            .stderr(predicates::str::contains("unavailable in one-shot mode"))
            .stderr(predicates::str::contains(next))
            .stderr(predicates::str::contains("SHADOW_EXECUTED_BY_MODEL").not());
    }
    std::fs::remove_dir_all(home).ok();
}

#[test]
fn absolute_paths_and_unregistered_custom_commands_keep_their_precedence() {
    let home = temp_home("precedence");
    let command_dir = home.join("aishe").join("commands");
    std::fs::create_dir_all(&command_dir).unwrap();
    std::fs::write(
        command_dir.join("custom-only.md"),
        "---\ndescription: unregistered command\nshell: true\n---\nprintf 'custom-precedence-ok\\n'\n",
    )
    .unwrap();

    aishe(&home)
        .args(["-c", "/custom-only"])
        .assert()
        .success()
        .stdout("custom-precedence-ok\n");

    aishe(&home)
        .args(["-c", "/usr/bin/env printf absolute-path-ok"])
        .assert()
        .success()
        .stdout("absolute-path-ok");

    std::fs::remove_dir_all(home).ok();
}

fn contains_exact_shell_token(haystack: &str, needle: &str) -> bool {
    haystack
        .split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        })
        .any(|token| token == needle)
}

#[test]
fn help_aliases_and_commands_cli_render_the_same_registry_inventory() {
    let home = temp_home("help-conformance");
    let outputs = [
        aishe(&home).args(["commands"]).output().unwrap(),
        aishe(&home).args(["-c", "/help"]).output().unwrap(),
        aishe(&home).args(["-c", "/commands"]).output().unwrap(),
    ];
    for output in &outputs {
        assert!(output.status.success());
    }
    let expected = String::from_utf8_lossy(&outputs[0].stdout);
    for output in &outputs[1..] {
        assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
    }
    for spec in COMMANDS
        .iter()
        .filter(|spec| spec.is_active() && !spec.hidden)
    {
        for alias in spec.slash_aliases {
            assert!(
                expected.contains(&format!("/{alias}")),
                "help inventory omitted /{alias} ({})",
                spec.id
            );
        }
    }

    std::fs::remove_dir_all(home).ok();
}

#[test]
fn canonical_cli_commands_are_in_root_help_and_both_completion_scripts() {
    let root_help = Command::cargo_bin("aishe")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    assert!(root_help.status.success());
    let root_help = String::from_utf8_lossy(&root_help.stdout);

    let mut completions = Vec::new();
    for shell in ["zsh", "bash"] {
        let output = Command::cargo_bin("aishe")
            .unwrap()
            .args(["completions", shell])
            .output()
            .unwrap();
        assert!(output.status.success());
        completions.push((shell, String::from_utf8_lossy(&output.stdout).into_owned()));
    }

    for spec in COMMANDS.iter().filter(|spec| spec.is_active()) {
        let Some(cli) = spec.cli else { continue };
        assert!(
            contains_exact_shell_token(&root_help, cli.command),
            "root CLI help omitted canonical command {} for {}",
            cli.command,
            spec.id
        );
        for (shell, completion) in &completions {
            assert!(
                contains_exact_shell_token(completion, cli.command),
                "{shell} completions omitted canonical command {} for {}",
                cli.command,
                spec.id
            );
        }
    }
}

#[test]
fn top_level_only_hints_surface_is_live_without_reserving_a_slash_name() {
    let home = temp_home("hints");
    aishe(&home)
        .args(["hints", "status", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::contains(r#""schema_version": 1"#))
        .stdout(predicates::str::contains(r#""launch_hint_seen""#));
    aishe(&home)
        .args(["hints", "reset"])
        .assert()
        .success()
        .stdout(predicates::str::contains("discovery hint seen-state reset"));

    let hints = by_id("hints").unwrap();
    assert!(hints.slash_aliases.is_empty());
    assert!(by_slash_alias("hints").is_none());
    std::fs::remove_dir_all(home).ok();
}
