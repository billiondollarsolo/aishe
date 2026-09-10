//! Wave 5 lean-native smoke: F01 live PTY known-cmd latency, F34 details/Ctrl-O,
//! F16/F17 /mcp /skills name lists.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use assert_cmd::Command as CargoCommand;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w5-{label}-{}-{}",
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
    std::fs::write(
        cfg_dir.join("config.toml"),
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
"#,
    )
    .unwrap();
    dir
}

fn bin_path() -> PathBuf {
    assert_cmd::cargo::cargo_bin("aishe")
}

fn percentile(sorted_ms: &[f64], pct: usize) -> f64 {
    if sorted_ms.is_empty() {
        return f64::NAN;
    }
    let idx = (sorted_ms.len() * pct / 100).min(sorted_ms.len() - 1);
    sorted_ms[idx]
}

fn buffer_contains(buf: &Arc<Mutex<Vec<u8>>>, needle: &str) -> bool {
    let guard = buf.lock().unwrap_or_else(|e| e.into_inner());
    String::from_utf8_lossy(&guard).contains(needle)
}

fn wait_for(buf: &Arc<Mutex<Vec<u8>>>, needle: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if buffer_contains(buf, needle) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    buffer_contains(buf, needle)
}

fn snapshot(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let guard = buf.lock().unwrap_or_else(|e| e.into_inner());
    String::from_utf8_lossy(&guard).into_owned()
}

#[test]
fn lean_hook_binds_ctrl_o_and_routes_mcp_skills() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("aishe-toggle-agent-details"),
        "lean hook must define Ctrl-O details toggle"
    );
    assert!(
        hook.contains("AISHE_DETAILS_KEY") || hook.contains("^O"),
        "lean hook must bind Ctrl-O / AISHE_DETAILS_KEY"
    );
    assert!(hook.contains("/mcp"), "lean hook must route /mcp");
    assert!(hook.contains("/skills"), "lean hook must route /skills");
    assert!(
        hook.contains("Ctrl-O") || hook.contains("/details"),
        "lean /help should mention density toggle"
    );
}

#[test]
fn lean_hotpath_doc_marks_f01_live_pty_and_f34() {
    let doc = include_str!("../docs/lean-hotpath.md");
    assert!(
        doc.contains("Live PTY") || doc.contains("live PTY"),
        "lean-hotpath.md must document live PTY known-cmd latency"
    );
    assert!(
        doc.contains("p50") && doc.contains("p95"),
        "must keep p50/p95 tables"
    );
    assert!(
        doc.contains("/details") && doc.contains("Ctrl-O"),
        "must document F34 density toggle"
    );
    assert!(
        doc.contains("/mcp") && doc.contains("/skills"),
        "must document F16/F17 name lists"
    );
}

/// Hang-detection smoke: spawn lean interactive PTY, time known-cmd roundtrips.
/// Release budgets live in docs/lean-hotpath.md; debug builds only fail on hangs.
///
/// Critical: every wait is bounded and the child is killed on the overall
/// ceiling. macOS CI previously deadlocked here when `compinit` prompted about
/// insecure dirs inside a nested PTY (outer write blocked → cargo test hung
/// until the job cancelled ~15m later).
#[test]
fn live_pty_known_cmd_roundtrip_hang_ceiling() {
    let overall = if std::env::var_os("CI").is_some() {
        Duration::from_secs(45)
    } else {
        Duration::from_secs(90)
    };
    let test_started = Instant::now();

    let config_home = temp_config_home();
    let data_home = temp_root("data");
    let hist = temp_root("hist").join("histfile");
    let _ = std::fs::write(&hist, "");

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new(bin_path());
    cmd.env("XDG_CONFIG_HOME", config_home.as_os_str());
    cmd.env("XDG_DATA_HOME", data_home.as_os_str());
    cmd.env("AISHE_CONFIG_DIR", config_home.as_os_str());
    cmd.env("AISHE_DATA_DIR", data_home.as_os_str());
    cmd.env("HOME", temp_root("home").as_os_str());
    cmd.env("AISHE_LEAN", "1");
    cmd.env("AISHE_UNICODE", "ascii");
    cmd.env("AISHE_HISTFILE", hist.as_os_str());
    cmd.env("TERM", "xterm-256color");
    cmd.env("NO_COLOR", "1");
    cmd.env_remove("AISHE_LEGACY_OPENCODE");
    cmd.env(
        "AISHE_FAKE_LLM",
        r#"{"type":"answer","command":null,"explanation":"ok"}"#,
    );

    let mut child = pair.slave.spawn_command(cmd).expect("spawn aishe");
    drop(pair.slave);
    let mut killer = child.clone_killer();
    let mut watchdog_killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let mut writer = pair.master.take_writer().expect("take writer");
    let collected = Arc::new(Mutex::new(Vec::<u8>::new()));
    let done = Arc::new(AtomicBool::new(false));
    {
        let collected = Arc::clone(&collected);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while !done.load(Ordering::Relaxed) {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => collected
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .extend_from_slice(&buffer[..n]),
                    Err(_) => break,
                }
            }
        });
    }

    // Watchdog: if the nested PTY deadlocks (e.g. interactive compinit prompt),
    // kill the child so this test fails fast instead of hanging the macOS job.
    {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            while test_started.elapsed() < overall {
                if done.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = watchdog_killer.kill();
        });
    }

    let remaining = |started: Instant, budget: Duration| -> Duration {
        budget.saturating_sub(started.elapsed())
    };

    assert!(
        wait_for(
            &collected,
            "ask",
            remaining(test_started, overall).min(Duration::from_secs(25))
        ),
        "lean PTY never showed ask prompt within budget: {:?}",
        snapshot(&collected)
    );
    assert!(
        test_started.elapsed() < overall,
        "live PTY hung before roundtrips; buf={:?}",
        snapshot(&collected)
    );

    // Settle after first prompt / compsys.
    std::thread::sleep(Duration::from_millis(150));

    let iters = match std::env::var("AISHE_LEAN_PTY_BENCH_N")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        Some(n) if n >= 4 => n,
        _ if std::env::var_os("CI").is_some() => 8,
        _ => 16,
    };
    let mut samples_ms = Vec::new();
    for i in 0..iters {
        assert!(
            test_started.elapsed() < overall,
            "live PTY overall ceiling hit at iter {i}; buf={:?}",
            snapshot(&collected)
        );
        // Wait until a prompt is visible so each sample starts from a settled shell.
        assert!(
            wait_for(
                &collected,
                "ask",
                remaining(test_started, overall).min(Duration::from_secs(5))
            ),
            "prompt lost before iter {i}: {:?}",
            snapshot(&collected)
        );
        std::thread::sleep(Duration::from_millis(5));
        let marker = format!("AISHE_RT_{i}");
        collected.lock().unwrap_or_else(|e| e.into_inner()).clear();
        let cmd_line = format!("printf '%s\n' {marker}\r");
        let start = Instant::now();
        writer.write_all(cmd_line.as_bytes()).expect("write cmd");
        let _ = writer.flush();
        assert!(
            wait_for(
                &collected,
                &marker,
                remaining(test_started, overall).min(Duration::from_secs(5))
            ),
            "marker {marker} not seen; buf={:?}",
            snapshot(&collected)
        );
        samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }

    let _ = writer.write_all(b"exit\r");
    let _ = writer.flush();
    drop(writer); // EOF on PTY master helps the child unwind
    done.store(true, Ordering::Relaxed);

    // Bounded reaping — never call unbounded child.wait() on CI.
    let reap_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < reap_deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = killer.kill();
                let hard = Instant::now() + Duration::from_secs(2);
                while Instant::now() < hard {
                    if matches!(child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                break;
            }
        }
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let steady = if samples_ms.len() > 2 {
        &samples_ms[1..]
    } else {
        &samples_ms[..]
    };
    let p50 = percentile(steady, 50);
    let p95 = percentile(steady, 95);
    let min = steady[0];
    eprintln!(
        "lean live PTY known-cmd roundtrip: min={min:.2}ms p50={p50:.2}ms p95={p95:.2}ms n={} samples={steady:?}",
        steady.len()
    );
    assert!(
        p95 < 2000.0,
        "live PTY known-cmd p95 {p95:.2}ms looks hung (debug hang ceiling 2s)"
    );

    if let Ok(path) = std::env::var("AISHE_LEAN_PTY_BENCH_OUT") {
        if !path.is_empty() {
            let _ = std::fs::write(
                path,
                format!(
                    "min_ms={min:.3}\np50_ms={p50:.3}\np95_ms={p95:.3}\nn={}\n",
                    steady.len()
                ),
            );
        }
    }
}

#[test]
fn known_cmd_still_skips_provider_after_wave5() {
    let root = temp_root("spy");
    let provider_spy = root.join("provider");
    let opencode_spy = root.join("opencode");
    let mut cmd = CargoCommand::cargo_bin("aishe").unwrap();
    let config_home = temp_config_home();
    let data_home = temp_root("data");
    cmd.env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("AISHE_CONFIG_DIR", &config_home)
        .env("AISHE_DATA_DIR", &data_home)
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1")
        .env("AISHE_SPY_PROVIDER_MAKE", &provider_spy)
        .env("AISHE_SPY_OPENCODE", &opencode_spy)
        .args(["-c", "true"])
        .assert()
        .success();
    assert!(!provider_spy.exists());
    assert!(!opencode_spy.exists());
}
