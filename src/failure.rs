//! Private, bounded last-failure capsules keyed to one live shell.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u32 = 1;
const MAX_COMMAND_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Capsule {
    pub schema_version: u32,
    pub shell_id: String,
    pub command: String,
    pub exit_status: i32,
    pub cwd: PathBuf,
    pub duration_ms: Option<u64>,
    pub created_at_ms: u128,
    pub redacted: bool,
}

pub fn record_from_env(command: &str) -> Result<u8> {
    let exit_status = std::env::var("AISHE_LAST_EXIT")
        .context("AISHE_LAST_EXIT is required")?
        .parse::<i32>()
        .context("AISHE_LAST_EXIT is invalid")?;
    if exit_status == 0 {
        return clear();
    }
    if command.is_empty() || command.len() > MAX_COMMAND_BYTES {
        anyhow::bail!("failure command must contain 1..={MAX_COMMAND_BYTES} bytes");
    }
    let shell_id = shell_id()?;
    let clean = crate::commands::display_safe(command);
    let command = crate::redact::redact(&clean);
    let capsule = Capsule {
        schema_version: SCHEMA_VERSION,
        shell_id,
        redacted: command != clean,
        command,
        exit_status,
        cwd: std::env::current_dir()?.canonicalize()?,
        duration_ms: std::env::var("AISHE_LAST_DURATION_MS")
            .ok()
            .and_then(|value| value.parse().ok()),
        created_at_ms: now_ms(),
    };
    save(&capsule)?;
    Ok(0)
}

pub fn current() -> Result<Capsule> {
    let path = path()?;
    let metadata =
        std::fs::symlink_metadata(&path).with_context(|| "no failure capsule for this shell")?;
    if !metadata.file_type().is_file() || metadata.len() > 128 * 1024 {
        anyhow::bail!("failure capsule is not a bounded regular file");
    }
    let capsule: Capsule = serde_json::from_slice(&std::fs::read(&path)?)?;
    if capsule.schema_version != SCHEMA_VERSION || capsule.shell_id != shell_id()? {
        anyhow::bail!("failure capsule identity/version mismatch");
    }
    Ok(capsule)
}

pub fn show(json: bool) -> Result<u8> {
    let capsule = current()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&capsule)?);
    } else {
        println!("command: {}", capsule.command);
        println!("exit: {}", capsule.exit_status);
        println!("cwd: {}", capsule.cwd.display());
        if let Some(duration) = capsule.duration_ms {
            println!("duration: {duration}ms");
        }
        if capsule.redacted {
            println!("note: likely secret material was redacted; retry is disabled");
        }
    }
    Ok(0)
}

pub fn clear() -> Result<u8> {
    match std::fs::remove_file(path()?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(0)
}

pub fn retry(execute: bool) -> Result<u8> {
    let capsule = current()?;
    if capsule.redacted {
        anyhow::bail!("retry is disabled because the stored command was redacted");
    }
    if !crate::fix::safe_to_rerun(&capsule.command) {
        println!("{}", capsule.command);
        eprintln!("aishe: unsafe or effectful retry was printed for review and not executed");
        return Ok(20);
    }
    if !execute {
        println!("{}", capsule.command);
        eprintln!("aishe: safe retry preview; add --execute to run it");
        return Ok(0);
    }
    let mut executor = crate::executor::Executor::new()?;
    executor.redirect_cwd(capsule.cwd);
    Ok(executor.run(&capsule.command) as u8)
}

fn save(capsule: &Capsule) -> Result<()> {
    let path = path()?;
    let parent = path.parent().context("failure path has no parent")?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    crate::config::write_atomic(&path, &serde_json::to_vec_pretty(capsule)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn path() -> Result<PathBuf> {
    let root = crate::config::data_root().context("no platform data directory")?;
    let id = shell_id()?;
    let hash = format!("{:x}", Sha256::digest(id.as_bytes()));
    Ok(root
        .join("aishe")
        .join("failures")
        .join(format!("{hash}.json")))
}

fn shell_id() -> Result<String> {
    let id = std::env::var("AISHE_SHELL_ID").context("this command requires an AIShe shell")?;
    if !(8..=128).contains(&id.len())
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        anyhow::bail!("AISHE_SHELL_ID is invalid");
    }
    Ok(id)
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
