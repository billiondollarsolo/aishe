//! Backward-compatibility gates for public JSON and persisted records.
//!
//! Fixtures under `v0.5` and `v0.6` are the two prior product-minor shapes.
//! They predate explicit document versions, so their only migration is adding
//! `schema_version: 1`. Removing or changing a required v1 field makes these
//! tests fail and requires an intentional schema bump plus new fixtures.

use serde::Deserialize;
use serde_json::Value;

const SUGGEST_V1: &str = include_str!("fixtures/api/v1/suggest.json");
const STATUS_V1: &str = include_str!("fixtures/api/v1/status.json");

#[derive(Debug, Deserialize, PartialEq)]
struct SuggestV1 {
    schema_version: u32,
    kind: String,
    command: String,
    explanation: String,
    risk: String,
    reason: String,
}

fn value(document: &str) -> Value {
    serde_json::from_str(document).expect("compatibility fixture must be valid JSON")
}

fn migrate_unversioned_v1(document: &str) -> Value {
    let mut document = value(document);
    let object = document
        .as_object_mut()
        .expect("public compatibility document must be an object");
    assert!(
        object
            .insert("schema_version".into(), Value::from(1))
            .is_none(),
        "prior-minor fixture unexpectedly already has a schema version"
    );
    document
}

#[test]
fn suggest_v1_contract_and_prior_two_minor_shapes_are_compatible() {
    let current: SuggestV1 =
        serde_json::from_str(SUGGEST_V1).expect("suggest v1 fixture must deserialize");
    assert_eq!(current.schema_version, 1);
    assert!(matches!(current.kind.as_str(), "command" | "answer"));
    assert!(matches!(
        current.risk.as_str(),
        "safe" | "dangerous" | "unknown" | "n/a"
    ));

    let current_value = value(SUGGEST_V1);
    for prior in [
        include_str!("fixtures/api/v0.5/suggest.json"),
        include_str!("fixtures/api/v0.6/suggest.json"),
    ] {
        assert_eq!(migrate_unversioned_v1(prior), current_value);
    }
}

#[test]
fn status_v1_contract_and_prior_two_minor_shapes_are_compatible() {
    let current = value(STATUS_V1);
    assert_eq!(current["schema_version"], 1);
    for required in [
        "model",
        "connection",
        "mode",
        "backend",
        "scope",
        "network",
        "output",
        "reasoning_effort",
        "status_line",
        "budget_usd",
        "audit",
        "session",
        "metrics",
    ] {
        assert!(
            current.get(required).is_some(),
            "status v1 lost required field {required}"
        );
    }

    for prior in [
        include_str!("fixtures/api/v0.5/status.json"),
        include_str!("fixtures/api/v0.6/status.json"),
    ] {
        assert_eq!(migrate_unversioned_v1(prior), current);
    }
}

#[test]
fn structured_error_fixture_uses_the_shared_contract() {
    let raw = include_str!("fixtures/api/v1/suggest-error.json");
    assert!(!raw.contains('\u{1b}'));
    let error: aishe::user_error::UserError =
        serde_json::from_str(raw).expect("suggest error fixture must deserialize");
    assert_eq!(error.schema_version(), 1);
    assert_eq!(error.code().as_str(), "cli.missing_request");
    assert_eq!(error.exit_code(), 2);
}

#[test]
fn persisted_task_v1_fixture_stays_readable() {
    let raw = include_str!("fixtures/api/v1/task-record.json");
    let record: aishe::tasks::Record =
        serde_json::from_str(raw).expect("persisted task v1 fixture must deserialize");
    assert_eq!(record.schema_version, aishe::tasks::TASK_SCHEMA_VERSION);
    assert_eq!(record.id, "fixture-task");
    assert_eq!(record.usage.requests, 1);
}

#[test]
fn legacy_public_json_surfaces_have_lossless_v1_migrations() {
    let expected = value(include_str!("fixtures/api/v1/legacy-json-surfaces.json"));
    for legacy in [
        include_str!("fixtures/api/v0.5/legacy-json-surfaces.json"),
        include_str!("fixtures/api/v0.6/legacy-json-surfaces.json"),
    ] {
        let legacy = value(legacy);
        let mut migrated = serde_json::Map::new();
        for (name, document) in legacy
            .as_object()
            .expect("legacy public surface fixture must be an object")
        {
            let document = match name.as_str() {
                "backend_install" | "backend_repair" => {
                    aishe::cli::json_contract::envelope("runtime", document).unwrap()
                }
                "models" => aishe::cli::json_contract::envelope("models", document).unwrap(),
                "config" => aishe::cli::json_contract::envelope("config", document).unwrap(),
                "connection_list" => {
                    aishe::cli::json_contract::envelope("connections", document).unwrap()
                }
                "auth_list" => aishe::cli::json_contract::envelope("profiles", document).unwrap(),
                "audit_event" => {
                    aishe::cli::json_contract::normalize_legacy_object(document.clone())
                }
                _ => aishe::cli::json_contract::version_object(document).unwrap(),
            };
            migrated.insert(name.clone(), document);
        }
        assert_eq!(Value::Object(migrated), expected);
    }
}

#[test]
fn public_json_fixtures_are_ansi_free() {
    for raw in [
        SUGGEST_V1,
        STATUS_V1,
        include_str!("fixtures/api/v1/suggest-error.json"),
        include_str!("fixtures/api/v1/task-record.json"),
        include_str!("fixtures/api/v1/legacy-json-surfaces.json"),
    ] {
        assert!(!raw.contains('\u{1b}'));
    }
}
