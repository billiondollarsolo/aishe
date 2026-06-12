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

#[test]
fn dash_c_propagates_exit_codes() {
    let home = temp_config_home();
    let run = |arg: &str| {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .arg("-c")
            .arg(arg)
            .assert()
    };
    // A failing command propagates its non-zero status.
    run("!false").code(1);
    // A succeeding command is 0.
    run("!true").code(0);
    // `exit N` propagates N.
    run("exit 3").code(3);
    // A pipeline's status is the last command's.
    run("!true | false").code(1);
    // `$?` reflects the previous command within one -c line.
    run("!false; echo done").code(0).stdout(contains("done"));
}

#[test]
fn piped_stdin_runs_each_line() {
    // Non-tty stdin with no `-c`: each line runs like a one-shot command.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .write_stdin("!echo piped-a\n!echo piped-b\n")
        .assert()
        .success()
        .stdout(contains("piped-a"))
        .stdout(contains("piped-b"));
}

#[test]
fn version_includes_build_metadata() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("aishe "))
        // build.rs appends "(<sha>, <date>)".
        .stdout(contains("("));
}

#[test]
fn cli_flags_are_accepted_over_config() {
    // The config selects anthropic/suggest; flags switch provider/model/mode.
    // The forced `!` command still runs, proving apply_overrides is wired in
    // without breaking the non-interactive path.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .args([
            "--provider",
            "openai",
            "--model",
            "some-model",
            "--mode",
            "yolo",
        ])
        .arg("-c")
        .arg("!echo flags-ok")
        .assert()
        .success()
        .stdout(contains("flags-ok"));
}

#[test]
fn legacy_llmsh_config_is_migrated_on_run() {
    // A pre-rename ~/.config/llmsh/config.toml (and no aishe config) is ported
    // to the new location on first run, with the [llmsh] section rewritten.
    let dir = std::env::temp_dir().join(format!("aishe-migrate-{}", std::process::id()));
    let legacy_dir = dir.join("llmsh");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("config.toml"),
        r#"[llmsh]
mode = "auto"
provider = "openai"

[providers.openai]
base_url = "http://localhost:11434"
api_key_env = "OPENAI_API_KEY"
model = "llama3"
"#,
    )
    .unwrap();

    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .arg("-c")
        .arg("!echo migrated-run")
        .assert()
        .success()
        .stdout(contains("migrated-run"))
        .stderr(contains("migrated config"));

    // The new aishe config exists with the section header rewritten.
    let ported = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(ported.contains("[aishe]"), "ported config: {ported}");
    assert!(!ported.contains("[llmsh]"), "ported config: {ported}");
    assert!(ported.contains("llama3"), "ported config: {ported}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn project_config_overlay_and_trust_flow() {
    // Isolated config + data homes so the trust store doesn't leak between runs.
    let dir = std::env::temp_dir().join(format!("aishe-trust-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[aishe]
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "user-model"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "user-openai"
"#,
    )
    .unwrap();

    // A project with a safe key (stream) and a sensitive one (provider switch).
    let proj = dir.join("repo");
    std::fs::create_dir_all(proj.join(".aishe")).unwrap();
    std::fs::write(
        proj.join(".aishe").join("config.toml"),
        "[aishe]\nstream = true\nprovider = \"openai\"\n",
    )
    .unwrap();

    let data = dir.join("data");
    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &dir)
            .env("XDG_DATA_HOME", &data)
            .current_dir(&proj)
            .args(args);
        c
    };

    // Untrusted: doctor reports the overlay, the sensitive provider switch is
    // deferred, and the effective provider stays anthropic.
    run(&["doctor"])
        .assert()
        .success()
        .stdout(contains("project config:"))
        .stdout(contains("untrusted"))
        .stdout(contains("deferred"))
        .stdout(contains("provider: anthropic"));

    // Trust it; the provider switch is reported as newly applying.
    run(&["trust"])
        .assert()
        .success()
        .stdout(contains("Trusted"))
        .stdout(contains("provider"));

    // Now trusted: the provider switch takes effect.
    run(&["doctor"])
        .assert()
        .success()
        .stdout(contains("trusted"))
        .stdout(contains("provider: openai"));

    // It shows up in the trust list, and untrust removes it.
    run(&["trust", "--list"])
        .assert()
        .success()
        .stdout(contains("config.toml"));
    run(&["untrust"])
        .assert()
        .success()
        .stdout(contains("Dropped trust"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn completions_emits_a_script() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("_aishe"));
}

#[test]
fn settings_subcommands_show_and_persist() {
    // A dedicated config home: these subcommands write to the config file, so
    // they must not share state with the other tests' config.
    let dir = std::env::temp_dir().join(format!("aishe-cli-set-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
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
"#,
    )
    .unwrap();

    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .args(args);
        c
    };

    // Show current values.
    run(&["mode"])
        .assert()
        .success()
        .stdout(contains("mode: suggest"));
    run(&["provider"])
        .assert()
        .success()
        .stdout(contains("provider: anthropic"));

    // Set and persist.
    run(&["mode", "auto"])
        .assert()
        .success()
        .stdout(contains("saved to"));
    run(&["provider", "openai"]).assert().success();
    // `model` targets the now-active provider (openai).
    run(&["model", "gpt-z2"]).assert().success();

    // The effective config reflects the persisted changes.
    run(&["config"])
        .assert()
        .success()
        .stdout(contains("mode = \"auto\""))
        .stdout(contains("provider = \"openai\""))
        .stdout(contains("gpt-z2"));

    // Inspectors work without any custom commands/skills configured.
    run(&["commands"])
        .assert()
        .success()
        .stdout(contains("no custom commands"));
    run(&["skills"])
        .assert()
        .success()
        .stdout(contains("no skills"));

    // Clap rejects an invalid mode.
    run(&["mode", "bogus"]).assert().failure();

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_restores_a_recorded_file_change() {
    // Seed an undo journal by hand (the format the file tools write) and confirm
    // `aishe undo` restores the file's prior contents. Uses the AISHE_UNDO_JOURNAL
    // override via the child's env, so it never touches the real journal.
    let dir = std::env::temp_dir().join(format!("aishe-cli-undo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("undo.jsonl");
    let target = dir.join("f.txt");
    std::fs::write(&target, "MODIFIED BY AI").unwrap();
    let rec = format!(
        r#"{{"kind":"change","batch":"b1","ts":1,"path":"{}","existed":true,"before":"ORIGINAL","tool":"write_file","summary":"write f.txt"}}"#,
        target.display()
    );
    std::fs::write(&journal, format!("{rec}\n")).unwrap();

    // `aishe undo --list` shows the recorded batch.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .args(["undo", "--list"])
        .assert()
        .success()
        .stdout(contains("b1"));

    // `aishe undo` restores the prior contents.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .arg("undo")
        .assert()
        .success()
        .stdout(contains("restored"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ORIGINAL");

    // A second undo has nothing left (the batch is now marked reverted).
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .arg("undo")
        .assert()
        .success()
        .stdout(contains("nothing to undo"));

    std::fs::remove_dir_all(&dir).ok();
}
