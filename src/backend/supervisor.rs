//! Private OpenCode process preparation and lifecycle.
//!
//! The long-lived supervisor/control protocol is built on the same isolated
//! layout and launch contract used by `smoke_test`; keeping the smoke path real
//! prevents setup from reporting a binary as ready when its server/plugin
//! configuration cannot actually start.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine;
use fs2::FileExt;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::control::{ServerContext, SupervisorState};
use super::opencode::config::{ProviderLaunch, ProviderSpec, PROVIDER_KEY_ENV};
use super::RuntimeManager;

const PLUGIN: &[u8] = include_bytes!("../../assets/backend/opencode/aishe-plugin.mjs");
const MAX_BOOTSTRAP_BYTES: u64 = 256 * 1024;
const MAX_CONTROL_CONNECTIONS: usize = 64;
static SUPERVISOR_TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_supervisor_term(_signal: libc::c_int) {
    SUPERVISOR_TERMINATED.store(true, Ordering::SeqCst);
}

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

#[derive(Serialize, Deserialize)]
struct SupervisorBootstrap {
    schema_version: u32,
    provider: ProviderSpec,
    api_key: Option<String>,
    idle_timeout_secs: u64,
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

    let config = super::opencode::config::generated_config(&plugin_path, None)?;
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

/// Return a verified compatible supervisor, starting one without inheriting
/// provider credentials in its environment when necessary.
pub fn ensure_running(config: &crate::config::Config) -> Result<SupervisorState> {
    let manager = RuntimeManager::new()?;
    manager.verify()?;
    let launch = ProviderLaunch::from_aishe(config)?;
    if let Some(state) = super::control::verified_state()? {
        if state.runtime_version == manager.manifest().version
            && state.plugin_sha256 == env!("AISHE_OPENCODE_PLUGIN_SHA256")
            && state.provider_id == launch.spec.provider_id
            && state.model_id == launch.spec.model_id
        {
            return Ok(state);
        }
        let _ = super::control::request_stop();
        wait_for_state_removal(Duration::from_secs(5));
    } else if let Some(state) = super::control::load_state()? {
        if super::control::state_processes_exist(&state) {
            anyhow::bail!(
                "backend processes exist but failed authenticated health verification; \
                 inspect `aishe backend logs` and retry (the private supervisor exits at its idle timeout)"
            );
        }
        remove_stale_state()?;
    }

    let bootstrap = SupervisorBootstrap {
        schema_version: 1,
        provider: launch.spec,
        api_key: launch.api_key,
        idle_timeout_secs: config.backend.idle_timeout_secs.clamp(30, 86_400),
    };
    spawn_supervisor(&bootstrap)?;
    let started = Instant::now();
    loop {
        if let Some(state) = super::control::verified_state()? {
            if state.runtime_version != manager.manifest().version
                || state.plugin_sha256 != env!("AISHE_OPENCODE_PLUGIN_SHA256")
                || state.provider_id != bootstrap.provider.provider_id
                || state.model_id != bootstrap.provider.model_id
            {
                anyhow::bail!("started backend identity does not match requested provider/model");
            }
            return Ok(state);
        }
        if started.elapsed() >= Duration::from_secs(25) {
            anyhow::bail!("timed out starting the managed agent backend; run `aishe backend logs`");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Internal hidden-command entrypoint. Bootstrap material arrives through a
/// bounded pipe, never argv or the process environment.
pub fn run_supervisor() -> Result<u8> {
    SUPERVISOR_TERMINATED.store(false, Ordering::SeqCst);
    #[cfg(unix)]
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_supervisor_term as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGHUP,
            handle_supervisor_term as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_supervisor_term as *const () as libc::sighandler_t,
        );
    }

    let root = backend_root()?;
    fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let lock = private_lock(&root.join("supervisor.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(0),
        Err(error) => return Err(error).context("locking backend supervisor"),
    }

    let bootstrap = read_bootstrap()?;
    let manager = RuntimeManager::new()?;
    manager.verify()?;
    let mut prepared = prepare_layout()?;
    prepared.config_json = serde_json::to_string(&super::opencode::config::generated_config(
        &prepared.plugin_path,
        Some(&bootstrap.provider),
    )?)?;

    let control_listener =
        TcpListener::bind(("127.0.0.1", 0)).context("binding private backend control listener")?;
    control_listener.set_nonblocking(true)?;
    let control_port = control_listener.local_addr()?.port();
    let control_url = format!("http://127.0.0.1:{control_port}");
    let control_token = random_hex(32);
    let plugin_token = random_hex(32);
    let opencode_password = random_hex(32);
    let startup_nonce = random_hex(32);
    let log_path = prepared.root.join("server.log");
    rotate_log(&log_path, 4 * 1024 * 1024)?;
    let log = private_log(&log_path)?;

    let child = start_opencode_with_retries(
        &manager,
        &prepared,
        &opencode_password,
        &plugin_token,
        &control_url,
        bootstrap.api_key.as_deref(),
        &log,
    )?;
    let opencode_port = child
        .1
        .strip_prefix("http://127.0.0.1:")
        .and_then(|value| value.parse::<u16>().ok())
        .context("managed OpenCode returned an invalid listener")?;
    let mut opencode = child.0;
    let opencode_url = format!("http://127.0.0.1:{opencode_port}");
    let state = SupervisorState::new(
        std::process::id(),
        opencode.id(),
        control_url,
        opencode_url,
        manager.manifest().version.clone(),
        env!("AISHE_OPENCODE_PLUGIN_SHA256").into(),
        bootstrap.provider.provider_id,
        bootstrap.provider.model_id,
        startup_nonce,
        now_ms(),
        control_token.clone(),
        opencode_password,
    );
    super::control::write_state(&state)?;
    let shutdown = Arc::new(AtomicBool::new(false));
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let bridge = Arc::new(super::bridge::Bridge::open_default()?);
    let context = ServerContext {
        state: state.clone(),
        control_token,
        plugin_token,
        shutdown: Arc::clone(&shutdown),
        last_activity: Arc::clone(&last_activity),
        bridge,
    };
    let active_connections = Arc::new(AtomicUsize::new(0));
    let idle_timeout = Duration::from_secs(bootstrap.idle_timeout_secs);

    let result = (|| -> Result<()> {
        loop {
            if SUPERVISOR_TERMINATED.load(Ordering::SeqCst) || shutdown.load(Ordering::SeqCst) {
                break;
            }
            if let Some(status) = opencode.try_wait()? {
                anyhow::bail!("managed OpenCode exited unexpectedly ({status})");
            }
            let idle = last_activity
                .lock()
                .map(|value| value.elapsed())
                .unwrap_or(idle_timeout);
            if idle >= idle_timeout && active_connections.load(Ordering::SeqCst) == 0 {
                break;
            }
            match control_listener.accept() {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback()
                        || active_connections.load(Ordering::SeqCst) >= MAX_CONTROL_CONNECTIONS
                    {
                        drop(stream);
                        continue;
                    }
                    active_connections.fetch_add(1, Ordering::SeqCst);
                    let context = context.clone();
                    let active = Arc::clone(&active_connections);
                    std::thread::spawn(move || {
                        let _ = super::control::serve_connection(stream, &context);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => return Err(error).context("accepting backend control request"),
            }
        }
        Ok(())
    })();

    terminate_process_group(&mut opencode);
    let _ = super::control::remove_state_if_nonce(&state.startup_nonce);
    FileExt::unlock(&lock).ok();
    result?;
    Ok(0)
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
        None,
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
    provider_api_key: Option<&str>,
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
    if let Some(api_key) = provider_api_key {
        command.env(PROVIDER_KEY_ENV, api_key);
    }
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

fn start_opencode_with_retries(
    manager: &RuntimeManager,
    prepared: &PreparedRuntime,
    password: &str,
    plugin_token: &str,
    control_url: &str,
    provider_api_key: Option<&str>,
    log: &File,
) -> Result<(Child, String)> {
    let mut last_error = None;
    for _ in 0..3 {
        let port = reserve_port()?;
        let url = format!("http://127.0.0.1:{port}");
        let mut child = spawn_opencode(
            manager,
            prepared,
            port,
            password,
            plugin_token,
            control_url,
            provider_api_key,
            log.try_clone()?,
            log.try_clone()?,
        )?;
        match wait_for_health(
            &mut child,
            &url,
            password,
            manager.manifest().version.as_str(),
        ) {
            Ok(()) => return Ok((child, url)),
            Err(error) => {
                terminate_process_group(&mut child);
                last_error = Some(error);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("failed to start managed OpenCode")))
}

fn spawn_supervisor(bootstrap: &SupervisorBootstrap) -> Result<()> {
    let root = backend_root()?;
    fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let log_path = root.join("supervisor.log");
    rotate_log(&log_path, 4 * 1024 * 1024)?;
    let log = private_log(&log_path)?;
    let executable = std::env::current_exe().context("resolving the Aishe executable")?;
    let mut command = Command::new(executable);
    command
        .arg("__backend-supervisor")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .env_clear();
    copy_safe_environment(&mut command);
    for name in ["AISHE_DATA_DIR"] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            command.env(name, value);
        }
    }
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("starting backend supervisor")?;
    let bytes = serde_json::to_vec(bootstrap)?;
    if bytes.len() as u64 > MAX_BOOTSTRAP_BYTES {
        let _ = child.kill();
        anyhow::bail!("backend bootstrap exceeds the 256 KiB limit");
    }
    let result = child
        .stdin
        .take()
        .context("backend supervisor bootstrap pipe is unavailable")?
        .write_all(&bytes);
    if let Err(error) = result {
        let _ = child.kill();
        return Err(error).context("sending private backend bootstrap");
    }
    // Do not wait: the detached supervisor owns the managed runtime.
    Ok(())
}

fn read_bootstrap() -> Result<SupervisorBootstrap> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_BOOTSTRAP_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BOOTSTRAP_BYTES {
        anyhow::bail!("backend bootstrap exceeds the 256 KiB limit");
    }
    let bootstrap: SupervisorBootstrap =
        serde_json::from_slice(&bytes).context("backend bootstrap is invalid")?;
    if bootstrap.schema_version != 1 {
        anyhow::bail!("backend bootstrap schema mismatch");
    }
    if bootstrap.provider.requires_auth && bootstrap.api_key.is_none() {
        anyhow::bail!("backend bootstrap omitted the required provider credential");
    }
    if let Some(key) = bootstrap.api_key.as_deref() {
        crate::credentials::validate_secret(key)?;
    }
    Ok(bootstrap)
}

fn private_lock(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("opening private backend lock {}", path.display()))
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

fn rotate_log(path: &Path, limit: u64) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("backend log path {} is not a regular file", path.display());
    }
    if metadata.len() < limit {
        return Ok(());
    }
    let prior = path.with_extension("log.1");
    match fs::symlink_metadata(&prior) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            anyhow::bail!("backend prior log path {} is unsafe", prior.display())
        }
        Ok(_) => fs::remove_file(&prior)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::rename(path, prior)?;
    Ok(())
}

fn remove_stale_state() -> Result<()> {
    let path = super::control::state_path()?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn wait_for_state_removal(timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if super::control::load_state().ok().flatten().is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn request_stop() -> Result<u8> {
    if !super::control::request_stop()? {
        println!("agent backend is not running");
        return Ok(0);
    }
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

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_config_denies_builtins_and_allows_only_bridge_control() {
        let config = super::super::opencode::config::generated_config(
            Path::new("/private/aishe-plugin.mjs"),
            None,
        )
        .unwrap();
        let permission = config.get("permission").unwrap();
        assert_eq!(permission.get("*").and_then(|v| v.as_str()), Some("deny"));
        assert!(permission.get("aishe_*").is_none());
        assert_eq!(
            config["agent"]["aishe-auto"]["permission"]["aishe_*"],
            "allow"
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
