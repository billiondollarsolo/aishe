//! Session grant for lean modes: ask (default), allow, agent.
//!
//! One typed word per live shell. Dangerous/unknown still need `yes` in allow.

use std::io::IsTerminal;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::agent::ExecutionScope;
use crate::config::Config;

/// Authority accepted by this live IPC session. Workspace grants are bound to
/// the canonical directory shown at acceptance, rather than following `cd`.
#[derive(Default)]
pub struct SessionGrants {
    allow: bool,
    workspace: Option<std::path::PathBuf>,
    host: bool,
}

impl SessionGrants {
    pub fn accepted(&self, mode: LeanMode, host: bool, cwd: &std::path::Path) -> bool {
        match mode {
            LeanMode::Ask => true,
            LeanMode::Allow => self.allow,
            LeanMode::Agent if host => self.host,
            LeanMode::Agent => {
                self.host
                    || self.workspace.as_ref().is_some_and(|root| {
                        cwd.canonicalize().is_ok_and(|cwd| cwd.starts_with(root))
                    })
            }
        }
    }

    pub fn accept(&mut self, mode: LeanMode, host: bool, cwd: &std::path::Path) -> Result<()> {
        match mode {
            LeanMode::Ask => {}
            LeanMode::Allow => self.allow = true,
            LeanMode::Agent if host => self.host = true,
            LeanMode::Agent => {
                let cwd = cwd.canonicalize()?;
                if !self
                    .workspace
                    .as_ref()
                    .is_some_and(|root| cwd.starts_with(root))
                {
                    self.workspace = Some(cwd);
                }
            }
        }
        Ok(())
    }

    pub fn workspace_root(&self) -> Option<&std::path::Path> {
        self.workspace.as_deref()
    }
}

pub fn validate_scope(
    config: &Config,
    mode: LeanMode,
    host: bool,
    cwd: &std::path::Path,
) -> Result<()> {
    if mode != LeanMode::Agent {
        return Ok(());
    }
    if host {
        if !config.sandbox.allow_host_yolo {
            anyhow::bail!("agent host scope is disabled by policy");
        }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    match crate::dependencies::bubblewrap_probe() {
        crate::dependencies::BubblewrapState::Usable { .. } => {}
        state => anyhow::bail!(
            "agent workspace requires functional bubblewrap; current state: {state:?}"
        ),
    }
    let network = crate::agent::NetworkPolicy::parse(&config.backend.workspace_network)
        .unwrap_or(crate::agent::NetworkPolicy::Deny);
    crate::sandbox::agent_bwrap_argv(cwd, cwd, network)?;
    Ok(())
}

static ALLOW_GRANTED: AtomicBool = AtomicBool::new(false);
static AGENT_WORKSPACE_ROOT: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);
static AGENT_HOST_GRANTED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeanMode {
    Ask,
    Allow,
    Agent,
}

impl LeanMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" | "auto" => Self::Allow,
            "agent" | "yolo" => Self::Agent,
            _ => Self::Ask,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::Agent => "agent",
        }
    }

    pub fn glyph(self) -> &'static str {
        match self {
            Self::Ask => "❯",
            Self::Allow => "»",
            Self::Agent => "*",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeanGrant {
    Accepted,
    Declined,
}

pub fn grant_accepted(mode: LeanMode, host: bool) -> bool {
    match mode {
        LeanMode::Ask => true,
        LeanMode::Allow => ALLOW_GRANTED.load(Ordering::SeqCst) || file_contains(&["allow"]),
        LeanMode::Agent if host => {
            AGENT_HOST_GRANTED.load(Ordering::SeqCst) || file_contains(&["agent-host", "host"])
        }
        LeanMode::Agent => {
            std::env::current_dir()
                .ok()
                .is_some_and(|cwd| workspace_grant_root(&cwd).is_some())
                || AGENT_HOST_GRANTED.load(Ordering::SeqCst)
                || file_contains(&["agent-host", "host"])
        }
    }
}

pub fn ensure_session_grant(config: &Config, mode: LeanMode) -> Result<LeanGrant> {
    if mode == LeanMode::Ask {
        return Ok(LeanGrant::Accepted);
    }
    let scope =
        ExecutionScope::parse(&config.backend.default_scope).unwrap_or(ExecutionScope::Workspace);
    let host = scope == ExecutionScope::Host;
    let workspace = std::env::current_dir().context("resolving workspace")?;
    validate_scope(config, mode, host, &workspace)?;
    if grant_accepted(mode, host) {
        remember(mode, host);
        return Ok(LeanGrant::Accepted);
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "{} requires one interactive grant in each AIShe shell (type {})",
            mode.as_str(),
            expected_word(mode, host)
        );
    }

    println!();
    match (mode, host) {
        (LeanMode::Allow, _) => {
            println!("Enter allow · tools for this shell?");
            println!();
            println!(
                "Safe run_command and reads auto-run. Dangerous / unknown require typing yes."
            );
            println!("Workspace: {}", workspace.display());
            print!("\nType allow to continue: ");
        }
        (LeanMode::Agent, false) => {
            println!("Enter agent · workspace?");
            println!();
            println!(
                "The agent may run commands and change files without asking again in this shell."
            );
            println!("  {}", workspace.display());
            #[cfg(target_os = "macos")]
            println!("Warning: macOS workspace mode is not kernel-isolated.");
            print!("\nType agent to continue: ");
        }
        (LeanMode::Agent, true) => {
            println!("Enter agent-host · host?");
            println!();
            println!(
                "The agent may execute any command available to your user without asking again."
            );
            print!("\nType agent-host to continue: ");
        }
        (LeanMode::Ask, _) => unreachable!(),
    }
    std::io::stdout().flush().ok();
    let expected = expected_word(mode, host);
    let answer = crate::promptui::read_terminal_line(true).context("reading session grant")?;
    if answer.as_deref().map(str::trim) != Some(expected) {
        println!("grant declined · mode stays ask");
        return Ok(LeanGrant::Declined);
    }
    persist(expected)?;
    remember(mode, host);
    Ok(LeanGrant::Accepted)
}

fn expected_word(mode: LeanMode, host: bool) -> &'static str {
    match (mode, host) {
        (LeanMode::Allow, _) => "allow",
        (LeanMode::Agent, true) => "agent-host",
        (LeanMode::Agent, false) => "agent",
        (LeanMode::Ask, _) => "",
    }
}

fn remember(mode: LeanMode, host: bool) {
    match (mode, host) {
        (LeanMode::Allow, _) => ALLOW_GRANTED.store(true, Ordering::SeqCst),
        (LeanMode::Agent, true) => AGENT_HOST_GRANTED.store(true, Ordering::SeqCst),
        (LeanMode::Agent, false) => {
            if let Ok(cwd) = std::env::current_dir() {
                let root = workspace_grant_root(&cwd).or_else(|| cwd.canonicalize().ok());
                if let Ok(mut stored) = AGENT_WORKSPACE_ROOT.lock() {
                    *stored = root;
                }
            }
        }
        (LeanMode::Ask, _) => {}
    }
}

fn file_contains(markers: &[&str]) -> bool {
    let Ok(path) = std::env::var("AISHE_ACCEPTANCE_FILE") else {
        return false;
    };
    if path.is_empty() {
        return false;
    }
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|text| text.lines().any(|line| markers.contains(&line.trim())))
}

pub(super) fn workspace_grant_root(cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    let cwd = cwd.canonicalize().ok()?;
    if let Some(root) = AGENT_WORKSPACE_ROOT
        .lock()
        .ok()
        .and_then(|root| root.clone())
    {
        if root.canonicalize().is_ok_and(|canonical| canonical == root) && cwd.starts_with(&root) {
            return Some(root);
        }
    }
    let path = std::env::var("AISHE_ACCEPTANCE_FILE").ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    rooted_marker(&text, &cwd)
}

fn rooted_marker(text: &str, cwd: &std::path::Path) -> Option<std::path::PathBuf> {
    text.lines()
        .filter_map(|line| {
            let (marker, root) = line.split_once('\t')?;
            if !matches!(marker, "agent" | "workspace") {
                return None;
            }
            let root = std::path::PathBuf::from(root);
            if !root.is_absolute() || root.canonicalize().ok()? != root || !cwd.starts_with(&root) {
                return None;
            }
            Some(root)
        })
        .max_by_key(|root| root.components().count())
}

fn persist(marker: &str) -> Result<()> {
    let Ok(path) = std::env::var("AISHE_ACCEPTANCE_FILE") else {
        return Ok(());
    };
    if path.is_empty() {
        return Ok(());
    }
    let marker = if marker == "agent" {
        let root = std::env::current_dir()?.canonicalize()?;
        let root = root.to_string_lossy();
        if root.contains(['\n', '\r', '\t']) {
            anyhow::bail!("workspace path cannot be stored in a shell grant");
        }
        format!("agent\t{root}")
    } else {
        marker.to_string()
    };
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;
    writeln!(file, "{marker}")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_needs_no_grant() {
        assert!(grant_accepted(LeanMode::Ask, false));
        assert_eq!(expected_word(LeanMode::Allow, false), "allow");
        assert_eq!(expected_word(LeanMode::Agent, false), "agent");
        assert_eq!(expected_word(LeanMode::Agent, true), "agent-host");
    }

    #[test]
    fn grants_belong_to_one_live_session_and_do_not_escalate() {
        let cwd = std::env::current_dir().unwrap();
        let mut shell = SessionGrants::default();
        shell.accept(LeanMode::Allow, false, &cwd).unwrap();
        assert!(shell.accepted(LeanMode::Allow, false, &cwd));
        assert!(!shell.accepted(LeanMode::Agent, false, &cwd));
        assert!(!SessionGrants::default().accepted(LeanMode::Allow, false, &cwd));
        shell.accept(LeanMode::Agent, false, &cwd).unwrap();
        assert!(shell.accepted(LeanMode::Agent, false, &cwd));
        assert!(!shell.accepted(LeanMode::Agent, true, &cwd));
        shell
            .accept(LeanMode::Agent, false, &cwd.join("src"))
            .unwrap();
        assert_eq!(shell.workspace_root(), Some(cwd.as_path()));
        assert!(shell.accepted(LeanMode::Agent, false, &cwd));
        let ancestor = cwd.parent().unwrap();
        assert!(!shell.accepted(LeanMode::Agent, false, ancestor));
    }

    #[test]
    fn cached_host_grant_cannot_override_host_policy() {
        let cwd = std::env::current_dir().unwrap();
        let mut shell = SessionGrants::default();
        shell.accept(LeanMode::Agent, true, &cwd).unwrap();
        assert!(shell.accepted(LeanMode::Agent, true, &cwd));
        let mut config = Config::default();
        config.sandbox.allow_host_yolo = false;
        assert!(validate_scope(&config, LeanMode::Agent, true, &cwd).is_err());
    }

    #[test]
    fn workspace_markers_bind_the_root_and_reject_legacy_unbound_grants() {
        let root = std::env::current_dir().unwrap().canonicalize().unwrap();
        let marker = format!("allow\nagent\t{}\n", root.display());
        assert_eq!(rooted_marker(&marker, &root), Some(root.clone()));
        assert_eq!(
            rooted_marker(&marker, &root.join("src")),
            Some(root.clone())
        );
        assert_eq!(rooted_marker(&marker, root.parent().unwrap()), None);
        assert_eq!(rooted_marker("agent\nworkspace\n", &root), None);
    }

    #[test]
    fn ipc_handshake_records_scope_and_rechecks_policy() {
        let mut config = Config::default();
        let mut provider = None;
        let mut executor = crate::executor::Executor::new().unwrap();
        let mut session = crate::session::Session::new(false);
        let mut store = None;
        let mut warm = crate::lean::LeanWarm::default();
        let pty = crate::lean::PtyOut::capture();
        let cwd = std::env::current_dir().unwrap();
        let mut request = |config: &mut Config, raw: &str| {
            crate::lean::handle_ipc_line(
                config,
                &mut provider,
                &mut executor,
                &mut session,
                &mut store,
                &mut warm,
                &pty,
                &format!("{raw}\t{}", cwd.display()),
            )
        };
        assert_eq!(request(&mut config, "MODE_CHECK\tallow"), "GRANT_REQUIRED");
        assert_eq!(
            request(&mut config, "MODE_ACCEPT\t AUTO "),
            "MODE_OK\tallow\tworkspace\t"
        );
        assert_eq!(request(&mut config, "MODE_CHECK\tallow"), "ACCEPTED");
        assert_eq!(
            request(&mut config, "MODE_ACCEPT\tagent-host"),
            "MODE_OK\tagent\thost\t"
        );
        assert_eq!(config.backend.default_scope, "host");
        config.sandbox.allow_host_yolo = false;
        assert!(request(&mut config, "MODE_CHECK\tagent-host").contains("disabled by policy"));
        assert!(request(&mut config, "MODE_ACCEPT\tagent-host").contains("disabled by policy"));
    }
}
