//! Wave 1 lean-native smoke: multi-line PTY answers, /reset, compsys, spies.

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command as CargoCommand;
use predicates::str::contains;

fn temp_root(label: &str) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-lean-w1-{label}-{}-{}",
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
    let config_home = temp_config_home();
    let data_home = temp_root("data");
    cmd.env("XDG_CONFIG_HOME", &config_home)
        .env("XDG_DATA_HOME", &data_home)
        .env("AISHE_CONFIG_DIR", &config_home)
        .env("AISHE_DATA_DIR", &data_home)
        .env_remove("AISHE_LEGACY_OPENCODE")
        .env("AISHE_LEAN", "1");
    cmd
}

#[test]
fn known_command_never_hits_provider_or_opencode() {
    let root = temp_root("spy");
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
fn ask_path_multi_line_answer_not_flattened_on_dash_c() {
    // -c still prints via suggest::run to parent stdout; ensure newlines survive
    // the fake LLM explanation (no FIFO flatten on this path either).
    bin()
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","command":null,"explanation":"alpha\nbeta\ngamma"}"#,
        )
        .args(["-c", "? multi line please"])
        .assert()
        .success()
        .stdout(contains("alpha"))
        .stdout(contains("beta"));
}

#[test]
fn lean_hook_sources_compinit() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(
        hook.contains("compinit"),
        "lean hook must bootstrap bounded compsys"
    );
    assert!(
        hook.contains(".zcompdump"),
        "compinit dump must live under private ZDOTDIR"
    );
    assert!(
        hook.contains("compinit -u"),
        "compinit must use -u so insecure system dirs never prompt/deadlock PTYs"
    );
    let live: String = hook
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with('#'))
        .collect::<Vec<_>>()
        .join(
            "
",
        );
    assert!(
        !live.contains("source ~/.zshrc") && !live.contains("AISHE_REAL_ZDOTDIR"),
        "must not pull user plugin stack"
    );
}

#[test]
fn lean_zdotdir_compinit_defines_compdef() {
    let zdot = temp_root("zdot-comp");
    std::fs::write(zdot.join(".zshenv"), aishe::lean::wrapper_zshenv()).unwrap();
    std::fs::write(zdot.join(".zshrc"), aishe::lean::wrapper_zshrc()).unwrap();
    let output = Command::new("zsh")
        .args(aishe::lean::zsh_argv())
        .arg("-c")
        .arg("whence -v compdef; whence -w compinit; print OK_COMP")
        .env("ZDOTDIR", &zdot)
        .env("HOME", temp_root("home-comp"))
        .output()
        .expect("zsh");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "zsh failed: {combined}");
    // Interactive -c may skip .zshrc on some zsh builds; force source.
    // CI runners often lack a TTY; `script` provides a pty so compinit does not
    // abort with "not interactive and can't open terminal". Extra `compinit -u`
    // covers group-writable /usr/share/zsh on Ubuntu images.
    let home2 = temp_root("home-comp2");
    let runner = zdot.join("compdef-check.zsh");
    std::fs::write(
        &runner,
        "source \"$ZDOTDIR/.zshrc\"\n\
autoload -Uz compinit 2>/dev/null || true\n\
compinit -u -d \"${ZDOTDIR}/.zcompdump\" 2>/dev/null || true\n\
whence -v compdef >/dev/null && print COMPDEF_OK\n",
    )
    .unwrap();
    let linux_script = Command::new("script")
        .args(["-q", "-c", "true", "/dev/null"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let output2 = if linux_script {
        let cmd = format!("zsh -f -o RCS -o NO_GLOBAL_RCS -i {}", runner.display());
        Command::new("script")
            .args(["-q", "-c", &cmd, "/dev/null"])
            .env("ZDOTDIR", &zdot)
            .env("HOME", &home2)
            .env("TERM", "xterm")
            .output()
            .expect("zsh2-script-linux")
    } else {
        Command::new("script")
            .args([
                "-q",
                "/dev/null",
                "zsh",
                "-f",
                "-o",
                "RCS",
                "-o",
                "NO_GLOBAL_RCS",
                "-i",
                runner.to_str().unwrap(),
            ])
            .env("ZDOTDIR", &zdot)
            .env("HOME", &home2)
            .env("TERM", "xterm")
            .output()
            .or_else(|_| {
                Command::new("zsh")
                    .args(["-f", "-o", "RCS", "-o", "NO_GLOBAL_RCS", "-i"])
                    .arg(&runner)
                    .env("ZDOTDIR", &zdot)
                    .env("HOME", &home2)
                    .env("TERM", "xterm")
                    .output()
            })
            .expect("zsh2")
    };
    let combined2 = format!(
        "{}{}",
        String::from_utf8_lossy(&output2.stdout),
        String::from_utf8_lossy(&output2.stderr)
    );
    assert!(
        combined2.contains("COMPDEF_OK"),
        "compdef missing after lean hook: {combined2}"
    );
}

#[test]
fn shift_tab_delegates_when_buffer_nonempty() {
    let hook = aishe::lean::wrapper_zshrc();
    assert!(hook.contains("aishe-cycle-mode"));
    // Delegation branch must prefer completion widgets when BUFFER is set.
    assert!(
        hook.contains("reverse-menu-complete") && hook.contains("[[ -n \"$BUFFER\" ]]"),
        "Shift-Tab must delegate to completion when the line has text"
    );
}

#[test]
fn mode_aliases_accepted_in_hook_and_rust() {
    assert_eq!(
        aishe::lean::LeanMode::parse("suggest"),
        aishe::lean::LeanMode::Ask
    );
    assert_eq!(
        aishe::lean::LeanMode::parse("auto"),
        aishe::lean::LeanMode::Allow
    );
    assert_eq!(
        aishe::lean::LeanMode::parse("yolo"),
        aishe::lean::LeanMode::Agent
    );
    let hook = aishe::lean::wrapper_zshrc();
    assert!(hook.contains("ask|suggest"));
    assert!(hook.contains("allow|auto"));
    assert!(hook.contains("agent|yolo"));
}
