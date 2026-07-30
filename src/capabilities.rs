//! Provider/model discovery, live validation, and persisted compatibility
//! evidence. Probes intentionally use the same provider implementation as
//! runtime requests so Setup cannot certify a path the shell will not use.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::providers::{self, ErrorKind, Msg, ProviderError, Reach, ResponseFormat, ToolDef};

pub const CACHE_SCHEMA_VERSION: u32 = 1;
pub const CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Pass,
    Warn,
    Fail,
    Skipped,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Check {
    pub state: State,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
}

impl Check {
    fn pass(detail: impl Into<String>) -> Self {
        Self {
            state: State::Pass,
            detail: detail.into(),
            error_kind: None,
        }
    }

    fn warn(detail: impl Into<String>) -> Self {
        Self {
            state: State::Warn,
            detail: detail.into(),
            error_kind: None,
        }
    }

    fn fail(detail: impl Into<String>, error_kind: Option<ErrorKind>) -> Self {
        Self {
            state: State::Fail,
            detail: detail.into(),
            error_kind,
        }
    }

    fn skipped(detail: impl Into<String>) -> Self {
        Self {
            state: State::Skipped,
            detail: detail.into(),
            error_kind: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub checked_at_ms: u128,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub transport: String,
    pub credential_env: String,
    #[serde(default)]
    pub credential_profile: String,
    #[serde(default)]
    pub credential_source: String,
    pub credential_required: bool,
    pub credential: Check,
    pub reachability: Check,
    pub model_list: Check,
    pub model_available: Check,
    pub text: Check,
    pub structured: Check,
    pub tools: Check,
    pub streaming: Check,
}

impl Report {
    pub fn verified(&self) -> bool {
        [
            &self.credential,
            &self.reachability,
            &self.model_available,
            &self.text,
            &self.structured,
            &self.tools,
            &self.streaming,
        ]
        .iter()
        .all(|check| check.state == State::Pass)
    }

    pub fn live_verified(&self) -> bool {
        [&self.text, &self.structured, &self.tools, &self.streaming]
            .iter()
            .all(|check| check.state == State::Pass)
    }
}

pub fn list_models(config: &Config, provider_name: &str) -> Result<Vec<String>, ProviderError> {
    let (provider, anthropic) = match provider_name {
        "openai" => {
            let provider = &config.providers.openai;
            (provider, false)
        }
        "anthropic" => {
            let provider = &config.providers.anthropic;
            (provider, true)
        }
        other => {
            return Err(ProviderError::Api {
                status: 0,
                message: format!("unknown provider '{other}'"),
            })
        }
    };
    let resolved = crate::credentials::resolve(provider).map_err(|error| ProviderError::Api {
        status: 0,
        message: crate::redact::redact(&error.to_string()),
    })?;
    let key = resolved.into_secret();
    if provider.requires_auth() && key.is_none() {
        let profile = provider.credential_profile();
        return Err(ProviderError::Api {
            status: 0,
            message: format!(
                "API key missing for credential profile '{profile}' — run \
                 `aishe auth set {profile}` or set ${}",
                provider.api_key_env
            ),
        });
    }

    let base = crate::provider_catalog::normalize_base_url(&provider.base_url);
    let endpoint = format!("{base}/v1/models");
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(10))
        .build();
    let mut request = agent.get(&endpoint);
    if let Some(key) = key.as_deref() {
        request = if anthropic {
            request
                .set("x-api-key", key)
                .set("anthropic-version", "2023-06-01")
        } else {
            request.set("Authorization", &format!("Bearer {key}"))
        };
    }
    let response = request.call().map_err(map_ureq_error)?;
    let value: Value = response
        .into_json()
        .map_err(|error| ProviderError::Parse(error.to_string()))?;
    let mut models = model_ids(&value);
    models.sort();
    models.dedup();
    if models.is_empty() {
        return Err(ProviderError::Parse(
            "models response contained no data[].id entries".into(),
        ));
    }
    Ok(models)
}

/// Make one minimal text request with the active model. Setup uses this only
/// when a manually entered model is not present in (or cannot be checked
/// against) `/v1/models`. Catalog-backed selections remain free; this fallback
/// proves that the exact model ID is accepted by the configured runtime path.
pub fn validate_model_request(config: &Config) -> Result<(), ProviderError> {
    let mut isolated = config.clone();
    isolated.aishe.provider_fallback.clear();
    isolated.aishe.cache = false;
    let provider = providers::make(&isolated).map_err(|error| ProviderError::Api {
        status: 0,
        message: crate::redact::redact(&error.to_string()),
    })?;
    provider
        .complete(
            "Reply with only setup-ok.",
            &[Msg::User("Validate this model.".into())],
            &ResponseFormat::Text,
        )
        .map(|_| ())
}

fn model_ids(value: &Value) -> Vec<String> {
    value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
        .map(ToOwned::to_owned)
        .collect()
}

pub fn validate(config: &Config, live: bool) -> Report {
    let provider_name = config.aishe.provider.clone();
    let (provider, transport) = if provider_name == "openai" {
        (
            &config.providers.openai,
            config.providers.openai.transport.clone(),
        )
    } else {
        (&config.providers.anthropic, "messages".to_string())
    };
    let profile = provider.credential_profile();
    let resolution = crate::credentials::resolve(provider);
    let credential_source = resolution
        .as_ref()
        .map(|resolved| resolved.source.label())
        .unwrap_or_else(|_| "error".to_string());
    let credential = match &resolution {
        Ok(resolved) if resolved.secret().is_some() => Check::pass(format!(
            "credential available from {}",
            resolved.source.label()
        )),
        Ok(_) if provider.requires_auth() => Check::fail(
            format!(
                "credential profile '{profile}' is missing; run `aishe auth set {profile}` \
                 or set ${}",
                provider.api_key_env
            ),
            Some(ErrorKind::MissingCredential),
        ),
        Ok(_) => Check::pass("credential not required for this endpoint"),
        Err(error) => Check::fail(
            format!(
                "credential store unavailable: {}",
                crate::redact::redact(&error.to_string())
            ),
            Some(ErrorKind::MissingCredential),
        ),
    };

    // A required credential that is already known to be absent is definitive.
    // Do not make anonymous network requests that can only add a confusing 401
    // beside the actionable missing-variable diagnosis.
    let credential_missing = credential.state == State::Fail;
    let (reachability, models, model_list) = if credential_missing {
        let detail = format!("blocked because credential profile '{profile}' is unavailable");
        (Check::skipped(detail.clone()), None, Check::skipped(detail))
    } else {
        let probe = providers::probe(config, &provider_name);
        let reachability = match probe.reach {
            Reach::Up(status) => Check::pass(format!("endpoint answered HTTP {status}")),
            Reach::Unauthorized(status) => Check::fail(
                format!("endpoint rejected credentials (HTTP {status})"),
                Some(ErrorKind::InvalidCredential),
            ),
            Reach::Down(error) => Check::fail(error, Some(ErrorKind::Network)),
        };
        let (models, model_list) = match list_models(config, &provider_name) {
            Ok(models) => {
                let count = models.len();
                (
                    Some(models),
                    Check::pass(format!("{count} model(s) returned")),
                )
            }
            Err(error) => (
                None,
                Check::warn(format!(
                    "model listing unavailable ({:?}: {})",
                    error.kind(),
                    crate::redact::redact(&error.to_string())
                )),
            ),
        };
        (reachability, models, model_list)
    };
    let mut model_available = match &models {
        Some(models) if models.iter().any(|model| model == &provider.model) => {
            Check::pass("configured model appears in endpoint list")
        }
        Some(_) => Check::warn("configured model was not present; manual models are still allowed"),
        None if credential_missing => {
            Check::skipped("blocked because the required credential is missing")
        }
        None => Check::warn("not verified because model listing was unavailable"),
    };

    let skipped = || Check::skipped("run with --live to make a minimal generation request");
    let (text, structured, tools, streaming) = if !live {
        (skipped(), skipped(), skipped(), skipped())
    } else if credential.state == State::Fail || reachability.state == State::Fail {
        let blocked = || Check::skipped("blocked by credential or reachability failure");
        (blocked(), blocked(), blocked(), blocked())
    } else {
        run_live_checks(config)
    };
    if text.state == State::Pass {
        model_available = Check::pass("configured model accepted a generation request");
    }

    let report = Report {
        schema_version: CACHE_SCHEMA_VERSION,
        checked_at_ms: now_ms(),
        provider: provider_name,
        endpoint: provider.base_url.clone(),
        model: provider.model.clone(),
        transport,
        credential_env: provider.api_key_env.clone(),
        credential_profile: profile,
        credential_source,
        credential_required: provider.requires_auth(),
        credential,
        reachability,
        model_list,
        model_available,
        text,
        structured,
        tools,
        streaming,
    };
    let _ = save(&report);
    report
}

fn run_live_checks(config: &Config) -> (Check, Check, Check, Check) {
    let mut isolated = config.clone();
    isolated.aishe.provider_fallback.clear();
    isolated.aishe.cache = false;
    if isolated.backend.engine == "opencode" {
        return match run_managed_live_checks(&isolated) {
            Ok(checks) => checks,
            Err(error) => {
                let detail =
                    crate::redact::redact(&format!("managed OpenCode validation failed: {error}"));
                let failed = || Check::fail(detail.clone(), Some(ErrorKind::Unknown));
                (failed(), failed(), failed(), failed())
            }
        };
    }
    run_native_live_checks(&isolated)
}

fn run_native_live_checks(config: &Config) -> (Check, Check, Check, Check) {
    let provider = match providers::make(config) {
        Ok(provider) => provider,
        Err(error) => {
            let detail = crate::redact::redact(&error.to_string());
            let failed = || Check::fail(detail.clone(), Some(ErrorKind::MissingCredential));
            return (failed(), failed(), failed(), failed());
        }
    };
    let messages = [Msg::User("Reply with setup-ok.".into())];
    let text = result_check(provider.complete("Be concise.", &messages, &ResponseFormat::Text));
    let schema = json!({
        "type": "object",
        "properties": {"status": {"type": "string"}},
        "required": ["status"],
        "additionalProperties": false
    });
    let structured = result_check(provider.complete(
        "Return status setup-ok.",
        &messages,
        &ResponseFormat::JsonSchema {
            name: "aishe_setup_check".into(),
            schema,
        },
    ));
    let tools = result_check(provider.complete_with_tools(
        "Do not call tools; reply setup-ok. The request tests tool compatibility.",
        &messages,
        &[ToolDef {
            name: "aishe_setup_noop".into(),
            description: "A no-op used only to validate function-tool compatibility.".into(),
            schema: json!({
                "type": "object",
                "properties": {},
                "required": [],
                "additionalProperties": false
            }),
        }],
    ));
    let mut received = String::new();
    let streaming = result_check(provider.complete_stream(
        "Be concise.",
        &messages,
        &ResponseFormat::Text,
        &mut |delta| received.push_str(delta),
    ));
    (text, structured, tools, streaming)
}

fn run_managed_live_checks(config: &Config) -> Result<(Check, Check, Check, Check)> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use crate::agent::{
        AgentEvent, BackendSession, ExecutionScope, Mode, NetworkPolicy, PromptRequest, ToolWorker,
    };
    use crate::backend::bridge::LeaseRegistration;
    use crate::backend::control::SupervisorClient;
    use crate::backend::opencode::OpenCodeClient;

    struct ValidationGuard(PathBuf);
    impl Drop for ValidationGuard {
        fn drop(&mut self) {
            let _ = crate::backend::control::request_stop();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    let workspace = std::env::temp_dir().join(format!(
        "aishe-managed-validation-{}-{:032x}",
        std::process::id(),
        rand::random::<u128>()
    ));
    std::fs::create_dir_all(&workspace)?;
    crate::config::set_private_dir(&workspace);
    let _guard = ValidationGuard(workspace.clone());

    let state = crate::backend::supervisor::ensure_running(config)?;
    let control = SupervisorClient::new(state)?;
    let client = OpenCodeClient::new(
        control.opencode_connection(),
        control.provider_id(),
        control.model_id(),
    )?;
    client.health().context("managed OpenCode health check")?;
    let scope = ExecutionScope::parse(&config.backend.default_scope)
        .context("backend.default_scope must be workspace or host")?;
    let network = if scope == ExecutionScope::Host {
        NetworkPolicy::Allow
    } else {
        NetworkPolicy::parse(&config.backend.workspace_network)
            .context("backend.workspace_network must be allow or deny")?
    };
    let session = client.create_session(&workspace, "Aishe setup validation", scope, network)?;
    let price = crate::usage::budget_price_for(config.active_model(), &config.pricing);
    let shell_id = format!("setup_{:032x}", rand::random::<u128>());
    let resolved = crate::credentials::resolve(active_provider(config))?;
    let secret = resolved.into_secret();

    let run = |mode: Mode, prompt: &str| -> Result<Vec<AgentEvent>> {
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker = ToolWorker::start_silent(
            control.clone(),
            LeaseRegistration {
                aishe_shell_id: shell_id.clone(),
                backend_session_id: session.id.clone(),
                workspace: workspace.clone(),
                mode,
                scope,
                network,
                interactive: false,
                budget_usd: None,
                price,
                baseline_spent_usd: 0.0,
            },
            config.clone(),
            cancelled,
        )?;
        let (handle, stream) = client.submit(&PromptRequest {
            session: BackendSession {
                id: session.id.clone(),
                workspace: workspace.clone(),
                backend: "opencode".into(),
            },
            text: prompt.into(),
            mode,
            max_output_tokens: Some(config.backend.max_output_tokens.clamp(32, 512)),
        })?;
        let events = client.read_events(&handle, stream);
        worker.stop();
        events
    };

    let suggest_events = run(
        Mode::Suggest,
        "This is setup validation. Answer with the single word setup-ok; do not propose a command.",
    )
    .context("managed text/structured/streaming turn")?;
    ensure_managed_turn_completed(&suggest_events)?;
    let answer = completed_text(&suggest_events);
    let usage_present = suggest_events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::Usage { usage }
                if usage.input_tokens > 0 || usage.output_tokens > 0
        )
    });
    let text = if answer.trim().is_empty() || !usage_present {
        Check::fail(
            "managed turn omitted terminal text or authoritative usage",
            Some(ErrorKind::MalformedResponse),
        )
    } else {
        Check::pass("managed OpenCode text and authoritative usage accepted")
    };
    let structured = match crate::modes::extract_json(&answer)
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
    {
        Some(value)
            if matches!(
                value.get("type").and_then(Value::as_str),
                Some("answer" | "command")
            ) && value.get("explanation").and_then(Value::as_str).is_some() =>
        {
            Check::pass("managed suggest contract returned valid structured output")
        }
        _ => Check::fail(
            "managed suggest response did not satisfy Aishe's structured contract",
            Some(ErrorKind::UnsupportedFormat),
        ),
    };
    let streaming = if suggest_events
        .iter()
        .any(|event| matches!(event, AgentEvent::TextDelta { text } if !text.is_empty()))
    {
        Check::pass("managed OpenCode delivered incremental text events")
    } else {
        Check::fail(
            "managed OpenCode completed without an incremental text event",
            Some(ErrorKind::MalformedResponse),
        )
    };

    let tool_events = run(
        Mode::Auto,
        "Use aishe_run_command exactly once to run `env`. Then reply setup-ok. Do not use any other tool.",
    )
    .context("managed foreground proxy-tool turn")?;
    ensure_managed_turn_completed(&tool_events)?;
    let completed_tools = tool_events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolCompleted { result, .. } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    let leaked = secret.as_deref().is_some_and(|secret| {
        !secret.is_empty()
            && completed_tools
                .iter()
                .any(|result| result.output.contains(secret))
    });
    let tools = if leaked {
        Check::fail(
            "managed foreground tool environment exposed the provider credential",
            Some(ErrorKind::Permission),
        )
    } else if completed_tools.is_empty() {
        Check::fail(
            "managed agent did not complete the requested Aishe proxy-tool round trip",
            Some(ErrorKind::UnsupportedTools),
        )
    } else {
        Check::pass("managed Aishe proxy tool completed with credential isolation")
    };

    Ok((text, structured, tools, streaming))
}

fn active_provider(config: &Config) -> &crate::config::ProviderConfig {
    if config.aishe.provider == "openai" {
        &config.providers.openai
    } else {
        &config.providers.anthropic
    }
}

fn completed_text(events: &[crate::agent::AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            crate::agent::AgentEvent::TextCompleted { text } if !text.is_empty() => {
                Some(text.as_str())
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn ensure_managed_turn_completed(events: &[crate::agent::AgentEvent]) -> Result<()> {
    if let Some(error) = events.iter().find_map(|event| match event {
        crate::agent::AgentEvent::Failed { error } => Some(error),
        _ => None,
    }) {
        anyhow::bail!("{} ({})", error.message, error.code);
    }
    if events
        .iter()
        .any(|event| matches!(event, crate::agent::AgentEvent::Aborted))
    {
        anyhow::bail!("managed validation turn was aborted");
    }
    if !events
        .iter()
        .any(|event| matches!(event, crate::agent::AgentEvent::Completed { .. }))
    {
        anyhow::bail!("managed validation turn did not reach completion");
    }
    Ok(())
}

fn result_check<T>(result: Result<T, ProviderError>) -> Check {
    match result {
        Ok(_) => Check::pass("request accepted"),
        Err(error) => Check::fail(
            crate::redact::redact(&error.to_string()),
            Some(error.kind()),
        ),
    }
}

fn map_ureq_error(error: ureq::Error) -> ProviderError {
    match error {
        ureq::Error::Status(status, response) => {
            let message = response
                .into_json::<Value>()
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
                .unwrap_or_else(|| format!("HTTP {status}"));
            ProviderError::Api { status, message }
        }
        ureq::Error::Transport(error) => ProviderError::Http(error.to_string()),
    }
}

pub fn cache_path(report: &Report) -> Option<PathBuf> {
    let key = cache_key(&report.endpoint, &report.model, &report.transport);
    crate::config::data_root().map(|root| {
        root.join("aishe")
            .join("capabilities")
            .join(format!("{key}.json"))
    })
}

pub fn save(report: &Report) -> Result<()> {
    let path = cache_path(report).context("data directory is unavailable")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        set_private(parent, 0o700);
    }
    let bytes = serde_json::to_vec_pretty(report)?;
    crate::config::write_atomic(&path, &bytes)?;
    set_private(&path, 0o600);
    Ok(())
}

pub fn load(config: &Config) -> Option<Report> {
    let provider = if config.aishe.provider == "openai" {
        &config.providers.openai
    } else {
        &config.providers.anthropic
    };
    let transport = if config.aishe.provider == "openai" {
        provider.transport.as_str()
    } else {
        "messages"
    };
    let key = cache_key(&provider.base_url, &provider.model, transport);
    let path = crate::config::data_root()?
        .join("aishe")
        .join("capabilities")
        .join(format!("{key}.json"));
    let report: Report = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let fresh = now_ms().saturating_sub(report.checked_at_ms) <= u128::from(CACHE_TTL_SECS) * 1000;
    (report.schema_version == CACHE_SCHEMA_VERSION && fresh).then_some(report)
}

pub fn clear() -> Result<usize> {
    let Some(root) = crate::config::data_root() else {
        return Ok(0);
    };
    let directory = root.join("aishe").join("capabilities");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Remove only expired or unreadable capability records. Used by Doctor's safe
/// repair path so a healthy, recent validation is never discarded.
pub fn clear_stale() -> Result<usize> {
    let Some(root) = crate::config::data_root() else {
        return Ok(0);
    };
    let directory = root.join("aishe").join("capabilities");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let now = now_ms();
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let stale = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Report>(&bytes).ok())
            .map(|report| {
                report.schema_version != CACHE_SCHEMA_VERSION
                    || now.saturating_sub(report.checked_at_ms) > u128::from(CACHE_TTL_SECS) * 1000
            })
            .unwrap_or(true);
        if stale && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn cache_key(endpoint: &str, model: &str, transport: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in endpoint
        .trim_end_matches('/')
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(0xff))
        .chain(model.as_bytes().iter().copied())
        .chain(std::iter::once(0xfe))
        .chain(transport.as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(unix)]
fn set_private(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_private(_path: &std::path::Path, _mode: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_scoped_and_secret_free() {
        let a = cache_key("https://one.test", "m", "chat");
        let b = cache_key("https://two.test", "m", "chat");
        let c = cache_key("https://one.test", "m", "responses");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert!(!a.contains("one"));
    }

    #[test]
    fn verified_requires_every_live_check() {
        let check = Check::pass("ok");
        let mut report = Report {
            schema_version: 1,
            checked_at_ms: 0,
            provider: "openai".into(),
            endpoint: "http://localhost".into(),
            model: "m".into(),
            transport: "chat".into(),
            credential_env: "KEY".into(),
            credential_profile: "local".into(),
            credential_source: "not_required".into(),
            credential_required: false,
            credential: check.clone(),
            reachability: check.clone(),
            model_list: check.clone(),
            model_available: check.clone(),
            text: check.clone(),
            structured: check.clone(),
            tools: check.clone(),
            streaming: check,
        };
        assert!(report.verified());
        report.tools = Check::fail("no", Some(ErrorKind::UnsupportedTools));
        assert!(!report.verified());
    }

    #[test]
    fn model_catalog_rejects_empty_oversized_and_control_bearing_ids() {
        let value = serde_json::json!({
            "data": [
                {"id": " valid-model "},
                {"id": ""},
                {"id": "bad\u{001b}[2K"},
                {"id": "x".repeat(257)},
                {"not_id": "ignored"}
            ]
        });
        assert_eq!(model_ids(&value), ["valid-model"]);
    }

    #[test]
    fn missing_required_credential_blocks_provider_network_checks() {
        let mut config = Config::default();
        config.aishe.provider = "openai".into();
        config.providers.openai.base_url = "http://127.0.0.1:9".into();
        config.providers.openai.credential = "test-missing-capability".into();
        config.providers.openai.api_key_env = "AISHE_TEST_MISSING_CAPABILITY_KEY".into();
        config.providers.openai.auth_required = Some(true);
        std::env::remove_var("AISHE_TEST_MISSING_CAPABILITY_KEY");

        let report = validate(&config, true);
        assert_eq!(report.credential.state, State::Fail);
        assert_eq!(report.reachability.state, State::Skipped);
        assert_eq!(report.model_list.state, State::Skipped);
        assert_eq!(report.model_available.state, State::Skipped);
        assert_eq!(report.text.state, State::Skipped);
        assert!(report
            .reachability
            .detail
            .contains("credential profile 'test-missing-capability' is unavailable"));
    }

    #[test]
    fn managed_validation_requires_an_explicit_terminal_completion() {
        assert!(ensure_managed_turn_completed(&[
            crate::agent::AgentEvent::Connected,
            crate::agent::AgentEvent::Completed {
                summary: String::new(),
            },
        ])
        .is_ok());
        assert!(ensure_managed_turn_completed(&[crate::agent::AgentEvent::Connected]).is_err());
        assert!(
            ensure_managed_turn_completed(&[crate::agent::AgentEvent::Failed {
                error: crate::agent::UserFacingError {
                    code: "invalid_model".into(),
                    message: "model rejected".into(),
                    retryable: false,
                },
            }])
            .is_err()
        );
    }

    #[test]
    fn managed_validation_collects_only_completed_text() {
        let events = [
            crate::agent::AgentEvent::TextDelta {
                text: "partial".into(),
            },
            crate::agent::AgentEvent::TextCompleted {
                text: "{\"type\":\"answer\",\"command\":\"\",\"explanation\":\"setup-ok\"}".into(),
            },
        ];
        let text = completed_text(&events);
        assert!(!text.contains("partial"));
        assert_eq!(
            crate::modes::extract_json(&text)
                .and_then(|json| serde_json::from_str::<Value>(&json).ok())
                .and_then(|value| value.get("explanation").cloned()),
            Some(Value::String("setup-ok".into()))
        );
    }
}
