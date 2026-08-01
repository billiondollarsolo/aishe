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
use super::{RuntimeManager, RuntimeManifest};

const PLUGIN: &[u8] = include_bytes!("../../assets/backend/opencode/aishe-plugin.mjs");
const MAX_BOOTSTRAP_BYTES: u64 = 256 * 1024;
// A single connection runtime can legitimately receive one authenticated
// health/lease request from every concurrently starting shell. Keep the bound
// finite, but above the 100-shell release qualification so a valid local burst
// is queued instead of being mistaken for a broken supervisor.
const MAX_CONTROL_CONNECTIONS: usize = 256;
static SUPERVISOR_TERMINATED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_supervisor_term(_signal: libc::c_int) {
    SUPERVISOR_TERMINATED.store(true, Ordering::SeqCst);
}

#[derive(Clone, Debug)]
pub struct PreparedRuntime {
    pub root: PathBuf,
    pub home: PathBuf,
    pub config_dir: PathBuf,
    pub auth_config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub plugin_path: PathBuf,
    pub config_json: String,
    /// Exact private-layout paths created, replaced, or removed by this call.
    /// Used by Doctor so repeated `--fix` reports are genuinely idempotent.
    pub changed_paths: Vec<PathBuf>,
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
        .context("cannot resolve AIShe data directory")?
        .join("aishe")
        .join("backend"))
}

pub fn prepare_layout() -> Result<PreparedRuntime> {
    let backend_dir = backend_root()?;
    let root = backend_dir.join("opencode");
    let home = root.join("home");
    let config_dir = root.join("config");
    let auth_config_dir = root.join("auth-config");
    let xdg_dir = root.join("xdg");
    let xdg_config_dir = xdg_dir.join("config");
    let global_config_dir = xdg_config_dir.join("opencode");
    let data_dir = xdg_dir.join("data");
    let cache_dir = xdg_dir.join("cache");
    let state_dir = xdg_dir.join("state");
    let mut changed_paths = Vec::new();
    for directory in [
        &backend_dir,
        &root,
        &home,
        &config_dir,
        &auth_config_dir,
        &config_dir.join("plugins"),
        &xdg_dir,
        &xdg_config_dir,
        &global_config_dir,
        &data_dir,
        &cache_dir,
        &state_dir,
    ] {
        let missing = !directory.exists();
        fs::create_dir_all(directory)
            .with_context(|| format!("creating private backend path {}", directory.display()))?;
        crate::config::set_private_dir(directory);
        if missing {
            changed_paths.push(directory.to_path_buf());
        }
    }

    // OpenCode 1.18.9 starts a background SDK installation for every config
    // directory even when a local plugin has no imports. The trusted AIShe
    // bridge deliberately uses its pinned JSON-Schema compatibility path and
    // needs no SDK. Seed the exact lock shape inspected by that pinned loader,
    // plus an empty node_modules directory, so cold startup is deterministic
    // and never reaches a package registry. The real-runtime contract test
    // freezes this behavior and fails if the pin's loader contract changes.
    let runtime_version = RuntimeManifest::embedded()?.version;
    let npm_cache = home.join(".npm");
    if remove_disposable_path(&npm_cache)? {
        changed_paths.push(npm_cache);
    }
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &config_dir,
        &runtime_version,
    )?);
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &auth_config_dir,
        &runtime_version,
    )?);
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &global_config_dir,
        &runtime_version,
    )?);

    let plugin_path = config_dir.join("plugins").join("aishe-plugin.mjs");
    let expected = env!("AISHE_OPENCODE_PLUGIN_SHA256");
    let current_matches = fs::read(&plugin_path)
        .ok()
        .map(|bytes| sha256_bytes(&bytes) == expected)
        .unwrap_or(false);
    if !current_matches {
        crate::config::write_atomic(&plugin_path, PLUGIN)
            .with_context(|| format!("writing trusted plugin {}", plugin_path.display()))?;
        changed_paths.push(plugin_path.clone());
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
        auth_config_dir,
        data_dir,
        cache_dir,
        state_dir,
        plugin_path,
        config_json: serde_json::to_string(&config)?,
        changed_paths,
    })
}

/// Prepare a complete isolated OpenCode HOME/XDG/config tree for one labeled
/// OAuth account. Nothing in this tree is shared with another profile except
/// the verified runtime executable; even caches and state are profile-local.
pub fn prepare_profile_layout(provider: &str, profile: &str) -> Result<PreparedRuntime> {
    let provider = crate::config::normalize_connection_id(provider)?;
    let profile = crate::oauth::normalize_profile(profile)?;
    let base = prepare_layout()?;
    let root = base.root.join("profiles").join(provider).join(profile);
    let home = root.join("home");
    let config_dir = root.join("config");
    let auth_config_dir = root.join("auth-config");
    let xdg_dir = root.join("xdg");
    let xdg_config_dir = xdg_dir.join("config");
    let global_config_dir = xdg_config_dir.join("opencode");
    let data_dir = xdg_dir.join("data");
    let cache_dir = xdg_dir.join("cache");
    let state_dir = xdg_dir.join("state");
    let mut changed_paths = Vec::new();
    for directory in [
        &root,
        &home,
        &config_dir,
        &auth_config_dir,
        &config_dir.join("plugins"),
        &xdg_dir,
        &xdg_config_dir,
        &global_config_dir,
        &data_dir,
        &cache_dir,
        &state_dir,
    ] {
        let missing = !directory.exists();
        fs::create_dir_all(directory)
            .with_context(|| format!("creating OAuth profile path {}", directory.display()))?;
        crate::config::set_private_dir(directory);
        if missing {
            changed_paths.push(directory.to_path_buf());
        }
    }
    let runtime_version = RuntimeManifest::embedded()?.version;
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &config_dir,
        &runtime_version,
    )?);
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &auth_config_dir,
        &runtime_version,
    )?);
    changed_paths.extend(seed_dependency_free_plugin_loader(
        &global_config_dir,
        &runtime_version,
    )?);
    let plugin_path = config_dir.join("plugins").join("aishe-plugin.mjs");
    if fs::read(&plugin_path).ok().as_deref() != Some(PLUGIN) {
        crate::config::write_atomic(&plugin_path, PLUGIN)?;
        changed_paths.push(plugin_path.clone());
    }
    if sha256_bytes(&fs::read(&plugin_path)?) != env!("AISHE_OPENCODE_PLUGIN_SHA256") {
        anyhow::bail!("trusted OpenCode plugin checksum verification failed");
    }
    let config = super::opencode::config::generated_config(&plugin_path, None)?;
    Ok(PreparedRuntime {
        root,
        home,
        config_dir,
        auth_config_dir,
        data_dir,
        cache_dir,
        state_dir,
        plugin_path,
        config_json: serde_json::to_string(&config)?,
        changed_paths,
    })
}

fn seed_dependency_free_plugin_loader(
    directory: &Path,
    runtime_version: &str,
) -> Result<Vec<PathBuf>> {
    let mut changed = Vec::new();
    let node_modules = directory.join("node_modules");
    // v0.5.0 prerelease builds briefly let OpenCode populate this private,
    // disposable tree. Always retire it before creating the empty compatibility
    // directory so upgrades reclaim the duplicate SDK and transitive cache.
    let populated_or_unsafe = match fs::symlink_metadata(&node_modules) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::read_dir(&node_modules)
                .with_context(|| format!("reading {}", node_modules.display()))?
                .next()
                .is_some()
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(error).with_context(|| format!("inspecting {}", node_modules.display()))
        }
    };
    if populated_or_unsafe {
        remove_disposable_path(&node_modules)?;
        changed.push(node_modules.clone());
    }
    let missing_node_modules = !node_modules.exists();
    fs::create_dir_all(&node_modules).with_context(|| {
        format!(
            "creating offline OpenCode compatibility path {}",
            node_modules.display()
        )
    })?;
    crate::config::set_private_dir(&node_modules);
    if missing_node_modules && !populated_or_unsafe {
        changed.push(node_modules.clone());
    }

    let package = serde_json::json!({
        "name": "aishe-managed-opencode-config",
        "private": true
    });
    // This is a compatibility attestation for OpenCode's pinned Npm.install
    // guard, not a vendored or imported JavaScript package. The bridge itself
    // has no third-party runtime imports.
    let package_lock = serde_json::json!({
        "name": "aishe-managed-opencode-config",
        "lockfileVersion": 3,
        "requires": true,
        "packages": {
            "": {
                "name": "aishe-managed-opencode-config",
                "dependencies": {
                    "@opencode-ai/plugin": runtime_version
                }
            }
        }
    });
    for (path, value) in [
        (directory.join("package.json"), package),
        (directory.join("package-lock.json"), package_lock),
    ] {
        let mut bytes = serde_json::to_vec_pretty(&value)?;
        bytes.push(b'\n');
        if fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
            crate::config::write_atomic(&path, &bytes)
                .with_context(|| format!("writing {}", path.display()))?;
            changed.push(path);
        }
    }
    Ok(changed)
}

fn remove_disposable_path(path: &Path) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("inspecting disposable path {}", path.display()))
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
            .with_context(|| format!("removing disposable directory {}", path.display()))?;
    } else {
        fs::remove_file(path)
            .with_context(|| format!("removing disposable file {}", path.display()))?;
    }
    Ok(true)
}

/// Return a verified compatible supervisor, starting one without inheriting
/// provider credentials in its environment when necessary.
pub fn ensure_running(config: &crate::config::Config) -> Result<SupervisorState> {
    let manager = RuntimeManager::new()?;
    let launch = ProviderLaunch::from_aishe(config)?;
    let supervisor_key = launch.spec.launch_identity.clone();
    if let Some(state) = super::control::verified_state_for(&supervisor_key)? {
        if state.runtime_version == manager.manifest().version
            && state.plugin_sha256 == env!("AISHE_OPENCODE_PLUGIN_SHA256")
            && state.connection_id == launch.spec.connection_id
            && state.provider_id == launch.spec.provider_id
            && state.model_id == launch.spec.model_id
        {
            return Ok(state);
        }
    }

    // Elect one cold-start client before spawning a detached supervisor. The
    // supervisor's own lock is still authoritative, but it is acquired inside
    // the child; without this parent-side election, a burst of clients can
    // spawn lock-losing children that exit before reading their bootstrap pipe.
    // Healthy warm calls returned above and never touch this lock.
    let root = backend_root()?.join("instances").join(&supervisor_key);
    fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let startup_lock = private_lock(&root.join("startup.lock"))?;
    startup_lock
        .lock_exclusive()
        .context("locking managed backend startup")?;

    // A different client may have completed startup while this process waited.
    if let Some(state) = super::control::verified_state_for(&supervisor_key)? {
        if state.runtime_version == manager.manifest().version
            && state.plugin_sha256 == env!("AISHE_OPENCODE_PLUGIN_SHA256")
            && state.connection_id == launch.spec.connection_id
            && state.provider_id == launch.spec.provider_id
            && state.model_id == launch.spec.model_id
        {
            return Ok(state);
        }
        let _ = super::control::request_stop_for(&supervisor_key);
        wait_for_state_removal(&supervisor_key, Duration::from_secs(5));
    } else if let Some(state) = super::control::load_state_for(&supervisor_key)? {
        if super::control::state_processes_exist(&state) {
            anyhow::bail!(
                "backend processes exist but failed authenticated health verification; \
                 inspect `aishe backend logs` and retry (the private supervisor exits at its idle timeout)"
            );
        }
        remove_stale_state(&supervisor_key)?;
    }

    manager.verify_startup_attestation()?;
    let bootstrap = SupervisorBootstrap {
        schema_version: 1,
        provider: launch.spec,
        api_key: launch.api_key,
        idle_timeout_secs: config.backend.idle_timeout_secs.clamp(30, 86_400),
    };
    enforce_instance_limit(config.backend.max_instances, &supervisor_key)?;
    spawn_supervisor(&bootstrap)?;
    let started = Instant::now();
    loop {
        if let Some(state) = super::control::verified_state_for(&supervisor_key)? {
            if state.runtime_version != manager.manifest().version
                || state.plugin_sha256 != env!("AISHE_OPENCODE_PLUGIN_SHA256")
                || state.connection_id != bootstrap.provider.connection_id
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

    let bootstrap = read_bootstrap()?;
    let supervisor_key = bootstrap.provider.launch_identity.clone();
    let root = backend_root()?.join("instances").join(&supervisor_key);
    fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let lock = private_lock(&root.join("supervisor.lock"))?;
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(0),
        Err(error) => return Err(error).context("locking backend supervisor"),
    }

    let manager = RuntimeManager::new()?;
    manager.verify_startup_attestation()?;
    let mut prepared = if let (Some(provider), Some(profile)) = (
        bootstrap.provider.oauth_provider.as_deref(),
        bootstrap.provider.oauth_profile.as_deref(),
    ) {
        prepare_profile_layout(provider, profile)?
    } else {
        prepare_profile_layout("connections", &supervisor_key)?
    };
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
        bootstrap.provider.connection_id,
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
    let mut prepared = prepare_layout()?;
    let smoke_provider = ProviderSpec {
        connection_id: "smoke".into(),
        launch_identity: "smoke".into(),
        provider_id: "aishe-local".into(),
        model_id: "smoke-model".into(),
        npm: "@ai-sdk/openai-compatible".into(),
        base_url: "http://127.0.0.1:9/v1".into(),
        requires_auth: false,
        oauth_provider: None,
        oauth_profile: None,
        price: None,
        reasoning_effort: None,
    };
    prepared.config_json = serde_json::to_string(&super::opencode::config::generated_config(
        &prepared.plugin_path,
        Some(&smoke_provider),
    )?)?;
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
    )
    .and_then(|()| verify_smoke_tool_policy(&url, &password, &prepared.root));
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
        .current_dir(&prepared.root)
        .env_clear()
        .env("HOME", &prepared.home)
        .env("XDG_CONFIG_HOME", prepared.root.join("xdg").join("config"))
        .env("XDG_DATA_HOME", &prepared.data_dir)
        .env("XDG_CACHE_HOME", &prepared.cache_dir)
        .env("XDG_STATE_HOME", &prepared.state_dir)
        .env("OPENCODE_CONFIG_DIR", &prepared.config_dir)
        .env("OPENCODE_CONFIG_CONTENT", &prepared.config_json)
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "1")
        .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "1")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "1")
        .env("OPENCODE_DISABLE_MODELS_FETCH", "1")
        .env("OPENCODE_DISABLE_LSP_DOWNLOAD", "1")
        .env("OPENCODE_SERVER_USERNAME", "aishe")
        .env("OPENCODE_SERVER_PASSWORD", password)
        .env("OPENCODE_CLIENT", "aishe")
        .env("AISHE_BRIDGE_URL", bridge_url)
        .env("AISHE_BRIDGE_TOKEN", bridge_token)
        .env("NO_COLOR", "1");
    if !prepared_uses_oauth(&prepared.config_json) {
        command.env("OPENCODE_DISABLE_DEFAULT_PLUGINS", "1");
    }
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

/// Generated provider config is the only input here; never inspect OAuth token
/// contents merely to decide whether the pinned built-in hooks must load.
fn prepared_uses_oauth(config_json: &str) -> bool {
    let Ok(config) = serde_json::from_str::<serde_json::Value>(config_json) else {
        return false;
    };
    config
        .get("enabled_providers")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|enabled| {
            enabled
                .iter()
                .filter_map(serde_json::Value::as_str)
                .any(|provider| matches!(provider, "openai" | "xai"))
        })
}

fn verify_smoke_tool_policy(url: &str, password: &str, directory: &Path) -> Result<()> {
    let authorization = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("aishe:{password}"))
    );
    let mut ids_url = url::Url::parse(&format!("{url}/experimental/tool/ids"))?;
    ids_url
        .query_pairs_mut()
        .append_pair("directory", &directory.to_string_lossy());
    let required = [
        "aishe_run_command",
        "aishe_read_file",
        "aishe_write_file",
        "aishe_apply_patch",
    ];
    // `/global/health` can become ready a fraction before OpenCode finishes
    // loading plugins for the requested directory. Poll the authoritative tool
    // endpoint so setup does not fail intermittently on that startup boundary.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let last_error = match ureq::get(ids_url.as_str())
            .header("Authorization", &authorization)
            .config()
            .max_redirects(5)
            .timeout_global(Some(Duration::from_secs(1)))
            .build()
            .call()
        {
            Ok(mut response) => match response
                .body_mut()
                .with_config()
                .limit(MAX_BOOTSTRAP_BYTES)
                .read_json::<Vec<String>>()
            {
                Ok(ids) => {
                    let missing = required
                        .iter()
                        .filter(|required| !ids.iter().any(|id| id == **required))
                        .copied()
                        .collect::<Vec<_>>();
                    if missing.is_empty() {
                        return Ok(());
                    }
                    format!(
                        "trusted OpenCode plugin did not register {}",
                        missing.join(", ")
                    )
                }
                Err(error) => format!("decoding trusted plugin tool identities: {error}"),
            },
            Err(error) => format!("querying trusted plugin tool identities: {error}"),
        };
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("{last_error}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub(crate) fn copy_safe_environment(command: &mut Command) {
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
            .header("Authorization", &authorization)
            .config()
            .max_redirects(5)
            .timeout_global(Some(Duration::from_secs(1)))
            .build()
            .call();
        if let Ok(mut response) = response {
            let body: serde_json::Value = response
                .body_mut()
                .with_config()
                .limit(MAX_BOOTSTRAP_BYTES)
                .read_json()?;
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
    let root = backend_root()?
        .join("instances")
        .join(&bootstrap.provider.launch_identity);
    fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let log_path = root.join("supervisor.log");
    rotate_log(&log_path, 4 * 1024 * 1024)?;
    let log = private_log(&log_path)?;
    let executable = std::env::current_exe().context("resolving the AIShe executable")?;
    let mut command = Command::new(executable);
    command
        .arg("__backend-supervisor")
        .stdin(Stdio::piped())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .env_clear()
        .env("AISHE_SUPERVISOR_KEY", &bootstrap.provider.launch_identity);
    copy_safe_environment(&mut command);
    // Preserve only AIShe-owned path overrides needed to find the same private
    // state/runtime after the detached process clears its environment. This is
    // also what makes centrally pre-provisioned runtime directories usable in
    // CI and managed enterprise images.
    for name in ["AISHE_DATA_DIR", "AISHE_RUNTIME_DIR"] {
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
    if let Some(oauth) = bootstrap.provider.oauth_provider.as_deref() {
        if !matches!(oauth, "openai" | "xai") || bootstrap.provider.provider_id != oauth {
            anyhow::bail!("backend bootstrap contains an invalid OAuth provider binding");
        }
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

pub fn instance_keys() -> Result<Vec<String>> {
    let root = backend_root()?.join("instances");
    let mut keys = Vec::new();
    let entries = match fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(keys),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let key = entry.file_name().to_string_lossy().into_owned();
        if super::control::state_path_for(&key).is_ok() {
            keys.push(key);
        }
    }
    keys.sort();
    Ok(keys)
}

fn enforce_instance_limit(limit: usize, requested: &str) -> Result<()> {
    let limit = limit.clamp(1, 32);
    let mut running = 0usize;
    let mut idle_candidates = Vec::new();
    for key in instance_keys()? {
        if key == requested {
            continue;
        }
        if let Some(activity) = super::control::verified_activity_for(&key)? {
            running += 1;
            if activity.active_leases == 0 {
                idle_candidates.push((key, activity.idle_ms));
            }
        }
    }
    let needed = running.saturating_add(1).saturating_sub(limit);
    let evictions = select_evictions(idle_candidates, needed);
    if evictions.len() < needed {
        anyhow::bail!(
            "backend instance limit ({limit}) is occupied by active turns; retry after one completes or increase backend.max_instances"
        );
    }
    for key in evictions {
        let _ = super::control::request_stop_for(&key);
        wait_for_state_removal(&key, Duration::from_secs(5));
    }
    Ok(())
}

fn select_evictions(mut states: Vec<(String, u64)>, count: usize) -> Vec<String> {
    states.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    states.into_iter().take(count).map(|(key, _)| key).collect()
}

fn remove_stale_state(key: &str) -> Result<()> {
    let path = super::control::state_path_for(key)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn wait_for_state_removal(key: &str, timeout: Duration) {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if super::control::load_state_for(key).ok().flatten().is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn request_stop() -> Result<u8> {
    let mut stopped = 0usize;
    for key in instance_keys()? {
        if super::control::request_stop_for(&key)? {
            stopped += 1;
        }
    }
    if super::control::request_stop()? {
        stopped += 1;
    }
    if stopped == 0 {
        println!("agent backend is not running");
        return Ok(0);
    }
    println!("agent backend stop requested for {stopped} instance(s)");
    Ok(0)
}

pub fn print_logs(tail: usize) -> Result<()> {
    let root = backend_root()?;
    let mut paths = Vec::new();
    for key in instance_keys()? {
        let path = root.join("instances").join(key).join("supervisor.log");
        if path.is_file() {
            paths.push(path);
        }
    }
    collect_profile_logs(&root.join("opencode").join("profiles"), 0, &mut paths)?;
    let legacy = root.join("opencode").join("server.log");
    if legacy.is_file() {
        paths.push(legacy);
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        println!("no backend logs");
        return Ok(());
    }
    for path in paths {
        println!("== {} ==", path.display());
        print_log_tail(&path, tail)?;
    }
    Ok(())
}

fn collect_profile_logs(root: &Path, depth: usize, paths: &mut Vec<PathBuf>) -> Result<()> {
    if depth > 4 {
        return Ok(());
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            collect_profile_logs(&path, depth + 1, paths)?;
        } else if file_type.is_file() && entry.file_name() == "server.log" {
            paths.push(path);
        }
    }
    Ok(())
}

fn print_log_tail(path: &Path, tail: usize) -> Result<()> {
    let mut file = File::open(path)?;
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
    fn supervisor_bound_evicts_oldest_with_a_stable_tie_break() {
        let states = vec![
            ("c-new".into(), 10),
            ("c-old-b".into(), 30),
            ("c-old-a".into(), 30),
            ("c-middle".into(), 20),
        ];
        assert_eq!(
            select_evictions(states.clone(), 2),
            vec!["c-old-a".to_string(), "c-old-b".to_string()]
        );
        assert!(select_evictions(states, 0).is_empty());
    }

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
        let source = std::str::from_utf8(PLUGIN).unwrap();
        assert!(!source.contains("@opencode-ai/plugin"));
        assert!(!source.contains(" from "));
    }

    #[test]
    fn dependency_free_loader_seed_is_exact_and_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "aishe-opencode-loader-seed-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("node_modules/@opencode-ai/plugin")).unwrap();
        fs::write(
            root.join("node_modules/@opencode-ai/plugin/package.json"),
            b"legacy disposable SDK",
        )
        .unwrap();
        let first_changes = seed_dependency_free_plugin_loader(&root, "1.18.9").unwrap();
        assert!(!first_changes.is_empty());
        let first = fs::read(root.join("package-lock.json")).unwrap();
        let second_changes = seed_dependency_free_plugin_loader(&root, "1.18.9").unwrap();
        assert!(
            second_changes.is_empty(),
            "idempotent layout preparation reported changes: {second_changes:?}"
        );
        assert_eq!(fs::read(root.join("package-lock.json")).unwrap(), first);
        assert!(root.join("node_modules").is_dir());
        assert!(!root
            .join("node_modules")
            .join("@opencode-ai")
            .join("plugin")
            .exists());
        let lock: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(
            lock["packages"][""]["dependencies"]["@opencode-ai/plugin"],
            "1.18.9"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
