//! End-to-end tests for semantic history search (`aishe history index|search`),
//! driven through the built binary with the deterministic fake embedder
//! (`AISHE_FAKE_LLM`), so no network or API key is needed.

use std::io::Write;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// A unique temp tree with a config + data home. `semantic` toggles the feature.
fn setup(label: &str, semantic: bool) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("aishe-semhist-{label}-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    let data_dir = dir.join("data").join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut f = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    writeln!(
        f,
        r#"[aishe]
mode = "suggest"
provider = "openai"
semantic_history = {semantic}
embedding_provider = "openai"
embedding_model = "fake-embed"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-x"
"#
    )
    .unwrap();
    (dir, data_dir)
}

/// Seed the timestamped history log the index reads from.
fn seed_history(data_dir: &std::path::Path, cmds: &[&str]) {
    let mut f = std::fs::File::create(data_dir.join("history.ext")).unwrap();
    for (i, c) in cmds.iter().enumerate() {
        writeln!(f, ": {}:0;{c}", 1_700_000_000 + i as u64).unwrap();
    }
}

fn run(dir: &std::path::Path) -> Command {
    let mut c = Command::cargo_bin("aishe").unwrap();
    c.env("XDG_CONFIG_HOME", dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("AISHE_FAKE_LLM", "x") // swaps in the deterministic fake embedder
        .env("OPENAI_API_KEY", "sk-test");
    c
}

#[test]
fn index_then_search_ranks_the_relevant_command_first() {
    let (dir, data) = setup("rank", true);
    seed_history(
        &data,
        &[
            "git status",
            "docker run -v /data/prometheus:/prom prom/prometheus",
            "ls -la /tmp",
            "kubectl get pods",
        ],
    );

    // Index the seeded history.
    run(&dir)
        .args(["history", "index"])
        .assert()
        .success()
        .stdout(contains("indexed").and(contains("command")));

    // A natural-language query lands on the docker/prometheus command.
    run(&dir)
        .args([
            "history",
            "search",
            "the docker run with the prometheus volume",
        ])
        .assert()
        .success()
        .stdout(contains("prom/prometheus"));
}

#[test]
fn search_first_result_is_the_best_match() {
    let (dir, data) = setup("best", true);
    seed_history(
        &data,
        &[
            "echo hello world",
            "docker compose up prometheus grafana",
            "find . -name '*.rs'",
        ],
    );
    run(&dir).args(["history", "index"]).assert().success();

    let out = run(&dir)
        .args(["history", "search", "prometheus grafana compose", "-n", "1"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("docker compose up prometheus grafana"),
        "top hit should be the compose command, got: {stdout}"
    );
}

#[test]
fn off_by_default_index_and_search_explain_how_to_enable() {
    let (dir, data) = setup("off", false);
    seed_history(&data, &["git status", "docker ps"]);

    run(&dir)
        .args(["history", "index"])
        .assert()
        .success()
        .stdout(contains("semantic history is off"));

    run(&dir)
        .args(["history", "search", "docker"])
        .assert()
        .success()
        .stdout(contains("semantic history is off"));

    // Nothing was embedded while the feature was off.
    assert!(
        !data.join("history.vec").exists(),
        "no vector store should be written when the feature is off"
    );
}

#[test]
fn search_before_indexing_prompts_to_index() {
    let (dir, _data) = setup("empty", true);
    run(&dir)
        .args(["history", "search", "anything"])
        .assert()
        .success()
        .stdout(contains("run `aishe history index`"));
}

#[test]
fn index_is_incremental_and_reports_up_to_date() {
    let (dir, data) = setup("incr", true);
    seed_history(&data, &["git status", "docker ps"]);
    // First index embeds both.
    run(&dir)
        .args(["history", "index"])
        .assert()
        .success()
        .stdout(contains("indexed 2 command"));
    // Re-running with no new commands reports up-to-date (nothing re-embedded).
    run(&dir)
        .args(["history", "index"])
        .assert()
        .success()
        .stdout(contains("up to date").and(contains("2 commands")));
}

#[test]
fn index_with_no_history_says_so() {
    let (dir, _data) = setup("nohist", true);
    run(&dir)
        .args(["history", "index"])
        .assert()
        .success()
        .stdout(contains("no history to index"));
}

#[test]
fn bare_search_prints_only_the_command_for_the_recall_widget() {
    let (dir, data) = setup("bare", true);
    seed_history(
        &data,
        &[
            "git status",
            "docker run -v /data/prometheus:/prom prom/prometheus",
        ],
    );
    run(&dir).args(["history", "index"]).assert().success();

    let out = run(&dir)
        .args([
            "history",
            "search",
            "docker prometheus volume",
            "-n",
            "1",
            "--bare",
        ])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Exactly the command, no score column, nothing else.
    assert_eq!(
        stdout.trim(),
        "docker run -v /data/prometheus:/prom prom/prometheus",
        "bare output must be just the command, got: {stdout:?}"
    );
}

#[test]
fn bare_search_keeps_stdout_clean_when_off() {
    // The recall widget shoves stdout into the line buffer, so when the feature
    // is off (or empty) bare mode must print nothing to stdout — notices go to
    // stderr instead.
    let (dir, data) = setup("bareoff", false);
    seed_history(&data, &["git status"]);
    let out = run(&dir)
        .args(["history", "search", "git", "--bare"])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).trim().is_empty(),
        "bare stdout should be empty when the feature is off"
    );
}

#[test]
fn init_zsh_wires_the_recall_widget() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["init", "zsh"])
        .assert()
        .success()
        .stdout(contains("aishe-recall"))
        .stdout(contains("AISHE_RECALL_KEY"));
}
