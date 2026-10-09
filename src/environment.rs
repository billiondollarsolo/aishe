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
    inspect_with_environment(config, cwd, |name| std::env::var(name).ok(), true)
}

/// Classify the environment the native agent will actually execute in. Live
/// exports and removals must not be replaced by the parent's launch identity.
pub fn inspect_executor(config: &Config, executor: &crate::executor::Executor) -> Identity {
    inspect_with_environment(
        config,
        executor.cwd(),
        |name| executor.execution_environment(name).map(str::to_owned),
        false,
    )
}

fn inspect_with_environment(
    config: &Config,
    cwd: &Path,
    environment: impl Fn(&str) -> Option<String>,
    parent_environment: bool,
) -> Identity {
    let hostname = safe(
        environment("HOSTNAME")
            .or_else(|| environment("HOST"))
            .unwrap_or_else(|| read_small(Path::new("/etc/hostname")).unwrap_or("unknown".into())),
    );
    let git_branch = git(
        cwd,
        &["symbolic-ref", "--short", "-q", "HEAD"],
        &environment,
    );
    let git_head = git(cwd, &["rev-parse", "--short=12", "HEAD"], &environment);
    let kubernetes_context = kube_context(&environment, parent_environment);
    let cloud_profile = [
        "AWS_PROFILE",
        "AWS_DEFAULT_PROFILE",
        "GOOGLE_CLOUD_PROJECT",
        "CLOUDSDK_CORE_PROJECT",
        "AZURE_SUBSCRIPTION_ID",
    ]
    .iter()
    .find_map(|name| environment(name).filter(|value| !value.trim().is_empty()))
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
        ssh: environment("SSH_CONNECTION").is_some() || environment("SSH_TTY").is_some(),
        container: Path::new("/.dockerenv").exists()
            || environment("container").is_some()
            || environment("KUBERNETES_SERVICE_HOST").is_some(),
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
    confirm_protected_identity(inspect(config, cwd))
}

pub fn confirm_protected_host_for_executor(
    config: &Config,
    executor: &crate::executor::Executor,
) -> anyhow::Result<()> {
    let identity = inspect_executor(config, executor);
    if identity.protected && executor.terminal_input_owned_elsewhere() {
        anyhow::bail!(
            "protected host target {} needs a separate terminal confirmation; run `aishe agent --scope host` in this shell, or use workspace scope",
            identity.label()
        );
    }
    confirm_protected_identity(identity)
}

fn confirm_protected_identity(identity: Identity) -> anyhow::Result<()> {
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
        if let Some(pattern) = &self.matched_pattern {
            if let Some(value) = [
                self.kubernetes_context.as_deref(),
                self.cloud_profile.as_deref(),
                self.git_branch.as_deref(),
                Some(self.hostname.as_str()),
            ]
            .into_iter()
            .flatten()
            .find(|value| pattern_matches(pattern, value))
            {
                return value.to_owned();
            }
        }
        self.kubernetes_context
            .clone()
            .or_else(|| self.git_branch.clone())
            .or_else(|| self.cloud_profile.clone())
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

fn git(cwd: &Path, args: &[&str], environment: &impl Fn(&str) -> Option<String>) -> Option<String> {
    // The binary and loader environment stay tied to the trusted parent. Live
    // PATH selects agent tools, not the executable performing admission.
    let binary = crate::executor::which("git")?.canonicalize().ok()?;
    let mut command = Command::new(binary);
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_CEILING_DIRECTORIES",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    ] {
        command.env_remove(name);
        if let Some(value) = environment(name) {
            command.env(name, value);
        }
    }
    let output = command.args(args).current_dir(cwd).output().ok()?;
    output
        .status
        .success()
        .then(|| safe(String::from_utf8_lossy(&output.stdout).trim().to_string()))
        .filter(|value| !value.is_empty())
}

fn kube_context(
    environment: &impl Fn(&str) -> Option<String>,
    parent_environment: bool,
) -> Option<String> {
    let paths = environment("KUBECONFIG")
        .filter(|value| !value.is_empty())
        .map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .unwrap_or_else(|| {
            environment("HOME")
                .map(std::path::PathBuf::from)
                .or_else(|| parent_environment.then(dirs::home_dir).flatten())
                .map(|home| vec![home.join(".kube/config")])
                .unwrap_or_default()
        });
    paths.iter().find_map(|path| {
        let text = read_small(path)?;
        text.lines()
            .find_map(|line| line.trim().strip_prefix("current-context:"))
            .map(|value| safe(value.trim().trim_matches(['\'', '"']).to_string()))
            .filter(|value| !value.is_empty())
    })
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
    use std::collections::{HashMap, HashSet};

    #[test]
    fn protected_patterns_respect_boundaries_and_wildcards() {
        assert!(pattern_matches("prod", "api-prod-us"));
        assert!(!pattern_matches("prod", "product-development"));
        assert!(pattern_matches("production-*", "production-east"));
        assert!(!pattern_matches("production-*", "staging-east"));
    }

    #[test]
    fn native_protection_uses_live_cloud_profile_and_propagates_unset() {
        let root = std::env::temp_dir().join(format!(
            "aishe-live-identity-{:016x}",
            rand::random::<u64>()
        ));
        std::fs::create_dir(&root).unwrap();
        let parent_profile = std::env::var_os("AWS_PROFILE");
        let mut config = Config::default();
        config.sandbox.protected_environment_patterns = vec!["production-*".into()];
        let mut executor = crate::executor::Executor::new_agent(&root, &HashSet::new()).unwrap();
        executor.replace_agent_environment(
            HashMap::from([
                ("HOSTNAME".into(), "development".into()),
                ("AWS_PROFILE".into(), "production-east".into()),
            ]),
            &HashSet::new(),
        );
        let identity = inspect_executor(&config, &executor);
        assert!(identity.protected);
        assert_eq!(identity.cloud_profile.as_deref(), Some("production-east"));
        assert_eq!(identity.label(), "production-east");
        executor.set_terminal_input_owned_elsewhere(true);
        assert!(
            confirm_protected_host_for_executor(&config, &executor).is_err(),
            "headless protected work must fail closed"
        );
        let refusal = confirm_protected_host_for_executor(&config, &executor)
            .unwrap_err()
            .to_string();
        assert!(refusal.contains("separate terminal confirmation"));
        assert!(refusal.contains("aishe agent --scope host"));
        executor.replace_agent_environment(
            HashMap::from([("HOSTNAME".into(), "development".into())]),
            &HashSet::new(),
        );
        let identity = inspect_executor(&config, &executor);
        assert!(identity.cloud_profile.is_none());
        assert!(!identity.protected);
        assert!(confirm_protected_host_for_executor(&config, &executor).is_ok());
        assert_eq!(std::env::var_os("AWS_PROFILE"), parent_profile);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_protection_reads_live_kubeconfig_list_and_live_home_only() {
        let root =
            std::env::temp_dir().join(format!("aishe-live-kube-{:016x}", rand::random::<u64>()));
        std::fs::create_dir_all(root.join(".kube")).unwrap();
        let first = root.join("empty-config");
        let second = root.join("production-config");
        std::fs::write(&first, "contexts: []\n").unwrap();
        std::fs::write(&second, "current-context: production-east\n").unwrap();
        std::fs::write(
            root.join(".kube/config"),
            "current-context: production-home\n",
        )
        .unwrap();
        let parent_config = std::env::var_os("KUBECONFIG");
        let parent_home = std::env::var_os("HOME");
        let mut config = Config::default();
        config.sandbox.protected_environment_patterns = vec!["production-*".into()];
        let mut executor = crate::executor::Executor::new_agent(&root, &HashSet::new()).unwrap();
        executor.replace_agent_environment(
            HashMap::from([
                ("HOSTNAME".into(), "development".into()),
                (
                    "KUBECONFIG".into(),
                    std::env::join_paths([&first, &second])
                        .unwrap()
                        .into_string()
                        .unwrap(),
                ),
            ]),
            &HashSet::new(),
        );
        let identity = inspect_executor(&config, &executor);
        assert!(identity.protected);
        assert_eq!(
            identity.kubernetes_context.as_deref(),
            Some("production-east")
        );
        executor.set_terminal_input_owned_elsewhere(true);
        assert!(confirm_protected_host_for_executor(&config, &executor).is_err());
        executor.replace_agent_environment(
            HashMap::from([
                ("HOSTNAME".into(), "development".into()),
                ("HOME".into(), root.display().to_string()),
            ]),
            &HashSet::new(),
        );
        assert_eq!(
            inspect_executor(&config, &executor)
                .kubernetes_context
                .as_deref(),
            Some("production-home")
        );
        executor.replace_agent_environment(
            HashMap::from([("HOSTNAME".into(), "development".into())]),
            &HashSet::new(),
        );
        assert!(inspect_executor(&config, &executor)
            .kubernetes_context
            .is_none());
        assert_eq!(std::env::var_os("KUBECONFIG"), parent_config);
        assert_eq!(std::env::var_os("HOME"), parent_home);
        std::fs::remove_dir_all(root).unwrap();
    }
}
