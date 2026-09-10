//! Wave 6 lean-native smoke: F40 custom slash-commands + 1.0 help surface.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use assert_cmd::Command as CargoCommand;

use aishe::config::Config;
use aishe::executor::Executor;
use aishe::lean::{handle_ipc_line, LeanWarm, PtyOut};
use aishe::session::Session;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w6-{label}-{}-{}",
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
    let mut cmd = CargoCommand::new(assert_cmd::cargo::cargo_bin("aishe"));
    cmd.env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("AISHE_CONFIG_DIR", &home)
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1");
    cmd
}

#[test]
fn lean_hook_routes_custom_and_commands_slash() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("/commands"),
        "lean hook must allowlist /commands"
    );
    assert!(
        hook.contains("/[[:alnum:]_-]##"),
        "lean hook must FIFO-route single-segment custom /name"
    );
    assert!(
        hook.contains("aishe-slash-tab"),
        "lean hook must offer slash tab completion"
    );
    assert!(
        hook.contains("AISHE_LEAN_CMDS_FILE"),
        "tab completion must read custom names file"
    );
}

#[test]
fn lean_custom_slash_shell_cmd_via_fifo() {
    let home = temp_config_home();
    let cmds = home.join("aishe").join("commands");
    std::fs::create_dir_all(&cmds).unwrap();
    std::fs::write(
        cmds.join("wave6ping.md"),
        "---\ndescription: Wave6 custom slash smoke\nshell: true\n---\ntrue\n",
    )
    .unwrap();

    std::env::set_var("XDG_CONFIG_HOME", &home);
    std::env::set_var("AISHE_CONFIG_DIR", &home);
    std::env::set_var("HOME", &home);

    let mut config = Config::default();
    let mut provider: Option<Arc<dyn aishe::providers::Provider>> = None;
    let mut executor = Executor::new().expect("executor");
    let mut session = Session::new(true);
    let mut store = None;
    let mut warm = LeanWarm::default();
    let pty = PtyOut::capture();

    let reply = handle_ipc_line(
        &mut config,
        &mut provider,
        &mut executor,
        &mut session,
        &mut store,
        &mut warm,
        &pty,
        "SLASH\task\t/tmp\t/help",
    );
    assert_eq!(reply, "OK");
    let shown = pty.take_capture();
    assert!(
        shown.contains("/mode") && shown.contains("/commands"),
        "help must list lean slashes: {shown:?}"
    );
    assert!(
        shown.contains("/wave6ping") || shown.to_ascii_lowercase().contains("custom"),
        "help must surface custom commands: {shown:?}"
    );

    let _ = pty.take_capture();
    let reply = handle_ipc_line(
        &mut config,
        &mut provider,
        &mut executor,
        &mut session,
        &mut store,
        &mut warm,
        &pty,
        "SLASH\task\t/tmp\t/wave6ping",
    );
    assert!(
        reply.starts_with("RAN") || reply.starts_with("CONFIRM_B64") || reply == "OK",
        "custom shell slash must run or confirm, got {reply}"
    );
}

#[test]
fn doctor_mentions_grok_auth() {
    let home = temp_root("doctor");
    std::fs::create_dir_all(home.join("aishe")).unwrap();
    std::fs::write(
        home.join("aishe").join("config.toml"),
        "[aishe]\nmode = \"ask\"\n",
    )
    .unwrap();
    let out = bin()
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("AISHE_CONFIG_DIR", &home)
        .args(["doctor"])
        .output()
        .expect("doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("lean.grok_auth"),
        "doctor must report lean.grok_auth: {stdout}"
    );
    assert!(
        stdout.contains("grok") && stdout.contains("login"),
        "doctor must point at grok login when missing: {stdout}"
    );
}
