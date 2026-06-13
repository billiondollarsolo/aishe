//! End-to-end test for `aishe dry-run` (proposal R2 overlay preview). Skips when
//! bubblewrap isn't installed, since the safe isolation depends on it.

use std::io::Write;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A temp working tree with a couple of files, plus a config home so the wizard
/// never runs.
fn setup(label: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!("aishe-dryrun-it-{label}-{}", std::process::id()));
    let cfg = root.join("cfg").join("aishe");
    let work = root.join("work");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    let mut f = std::fs::File::create(cfg.join("config.toml")).unwrap();
    writeln!(
        f,
        "[aishe]\nmode = \"suggest\"\nprovider = \"anthropic\"\n\n[providers.anthropic]\nbase_url = \"https://api.anthropic.com\"\napi_key_env = \"ANTHROPIC_API_KEY\"\nmodel = \"claude-x\""
    )
    .unwrap();
    std::fs::write(work.join("config.ini"), "v1\n").unwrap();
    (root.join("cfg"), work)
}

#[test]
fn dry_run_previews_then_discards_by_default() {
    if !bwrap_available() {
        eprintln!("SKIP: bubblewrap not installed");
        return;
    }
    let (cfg, work) = setup("discard");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &cfg)
        .env("XDG_DATA_HOME", cfg.join("data"))
        .current_dir(&work)
        .args(["dry-run", "echo v2 > config.ini; echo hi > new.txt"])
        .assert()
        .success()
        .stdout(contains("added").and(contains("new.txt")))
        .stdout(contains("modified").and(contains("config.ini")))
        .stdout(contains("discarded"));
    // Discard is the default: the real tree is untouched.
    assert_eq!(
        std::fs::read_to_string(work.join("config.ini")).unwrap(),
        "v1\n"
    );
    assert!(
        !work.join("new.txt").exists(),
        "discard must not create files"
    );
    std::fs::remove_dir_all(cfg.parent().unwrap()).ok();
}

#[test]
fn dry_run_apply_writes_the_changes() {
    if !bwrap_available() {
        eprintln!("SKIP: bubblewrap not installed");
        return;
    }
    let (cfg, work) = setup("apply");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &cfg)
        .env("XDG_DATA_HOME", cfg.join("data"))
        .current_dir(&work)
        .args([
            "dry-run",
            "--apply",
            "echo v2 > config.ini; echo hi > new.txt",
        ])
        .assert()
        .success()
        .stdout(contains("applied"));
    assert_eq!(
        std::fs::read_to_string(work.join("config.ini")).unwrap(),
        "v2\n"
    );
    assert_eq!(
        std::fs::read_to_string(work.join("new.txt")).unwrap(),
        "hi\n"
    );
    std::fs::remove_dir_all(cfg.parent().unwrap()).ok();
}

#[test]
fn yolo_dry_run_session_previews_applies_and_is_undoable() {
    if !bwrap_available() {
        eprintln!("SKIP: bubblewrap not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("aishe-yolodry-{}", std::process::id()));
    let cfg = root.join("cfg").join("aishe");
    let work = root.join("work");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        cfg.join("config.toml"),
        "[aishe]\nmode = \"yolo\"\nprovider = \"anthropic\"\nyolo_dry_run = true\nyolo_confirm = \"never\"\n\n[providers.anthropic]\nbase_url = \"https://api.anthropic.com\"\napi_key_env = \"ANTHROPIC_API_KEY\"\nmodel = \"claude-x\"\n",
    )
    .unwrap();
    std::fs::write(work.join("data.txt"), "v1\n").unwrap();

    let cfg_home = root.join("cfg");
    let data_home = cfg_home.join("data");
    let run = |args: &[&str]| {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &cfg_home)
            .env("XDG_DATA_HOME", &data_home)
            .env("ANTHROPIC_API_KEY", "sk-test")
            .env("AISHE_FAKE_LLM", "done")
            .env(
                "AISHE_FAKE_TOOL",
                "echo v2 > data.txt; echo new > created.txt",
            )
            .current_dir(&work)
            .args(args)
            .assert()
            .success()
    };
    // A non-interactive (-c) yolo session runs in the staging copy, previews the
    // changes, and auto-applies them (journaled).
    run(&["-c", "update the data file"]).stdout(contains("dry-run").and(contains("applied")));
    assert_eq!(
        std::fs::read_to_string(work.join("data.txt")).unwrap(),
        "v2\n"
    );
    assert!(work.join("created.txt").exists());

    // The whole batch is reversible.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &cfg_home)
        .env("XDG_DATA_HOME", &data_home)
        .current_dir(&work)
        .arg("undo")
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(work.join("data.txt")).unwrap(),
        "v1\n"
    );
    assert!(
        !work.join("created.txt").exists(),
        "undo removes the added file"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn dry_run_apply_is_undoable() {
    if !bwrap_available() {
        eprintln!("SKIP: bubblewrap not installed");
        return;
    }
    let (cfg, work) = setup("undo");
    let data = cfg.join("data");
    let run = |args: &[&str]| {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &cfg)
            .env("XDG_DATA_HOME", &data)
            .current_dir(&work)
            .args(args)
            .assert()
            .success();
    };
    // Apply a change set (modify + add), then revert it with `aishe undo`.
    run(&[
        "dry-run",
        "--apply",
        "echo v2 > config.ini; echo hi > new.txt",
    ]);
    assert_eq!(
        std::fs::read_to_string(work.join("config.ini")).unwrap(),
        "v2\n"
    );
    assert!(work.join("new.txt").exists());

    run(&["undo"]);
    assert_eq!(
        std::fs::read_to_string(work.join("config.ini")).unwrap(),
        "v1\n",
        "undo restores the modified file"
    );
    assert!(
        !work.join("new.txt").exists(),
        "undo removes the added file"
    );
    std::fs::remove_dir_all(cfg.parent().unwrap()).ok();
}
