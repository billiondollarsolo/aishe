//! Static gate for every public JSON/JSONL CLI path.
//!
//! This deliberately reads the two Clap source files: adding a `json: bool`
//! flag changes the declaration count and fails until the command is entered in
//! `PUBLIC_SURFACES` with an explicit nonzero schema version.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use aishe::cli::json_contract::{Format, PUBLIC_SURFACES};
use assert_cmd::Command;
use serde_json::Value;

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
    // The capability report is nested under `provider` in `aishe test --json`
    // now that `provider test` is gone.
    assert_eq!(
        PUBLIC_SURFACES
            .iter()
            .find(|surface| surface.command == "test --json")
            .unwrap()
            .schema_version,
        1,
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
    assert!(machine_output.contains("Cmd::Activate { json, .. }"));
    assert!(machine_output.contains("AuthCommand::Status { json, .. }"));
    assert!(machine_output.contains("AuthCommand::List { json }"));
}

struct ActivationFixture(PathBuf);

impl ActivationFixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aishe-activation-json-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("config/aishe")).unwrap();
        // Activation owns its JSON stream before configuration is loaded.
        // Even an unusable config must not print migration/setup notices.
        std::fs::write(root.join("config/aishe/config.toml"), "invalid = [").unwrap();
        Self(root)
    }

    fn command(&self, rcfile: &Path, operation: Option<&str>) -> Command {
        let mut command = Command::cargo_bin("aishe").unwrap();
        command
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_DATA_HOME", self.0.join("data"))
            .env("AISHE_CONFIG_DIR", self.0.join("config"))
            .env("AISHE_DATA_DIR", self.0.join("data"))
            .args(["activate", "zsh", "--json", "--rcfile"])
            .arg(rcfile);
        if let Some(operation) = operation {
            command.arg(operation);
        }
        command
    }

    fn document(&self, rcfile: &Path, operation: Option<&str>) -> Value {
        let output = self.command(rcfile, operation).output().unwrap();
        assert!(
            output.status.success(),
            "activation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "JSON success emitted stderr");
        // Parsing the whole stdout rejects additional documents and human
        // notices, rather than finding one JSON-looking line in mixed output.
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["schema_version"], 1);
        assert_eq!(document["shell"], "zsh");
        assert_eq!(document["path"], rcfile.to_str().unwrap());
        assert_eq!(document["login_shell_changed"], false);
        let fields: BTreeSet<_> = document
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            fields,
            BTreeSet::from([
                "schema_version",
                "operation",
                "shell",
                "path",
                "block",
                "changed",
                "backup",
                "login_shell_changed",
            ])
        );
        document
    }
}

impl Drop for ActivationFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn activation_json_is_one_versioned_document_for_preview_apply_and_remove() {
    let surface = PUBLIC_SURFACES
        .iter()
        .find(|surface| surface.command == "activate --json")
        .expect("activation JSON must be inventoried");
    assert_eq!(surface.format, Format::Json);
    assert_eq!(surface.schema_version, 1);

    let fixture = ActivationFixture::new();
    let rcfile = fixture.0.join("startup with spaces.zsh");
    let original = "alias preserved='printf retained'\n";
    std::fs::write(&rcfile, original).unwrap();
    let preview = fixture.document(&rcfile, None);
    assert_eq!(preview["operation"], "preview");
    assert_eq!(preview["changed"], false);
    assert!(preview["backup"].is_null());
    let block = preview["block"].as_str().unwrap();
    assert!(block.starts_with("# >>> AIShe native shell >>>\n"));
    assert!(block.ends_with("# <<< AIShe native shell <<<\n"));
    assert!(block.contains("exec "));
    assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);

    let applied = fixture.document(&rcfile, Some("--apply"));
    assert_eq!(applied["operation"], "apply");
    assert_eq!(applied["changed"], true);
    assert_eq!(applied["block"], preview["block"]);
    let backup = applied["backup"].as_str().unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
    assert_eq!(
        std::fs::read_to_string(&rcfile).unwrap(),
        format!("{block}{original}")
    );
    let unchanged = fixture.document(&rcfile, Some("--apply"));
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged["backup"].is_null());

    let removed = fixture.document(&rcfile, Some("--remove"));
    assert_eq!(removed["operation"], "remove");
    assert_eq!(removed["changed"], true);
    assert!(removed["backup"].is_string());
    assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), original);
    let unchanged = fixture.document(&rcfile, Some("--remove"));
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged["backup"].is_null());
}

#[test]
fn activation_json_runtime_errors_keep_stdout_empty_and_serialize_stderr() {
    let fixture = ActivationFixture::new();
    let rcfile = fixture.0.join("malformed.zsh");
    let original = "# >>> AIShe native shell >>>\n# unterminated managed block\n";
    std::fs::write(&rcfile, original).unwrap();
    let output = fixture.command(&rcfile, Some("--apply")).output().unwrap();
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "failed activation emitted a stdout document"
    );
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["schema_version"], 1);
    assert_eq!(error["exit_code"], output.status.code().unwrap());
    assert!(error["code"].is_string());
    assert!(error["message"].is_string());
    assert!(error["retryable"].is_boolean());
    assert_eq!(std::fs::read_to_string(&rcfile).unwrap(), original);
    assert_eq!(std::fs::read_dir(&fixture.0).unwrap().count(), 2);
}
