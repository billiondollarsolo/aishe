//! Live Grok subscription smoke for lean NL. Gated behind AISHE_LIVE_LLM=1.
//!
//! Not run in default CI. Requires a Grok Build CLI login (`~/.grok/auth.json`)
//! and network access to api.x.ai — not an API key.
//!
//!   AISHE_LIVE_LLM=1 cargo test --test lean_live_xai -- --nocapture
//!
//! Optional: AISHE_GROK_AUTH=/path/to/auth.json

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use assert_cmd::Command as CargoCommand;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-live-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lean_config_home() -> PathBuf {
    let dir = temp_root("config");
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut file = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    // Minimal config — prefer_grok_live_auth rewrites the xAI connection at startup.
    writeln!(
        file,
        r#"[aishe]
mode = "suggest"
stream = false

[backend]
engine = "opencode"
"#
    )
    .unwrap();
    dir
}

fn live_enabled() -> bool {
    matches!(
        std::env::var("AISHE_LIVE_LLM")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn grok_auth_present() -> bool {
    if let Ok(p) = std::env::var("AISHE_GROK_AUTH") {
        let p = p.trim();
        if !p.is_empty() {
            return std::path::Path::new(p).is_file();
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return std::path::Path::new(home).join("auth.json").is_file();
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return std::path::Path::new(&home)
            .join(".grok")
            .join("auth.json")
            .is_file();
    }
    false
}

#[test]
fn live_grok_subscription_lean_nl_smoke() {
    if !live_enabled() {
        eprintln!("skip: set AISHE_LIVE_LLM=1 to run live Grok subscription smoke");
        return;
    }
    assert!(
        grok_auth_present(),
        "AISHE_LIVE_LLM=1 requires a Grok Build CLI session (~/.grok/auth.json). \
         Log in with `grok` / device login on this host. Do not use XAI_API_KEY for the happy path."
    );

    let started = Instant::now();
    let mut cmd = CargoCommand::cargo_bin("aishe").unwrap();
    let config_home = lean_config_home();
    let data_home = temp_root("data");
    cmd.env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("AISHE_CONFIG_DIR", &config_home)
        .env("AISHE_DATA_DIR", &data_home)
        .env_remove("XAI_API_KEY")
        .env_remove("AISHE_FAKE_LLM")
        .env_remove("AISHE_FAKE_LLM_FILE")
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1")
        .args(["-c", "? Reply with exactly one word: pong. No punctuation."]);
    let output = cmd.output().expect("spawn aishe");
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("live Grok subscription elapsed={elapsed:?}");
    eprintln!("stdout:\n{stdout}");
    if !stderr.is_empty() {
        eprintln!("stderr:\n{stderr}");
    }
    assert!(
        output.status.success(),
        "aishe lean NL failed status={:?} stderr={stderr}",
        output.status
    );
    let lower = stdout.to_ascii_lowercase();
    assert!(
        lower.contains("pong"),
        "expected model answer to contain 'pong', got: {stdout}"
    );
    eprintln!("live smoke ok in {elapsed:?}");
}
