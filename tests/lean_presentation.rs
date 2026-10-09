//! Observe the actual native agent transcript and shell highlight contract.

use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use aishe::config::Config;
use aishe::executor::Executor;
use aishe::mcp::McpRegistry;
use aishe::providers::fake::FakeProvider;
use aishe::session::Session;
use aishe::skills::SkillRegistry;
use aishe::ui::{CapabilityInputs, TerminalCapabilities};

struct Temp(std::path::PathBuf);
impl Temp {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "aishe-presentation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("isolated presentation directory");
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn native_density_fixture() {
    let Ok(density) = std::env::var("AISHE_DENSITY_FIXTURE") else {
        return;
    };
    let mut config = Config::default();
    config.backend.output = density;
    config.aishe.yolo_confirm = "never".into();
    config.aishe.yolo_confirm_dangerous = false;
    config.aishe.show_usage = false;
    config.aishe.stream = false;
    config.sandbox.linux_backend = "off".into();
    let provider = FakeProvider::new("DENSITY_FINAL".into());
    let mut executor = Executor::new().expect("test executor");
    println!("DENSITY_BEGIN");
    if std::env::var_os("AISHE_DIFF_FIXTURE").is_some() {
        std::fs::write("edit.txt", "OLD_DIFF_LINE\n").expect("fixture file");
        let (_, result) = aishe::tools::execute_rendered(
            "edit_file",
            &serde_json::json!({"path":"edit.txt", "find":"OLD_DIFF_LINE", "replace":"NEW_DIFF_LINE"}),
            executor.cwd(),
            false,
            false,
            config.backend.output == "detailed",
        );
        assert!(result.starts_with("Replaced "), "{result}");
        assert_eq!(
            std::fs::read_to_string("edit.txt").expect("edited fixture"),
            "NEW_DIFF_LINE\n"
        );
        println!("DENSITY_END");
        return;
    }
    aishe::modes::yolo::run(
        "show the fixture output",
        &provider,
        &mut executor,
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
        &mut Session::new(false),
    )
    .expect("fixture agent turn");
    println!("DENSITY_END");
}

fn transcript(density: &str) -> String {
    capture(density, false)
}

fn capture(density: &str, diff: bool) -> String {
    let temp = Temp::new();
    let mut command = Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", "native_density_fixture", "--nocapture"])
        .env("AISHE_DENSITY_FIXTURE", density)
        .env("AISHE_FAKE_TOOL", "printf 'DENSITY_RAW\\n'")
        .env("AISHE_UNICODE", "ascii")
        .env("NO_COLOR", "1")
        .env("AISHE_DATA_DIR", temp.path())
        .env("XDG_DATA_HOME", temp.path())
        .env_remove("AISHE_FAKE_LLM_FILE")
        .current_dir(temp.path());
    if diff {
        command.env("AISHE_DIFF_FIXTURE", "1");
    }
    let output = command.output().expect("capture agent fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).expect("UTF-8 transcript");
    raw.split_once("DENSITY_BEGIN")
        .expect("begin marker")
        .1
        .split_once("DENSITY_END")
        .expect("end marker")
        .0
        .to_string()
}

#[test]
fn completed_edit_diffs_follow_output_density() {
    for density in ["focus", "compact"] {
        let output = capture(density, true);
        assert!(
            !output.contains("OLD_DIFF_LINE") && !output.contains("NEW_DIFF_LINE"),
            "{output}"
        );
    }
    let detailed = capture("detailed", true);
    assert!(detailed.contains("- OLD_DIFF_LINE"), "{detailed}");
    assert!(detailed.contains("+ NEW_DIFF_LINE"), "{detailed}");
    assert!(!detailed.contains('\u{1b}'), "{detailed}");
}

#[test]
fn native_agent_densities_change_the_rendered_transcript() {
    let focus = transcript("focus");
    let compact = transcript("compact");
    let detailed = transcript("detailed");
    for output in [&focus, &compact, &detailed] {
        assert_eq!(output.matches("DENSITY_FINAL").count(), 1, "{output}");
        assert!(
            !output.contains('\u{1b}'),
            "plain transcript leaked ANSI: {output}"
        );
    }
    assert!(focus.contains("attempted commands:"), "{focus}");
    assert!(focus.contains("1 action"), "{focus}");
    assert!(
        !focus.contains("exit 0"),
        "focus persisted an action row: {focus}"
    );
    assert!(!compact.contains("commands:"), "{compact}");
    assert!(compact.contains("OK run command"), "{compact}");
    assert!(compact.contains("exit 0"), "{compact}");
    assert!(
        detailed.lines().any(|line| line == "DENSITY_RAW"),
        "{detailed}"
    );
    assert!(!focus.lines().any(|line| line == "DENSITY_RAW"), "{focus}");
    assert!(
        !compact.lines().any(|line| line == "DENSITY_RAW"),
        "{compact}"
    );
}

#[test]
fn shell_highlight_policy_preserves_unrelated_regions_and_disables_colors() {
    let temp = Temp::new();
    let hook = temp.path().join("hook.zsh");
    std::fs::write(&hook, aishe::lean::wrapper_zshrc()).expect("write hook");
    for (theme, no_color, term) in [
        ("dark", false, "xterm-256color"),
        ("mono", false, "xterm-256color"),
        ("dark", true, "xterm-256color"),
        ("dark", false, "dumb"),
    ] {
        let caps = TerminalCapabilities::resolve(&CapabilityInputs {
            is_tty: true,
            theme: Some(theme.into()),
            no_color,
            term: Some(term.into()),
            locale: Some("C.UTF-8".into()),
            ..CapabilityInputs::default()
        });
        let mut command = Command::new("zsh");
        command
            .args([
                "-fc",
                r#"source "$1"
BUFFER='echo hello'
region_highlight=('0 1 underline,memo=plugin')
_aishe_highlight_command
print -r -- "${(j:|:)region_highlight}"
BUFFER='please explain this'
_aishe_highlight_command
print -r -- "${(j:|:)region_highlight}"
"#,
                "fixture",
            ])
            .arg(&hook)
            .env("TERM", term)
            .env("AISHE_MODE", "ask")
            .env("AISHE_STYLE", if caps.styled() { "on" } else { "none" })
            .env_remove("NO_COLOR");
        if no_color {
            command.env("NO_COLOR", "1");
        }
        for (key, value) in aishe::ui::zsh_color_map(&caps) {
            command.env(key, value);
        }
        let output = command.output().expect("run zsh highlight policy");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).expect("UTF-8 highlight policy");
        assert_eq!(text.matches("memo=plugin").count(), 2, "{text}");
        if caps.styled() {
            assert_eq!(text.matches("memo=aishe").count(), 2, "{text}");
            if theme == "mono" {
                assert!(!text.contains("fg="), "{text}");
            }
        } else {
            assert!(!text.contains("memo=aishe"), "{text}");
        }
    }
}
