//! Behavioral native turn tests: terminal state, admission of effects, and
//! checkpoint continuation are independent of the entrypoint/UI.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use aishe::agent::NativeTurnState;
use aishe::config::Config;
use aishe::executor::Executor;
use aishe::mcp::McpRegistry;
use aishe::modes::yolo;
use aishe::providers::{
    Completion, Msg, Provider, ProviderError, ResponseFormat, ToolCall, ToolDef,
};
use aishe::session::Session;
use aishe::skills::SkillRegistry;
use aishe::usage::{Usage, UsageMeter};
use serde_json::json;

static ENV: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let root = std::env::temp_dir().join(format!(
            "aishe-native-runtime-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut fixture = Self {
            root,
            saved: Vec::new(),
        };
        for name in [
            "AISHE_TASK_MAX_TOOL_CALLS",
            "AISHE_TASK_MAX_NETWORK_CALLS",
            "AISHE_TASK_MAX_PROVIDER_TURNS",
            "AISHE_TASK_MAX_MINUTES",
            "AISHE_TASK_MAX_COST_USD",
            "AISHE_BACKGROUND_TASK_ID",
        ] {
            fixture.save(name);
            std::env::remove_var(name);
        }
        fixture.save("AISHE_TASKS_DIR");
        std::env::set_var("AISHE_TASKS_DIR", fixture.root.join("tasks"));
        fixture.save("AISHE_POLICY_FILE");
        std::env::set_var("AISHE_POLICY_FILE", fixture.root.join("policy.toml"));
        fixture.save("AISHE_DATA_DIR");
        std::env::set_var("AISHE_DATA_DIR", fixture.root.join("data"));
        fixture
    }

    fn save(&mut self, name: &'static str) {
        self.saved.push((name, std::env::var_os(name)));
    }

    fn executor(&self) -> Executor {
        let mut executor = Executor::new().unwrap();
        executor.redirect_cwd(self.root.clone());
        executor.prefer_posix_capture();
        executor
    }

    fn config(&self) -> Config {
        let mut config = Config::default();
        config.aishe.yolo_confirm = "never".into();
        config.aishe.yolo_confirm_dangerous = false;
        config.aishe.show_usage = false;
        config.aishe.stream = false;
        config.aishe.yolo_preview = false;
        config
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for (name, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct Script {
    responses: Mutex<VecDeque<Result<Completion, ProviderError>>>,
    calls: AtomicU32,
    messages: Mutex<Vec<Vec<Msg>>>,
    meter: Arc<UsageMeter>,
    cancel_on_call: Option<Arc<AtomicBool>>,
}

impl Script {
    fn new(responses: Vec<Result<Completion, ProviderError>>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
            calls: AtomicU32::new(0),
            messages: Mutex::new(Vec::new()),
            meter: Arc::new(UsageMeter::default()),
            cancel_on_call: None,
        }
    }
}

impl Provider for Script {
    fn complete(&self, _: &str, _: &[Msg], _: &ResponseFormat) -> Result<String, ProviderError> {
        unreachable!("noninteractive fixture must not call planning")
    }

    fn complete_with_tools(
        &self,
        _: &str,
        messages: &[Msg],
        _: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.messages.lock().unwrap().push(messages.to_vec());
        self.meter.record(20, 10);
        if let Some(flag) = &self.cancel_on_call {
            flag.store(true, Ordering::SeqCst);
        }
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra provider request")
    }

    fn meter(&self) -> Arc<UsageMeter> {
        Arc::clone(&self.meter)
    }
}

fn completion(calls: Vec<ToolCall>) -> Result<Completion, ProviderError> {
    Ok(Completion {
        text: None,
        tool_calls: calls,
        provider_items: Vec::new(),
    })
}

fn final_answer() -> Result<Completion, ProviderError> {
    Ok(Completion {
        text: Some("Verified and complete.".into()),
        ..Completion::default()
    })
}

fn write(id: &str, path: &str) -> ToolCall {
    ToolCall {
        id: id.into(),
        name: "write_file".into(),
        arguments: json!({"path":path,"content":"effect"}),
    }
}

fn run(
    fixture: &Fixture,
    provider: &Script,
    config: &Config,
    interrupt: &AtomicBool,
) -> aishe::agent::NativeTurnOutcome {
    yolo::run(
        "Complete this task",
        provider,
        &mut fixture.executor(),
        config,
        interrupt,
        &SkillRegistry::default(),
        &McpRegistry::default(),
        &mut Session::new(false),
    )
    .unwrap()
}

#[test]
fn provider_failure_and_empty_response_are_failed_turns() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    for response in [
        Err(ProviderError::Http("fixture transport failure".into())),
        Ok(Completion::default()),
    ] {
        let provider = Script::new(vec![response]);
        let outcome = run(
            &fixture,
            &provider,
            &fixture.config(),
            &AtomicBool::new(false),
        );
        assert_eq!(outcome.state, NativeTurnState::Failed);
        assert_ne!(outcome.exit_code(), 0);
        assert!(outcome.final_text.is_none());
    }
}

#[test]
fn cancellation_before_and_during_provider_admits_no_tools() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let flag = Arc::new(AtomicBool::new(true));
    let mut provider = Script::new(vec![completion(vec![write("write", "cancelled-effect")])]);
    let outcome = run(&fixture, &provider, &fixture.config(), &flag);
    assert_eq!(outcome.state, NativeTurnState::Cancelled);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    flag.store(false, Ordering::SeqCst);
    provider.cancel_on_call = Some(Arc::clone(&flag));
    let outcome = run(&fixture, &provider, &fixture.config(), &flag);
    assert_eq!(outcome.state, NativeTurnState::Cancelled);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(!fixture.root.join("cancelled-effect").exists());
}

#[test]
fn multi_tool_batch_stops_at_budget_before_next_file_effect() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_TOOL_CALLS", "1");
    let provider = Script::new(vec![completion(vec![
        write("first", "first"),
        write("second", "second"),
        write("third", "third"),
    ])]);
    let outcome = run(
        &fixture,
        &provider,
        &fixture.config(),
        &AtomicBool::new(false),
    );
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(fixture.root.join("first").exists());
    assert!(!fixture.root.join("second").exists());
    assert!(!fixture.root.join("third").exists());
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn tool_budget_counts_skill_loading_before_commands() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_TOOL_CALLS", "1");
    let provider = Script::new(vec![completion(vec![
        ToolCall {
            id: "skill".into(),
            name: "use_skill".into(),
            arguments: json!({"name":"aishe-product"}),
        },
        write("write", "after-skill"),
    ])]);
    let outcome = yolo::run(
        "Use the product skill",
        &provider,
        &mut fixture.executor(),
        &fixture.config(),
        &AtomicBool::new(false),
        &SkillRegistry::load(),
        &McpRegistry::default(),
        &mut Session::new(false),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(!fixture.root.join("after-skill").exists());
}

#[test]
fn provider_turn_cap_is_enforced_across_checkpoint_resume() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_PROVIDER_TURNS", "2");
    let config = fixture.config();
    let mut active =
        aishe::tasks::Active::start(&config, &fixture.root, "Resume original objective");
    active.interrupted(&[], Usage::default());
    let mut record = active.record().clone();
    record.execution.provider_turns = 2;
    let provider = Script::new(vec![final_answer()]);
    let outcome = yolo::resume(
        record,
        &provider,
        &mut fixture.executor(),
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn resumed_tool_budget_cannot_restart_its_allowance() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_TOOL_CALLS", "1");
    let config = fixture.config();
    let mut active =
        aishe::tasks::Active::start(&config, &fixture.root, "Resume original objective");
    active.interrupted(&[], Usage::default());
    let mut record = active.record().clone();
    record.execution.tool_calls = 1;
    let provider = Script::new(vec![completion(vec![write("new", "resume-effect")])]);
    let outcome = yolo::resume(
        record,
        &provider,
        &mut fixture.executor(),
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(!fixture.root.join("resume-effect").exists());
}

#[test]
fn resumed_task_retains_original_caps_when_new_worker_environment_relaxes_them() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_TOOL_CALLS", "100");
    let config = fixture.config();
    let mut active =
        aishe::tasks::Active::start(&config, &fixture.root, "Resume original objective");
    active.interrupted(&[], Usage::default());
    let mut record = active.record().clone();
    record.execution.tool_calls = 1;
    record.execution_limits = Some(aishe::agent::native::NativeLimits {
        tool_calls: Some(1),
        ..aishe::agent::native::NativeLimits::default()
    });
    let provider = Script::new(vec![completion(vec![write("new", "relaxed-effect")])]);
    let outcome = yolo::resume(
        record,
        &provider,
        &mut fixture.executor(),
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(!fixture.root.join("relaxed-effect").exists());
}

#[test]
fn resume_resolves_unattempted_batch_tools_without_replaying_them() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut active =
        aishe::tasks::Active::start(&config, &fixture.root, "Resume original objective");
    let calls = vec![
        write("attempted", "attempted"),
        write("unattempted", "unattempted"),
    ];
    active.interrupted(
        &[
            Msg::User("Original objective".into()),
            Msg::Assistant(aishe::providers::AssistantMsg {
                text: None,
                tool_calls: calls,
            }),
            Msg::ToolResult {
                call_id: "attempted".into(),
                content: "Previous result".into(),
            },
        ],
        Usage::default(),
    );
    let provider = Script::new(vec![final_answer()]);
    let outcome = yolo::resume(
        active.record().clone(),
        &provider,
        &mut fixture.executor(),
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::Completed);
    assert!(!fixture.root.join("attempted").exists());
    assert!(!fixture.root.join("unattempted").exists());
    let seen = provider.messages.lock().unwrap();
    assert!(seen[0].iter().any(|message| matches!(message, Msg::ToolResult {call_id,content} if call_id == "unattempted" && content.starts_with("Not executed"))));
}

#[test]
fn iteration_limit_is_not_completion_but_a_final_summary_is() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.aishe.max_yolo_iterations = 1;
    let provider = Script::new(vec![completion(vec![write("one", "one")])]);
    let outcome = run(&fixture, &provider, &config, &AtomicBool::new(false));
    assert_eq!(outcome.state, NativeTurnState::IterationLimit);
    assert_ne!(outcome.exit_code(), 0);
    let outcome = run(
        &fixture,
        &Script::new(vec![final_answer()]),
        &config,
        &AtomicBool::new(false),
    );
    assert_eq!(outcome.state, NativeTurnState::Completed);
    assert_eq!(
        outcome.final_text.as_deref(),
        Some("Verified and complete.")
    );
}

#[test]
fn unoffered_tools_and_disabled_file_tools_cannot_invoke_effects() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let mut config = fixture.config();
    config.aishe.file_tools = false;
    let provider = Script::new(vec![
        completion(vec![
            write("disabled", "disabled"),
            ToolCall {
                id: "unknown".into(),
                name: "unknown_tool".into(),
                arguments: json!({"command":"touch unexpected-command"}),
            },
        ]),
        final_answer(),
    ]);
    let outcome = run(&fixture, &provider, &config, &AtomicBool::new(false));
    assert_eq!(outcome.state, NativeTurnState::Completed);
    assert!(!fixture.root.join("disabled").exists());
    assert!(!fixture.root.join("unexpected-command").exists());
}

#[test]
fn malformed_budget_environment_fails_before_provider_work() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    for (name, values) in [
        ("AISHE_TASK_MAX_TOOL_CALLS", vec!["oops", "-1", "1.5"]),
        ("AISHE_TASK_MAX_MINUTES", vec!["oops", "-1"]),
        ("AISHE_TASK_MAX_COST_USD", vec!["oops", "-1", "NaN", "inf"]),
    ] {
        for value in values {
            std::env::set_var(name, value);
            let provider = Script::new(vec![final_answer()]);
            let outcome = run(
                &fixture,
                &provider,
                &fixture.config(),
                &AtomicBool::new(false),
            );
            assert_eq!(outcome.state, NativeTurnState::Failed, "{name}={value}");
            assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
        }
        std::env::remove_var(name);
    }
}

#[test]
fn explicit_unpriced_cost_cap_fails_before_provider_work() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_COST_USD", "0.1");
    let mut config = fixture.config();
    config.set_active_model("unpriced-fixture-model".into());
    let provider = Script::new(vec![final_answer()]);
    let outcome = run(&fixture, &provider, &config, &AtomicBool::new(false));
    assert_eq!(outcome.state, NativeTurnState::Failed);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    for input in [f64::NAN, f64::INFINITY, -1.0] {
        config.pricing.insert(
            "unpriced-fixture-model".into(),
            aishe::usage::Price { input, output: 1.0 },
        );
        let provider = Script::new(vec![final_answer()]);
        let outcome = run(&fixture, &provider, &config, &AtomicBool::new(false));
        assert_eq!(outcome.state, NativeTurnState::Failed);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn network_budget_prevents_a_second_http_request_and_later_effects() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    std::env::set_var("AISHE_TASK_MAX_NETWORK_CALLS", "1");
    let mut server = mockito::Server::new();
    let first = server
        .mock("GET", "/first")
        .with_body("first network operation")
        .expect(1)
        .create();
    let second = server.mock("GET", "/second").expect(0).create();
    let fetch = |id: &str, route: &str| ToolCall {
        id: id.into(),
        name: "fetch_url".into(),
        arguments: json!({"url":format!("{}{route}", server.url())}),
    };
    let provider = Script::new(vec![completion(vec![
        fetch("first", "/first"),
        fetch("second", "/second"),
        write("late", "after-http"),
    ])]);
    let mut config = fixture.config();
    config.aishe.web_tool = true;
    let outcome = run(&fixture, &provider, &config, &AtomicBool::new(false));
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(!fixture.root.join("after-http").exists());
    first.assert();
    second.assert();
}

#[test]
fn admission_rejects_invalid_scope_policy_host_and_protected_target_without_mutating_authority() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let mut config = fixture.config();
    let mut executor = fixture.executor();
    executor.set_lean_scope(Some((
        aishe::agent::ExecutionScope::Workspace,
        fixture.root.clone(),
        aishe::agent::NetworkPolicy::Deny,
    )));
    let original = executor.lean_scope().cloned();
    config.backend.default_scope = "invalid".into();
    assert!(aishe::agent::native::prepare_executor(&mut executor, &config, &fixture.root).is_err());
    assert_eq!(executor.lean_scope(), original.as_ref());
    config.backend.default_scope = "host".into();
    config.sandbox.allow_host_yolo = false;
    assert!(aishe::agent::native::prepare_executor(&mut executor, &config, &fixture.root).is_err());
    config.sandbox.allow_host_yolo = true;
    config.sandbox.protected_environment_patterns = vec!["*".into()];
    let failure =
        aishe::agent::native::prepare_executor(&mut executor, &config, &fixture.root).unwrap_err();
    assert!(failure.to_string().contains("protected environment"));
    assert_eq!(executor.lean_scope(), original.as_ref());
    config.sandbox.protected_environment_patterns.clear();
    std::fs::write(
        fixture.root.join("policy.toml"),
        "version = 1\nallow_network = false\n",
    )
    .unwrap();
    config.backend.workspace_network = "deny".into();
    let failure =
        aishe::agent::native::prepare_executor(&mut executor, &config, &fixture.root).unwrap_err();
    assert!(failure.to_string().contains("network restriction"));
    assert_eq!(executor.lean_scope(), original.as_ref());
}

#[test]
fn resumed_task_deadline_bounds_provider_retries_and_stops_before_tools() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let config = fixture.config();
    let mut active =
        aishe::tasks::Active::start(&config, &fixture.root, "Resume original objective");
    active.interrupted(&[], Usage::default());
    let mut record = active.record().clone();
    record.execution.elapsed_ms = 59_850;
    record.execution_limits = Some(aishe::agent::native::NativeLimits {
        elapsed: Some(std::time::Duration::from_secs(60)),
        ..aishe::agent::native::NativeLimits::default()
    });
    let mut server = mockito::Server::new();
    let unavailable = server
        .mock("POST", "/v1/chat/completions")
        .with_status(503)
        .with_header("content-type", "application/json")
        .with_body(r#"{"error":{"message":"fixture temporarily unavailable"}}"#)
        .expect(1)
        .create();
    let provider = aishe::providers::openai_compat::OpenAiProvider::with_options(
        server.url(),
        String::new(),
        "deadline-fixture".into(),
        "chat_completions",
        "off",
    );
    let started = std::time::Instant::now();
    let outcome = yolo::resume(
        record,
        &provider,
        &mut fixture.executor(),
        &config,
        &AtomicBool::new(false),
        &SkillRegistry::default(),
        &McpRegistry::default(),
    )
    .unwrap();
    assert_eq!(outcome.state, NativeTurnState::BudgetExhausted);
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    unavailable.assert();
}

#[test]
fn authoritative_background_cancellation_before_admission_persists_cancelled_native_checkpoint() {
    let _lock = ENV.lock().unwrap();
    let fixture = Fixture::new();
    let id = "cancelled-before-admission";
    let directory = fixture.root.join("data/aishe/background-tasks").join(id);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("record.json"),
        serde_json::to_vec(&json!({
            "schema_version": 1, "id": id, "objective": "Cancelled objective",
            "source_cwd": fixture.root, "run_cwd": fixture.root,
            "created_at_ms": 1, "updated_at_ms": 1, "state": "cancelled",
            "budget": {"max_minutes": 1, "max_provider_turns": 1, "max_cost_usd": 0.0},
            "exit_code": 130
        }))
        .unwrap(),
    )
    .unwrap();
    std::env::set_var("AISHE_BACKGROUND_TASK_ID", id);
    let provider = Script::new(vec![final_answer()]);
    let outcome = run(
        &fixture,
        &provider,
        &fixture.config(),
        &AtomicBool::new(false),
    );
    assert_eq!(outcome.state, NativeTurnState::Cancelled);
    assert_eq!(outcome.exit_code(), 130);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let checkpoint = aishe::tasks::load(&outcome.task_id).unwrap();
    assert_eq!(checkpoint.status, aishe::tasks::Status::Interrupted);
    assert_eq!(checkpoint.native_state.as_deref(), Some("cancelled"));
    assert!(aishe::background::is_cancelled(id).unwrap());
}
