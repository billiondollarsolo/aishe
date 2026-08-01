//! Architectural guardrails for the CLI/domain and terminal-view split.

use std::path::PathBuf;

fn repository_file(relative: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn binary_remains_below_the_orchestration_line_budget() {
    let main = repository_file("src/main.rs");
    assert!(
        main.lines().count() <= 1_500,
        "src/main.rs grew to {} lines; move behavior into src/cli domain modules",
        main.lines().count()
    );
    for migrated_function in [
        "fn status_command(",
        "fn connection_command(",
        "fn tasks_list_command(",
        "fn history_command(",
        "fn log_command(",
        "fn runbook_command(",
    ] {
        assert!(
            !main.contains(migrated_function),
            "{migrated_function} belongs in a src/cli domain module"
        );
    }
}

#[test]
fn pure_ui_views_do_not_depend_on_business_or_prompt_layers() {
    let render = repository_file("src/ui/render.rs");
    for forbidden in [
        "crate::agent",
        "crate::backend",
        "crate::promptui",
        "crate::providers",
        "crate::safety",
    ] {
        assert!(
            !render.contains(forbidden),
            "pure UI renderer acquired forbidden dependency {forbidden}"
        );
    }
    let promptui = repository_file("src/promptui.rs");
    assert!(promptui.contains("pub use crate::ui::render"));
}

#[test]
fn hidden_hook_typo_is_local_silent_on_stdout_and_once_per_shell() {
    let nonce = format!("{}{}", std::process::id(), rand::random::<u64>());
    let shell_id = format!("shell{nonce}");
    let state = std::env::temp_dir().join(format!("aishe-yolo-accept-{shell_id}"));
    let config_root = std::env::temp_dir().join(format!("aishe-arch-config-{nonce}"));
    let session = std::env::temp_dir().join(format!("aishe-session-mem-{nonce}"));
    let _ = std::fs::remove_file(&state);
    let _ = std::fs::remove_file(&session);
    let _ = std::fs::remove_dir_all(&config_root);
    let config_dir = config_root.join("aishe");
    std::fs::create_dir_all(&config_dir).expect("create isolated config directory");
    std::fs::write(
        config_dir.join("config.toml"),
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[backend]
engine = "native"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "test-model"
"#,
    )
    .expect("write isolated config");

    let invoke = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_aishe"))
            .args(["--suggest-line", "gti status"])
            .env("XDG_CONFIG_HOME", &config_root)
            .env("XDG_DATA_HOME", config_root.join("data"))
            .env("AISHE_CONFIG_DIR", &config_root)
            .env("AISHE_DATA_DIR", config_root.join("data"))
            .env("AISHE_SHELL_ID", &shell_id)
            .env("AISHE_ACCEPTANCE_FILE", &state)
            .env("AISHE_SESSION_FILE", &session)
            .output()
            .expect("run hidden hook")
    };

    let first = invoke();
    assert!(
        first.status.success(),
        "first hidden-hook invocation failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(
        first.stdout.is_empty(),
        "hook protocol stdout must stay empty"
    );
    let first_stderr = String::from_utf8_lossy(&first.stderr);
    assert!(
        first_stderr.contains("did you mean 'git'?"),
        "{first_stderr}"
    );
    assert!(first_stderr.contains("Nothing ran"), "{first_stderr}");

    let second = invoke();
    assert!(second.status.success());
    assert!(second.stdout.is_empty());
    assert!(
        !String::from_utf8_lossy(&second.stderr).contains("did you mean"),
        "the same typo head should be silent after its first cue"
    );

    let _ = std::fs::remove_file(state);
    let _ = std::fs::remove_file(session);
    let _ = std::fs::remove_dir_all(config_root);
}
