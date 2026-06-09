//! Build the environment context block prepended to LLM requests. Never
//! includes file contents — only metadata, a directory listing, and recent
//! command history.

use std::process::Command;
use std::sync::OnceLock;

use crate::executor::Executor;

const MAX_DIR_ENTRIES: usize = 50;
const MAX_HISTORY: usize = 10;

/// OS description, computed once (e.g. "macOS 14.5 (arm64)" / "Linux 6.x (x86_64)").
static OS_INFO: OnceLock<String> = OnceLock::new();
/// Shell backend version, computed once (e.g. "zsh 5.9").
static SHELL_INFO: OnceLock<String> = OnceLock::new();

/// Initialize the cached OS and shell version strings (call once at startup).
pub fn init(shell: &std::path::Path) {
    OS_INFO.get_or_init(detect_os);
    SHELL_INFO.get_or_init(|| detect_shell_version(shell));
}

/// Build the context block string for the current executor state.
pub fn build(executor: &Executor) -> String {
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
        out.push_str(&format!("  [{code}] {cmd}\n"));
    }

    out
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
        let block = build(&exec);
        assert!(block.contains("OS: "));
        assert!(block.contains("Shell backend: "));
        assert!(block.contains("CWD: "));
        assert!(block.contains("Directory listing"));
        assert!(block.contains("Recent commands"));
    }

    #[test]
    fn directory_listing_respects_cap() {
        let dir = std::env::temp_dir().join(format!("llmsh_ctx_{}", std::process::id()));
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
