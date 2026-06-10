//! Mode tests driven by a scripted MockProvider (no network, no real LLM).

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use aishe::config::Config;
use aishe::executor::Executor;
use aishe::modes::{suggest, yolo};
use aishe::providers::{Completion, Msg, Provider, ProviderError, ResponseFormat, ToolCall};
use aishe::session::Session;
use aishe::skills::SkillRegistry;
use aishe::usage::UsageMeter;
use serde_json::json;

/// A provider that replays scripted responses. When `repeat_last` is set and
/// only one response remains, it is returned indefinitely.
struct MockProvider {
    completions: Mutex<VecDeque<Completion>>,
    texts: Mutex<VecDeque<String>>,
    repeat_last: bool,
    meter: Arc<UsageMeter>,
}

impl MockProvider {
    fn with_completions(items: Vec<Completion>, repeat_last: bool) -> Self {
        Self {
            completions: Mutex::new(items.into()),
            texts: Mutex::new(VecDeque::new()),
            repeat_last,
            meter: Arc::new(UsageMeter::default()),
        }
    }
    fn with_text(text: &str) -> Self {
        let mut q = VecDeque::new();
        q.push_back(text.to_string());
        Self {
            completions: Mutex::new(VecDeque::new()),
            texts: Mutex::new(q),
            repeat_last: false,
            meter: Arc::new(UsageMeter::default()),
        }
    }
}

impl Provider for MockProvider {
    fn complete(&self, _s: &str, _m: &[Msg], _f: &ResponseFormat) -> Result<String, ProviderError> {
        self.meter.record(10, 5);
        Ok(self.texts.lock().unwrap().pop_front().unwrap_or_default())
    }
    fn complete_with_tools(
        &self,
        _s: &str,
        _m: &[Msg],
        _t: &[aishe::providers::ToolDef],
    ) -> Result<Completion, ProviderError> {
        self.meter.record(10, 5);
        let mut q = self.completions.lock().unwrap();
        if self.repeat_last && q.len() == 1 {
            return Ok(q.front().unwrap().clone());
        }
        Ok(q.pop_front().unwrap_or_default())
    }
    fn meter(&self) -> Arc<UsageMeter> {
        Arc::clone(&self.meter)
    }
}

fn tool_call(cmd: &str) -> ToolCall {
    ToolCall {
        id: "call".into(),
        name: "run_command".into(),
        arguments: json!({"command": cmd, "reason": "test"}),
    }
}

fn named_tool_call(name: &str, args: serde_json::Value) -> ToolCall {
    ToolCall {
        id: "call".into(),
        name: name.into(),
        arguments: args,
    }
}

#[test]
fn yolo_file_tools_write_and_read() {
    let dir = std::env::temp_dir().join(format!("aishe-ft-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("out.txt");
    // Turn 1: write_file with exact content. Turn 2: read it back. Turn 3: finish.
    let provider = MockProvider::with_completions(
        vec![
            Completion {
                text: None,
                tool_calls: vec![named_tool_call(
                    "write_file",
                    json!({"path": target.to_str().unwrap(), "content": "exact-content-42"}),
                )],
            },
            Completion {
                text: None,
                tool_calls: vec![named_tool_call(
                    "read_file",
                    json!({"path": target.to_str().unwrap()}),
                )],
            },
            Completion {
                text: Some("done".into()),
                tool_calls: vec![],
            },
        ],
        false,
    );
    let mut exec = Executor::new().unwrap();
    let mut config = Config::default();
    // Absolute temp path is "outside the tree"; disable the write confirm so the
    // test doesn't block on stdin.
    config.aishe.yolo_confirm_dangerous = false;
    let flag = AtomicBool::new(false);

    yolo::run(
        "make the file",
        &provider,
        &mut exec,
        &config,
        &flag,
        &SkillRegistry::default(),
        &mut Session::new(false),
    )
    .unwrap();

    assert!(target.exists(), "write_file tool did not create the file");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "exact-content-42"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn yolo_runs_tool_then_finishes() {
    let provider = MockProvider::with_completions(
        vec![
            Completion {
                text: None,
                tool_calls: vec![tool_call("echo yolo-ran")],
            },
            Completion {
                text: Some("all done".into()),
                tool_calls: vec![],
            },
        ],
        false,
    );
    let mut exec = Executor::new().unwrap();
    let mut config = Config::default();
    config.aishe.max_yolo_iterations = 10;
    let flag = AtomicBool::new(false);

    yolo::run(
        "do a thing",
        &provider,
        &mut exec,
        &config,
        &flag,
        &SkillRegistry::default(),
        &mut Session::new(true),
    )
    .unwrap();

    let ran = exec
        .history
        .iter()
        .any(|(cmd, code)| cmd == "echo yolo-ran" && *code == 0);
    assert!(
        ran,
        "expected the tool command to have run: {:?}",
        exec.history
    );
}

#[test]
fn yolo_streaming_runs_tool_then_finishes() {
    // With stream = true, yolo uses complete_with_tools_stream. The MockProvider
    // does not override it, so the default (fall back to non-streaming + sink the
    // text) applies; the loop must still run the tool and finish.
    let provider = MockProvider::with_completions(
        vec![
            Completion {
                text: Some("let me run it".into()),
                tool_calls: vec![tool_call("echo streamed-yolo")],
            },
            Completion {
                text: Some("# Done\n\nAll good.".into()),
                tool_calls: vec![],
            },
        ],
        false,
    );
    let mut exec = Executor::new().unwrap();
    let mut config = Config::default();
    config.aishe.stream = true;
    let flag = AtomicBool::new(false);

    yolo::run(
        "do a thing",
        &provider,
        &mut exec,
        &config,
        &flag,
        &SkillRegistry::default(),
        &mut Session::new(true),
    )
    .unwrap();

    assert!(
        exec.history
            .iter()
            .any(|(cmd, code)| cmd == "echo streamed-yolo" && *code == 0),
        "expected the streamed tool command to have run: {:?}",
        exec.history
    );
}

#[test]
fn yolo_respects_iteration_cap() {
    let provider = MockProvider::with_completions(
        vec![Completion {
            text: None,
            tool_calls: vec![tool_call("true")],
        }],
        true, // never finishes
    );
    let mut exec = Executor::new().unwrap();
    let mut config = Config::default();
    config.aishe.max_yolo_iterations = 3;
    let flag = AtomicBool::new(false);

    yolo::run(
        "loop forever",
        &provider,
        &mut exec,
        &config,
        &flag,
        &SkillRegistry::default(),
        &mut Session::new(true),
    )
    .unwrap();

    // Exactly the cap number of commands should have run.
    let count = exec.history.iter().filter(|(c, _)| c == "true").count();
    assert_eq!(count, 3, "history: {:?}", exec.history);
}

#[test]
fn session_memory_primes_next_request() {
    // The first turn is recorded; the second turn's request must include the
    // prior user+assistant messages so the model has context.
    let provider = MockProvider::with_completions(vec![], false);
    // We exercise the recording + priming via the modes path using a text mock.
    let p1 = MockProvider::with_text(r#"{"type":"command","command":"ls","explanation":"list"}"#);
    let mut exec = Executor::new().unwrap();
    let config = Config::default();
    let mut session = Session::new(true);

    // Turn 1: records user + assistant.
    suggest::run(
        "list files",
        &p1,
        &mut exec,
        &config,
        true,
        false,
        &mut session,
    )
    .unwrap();
    assert_eq!(session.turns(), 1, "first turn should be remembered");

    // History fed into the next request must carry both messages.
    let primed = session.history();
    assert_eq!(primed.len(), 2);
    assert!(matches!(&primed[0], Msg::User(t) if t == "list files"));
    let _ = provider; // silence unused in case of future edits
}

#[test]
fn disabled_session_does_not_record() {
    let p = MockProvider::with_text(r#"{"type":"command","command":"ls","explanation":"list"}"#);
    let mut exec = Executor::new().unwrap();
    let config = Config::default();
    let mut session = Session::new(false);
    suggest::run(
        "list files",
        &p,
        &mut exec,
        &config,
        true,
        false,
        &mut session,
    )
    .unwrap();
    assert_eq!(session.turns(), 0);
    assert!(session.history().is_empty());
}

#[test]
fn suggest_request_parses_command() {
    let provider =
        MockProvider::with_text(r#"{"type":"command","command":"ls -la","explanation":"list"}"#);
    let exec = Executor::new().unwrap();
    let config = Config::default();
    let s = suggest::request("show files", &provider, &exec, &config, Vec::new()).unwrap();
    assert_eq!(
        s,
        suggest::Suggestion::Command {
            command: "ls -la".into(),
            explanation: "list".into()
        }
    );
}

#[test]
fn suggest_request_parses_answer() {
    let provider =
        MockProvider::with_text(r#"{"type":"answer","command":null,"explanation":"the answer"}"#);
    let exec = Executor::new().unwrap();
    let config = Config::default();
    let s = suggest::request("what is 2+2", &provider, &exec, &config, Vec::new()).unwrap();
    assert!(matches!(s, suggest::Suggestion::Answer { .. }));
}

#[test]
fn suggest_scriptable_prints_command_without_running() {
    let provider =
        MockProvider::with_text(r#"{"type":"command","command":"echo never","explanation":"x"}"#);
    let mut exec = Executor::new().unwrap();
    let config = Config::default();
    // scriptable=true must not execute the command.
    suggest::run(
        "do it",
        &provider,
        &mut exec,
        &config,
        true,
        false,
        &mut Session::new(false),
    )
    .unwrap();
    assert!(
        !exec.history.iter().any(|(c, _)| c == "echo never"),
        "scriptable mode must not run the command"
    );
}
