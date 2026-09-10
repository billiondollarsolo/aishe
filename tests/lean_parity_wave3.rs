//! Wave 3 lean-native smoke: hook surfaces, mode aliases, CLI help.
//! FIFO /connection /model /status /redact coverage lives in `lean::nl` unit tests.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command as CargoCommand;
use predicates::str::contains;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w3-{label}-{}-{}",
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
    cmd.env("XDG_CONFIG_HOME", temp_config_home())
        .env("XDG_DATA_HOME", temp_root("data"))
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1");
    cmd
}

#[test]
fn lean_hook_surfaces_connection_and_mode_aliases() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("/connection"),
        "lean hook must route /connection on FIFO"
    );
    assert!(
        hook.contains("ask|suggest") && hook.contains("allow|auto") && hook.contains("agent|yolo"),
        "lean /mode must accept product+legacy aliases"
    );
    assert!(
        hook.contains("/help|/commands") || hook.contains("/commands"),
        "lean hook must FIFO-route /help and /commands"
    );
    assert!(
        hook.contains("/connection") && hook.contains("/model"),
        "lean hook must route /connection and /model"
    );
}

#[test]
fn cli_mode_help_lists_ask_allow_agent() {
    bin()
        .args(["mode", "--help"])
        .assert()
        .success()
        .stdout(contains("ask"))
        .stdout(contains("allow"))
        .stdout(contains("agent"));
}

#[test]
fn mode_parse_accepts_product_and_legacy_names() {
    use aishe::agent::Mode;
    assert_eq!(Mode::parse("ask"), Some(Mode::Suggest));
    assert_eq!(Mode::parse("allow"), Some(Mode::Auto));
    assert_eq!(Mode::parse("agent"), Some(Mode::Yolo));
    assert_eq!(Mode::parse("suggest"), Some(Mode::Suggest));
    assert_eq!(Mode::parse("auto"), Some(Mode::Auto));
    assert_eq!(Mode::parse("yolo"), Some(Mode::Yolo));
}

#[test]
fn lean_hotpath_doc_mentions_no_opencode_payload() {
    let doc = include_str!("../docs/lean-hotpath.md");
    assert!(
        doc.contains("Wave 3") && doc.to_ascii_lowercase().contains("no opencode"),
        "lean-hotpath.md must document Wave 3 / no OpenCode payload"
    );
}
