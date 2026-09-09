//! Wave 2 lean-native smoke: durable sessions, /usage, @file, failure/?, doctor.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command as CargoCommand;
use predicates::str::contains;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w2-{label}-{}-{}",
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
mode = "suggest"
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
fn lean_hook_records_failure_and_binds_fix() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("--record-failure"),
        "lean hook must record failure capsules"
    );
    assert!(
        hook.contains("aishe-fix-command") && hook.contains("AISHE_FIX_KEY"),
        "lean hook must bind fix-last"
    );
    assert!(
        hook.contains("_aishe_lean_nl \"?\""),
        "empty ? must route to explain-last"
    );
    assert!(
        hook.contains("/sessions"),
        "lean slash surface includes /sessions"
    );
}

#[test]
fn doctor_includes_lean_section_without_tokens() {
    let report = aishe::diagnostics::inspect(
        env!("CARGO_PKG_VERSION"),
        &aishe::diagnostics::Options::default(),
    );
    let ids: Vec<_> = report.checks.iter().map(|c| c.id.as_str()).collect();
    for need in [
        "lean.enabled",
        "lean.fifo",
        "lean.compsys",
        "lean.grok_auth",
        "lean.bwrap",
        "lean.leanrc",
        "lean.sessions",
    ] {
        assert!(
            ids.contains(&need),
            "missing doctor check {need}; have {ids:?}"
        );
    }
    for check in &report.checks {
        if check.id.starts_with("lean.") {
            let blob = format!("{} {} {}", check.summary, check.detail, check.id);
            assert!(
                !blob.to_ascii_lowercase().contains("eyj")
                    && !blob.contains("Bearer ")
                    && !blob.contains("access_token"),
                "doctor must not print tokens: {blob}"
            );
        }
    }
}

#[test]
fn sessions_cli_lists_lean_jsonl_store() {
    let root = temp_root("lean-sess");
    std::fs::create_dir_all(&root).unwrap();
    let id = "lean-wave2-test-id";
    std::fs::write(
        root.join(format!("{id}.jsonl")),
        r#"{"role":"user","content":"wave2 durable"}
{"role":"assistant","content":"ok"}
"#,
    )
    .unwrap();
    std::fs::write(
        root.join("index.json"),
        format!(
            r#"{{"schema_version":1,"sessions":[{{"id":"{id}","created_at_ms":1,"updated_at_ms":2,"title":"wave2 durable","cwd":"/tmp","model":"m","turns":1}}]}}"#
        ),
    )
    .unwrap();
    bin()
        .env("AISHE_LEAN_SESSIONS", &root)
        .args(["sessions", "--json"])
        .assert()
        .success()
        .stdout(contains("lean"))
        .stdout(contains(id));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn lean_sessions_root_is_under_xdg_or_override() {
    let root = temp_root("override");
    std::env::set_var("AISHE_LEAN_SESSIONS", &root);
    assert_eq!(aishe::lean::lean_sessions_root(), root);
    std::env::remove_var("AISHE_LEAN_SESSIONS");
}
