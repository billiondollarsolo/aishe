//! Built-in agentic tools beyond `run_command`: precise file read / write / edit
//! and directory listing. These let the yolo loop work with files directly
//! instead of round-tripping everything through the shell (heredoc quoting, `sed`
//! escaping, `cat` capture/truncation), which the model gets wrong more often.
//!
//! Writes to a path outside the working tree (absolute, `~`, or `..`-escaping)
//! are confirmed when `yolo_confirm_dangerous` is on, mirroring the command
//! safety gate.

use std::io::Write;
use std::path::{Path, PathBuf};

use crossterm::style::Stylize;
use serde_json::{json, Value};

use crate::providers::ToolDef;

/// Cap on file content returned to the model (chars), so a huge file can't blow
/// the context window.
const READ_LIMIT: usize = 16_000;

/// The built-in file tool definitions, offered to yolo when `file_tools` is on.
pub fn file_tool_defs() -> Vec<ToolDef> {
    vec![
        read_file_tool(),
        write_file_tool(),
        edit_file_tool(),
        list_dir_tool(),
    ]
}

/// True if `name` is one of the built-in file tools.
pub fn is_file_tool(name: &str) -> bool {
    matches!(name, "read_file" | "write_file" | "edit_file" | "list_dir")
}

/// Execute a file tool. Returns `(short label for the audit log, result content
/// fed back to the model)`. `confirm_writes` gates writes outside the work tree.
pub fn execute(name: &str, args: &Value, cwd: &Path, confirm_writes: bool) -> (String, String) {
    match name {
        "read_file" => read_file(args, cwd),
        "write_file" => write_file(args, cwd, confirm_writes),
        "edit_file" => edit_file(args, cwd, confirm_writes),
        "list_dir" => list_dir(args, cwd),
        other => (other.to_string(), format!("Error: unknown tool '{other}'.")),
    }
}

fn read_file_tool() -> ToolDef {
    ToolDef {
        name: "read_file".to_string(),
        description: "Read a text file and return its contents. Prefer this over \
            `cat` for inspecting files."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "file path (relative to the cwd or absolute)"}},
            "required": ["path"]
        }),
    }
}

fn write_file_tool() -> ToolDef {
    ToolDef {
        name: "write_file".to_string(),
        description: "Create or overwrite a file with exact contents. Prefer this \
            over shell heredocs/redirection for writing files."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string", "description": "the full file contents"}
            },
            "required": ["path", "content"]
        }),
    }
}

fn edit_file_tool() -> ToolDef {
    ToolDef {
        name: "edit_file".to_string(),
        description: "Replace text in a file: substitute `find` with `replace`. \
            Prefer this over `sed` for precise edits. `find` must occur in the file."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "find": {"type": "string", "description": "exact text to replace"},
                "replace": {"type": "string", "description": "replacement text"},
                "all": {"type": "boolean", "description": "replace every occurrence (default: first only)"}
            },
            "required": ["path", "find", "replace"]
        }),
    }
}

fn list_dir_tool() -> ToolDef {
    ToolDef {
        name: "list_dir".to_string(),
        description: "List the entries of a directory (directories shown with a \
            trailing slash)."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "directory (default: cwd)"}}
        }),
    }
}

fn arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

/// Resolve a tool path against the working directory.
fn resolve(cwd: &Path, path: &str) -> PathBuf {
    let pb = PathBuf::from(path);
    if pb.is_absolute() {
        pb
    } else {
        cwd.join(pb)
    }
}

/// A write target that needs confirmation: outside the working tree (absolute,
/// home-relative, or escaping via `..`).
fn outside_tree(path: &str) -> bool {
    path.starts_with('/') || path.starts_with('~') || path.split('/').any(|seg| seg == "..")
}

/// Confirm a risky write. With an interactive terminal it asks (only `yes`
/// proceeds); without one (e.g. `-c`/piped, where no human can answer) it
/// proceeds, consistent with `run_command`, which can already write anywhere.
fn confirm(action: &str, path: &str) -> bool {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return true;
    }
    print!(
        "  {} {} {} (type 'yes'): ",
        action.yellow().bold(),
        "outside the working tree:".dim(),
        path.white(),
    );
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    line.trim().eq_ignore_ascii_case("yes")
}

fn read_file(args: &Value, cwd: &Path) -> (String, String) {
    let path = arg(args, "path");
    if path.is_empty() {
        return ("read_file".into(), "Error: no path given.".into());
    }
    println!("  {} {}", "📄".cyan(), format!("read {path}").dim());
    match std::fs::read_to_string(resolve(cwd, path)) {
        Ok(s) => {
            let content = if s.chars().count() > READ_LIMIT {
                let kept: String = s.chars().take(READ_LIMIT).collect();
                format!("{kept}\n[truncated to {READ_LIMIT} chars]")
            } else {
                s
            };
            (format!("read {path}"), content)
        }
        Err(e) => (
            format!("read {path}"),
            format!("Error reading '{path}': {e}"),
        ),
    }
}

fn write_file(args: &Value, cwd: &Path, confirm_writes: bool) -> (String, String) {
    let path = arg(args, "path");
    let content = arg(args, "content");
    if path.is_empty() {
        return ("write_file".into(), "Error: no path given.".into());
    }
    if confirm_writes && outside_tree(path) && !confirm("write", path) {
        return (format!("write {path}"), "User declined the write.".into());
    }
    println!("  {} {}", "✏️".yellow(), format!("write {path}").dim());
    match std::fs::write(resolve(cwd, path), content) {
        Ok(_) => (
            format!("write {path}"),
            format!("Wrote {} bytes to '{path}'.", content.len()),
        ),
        Err(e) => (
            format!("write {path}"),
            format!("Error writing '{path}': {e}"),
        ),
    }
}

fn edit_file(args: &Value, cwd: &Path, confirm_writes: bool) -> (String, String) {
    let path = arg(args, "path");
    let find = arg(args, "find");
    let replace = arg(args, "replace");
    let all = args.get("all").and_then(|v| v.as_bool()).unwrap_or(false);
    if path.is_empty() || find.is_empty() {
        return (
            "edit_file".into(),
            "Error: 'path' and 'find' are required.".into(),
        );
    }
    if confirm_writes && outside_tree(path) && !confirm("edit", path) {
        return (format!("edit {path}"), "User declined the edit.".into());
    }
    let resolved = resolve(cwd, path);
    let original = match std::fs::read_to_string(&resolved) {
        Ok(s) => s,
        Err(e) => {
            return (
                format!("edit {path}"),
                format!("Error reading '{path}': {e}"),
            )
        }
    };
    if !original.contains(find) {
        return (
            format!("edit {path}"),
            format!("Error: the `find` text was not found in '{path}'."),
        );
    }
    let count = original.matches(find).count();
    let new = if all {
        original.replace(find, replace)
    } else {
        original.replacen(find, replace, 1)
    };
    let replaced = if all { count } else { 1 };
    println!("  {} {}", "✏️".yellow(), format!("edit {path}").dim());
    match std::fs::write(&resolved, new) {
        Ok(_) => (
            format!("edit {path}"),
            format!("Replaced {replaced} occurrence(s) in '{path}'."),
        ),
        Err(e) => (
            format!("edit {path}"),
            format!("Error writing '{path}': {e}"),
        ),
    }
}

fn list_dir(args: &Value, cwd: &Path) -> (String, String) {
    let path = {
        let p = arg(args, "path");
        if p.is_empty() {
            "."
        } else {
            p
        }
    };
    println!("  {} {}", "📂".cyan(), format!("list {path}").dim());
    let dir = resolve(cwd, path);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            return (
                format!("list {path}"),
                format!("Error listing '{path}': {e}"),
            )
        }
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        names.push(if is_dir { format!("{name}/") } else { name });
    }
    names.sort();
    let body = if names.is_empty() {
        "(empty)".to_string()
    } else {
        names.join("\n")
    };
    (format!("list {path}"), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_tree_detection() {
        assert!(outside_tree("/etc/passwd"));
        assert!(outside_tree("~/secrets"));
        assert!(outside_tree("../escape"));
        assert!(outside_tree("a/../../b"));
        assert!(!outside_tree("src/main.rs"));
        assert!(!outside_tree("file.txt"));
        assert!(!outside_tree("a/b/c"));
    }

    #[test]
    fn write_then_read_roundtrip() {
        let dir = std::env::temp_dir().join(format!("aishe-tools-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (_, msg) = write_file(
            &json!({"path": "note.txt", "content": "hello\nworld"}),
            &dir,
            false,
        );
        assert!(msg.contains("Wrote"));
        let (_, content) = read_file(&json!({"path": "note.txt"}), &dir);
        assert_eq!(content, "hello\nworld");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edit_replaces_text() {
        let dir = std::env::temp_dir().join(format!("aishe-tools-e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_file(&json!({"path": "f.txt", "content": "a b a b"}), &dir, false);
        // first only
        let (_, m1) = edit_file(
            &json!({"path": "f.txt", "find": "a", "replace": "X"}),
            &dir,
            false,
        );
        assert!(m1.contains("Replaced 1"));
        assert_eq!(read_file(&json!({"path": "f.txt"}), &dir).1, "X b a b");
        // all
        edit_file(
            &json!({"path": "f.txt", "find": "b", "replace": "Y", "all": true}),
            &dir,
            false,
        );
        assert_eq!(read_file(&json!({"path": "f.txt"}), &dir).1, "X Y a Y");
        // missing find string is an error
        let (_, m3) = edit_file(
            &json!({"path": "f.txt", "find": "zzz", "replace": "q"}),
            &dir,
            false,
        );
        assert!(m3.contains("not found"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
