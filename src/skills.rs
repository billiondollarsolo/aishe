//! Model-invoked **skills** — the progressive-disclosure half of Claude Code's
//! skills. Unlike user-invoked `/commands`, skills are selected by the *model*:
//! the yolo loop is told the available skill names + descriptions, and can pull
//! a skill's full instructions into context on demand via a `use_skill` tool.
//!
//! Skills are discovered from:
//!   - `~/.config/aishe/skills/<name>/SKILL.md` or `~/.config/aishe/skills/*.md`
//!   - the same under `<cwd>/.aishe/skills/` (project — overrides user by name)
//!
//! A skill file is Markdown with frontmatter:
//! ```text
//! ---
//! name: rust-release            # optional; defaults to the file/dir stem
//! description: How to cut a Rust release (when to bump, tag, publish)
//! ---
//! <full instructions the model loads when it decides this skill is relevant>
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A model-invokable skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Registry of skills, keyed by name.
#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    skills: BTreeMap<String, Skill>,
}

impl SkillRegistry {
    /// Load from the user and project skill directories (project overrides).
    pub fn load() -> Self {
        let mut reg = SkillRegistry::default();
        for dir in skill_dirs() {
            reg.load_dir(&dir);
        }
        reg
    }

    fn load_dir(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // `<name>/SKILL.md` (directory) or `<name>.md` (flat file).
            let (stem, file) = if path.is_dir() {
                match path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => (s.to_string(), path.join("SKILL.md")),
                    None => continue,
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
                match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => (s.to_string(), path.clone()),
                    None => continue,
                }
            } else {
                continue;
            };
            if let Ok(text) = std::fs::read_to_string(&file) {
                if let Some(skill) = parse_skill(&stem, &text) {
                    self.skills.insert(skill.name.clone(), skill);
                }
            }
        }
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// `(name, description)` pairs (for listing).
    pub fn list(&self) -> Vec<(String, String)> {
        self.skills
            .values()
            .map(|s| (s.name.clone(), s.description.clone()))
            .collect()
    }

    /// A compact catalog injected into the model's system prompt: one
    /// `- name: description` line per skill. Empty string when there are none.
    pub fn catalog(&self) -> String {
        self.skills
            .values()
            .map(|s| format!("- {}: {}", s.name, s.description))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn skill_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(cfg) = dirs::config_dir() {
        dirs.push(cfg.join("aishe").join("skills"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join(".aishe").join("skills"));
    }
    dirs
}

/// Parse a skill file. Requires a non-empty body; `name`/`description` come from
/// frontmatter (falling back to the stem).
fn parse_skill(stem: &str, text: &str) -> Option<Skill> {
    let (meta, body) = crate::commands::split_frontmatter(text);
    let body = body.trim().to_string();
    if body.is_empty() {
        return None;
    }
    let name = meta
        .get("name")
        .cloned()
        .unwrap_or_else(|| stem.to_string());
    let description = meta
        .get("description")
        .cloned()
        .unwrap_or_else(|| format!("skill {name}"));
    Some(Skill {
        name,
        description,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_skill_with_frontmatter() {
        let s = parse_skill(
            "deploy",
            "---\nname: release\ndescription: how to release\n---\nStep 1. Bump version.\n",
        )
        .unwrap();
        assert_eq!(s.name, "release"); // frontmatter name wins over stem
        assert_eq!(s.description, "how to release");
        assert_eq!(s.body, "Step 1. Bump version.");
    }

    #[test]
    fn defaults_name_to_stem() {
        let s = parse_skill("gitflow", "do the thing").unwrap();
        assert_eq!(s.name, "gitflow");
        assert!(s.description.contains("gitflow"));
    }

    #[test]
    fn empty_body_is_rejected() {
        assert!(parse_skill("x", "---\ndescription: nothing\n---\n").is_none());
    }

    #[test]
    fn catalog_lists_descriptions() {
        let mut reg = SkillRegistry::default();
        reg.skills.insert(
            "a".into(),
            Skill {
                name: "a".into(),
                description: "does A".into(),
                body: "x".into(),
            },
        );
        assert_eq!(reg.catalog(), "- a: does A");
    }
}
