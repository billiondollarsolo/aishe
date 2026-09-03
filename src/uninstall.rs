//! Explicit, category-based uninstall planning.
//!
//! The default removes only installed program artifacts and managed runtimes.
//! User state is opt-in, separately named, and never inferred from a parent
//! directory. Planning is side-effect free so the CLI can preview the exact
//! targets before asking for confirmation.

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    pub binary: bool,
    pub runtime: bool,
    pub sessions: bool,
    pub config: bool,
    pub history: bool,
    pub audit_undo: bool,
}

impl Selection {
    pub fn has_explicit_category(self) -> bool {
        self.binary
            || self.runtime
            || self.sessions
            || self.config
            || self.history
            || self.audit_undo
    }

    pub fn with_default(self) -> Self {
        if self.has_explicit_category() {
            self
        } else {
            Self {
                binary: true,
                runtime: true,
                ..Self::default()
            }
        }
    }

    pub fn includes_user_state(self) -> bool {
        self.sessions || self.config || self.history || self.audit_undo
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TargetKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub category: &'static str,
    pub path: PathBuf,
    pub kind: TargetKind,
    pub recoverable: bool,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub selection: Selection,
    pub targets: Vec<Target>,
}

impl Plan {
    pub fn discover(selection: Selection) -> Result<Self> {
        let selection = selection.with_default();
        let config_dir = crate::config::Config::path()
            .parent()
            .context("AIShe config path has no parent")?
            .to_path_buf();
        let data_dir = crate::config::data_root()
            .context("cannot resolve AIShe data directory")?
            .join("aishe");
        let mut targets = Vec::new();

        if selection.binary {
            add_program_artifacts(&mut targets)?;
        }
        if selection.runtime {
            targets.push(directory(
                "managed runtime/cache",
                data_dir.join("runtime"),
                true,
            ));
        }
        if selection.sessions {
            for path in [
                data_dir.join("tasks"),
                data_dir.join("backend").join("sessions"),
                data_dir.join("backend").join("journal"),
            ] {
                targets.push(directory("AI sessions/tool journals", path, false));
            }
        }
        if selection.config {
            for path in [
                crate::config::Config::path(),
                crate::credentials::path(),
                config_dir.join("aishrc"),
                data_dir.join("setup-draft.json"),
                data_dir.join("tour-state.json"),
            ] {
                targets.push(file("config/credentials", path, false));
            }
            for path in [config_dir.join("commands"), config_dir.join("skills")] {
                targets.push(directory("config/credentials", path, false));
            }
            // Managed OAuth tokens and per-profile authentication state live
            // under the private OpenCode XDG root. They are credentials, not
            // conversation/session history: `--sessions` must preserve them.
            targets.push(directory(
                "config/credentials",
                data_dir.join("backend").join("opencode").join("xdg"),
                false,
            ));
        }
        if selection.history {
            for path in [data_dir.join("history.ext"), data_dir.join("history.vec")] {
                targets.push(file("shell history", path, false));
            }
        }
        if selection.audit_undo {
            let loaded = crate::config::Config::load_quiet().ok().flatten();
            let audit = std::env::var_os("AISHE_LOG_FILE")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .or_else(|| {
                    loaded
                        .as_ref()
                        .and_then(|config| config.logging.file.as_ref())
                        .map(PathBuf::from)
                })
                .unwrap_or_else(|| data_dir.join("audit.jsonl"));
            let undo = std::env::var_os("AISHE_UNDO_JOURNAL")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| data_dir.join("undo.jsonl"));
            // The usage ledger is separate from the audit log but is the same
            // category of retained local record, so it goes with it.
            let ledger = data_dir.join("usage.jsonl");
            for path in [
                audit.clone(),
                suffixed(&audit, ".1"),
                undo.clone(),
                suffixed(&undo, ".1"),
                ledger.clone(),
                suffixed(&ledger, ".1"),
            ] {
                targets.push(file("audit/undo data", path, false));
            }
        }

        let current_dir = std::env::current_dir().context("resolving current directory")?;
        for target in &mut targets {
            if !target.path.is_absolute() {
                target.path = current_dir.join(&target.path);
            }
        }
        targets.sort_by(|left, right| {
            left.category
                .cmp(right.category)
                .then_with(|| left.path.cmp(&right.path))
        });
        targets.dedup_by(|left, right| left.path == right.path);
        for target in &targets {
            validate_target(target, &config_dir, &data_dir)?;
        }
        Ok(Self { selection, targets })
    }

    pub fn existing_targets(&self) -> Vec<&Target> {
        self.targets
            .iter()
            .filter(|target| fs::symlink_metadata(&target.path).is_ok())
            .collect()
    }

    pub fn apply(&self) -> Result<Vec<PathBuf>> {
        let mut removed = Vec::new();
        // Stop before deleting runtime or backend state. A stale/missing
        // supervisor is already a successful no-op.
        if self.selection.runtime || self.selection.sessions {
            let _ = crate::backend::control::request_stop();
        }
        // Remove the running binary last. If an earlier target is denied by
        // permissions, the user still has a working `aishe` command to inspect
        // and retry the remaining plan.
        let ordered = self
            .targets
            .iter()
            .filter(|target| target.category != "binary/completions/man")
            .chain(
                self.targets
                    .iter()
                    .filter(|target| target.category == "binary/completions/man"),
            );
        for target in ordered {
            let metadata = match fs::symlink_metadata(&target.path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("inspecting {}", target.path.display()));
                }
            };
            if metadata.file_type().is_symlink() || metadata.is_file() {
                fs::remove_file(&target.path)
                    .with_context(|| format!("removing {}", target.path.display()))?;
            } else if metadata.is_dir() {
                fs::remove_dir_all(&target.path)
                    .with_context(|| format!("removing {}", target.path.display()))?;
            } else {
                anyhow::bail!(
                    "refusing to remove unsupported filesystem object {}",
                    target.path.display()
                );
            }
            removed.push(target.path.clone());
        }
        Ok(removed)
    }
}

fn add_program_artifacts(targets: &mut Vec<Target>) -> Result<()> {
    let executable = std::env::current_exe().context("locating the running AIShe binary")?;
    if executable.file_name().and_then(|value| value.to_str()) == Some("aishe") {
        targets.push(file("binary/completions/man", executable, true));
    }
    let home = dirs::home_dir().context("cannot resolve the home directory")?;
    for path in [
        home.join(".local/share/bash-completion/completions/aishe"),
        home.join(".local/share/zsh/site-functions/_aishe"),
        home.join(".local/share/fish/vendor_completions.d/aishe.fish"),
        home.join(".local/share/man/man1/aishe.1"),
    ] {
        targets.push(file("binary/completions/man", path, true));
    }
    for path in [
        PathBuf::from("/usr/local/share/man/man1/aishe.1"),
        PathBuf::from("/usr/share/man/man1/aishe.1"),
        PathBuf::from("/usr/local/share/bash-completion/completions/aishe"),
        PathBuf::from("/usr/share/bash-completion/completions/aishe"),
        PathBuf::from("/usr/local/share/zsh/site-functions/_aishe"),
        PathBuf::from("/usr/share/zsh/site-functions/_aishe"),
        PathBuf::from("/usr/local/share/fish/vendor_completions.d/aishe.fish"),
        PathBuf::from("/usr/share/fish/vendor_completions.d/aishe.fish"),
    ] {
        targets.push(file("binary/completions/man", path, true));
    }
    Ok(())
}

fn file(category: &'static str, path: PathBuf, recoverable: bool) -> Target {
    Target {
        category,
        path,
        kind: TargetKind::File,
        recoverable,
    }
}

fn directory(category: &'static str, path: PathBuf, recoverable: bool) -> Target {
    Target {
        category,
        path,
        kind: TargetKind::Directory,
        recoverable,
    }
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_target(target: &Target, config_dir: &Path, data_dir: &Path) -> Result<()> {
    if !target.path.is_absolute() {
        anyhow::bail!(
            "refusing non-absolute uninstall target {}",
            target.path.display()
        );
    }
    if target
        .path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        anyhow::bail!(
            "refusing uninstall target containing '..': {}",
            target.path.display()
        );
    }
    if matches!(target.kind, TargetKind::Directory) {
        if target.path == Path::new("/") || target.path == config_dir || target.path == data_dir {
            anyhow::bail!(
                "refusing broad uninstall directory {}",
                target.path.display()
            );
        }
        let allowed = target.path.starts_with(config_dir) || target.path.starts_with(data_dir);
        if !allowed {
            anyhow::bail!(
                "refusing uninstall directory outside AIShe roots: {}",
                target.path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> Selection {
        Selection {
            binary: false,
            runtime: false,
            sessions: false,
            config: false,
            history: false,
            audit_undo: false,
        }
    }

    #[test]
    fn default_selection_preserves_every_user_state_category() {
        let selected = selection().with_default();
        assert!(selected.binary);
        assert!(selected.runtime);
        assert!(!selected.includes_user_state());
    }

    #[test]
    fn destructive_categories_are_detected() {
        let mut selected = selection();
        selected.history = true;
        assert!(selected.includes_user_state());
    }

    #[test]
    fn broad_directory_targets_are_rejected() {
        let root = PathBuf::from("/tmp/aishe-uninstall-test");
        let target = directory("test", root.clone(), false);
        assert!(validate_target(&target, &root, &root).is_err());
    }
}
