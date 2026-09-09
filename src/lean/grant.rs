//! Session grant for lean modes: ask (default), allow, agent.
//!
//! One typed word per live shell. Dangerous/unknown still need `yes` in allow.

use std::io::IsTerminal;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

use crate::agent::ExecutionScope;
use crate::config::Config;

static ALLOW_GRANTED: AtomicBool = AtomicBool::new(false);
static AGENT_WORKSPACE_GRANTED: AtomicBool = AtomicBool::new(false);
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
            AGENT_WORKSPACE_GRANTED.load(Ordering::SeqCst)
                || AGENT_HOST_GRANTED.load(Ordering::SeqCst)
                || file_contains(&["agent", "agent-host", "workspace", "host"])
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

    let workspace = std::env::current_dir().context("resolving workspace")?;
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
            #[cfg(target_os = "linux")]
            match crate::dependencies::bubblewrap_probe() {
                crate::dependencies::BubblewrapState::Usable { .. } => {}
                state => anyhow::bail!(
                    "agent workspace requires functional bubblewrap; current state: {state:?}"
                ),
            }
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
            if !config.sandbox.allow_host_yolo {
                anyhow::bail!("agent host scope is disabled by policy");
            }
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
        (LeanMode::Agent, false) => AGENT_WORKSPACE_GRANTED.store(true, Ordering::SeqCst),
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

fn persist(marker: &str) -> Result<()> {
    let Ok(path) = std::env::var("AISHE_ACCEPTANCE_FILE") else {
        return Ok(());
    };
    if path.is_empty() {
        return Ok(());
    }
    std::fs::write(&path, format!("{marker}\n"))?;
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
}
