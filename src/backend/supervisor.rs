//! Private OpenCode process preparation and lifecycle.
//!
//! The long-lived supervisor/control protocol is built on the same isolated
//! layout and launch contract used by `smoke_test`; keeping the smoke path real
//! prevents setup from reporting a binary as ready when its server/plugin
//! configuration cannot actually start.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::RuntimeManager;

const PLUGIN: &[u8] = include_bytes!("../../assets/backend/opencode/aishe-plugin.mjs");
const SUPERVISOR_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct PreparedRuntime {
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub plugin_path: PathBuf,
    pub config_json: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SupervisorState {
    schema_version: u32,
    protocol_version: u32,
    supervisor_pid: u32,
    opencode_pid: u32,
    opencode_url: String,
    runtime_version: String,
    startup_nonce: String,
    started_at_ms: u128,
}

pub fn backend_root() -> Result<PathBuf> {
    Ok(crate::config::data_root()
        .context("cannot resolve Aishe data directory")?
        .join("aishe")
        .join("backend"))
}

pub fn prepare_layout() -> Result<PreparedRuntime> {
    let root = backend_root()?.join("opencode");
    let home = root.join("home");
    let config_dir = root.join("config");
    let data_dir = root.join("xdg").join("data");
    let cache_dir = root.join("xdg").join("cache");
    let state_dir = root.join("xdg").join("state");
    for directory in [
        &root,
        &home,
        &config_dir,
        &config_dir.join("plugins"),
        &data_dir,
        &cache_dir,
        &state_dir,
    ] {
        fs::create_dir_all(directory)
            .with_context(|| format!("creating private backend path {}", directory.display()))?;
        crate::config::set_private_dir(directory);
    }

    let plugin_path = config_dir.join("plugins").join("aishe-plugin.mjs");
    let expected = env!("AISHE_OPENCODE_PLUGIN_SHA256");
    let current_matches = fs::read(&plugin_path)
        .ok()
        .map(|bytes| sha256_bytes(&bytes) == expected)
        .unwrap_or(false);
    if !current_matches {
        crate::config::write_atomic(&plugin_path, PLUGIN)
            .with_context(|| format!("writing trusted plugin {}", plugin_path.display()))?;
    }
    let installed = sha256_bytes(&fs::read(&plugin_path)?);
    if installed != expected {
        anyhow::bail!("trusted OpenCode plugin checksum verification failed");
    }

    let config = generated_base_config();
    Ok(PreparedRuntime {
        root,
        home,
        config_dir,
        data_dir,
        cache_dir,
        state_dir,
        plugin_path,
        config_json: serde_json::to_string(&config)?,
    })
}

fn generated_base_config() -> serde_json::Value {
    serde_json::json!({
        "$schema": "https://opencode.ai/config.json",
        "share": "disabled",
        "autoupdate": false,
        "mcp": {},
        "lsp": false,
        "permission": {
            "*": "deny",
            "aishe_*": "allow",
            "task": "allow",
            "todowrite": "allow",
            "todoread": "allow"
        },
        "compaction": {
            "auto": true,
            "prune": true
        }
    })
}

pub fn smoke_test(manager: &RuntimeManager) -> Result<()> {
    manager.verify()?;
    let prepared = prepare_layout()?;
    let port = reserve_port()?;
    let password = random_hex(32);
    let bridge_token = random_hex(32);
    let log_path = prepared.root.join("smoke.log");
    let log = private_log(&log_path)?;
    let mut child = spawn_opencode(
        manager,
        &prepared,
        port,
        &password,
        &bridge_token,
        "http://127.0.0.1:1",
        log.try_clone()?,
        log,
    )?;
    let url = format!("http://127.0.0.1:{port}");
    let result = wait_for_health(
        &mut child,
        &url,
        &password,
        manager.manifest().version.as_str(),
    );
    terminate_process_group(&mut child);
    result
}

#[allow(clippy::too_many_arguments)]
pub fn spawn_opencode(
    manager: &RuntimeManager,
    prepared: &PreparedRuntime,
    port: u16,
    password: &str,
    bridge_token: &str,
    bridge_url: &str,
    stdout: File,
    stderr: File,
) -> Result<Child> {
    let binary = manager.binary_path();
    let mut command = Command::new(&binary);
    command
        .args([
            "serve",
            "--hostname",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--mdns=false",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .env_clear()
        .env("HOME", &prepared.home)
        .env("XDG_CONFIG_HOME", prepared.root.join("xdg").join("config"))
        .env("XDG_DATA_HOME", &prepared.data_dir)
        .env("XDG_CACHE_HOME", &prepared.cache_dir)
        .env("XDG_STATE_HOME", &prepared.state_dir)
        .env("OPENCODE_CONFIG_DIR", &prepared.config_dir)
        .env("OPENCODE_CONFIG_CONTENT", &prepared.config_json)
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1")
        .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
        .env("OPENCODE_DISABLE_LSP_DOWNLOAD", "1")
        .env("OPENCODE_SERVER_USERNAME", "aishe")
        .env("OPENCODE_SERVER_PASSWORD", password)
        .env("OPENCODE_CLIENT", "aishe")
        .env("AISHE_BRIDGE_URL", bridge_url)
        .env("AISHE_BRIDGE_TOKEN", bridge_token)
        .env("NO_COLOR", "1");
    copy_safe_environment(&mut command);
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
        .spawn()
        .with_context(|| format!("starting managed OpenCode at {}", binary.display()))
}

fn copy_safe_environment(command: &mut Command) {
    for name in [
        "PATH",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "NO_PROXY",
        "https_proxy",
        "http_proxy",
        "no_proxy",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

fn wait_for_health(
    child: &mut Child,
    url: &str,
    password: &str,
    expected_version: &str,
) -> Result<()> {
    let started = Instant::now();
    let authorization = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("aishe:{password}"))
    );
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("OpenCode server exited before health check ({status})");
        }
        let response = ureq::get(&format!("{url}/global/health"))
            .set("Authorization", &authorization)
            .timeout(Duration::from_secs(1))
            .call();
        if let Ok(response) = response {
            let body: serde_json::Value = response.into_json()?;
            if body.get("healthy").and_then(|value| value.as_bool()) == Some(true)
                && body.get("version").and_then(|value| value.as_str()) == Some(expected_version)
            {
                return Ok(());
            }
            anyhow::bail!(
                "OpenCode health identity mismatch (expected version {expected_version})"
            );
        }
        if started.elapsed() >= Duration::from_secs(20) {
            anyhow::bail!("timed out waiting for authenticated OpenCode health");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn reserve_port() -> Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    Ok(listener.local_addr()?.port())
}

fn private_log(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}

pub fn request_stop() -> Result<u8> {
    let state_path = backend_root()?.join("supervisor.json");
    if !state_path.exists() {
        println!("agent backend is not running");
        return Ok(0);
    }
    let state: SupervisorState = serde_json::from_slice(&fs::read(&state_path)?)
        .context("backend supervisor state is invalid; run `aishe doctor --fix`")?;
    if state.schema_version != 1 || state.protocol_version != SUPERVISOR_PROTOCOL_VERSION {
        anyhow::bail!("backend supervisor protocol mismatch; run `aishe doctor --fix`");
    }
    // A control-channel shutdown replaces this signal path when the long-lived
    // bridge is enabled. Until then, verify the PID belongs to an Aishe child by
    // matching the private state and refuse broad/process-name killing.
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(state.supervisor_pid as i32, libc::SIGTERM) };
        if result == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error).context("stopping backend supervisor");
            }
        }
    }
    let _ = fs::remove_file(state_path);
    println!("agent backend stop requested");
    Ok(0)
}

pub fn print_logs(tail: usize) -> Result<()> {
    let path = backend_root()?.join("opencode").join("server.log");
    if !path.exists() {
        println!("no backend log at {}", path.display());
        return Ok(());
    }
    let mut file = File::open(&path)?;
    // Cap reads to the last 1 MiB even if an external failure defeated rotation.
    let length = file.metadata()?.len();
    if length > 1024 * 1024 {
        file.seek(SeekFrom::Start(length - 1024 * 1024))?;
    }
    let mut lines = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        lines.push(crate::redact::redact(&line));
        if lines.len() > tail.max(1) {
            lines.remove(0);
        }
    }
    for line in lines {
        println!("{}", crate::commands::display_safe(&line));
    }
    Ok(())
}

fn terminate_process_group(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(2) {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_config_denies_builtins_and_allows_only_bridge_control() {
        let config = generated_base_config();
        let permission = config.get("permission").unwrap();
        assert_eq!(permission.get("*").and_then(|v| v.as_str()), Some("deny"));
        assert_eq!(
            permission.get("aishe_*").and_then(|v| v.as_str()),
            Some("allow")
        );
        for builtin in ["bash", "read", "edit", "glob", "grep", "webfetch", "skill"] {
            assert_ne!(
                permission.get(builtin).and_then(|v| v.as_str()),
                Some("allow")
            );
        }
        assert_eq!(
            config.get("share").and_then(|v| v.as_str()),
            Some("disabled")
        );
    }

    #[test]
    fn embedded_plugin_matches_build_time_digest() {
        assert_eq!(sha256_bytes(PLUGIN), env!("AISHE_OPENCODE_PLUGIN_SHA256"));
    }
}
