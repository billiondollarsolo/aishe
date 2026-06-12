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
pub fn build(executor: &Executor, config: &crate::config::Config) -> String {
    let redact_secrets = config.aishe.redact_secrets;
    let project_context = config.aishe.project_context;
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

    // What's actually installed on this host, so the model proposes commands that
    // exist here (apt vs dnf vs brew, docker vs podman, ...). Cached.
    if config.aishe.host_profile {
        let tools = host_capabilities();
        if !tools.is_empty() {
            out.push_str(&format!("Installed tools: {tools}\n"));
        }
    }

    // This repo's task surface (justfile/Makefile/package.json/...), so "run the
    // tests" resolves to *this* project's actual command.
    if config.aishe.project_tasks {
        if let Some(tasks) = project_tasks_block(executor.cwd()) {
            out.push_str("Project tasks (prefer these for repo actions):\n  ");
            out.push_str(&tasks);
            out.push('\n');
        }
    }

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

/// Cap on a build file we read for task extraction.
const TASK_FILE_CAP: u64 = 128 * 1024;
/// Cap on items listed per task source.
const MAX_TASKS_PER_SOURCE: usize = 12;

/// Tools probed for the host-capabilities line (cached for the process).
const PROBED_TOOLS: &[&str] = &[
    "apt-get",
    "dnf",
    "yum",
    "pacman",
    "apk",
    "zypper",
    "brew",
    "docker",
    "podman",
    "kubectl",
    "helm",
    "systemctl",
    "git",
    "rustc",
    "cargo",
    "go",
    "node",
    "npm",
    "pnpm",
    "yarn",
    "python3",
    "pip",
    "just",
    "make",
];

/// A comma-separated list of the [`PROBED_TOOLS`] present on `$PATH`. Cached:
/// `$PATH` does not change within a session, and `which` is a cheap lookup.
fn host_capabilities() -> String {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            PROBED_TOOLS
                .iter()
                .copied()
                .filter(|t| crate::executor::which(t).is_some())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .clone()
}

/// Read the first of `names` that exists under `dir`, capped at [`TASK_FILE_CAP`].
fn read_capped_file(dir: &std::path::Path, names: &[&str]) -> Option<String> {
    use std::io::Read;
    for name in names {
        let path = dir.join(name);
        if let Ok(f) = std::fs::File::open(&path) {
            let mut buf = Vec::new();
            if f.take(TASK_FILE_CAP).read_to_end(&mut buf).is_ok() {
                return Some(String::from_utf8_lossy(&buf).into_owned());
            }
        }
    }
    None
}

/// Summarize the working directory's task surface (build/run entry points), so the
/// model maps "run the tests" / "build it" to *this* repo's real command. Returns
/// `None` when nothing recognizable is present.
fn project_tasks_block(cwd: &std::path::Path) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let cap = |mut v: Vec<String>| -> Vec<String> {
        v.truncate(MAX_TASKS_PER_SOURCE);
        v
    };

    if let Some(t) = read_capped_file(cwd, &["justfile", "Justfile", ".justfile"]) {
        let recipes = cap(just_recipes(&t));
        if !recipes.is_empty() {
            lines.push(format!("just: {}", recipes.join(", ")));
        }
    }
    if let Some(t) = read_capped_file(cwd, &["Makefile", "makefile", "GNUmakefile"]) {
        let targets = cap(make_targets(&t));
        if !targets.is_empty() {
            lines.push(format!("make: {}", targets.join(", ")));
        }
    }
    if let Some(t) = read_capped_file(cwd, &["package.json"]) {
        let scripts = cap(json_section_keys(&t, "scripts"));
        if !scripts.is_empty() {
            lines.push(format!("npm run: {}", scripts.join(", ")));
        }
    }
    if let Some(t) = read_capped_file(cwd, &["composer.json"]) {
        let scripts = cap(json_section_keys(&t, "scripts"));
        if !scripts.is_empty() {
            lines.push(format!("composer: {}", scripts.join(", ")));
        }
    }
    if cwd.join("Cargo.toml").exists() {
        lines.push("cargo: build, test, run, clippy, fmt".to_string());
    }
    if cwd.join("pyproject.toml").exists() || cwd.join("setup.py").exists() {
        lines.push("python project (pytest; pip install -e .)".to_string());
    }
    if let Some(t) = read_capped_file(
        cwd,
        &[
            "compose.yaml",
            "compose.yml",
            "docker-compose.yml",
            "docker-compose.yaml",
        ],
    ) {
        let svcs = cap(compose_services(&t));
        if !svcs.is_empty() {
            lines.push(format!("compose services: {}", svcs.join(", ")));
        }
    }
    if cwd.join(".github").join("workflows").is_dir() {
        lines.push("GitHub Actions CI (.github/workflows)".to_string());
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n  "))
    }
}

/// Recipe names from a `justfile`: a name at column 0 followed (after optional
/// args) by `:`, skipping comments, assignments (`x := ...`), and settings.
fn just_recipes(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#', '@']) || line.trim().is_empty() {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        // `name := value` is an assignment, not a recipe.
        if line[..colon].contains(":=") || line[colon..].starts_with(":=") {
            continue;
        }
        let head = line[..colon].split_whitespace().next().unwrap_or("");
        if !head.is_empty()
            && head
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            out.push(head.to_string());
        }
    }
    out
}

/// Target names from a `Makefile`: `name:` at column 0, skipping comments,
/// variable assignments, pattern rules (`%`), and special `.PHONY`-style targets.
fn make_targets(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let Some(colon) = line.find(':') else {
            continue;
        };
        let head = &line[..colon];
        if head.contains('=') || head.contains('%') || head.starts_with('.') {
            continue;
        }
        for name in head.split_whitespace() {
            if name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
            {
                out.push(name.to_string());
            }
        }
    }
    out.dedup();
    out
}

/// Keys of a top-level JSON object field (e.g. `scripts` in package.json).
fn json_section_keys(text: &str, field: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| {
            v.get(field)
                .and_then(|s| s.as_object())
                .map(|m| m.keys().cloned().collect())
        })
        .unwrap_or_default()
}

/// Service names from a compose file: keys nested one level under `services:`.
/// Line-based (no YAML dependency); best-effort.
fn compose_services(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_services = false;
    for line in text.lines() {
        if line.starts_with("services:") {
            in_services = true;
            continue;
        }
        if in_services {
            // A new top-level key (non-indented, non-comment) ends the section.
            if !line.starts_with([' ', '\t'])
                && !line.trim_start().starts_with('#')
                && !line.trim().is_empty()
            {
                break;
            }
            // A service is a key indented exactly two spaces: `  name:`.
            if let Some(rest) = line.strip_prefix("  ") {
                if !rest.starts_with([' ', '\t', '#']) {
                    if let Some(colon) = rest.find(':') {
                        let name = rest[..colon].trim();
                        if !name.is_empty()
                            && name.chars().all(|c| {
                                c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
                            })
                        {
                            out.push(name.to_string());
                        }
                    }
                }
            }
        }
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

    /// A config for the build() tests, with the project-tasks/host-profile blocks
    /// off so those tests stay focused on the field they exercise.
    fn cfg(redact: bool, project_context: bool) -> crate::config::Config {
        let mut c = crate::config::Config::default();
        c.aishe.redact_secrets = redact;
        c.aishe.project_context = project_context;
        c.aishe.project_tasks = false;
        c.aishe.host_profile = false;
        c
    }

    #[test]
    fn build_contains_required_fields() {
        let exec = Executor::new().unwrap();
        let block = build(&exec, &cfg(true, false));
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
        let redacted = build(&exec, &cfg(true, false));
        assert!(redacted.contains("API_TOKEN=<redacted>"), "{redacted}");
        assert!(!redacted.contains("supersecretvalue123"));
        // With redaction off, the raw command is included verbatim.
        let raw = build(&exec, &cfg(false, false));
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
        let on = build(&exec, &cfg(true, true));
        assert!(on.contains("Project context"), "{on}");
        assert!(on.contains("REPO_MARKER_TOKEN"), "{on}");
        // Off: nothing from the file.
        let off = build(&exec, &cfg(true, false));
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

    #[test]
    fn just_recipes_extracts_recipe_names() {
        let text = "set shell := ['bash']\n# a comment\nbuild:\n\tcargo build\ntest arg='x':\n\tcargo test\nexport := 'no'\n";
        let r = just_recipes(text);
        assert_eq!(r, vec!["build", "test"]); // assignments/comments/bodies skipped
    }

    #[test]
    fn make_targets_extracts_target_names() {
        let text = "CC = gcc\n# comment\nall: build test\nbuild:\n\t$(CC) x.c\n%.o: %.c\n\tcc -c $<\n.PHONY: all\n";
        let t = make_targets(text);
        assert!(t.contains(&"all".to_string()));
        assert!(t.contains(&"build".to_string()));
        assert!(!t
            .iter()
            .any(|x| x.contains('%') || x.starts_with('.') || x == "CC"));
    }

    #[test]
    fn json_section_keys_reads_scripts() {
        let pkg = r#"{"name":"x","scripts":{"build":"tsc","test":"jest","lint":"eslint ."}}"#;
        let mut k = json_section_keys(pkg, "scripts");
        k.sort();
        assert_eq!(k, vec!["build", "lint", "test"]);
        assert!(json_section_keys(pkg, "missing").is_empty());
        assert!(json_section_keys("not json", "scripts").is_empty());
    }

    #[test]
    fn compose_services_parsed_line_based() {
        let yaml = "version: '3'\nservices:\n  web:\n    image: nginx\n  db:\n    image: postgres\nvolumes:\n  data:\n";
        let s = compose_services(yaml);
        assert_eq!(s, vec!["web", "db"]); // stops at the `volumes:` top-level key
    }

    #[test]
    fn project_tasks_block_summarizes_a_repo() {
        let dir = std::env::temp_dir().join(format!("aishe-tasks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("justfile"),
            "build:\n\tcargo build\ntest:\n\tcargo test\n",
        )
        .unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname='x'\n").unwrap();
        let block = project_tasks_block(&dir).unwrap();
        assert!(block.contains("just: build, test"), "{block}");
        assert!(block.contains("cargo:"), "{block}");
        // An empty dir yields nothing.
        let empty = std::env::temp_dir().join(format!("aishe-tasks-empty-{}", std::process::id()));
        std::fs::create_dir_all(&empty).unwrap();
        assert!(project_tasks_block(&empty).is_none());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn host_capabilities_is_cached_and_safe() {
        // Just must not panic; it returns whatever subset of PROBED_TOOLS exists.
        let a = host_capabilities();
        let b = host_capabilities();
        assert_eq!(a, b);
    }
}
