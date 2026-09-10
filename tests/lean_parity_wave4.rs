//! Wave 4 lean-native smoke: F19 token streaming, /backend opt-in, LEGACY gates.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command as CargoCommand;
use predicates::str::contains;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w4-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn temp_config_home() -> PathBuf {
    let dir = temp_root("config");
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut file = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    writeln!(
        file,
        r#"[aishe]
mode = "ask"
provider = "anthropic"

[backend]
engine = "opencode"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"
"#
    )
    .unwrap();
    dir
}

fn bin() -> CargoCommand {
    let mut cmd = CargoCommand::cargo_bin("aishe").unwrap();
    let config_home = temp_config_home();
    let data_home = temp_root("data");
    cmd.env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("AISHE_CONFIG_DIR", &config_home)
        .env("AISHE_DATA_DIR", &data_home)
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1");
    cmd
}

#[test]
fn lean_hook_handles_stream_end_and_backend() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("STREAM_END"),
        "lean hook must treat STREAM_END like OK"
    );
    assert!(
        hook.contains("/backend"),
        "lean hook must surface /backend opt-in"
    );
}

#[test]
fn lean_hotpath_doc_marks_wave4_streaming() {
    let doc = include_str!("../docs/lean-hotpath.md");
    assert!(
        doc.contains("Wave 4") && doc.contains("complete_stream"),
        "lean-hotpath.md must document Wave 4 streaming"
    );
    assert!(
        doc.to_ascii_lowercase().contains("no opencode") || doc.contains("AISHE_LEGACY_OPENCODE"),
        "must keep no-OpenCode / LEGACY hatch language"
    );
}

#[test]
fn heavy_default_still_none_and_backend_slash_in_hook_routes() {
    assert_eq!(aishe::lean::heavy::select_default(), None);
    let hook = aishe::lean::wrapper_zshrc();
    assert!(hook.contains("/backend"));
}

#[test]
fn known_cmd_still_skips_provider_and_opencode_after_stream_work() {
    let root = temp_root("spy");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .args(["-c", "true"])
        .assert()
        .success();
    assert!(!provider_spy.exists(), "known-cmd must not build Provider");
    assert!(!opencode_spy.exists(), "known-cmd must not start OpenCode");
}

#[test]
fn dash_c_ask_answer_still_works_with_fake_stream_path() {
    // `-c` uses run_nl → modes::suggest::run (config.stream default false),
    // not the FIFO streamer — still must not touch OpenCode.
    let root = temp_root("nl");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","command":null,"explanation":"streamed-ish\nline2"}"#,
        )
        .args(["-c", "? wave4 stream check"])
        .assert()
        .success()
        .stdout(contains("streamed-ish"));
    assert!(!opencode_spy.exists());
}
