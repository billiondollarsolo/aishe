//! Focused acceptance fixtures for agent activity and streaming UX contracts.
//!
//! The renderer child-process pattern observes real stdout bytes without adding
//! a production capture API. The child uses only public backend-neutral events.

use std::io::Write;
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU64, Ordering};

use aishe::agent::renderer::AgentRenderer;
use aishe::agent::{
    AgentEvent, DiffView, ToolCallView, ToolResultView, UserFacingError, UserQuestion,
};
use assert_cmd::Command;

const FIXTURE_ENV: &str = "AISHE_AGENT_RENDERER_FIXTURE";
const BEGIN: &str = "__AISHE_RENDER_BEGIN__";
const END: &str = "__AISHE_RENDER_END__";

fn call(id: &str, name: &str, arguments: serde_json::Value) -> ToolCallView {
    ToolCallView {
        call_id: id.into(),
        name: name.into(),
        arguments,
        title: String::new(),
    }
}

/// Child target invoked by `capture_renderer`. It is otherwise an inert test.
#[test]
fn renderer_fixture_child() {
    let Ok(scenario) = std::env::var(FIXTURE_ENV) else {
        return;
    };
    println!("{BEGIN}");
    let mut renderer = AgentRenderer::new(if scenario.ends_with("_detailed") {
        "detailed"
    } else {
        "focus"
    });
    match scenario.as_str() {
        "short" => {
            renderer.render(&AgentEvent::TextDelta {
                text: "short answer".into(),
            });
            renderer.render(&AgentEvent::TextCompleted {
                text: "short answer".into(),
            });
            renderer.render(&AgentEvent::Completed {
                summary: "short answer".into(),
            });
        }
        "long" => {
            let body = (0..300)
                .map(|index| format!("long line {index}\n"))
                .collect::<String>();
            for chunk in body.as_bytes().chunks(137) {
                renderer.render(&AgentEvent::TextDelta {
                    text: String::from_utf8(chunk.to_vec()).unwrap(),
                });
            }
            renderer.render(&AgentEvent::TextCompleted { text: body.clone() });
            renderer.render(&AgentEvent::Completed { summary: body });
        }
        "recovered" | "recovered_detailed" => {
            renderer.render(&AgentEvent::ToolStarted {
                call: call(
                    "failed",
                    "aishe_run_command",
                    serde_json::json!({"command":"false"}),
                ),
            });
            renderer.render(&AgentEvent::ToolFailed {
                call_id: "failed".into(),
                error: UserFacingError {
                    code: "tool.exit".into(),
                    message: "first attempt failed".into(),
                    retryable: true,
                },
            });
            renderer.render(&AgentEvent::ToolStarted {
                call: call(
                    "ok",
                    "aishe_write_file",
                    serde_json::json!({"path":"src/fixed.rs"}),
                ),
            });
            renderer.render(&AgentEvent::ToolCompleted {
                call_id: "ok".into(),
                result: ToolResultView {
                    success: true,
                    exit_code: None,
                    output: "written".into(),
                    metadata: serde_json::Value::Null,
                },
            });
            renderer.render(&AgentEvent::Diff {
                diff: DiffView {
                    path: "src/fixed.rs".into(),
                    patch: "+fixed".into(),
                },
            });
            renderer.render(&AgentEvent::Reconnecting { attempt: 1 });
            renderer.render(&AgentEvent::Reconciled);
            renderer.render(&AgentEvent::TextCompleted {
                text: "Recovered and completed.".into(),
            });
            renderer.render(&AgentEvent::Completed {
                summary: "Recovered and completed.".into(),
            });
        }
        "terminal" => {
            renderer.render(&AgentEvent::ToolStarted {
                call: call(
                    "failed",
                    "aishe_run_command",
                    serde_json::json!({"command":"false"}),
                ),
            });
            renderer.render(&AgentEvent::ToolFailed {
                call_id: "failed".into(),
                error: UserFacingError {
                    code: "tool.exit".into(),
                    message: "attempt failed".into(),
                    retryable: false,
                },
            });
            renderer.render(&AgentEvent::Diff {
                diff: DiffView {
                    path: "src/partial.rs".into(),
                    patch: "+partial".into(),
                },
            });
            renderer.render(&AgentEvent::Failed {
                error: UserFacingError {
                    code: "agent.terminal".into(),
                    message: "terminal failure".into(),
                    retryable: false,
                },
            });
        }
        "questions" => {
            let pause = std::env::var("AISHE_AGENT_UI_PAUSE_MS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0)
                .min(2_000);
            for index in 0..2 {
                renderer.render(&AgentEvent::WaitingForUser {
                    request: UserQuestion {
                        request_id: format!("question-{index}"),
                        prompt: format!(
                            "Question {index}: {}",
                            "choose a safe deployment region ".repeat(180)
                        ),
                        agent: Some("planner".into()),
                        task: Some("task-acceptance".into()),
                    },
                });
                renderer.render(&AgentEvent::Reconnecting { attempt: index + 1 });
                renderer.render(&AgentEvent::Reconciled);
                if index == 0 && pause > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(pause));
                }
            }
        }
        other => panic!("unknown fixture scenario {other}"),
    }
    println!("{END}");
}

fn capture_renderer(scenario: &str) -> String {
    let output = StdCommand::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "renderer_fixture_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(FIXTURE_ENV, scenario)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "renderer fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let body = stdout
        .split_once(BEGIN)
        .unwrap()
        .1
        .split_once(END)
        .unwrap()
        .0;
    body.trim_matches('\n').to_string()
}

#[test]
fn no_color_short_and_long_answers_have_one_boundary_and_no_duplicates() {
    let short = capture_renderer("short");
    let long = capture_renderer("long");
    for rendered in [&short, &long] {
        assert!(!rendered.contains('\u{1b}'));
        assert_eq!(rendered.matches("AIShe · answer").count(), 1);
    }
    assert_eq!(short.matches("short answer").count(), 1);
    assert_eq!(long.matches("long line 0").count(), 1);
    assert_eq!(long.matches("long line 299").count(), 1);
}

#[test]
fn recovered_and_terminal_failures_have_distinct_truthful_final_outcomes() {
    let recovered = capture_renderer("recovered");
    assert!(recovered.contains("1 recovered attempt"));
    assert!(recovered.contains("1 file changed"));
    assert!(recovered.contains("files: src/fixed.rs"));
    assert!(recovered.contains("1 reconnect"));
    assert!(!recovered.contains("AIShe error"));

    let recovered_detailed = capture_renderer("recovered_detailed");
    assert!(recovered_detailed.contains("1 recovered attempt"));
    assert!(recovered_detailed.contains("first attempt failed"));
    assert!(!recovered_detailed.contains("AIShe error"));

    let terminal = capture_renderer("terminal");
    assert!(terminal.contains("1 failed attempt"));
    assert!(!terminal.contains("recovered attempt"));
    assert!(terminal.contains("files: src/partial.rs"));
    assert!(terminal.contains("AIShe error: terminal failure"));
}

#[test]
fn long_multiple_questions_and_reconnects_are_bounded_and_plain() {
    let rendered = capture_renderer("questions");
    assert!(!rendered.contains('\u{1b}'));
    assert_eq!(rendered.matches("waiting for you: agent").count(), 2);
    assert!(rendered.contains("agent    planner"));
    assert!(rendered.contains("task     task-acceptance"));
    assert!(rendered.contains("question-0"));
    assert!(rendered.contains("question-1"));
    assert!(rendered.len() < 16_000);
}

fn temp_config_home() -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let root = std::env::temp_dir().join(format!(
        "aishe-agent-ui-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let config = root.join("aishe");
    std::fs::create_dir_all(&config).unwrap();
    let mut file = std::fs::File::create(config.join("config.toml")).unwrap();
    writeln!(
        file,
        r#"version = 7

[aishe]
mode = "suggest"
provider = "anthropic"

[backend]
engine = "native"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "test-model"
"#
    )
    .unwrap();
    root
}

#[test]
fn piped_answer_emits_exactly_one_plain_body() {
    let config = temp_config_home();
    let response = r#"{"type":"answer","command":null,"explanation":"PIPE_SINGLE_BODY_42"}"#;
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &config)
        .env("AISHE_DATA_DIR", config.join("data"))
        .env("XDG_CONFIG_HOME", &config)
        .env("XDG_DATA_HOME", config.join("data"))
        .env("AISHE_FAKE_LLM", response)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .write_stdin("what is the acceptance marker?\n")
        .assert()
        .success()
        .stdout("PIPE_SINGLE_BODY_42\n")
        .stderr("");
    std::fs::remove_dir_all(config).ok();
}
