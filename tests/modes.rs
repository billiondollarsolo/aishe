//! Mode tests driven by a scripted MockProvider (no network, no real LLM).

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use aishe::config::Config;
use aishe::executor::Executor;
use aishe::modes::{suggest, yolo};
use aishe::providers::{Completion, Msg, Provider, ProviderError, ResponseFormat, ToolCall};
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
    )
    .unwrap();

    // Exactly the cap number of commands should have run.
    let count = exec.history.iter().filter(|(c, _)| c == "true").count();
    assert_eq!(count, 3, "history: {:?}", exec.history);
}

#[test]
fn suggest_request_parses_command() {
    let provider =
        MockProvider::with_text(r#"{"type":"command","command":"ls -la","explanation":"list"}"#);
    let exec = Executor::new().unwrap();
    let config = Config::default();
    let s = suggest::request("show files", &provider, &exec, &config).unwrap();
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
    let s = suggest::request("what is 2+2", &provider, &exec, &config).unwrap();
    assert!(matches!(s, suggest::Suggestion::Answer { .. }));
}

#[test]
fn suggest_scriptable_prints_command_without_running() {
    let provider =
        MockProvider::with_text(r#"{"type":"command","command":"echo never","explanation":"x"}"#);
    let mut exec = Executor::new().unwrap();
    let config = Config::default();
    // scriptable=true must not execute the command.
    suggest::run("do it", &provider, &mut exec, &config, true, false).unwrap();
    assert!(
        !exec.history.iter().any(|(c, _)| c == "echo never"),
        "scriptable mode must not run the command"
    );
}
