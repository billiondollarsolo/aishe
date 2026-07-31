//! User-defined slash commands ("plugins" / "skills"), modeled on Claude Code's
//! file-based custom commands.
//!
//! Commands are Markdown files discovered from:
//!   - `~/.config/aishe/commands/*.md`  (user)
//!   - `<cwd>/.aishe/commands/*.md`      (project — cannot shadow a user command
//!     of the same name; the user's own definition always wins)
//!
//! The file stem is the command name (`deploy.md` → `/deploy`). An optional
//! YAML-ish frontmatter block configures it; the body is a template:
//!
//! ```text
//! ---
//! description: Summarize recent git history
//! mode: suggest          # suggest | auto | yolo  (NL commands only)
//! shell: false           # true → run the body as a shell command, not NL
//! ---
//! Summarize the last $1 commits in this repo for: $ARGUMENTS
//! ```
//!
//! Templating: `$ARGUMENTS` → all args (space-joined); `$1`..`$9` → positional
//! args (missing → empty). Invoked as `/deploy 5 the release`.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A loaded custom command.
#[derive(Debug, Clone)]
pub struct CustomCommand {
    pub name: String,
    pub description: String,
    /// Run the expanded body as a shell command rather than a natural-language
    /// request.
    pub shell: bool,
    /// NL mode override (`suggest`/`auto`/`yolo`); `None` = use the current mode.
    pub mode: Option<String>,
    pub body: String,
    /// Origin file for a *project* command (`<cwd>/.aishe/commands/*.md`), used to
    /// gate its shell execution against trust. `None` for user commands (authored
    /// by the user, so trusted by construction).
    pub source: Option<PathBuf>,
}

/// The result of expanding a command with arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Expanded {
    pub text: String,
    pub shell: bool,
    pub mode: Option<String>,
}

impl CustomCommand {
    /// Whether running this command's `shell:true` body must be explicitly
    /// confirmed on the trust dimension before it executes — the trust/confirm
    /// decision at the core of `main::gate_custom_shell`. A **user**-origin command
    /// (`source == None`) is trusted by construction and never needs confirmation;
    /// a **project**-origin command needs it unless its source file is currently
    /// trusted (pass `trusted = trust::is_trusted(src, contents)`).
    pub fn needs_trust_confirm(&self, trusted: bool) -> bool {
        self.source.is_some() && !trusted
    }

    /// The NL mode this command actually runs in, given the user's `configured`
    /// mode and whether its source file is trusted.
    ///
    /// A **project**-origin command's `mode:` frontmatter is attacker-controlled
    /// (it ships inside a cloned repo), so an untrusted one must not *escalate*
    /// past the user's configured mode: `mode: yolo` would otherwise hand a repo
    /// the agentic loop with no trust prompt and no confirmation, and `mode: auto`
    /// would run Safe commands unconfirmed. Such an override is ignored (the
    /// configured mode is used). De-escalation is always honored, and so is any
    /// **user**-origin override — the user wrote it.
    pub fn effective_mode<'a>(&'a self, configured: &'a str, trusted: bool) -> &'a str {
        let Some(want) = self.mode.as_deref() else {
            return configured;
        };
        if self.needs_trust_confirm(trusted) && mode_rank(want) > mode_rank(configured) {
            configured
        } else {
            want
        }
    }

    /// Substitute `$ARGUMENTS` and `$1`..`$9` in the body.
    pub fn expand(&self, args: &[&str]) -> Expanded {
        let mut text = self.body.replace("$ARGUMENTS", &args.join(" "));
        for n in 1..=9 {
            let val = args.get(n - 1).copied().unwrap_or("");
            text = text.replace(&format!("${n}"), val);
        }
        Expanded {
            text: text.trim().to_string(),
            shell: self.shell,
            mode: self.mode.clone(),
        }
    }
}

/// How much a mode may do without asking: `suggest` < `auto` < `yolo`. Anything
/// unrecognized ranks as `suggest` (the parser already rejects other values).
fn mode_rank(mode: &str) -> u8 {
    match mode {
        "yolo" => 2,
        "auto" => 1,
        _ => 0,
    }
}

/// Escape C0/C1 control characters (and DEL) for terminal display.
///
/// A command body is repo-supplied text: printing it raw lets `\r` plus
/// `ESC[2K` repaint the line, so the "will run:" preview shows one command
/// while a different one executes.
///
// ponytail: blanket-escapes control bytes instead of parsing escape sequences —
// the ceiling is a *faithful* preview, not a pretty one, so a body with real
// tabs/newlines previews as `\x09`/`\x0a`.
pub fn display_safe(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

/// Escape terminal controls while preserving the line structure of prose.
///
/// Model answers and Markdown need real newlines to remain readable. Carriage
/// returns, escape sequences, and the other C0/C1 controls are still rendered
/// visibly so untrusted output cannot repaint the terminal. Tabs are expanded
/// to four spaces for deterministic wrapping and copy/paste.
pub fn display_safe_multiline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push('\n'),
            '\t' => out.push_str("    "),
            '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            _ => out.push(c),
        }
    }
    out
}

/// One-line summary of a shell script for status lines / tool labels.
///
/// Keeps the first non-empty line (truncated) and appends `(+N lines)` when the
/// command is multi-line, so multi-step agent scripts stay scannable.
pub fn command_status_summary(command: &str, limit: usize) -> String {
    let safe = display_safe_multiline(command);
    let lines: Vec<&str> = safe
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .collect();
    let total = lines.len().max(1);
    let first = lines
        .first()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| "run command".into());
    let mut out = first;
    if total > 1 {
        out.push_str(&format!("  (+{} lines)", total - 1));
    }
    if out.chars().count() > limit {
        out = out
            .chars()
            .take(limit.saturating_sub(1))
            .collect::<String>();
        out.push('…');
    }
    out
}

/// Pretty-print lines for an agent approval / detailed tool preview.
///
/// Returns display-safe lines (real newlines already split). Caps body lines and
/// appends a remaining-count line when truncated.
pub fn command_preview_lines(command: &str, max_body_lines: usize) -> Vec<String> {
    let safe = display_safe_multiline(command);
    let raw: Vec<&str> = safe.lines().collect();
    let total = raw.len();
    let max_body = max_body_lines.max(1);
    let mut out = Vec::new();
    for (index, line) in raw.iter().take(max_body).enumerate() {
        if index == 0 {
            out.push(format!("$ {line}"));
        } else {
            out.push(format!("  {line}"));
        }
    }
    if total > max_body {
        out.push(format!("  … (+{} more lines)", total - max_body));
    }
    if out.is_empty() {
        out.push("$ ".into());
    }
    out
}

/// Registry of custom commands, keyed by name (sorted for stable listing).
#[derive(Debug, Default, Clone)]
pub struct CommandRegistry {
    cmds: BTreeMap<String, CustomCommand>,
}

impl CommandRegistry {
    /// Load from the user and project command directories. On a name collision
    /// the **user's** command wins — a project command never shadows it.
    pub fn load() -> Self {
        let mut reg = CommandRegistry::default();
        // `command_dirs` yields user first, then project; the flag marks the origin.
        for (dir, is_project) in command_dirs() {
            reg.load_dir(&dir, is_project);
        }
        reg
    }

    fn load_dir(&mut self, dir: &std::path::Path, is_project: bool) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(&path) {
                let mut cmd = parse_command(stem, &text);
                if is_project {
                    cmd.source = Some(path.clone());
                }
                // ponytail: user-first + `or_insert` means a project command never
                // silently shadows a same-named user command (the user's wins). This
                // changes the old "project overrides user" behavior — documented in
                // CHANGELOG.md ([Unreleased] → Changed).
                self.cmds.entry(cmd.name.clone()).or_insert(cmd);
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&CustomCommand> {
        self.cmds.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cmds.len()
    }

    /// `(name, description)` pairs for completion and listing.
    pub fn list(&self) -> Vec<(String, String)> {
        self.cmds
            .values()
            .map(|c| (c.name.clone(), c.description.clone()))
            .collect()
    }
}

/// Directories searched for command files, each paired with `is_project` (user
/// first with `false`, then the project dir with `true`).
fn command_dirs() -> Vec<(PathBuf, bool)> {
    let mut dirs = Vec::new();
    if let Some(dir) = user_dir() {
        dirs.push((dir, false));
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push((cwd.join(".aishe").join("commands"), true));
    }
    dirs
}

/// The user's own command directory, resolved for this platform. Exposed so the
/// "no custom commands" hint can name the directory aishe actually reads —
/// printing a hardcoded `~/.config/...` is wrong on macOS, where the real
/// location is `~/Library/Application Support/aishe/commands`.
pub fn user_dir() -> Option<PathBuf> {
    crate::config::config_root().map(|c| c.join("aishe").join("commands"))
}

/// Parse a command file: optional `---`-fenced frontmatter, then the body.
fn parse_command(name: &str, text: &str) -> CustomCommand {
    let (meta, body) = split_frontmatter(text);
    CustomCommand {
        name: name.to_string(),
        description: meta
            .get("description")
            .cloned()
            .unwrap_or_else(|| format!("custom command /{name}")),
        shell: meta
            .get("shell")
            .map(|v| matches!(v.as_str(), "true" | "yes" | "1"))
            .unwrap_or(false),
        mode: meta
            .get("mode")
            .filter(|m| matches!(m.as_str(), "suggest" | "auto" | "yolo"))
            .cloned(),
        body: body.trim().to_string(),
        source: None,
    }
}

/// Split a `---`-fenced frontmatter block (simple `key: value` lines) from the
/// body. If there's no frontmatter, returns an empty map and the whole text.
pub(crate) fn split_frontmatter(text: &str) -> (BTreeMap<String, String>, String) {
    let mut meta = BTreeMap::new();
    let trimmed = text.strip_prefix('\u{feff}').unwrap_or(text); // tolerate BOM
    let rest = match trimmed.strip_prefix("---") {
        Some(r) => r.trim_start_matches(['\r', '\n']),
        None => return (meta, text.to_string()),
    };
    // Find the closing fence at the start of a line.
    let mut body_start = None;
    let mut consumed = 0;
    for line in rest.split_inclusive('\n') {
        let l = line.trim_end_matches(['\r', '\n']);
        consumed += line.len();
        if l.trim() == "---" {
            body_start = Some(consumed);
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            meta.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    match body_start {
        Some(start) => (meta, rest[start..].to_string()),
        None => (BTreeMap::new(), text.to_string()), // unterminated → no frontmatter
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_frontmatter_and_body() {
        let c = parse_command(
            "deploy",
            "---\ndescription: Ship it\nmode: yolo\nshell: false\n---\nDeploy $ARGUMENTS now\n",
        );
        assert_eq!(c.name, "deploy");
        assert_eq!(c.description, "Ship it");
        assert_eq!(c.mode.as_deref(), Some("yolo"));
        assert!(!c.shell);
        assert_eq!(c.body, "Deploy $ARGUMENTS now");
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        let c = parse_command("hi", "just a prompt $1");
        assert_eq!(c.body, "just a prompt $1");
        assert!(c.mode.is_none());
        assert!(!c.shell);
        assert!(c.description.contains("/hi"));
    }

    #[test]
    fn shell_flag_and_invalid_mode() {
        let c = parse_command(
            "ll",
            "---\nshell: true\nmode: bogus\n---\nls -lah $ARGUMENTS",
        );
        assert!(c.shell);
        assert!(c.mode.is_none()); // invalid mode ignored
    }

    #[test]
    fn expands_arguments() {
        let c = parse_command("x", "run $1 and $2 over $ARGUMENTS");
        let e = c.expand(&["a", "b", "c"]);
        assert_eq!(e.text, "run a and b over a b c");
    }

    #[test]
    fn missing_positional_is_empty() {
        let c = parse_command("x", "[$1][$2]");
        assert_eq!(c.expand(&["only"]).text, "[only][]");
    }

    #[test]
    fn unterminated_frontmatter_is_body() {
        let c = parse_command("x", "---\ndescription: oops\nno closing fence");
        assert!(c.body.contains("description: oops"));
    }

    // T4.6: a project-origin command is tagged with `source: Some`, a user-origin
    // one with `None`.
    #[test]
    fn origin_is_tagged_by_load_dir() {
        let base = std::env::temp_dir().join(format!("aishe_cmds_origin_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("deploy.md"), "---\nshell: true\n---\nls").unwrap();

        let mut proj = CommandRegistry::default();
        proj.load_dir(&base, true);
        assert!(proj.get("deploy").unwrap().source.is_some());
        assert!(proj.get("deploy").unwrap().shell);

        let mut user = CommandRegistry::default();
        user.load_dir(&base, false);
        assert!(user.get("deploy").unwrap().source.is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    // T4.8: pure unit test on the gate's trust/confirm decision (the core of
    // `main::gate_custom_shell`, extracted as `needs_trust_confirm`). A project
    // command must be confirmed unless trusted; a user command never is.
    #[test]
    fn gate_trust_decision() {
        let mut c = parse_command("deploy", "---\nshell: true\n---\nrm -rf /");

        // User origin (source == None): never needs confirmation, trusted or not.
        assert!(!c.needs_trust_confirm(false));
        assert!(!c.needs_trust_confirm(true));

        // Project origin, untrusted: must be confirmed before running.
        c.source = Some(PathBuf::from("/repo/.aishe/commands/deploy.md"));
        assert!(c.needs_trust_confirm(false));

        // Project origin, trusted (`aishe trust <file>`): runs without a prompt.
        assert!(!c.needs_trust_confirm(true));
    }

    // T6: an untrusted *project* command must not escalate the mode (audit
    // finding #3, the `mode:`-half Task 4 left open).
    #[test]
    fn untrusted_project_mode_escalation_is_ignored() {
        let mut c = parse_command("deploy", "---\nmode: yolo\n---\nship it");
        c.source = Some(PathBuf::from("/repo/.aishe/commands/deploy.md"));

        // Untrusted: the repo-supplied `yolo` is dropped for the user's mode.
        assert_eq!(c.effective_mode("suggest", false), "suggest");
        assert_eq!(c.effective_mode("auto", false), "auto");
        // Trusted (`aishe trust <file>`): honored.
        assert_eq!(c.effective_mode("suggest", true), "yolo");
        // Already at yolo: no escalation, so honoring it changes nothing.
        assert_eq!(c.effective_mode("yolo", false), "yolo");

        // `auto` is the same escalation one notch weaker.
        c.mode = Some("auto".to_string());
        assert_eq!(c.effective_mode("suggest", false), "suggest");
        assert_eq!(c.effective_mode("yolo", false), "auto"); // de-escalation is fine
    }

    // A *user*-origin override is always honored — the user wrote the file.
    #[test]
    fn user_command_mode_override_is_honored() {
        let c = parse_command("deploy", "---\nmode: yolo\n---\nship it");
        assert!(c.source.is_none());
        assert_eq!(c.effective_mode("suggest", false), "yolo");
        assert_eq!(c.effective_mode("suggest", true), "yolo");

        // No override at all → the configured mode, whatever the origin.
        let mut plain = parse_command("x", "just a prompt");
        assert_eq!(plain.effective_mode("auto", false), "auto");
        plain.source = Some(PathBuf::from("/repo/.aishe/commands/x.md"));
        assert_eq!(plain.effective_mode("auto", false), "auto");
    }

    // The `will run:` preview must not be repaintable by the command body.
    #[test]
    fn display_safe_escapes_terminal_controls() {
        assert_eq!(
            display_safe("ls\r\u{1b}[2Krm -rf /"),
            "ls\\x0d\\x1b[2Krm -rf /"
        );
        assert_eq!(display_safe("echo hi"), "echo hi"); // untouched
        assert_eq!(display_safe("a\u{7f}b\u{9b}c"), "a\\x7fb\\x9bc");
    }

    #[test]
    fn display_safe_multiline_preserves_layout_but_not_repainting_controls() {
        assert_eq!(
            display_safe_multiline("# Audit\n\n- **ok**\n```bash\nprintf 'ok'\n```\n"),
            "# Audit\n\n- **ok**\n```bash\nprintf 'ok'\n```\n"
        );
        assert_eq!(
            display_safe_multiline("one\r\u{1b}[2J\ttwo"),
            "one\\x0d\\x1b[2J    two"
        );
    }

    #[test]
    fn command_status_summary_shows_first_line_and_extra_count() {
        assert_eq!(
            command_status_summary("set -eu\nuser=mj1\nid \"$user\"\n", 180),
            "set -eu  (+2 lines)"
        );
        assert_eq!(command_status_summary("echo hi", 180), "echo hi");
        assert!(!command_status_summary("a\nb\n", 180).contains("\\x0a"));
    }

    #[test]
    fn command_preview_lines_are_readable_scripts() {
        let lines = command_preview_lines("set -eu\necho ready\ntrue\n", 2);
        assert_eq!(lines[0], "$ set -eu");
        assert_eq!(lines[1], "  echo ready");
        assert!(lines.iter().any(|line| line.contains("+1 more")));
        assert!(!lines.join("\n").contains("\\x0a"));
    }

    // T4.7: a project command must not overwrite a same-named user command.
    #[test]
    fn project_does_not_overwrite_user() {
        let base = std::env::temp_dir().join(format!("aishe_cmds_dup_{}", std::process::id()));
        let user_dir = base.join("user");
        let proj_dir = base.join("proj");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::create_dir_all(&proj_dir).unwrap();
        std::fs::write(user_dir.join("dup.md"), "user body").unwrap();
        std::fs::write(proj_dir.join("dup.md"), "project body").unwrap();

        let mut reg = CommandRegistry::default();
        reg.load_dir(&user_dir, false); // user first
        reg.load_dir(&proj_dir, true); // project second must not clobber

        let c = reg.get("dup").unwrap();
        assert_eq!(c.body, "user body");
        assert!(c.source.is_none());

        let _ = std::fs::remove_dir_all(&base);
    }
}
