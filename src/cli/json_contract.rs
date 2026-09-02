//! Public machine-readable CLI contracts.
//!
//! Keep this inventory aligned with every public `--json` declaration.  The
//! conformance test intentionally counts those declarations so a new JSON
//! surface cannot be introduced without choosing a schema and stream format.

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Format {
    Json,
    JsonLines,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Surface {
    pub command: &'static str,
    pub format: Format,
    pub schema_version: u32,
}

/// Every public command path that owns a JSON or JSONL stream.
///
/// A command with multiple runtime shapes (notably `auth status`) still has one
/// inventory row because one Clap `--json` declaration owns that contract.
pub const PUBLIC_SURFACES: &[Surface] = &[
    surface("setup --json", Format::Json),
    surface("settings --json", Format::Json),
    surface("doctor --json", Format::Json),
    surface_version("provider test --json", Format::Json, 2),
    surface("models --json", Format::Json),
    surface("readiness --json", Format::Json),
    surface("config --json", Format::Json),
    surface("route --json", Format::Json),
    surface("status --json", Format::Json),
    surface("log --json", Format::JsonLines),
    surface("suggest --json", Format::Json),
    surface("ask --json", Format::Json),
    surface("index --json", Format::Json),
    surface("palette --json", Format::Json),
    surface("sessions --json", Format::Json),
    surface("context --json", Format::Json),
    surface("connection list --json", Format::Json),
    surface("connection show --json", Format::Json),
    surface("hints status --json", Format::Json),
    surface("backend status --json", Format::Json),
    surface("backend install --json", Format::Json),
    surface("backend verify --json", Format::Json),
    surface("backend repair --json", Format::Json),
    surface("session show --json", Format::Json),
    surface("task list --json", Format::Json),
    surface("task show --json", Format::Json),
    surface("last show --json", Format::Json),
    surface("role list --json", Format::Json),
    surface("mcp list --json", Format::Json),
    surface("mcp show --json", Format::Json),
    surface("mcp test --json", Format::Json),
    surface("update check --json", Format::Json),
    surface("auth status --json", Format::Json),
    surface("auth list --json", Format::Json),
];

const fn surface(command: &'static str, format: Format) -> Surface {
    surface_version(command, format, SCHEMA_VERSION)
}

const fn surface_version(command: &'static str, format: Format, schema_version: u32) -> Surface {
    Surface {
        command,
        format,
        schema_version,
    }
}

/// Add the public document version to an existing object without moving or
/// renaming any of its legacy fields.
pub fn version_object<T: Serialize>(value: &T) -> Result<Value> {
    let value = serde_json::to_value(value).context("serializing public JSON document")?;
    let Value::Object(mut object) = value else {
        anyhow::bail!("public JSON object contract produced a non-object root");
    };
    insert_version(&mut object)?;
    Ok(Value::Object(object))
}

/// Wrap a legacy scalar/array/raw document under a meaning-bearing field.
pub fn envelope<T: Serialize>(field: &str, value: &T) -> Result<Value> {
    let mut object = Map::new();
    object.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
    object.insert(
        field.into(),
        serde_json::to_value(value).context("serializing public JSON envelope")?,
    );
    Ok(Value::Object(object))
}

pub fn print_object<T: Serialize>(value: &T) -> Result<()> {
    print_value(&version_object(value)?)
}

pub fn print_envelope<T: Serialize>(field: &str, value: &T) -> Result<()> {
    print_value(&envelope(field, value)?)
}

fn print_value(value: &Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Normalize a stored legacy object before it crosses the public JSONL
/// boundary. Existing audit files remain readable; newly written and replayed
/// records are explicit v1 documents.
pub fn normalize_legacy_object(mut value: Value) -> Value {
    if let Value::Object(object) = &mut value {
        if !object.contains_key("schema_version") {
            object.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
        }
    }
    value
}

fn insert_version(object: &mut Map<String, Value>) -> Result<()> {
    match object.get("schema_version") {
        Some(Value::Number(value)) if value.as_u64() == Some(u64::from(SCHEMA_VERSION)) => Ok(()),
        Some(_) => anyhow::bail!("public JSON object has an incompatible schema_version"),
        None => {
            object.insert("schema_version".into(), Value::from(SCHEMA_VERSION));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_versioning_is_additive_and_envelopes_arrays() {
        let object = version_object(&serde_json::json!({"ready": false})).unwrap();
        assert_eq!(object["schema_version"], 1);
        assert_eq!(object["ready"], false);

        let wrapped = envelope("models", &vec!["one", "two"]).unwrap();
        assert_eq!(wrapped["schema_version"], 1);
        assert_eq!(wrapped["models"], serde_json::json!(["one", "two"]));
    }

    #[test]
    fn legacy_jsonl_objects_are_normalized_without_changing_fields() {
        let normalized = normalize_legacy_object(serde_json::json!({
            "kind": "action",
            "command": "true"
        }));
        assert_eq!(normalized["schema_version"], 1);
        assert_eq!(normalized["kind"], "action");
        assert_eq!(normalized["command"], "true");
    }
}
