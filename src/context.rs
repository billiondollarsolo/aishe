//! Build the environment context block prepended to LLM requests. Never
//! includes file contents — only metadata, a directory listing, and recent
//! command history.

use std::process::Command;
use std::sync::OnceLock;

use crate::executor::Executor;

const MAX_DIR_ENTRIES: usize = 50;
const MAX_HISTORY: usize = 10;
/// Cap on the per-project context file included in the block (chars).
const MAX_PROJECT_CONTEXT: usize = 4_000;

/// OS description, computed once (e.g. "macOS 14.5 (arm64)" / "Linux 6.x (x86_64)").
static OS_INFO: OnceLock<String> = OnceLock::new();
/// Shell backend version, computed once (e.g. "zsh 5.9").
static SHELL_INFO: OnceLock<String> = OnceLock::new();

/// Initialize the cached OS and shell version strings (call once at startup).
pub fn init(shell: &std::path::Path) {
    OS_INFO.get_or_init(detect_os);
    SHELL_INFO.get_or_init(|| detect_shell_version(shell));
}

/// Build the context block string for the current executor state. When
/// `redact_secrets` is set, recent commands are scrubbed of likely credentials
/// before being included (they can contain `export TOKEN=...`, `mysql -p...`, or
/// URLs with passwords). When `project_context` is set, a per-project
/// `.aishe/context.md` found at or above the cwd is appended so repo-specific
/// conventions reach the model.
pub fn build(executor: &Executor, redact_secrets: bool, project_context: bool) -> String {
    let os = OS_INFO.get().cloned().unwrap_or_else(detect_os);
    let shell = SHELL_INFO
        .get()
        .cloned()
        .unwrap_or_else(|| detect_shell_version(executor.shell()));

    let cwd = executor.cwd().display().to_string();

    let mut out = String::new();
    out.push_str(&format!("OS: {os}\n"));
    out.push_str(&format!("Shell backend: {shell}\n"));
    out.push_str(&format!("CWD: {cwd}\n"));

    out.push_str(&format!(
        "Directory listing (max {MAX_DIR_ENTRIES} entries, dirs have trailing /):\n"
    ));
    out.push_str("  ");
    out.push_str(&directory_listing(executor.cwd()));
    out.push('\n');

    out.push_str(&format!(
        "Recent commands (last {MAX_HISTORY}, [exit_code] cmd):\n"
    ));
    for (cmd, code) in executor.history.iter().take(MAX_HISTORY) {
        let cmd = if redact_secrets {
            crate::redact::redact(cmd)
        } else {
            cmd.clone()
        };
        out.push_str(&format!("  [{code}] {cmd}\n"));
    }

    if project_context {
        if let Some(block) = project_context_block(executor.cwd(), MAX_PROJECT_CONTEXT) {
            out.push_str("Project context (.aishe/context.md):\n");
            out.push_str(&block);
            out.push('\n');
        }
    }

    out
}

/// Find a `.aishe/context.md` at `start` or any ancestor directory and return its
/// contents, truncated (char-safe) to `max` chars. The nearest file wins.
fn project_context_block(start: &std::path::Path, max: usize) -> Option<String> {
    let mut dir = Some(start);
    while let Some(d) = dir {
        let candidate = d.join(".aishe").join("context.md");
        if let Ok(text) = std::fs::read_to_string(&candidate) {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return None;
            }
            if trimmed.chars().count() > max {
                let kept: String = trimmed.chars().take(max).collect();
                return Some(format!("{kept}\n[truncated to {max} chars]"));
            }
            return Some(trimmed.to_string());
        }
        dir = d.parent();
    }
    None
}

fn directory_listing(dir: &std::path::Path) -> String {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return "(unreadable)".to_string(),
    };
    let mut names: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        names.push(if is_dir { format!("{name}/") } else { name });
        if names.len() >= MAX_DIR_ENTRIES {
            break;
        }
    }
    names.sort();
    names.join("  ")
}

fn detect_os() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => {
            let ver = Command::new("sw_vers")
                .arg("-productVersion")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            format!("macOS {ver} ({arch})")
        }
        "linux" => {
            let kernel = Command::new("uname")
                .arg("-sr")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Linux".to_string());
            format!("{kernel} ({arch})")
        }
        other => format!("{other} ({arch})"),
    }
}

fn detect_shell_version(shell: &std::path::Path) -> String {
    let out = Command::new(shell)
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    match out {
        Some(s) => s.lines().next().unwrap_or(&s).to_string(),
        None => shell
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_contains_required_fields() {
        let exec = Executor::new().unwrap();
        let block = build(&exec, true, false);
        assert!(block.contains("OS: "));
        assert!(block.contains("Shell backend: "));
        assert!(block.contains("CWD: "));
        assert!(block.contains("Directory listing"));
        assert!(block.contains("Recent commands"));
    }

    #[test]
    fn build_redacts_secrets_in_history_when_enabled() {
        let mut exec = Executor::new().unwrap();
        exec.history
            .push_front(("export API_TOKEN=supersecretvalue123".to_string(), 0));
        let redacted = build(&exec, true, false);
        assert!(redacted.contains("API_TOKEN=<redacted>"), "{redacted}");
        assert!(!redacted.contains("supersecretvalue123"));
        // With redaction off, the raw command is included verbatim.
        let raw = build(&exec, false, false);
        assert!(raw.contains("supersecretvalue123"));
    }

    #[test]
    fn project_context_found_capped_and_absent() {
        let base = std::env::temp_dir().join(format!("aishe_pctx_{}", std::process::id()));
        let nested = base.join("sub").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        let aishe = base.join(".aishe");
        std::fs::create_dir_all(&aishe).unwrap();
        std::fs::write(aishe.join("context.md"), "Use tabs, not spaces.").unwrap();

        // Found from a nested cwd by walking up to the ancestor that has it.
        let block = project_context_block(&nested, MAX_PROJECT_CONTEXT).unwrap();
        assert!(block.contains("Use tabs, not spaces."));

        // Large content is truncated.
        std::fs::write(
            aishe.join("context.md"),
            "x".repeat(MAX_PROJECT_CONTEXT + 500),
        )
        .unwrap();
        let capped = project_context_block(&nested, MAX_PROJECT_CONTEXT).unwrap();
        assert!(capped.contains("[truncated to"));
        assert!(capped.chars().count() < MAX_PROJECT_CONTEXT + 100);

        // Absent file -> None.
        let other = std::env::temp_dir().join(format!("aishe_pctx_none_{}", std::process::id()));
        std::fs::create_dir_all(&other).unwrap();
        assert!(project_context_block(&other, MAX_PROJECT_CONTEXT).is_none());

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn build_includes_and_omits_project_context_per_flag() {
        use std::time::{SystemTime, UNIX_EPOCH};
        // Unique per run so parallel tests never collide on the cwd we cd into.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aishe_pctx_build_{nanos}"));
        std::fs::create_dir_all(dir.join(".aishe")).unwrap();
        std::fs::write(dir.join(".aishe").join("context.md"), "REPO_MARKER_TOKEN").unwrap();
        let mut exec = Executor::new().unwrap();
        assert_eq!(
            exec.run_builtin(&["cd".to_string(), dir.to_string_lossy().to_string()]),
            0,
            "cd into the temp dir failed"
        );
        // On: the marker appears under the project-context heading.
        let on = build(&exec, true, true);
        assert!(on.contains("Project context"), "{on}");
        assert!(on.contains("REPO_MARKER_TOKEN"), "{on}");
        // Off: nothing from the file.
        let off = build(&exec, true, false);
        assert!(!off.contains("REPO_MARKER_TOKEN"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn directory_listing_respects_cap() {
        let dir = std::env::temp_dir().join(format!("aishe_ctx_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..80 {
            std::fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
        }
        let listing = directory_listing(&dir);
        let count = listing.split("  ").filter(|s| !s.is_empty()).count();
        assert!(count <= MAX_DIR_ENTRIES, "got {count} entries");
        std::fs::remove_dir_all(&dir).ok();
    }
}
