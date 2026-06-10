//! User-defined slash commands ("plugins" / "skills"), modeled on Claude Code's
//! file-based custom commands.
//!
//! Commands are Markdown files discovered from:
//!   - `~/.config/aishe/commands/*.md`  (user)
//!   - `<cwd>/.aishe/commands/*.md`      (project — overrides user by name)
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
}

/// The result of expanding a command with arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct Expanded {
    pub text: String,
    pub shell: bool,
    pub mode: Option<String>,
}

impl CustomCommand {
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

/// Registry of custom commands, keyed by name (sorted for stable listing).
#[derive(Debug, Default, Clone)]
pub struct CommandRegistry {
    cmds: BTreeMap<String, CustomCommand>,
}

impl CommandRegistry {
    /// Load from the user and project command directories (project overrides).
    pub fn load() -> Self {
        let mut reg = CommandRegistry::default();
        for dir in command_dirs() {
            reg.load_dir(&dir);
        }
        reg
    }

    fn load_dir(&mut self, dir: &std::path::Path) {
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
                let cmd = parse_command(stem, &text);
                self.cmds.insert(cmd.name.clone(), cmd);
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

/// Directories searched for command files (user first, then project).
fn command_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        dirs.push(cfg.join("aishe").join("commands"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".aishe").join("commands"));
    }
    dirs
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
}
