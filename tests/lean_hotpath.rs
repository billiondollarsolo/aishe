//! Lean hot-path contracts: known commands never construct a provider or start
//! OpenCode; NL uses the in-process fake/HTTP client; isolated zsh -f hook.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use assert_cmd::Command as CargoCommand;
use predicates::str::contains;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-{label}-{}-{}",
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
fn known_command_dash_c_never_constructs_provider_or_opencode() {
    let root = temp_root("spy-shell");
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
        "known-command path constructed a Provider"
    );
    assert!(
        !opencode_spy.exists(),
        "known-command path started OpenCode"
    );
}

#[test]
fn printf_known_command_never_constructs_provider_or_opencode() {
    let root = temp_root("spy-printf");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .args(["-c", "printf hi"])
        .assert()
        .success()
        .stdout(contains("hi"));
    assert!(!provider_spy.exists());
    assert!(!opencode_spy.exists());
}

#[test]
fn lean_nl_uses_in_process_provider_and_skips_opencode() {
    let root = temp_root("spy-nl");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    let wire_spy = root.join("wire");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .env("AISHE_SPY_WIRE_NS", &wire_spy)
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","command":null,"explanation":"lean-nl-ok"}"#,
        )
        .args(["-c", "? what is the lean path"])
        .assert()
        .success()
        .stdout(contains("lean-nl-ok"));
    assert!(
        provider_spy.exists(),
        "NL path should construct the in-process provider"
    );
    assert!(
        !opencode_spy.exists(),
        "lean NL must not call backend::supervisor / OpenCode"
    );
    assert!(
        wire_spy.exists(),
        "fake provider should still mark the NL-ready instant"
    );
}

#[test]
fn forced_shell_bang_never_hits_provider() {
    let root = temp_root("spy-bang");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    bin()
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .args(["-c", "!true"])
        .assert()
        .success();
    assert!(!provider_spy.exists());
    assert!(!opencode_spy.exists());
}

#[test]
fn lean_hook_classifier_matches_router_table() {
    let hook = aishe::lean::wrapper_zshrc();
    let dir = temp_root("hook");
    let path = dir.join("hook.zsh");
    std::fs::write(&path, &hook).unwrap();
    let script = format!(
        r#"
source {path}
fail=0
expect_shell() {{
  if _aishe_routes_to_agent "$1"; then
    print -r -- "expected shell: $1"
    fail=1
  fi
}}
expect_agent() {{
  if ! _aishe_routes_to_agent "$1"; then
    print -r -- "expected agent: $1"
    fail=1
  fi
}}
expect_shell 'printf hi'
expect_shell 'ls'
expect_shell '!unknown-but-forced'
expect_agent '? printf hi'
expect_agent 'what is the capital of France'
expect_agent 'please list large files'
exit $fail
"#,
        path = path.display()
    );
    let status = Command::new("zsh")
        .args(["-f", "-c", &script])
        .status()
        .expect("zsh");
    assert!(
        status.success(),
        "lean zsh classifier disagreed with the router table"
    );
}

#[test]
fn isolated_zdotdir_does_not_source_user_zshrc() {
    let home = temp_root("poison-home");
    std::fs::write(home.join(".zshrc"), "echo SOURCED_USER_ZSHRC\n").unwrap();
    std::fs::write(home.join(".zshenv"), "echo SOURCED_USER_ZSHENV\n").unwrap();
    let zdot = temp_root("zdot");
    std::fs::write(zdot.join(".zshenv"), aishe::lean::wrapper_zshenv()).unwrap();
    std::fs::write(zdot.join(".zshrc"), "echo LEAN_RC\n").unwrap();
    let output = Command::new("zsh")
        .args(aishe::lean::zsh_argv())
        .arg("-c")
        .arg("echo AFTER")
        .env("HOME", &home)
        .env("ZDOTDIR", &zdot)
        .output()
        .expect("zsh");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // `-c` is non-interactive so `.zshrc` is skipped; `.zshenv` still runs and
    // must be ours, not `$HOME/.zshenv`.
    assert!(
        !combined.contains("SOURCED_USER_ZSHRC"),
        "user .zshrc leaked into zsh -f: {combined}"
    );
    assert!(
        !combined.contains("SOURCED_USER_ZSHENV"),
        "user .zshenv leaked into isolated ZDOTDIR: {combined}"
    );
}

#[test]
fn binary_version_and_known_cmd_are_in_the_same_process_budget_band() {
    // Debug binaries miss the release 5ms/2ms budgets. Record numbers and keep
    // a loose ceiling so a hang still fails CI. Release numbers live in
    // docs/lean-hotpath.md.
    let mut samples = Vec::new();
    for _ in 0..8 {
        let start = Instant::now();
        bin().arg("--version").assert().success();
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = samples[(samples.len() * 95 / 100).min(samples.len() - 1)];
    assert!(
        p95 < 500.0,
        "--version debug p95 {p95:.2}ms is a hang, not a slow debug binary"
    );

    let mut cmd_samples = Vec::new();
    for _ in 0..8 {
        let start = Instant::now();
        bin().args(["-c", "true"]).assert().success();
        cmd_samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    cmd_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let cmd_p95 = cmd_samples[(cmd_samples.len() * 95 / 100).min(cmd_samples.len() - 1)];
    assert!(
        cmd_p95 < 800.0,
        "-c true debug p95 {cmd_p95:.2}ms is a hang, not a slow debug binary"
    );
    eprintln!(
        "lean debug benches: --version p95={p95:.2}ms  -c true p95={cmd_p95:.2}ms (see docs/lean-hotpath.md for release)"
    );
}

#[test]
fn main_resolves_fast_shell_before_provider_construction() {
    let main =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
            .unwrap();
    let fast = main
        .find("dispatcher::fast_shell_line")
        .expect("fast_shell_line admission");
    let provider = main.find("providers::make").expect("providers::make");
    assert!(
        fast < provider,
        "known-command -c path must not reach providers::make"
    );
}
