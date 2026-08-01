use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use aishe::dispatcher::{self, CommandCache};
use assert_cmd::prelude::*;
use predicates::prelude::*;
use serde::Deserialize;

const CORPUS: &str = include_str!("fixtures/routing/v1.json");
const TYPO_CORPUS: &str = include_str!("fixtures/routing/typo-assistance-v1.json");
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Deserialize)]
struct Corpus {
    schema_version: u32,
    normative: Vec<RouteCase>,
    research: Vec<RouteCase>,
}

#[derive(Debug, Deserialize)]
struct RouteCase {
    id: String,
    input: String,
    expected: String,
    reason: String,
    platform: Vec<String>,
    known_commands: Vec<String>,
    aliases_functions: Vec<String>,
    critical: bool,
    notes: String,
}

#[derive(Debug, Deserialize)]
struct TypoCorpus {
    schema_version: u32,
    false_positive_budget_percent: f64,
    cases: Vec<TypoCase>,
}

#[derive(Debug, Deserialize)]
struct TypoCase {
    id: String,
    label: String,
    input: String,
    known_commands: Vec<String>,
    expected_candidate: Option<String>,
}

fn corpus() -> Corpus {
    serde_json::from_str(CORPUS).expect("routing v1 fixture must be valid JSON")
}

fn typo_corpus() -> TypoCorpus {
    serde_json::from_str(TYPO_CORPUS).expect("typo-assistance v1 fixture must be valid JSON")
}

fn assert_case(case: &RouteCase) {
    let cache = CommandCache::new();
    let names: Vec<&str> = case
        .known_commands
        .iter()
        .chain(&case.aliases_functions)
        .map(String::as_str)
        .collect();
    cache.insert_all(&names);
    let actual = dispatcher::route(&case.input, &cache);
    assert_eq!(
        actual.kind.to_string(),
        case.expected,
        "kind for {}",
        case.id
    );
    assert_eq!(
        actual.reason.to_string(),
        case.reason,
        "reason for {}",
        case.id
    );
}

#[test]
fn v1_corpus_is_well_formed_and_has_unique_ids() {
    let corpus = corpus();
    assert_eq!(corpus.schema_version, 1);
    assert!(
        corpus.normative.len() >= 40,
        "v1 needs broad normative coverage"
    );
    assert!(!corpus.research.is_empty());

    let mut ids = HashSet::new();
    for case in corpus.normative.iter().chain(&corpus.research) {
        assert!(ids.insert(&case.id), "duplicate route id {}", case.id);
        assert!(
            !case.notes.trim().is_empty(),
            "missing notes for {}",
            case.id
        );
        assert!(
            !case.platform.is_empty()
                && case
                    .platform
                    .iter()
                    .all(|name| matches!(name.as_str(), "linux" | "macos")),
            "invalid platform metadata for {}",
            case.id
        );
    }
}

#[test]
fn normative_v1_corpus_matches_the_rust_classifier_on_every_host() {
    // Cases declare command/alias/function evidence, so macOS-origin and
    // Linux-origin collisions run identically on both CI platforms.
    for case in corpus().normative {
        assert_case(&case);
    }
}

#[test]
fn research_cases_are_separate_but_characterized() {
    for case in corpus().research {
        assert!(
            !case.critical,
            "research case {} cannot be critical",
            case.id
        );
        assert_case(&case);
    }
}

#[test]
fn critical_natural_language_cases_have_zero_false_shell_routes() {
    let corpus = corpus();
    let cases: Vec<_> = corpus
        .normative
        .iter()
        .filter(|case| case.critical && case.expected == "natural_language")
        .collect();
    assert!(cases.len() >= 8, "critical NL set is too small");
    for case in cases {
        let cache = CommandCache::new();
        let names: Vec<&str> = case
            .known_commands
            .iter()
            .chain(&case.aliases_functions)
            .map(String::as_str)
            .collect();
        cache.insert_all(&names);
        let actual = dispatcher::route(&case.input, &cache);
        assert_ne!(
            actual.kind.to_string(),
            "shell",
            "false shell for {}",
            case.id
        );
    }
}

#[test]
fn typo_assistance_v1_is_advisory_and_meets_false_positive_budget() {
    let corpus = typo_corpus();
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(corpus.false_positive_budget_percent, 1.0);
    assert!(corpus.cases.len() >= 20);

    let mut ids = HashSet::new();
    let mut prose_cases = 0usize;
    let mut false_positives = 0usize;
    for case in corpus.cases {
        assert!(
            ids.insert(case.id.clone()),
            "duplicate typo case {}",
            case.id
        );
        assert!(matches!(case.label.as_str(), "typo" | "natural_language"));
        let cache = CommandCache::new();
        let names: Vec<&str> = case.known_commands.iter().map(String::as_str).collect();
        cache.insert_all(&names);

        let before = dispatcher::route(&case.input, &cache);
        let actual = dispatcher::typo_assistance(&case.input, &cache);
        let emitted_candidate = actual.is_some();
        let after = dispatcher::route(&case.input, &cache);
        assert_eq!(before, after, "assistance changed route for {}", case.id);
        assert_eq!(
            actual.as_ref().map(|cue| cue.candidate.clone()),
            case.expected_candidate,
            "candidate for {}",
            case.id
        );
        if let Some(cue) = actual {
            assert!(
                !cue.executes_automatically,
                "cue could execute for {}",
                case.id
            );
            assert_eq!(cue.schema_version, 1);
            assert_eq!(cue.edit_distance, 1);
        }
        if case.label == "natural_language" {
            prose_cases += 1;
            false_positives += usize::from(emitted_candidate);
        }
    }
    let false_positive_percent = false_positives as f64 * 100.0 / prose_cases as f64;
    assert!(
        false_positive_percent <= corpus.false_positive_budget_percent,
        "false-positive rate {false_positive_percent:.2}% exceeds {:.2}%",
        corpus.false_positive_budget_percent
    );
}

fn fixture_path(command_name: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "aishe-route-fixture-{}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    let executable = path.join(command_name);
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(executable, permissions).unwrap();
    path
}

#[test]
fn route_text_explains_a_path_fixture_collision_and_override() {
    let path = fixture_path("install");
    Command::cargo_bin("aishe")
        .unwrap()
        .env("PATH", &path)
        .args(["route", "--", "install kubectl please"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("route: shell")
                .and(predicate::str::contains("reason: known_command"))
                .and(predicate::str::contains(
                    "effective head: install (known command: yes)",
                ))
                .and(predicate::str::contains("ambiguous command phrase: yes"))
                .and(predicate::str::contains(
                    "prefix ? to force the agent route",
                )),
        )
        .stderr(predicate::str::is_empty());
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn route_json_is_schema_versioned_stable_and_ansi_free() {
    let path = fixture_path("install");
    let output = Command::cargo_bin("aishe")
        .unwrap()
        .env("PATH", &path)
        .args(["route", "--json", "--", "? install kubectl please"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!output.stdout.contains(&0x1b));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report,
        serde_json::json!({
            "schema_version": 1,
            "kind": "natural_language",
            "reason": "forced_agent",
            "normalized": "install kubectl please",
            "head": "install",
            "known_command": true,
            "ambiguous": true,
            "source": "explicit",
            "safety_bypass": false,
            "opposite_route_override": {
                "kind": "shell",
                "prefix": "!",
                "guidance": "prefix ! to force the shell route; this bypasses the AI safety gate",
                "safety_bypass": true
            }
        })
    );
    std::fs::remove_dir_all(path).unwrap();
}

#[test]
fn forced_shell_explanation_names_the_one_line_safety_bypass() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["route", "--", "! command"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("route: shell")
                .and(predicate::str::contains("reason: forced_shell"))
                .and(predicate::str::contains("applies to this line only"))
                .and(predicate::str::contains("bypasses the AI safety gate")),
        );
}

#[test]
fn legacy_hash_prefix_explains_its_canonical_replacement() {
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["route", "--", "# explain this repository"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("reason: forced_agent")
                .and(predicate::str::contains("deprecated agent prefix"))
                .and(predicate::str::contains("use ?"))
                .and(predicate::str::contains("AIShe 0.9")),
        );
}

#[test]
fn route_requires_a_line_and_never_materializes_config() {
    Command::cargo_bin("aishe")
        .unwrap()
        .arg("route")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("required"));
    Command::cargo_bin("aishe")
        .unwrap()
        .args(["route", "--", ""])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("needs a non-empty line"));

    let config_root = std::env::temp_dir().join(format!(
        "aishe-route-no-config-{}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    assert!(!config_root.exists());
    Command::cargo_bin("aishe")
        .unwrap()
        .env("AISHE_CONFIG_DIR", &config_root)
        .env("AISHE_DATA_DIR", config_root.join("data"))
        .args(["route", "--", "explain this repository"])
        .assert()
        .success()
        .stdout(predicate::str::contains("reason: unknown_input"));
    assert!(
        !config_root.exists(),
        "route inspection must not load or initialize config/backend state"
    );
}
