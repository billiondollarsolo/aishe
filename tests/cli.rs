//! End-to-end CLI tests via the built binary.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, Ordering};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

/// Write a minimal valid config into a temp XDG_CONFIG_HOME so the binary does
/// not invoke the interactive first-run wizard.
fn temp_config_home() -> std::path::PathBuf {
    let dir = temp_root("config");
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let mut f = std::fs::File::create(cfg_dir.join("config.toml")).unwrap();
    writeln!(
        f,
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[backend]
engine = "native"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-x"
"#
    )
    .unwrap();
    dir
}

fn temp_root(label: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "aishe-cli-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn long_help_mentions_connection_vs_model_and_aishe_brand() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("AIShe"))
        .stdout(contains("/connection"))
        .stdout(contains("/model"));
}

#[test]
fn repository_index_is_incremental_searchable_and_machine_readable() {
    let root = temp_root("repo-index");
    let config = temp_config_home();
    let data = root.join("data");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/auth.rs"),
        "fn validate_token(token: &str) { assert!(!token.is_empty()); }\n",
    )
    .unwrap();
    std::fs::write(root.join("ignored.bin"), [0, 1, 2]).unwrap();
    for args in [
        &["init", "-q"][..],
        &["config", "user.email", "aishe@example.invalid"],
        &["config", "user.name", "AIShe Test"],
        &["add", "."],
        &["commit", "-qm", "base"],
    ] {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
    }
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .current_dir(&root)
            .env("AISHE_CONFIG_DIR", &config)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    let built = run(&["index", "--json"]).output().unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&built.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["index"]["files"], 1);
    assert_eq!(value["changed_files"], 1);

    let unchanged = run(&["index", "--json"]).output().unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&unchanged.stdout).unwrap()["changed_files"],
        0
    );
    run(&["index", "--query", "validate token", "--json"])
        .assert()
        .success()
        .stdout(contains("src/auth.rs").and(contains("validate_token")));
    run(&["index", "--status", "--json"])
        .assert()
        .success()
        .stdout(contains("\"schema_version\": 1"));
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn failure_capsule_is_private_redacted_and_never_retries_effectful_commands() {
    let root = temp_root("failure-capsule");
    let data = root.join("data");
    let marker = root.join("must-not-exist");
    let shell = "shell-test-12345678";
    let command_text = "touch must-not-exist";
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .current_dir(&root)
            .env("AISHE_DATA_DIR", &data)
            .env("AISHE_SHELL_ID", shell)
            .args(args);
        command
    };
    run(&["--record-failure", command_text])
        .env("AISHE_LAST_EXIT", "1")
        .env("AISHE_LAST_DURATION_MS", "42")
        .assert()
        .success();
    let shown = run(&["last", "show", "--json"]).output().unwrap();
    assert!(shown.status.success());
    let capsule: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(capsule["schema_version"], 1);
    assert_eq!(capsule["duration_ms"], 42);
    run(&["last", "retry", "--execute"])
        .assert()
        .code(20)
        .stdout(contains("touch"));
    assert!(!marker.exists());
    let stored = std::fs::read_dir(data.join("aishe/failures"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(stored).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    run(&["last", "clear"]).assert().success();
    run(&["last", "show"]).assert().failure();
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn roles_mcp_and_profile_management_are_secret_reference_safe() {
    let config = temp_config_home();
    let data = config.join("data");
    let profile = config.join("portable.toml");
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &config)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    run(&[
        "role",
        "set",
        "compose",
        "--model",
        "fast-compose-model",
        "--reasoning",
        "low",
    ])
    .assert()
    .success();
    run(&["role", "list", "--json"])
        .assert()
        .success()
        .stdout(contains("fast-compose-model"));
    run(&[
        "mcp",
        "add",
        "local",
        "--command",
        "mcp-test-server",
        "--arg",
        "-y",
        "--arg",
        "serve",
        "--env",
        "ACCESS_TOKEN=SAFE_SOURCE_NAME",
    ])
    .assert()
    .success();
    run(&["mcp", "show", "local", "--json"])
        .assert()
        .success()
        .stdout(contains("env:SAFE_SOURCE_NAME"));
    run(&["mcp", "disable", "local"]).assert().success();
    run(&["profile", "export", profile.to_str().unwrap()])
        .assert()
        .success();
    let exported = std::fs::read_to_string(&profile).unwrap();
    assert!(exported.contains("fast-compose-model"));
    assert!(exported.contains("env:SAFE_SOURCE_NAME"));
    assert!(!exported.contains("sk-"));
    run(&["profile", "import", profile.to_str().unwrap(), "--yes"])
        .assert()
        .success()
        .stdout(contains("credentials: preserved separately"));
    run(&["mcp", "remove", "local"]).assert().success();
    run(&["role", "remove", "compose"]).assert().success();
    std::fs::remove_dir_all(config).ok();
}

#[test]
fn setup_rejects_conflicting_or_ignored_options() {
    for (args, message) in [
        (
            &["setup", "--resume", "--restart"][..],
            "cannot be used with",
        ),
        (
            &["setup", "--service", "openai"][..],
            "required arguments were not provided",
        ),
    ] {
        Command::cargo_bin("aishe")
            .unwrap()
            .args(args)
            .assert()
            .failure()
            .stderr(contains(message));
    }
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["setup", "--json"])
        .assert()
        .failure()
        .stdout(contains("--json requires --verify or --non-interactive"));
}

#[test]
fn oauth_commands_are_discoverable_and_status_never_exposes_tokens() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["auth", "--help"])
        .assert()
        .success()
        .stdout(
            contains("login")
                .and(contains("logout"))
                .and(contains("status")),
        );
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["auth", "login", "--help"])
        .assert()
        .success()
        .stdout(
            contains("--headless")
                .and(contains("--browser"))
                .and(contains("xai")),
        );

    let config = temp_config_home();
    let data = temp_root("oauth-data");
    let store = data
        .join("aishe/backend/opencode/xdg/data/opencode")
        .join("auth.json");
    std::fs::create_dir_all(store.parent().unwrap()).unwrap();
    std::fs::write(
        &store,
        r#"{
  "openai": {"type":"oauth","refresh":"openai-refresh-secret","access":"openai-access-secret","expires":9999999999999},
  "xai": {"type":"oauth","refresh":"xai-refresh-secret","access":"xai-access-secret","expires":9999999999999}
}"#,
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&store, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &config)
        .env("AISHE_DATA_DIR", &data)
        .args(["auth", "status", "xai", "--json"])
        .assert()
        .success()
        .stdout(
            contains(r#""selected": "oauth""#)
                .and(contains(r#""usable": true"#))
                .and(predicates::str::contains("xai-access-secret").not())
                .and(predicates::str::contains("xai-refresh-secret").not()),
        );

    std::fs::write(
        config.join("aishe/config.toml"),
        r#"[aishe]
provider = "openai"

[providers.openai]
base_url = "https://api.x.ai"
credential = "xai"
api_key_env = "XAI_API_KEY"
model = "grok-4.5"
transport = "responses"
"#,
    )
    .unwrap();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &config)
        .env("AISHE_DATA_DIR", &data)
        .env("XAI_API_KEY", "higher-precedence-api-secret")
        .args(["auth", "status", "xai", "--json"])
        .assert()
        .success()
        .stdout(
            contains(r#""selected": "api_key""#)
                .and(contains(r#""api_key_available": true"#))
                .and(predicates::str::contains("higher-precedence-api-secret").not()),
        );

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &config)
        .env("AISHE_DATA_DIR", &data)
        .args(["auth", "logout", "xai", "--yes"])
        .assert()
        .success()
        .stdout(contains("removed"));
    let remaining: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert!(remaining.get("openai").is_some());
    assert!(remaining.get("xai").is_none());

    std::fs::remove_dir_all(config).ok();
    std::fs::remove_dir_all(data).ok();
}

#[test]
fn named_connections_are_crud_safe_ambiguous_by_provider_and_audit_attributed() {
    let root = temp_root("named-connections");
    let config_dir = root.join("aishe");
    let data = root.join("data");
    let audit = data.join("audit.jsonl");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"version = 7

[aishe]
mode = "suggest"
provider = "openai"
connection = "openai-work"
connection_fallback = "openai-work"

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "https://api.openai.com"
model = "gpt-work"
transport = "responses"
[connections.openai-work.auth]
type = "api_key"
credential = "work-key"
api_key_env = "AISHE_WORK_KEY"

[connections.openai-personal]
provider = "openai"
label = "OpenAI personal"
base_url = "https://api.openai.com"
model = "gpt-personal"
transport = "responses"
[connections.openai-personal.auth]
type = "api_key"
credential = "personal-key"
api_key_env = "AISHE_PERSONAL_KEY"

[backend]
engine = "native"

[logging]
enabled = true
redact = true
"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &root)
            .env("AISHE_DATA_DIR", &data)
            .env("AISHE_LOG_FILE", &audit)
            .env("AISHE_WORK_KEY", "work-secret-never-log")
            .env("AISHE_PERSONAL_KEY", "personal-secret-never-log")
            .args(args);
        command
    };

    run(&["connection", "list"])
        .assert()
        .success()
        .stdout(contains("openai-work").and(contains("openai-personal")));
    let listed = run(&["connection", "list", "--json"]).output().unwrap();
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["schema_version"], 1);
    assert_eq!(listed["connections"].as_array().unwrap().len(), 2);
    run(&["provider", "openai"])
        .assert()
        .failure()
        .stderr(contains("matches multiple connections").and(contains("openai-work")));
    run(&["auth", "status", "--connection", "openai-work", "--json"])
        .assert()
        .success()
        .stdout(contains("work-secret-never-log").not())
        .stdout(contains(r#""schema_version": 1"#))
        .stdout(contains(r#""connection_id": "openai-work""#));
    run(&["settings", "--json"]).assert().success().stdout(
        contains(r#""schema_version": 1"#)
            .and(contains(r#""path": "aishe.connection""#))
            .and(contains(r#""value": "openai-work""#))
            .and(contains(r#""path": "connections.openai-work.auth.type""#))
            .and(contains("work-secret-never-log").not()),
    );
    let auth_profiles = run(&["auth", "list", "--json"]).output().unwrap();
    assert!(auth_profiles.status.success());
    let auth_profiles: serde_json::Value = serde_json::from_slice(&auth_profiles.stdout).unwrap();
    assert_eq!(auth_profiles["schema_version"], 1);
    assert!(auth_profiles["profiles"].is_array());
    run(&["status", "--json"]).assert().success().stdout(
        contains(r#""id": "openai-work""#)
            .and(contains(r#""auth": "Codex - API""#))
            .and(contains(r#""selection_scope": "default""#))
            .and(contains("work-secret-never-log").not()),
    );

    run(&[
        "connection",
        "add",
        "local-test",
        "--provider",
        "openai",
        "--base-url",
        "http://127.0.0.1:11434",
        "--model",
        "local-model",
        "--transport",
        "chat",
        "--auth",
        "none",
    ])
    .assert()
    .success();
    run(&[
        "connection",
        "edit",
        "local-test",
        "--label",
        "Local test edited",
        "--model",
        "local-model-2",
    ])
    .assert()
    .success();
    run(&["connection", "show", "local-test"])
        .assert()
        .success()
        .stdout(contains("Local test edited").and(contains("local-model-2")));
    let shown = run(&["connection", "show", "local-test", "--json"])
        .output()
        .unwrap();
    let shown: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["schema_version"], 1);
    assert_eq!(shown["model"], "local-model-2");
    run(&["connection", "remove", "local-test", "--yes"])
        .assert()
        .success()
        .stdout(contains("credentials were preserved"));

    let mut request = run(&["-c", "?answer this audit test"]);
    request.env(
        "AISHE_FAKE_LLM",
        r#"{"type":"answer","text":"audit test complete"}"#,
    );
    request.assert().success();
    let records = std::fs::read_to_string(&audit).unwrap();
    assert!(records.contains(r#""connection_id":"openai-work""#));
    assert!(records.contains(r#""connection_label":"OpenAI work""#));
    assert!(records.contains(r#""auth_type":"api_key""#));
    assert!(records.contains(r#""auth_profile":"work-key""#));
    assert!(records.lines().all(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .is_ok_and(|event| event["schema_version"] == 1)
    }));
    assert!(!records.contains("work-secret-never-log"));
    assert!(!records.contains("personal-secret-never-log"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn isolated_oauth_profiles_report_and_logout_independently() {
    let root = temp_root("oauth-profiles");
    let config = temp_config_home();
    let data = root.join("data");
    let auth_root = data.join("aishe/backend/opencode/profiles/openai");
    for profile in ["work", "personal"] {
        let path = auth_root.join(profile).join("xdg/data/opencode/auth.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                r#"{{"openai":{{"type":"oauth","refresh":"{profile}-refresh-secret","access":"{profile}-access-secret","expires":9999999999999}}}}"#
            ),
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &config)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    for profile in ["work", "personal"] {
        run(&["auth", "status", "openai", "--profile", profile, "--json"])
            .assert()
            .success()
            .stdout(contains(r#""available": true"#))
            .stdout(contains("access-secret").not())
            .stdout(contains("refresh-secret").not());
    }
    run(&["auth", "logout", "openai", "--profile", "work", "--yes"])
        .assert()
        .success();
    run(&["auth", "status", "openai", "--profile", "work", "--json"])
        .assert()
        .failure()
        .stdout(contains(r#""available": false"#));
    run(&[
        "auth",
        "status",
        "openai",
        "--profile",
        "personal",
        "--json",
    ])
    .assert()
    .success()
    .stdout(contains(r#""available": true"#));
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(config).ok();
}

fn serve_model_catalog(requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for _ in 0..requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let body = r#"{"data":[{"id":"local-model-b"},{"id":"local-model-a"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        }
    });
    (format!("http://{address}"), handle)
}

#[test]
fn man_emits_a_roff_page() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("man")
        .assert()
        .success()
        .stdout(contains(".TH aishe 1").and(contains("natural")));
}

#[test]
fn suggest_subcommand_scripting_contract() {
    let home = temp_config_home();
    let base = || {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", home.join("data"))
            .env("ANTHROPIC_API_KEY", "sk-test");
        c
    };
    // Safe command in JSON: stdout is one object, exit 0.
    base()
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"ls -la","explanation":"lists"}"#,
        )
        .args(["suggest", "--json", "list", "files"])
        .assert()
        .success()
        .stdout(
            contains("\"schema_version\":1")
                .and(contains("\"command\":\"ls -la\""))
                .and(contains("\"risk\":\"safe\"")),
        );
    // Dangerous command → exit 20 (still printed for review).
    base()
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"rm -rf /","explanation":"boom"}"#,
        )
        .args(["suggest", "--json", "wipe", "everything"])
        .assert()
        .code(20)
        .stdout(contains("\"risk\":\"dangerous\""));
    // Empty query → exit 1 with guidance.
    base()
        .args(["suggest", "--json"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            contains("\"schema_version\":1")
                .and(contains("\"code\":\"cli.missing_request\""))
                .and(contains("\u{1b}[").not()),
        );
}

#[test]
fn ask_supports_plain_json_and_schema_validated_output() {
    let home = temp_config_home();
    let data = temp_root("ask-data");
    let base = || {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", &data)
            .env("ANTHROPIC_API_KEY", "sk-test");
        command
    };
    base()
        .env("AISHE_FAKE_LLM", "plain answer")
        .args(["ask", "what", "changed?"])
        .assert()
        .success()
        .stdout("plain answer\n");
    base()
        .env("AISHE_FAKE_LLM", "plain answer")
        .args(["ask", "--json", "what", "changed?"])
        .assert()
        .success()
        .stdout(contains("\"schema_version\":1").and(contains("\"answer\":\"plain answer\"")));

    let schema = data.join("answer.schema.json");
    std::fs::write(
        &schema,
        r#"{"type":"object","additionalProperties":false,"properties":{"ok":{"type":"boolean"}},"required":["ok"]}"#,
    )
    .unwrap();
    base()
        .env("AISHE_FAKE_LLM", r#"{"ok":true}"#)
        .arg("ask")
        .arg("--schema")
        .arg(&schema)
        .arg("report")
        .assert()
        .success()
        .stdout(contains("\"result\":{\"ok\":true}"));
    base()
        .env("AISHE_FAKE_LLM", r#"{"wrong":true}"#)
        .arg("ask")
        .arg("--schema")
        .arg(&schema)
        .arg("report")
        .assert()
        .failure()
        .stdout("")
        .stderr(contains("missing required property"));
}

#[test]
fn ask_insert_uses_private_shell_handoff_and_never_executes() {
    let home = temp_config_home();
    let data = temp_root("ask-insert-data");
    let pending = data.join("pending");
    let marker = data.join("must-not-exist");
    Command::cargo_bin("aishe")
        .unwrap()
        .current_dir(&data)
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .env("AISHE_PENDING_FILE", &pending)
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"touch must-not-exist","explanation":"test"}"#,
        )
        .args(["ask", "--insert", "make", "a", "marker"])
        .assert()
        .success()
        .stdout("");
    assert_eq!(
        std::fs::read_to_string(&pending).unwrap(),
        "fill\ntouch must-not-exist\n"
    );
    assert!(!marker.exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&pending).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    std::fs::remove_dir_all(home).ok();
    std::fs::remove_dir_all(data).ok();
}

#[test]
fn background_task_isolates_reviews_applies_and_discards() {
    let home = temp_config_home();
    let data = temp_root("background-data");
    let repo = temp_root("background-repo");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success(), "git command failed: {args:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "aishe@example.test"]);
    git(&["config", "user.name", "AIShe Test"]);
    std::fs::write(repo.join("tracked.txt"), "original\n").unwrap();
    git(&["add", "tracked.txt"]);
    git(&["commit", "-qm", "base"]);

    let base = || {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .current_dir(&repo)
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", &data)
            .env("ANTHROPIC_API_KEY", "sk-test")
            .env("AISHE_FAKE_LLM", "task complete")
            .env("AISHE_FAKE_TOOL", "printf 'changed\\n' > tracked.txt");
        command
    };
    let output = base()
        .args(["task", "start", "update", "the", "tracked", "file"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("started task "))
        .expect("task id")
        .to_string();

    let mut state = String::new();
    for _ in 0..100 {
        let shown = base()
            .env_remove("AISHE_FAKE_TOOL")
            .args(["task", "show", &id, "--json"])
            .output()
            .unwrap();
        assert!(shown.status.success());
        state = String::from_utf8(shown.stdout).unwrap();
        if state.contains("\"state\": \"completed\"") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(state.contains("\"state\": \"completed\""), "{state}");
    assert_eq!(
        std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
        "original\n"
    );

    base()
        .env_remove("AISHE_FAKE_TOOL")
        .args(["task", "review", &id])
        .assert()
        .success()
        .stdout(contains("+changed"));
    base()
        .env_remove("AISHE_FAKE_TOOL")
        .args(["task", "apply", &id])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(repo.join("tracked.txt")).unwrap(),
        "changed\n"
    );
    base()
        .env_remove("AISHE_FAKE_TOOL")
        .args(["task", "discard", &id])
        .assert()
        .success();
}

#[test]
fn explicit_suggest_is_not_cut_off_by_the_shell_hook_budget() {
    let home = temp_config_home();
    let path = home.join("aishe").join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap().replace(
        "mode = \"suggest\"",
        "mode = \"suggest\"\nhook_timeout_secs = 1\ncache = false",
    );
    std::fs::write(&path, text).unwrap();
    let response = r#"{"type":"command","command":"ls -la","explanation":"lists"}"#;
    let base = || {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", home.join("data"))
            .env("ANTHROPIC_API_KEY", "sk-test")
            .env("AISHE_FAKE_LLM", response)
            .env("AISHE_FAKE_DELAY_MS", "1500");
        command
    };

    // Explicit scripting waits for the real result, even when it takes longer
    // than the interactive hook's responsiveness budget.
    base()
        .args(["suggest", "--json", "list", "files"])
        .assert()
        .success()
        .stdout(contains("\"command\":\"ls -la\""));

    // The native shell hook remains bounded by that configured budget.
    base()
        .args(["--suggest-line", "show files slowly"])
        .assert()
        .success()
        .stdout("")
        .stderr(contains("suggestion timed out"));
}

#[test]
fn explicit_suggest_propagates_provider_failure() {
    let home = temp_config_home();
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_FAKE_LLM", "unused")
        .env("AISHE_FAKE_ERROR", "synthetic upstream failure")
        .args(["suggest", "--json", "list", "files"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.contains(&0x1b));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr)
        .expect("JSON-mode suggest failure must own stderr with one JSON document");
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["code"], "provider.server_unavailable");
    assert!(error["next_action"]
        .as_str()
        .is_some_and(|value| !value.is_empty()));
}

#[test]
fn suggest_hook_appends_to_the_session_usage_tally() {
    // Under the interactive PTY, each NL child appends its metered usage to the
    // shared tally named by AISHE_USAGE_FILE; the PTY prints a one-line summary on
    // exit. Drive one suggest-hook invocation directly (the fake records the
    // AISHE_FAKE_USAGE tokens) and assert the tally line was written.
    let home = temp_config_home();
    let tally = home.join("usage.tally");
    std::fs::remove_file(&tally).ok();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_FAKE_LLM", "ls -la")
        .env("AISHE_FAKE_USAGE", "120,30")
        .env("AISHE_USAGE_FILE", &tally)
        .args(["--suggest-line", "list files in long form"])
        .assert()
        .success();
    let contents = std::fs::read_to_string(&tally).unwrap_or_default();
    // One tab-separated tally line: "<input>\t<output>\t<model>".
    assert!(
        contents.contains("120\t30\t"),
        "expected a usage tally line, got: {contents:?}"
    );
    std::fs::remove_file(&tally).ok();
}

#[test]
fn no_usage_file_means_no_tally_written() {
    // Without AISHE_USAGE_FILE (i.e. not under a PTY session), nothing is tallied.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_FAKE_LLM", "ls -la")
        .env("AISHE_FAKE_USAGE", "10,5")
        .env_remove("AISHE_USAGE_FILE")
        .args(["--suggest-line", "list files"])
        .assert()
        .success();
    // Nothing to assert beyond a clean run: the absence of a tally file is the
    // point (no env var, no path to write to).
}

#[test]
fn dash_c_commands_are_recorded_in_history() {
    // Commands run via `-c` are persisted to the timestamped history log, so a
    // later `aishe history` (and semantic indexing) can see them.
    let home = temp_config_home();
    let data = home.join("data");
    for c in ["!echo alpha", "!echo bravo"] {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", &data)
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", &data)
            .arg("-c")
            .arg(c)
            .assert()
            .success();
    }
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .arg("-c")
        .arg("history")
        .assert()
        .success()
        .stdout(contains("echo alpha").and(contains("echo bravo")));
}

#[test]
fn fix_line_prints_a_corrected_command() {
    // The fix-the-last-command hook returns a corrected command for the widget to
    // pre-fill. With fix_capture_stderr on, a read-only failed command is re-run
    // to capture its error (here the fake model just echoes a fixed command).
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env("AISHE_LAST_EXIT", "2")
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"command","command":"ls -la /tmp","explanation":"fixed"}"#,
        )
        .args(["--fix-line", "ls /nonexistent-aishe-xyz"])
        .assert()
        .success()
        .stdout(contains("ls -la /tmp"));
}

#[test]
fn dash_c_custom_slash_commands_bypass_only_the_shell_fast_path() {
    let home = temp_config_home();
    let commands = home.join("aishe").join("commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(
        commands.join("echo-args.md"),
        "---\ndescription: test custom command\nshell: true\n---\nprintf 'custom=%s\\n' \"$ARGUMENTS\"\n",
    )
    .unwrap();

    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .args(["-c", "/echo-args hello world"])
        .assert()
        .success()
        .stdout("custom=hello world\n");
}

#[test]
fn dash_c_runs_forced_shell_command() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .arg("-c")
        .arg("!echo hi-from-aishe")
        .assert()
        .success()
        .stdout(contains("hi-from-aishe"))
        .stderr(
            contains("AIShe · shell override")
                .and(contains("safety gate bypassed"))
                .and(contains("this line only"))
                .and(contains("\u{1b}[").not()),
        );
}

#[test]
fn legacy_hash_agent_prefix_warns_and_remains_non_sticky() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .env(
            "AISHE_FAKE_LLM",
            r#"{"type":"answer","explanation":"fixture answer"}"#,
        )
        .arg("-c")
        .arg("# explain this fixture")
        .assert()
        .success()
        .stdout("fixture answer\n")
        .stderr(
            contains("# agent prefix is deprecated; use ?")
                .and(contains("removal planned for 0.9"))
                .and(contains("\u{1b}[").not()),
        );
}

#[test]
fn version_flag_works() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("aishe"));
}

#[test]
fn init_zsh_emits_integration() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["init", "zsh"])
        .assert()
        .success()
        .stdout(contains("command_not_found_handler"))
        .stdout(contains("print -z"));
}

#[test]
fn doctor_reports_environment() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env("ANTHROPIC_API_KEY", "sk-test")
        .arg("doctor")
        .assert()
        .success()
        .stdout(contains("backing shell"))
        .stdout(contains("front-end"))
        .stdout(contains("provider: anthropic"))
        .stdout(contains("environment:$ANTHROPIC_API_KEY"));
}

#[test]
fn missing_openai_key_names_the_exact_environment_variable() {
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .env_remove("OPENAI_API_KEY")
        .env_remove("AISHE_FAKE_LLM")
        .args([
            "--provider",
            "openai",
            "-c",
            "?what is the capital of France",
        ])
        .assert()
        .code(1)
        .stderr(
            contains("API key missing for credential profile 'openai'")
                .and(contains("aishe auth set openai"))
                .and(contains("$OPENAI_API_KEY")),
        );
}

#[test]
fn shared_credentials_power_real_provider_paths_and_env_wins() {
    fn serve_once() -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let count = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..count]).into_owned();
            let body = r#"{"data":[{"id":"credential-test-model"}]}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), handle)
    }

    let dir = temp_root("shared-credentials");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    let (first_endpoint, first_server) = serve_once();
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"version = 3

[aishe]
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "{first_endpoint}"
credential = "openai"
api_key_env = "AISHE_CREDENTIAL_TEST_ENV"
model = "credential-test-model"
transport = "chat"
auth_required = true
"#
        ),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .env_remove("AISHE_CREDENTIAL_TEST_ENV")
            .args(args);
        command
    };
    let stored_secret = "stored-test-key-never-print";
    run(&["auth", "set", "openai", "--stdin"])
        .write_stdin(format!("{stored_secret}\n"))
        .assert()
        .success()
        .stdout(contains(stored_secret).not());

    let credentials_path = config_dir.join("credentials.toml");
    let credentials_before = std::fs::read(&credentials_path).unwrap();
    assert!(String::from_utf8_lossy(&credentials_before).contains(stored_secret));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&credentials_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    run(&["auth", "status", "openai", "--json"])
        .assert()
        .success()
        .stdout(contains("\"type\": \"credentials_file\""))
        .stdout(contains(stored_secret).not());
    run(&["models", "--provider", "openai", "--json"])
        .assert()
        .success()
        .stdout(contains("credential-test-model"));
    let first_request = first_server.join().unwrap();
    assert!(
        first_request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {stored_secret}")),
        "HTTP field names are case-insensitive"
    );

    let (second_endpoint, second_server) = serve_once();
    let config_text = std::fs::read_to_string(config_dir.join("config.toml"))
        .unwrap()
        .replace(&first_endpoint, &second_endpoint);
    std::fs::write(config_dir.join("config.toml"), config_text).unwrap();
    let environment_secret = "environment-test-key-never-print";
    let mut overridden = run(&["models", "--provider", "openai", "--json"]);
    overridden.env("AISHE_CREDENTIAL_TEST_ENV", environment_secret);
    overridden.assert().success();
    let second_request = second_server.join().unwrap();
    assert!(
        second_request
            .to_ascii_lowercase()
            .contains(&format!("authorization: bearer {environment_secret}")),
        "HTTP field names are case-insensitive"
    );
    assert_eq!(
        std::fs::read(&credentials_path).unwrap(),
        credentials_before
    );

    run(&["auth", "remove", "openai", "--yes"])
        .assert()
        .success()
        .stdout(contains(stored_secret).not());
    run(&["auth", "status", "openai", "--json"])
        .assert()
        .failure()
        .stdout(contains("\"available\": false"))
        .stdout(contains(stored_secret).not());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn schema_two_migration_adds_profile_without_importing_environment_secret() {
    let dir = temp_root("credential-migration");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    let original = br#"version = 2

[aishe]
provider = "openai"
front_end = "reedline"
suggestion_style = "ghost_text"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-test"
transport = "responses"
"#;
    std::fs::write(config_dir.join("config.toml"), original).unwrap();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("OPENAI_API_KEY", "migration-secret-must-not-be-imported")
        .arg("config")
        .assert()
        .success();
    let migrated = std::fs::read_to_string(config_dir.join("config.toml")).unwrap();
    assert!(migrated.contains("version = 7"));
    assert!(migrated.contains("[connections.openai]"));
    assert!(migrated.contains("[backend]"));
    assert!(migrated.contains("engine = \"opencode\""));
    assert!(migrated.contains("[sandbox]"));
    assert!(migrated.contains("[ui]"));
    assert!(migrated.contains("theme = \"auto\""));
    assert!(migrated.contains("color_depth = \"auto\""));
    assert!(migrated.contains("unicode = \"auto\""));
    assert!(migrated.contains("motion = \"auto\""));
    assert!(migrated.contains("credential = \"openai\""));
    assert!(!migrated.contains("front_end"));
    assert!(!migrated.contains("suggestion_style"));
    assert!(!config_dir.join("credentials.toml").exists());
    let backups: Vec<_> = std::fs::read_dir(&config_dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".v2."))
        .collect();
    assert_eq!(backups.len(), 1);
    assert_eq!(std::fs::read(backups[0].path()).unwrap(), original);
    std::fs::remove_dir_all(dir).ok();
}

#[cfg(unix)]
#[test]
fn doctor_repairs_credential_permissions_without_exposing_or_changing_key() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_config_home();
    let credentials = dir.join("aishe").join("credentials.toml");
    let secret = "doctor-permission-test-key";
    std::fs::write(
        &credentials,
        format!("version = 1\n[profiles.anthropic]\napi_key = \"{secret}\"\n"),
    )
    .unwrap();
    std::fs::set_permissions(&credentials, std::fs::Permissions::from_mode(0o644)).unwrap();
    let before = std::fs::read(&credentials).unwrap();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env_remove("ANTHROPIC_API_KEY")
        .args(["doctor", "--fix", "--json"])
        .assert()
        .success()
        .stdout(contains("\"id\": \"credentials.file\""))
        .stdout(contains(secret).not());
    assert_eq!(
        std::fs::metadata(&credentials)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(std::fs::read(&credentials).unwrap(), before);
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env_remove("ANTHROPIC_API_KEY")
        .args(["auth", "status", "anthropic", "--json"])
        .assert()
        .success()
        .stdout(contains("\"type\": \"credentials_file\""))
        .stdout(contains(secret).not());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn credentials_path_override_is_honored_without_touching_default_file() {
    let dir = temp_root("credentials-path-override");
    let external_parent = dir.join("mounted-private");
    std::fs::create_dir_all(&external_parent).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&external_parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let custom = external_parent.join("shared.toml");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_CREDENTIALS_FILE", &custom)
        .args(["auth", "set", "openai", "--stdin"])
        .write_stdin("override-path-test-key\n")
        .assert()
        .success()
        .stdout(contains("override-path-test-key").not());
    assert!(custom.is_file());
    assert!(!dir.join("aishe").join("credentials.toml").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&external_parent)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755,
            "Aishe changed an existing override parent directory"
        );
    }
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_CREDENTIALS_FILE", &custom)
        .args(["auth", "path"])
        .assert()
        .success()
        .stdout(contains(custom.display().to_string()));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn doctor_probe_runs_reachability_section() {
    // `--probe` adds the reachability section. Point the provider at a dead local
    // port so the probe deterministically reports unreachable without real
    // network, and doctor still exits 0 (a down endpoint is a warning, not
    // critical).
    let dir = std::env::temp_dir().join(format!("aishe-probe-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[providers.anthropic]
base_url = "http://127.0.0.1:1"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"

[backend]
engine = "native"
"#,
    )
    .unwrap();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .arg("doctor")
        .arg("--probe")
        .assert()
        .success()
        .stdout(contains("reachability probe:"))
        .stdout(contains("anthropic: unreachable"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn init_unsupported_shell_fails() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["init", "fish"])
        .assert()
        .failure();
}

#[test]
fn dash_c_propagates_exit_codes() {
    let home = temp_config_home();
    let run = |arg: &str| {
        Command::cargo_bin("aishe")
            .unwrap()
            .env("XDG_CONFIG_HOME", &home)
            .env("XDG_DATA_HOME", home.join("data"))
            .env("AISHE_CONFIG_DIR", &home)
            .env("AISHE_DATA_DIR", home.join("data"))
            .arg("-c")
            .arg(arg)
            .assert()
    };
    // A failing command propagates its non-zero status.
    run("!false").code(1);
    // A succeeding command is 0.
    run("!true").code(0);
    // `exit N` propagates N.
    run("exit 3").code(3);
    // A pipeline's status is the last command's.
    run("!true | false").code(1);
    // `$?` reflects the previous command within one -c line.
    run("!false; echo done").code(0).stdout(contains("done"));
}

#[test]
fn piped_stdin_runs_each_line() {
    // Non-tty stdin with no `-c`: each line runs like a one-shot command.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .write_stdin("!echo piped-a\n!echo piped-b\n")
        .assert()
        .success()
        .stdout(contains("piped-a"))
        .stdout(contains("piped-b"));
}

#[test]
fn version_includes_build_metadata() {
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let version = String::from_utf8_lossy(&output.stdout);
    assert!(
        version.starts_with("aishe ") && version.contains('('),
        "missing build metadata: {version:?}"
    );

    // In a Git checkout the embedded revision must identify this exact source
    // state. This catches build.rs watching `.git/HEAD` but not the branch ref,
    // which otherwise leaves incremental builds reporting an older commit.
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    if repo.join(".git").exists() {
        let git = std::process::Command::new("git")
            .args(["rev-parse", "--short", "HEAD"])
            .current_dir(repo)
            .output()
            .unwrap();
        if git.status.success() {
            let expected = String::from_utf8_lossy(&git.stdout);
            let expected = expected.trim();
            assert!(
                version.contains(expected),
                "version {version:?} does not contain current Git revision {expected:?}"
            );
        }
    }
}

#[test]
fn cli_flags_are_accepted_over_config() {
    // The config selects anthropic/suggest; flags switch provider/model/mode.
    // The forced `!` command still runs, proving apply_overrides is wired in
    // without breaking the non-interactive path.
    let home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", home.join("data"))
        .args([
            "--provider",
            "openai",
            "--model",
            "some-model",
            "--mode",
            "yolo",
        ])
        .arg("-c")
        .arg("!echo flags-ok")
        .assert()
        .success()
        .stdout(contains("flags-ok"));
}

#[test]
fn output_command_persists_density_without_touching_history() {
    let home = temp_config_home();
    let data = home.join("data");
    let history = data.join("aishe").join("history.ext");
    std::fs::create_dir_all(history.parent().unwrap()).unwrap();
    std::fs::write(&history, ": 1:0;echo keep-output-history\n").unwrap();
    let unrelated = home.join("must-not-be-an-output-handoff");
    std::fs::write(&unrelated, "keep-unrelated\n").unwrap();

    let mut command = Command::cargo_bin("aishe").unwrap();
    command
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .env("AISHE_SHELL_ID", "0123456789abcdef")
        .env("AISHE_OUTPUT_FILE", &unrelated)
        .args(["output", "detailed"])
        .assert()
        .success()
        .stdout(contains("output = detailed"));
    let persisted = std::fs::read_to_string(home.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("output = \"detailed\""));
    assert_eq!(
        std::fs::read_to_string(&history).unwrap(),
        ": 1:0;echo keep-output-history\n"
    );
    assert_eq!(
        std::fs::read_to_string(&unrelated).unwrap(),
        "keep-unrelated\n"
    );

    std::fs::remove_dir_all(home).ok();
}

#[test]
fn primary_commands_and_live_status_are_discoverable() {
    let home = temp_config_home();
    let data = home.join("data");
    let usage = home.join("usage.tsv");
    let status = home.join("status.tsv");
    std::fs::write(&usage, "1000\t250\t2\tclaude-x\n").unwrap();
    std::fs::write(
        &status,
        "task\ttask abc123\nelapsed\tlast 4.2s\ncontext\tcontext 1,000 tok\n",
    )
    .unwrap();

    let mut commands = Command::cargo_bin("aishe").unwrap();
    commands
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .arg("commands")
        .assert()
        .success()
        .stdout(
            contains("AIShe")
                .and(contains("/help"))
                .and(contains("/connection"))
                .and(contains("/model"))
                .and(contains("/status"))
                .and(contains("/reasoning"))
                .and(contains("Ctrl-O")),
        );

    let mut live = Command::cargo_bin("aishe").unwrap();
    live.env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .env("AISHE_USAGE_FILE", &usage)
        .env("AISHE_STATUS_FILE", &status)
        .env("AISHE_MODEL", "session-model")
        .env("AISHE_MODE", "yolo")
        .env("AISHE_SCOPE", "host")
        .env("AISHE_AGENT_OUTPUT", "focus")
        .args(["status", "--json"])
        .assert()
        .success()
        .stdout(
            contains(r#""schema_version": 1"#)
                .and(contains(r#""model": "session-model""#))
                .and(contains(r#""mode": "yolo""#))
                .and(contains(r#""scope": "host""#))
                .and(contains(r#""output": "focus""#))
                .and(contains(r#""reasoning_effort": "auto""#))
                .and(contains(r#""audit""#))
                .and(contains(r#""enabled": false"#))
                .and(contains("audit.jsonl"))
                .and(contains("aishe session: 1,000 in · 250 out · 2 reqs"))
                .and(contains("task abc123")),
        );

    std::fs::remove_dir_all(home).ok();
}

#[test]
fn reasoning_command_persists_and_slash_command_reports_it() {
    let home = temp_config_home();
    let data = home.join("data");

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .args(["reasoning", "high"])
        .assert()
        .success()
        .stdout(contains("reasoning = high"));

    let persisted = std::fs::read_to_string(home.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("reasoning_effort = \"high\""));

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &home)
        .env("AISHE_DATA_DIR", &data)
        .args(["-c", "/reasoning"])
        .assert()
        .success()
        .stdout(contains("reasoning: high"));

    std::fs::remove_dir_all(home).ok();
}

#[test]
fn legacy_llmsh_config_is_migrated_on_run() {
    // A pre-rename ~/.config/llmsh/config.toml (and no aishe config) is ported
    // to the new location on first run, with the [llmsh] section rewritten.
    let dir = std::env::temp_dir().join(format!("aishe-migrate-{}", std::process::id()));
    let legacy_dir = dir.join("llmsh");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(
        legacy_dir.join("config.toml"),
        r#"[llmsh]
mode = "auto"
provider = "openai"

[providers.openai]
base_url = "http://localhost:11434"
api_key_env = "OPENAI_API_KEY"
model = "llama3"
"#,
    )
    .unwrap();

    Command::cargo_bin("aishe")
        .unwrap()
        .env("XDG_CONFIG_HOME", &dir)
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .arg("-c")
        .arg("!echo migrated-run")
        .assert()
        .success()
        .stdout(contains("migrated-run"))
        .stderr(contains("migrated config"));

    // The new aishe config exists with the section header rewritten.
    let ported = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(ported.contains("[aishe]"), "ported config: {ported}");
    assert!(!ported.contains("[llmsh]"), "ported config: {ported}");
    assert!(ported.contains("llama3"), "ported config: {ported}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn project_config_overlay_and_trust_flow() {
    // Isolated config + data homes so the trust store doesn't leak between runs.
    let dir = std::env::temp_dir().join(format!("aishe-trust-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[aishe]
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "user-model"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "user-openai"

[backend]
engine = "native"
"#,
    )
    .unwrap();

    // A project with a safe key (stream) and a sensitive one (provider switch).
    let proj = dir.join("repo");
    std::fs::create_dir_all(proj.join(".aishe")).unwrap();
    std::fs::write(
        proj.join(".aishe").join("config.toml"),
        "[aishe]\nstream = true\nprovider = \"openai\"\n",
    )
    .unwrap();

    let data = dir.join("data");
    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &dir)
            .env("XDG_DATA_HOME", &data)
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .current_dir(&proj)
            .args(args);
        c
    };

    // Untrusted: doctor reports the overlay, the sensitive provider switch is
    // deferred, and the effective provider stays anthropic.
    run(&["doctor"])
        .assert()
        .success()
        .stdout(contains("project config:"))
        .stdout(contains("untrusted"))
        .stdout(contains("deferred"))
        .stdout(contains("provider: anthropic"));

    // Trust it; the provider switch is reported as newly applying.
    run(&["trust"])
        .assert()
        .success()
        .stdout(contains("Trusted"))
        .stdout(contains("provider"));

    // Now trusted: the provider switch takes effect.
    run(&["doctor"])
        .assert()
        .success()
        .stdout(contains("trusted"))
        .stdout(contains("provider: openai"));

    // It shows up in the trust list, and untrust removes it.
    run(&["trust", "--list"])
        .assert()
        .success()
        .stdout(contains("config.toml"));
    run(&["untrust"])
        .assert()
        .success()
        .stdout(contains("Dropped trust"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn completions_emits_a_script() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("_aishe"));
}

#[test]
fn settings_subcommands_show_and_persist() {
    // A dedicated config home: these subcommands write to the config file, so
    // they must not share state with the other tests' config.
    let dir = std::env::temp_dir().join(format!("aishe-cli-set-{}", std::process::id()));
    let cfg_dir = dir.join("aishe");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("config.toml"),
        r#"[aishe]
mode = "suggest"
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-x"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-x"
"#,
    )
    .unwrap();
    let model_file = dir.join("pty-model");

    let run = |args: &[&str]| {
        let mut c = Command::cargo_bin("aishe").unwrap();
        c.env("XDG_CONFIG_HOME", &dir)
            .env("XDG_DATA_HOME", dir.join("data"))
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .env("AISHE_MODEL_FILE", &model_file)
            .args(args);
        c
    };

    // Show current values.
    run(&["mode"])
        .assert()
        .success()
        .stdout(contains("mode: suggest"));
    run(&["provider"])
        .assert()
        .success()
        .stdout(contains("provider: anthropic"));

    // Set and persist.
    run(&["mode", "auto"])
        .assert()
        .success()
        .stdout(contains("saved to"));
    run(&["provider", "openai"]).assert().success();
    // `model` targets the now-active provider (openai).
    run(&["model", "gpt-z2"]).assert().success();
    assert_eq!(std::fs::read_to_string(&model_file).unwrap(), "gpt-z2");

    // The effective config reflects the persisted changes.
    run(&["config"])
        .assert()
        .success()
        .stdout(contains("mode = \"auto\""))
        .stdout(contains("provider = \"openai\""))
        .stdout(contains("gpt-z2"));

    // Inspectors work without any custom commands/skills configured.
    run(&["commands"])
        .assert()
        .success()
        .stdout(contains("custom slash-commands: none").or(contains("AIShe")));
    run(&["skills"])
        .assert()
        .success()
        .stdout(contains("aishe-product"));

    // Clap rejects an invalid mode.
    run(&["mode", "bogus"]).assert().failure();

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn profile_changes_are_transparent_and_readiness_json_is_stable() {
    let dir = temp_root("profile-readiness");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        r#"version = 2

[aishe]
safety_profile = "custom"
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "http://127.0.0.1:9"
api_key_env = "UNUSED_LOCAL_KEY"
model = "local-readiness-model"
transport = "chat"
auth_required = false
"#,
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .args(args);
        command
    };

    run(&["profile", "balanced"])
        .assert()
        .success()
        .stdout(contains("profile = balanced"))
        .stdout(contains("mode: suggest → auto"))
        .stdout(contains("yolo_confirm: dangerous → writes"));
    run(&["readiness", "--json"])
        .assert()
        .failure()
        .stdout(contains("\"schema_version\": 1"))
        .stdout(contains("\"ready\": false"))
        .stdout(contains("\"id\": \"provider_tools\""))
        .stdout(contains("\"id\": \"sandbox\""))
        .stdout(contains("\"id\": \"redaction\""));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn undo_restores_a_recorded_file_change() {
    // Seed an undo journal by hand (the format the file tools write) and confirm
    // `aishe undo` restores the file's prior contents. Uses the AISHE_UNDO_JOURNAL
    // override via the child's env, so it never touches the real journal.
    let dir = std::env::temp_dir().join(format!("aishe-cli-undo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("undo.jsonl");
    let target = dir.join("f.txt");
    std::fs::write(&target, "MODIFIED BY AI").unwrap();
    let rec = format!(
        r#"{{"kind":"change","batch":"b1","ts":1,"path":"{}","existed":true,"before":"ORIGINAL","tool":"write_file","summary":"write f.txt"}}"#,
        target.display()
    );
    std::fs::write(&journal, format!("{rec}\n")).unwrap();

    // `aishe undo --list` shows the recorded batch.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .args(["undo", "--list"])
        .assert()
        .success()
        .stdout(contains("b1"));

    // `aishe undo` restores the prior contents.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .arg("undo")
        .assert()
        .success()
        .stdout(contains("restored"));
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ORIGINAL");

    // A second undo has nothing left (the batch is now marked reverted).
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_UNDO_JOURNAL", &journal)
        .arg("undo")
        .assert()
        .success()
        .stdout(contains("nothing to undo"));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn log_and_usage_read_the_audit_log() {
    // Seed an audit log and confirm `aishe log` / `aishe usage` read it via the
    // AISHE_LOG_FILE override (child env only; never touches a real log).
    let dir = std::env::temp_dir().join(format!("aishe-cli-log-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(
        &log,
        "{\"ts_ms\":1781304002000,\"session\":\"s1\",\"kind\":\"ai_response\",\"connection_id\":\"openai-work\",\"model\":\"gpt-4o\",\"tokens_in\":1000,\"tokens_out\":200,\"summary\":\"ok\"}\n\
         {\"ts_ms\":1781304002500,\"session\":\"s1\",\"kind\":\"ai_response\",\"connection_id\":\"openai-personal\",\"model\":\"gpt-4o\",\"tokens_in\":50,\"tokens_out\":10,\"summary\":\"ok\"}\n\
         {\"ts_ms\":1781304003000,\"session\":\"s1\",\"kind\":\"action\",\"source\":\"yolo\",\"command\":\"apt-get install nginx\",\"exit\":0}\n\
         {\"ts_ms\":1781304004000,\"session\":\"s1\",\"kind\":\"tool_call\",\"backend_session\":\"ses_backend\",\"message_id\":\"msg_1\",\"call_id\":\"call_1\",\"tool\":\"write_file\",\"path\":\"README.md\"}\n\
         {\"ts_ms\":1781304005000,\"session\":\"s1\",\"kind\":\"tool_result\",\"backend_session\":\"ses_backend\",\"message_id\":\"msg_1\",\"call_id\":\"call_1\",\"tool\":\"write_file\",\"success\":true,\"duration_ms\":12,\"output\":\"Wrote README.md\"}\n",
    )
    .unwrap();

    // `aishe log` shows both entries.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .arg("log")
        .assert()
        .success()
        .stdout(
            contains("apt-get install nginx")
                .and(contains("gpt-4o"))
                .and(contains("tool write_file")),
        );

    let jsonl = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["log", "--json"])
        .output()
        .unwrap();
    assert!(jsonl.status.success());
    assert!(jsonl.stderr.is_empty());
    let jsonl = String::from_utf8(jsonl.stdout).unwrap();
    assert!(!jsonl.contains('\u{1b}'));
    assert!(jsonl.lines().all(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .is_ok_and(|event| event["schema_version"] == 1)
    }));

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["log", "--action", "does-not-exist", "--json"])
        .assert()
        .success()
        .stdout(predicates::str::is_empty())
        .stderr(contains("no matching audit entries"));

    // `aishe log --action action` filters to the command.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["log", "--action", "action"])
        .assert()
        .success()
        .stdout(contains("apt-get install nginx"));

    // Managed backend session IDs are first-class filters, independent of the
    // short-lived audit-writer process session.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["log", "--session", "ses_backend"])
        .assert()
        .success()
        .stdout(contains("tool write_file").and(contains("12ms")));

    // The primary `/log` alias is intentionally concise and reads the same
    // file without requiring the interactive shell.
    let slash_home = temp_config_home();
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &slash_home)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["-c", "/log"])
        .assert()
        .success()
        .stdout(contains("apt-get install nginx").and(contains("gpt-4o")));
    std::fs::remove_dir_all(slash_home).ok();

    // `aishe usage` totals tokens and estimates cost (gpt-4o known price).
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .arg("usage")
        .assert()
        .success()
        .stdout(
            contains("1050 in")
                .and(contains("~$"))
                .and(contains("TOTAL")),
        );

    // Connection grouping preserves attribution even when the model is the same.
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["usage", "--by", "connection"])
        .assert()
        .success()
        .stdout(
            contains("usage by connection:")
                .and(contains("openai-work"))
                .and(contains("openai-personal")),
        );

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["usage", "--connection", "openai-personal"])
        .assert()
        .success()
        .stdout(contains("50 in").and(contains("1050 in").not()));

    assert!(!dir
        .join("config")
        .join("aishe")
        .join("config.toml")
        .exists());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runbook_generates_script_and_markdown() {
    let dir = std::env::temp_dir().join(format!("aishe-cli-rb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let log = dir.join("audit.jsonl");
    std::fs::write(
        &log,
        "{\"ts_ms\":1781304001000,\"session\":\"sx\",\"kind\":\"ai_request\",\"mode\":\"yolo\",\"model\":\"gpt-4o\",\"prompt\":\"install nginx\"}\n\
         {\"ts_ms\":1781304003000,\"session\":\"sx\",\"kind\":\"action\",\"source\":\"yolo:run_command\",\"command\":\"apt-get install -y nginx\",\"exit\":0}\n",
    )
    .unwrap();

    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", dir.join("config"))
        .env("AISHE_DATA_DIR", dir.join("data"))
        .env("AISHE_LOG_FILE", &log)
        .args(["runbook", "-o", dir.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("runbook-sx.sh").and(contains("runbook-sx.md")));

    let sh = std::fs::read_to_string(dir.join("runbook-sx.sh")).unwrap();
    assert!(sh.starts_with("#!/usr/bin/env bash"));
    assert!(sh.contains("apt-get install -y nginx"));
    assert!(sh.contains("install nginx")); // request in the header

    let md = std::fs::read_to_string(dir.join("runbook-sx.md")).unwrap();
    assert!(md.contains("# Runbook: install nginx"));
    assert!(md.contains("`apt-get install -y nginx`"));
    assert!(md.contains("## Reproduce"));
    assert!(!dir
        .join("config")
        .join("aishe")
        .join("config.toml")
        .exists());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn missing_config_in_non_tty_mode_is_actionable_and_does_not_write_defaults() {
    let dir = temp_root("missing-config");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", dir.join("data"))
        .arg("-c")
        .arg("!true")
        .assert()
        .failure()
        .stderr(contains("aishe setup --non-interactive"));
    assert!(!dir.join("aishe").join("config.toml").exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn noninteractive_setup_is_rerunnable_and_preserves_existing_fields() {
    let dir = temp_root("setup");
    let data = dir.join("data");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "version = 4\n\n[backend]\nengine = \"native\"\n",
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    run(&[
        "setup",
        "--non-interactive",
        "--service",
        "ollama",
        "--model",
        "local-test-model",
        "--profile",
        "balanced",
        "--sandbox",
        "policy",
        "--input-price",
        "0.25",
        "--output-price",
        "0.75",
    ])
    .assert()
    .success()
    .stdout(contains("Saved config"));

    let config_path = dir.join("aishe").join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str("\n[named_dirs]\nimportant = \"/srv/keep-me\"\n");
    std::fs::write(&config_path, config).unwrap();

    // With an existing config, omitting --service must preserve provider
    // endpoint/auth/transport and unrelated tables while allowing an override.
    run(&[
        "setup",
        "--non-interactive",
        "--model",
        "local-test-model-v2",
    ])
    .assert()
    .success();
    let updated = std::fs::read_to_string(&config_path).unwrap();
    assert!(updated.contains("base_url = \"http://localhost:11434\""));
    assert!(updated.contains("auth_required = false"));
    assert!(updated.contains("model = \"local-test-model-v2\""));
    assert!(updated.contains("important = \"/srv/keep-me\""));
    assert!(updated.contains("[pricing.local-test-model]"));
    run(&[
        "setup",
        "--non-interactive",
        "--model",
        "local-test-model-v3",
    ])
    .assert()
    .success();
    let backup_texts: Vec<String> = std::fs::read_dir(dir.join("aishe"))
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".setup."))
        .map(|entry| std::fs::read_to_string(entry.path()).unwrap())
        .collect();
    assert_eq!(
        backup_texts.len(),
        3,
        "rapid setup applies must create distinct backups"
    );
    assert!(
        backup_texts
            .iter()
            .any(|text| text.contains("model = \"local-test-model-v2\"")),
        "second setup state was not preserved in its own backup"
    );
    assert!(
        backup_texts
            .iter()
            .any(|text| text.contains("model = \"local-test-model\"")),
        "original setup state was not preserved in its own backup"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn price_commands_persist_exact_model_rates_and_validate_values() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .args(args);
        command
    };
    run(&[
        "price",
        "set",
        "gpt-5.6-luna",
        "--input",
        "1.125",
        "--output",
        "7.25",
    ])
    .assert()
    .success()
    .stdout(contains("gpt-5.6-luna"));
    run(&["price", "list"])
        .assert()
        .success()
        .stdout(contains("input $1.125000").and(contains("output $7.250000")));
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("gpt-5.6-luna"));
    run(&["price", "set", "bad", "--input=-1", "--output", "2"])
        .assert()
        .failure()
        .stderr(contains("non-negative"));
    run(&["price", "remove", "gpt-5.6-luna"]).assert().success();
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(!persisted.contains("gpt-5.6-luna"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let config_path = dir.join("aishe").join("config.toml");
    let mut config = std::fs::read_to_string(&config_path).unwrap();
    config.push_str(
        r#"
[mcp_servers.private]
command = "private-tool"

[mcp_servers.private.env]
TOKEN = "sk-proj-this-is-only-a-fake-test-secret"

[mcp_servers.private.headers]
Authorization = "Bearer fake-private-header"
"#,
    );
    std::fs::write(&config_path, config).unwrap();
    let bundle = dir.join("support.json");
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", &data)
        .env("ANTHROPIC_API_KEY", "fake-test-key")
        .args([
            "doctor",
            "--json",
            "--fix",
            "--bundle",
            bundle.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<&str> = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|check| check["id"].as_str())
        .collect();
    assert!(ids.contains(&"config.file"));
    assert!(ids.contains(&"provider.credential"));
    assert!(ids.contains(&"history.persistence"));
    assert!(ids.contains(&"repair.safe"));
    let config_check = report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "config.file")
        .unwrap();
    assert!(
        config_check["detail"]
            .as_str()
            .unwrap()
            .contains("schema 1 on disk"),
        "Doctor must report the source schema rather than serde's current-schema default"
    );

    let support = std::fs::read_to_string(&bundle).unwrap();
    assert!(!support.contains("sk-proj-this-is-only-a-fake-test-secret"));
    assert!(!support.contains("Bearer fake-private-header"));
    assert!(support.contains("<redacted>"));
    assert!(support.contains("\"command history\""));

    // The safe repair path is idempotent.
    let second = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &dir)
        .env("AISHE_DATA_DIR", &data)
        .env("ANTHROPIC_API_KEY", "fake-test-key")
        .args(["doctor", "--json", "--fix"])
        .output()
        .unwrap();
    assert!(second.status.success());
    let second: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    let repair = second["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "repair.safe")
        .unwrap();
    assert_eq!(
        repair["changed_paths"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0),
        0,
        "second Doctor repair was not idempotent: {repair}"
    );
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn effective_config_and_context_json_are_structured_and_content_free() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let project = dir.join("project");
    std::fs::create_dir_all(project.join(".aishe")).unwrap();
    let fake_secret = "sk-proj-fake-context-secret-abcdefghijklmnopqrstuvwxyz";
    std::fs::write(
        project.join(".aishe").join("context.md"),
        format!("never expose {fake_secret}\n"),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .current_dir(&project)
            .args(args);
        command
    };
    let output = run(&["config", "--effective", "--json"]).output().unwrap();
    assert!(output.status.success());
    let effective: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(effective["schema_version"], 1);
    assert!(effective["config"].is_object());
    assert!(effective["provenance"]["fields"].is_array());

    let output = run(&["config", "--json"]).output().unwrap();
    assert!(output.status.success());
    let raw_config: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(raw_config["schema_version"], 1);
    assert!(raw_config["config"]["version"].as_u64().is_some());

    let request = "private request text must not be echoed";
    let output = run(&["context", "--preview", request, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let raw = String::from_utf8(output.stdout).unwrap();
    assert!(!raw.contains(request));
    assert!(!raw.contains(fake_secret));
    let preview: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(preview["schema_version"], 1);
    assert!(preview["sections"].is_array());
    assert!(preview["total_estimated_tokens"].as_u64().is_some());

    run(&["context", "--exclude", "project_context", "--json"])
        .assert()
        .success();
    let persisted = std::fs::read_to_string(dir.join("aishe").join("config.toml")).unwrap();
    assert!(persisted.contains("project_context"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn noninteractive_tour_is_isolated_resumable_and_proves_undo() {
    let dir = temp_root("tour");
    let cwd = dir.join("invocation");
    std::fs::create_dir_all(&cwd).unwrap();
    let sentinel = cwd.join("keep.txt");
    std::fs::write(&sentinel, "unchanged").unwrap();
    let run = || {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", dir.join("config"))
            .env("AISHE_DATA_DIR", dir.join("data"))
            .current_dir(&cwd)
            .args(["tour", "--non-interactive"]);
        command
    };
    run()
        .assert()
        .success()
        .stdout(contains("Tour complete").and(contains("proved undo")));
    assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "unchanged");
    assert!(!dir.join("data/aishe/tour/workspace/undo-demo.txt").exists());
    run()
        .assert()
        .success()
        .stdout(contains("tour is complete"));
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("data/aishe/tour/state.json")).unwrap())
            .unwrap();
    assert_eq!(state["completed"], true);
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn durable_task_cli_lifecycle_is_private_and_redacted() {
    let dir = temp_config_home();
    let data = dir.join("data");
    let fake_secret = "sk-proj-fake-task-secret-abcdefghijklmnopqrstuvwxyz";
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", &data)
            .env("AISHE_FAKE_LLM", "task complete")
            .args(args);
        command
    };
    run(&["--yolo-line", &format!("summarize {fake_secret}")])
        .assert()
        .success()
        .stdout(contains("task complete"));
    let listing = run(&["sessions", "--json"]).output().unwrap();
    assert!(listing.status.success());
    let records: serde_json::Value = serde_json::from_slice(&listing.stdout).unwrap();
    assert_eq!(records["schema_version"], 1);
    let record = &records["legacy"].as_array().unwrap()[0];
    let id = record["id"].as_str().unwrap();
    assert_eq!(record["status"], "completed");
    let task_path = data.join("aishe").join("tasks").join(format!("{id}.json"));
    let task_text = std::fs::read_to_string(&task_path).unwrap();
    assert!(!task_text.contains(fake_secret));
    assert!(task_text.contains("<redacted>"));

    run(&["session", "show", id, "--json"])
        .assert()
        .success()
        .stdout(contains("\"status\": \"completed\""));
    run(&["session", "rename", id, "deployment check"])
        .assert()
        .success();
    run(&["session", "delete", id]).assert().success();
    assert!(!task_path.exists());
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn provider_test_and_model_listing_support_local_unauthenticated_endpoints() {
    let dir = temp_root("provider-test");
    let config_dir = dir.join("aishe");
    std::fs::create_dir_all(&config_dir).unwrap();
    let (endpoint, server) = serve_model_catalog(3);
    std::fs::write(
        config_dir.join("config.toml"),
        format!(
            r#"version = 2

[aishe]
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "{endpoint}"
api_key_env = "LOCAL_UNUSED_KEY"
model = "local-model-a"
transport = "chat"
auth_required = false
"#
        ),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", &dir)
            .env("AISHE_DATA_DIR", dir.join("data"))
            .env_remove("LOCAL_UNUSED_KEY")
            .args(args);
        command
    };
    let output = run(&["provider", "test", "--json"]).output().unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 2);
    assert_eq!(report["credential"]["state"], "pass");
    assert_eq!(report["credential_required"], false);
    assert_eq!(report["model_available"]["state"], "pass");
    assert_eq!(report["text"]["state"], "skipped");

    let output = run(&["models", "--provider", "openai", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let models: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(models["schema_version"], 1);
    assert_eq!(
        models["models"],
        serde_json::json!(["local-model-a", "local-model-b"])
    );
    server.join().unwrap();
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn backend_status_json_is_versioned_and_never_serializes_private_tokens() {
    let root = temp_root("backend-status");
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_DATA_DIR", root.join("data"))
        .env("AISHE_RUNTIME_DIR", root.join("runtime"))
        .args(["backend", "status", "--json"])
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a missing runtime must remain nonzero"
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["schema_version"], 1);
    assert_eq!(report["supervisor"]["state"], "stopped");
    assert!(report.get("runtime").is_some());
    let serialized = String::from_utf8(output.stdout).unwrap();
    for forbidden in [
        "control_token",
        "opencode_password",
        "startup_nonce",
        "control_url",
        "opencode_url",
    ] {
        assert!(!serialized.contains(forbidden));
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn uninstall_preview_defaults_to_replaceable_components_and_preserves_state() {
    let dir = temp_root("uninstall");
    let config_dir = dir.join("config").join("aishe");
    let data_dir = dir.join("data").join("aishe");
    std::fs::create_dir_all(data_dir.join("runtime/opencode/test")).unwrap();
    std::fs::create_dir_all(data_dir.join("tasks")).unwrap();
    std::fs::create_dir_all(data_dir.join("backend/opencode/xdg/data/opencode")).unwrap();
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "[aishe]\nmode = \"suggest\"\n",
    )
    .unwrap();
    std::fs::write(config_dir.join("credentials.toml"), "version = 1\n").unwrap();
    std::fs::write(data_dir.join("history.ext"), ": 1:0;echo preserved\n").unwrap();
    std::fs::write(data_dir.join("tasks/task.json"), "{}").unwrap();
    for name in ["audit.jsonl", "audit.jsonl.1", "undo.jsonl", "undo.jsonl.1"] {
        std::fs::write(data_dir.join(name), "{}\n").unwrap();
    }
    let oauth = data_dir.join("backend/opencode/xdg/data/opencode/auth.json");
    std::fs::write(&oauth, "{\"openai\":{}}").unwrap();

    let run = |args: &[&str]| {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("AISHE_CONFIG_DIR", dir.join("config"))
            .env("AISHE_DATA_DIR", dir.join("data"))
            .args(args);
        command
    };

    run(&["uninstall", "--dry-run"]).assert().success().stdout(
        contains("managed runtime/cache")
            .and(contains("No files were changed"))
            .and(predicates::str::is_match("shell history").unwrap().not()),
    );
    for path in [
        config_dir.join("config.toml"),
        config_dir.join("credentials.toml"),
        data_dir.join("history.ext"),
        data_dir.join("tasks/task.json"),
        oauth.clone(),
        data_dir.join("runtime/opencode/test"),
    ] {
        assert!(path.exists(), "{} was changed by dry-run", path.display());
    }

    run(&["uninstall", "--history"])
        .assert()
        .failure()
        .stderr(contains("confirmation required"));
    assert!(data_dir.join("history.ext").exists());

    run(&["uninstall", "--runtime", "--yes"])
        .assert()
        .success()
        .stdout(contains("User state").and(contains("preserved")));
    assert!(!data_dir.join("runtime").exists());
    for path in [
        config_dir.join("config.toml"),
        config_dir.join("credentials.toml"),
        data_dir.join("history.ext"),
        data_dir.join("tasks/task.json"),
        oauth.clone(),
    ] {
        assert!(path.exists(), "{} was not preserved", path.display());
    }

    run(&["uninstall", "--all", "--dry-run"])
        .assert()
        .success()
        .stdout(
            contains("config/credentials")
                .and(contains("shell history"))
                .and(contains("AI sessions/tool journals"))
                .and(contains("audit/undo data")),
        );

    run(&["uninstall", "--sessions", "--yes"])
        .assert()
        .success()
        .stdout(contains("AI sessions/tool journals"));
    assert!(!data_dir.join("tasks").exists());
    assert!(
        oauth.exists(),
        "session deletion must preserve OAuth credentials"
    );

    run(&["uninstall", "--config", "--yes"])
        .assert()
        .success()
        .stdout(contains("config/credentials"));
    assert!(
        !oauth.exists(),
        "credential deletion must remove managed OAuth state"
    );

    run(&["uninstall", "--audit-undo", "--yes"])
        .assert()
        .success()
        .stdout(contains("audit/undo data"));
    for name in ["audit.jsonl", "audit.jsonl.1", "undo.jsonl", "undo.jsonl.1"] {
        let path = data_dir.join(name);
        assert!(!path.exists(), "{} was not removed", path.display());
    }

    std::fs::remove_dir_all(dir).ok();
}
