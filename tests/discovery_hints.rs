//! End-to-end contract for privacy-preserving discovery-hint state.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command;

fn isolated_home() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "aishe-discovery-hints-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let config_dir = root.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut config = std::fs::File::create(config_dir.join("config.toml")).unwrap();
    writeln!(
        config,
        r#"[aishe]
mode = "suggest"
provider = "anthropic"
discovery_hints = true

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
        .env("AISHE_DATA_DIR", home.join("data"));
    command
}

#[test]
fn status_and_reset_expose_only_bounded_boolean_seen_state() {
    let home = isolated_home();
    let state_path = home.join("data/aishe/discovery-hints.json");

    let initial = aishe(&home)
        .args(["hints", "status", "--json"])
        .output()
        .unwrap();
    assert!(initial.status.success());
    let initial: serde_json::Value = serde_json::from_slice(&initial.stdout).unwrap();
    assert_eq!(initial["schema_version"], 1);
    assert_eq!(initial["enabled"], true);
    assert_eq!(initial["launch_hint_seen"], false);
    assert_eq!(initial["first_answer_hint_seen"], false);

    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    std::fs::write(
        &state_path,
        b"{\"schema_version\":1,\"launch_hint_seen\":true,\"first_answer_hint_seen\":true}\n",
    )
    .unwrap();
    let unrelated = home.join("data/aishe/unrelated-state");
    std::fs::write(&unrelated, b"preserve me").unwrap();
    let config_path = home.join("aishe/config.toml");
    let config_before = std::fs::read(&config_path).unwrap();

    aishe(&home).args(["hints", "reset"]).assert().success();

    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    let object = state.as_object().unwrap();
    assert_eq!(object.len(), 3, "state must contain no user content");
    assert_eq!(state["schema_version"], 1);
    assert_eq!(state["launch_hint_seen"], false);
    assert_eq!(state["first_answer_hint_seen"], false);
    assert_eq!(std::fs::read(&unrelated).unwrap(), b"preserve me");
    assert_eq!(std::fs::read(&config_path).unwrap(), config_before);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    std::fs::remove_dir_all(home).ok();
}
