//! Live xAI / Grok smoke for lean NL. Gated behind AISHE_LIVE_LLM=1.
//!
//! Not run in default CI. Requires XAI_API_KEY and network access to api.x.ai.
//!
//!   AISHE_LIVE_LLM=1 XAI_API_KEY=... cargo test --test lean_live_xai -- --nocapture

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

fn xai_config_home() -> PathBuf {
    let dir = temp_root("config");
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut file = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    // Catalog-aligned Grok - API connection (provider_catalog::xai).
    writeln!(
        file,
        r#"[aishe]
mode = "suggest"
provider = "xai"
connection = "xai"
stream = false

[backend]
engine = "opencode"

[providers.openai]
base_url = "https://api.x.ai"
api_key_env = "XAI_API_KEY"
model = "grok-4.5"
credential = "xai"
transport = "responses"
auth_required = true

[connections.xai]
provider = "xai"
label = "Grok - API"

[connections.xai.settings]
base_url = "https://api.x.ai"
api_key_env = "XAI_API_KEY"
model = "grok-4.5"
credential = "xai"
transport = "responses"
auth_required = true

[connections.xai.auth]
type = "api_key"
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

#[test]
fn live_xai_lean_nl_smoke() {
    if !live_enabled() {
        eprintln!("skip: set AISHE_LIVE_LLM=1 to run live xAI smoke");
        return;
    }
    let key = std::env::var("XAI_API_KEY").unwrap_or_default();
    assert!(
        !key.trim().is_empty(),
        "AISHE_LIVE_LLM=1 requires XAI_API_KEY (Grok API key for https://api.x.ai). \
         Grok Build CLI OAuth is not the same credential."
    );

    let started = Instant::now();
    let mut cmd = CargoCommand::cargo_bin("aishe").unwrap();
    cmd.env("XDG_CONFIG_HOME", xai_config_home())
        .env("XDG_DATA_HOME", temp_root("data"))
        .env("XAI_API_KEY", &key)
        .env_remove("AISHE_FAKE_LLM")
        .env_remove("AISHE_FAKE_LLM_FILE")
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1")
        .args([
            "-c",
            "? Reply with exactly one word: pong. No punctuation.",
        ]);
    let output = cmd.output().expect("spawn aishe");
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eprintln!("live xAI elapsed={elapsed:?}");
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
    // Soft latency note — network-bound; not a CI gate.
    eprintln!("live smoke ok in {elapsed:?}");
}
