//! Static gate for every public JSON/JSONL CLI path.
//!
//! This deliberately reads the two Clap source files: adding a `json: bool`
//! flag changes the declaration count and fails until the command is entered in
//! `PUBLIC_SURFACES` with an explicit nonzero schema version.

use std::collections::BTreeSet;

use aishe::cli::json_contract::{Format, PUBLIC_SURFACES};

const CLI_ARGS: &str = include_str!("../src/cli/args.rs");
const AUTH_ARGS: &str = include_str!("../src/auth.rs");

fn public_json_flag_count(source: &str) -> usize {
    source
        .lines()
        .map(str::trim)
        .filter(|line| *line == "json: bool," || *line == "pub(crate) json: bool,")
        .count()
}

#[test]
fn every_public_json_flag_has_a_versioned_inventory_entry() {
    let declared = public_json_flag_count(CLI_ARGS) + public_json_flag_count(AUTH_ARGS);
    assert_eq!(
        declared,
        PUBLIC_SURFACES.len(),
        "a public JSON flag was added or removed; update PUBLIC_SURFACES and choose an explicit schema"
    );

    let mut commands = BTreeSet::new();
    for surface in PUBLIC_SURFACES {
        assert!(
            surface.schema_version > 0,
            "{} is still an unversioned public JSON surface",
            surface.command
        );
        assert!(
            surface.command.ends_with("--json"),
            "inventory paths must identify their public JSON flag: {}",
            surface.command
        );
        assert!(
            commands.insert(surface.command),
            "duplicate public JSON inventory entry: {}",
            surface.command
        );
    }
    assert_eq!(
        PUBLIC_SURFACES
            .iter()
            .find(|surface| surface.command == "provider test --json")
            .unwrap()
            .schema_version,
        aishe::capabilities::CACHE_SCHEMA_VERSION,
    );
}

#[test]
fn only_the_audit_log_owns_a_jsonl_stream() {
    let jsonl: Vec<_> = PUBLIC_SURFACES
        .iter()
        .filter(|surface| surface.format == Format::JsonLines)
        .map(|surface| surface.command)
        .collect();
    assert_eq!(jsonl, ["log --json"]);
}

#[test]
fn machine_output_match_covers_every_json_flag_declaration() {
    let machine_output = CLI_ARGS
        .split("impl Args {")
        .nth(1)
        .expect("Args::machine_output must remain present");
    assert_eq!(
        machine_output.matches("{ json").count(),
        PUBLIC_SURFACES.len() - 1,
        "every public JSON flag except SetupArgs must be routed through machine_output"
    );
    assert!(machine_output.contains("Some(Cmd::Setup(setup)) => setup.json"));
    assert!(machine_output.contains("AuthCommand::Status { json, .. }"));
    assert!(machine_output.contains("AuthCommand::List { json }"));
}
