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
use std::time::Duration;

use crossterm::style::Stylize;
use serde_json::{json, Value};

use crate::providers::ToolDef;

/// Cap on file content returned to the model (chars), so a huge file can't blow
/// the context window.
const READ_LIMIT: usize = 16_000;

/// Cap on fetched web content returned to the model (chars).
const FETCH_LIMIT: usize = 16_000;

/// Cap on bytes read from a remote response before truncation, so a giant page
/// (or a non-text body) can't exhaust memory.
const FETCH_BYTE_CAP: u64 = 4 * 1024 * 1024;

/// Timeout for a single `fetch_url` request.
const FETCH_TIMEOUT_SECS: u64 = 20;

/// The built-in file tool definitions, offered to yolo when `file_tools` is on.
pub fn file_tool_defs() -> Vec<ToolDef> {
    vec![
        read_file_tool(),
        write_file_tool(),
        edit_file_tool(),
        list_dir_tool(),
    ]
}

/// The built-in web tools, offered to yolo when `web_tool` is on.
pub fn web_tool_defs() -> Vec<ToolDef> {
    vec![fetch_url_tool()]
}

/// True if `name` is one of the built-in file tools.
pub fn is_file_tool(name: &str) -> bool {
    matches!(name, "read_file" | "write_file" | "edit_file" | "list_dir")
}

/// True if `name` is one of the built-in web tools.
pub fn is_web_tool(name: &str) -> bool {
    name == "fetch_url"
}

/// True if `name` is any built-in tool dispatched by [`execute`].
pub fn is_builtin_tool(name: &str) -> bool {
    is_file_tool(name) || is_web_tool(name)
}

/// Execute a built-in tool. Returns `(short label for the audit log, result
/// content fed back to the model)`. `confirm_writes` gates writes outside the
/// work tree.
pub fn execute(name: &str, args: &Value, cwd: &Path, confirm_writes: bool) -> (String, String) {
    match name {
        "read_file" => read_file(args, cwd),
        "write_file" => write_file(args, cwd, confirm_writes),
        "edit_file" => edit_file(args, cwd, confirm_writes),
        "list_dir" => list_dir(args, cwd),
        "fetch_url" => fetch_url(args),
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

fn fetch_url_tool() -> ToolDef {
    ToolDef {
        name: "fetch_url".to_string(),
        description: "Fetch an http(s) URL and return its text. HTML is stripped \
            to readable text. Use this to read docs, release notes, or pages the \
            task references."
            .to_string(),
        schema: json!({
            "type": "object",
            "properties": {"url": {"type": "string", "description": "the http or https URL to fetch"}},
            "required": ["url"]
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
    let resolved = resolve(cwd, path);
    // Capture the pre-image before overwriting so the change is reversible.
    let existed = resolved.exists();
    let before = if existed {
        std::fs::read_to_string(&resolved).ok()
    } else {
        None
    };
    match std::fs::write(&resolved, content) {
        Ok(_) => {
            // Journal for `aishe undo`. Skip when an existing file wasn't valid
            // UTF-8 (we can't faithfully restore it); the write still stands.
            if !(existed && before.is_none()) {
                crate::undo::record(
                    &resolved,
                    existed,
                    before.clone(),
                    "write_file",
                    &format!("write {path}"),
                );
            }
            if let Some(b) = &before {
                print_diff(&crate::undo::unified_diff(b, content));
            }
            (
                format!("write {path}"),
                format!("Wrote {} bytes to '{path}'.", content.len()),
            )
        }
        Err(e) => (
            format!("write {path}"),
            format!("Error writing '{path}': {e}"),
        ),
    }
}

/// Print a (already `-`/`+`/` `-prefixed) diff under a tool step, colorized.
fn print_diff(diff: &str) {
    if diff.is_empty() {
        return;
    }
    for line in diff.lines() {
        let colored = if line.starts_with('-') {
            line.red().to_string()
        } else if line.starts_with('+') {
            line.green().to_string()
        } else {
            line.dim().to_string()
        };
        println!("    {colored}");
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
    match std::fs::write(&resolved, &new) {
        Ok(_) => {
            crate::undo::record(
                &resolved,
                true,
                Some(original.clone()),
                "edit_file",
                &format!("edit {path}"),
            );
            print_diff(&crate::undo::unified_diff(&original, &new));
            (
                format!("edit {path}"),
                format!("Replaced {replaced} occurrence(s) in '{path}'."),
            )
        }
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

/// Fetch an http(s) URL and return its text (HTML reduced to readable text).
/// Network and parse errors are returned as `Error: ...` for the model to react
/// to. The response is byte-capped while reading and char-capped before return.
fn fetch_url(args: &Value) -> (String, String) {
    let url = arg(args, "url");
    if url.is_empty() {
        return ("fetch_url".into(), "Error: no url given.".into());
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return (
            format!("fetch {url}"),
            "Error: only http and https URLs are supported.".into(),
        );
    }
    println!("  {} {}", "🌐".cyan(), format!("fetch {url}").dim());

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(FETCH_TIMEOUT_SECS))
        .build();
    let resp = match agent
        .get(url)
        .set("User-Agent", "aishe/fetch_url")
        .set(
            "Accept",
            "text/html,text/plain,application/json;q=0.9,*/*;q=0.8",
        )
        .call()
    {
        Ok(r) => r,
        Err(ureq::Error::Status(status, r)) => {
            let ct = r.content_type().to_string();
            let body = read_capped(r.into_reader());
            let snippet = html_to_text(&body, &ct);
            return (
                format!("fetch {url}"),
                format!("HTTP {status} from '{url}'.\n{}", cap_chars(&snippet)),
            );
        }
        Err(e) => {
            return (
                format!("fetch {url}"),
                format!("Error fetching '{url}': {e}"),
            )
        }
    };

    let content_type = resp.content_type().to_string();
    let body = read_capped(resp.into_reader());
    let text = html_to_text(&body, &content_type);
    let text = text.trim();
    let out = if text.is_empty() {
        format!("(no readable text at '{url}'; content-type {content_type})")
    } else {
        cap_chars(text)
    };
    (format!("fetch {url}"), out)
}

/// Read up to [`FETCH_BYTE_CAP`] bytes from a response reader, lossily decoded as
/// UTF-8 (good enough for HTML/text/JSON; binary bodies become mostly empty text).
fn read_capped(reader: impl std::io::Read) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = reader.take(FETCH_BYTE_CAP).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

/// Truncate a string to [`FETCH_LIMIT`] chars, noting the cut.
fn cap_chars(s: &str) -> String {
    if s.chars().count() > FETCH_LIMIT {
        let kept: String = s.chars().take(FETCH_LIMIT).collect();
        format!("{kept}\n[truncated to {FETCH_LIMIT} chars]")
    } else {
        s.to_string()
    }
}

/// Reduce a body to readable text. For HTML content types (or bodies that look
/// like HTML) drop `<script>`/`<style>` blocks and tags, decode a few common
/// entities, and collapse whitespace. Plain text and JSON pass through.
fn html_to_text(body: &str, content_type: &str) -> String {
    let looks_html = content_type.contains("html")
        || (!content_type.contains("json")
            && !content_type.contains("text/plain")
            && body.trim_start().starts_with('<'));
    if !looks_html {
        return body.to_string();
    }
    strip_html(body)
}

/// Strip HTML tags (and `script`/`style` contents) to plain text. Walks the
/// string by byte index but only ever pushes whole `&str` slices, so multibyte
/// UTF-8 is preserved.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        // Find the next tag start from i.
        match html[i..].find('<') {
            None => {
                out.push_str(&html[i..]);
                break;
            }
            Some(rel) => {
                out.push_str(&html[i..i + rel]);
                i += rel; // now at '<'
            }
        }
        // Skip the contents of <script>/<style> blocks entirely.
        let mut skipped_block = false;
        for tag in ["script", "style"] {
            if lower[i..].starts_with(&format!("<{tag}")) {
                let close = format!("</{tag}>");
                match lower[i..].find(&close) {
                    Some(end) => i += end + close.len(),
                    None => i = html.len(),
                }
                out.push(' ');
                skipped_block = true;
                break;
            }
        }
        if skipped_block {
            continue;
        }
        // Otherwise skip to the end of this tag.
        match html[i..].find('>') {
            Some(end) => i += end + 1,
            None => break,
        }
        out.push(' ');
    }
    collapse_ws(&decode_entities(&out))
}

/// Decode a handful of common HTML entities.
fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ")
}

/// Collapse runs of whitespace, keeping paragraph breaks readable.
fn collapse_ws(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.lines() {
        let trimmed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if trimmed.is_empty() {
            if lines.last().map(|l| !l.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
        } else {
            lines.push(trimmed);
        }
    }
    lines.join("\n").trim().to_string()
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
        crate::undo::set_test_journal(Some(dir.join("undo.jsonl")));
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
        crate::undo::set_test_journal(Some(dir.join("undo.jsonl")));
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

    #[test]
    fn file_tool_changes_are_undoable() {
        let dir = std::env::temp_dir().join(format!("aishe-tools-u-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        crate::undo::set_test_journal(Some(dir.join("undo.jsonl")));

        // create a file, then edit it (both within this process's batch)
        write_file(
            &json!({"path": "c.txt", "content": "one\ntwo"}),
            &dir,
            false,
        );
        edit_file(
            &json!({"path": "c.txt", "find": "two", "replace": "TWO"}),
            &dir,
            false,
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("c.txt")).unwrap(),
            "one\nTWO"
        );

        // One undo reverts the whole batch in reverse: the edit is rolled back and
        // then the create is removed, so the file is gone (its original state).
        let undone = crate::undo::undo_last().unwrap().unwrap();
        assert!(undone.errors.is_empty(), "{:?}", undone.errors);
        assert!(
            !dir.join("c.txt").exists(),
            "created-then-edited file should be removed on undo"
        );

        crate::undo::set_test_journal(None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn html_stripped_to_text() {
        let html = "<html><head><style>.a{color:red}</style>\
            <script>var x=1<2;</script><title>T</title></head>\
            <body><h1>Hello &amp; bye</h1><p>Caf\u{e9} &lt;ok&gt;</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello & bye"), "got: {text}");
        assert!(text.contains("Caf\u{e9} <ok>"), "got: {text}");
        // script/style contents must be gone.
        assert!(!text.contains("color:red"), "got: {text}");
        assert!(!text.contains("var x"), "got: {text}");
    }

    #[test]
    fn html_to_text_passes_through_json_and_plain() {
        let json = r#"{"a": 1, "b": "<not html>"}"#;
        assert_eq!(html_to_text(json, "application/json"), json);
        let plain = "line one\nline two";
        assert_eq!(html_to_text(plain, "text/plain; charset=utf-8"), plain);
    }

    #[test]
    fn fetch_url_rejects_non_http() {
        let (_, msg) = fetch_url(&json!({"url": "file:///etc/passwd"}));
        assert!(msg.contains("only http"), "got: {msg}");
        let (_, msg2) = fetch_url(&json!({}));
        assert!(msg2.contains("no url"), "got: {msg2}");
    }
}
