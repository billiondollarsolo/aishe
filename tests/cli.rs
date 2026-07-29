//! End-to-end CLI tests via the built binary.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// Write a minimal valid config into a temp XDG_CONFIG_HOME so the binary does
/// not invoke the interactive first-run wizard.
fn temp_config_home() -> std::path::PathBuf {
    let dir = temp_root("config");
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

fn temp_root(label: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-cli-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn serve_model_catalog(requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"data":[{"id":"local-model-b"},{"id":"local-model-a"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    (format!("http://{address}"), handle)
}

#[test]
fn man_emits_a_roff_page() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("man")
        .assert()
        .success()
        .stdout(contains(".TH aishe 1").and(contains("natural")));
}

#[test]
fn suggest_subcommand_scripting_contract() {
    let home = temp_config_home();
    let base = || {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", home.join("data"))
            .env("ANTHROPIC_API_KEY", "sk-test");
        c
    };
    // Safe command in JSON: stdout is one object, exit 0.
    base()
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"ls -la","explanation":"lists"}"#,
        )
        .args(["suggest", "--json", "list", "files"])
        .assert()
        .success()
        .stdout(contains("\"command\":\"ls -la\"").and(contains("\"risk\":\"safe\"")));
    // Dangerous command → exit 20 (still printed for review).
    base()
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"rm -rf /","explanation":"boom"}"#,
        )
        .args(["suggest", "--json", "wipe", "everything"])
        .assert()
        .code(20)
        .stdout(contains("\"risk\":\"dangerous\""));
    // Empty query → exit 1 with guidance.
    base()
        .arg("suggest")
        .assert()
        .code(1)
        .stderr(contains("suggest needs a request"));
}

#[test]
fn suggest_hook_appends_to_the_session_usage_tally() {
    // Under the interactive PTY, each NL child appends its metered usage to the
    // shared tally named by AISHE_USAGE_FILE; the PTY prints a one-line summary on
    // exit. Drive one suggest-hook invocation directly (the fake records the
    // AISHE_FAKE_USAGE tokens) and assert the tally line was written.
    let home = temp_config_home();
    let tally = home.join("usage.tally");
    std::fs::remove_file(&tally).ok();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_FAKE_LLM", "ls -la")
        .env("AISHE_FAKE_USAGE", "120,30")
        .env("AISHE_USAGE_FILE", &tally)
        .args(["--suggest-line", "list files in long form"])
        .assert()
        .success();
    let contents = std::fs::read_to_string(&tally).unwrap_or_default();
    // One tab-separated tally line: "<input>\t<output>\t<model>".
    assert!(
        contents.contains("120\t30\t"),
        "expected a usage tally line, got: {contents:?}"
    );
    std::fs::remove_file(&tally).ok();
}

#[test]
fn no_usage_file_means_no_tally_written() {
    // Without AISHE_USAGE_FILE (i.e. not under a PTY session), nothing is tallied.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_FAKE_LLM", "ls -la")
        .env("AISHE_FAKE_USAGE", "10,5")
        .env_remove("AISHE_USAGE_FILE")
        .args(["--suggest-line", "list files"])
        .assert()
        .success();
    // Nothing to assert beyond a clean run: the absence of a tally file is the
    // point (no env var, no path to write to).
}

#[test]
fn dash_c_commands_are_recorded_in_history() {
    // Commands run via `-c` are persisted to the timestamped history log, so a
    // later `aishe history` (and semantic indexing) can see them.
    let home = temp_config_home();
    let data = home.join("data");
    for c in ["!echo alpha", "!echo bravo"] {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", &data)
            .arg("-c")
            .arg(c)
            .assert()
            .success();
    }
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .arg("-c")
        .arg("history")
        .assert()
        .success()
        .stdout(contains("echo alpha").and(contains("echo bravo")));
}

#[test]
fn fix_line_prints_a_corrected_command() {
    // The fix-the-last-command hook returns a corrected command for the widget to
    // pre-fill. With fix_capture_stderr on, a read-only failed command is re-run
    // to capture its error (here the fake model just echoes a fixed command).
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_LAST_EXIT", "2")
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"ls -la /tmp","explanation":"fixed"}"#,
        )
        .args(["--fix-line", "ls /nonexistent-aishe-xyz"])
        .assert()
        .success()
        .stdout(contains("ls -la /tmp"));
}

#[test]
fn dash_c_runs_forced_shell_command() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
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
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
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
fn missing_openai_key_names_the_exact_environment_variable() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("AISHE_FAKE_LLM")
        .args([
            "--provider",
            "openai",
            "-c",
            "?what is the capital of France",
        ])
        .assert()
        .code(1)
        .stderr(contains("API key $OPENAI_API_KEY not set").and(contains("LLM not configured")));
}

#[test]
fn doctor_probe_runs_reachability_section() {
    // `--probe` adds the reachability section. Point the provider at a dead local
    // port so the probe deterministically reports unreachable without real
    // network, and doctor still exits 0 (a down endpoint is a warning, not
    // critical).
    let dir = std::env::temp_dir().join(format!("aishe-probe-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[providers.anthropic]
base_url = "http://127.0.0.1:1"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"
"#,
    )
    .unwrap();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .arg("doctor")
        .arg("--probe")
        .assert()
        .success()
        .stdout(contains("reachability probe:"))
        .stdout(contains("anthropic: unreachable"));
    std::fs::remove_dir_all(&dir).ok();
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
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", home.join("data"))
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
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .write_stdin("!echo piped-a\n!echo piped-b\n")
        .assert()
        .success()
        .stdout(contains("piped-a"))
        .stdout(contains("piped-b"));
}

#[test]
fn version_includes_build_metadata() {
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.starts_with("aishe ") && version.contains('('),
        "missing build metadata: {version:?}"
    );

    // In a Git checkout the embedded revision must identify this exact source
    // state. This catches build.rs watching `.git/HEAD` but not the branch ref,
    // which otherwise leaves incremental builds reporting an older commit.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if repo.join(".git").exists() {
        let git = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        if git.status.success() {
            let expected = String::from_utf8_lossy(&git.stdout);
            let expected = expected.trim();
            assert!(
                version.contains(expected),
                "version {version:?} does not contain current Git revision {expected:?}"
            );
        }
    }
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
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
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
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
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
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
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
    let model_file = dir.join("pty-model");

    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .env("AISHE_MODEL_FILE", &model_file)
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
    assert_eq!(std::fs::read_to_string(&model_file).unwrap(), "gpt-z2");

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
fn profile_changes_are_transparent_and_readiness_json_is_stable() {
    let dir = temp_root("profile-readiness");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"version = 2

[aishe]
safety_profile = "custom"
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "http://127.0.0.1:9"
api_key_env = "UNUSED_LOCAL_KEY"
model = "local-readiness-model"
transport = "chat"
auth_required = false
"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .args(args);
        command
    };

    run(&["profile", "balanced"])
        .assert()
        .success()
        .stdout(contains("profile = balanced"))
        .stdout(contains("mode: suggest → auto"))
        .stdout(contains("yolo_confirm: dangerous → writes"));
    run(&["readiness", "--json"])
        .assert()
        .failure()
        .stdout(contains("\"ready\": false"))
        .stdout(contains("\"id\": \"provider_tools\""))
        .stdout(contains("\"id\": \"sandbox\""))
        .stdout(contains("\"id\": \"redaction\""));

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

#[test]
fn log_and_usage_read_the_audit_log() {
    // Seed an audit log and confirm `aishe log` / `aishe usage` read it via the
    // AISHE_LOG_FILE override (child env only; never touches a real log).
    let dir = std::env::temp_dir().join(format!("aishe-cli-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(
        &log,
        "{\"ts_ms\":1781304002000,\"session\":\"s1\",\"kind\":\"ai_response\",\"model\":\"gpt-4o\",\"tokens_in\":1000,\"tokens_out\":200,\"summary\":\"ok\"}\n\
         {\"ts_ms\":1781304003000,\"session\":\"s1\",\"kind\":\"action\",\"source\":\"yolo\",\"command\":\"apt-get install nginx\",\"exit\":0}\n",
    )
    .unwrap();

    // `aishe log` shows both entries.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_LOG_FILE", &log)
        .arg("log")
        .assert()
        .success()
        .stdout(contains("apt-get install nginx").and(contains("gpt-4o")));

    // `aishe log --action action` filters to the command.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_LOG_FILE", &log)
        .args(["log", "--action", "action"])
        .assert()
        .success()
        .stdout(contains("apt-get install nginx"));

    // `aishe usage` totals tokens and estimates cost (gpt-4o known price).
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_LOG_FILE", &log)
        .arg("usage")
        .assert()
        .success()
        .stdout(
            contains("1000 in")
                .and(contains("~$"))
                .and(contains("TOTAL")),
        );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runbook_generates_script_and_markdown() {
    let dir = std::env::temp_dir().join(format!("aishe-cli-rb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(
        &log,
        "{\"ts_ms\":1781304001000,\"session\":\"sx\",\"kind\":\"ai_request\",\"mode\":\"yolo\",\"model\":\"gpt-4o\",\"prompt\":\"install nginx\"}\n\
         {\"ts_ms\":1781304003000,\"session\":\"sx\",\"kind\":\"action\",\"source\":\"yolo:run_command\",\"command\":\"apt-get install -y nginx\",\"exit\":0}\n",
    )
    .unwrap();

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_LOG_FILE", &log)
        .args(["runbook", "-o", dir.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("runbook-sx.sh").and(contains("runbook-sx.md")));

    let sh = std::fs::read_to_string(dir.join("runbook-sx.sh")).unwrap();
    assert!(sh.starts_with("#!/usr/bin/env bash"));
    assert!(sh.contains("apt-get install -y nginx"));
    assert!(sh.contains("install nginx")); // request in the header

    let md = std::fs::read_to_string(dir.join("runbook-sx.md")).unwrap();
    assert!(md.contains("# Runbook: install nginx"));
    assert!(md.contains("`apt-get install -y nginx`"));
    assert!(md.contains("## Reproduce"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_config_in_non_tty_mode_is_actionable_and_does_not_write_defaults() {
    let dir = temp_root("missing-config");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .arg("-c")
        .arg("!true")
        .assert()
        .failure()
        .stderr(contains("aishe setup --non-interactive"));
    assert!(!dir.join("aishe").join("config.toml").exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn noninteractive_setup_is_rerunnable_and_preserves_existing_fields() {
    let dir = temp_root("setup");
    let data = dir.join("data");
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    run(&[
        "setup",
        "--non-interactive",
        "--service",
        "ollama",
        "--model",
        "local-test-model",
        "--profile",
        "balanced",
        "--input-price",
        "0.25",
        "--output-price",
        "0.75",
    ])
    .assert()
    .success()
    .stdout(contains("Saved config"));

    let config_path = dir.join("aishe").join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[named_dirs]\nimportant = \"/srv/keep-me\"\n");
    std::fs::write(&config_path, config).unwrap();

    // With an existing config, omitting --service must preserve provider
    // endpoint/auth/transport and unrelated tables while allowing an override.
    run(&[
        "setup",
        "--non-interactive",
        "--model",
        "local-test-model-v2",
    ])
    .assert()
    .success();
    let updated = std::fs::read_to_string(&config_path).unwrap();
    assert!(updated.contains("base_url = \"http://localhost:11434\""));
    assert!(updated.contains("auth_required = false"));
    assert!(updated.contains("model = \"local-test-model-v2\""));
    assert!(updated.contains("important = \"/srv/keep-me\""));
    assert!(updated.contains("[pricing.local-test-model]"));
    run(&[
        "setup",
        "--non-interactive",
        "--model",
        "local-test-model-v3",
    ])
    .assert()
    .success();
    let backup_texts: Vec<String> = std::fs::read_dir(dir.join("aishe"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".setup."))
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert_eq!(
        backup_texts.len(),
        2,
        "rapid setup applies must create distinct backups"
    );
    assert!(
        backup_texts
            .iter()
            .any(|text| text.contains("model = \"local-test-model-v2\"")),
        "second setup state was not preserved in its own backup"
    );
    assert!(
        backup_texts
            .iter()
            .any(|text| text.contains("model = \"local-test-model\"")),
        "original setup state was not preserved in its own backup"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn price_commands_persist_exact_model_rates_and_validate_values() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    run(&[
        "price",
        "set",
        "gpt-5.6-luna",
        "--input",
        "1.125",
        "--output",
        "7.25",
    ])
    .assert()
    .success()
    .stdout(contains("gpt-5.6-luna"));
    run(&["price", "list"])
        .assert()
        .success()
        .stdout(contains("input $1.125000").and(contains("output $7.250000")));
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("gpt-5.6-luna"));
    run(&["price", "set", "bad", "--input=-1", "--output", "2"])
        .assert()
        .failure()
        .stderr(contains("non-negative"));
    run(&["price", "remove", "gpt-5.6-luna"]).assert().success();
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(!persisted.contains("gpt-5.6-luna"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let config_path = dir.join("aishe").join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str(
        r#"
[mcp_servers.private]
command = "private-tool"

[mcp_servers.private.env]
TOKEN = "sk-proj-this-is-only-a-fake-test-secret"

[mcp_servers.private.headers]
Authorization = "Bearer fake-private-header"
"#,
    );
    std::fs::write(&config_path, config).unwrap();
    let bundle = dir.join("support.json");
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", &data)
        .env("ANTHROPIC_API_KEY", "fake-test-key")
        .args([
            "doctor",
            "--json",
            "--fix",
            "--bundle",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<&str> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|check| check["id"].as_str())
        .collect();
    assert!(ids.contains(&"config.file"));
    assert!(ids.contains(&"provider.credential"));
    assert!(ids.contains(&"history.persistence"));
    assert!(ids.contains(&"repair.safe"));
    let config_check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "config.file")
        .unwrap();
    assert!(
        config_check["detail"]
            .as_str()
            .unwrap()
            .contains("schema 1 on disk"),
        "Doctor must report the source schema rather than serde's current-schema default"
    );

    let support = std::fs::read_to_string(&bundle).unwrap();
    assert!(!support.contains("sk-proj-this-is-only-a-fake-test-secret"));
    assert!(!support.contains("Bearer fake-private-header"));
    assert!(support.contains("<redacted>"));
    assert!(support.contains("\"command history\""));

    // The safe repair path is idempotent.
    let second = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", &data)
        .env("ANTHROPIC_API_KEY", "fake-test-key")
        .args(["doctor", "--json", "--fix"])
        .output()
        .unwrap();
    assert!(second.status.success());
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let repair = second["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "repair.safe")
        .unwrap();
    assert_eq!(
        repair["changed_paths"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        0
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn effective_config_and_context_json_are_structured_and_content_free() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let project = dir.join("project");
    std::fs::create_dir_all(project.join(".aishe")).unwrap();
    let fake_secret = "sk-proj-fake-context-secret-abcdefghijklmnopqrstuvwxyz";
    std::fs::write(
        project.join(".aishe").join("context.md"),
        format!("never expose {fake_secret}\n"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .current_dir(&project)
            .args(args);
        command
    };
    let output = run(&["config", "--effective", "--json"]).output().unwrap();
    assert!(output.status.success());
    let effective: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(effective["config"].is_object());
    assert!(effective["provenance"]["fields"].is_array());

    let request = "private request text must not be echoed";
    let output = run(&["context", "--preview", request, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(!raw.contains(request));
    assert!(!raw.contains(fake_secret));
    let preview: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(preview["sections"].is_array());
    assert!(preview["total_estimated_tokens"].as_u64().is_some());

    run(&["context", "--exclude", "project_context", "--json"])
        .assert()
        .success();
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("project_context"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn noninteractive_tour_is_isolated_resumable_and_proves_undo() {
    let dir = temp_root("tour");
    let cwd = dir.join("invocation");
    std::fs::create_dir_all(&cwd).unwrap();
    let sentinel = cwd.join("keep.txt");
    std::fs::write(&sentinel, "unchanged").unwrap();
    let run = || {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", dir.join("config"))
            .env("AISHE_DATA_DIR", dir.join("data"))
            .current_dir(&cwd)
            .args(["tour", "--non-interactive"]);
        command
    };
    run()
        .assert()
        .success()
        .stdout(contains("Tour complete").and(contains("proved undo")));
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "unchanged");
    assert!(!dir.join("data/aishe/tour/workspace/undo-demo.txt").exists());
    run()
        .assert()
        .success()
        .stdout(contains("tour is complete"));
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("data/aishe/tour/state.json")).unwrap())
            .unwrap();
    assert_eq!(state["completed"], true);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn durable_task_cli_lifecycle_is_private_and_redacted() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let fake_secret = "sk-proj-fake-task-secret-abcdefghijklmnopqrstuvwxyz";
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .env("AISHE_FAKE_LLM", "task complete")
            .args(args);
        command
    };
    run(&["--yolo-line", &format!("summarize {fake_secret}")])
        .assert()
        .success()
        .stdout(contains("task complete"));
    let listing = run(&["sessions", "--json"]).output().unwrap();
    assert!(listing.status.success());
    let records: serde_json::Value = serde_json::from_slice(&listing.stdout).unwrap();
    let record = &records.as_array().unwrap()[0];
    let id = record["id"].as_str().unwrap();
    assert_eq!(record["status"], "completed");
    let task_path = data.join("aishe").join("tasks").join(format!("{id}.json"));
    let task_text = std::fs::read_to_string(&task_path).unwrap();
    assert!(!task_text.contains(fake_secret));
    assert!(task_text.contains("<redacted>"));

    run(&["session", "show", id, "--json"])
        .assert()
        .success()
        .stdout(contains("\"status\": \"completed\""));
    run(&["session", "rename", id, "deployment check"])
        .assert()
        .success();
    run(&["session", "delete", id]).assert().success();
    assert!(!task_path.exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn provider_test_and_model_listing_support_local_unauthenticated_endpoints() {
    let dir = temp_root("provider-test");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    let (endpoint, server) = serve_model_catalog(3);
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"version = 2

[aishe]
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "{endpoint}"
api_key_env = "LOCAL_UNUSED_KEY"
model = "local-model-a"
transport = "chat"
auth_required = false
"#
        ),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .env_remove("LOCAL_UNUSED_KEY")
            .args(args);
        command
    };
    let output = run(&["provider", "test", "--json"]).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["credential"]["state"], "pass");
    assert_eq!(report["credential_required"], false);
    assert_eq!(report["model_available"]["state"], "pass");
    assert_eq!(report["text"]["state"], "skipped");

    run(&["models", "--provider", "openai", "--json"])
        .assert()
        .success()
        .stdout(contains("local-model-a").and(contains("local-model-b")));
    server.join().unwrap();
    std::fs::remove_dir_all(dir).ok();
}
