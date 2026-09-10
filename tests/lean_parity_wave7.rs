//! Wave 7: noninteractive lean agent smoke (closes lean-extra-validation SKIP).
//!
//! Exercises the in-process agent/tool path with FakeProvider + preloaded
//! `AISHE_ACCEPTANCE_FILE` so CI never needs a TTY grant dance or OpenCode.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use assert_cmd::Command as CargoCommand;

use aishe::config::Config;
use aishe::executor::Executor;
use aishe::lean::{self, grant_accepted, handle_ipc_line, LeanMode, LeanWarm, PtyOut};
use aishe::mcp::McpRegistry;
use aishe::providers::fake::FakeProvider;
use aishe::session::Session;
use aishe::skills::SkillRegistry;

/// Process-global env + grant atomics are shared across #[test] threads.
static GRANT_TEST_LOCK: Mutex<()> = Mutex::new(());

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w7-{label}-{}-{}",
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
output = "focus"

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
    let home = temp_config_home();
    let mut cmd = CargoCommand::cargo_bin("aishe").unwrap();
    cmd.env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("AISHE_CONFIG_DIR", &home)
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1")
        .env_remove("XAI_API_KEY")
        .env_remove("ANTHROPIC_API_KEY");
    cmd
}

/// Preload session grant via `AISHE_ACCEPTANCE_FILE` (documented test/CI hook).
#[test]
fn acceptance_file_preloads_agent_grant() {
    let _guard = GRANT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("grant");
    let accept = root.join("accept");
    std::fs::write(&accept, "agent\n").unwrap();
    std::env::set_var("AISHE_ACCEPTANCE_FILE", &accept);
    assert!(
        grant_accepted(LeanMode::Agent, false),
        "AISHE_ACCEPTANCE_FILE with `agent` must satisfy workspace agent grant"
    );
    std::env::remove_var("AISHE_ACCEPTANCE_FILE");
}

/// `lean::run_nl` agent mode + FakeProvider tool call — no TTY, no OpenCode.
#[test]
fn lean_run_nl_agent_smoke_with_fake_tool_and_acceptance() {
    let _guard = GRANT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("run-nl-agent");
    let accept = root.join("accept");
    let opencode_spy = root.join("opencode");
    std::fs::write(&accept, "agent\n").unwrap();
    std::env::set_var("AISHE_ACCEPTANCE_FILE", &accept);
    std::env::set_var("AISHE_SPY_OPENCODE", &opencode_spy);
    std::env::set_var("AISHE_FAKE_TOOL", "true");
    std::env::set_var("AISHE_FAKE_LLM", "agent-smoke-done");
    std::env::remove_var("AISHE_LEGACY_OPENCODE");
    std::env::set_var("AISHE_LEAN", "1");

    let provider = FakeProvider::new("agent-smoke-done".into());
    let mut executor = Executor::new().expect("executor");
    let mut config = Config::default();
    config.aishe.yolo_confirm = "never".into();
    config.aishe.yolo_confirm_dangerous = false;
    config.backend.default_scope = "workspace".into();
    // Keep sandbox off OS wrap so CI does not require bwrap for this unit path.
    config.sandbox.linux_backend = "off".into();

    lean::run_nl(
        "please run the true command",
        "agent",
        Some(&provider),
        &mut executor,
        &config,
        &SkillRegistry::default(),
        &McpRegistry::default(),
        &mut Session::new(false),
    )
    .expect("lean agent run_nl must succeed noninteractively with acceptance file");

    assert!(
        !opencode_spy.exists(),
        "lean agent path must not start OpenCode"
    );
    assert!(
        executor
            .history
            .iter()
            .any(|(cmd, code)| cmd.trim() == "true" && *code == 0),
        "fake tool should have run `true` via agent loop; history={:?}",
        executor.history
    );

    std::env::remove_var("AISHE_ACCEPTANCE_FILE");
    std::env::remove_var("AISHE_SPY_OPENCODE");
    std::env::remove_var("AISHE_FAKE_TOOL");
    std::env::remove_var("AISHE_FAKE_LLM");
}

/// FIFO `NL` agent mode uses in-process yolo (same FakeProvider tool hook).
#[test]
fn lean_fifo_agent_nl_smoke_no_opencode() {
    let _guard = GRANT_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = temp_root("fifo-agent");
    let accept = root.join("accept");
    let opencode_spy = root.join("opencode");
    std::fs::write(&accept, "agent\n").unwrap();
    std::env::set_var("AISHE_ACCEPTANCE_FILE", &accept);
    std::env::set_var("AISHE_SPY_OPENCODE", &opencode_spy);
    std::env::set_var("AISHE_FAKE_TOOL", "true");
    std::env::set_var("AISHE_FAKE_LLM", "fifo-agent-done");
    std::env::set_var("AISHE_LEAN", "1");

    let home = temp_config_home();
    std::env::set_var("XDG_CONFIG_HOME", &home);
    std::env::set_var("AISHE_CONFIG_DIR", &home);
    std::env::set_var("HOME", &home);

    let mut config = Config::default();
    config.aishe.yolo_confirm = "never".into();
    config.aishe.yolo_confirm_dangerous = false;
    config.sandbox.linux_backend = "off".into();

    let mut provider: Option<Arc<dyn aishe::providers::Provider>> =
        Some(Arc::new(FakeProvider::new("fifo-agent-done".into())));
    let mut executor = Executor::new().expect("executor");
    let mut session = Session::new(true);
    let mut store = None;
    let mut warm = LeanWarm::default();
    let pty = PtyOut::capture();
    let cwd = std::env::current_dir().unwrap();
    let cwd_s = cwd.to_string_lossy();

    let reply = handle_ipc_line(
        &mut config,
        &mut provider,
        &mut executor,
        &mut session,
        &mut store,
        &mut warm,
        &pty,
        &format!("NL\tagent\t{cwd_s}\tplease run true"),
    );
    assert_eq!(
        reply,
        "RAN",
        "FIFO agent NL should return RAN, got {reply:?}; pty={}",
        pty.take_capture()
    );
    assert!(!opencode_spy.exists(), "FIFO agent must not start OpenCode");
    assert!(
        executor
            .history
            .iter()
            .any(|(cmd, code)| cmd.trim() == "true" && *code == 0),
        "FIFO agent should execute fake tool `true`; history={:?}",
        executor.history
    );

    std::env::remove_var("AISHE_ACCEPTANCE_FILE");
    std::env::remove_var("AISHE_SPY_OPENCODE");
    std::env::remove_var("AISHE_FAKE_TOOL");
    std::env::remove_var("AISHE_FAKE_LLM");
}

/// One-shot `-c --mode agent` with FakeProvider: tool loop, spies green.
#[test]
fn dash_c_mode_agent_fake_tool_skips_opencode() {
    let root = temp_root("dash-c-agent");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .env("AISHE_FAKE_LLM", "dash-c-agent-done")
        .env("AISHE_FAKE_TOOL", "true")
        .args(["--mode", "agent", "-c", "? please run true"])
        .assert()
        .success();
    assert!(
        provider_spy.exists(),
        "agent NL must construct in-process provider"
    );
    assert!(
        !opencode_spy.exists(),
        "lean --mode agent -c must not start OpenCode"
    );
}

/// Known-cmd hot path stays spy-clean after agent work (regression guard).
#[test]
fn known_cmd_spies_still_green_after_agent_helpers() {
    let root = temp_root("known-cmd");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .args(["-c", "true"])
        .assert()
        .success();
    assert!(
        !provider_spy.exists(),
        "known cmd must not build a Provider"
    );
    assert!(!opencode_spy.exists(), "known cmd must not start OpenCode");
}
