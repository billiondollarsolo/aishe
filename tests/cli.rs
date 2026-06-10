//! End-to-end CLI tests via the built binary.

use std::io::Write;

use assert_cmd::Command;
use predicates::str::contains;

/// Write a minimal valid config into a temp XDG_CONFIG_HOME so the binary does
/// not invoke the interactive first-run wizard.
fn temp_config_home() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aishe-cli-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut f = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    writeln!(
        f,
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-x"
"#
    )
    .unwrap();
    dir
}

#[test]
fn dash_c_runs_forced_shell_command() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .arg("-c")
        .arg("!echo hi-from-aishe")
        .assert()
        .success()
        .stdout(contains("hi-from-aishe"));
}

#[test]
fn version_flag_works() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("aishe"));
}

#[test]
fn init_zsh_emits_integration() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["init", "zsh"])
        .assert()
        .success()
        .stdout(contains("command_not_found_handler"))
        .stdout(contains("print -z"));
}

#[test]
fn doctor_reports_environment() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains("backing shell"))
        .stdout(contains("front-end"))
        .stdout(contains("provider: anthropic"))
        .stdout(contains("$ANTHROPIC_API_KEY is set"));
}

#[test]
fn init_unsupported_shell_fails() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["init", "fish"])
        .assert()
        .failure();
}
