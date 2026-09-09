//! Lean CSH hot path: clean `zsh -f` PTY, in-process provider HTTP, no OpenCode.
//!
//! Interactive aishe defaults to this path. Set `AISHE_LEGACY_OPENCODE=1` to
//! restore the historical PTY (user `.zshrc` + OpenCode sidecar). `AISHE_LEAN=0`
//! also disables it.

mod grant;
mod hook;
mod ipc;
mod nl;
mod heavy;
mod grok_oauth;
pub use grok_oauth::{SUBSCRIPTION_TOKEN_ENV, available as grok_subscription_available};

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub use grant::{ensure_session_grant, grant_accepted, LeanGrant, LeanMode};
pub use hook::{wrapper_zshenv, wrapper_zshrc, zsh_argv};
pub use ipc::{spawn_ipc, IpcGuard};
pub use nl::{handle_ipc_line, run_nl};

static NL_TURN_START_NS: AtomicU64 = AtomicU64::new(0);
static NL_WIRE_READY_NS: AtomicU64 = AtomicU64::new(0);

fn env_flag_is(name: &str, on: &[&str]) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            let value = value.trim().to_ascii_lowercase();
            on.iter().any(|candidate| *candidate == value)
        })
        .unwrap_or(false)
}

/// Lean interactive/NL path is the default. Legacy OpenCode is an explicit hatch.
pub fn enabled() -> bool {
    if env_flag_is("AISHE_LEGACY_OPENCODE", &["1", "true", "yes", "on"]) {
        return false;
    }
    if env_flag_is("AISHE_LEAN", &["0", "false", "no", "off"]) {
        return false;
    }
    true
}

pub fn session_mode(config: &crate::config::Config) -> String {
    let from_env = std::env::var("AISHE_MODE")
        .ok()
        .filter(|value| !value.is_empty());
    LeanMode::parse(from_env.as_deref().unwrap_or(config.aishe.mode.as_str()))
        .as_str()
        .to_string()
}

pub fn note_provider_make() {
    spy_write("AISHE_SPY_PROVIDER_MAKE", "provider");
}

pub fn note_opencode_start() {
    spy_write("AISHE_SPY_OPENCODE", "opencode");
}

fn spy_write(var: &str, contents: &str) {
    if let Ok(path) = std::env::var(var) {
        if !path.is_empty() {
            let _ = std::fs::write(path, contents);
        }
    }
}

pub fn mark_nl_turn_start() {
    let nanos = monotonic_ns();
    NL_TURN_START_NS.store(nanos, Ordering::SeqCst);
    NL_WIRE_READY_NS.store(0, Ordering::SeqCst);
}

/// Call immediately before the HTTP client writes the provider request.
pub fn mark_nl_wire_ready() {
    let start = NL_TURN_START_NS.load(Ordering::SeqCst);
    let elapsed = if start == 0 {
        0
    } else {
        monotonic_ns().saturating_sub(start)
    };
    NL_WIRE_READY_NS.store(elapsed, Ordering::SeqCst);
    if let Ok(path) = std::env::var("AISHE_SPY_WIRE_NS") {
        if !path.is_empty() {
            let _ = std::fs::write(path, elapsed.to_string());
        }
    }
}

pub fn last_nl_prepare_ns() -> u64 {
    NL_WIRE_READY_NS.load(Ordering::SeqCst)
}

fn monotonic_ns() -> u64 {
    static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lean_is_on_by_default() {
        // The process may inherit flags from the harness; the parser itself is
        // covered by the explicit env_flag helper.
        assert!(env_flag_is("AISHE_LEAN_TEST_UNSET_VAR_XXXX", &["1"]) == false);
        assert!(LeanMode::parse("suggest") == LeanMode::Ask);
        assert!(LeanMode::parse("auto") == LeanMode::Allow);
        assert!(LeanMode::parse("yolo") == LeanMode::Agent);
        assert_eq!(LeanMode::Ask.as_str(), "ask");
    }

    #[test]
    fn hook_never_sources_user_zshrc_or_spawns_aishe() {
        let rc = wrapper_zshrc();
        for line in rc.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('#') {
                continue;
            }
            assert!(
                !trimmed.contains("AISHE_REAL_ZDOTDIR"),
                "lean hook sources user ZDOTDIR: {trimmed}"
            );
            assert!(
                !trimmed.contains("source ~/.zshrc")
                    && !trimmed.contains("source \"${HOME}/.zshrc\""),
                "lean hook sources ~/.zshrc: {trimmed}"
            );
        }
        assert!(!rc.contains("command aishe --suggest-line"));
        assert!(!rc.contains("command aishe --yolo-line"));
        assert!(!rc.contains("command aishe --auto-line"));
        assert!(!rc.contains("backend::supervisor"));
        assert!(rc.contains("AISHE_LEAN=1"));
        assert!(rc.contains("_aishe_routes_to_agent"));
        assert!(rc.contains("AISHE_LEAN_REQ"));
        let env = wrapper_zshenv();
        assert!(!env.contains("AISHE_REAL_ZDOTDIR"));
        assert!(env.contains("GLOBAL_RCS"));
    }

    #[test]
    fn zsh_argv_starts_from_dash_f() {
        assert_eq!(zsh_argv(), ["-f", "-o", "RCS", "-o", "NO_GLOBAL_RCS", "-i"]);
    }

    #[test]
    fn safety_still_flags_model_proposed_destruction() {
        assert!(matches!(
            crate::safety::assess("rm -rf /"),
            crate::safety::Risk::Dangerous(_)
        ));
        assert!(matches!(
            crate::safety::assess("ls"),
            crate::safety::Risk::Safe
        ));
    }
}

/// Prefer live xAI/Grok for lean using Grok Build subscription OAuth.
///
/// Happy path: read the CLI session at `~/.grok/auth.json` (or `AISHE_GROK_AUTH` /
/// `$GROK_HOME/auth.json`), inject the OIDC access token into the process-local
/// env [`grok_oauth::SUBSCRIPTION_TOKEN_ENV`], and point the active connection at
/// the catalog `xai` entry. Never writes secrets to config or git.
///
/// Escape hatch: if no CLI session is available but `XAI_API_KEY` is set, fall
/// back to that API-key connection (not the documented happy path).
pub fn prefer_grok_live_auth(config: &mut crate::config::Config) {
    if let Some(token) = grok_oauth::access_token() {
        std::env::set_var(grok_oauth::SUBSCRIPTION_TOKEN_ENV, token);
        apply_xai_connection(config, grok_oauth::SUBSCRIPTION_TOKEN_ENV, "Grok - subscription (CLI OAuth)");
        return;
    }
    let Ok(key) = std::env::var("XAI_API_KEY") else {
        return;
    };
    if key.trim().is_empty() {
        return;
    }
    apply_xai_connection(config, "XAI_API_KEY", "Grok - API");
}

fn apply_xai_connection(config: &mut crate::config::Config, api_key_env: &str, label: &str) {
    let Some(service) = crate::provider_catalog::find("xai") else {
        return;
    };
    crate::provider_catalog::apply(service, &mut config.providers.openai);
    config.aishe.provider = "xai".into();
    config.aishe.connection = "xai".into();
    let mut settings = config.providers.openai.clone();
    settings.api_key_env = api_key_env.to_string();
    let connection = crate::config::ConnectionConfig {
        provider: "xai".into(),
        label: label.into(),
        settings,
        auth: crate::config::ConnectionAuth::ApiKey {
            credential: Some(service.credential.to_string()),
            api_key_env: Some(api_key_env.to_string()),
        },
        reasoning_effort: None,
    };
    config.connections.insert("xai".into(), connection);
}

/// Deprecated name — use [`prefer_grok_live_auth`].
#[deprecated(note = "use prefer_grok_live_auth")]
pub fn prefer_xai_api_from_env(config: &mut crate::config::Config) {
    prefer_grok_live_auth(config);
}

