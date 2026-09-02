//! Local target identity and explicit protected-environment classification.

use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::config::Config;

#[derive(Clone, Debug, Serialize)]
pub struct Identity {
    pub schema_version: u32,
    pub hostname: String,
    pub ssh: bool,
    pub container: bool,
    pub git_branch: Option<String>,
    pub git_head: Option<String>,
    pub kubernetes_context: Option<String>,
    pub cloud_profile: Option<String>,
    pub protected: bool,
    pub matched_pattern: Option<String>,
}

pub fn inspect(config: &Config, cwd: &Path) -> Identity {
    let hostname = safe(
        std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("HOST"))
            .unwrap_or_else(|_| read_small(Path::new("/etc/hostname")).unwrap_or("unknown".into())),
    );
    let git_branch = git(cwd, &["symbolic-ref", "--short", "-q", "HEAD"]);
    let git_head = git(cwd, &["rev-parse", "--short=12", "HEAD"]);
    let kubernetes_context = kube_context();
    let cloud_profile = [
        "AWS_PROFILE",
        "AWS_DEFAULT_PROFILE",
        "GOOGLE_CLOUD_PROJECT",
        "CLOUDSDK_CORE_PROJECT",
        "AZURE_SUBSCRIPTION_ID",
    ]
    .iter()
    .find_map(|name| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.trim().is_empty())
    })
    .map(safe);
    let candidates = [
        Some(hostname.as_str()),
        git_branch.as_deref(),
        kubernetes_context.as_deref(),
        cloud_profile.as_deref(),
    ];
    let matched_pattern = config
        .sandbox
        .protected_environment_patterns
        .iter()
        .find(|pattern| {
            !pattern.trim().is_empty()
                && candidates
                    .iter()
                    .flatten()
                    .any(|value| pattern_matches(pattern, value))
        })
        .cloned();
    Identity {
        schema_version: 1,
        hostname,
        ssh: std::env::var_os("SSH_CONNECTION").is_some() || std::env::var_os("SSH_TTY").is_some(),
        container: Path::new("/.dockerenv").exists()
            || std::env::var_os("container").is_some()
            || std::env::var_os("KUBERNETES_SERVICE_HOST").is_some(),
        git_branch,
        git_head,
        kubernetes_context,
        cloud_profile,
        protected: matched_pattern.is_some(),
        matched_pattern,
    }
}

/// Require a fresh typed acknowledgement before a yolo turn receives host scope
/// in a protected environment. Noninteractive callers fail closed.
pub fn confirm_protected_host(config: &Config, cwd: &Path) -> anyhow::Result<()> {
    let identity = inspect(config, cwd);
    if !identity.protected {
        return Ok(());
    }
    if !(std::io::IsTerminal::is_terminal(&std::io::stdin())
        && std::io::IsTerminal::is_terminal(&std::io::stdout()))
    {
        anyhow::bail!(
            "host-scope autonomous work is blocked in protected environment {}; use workspace scope",
            identity.label()
        );
    }
    use std::io::Write;
    let expected = format!("host {}", identity.label());
    eprintln!(
        "AIShe protected target: {}. Type `{expected}` to allow host-scope work for this turn.",
        identity.label()
    );
    eprint!("> ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim() != expected {
        anyhow::bail!("protected-environment confirmation did not match; no agent work started");
    }
    Ok(())
}

impl Identity {
    pub fn label(&self) -> String {
        self.kubernetes_context
            .clone()
            .or_else(|| self.git_branch.clone())
            .unwrap_or_else(|| self.hostname.clone())
    }

    pub fn marker(&self) -> String {
        let mut parts = Vec::new();
        if self.protected {
            parts.push("PROD");
        }
        if self.ssh {
            parts.push("SSH");
        }
        if self.container {
            parts.push("container");
        }
        parts.join("/")
    }
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| safe(String::from_utf8_lossy(&output.stdout).trim().to_string()))
        .filter(|value| !value.is_empty())
}

fn kube_context() -> Option<String> {
    let path = std::env::var_os("KUBECONFIG")
        .and_then(|paths| std::env::split_paths(&paths).next())
        .or_else(|| dirs::home_dir().map(|home| home.join(".kube/config")))?;
    let text = read_small(&path)?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("current-context:"))
        .map(|value| safe(value.trim().trim_matches(['\'', '"']).to_string()))
        .filter(|value| !value.is_empty())
}

fn read_small(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 * 1024 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.trim().to_ascii_lowercase();
    let value = value.to_ascii_lowercase();
    if pattern.contains('*') {
        let pieces: Vec<&str> = pattern
            .split('*')
            .filter(|piece| !piece.is_empty())
            .collect();
        let mut rest = value.as_str();
        return pieces.into_iter().all(|piece| {
            let Some(index) = rest.find(piece) else {
                return false;
            };
            rest = &rest[index + piece.len()..];
            true
        });
    }
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|token| token == pattern)
}

fn safe(value: String) -> String {
    crate::commands::display_safe(value.chars().take(160).collect::<String>().trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protected_patterns_respect_boundaries_and_wildcards() {
        assert!(pattern_matches("prod", "api-prod-us"));
        assert!(!pattern_matches("prod", "product-development"));
        assert!(pattern_matches("production-*", "production-east"));
        assert!(!pattern_matches("production-*", "staging-east"));
    }
}
