//! Provider/model discovery, live validation, and persisted compatibility
//! evidence. Probes intentionally use the same provider implementation as
//! runtime requests so Setup cannot certify a path the shell will not use.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::Config;
use crate::providers::{self, ErrorKind, Msg, ProviderError, Reach, ResponseFormat, ToolDef};

pub const CACHE_SCHEMA_VERSION: u32 = 2;
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
    #[serde(default)]
    pub connection_id: String,
    #[serde(default)]
    pub connection_identity: String,
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub models: Vec<String>,
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
        self.credential.state == State::Pass
            && self.reachability.state != State::Fail
            && self.model_available.state == State::Pass
            && self.live_verified()
    }

    /// Everything that can be checked without spending tokens.
    pub fn locally_verified(&self) -> bool {
        self.credential.state == State::Pass
            && self.reachability.state != State::Fail
            && self.model_available.state == State::Pass
    }

    /// The paid checks were declined, not failed.
    pub fn live_skipped(&self) -> bool {
        [&self.text, &self.structured, &self.tools, &self.streaming]
            .iter()
            .all(|check| check.state == State::Skipped)
    }

    /// One sentence for setup, review, and the tour, so a working install is
    /// never told three times that it has "warnings".
    pub fn verdict_label(&self) -> &'static str {
        if self.verified() {
            "verified"
        } else if self.locally_verified() && self.live_skipped() {
            "local checks passed · live checks not run (aishe setup --verify --live)"
        } else {
            "warnings remain; run `aishe setup --verify --live`"
        }
    }

    pub fn live_verified(&self) -> bool {
        [&self.text, &self.structured, &self.tools, &self.streaming]
            .iter()
            .all(|check| check.state == State::Pass)
    }
}

pub fn list_models(config: &Config, provider_name: &str) -> Result<Vec<String>, ProviderError> {
    let connection_id = config
        .resolve_connection_id(provider_name)
        .map_err(|error| ProviderError::Api {
            status: 0,
            message: error.to_string(),
        })?;
    let resolved = crate::connection::resolve_id(config, &connection_id).map_err(|error| {
        ProviderError::Api {
            status: 0,
            message: crate::redact::redact(&error.to_string()),
        }
    })?;
    // Subscription OAuth (Codex / Grok) is enumerated through the managed
    // OpenCode runtime for this connection — not public /v1/models.
    if matches!(resolved.auth, crate::connection::ResolvedAuth::OAuth { .. }) {
        return list_models_via_opencode(config, &connection_id, &resolved.settings.model);
    }
    let anthropic = resolved.provider == "anthropic";
    let provider = resolved.settings;
    let key = resolved.api_key;

    let base = crate::provider_catalog::normalize_base_url(&provider.base_url);
    let endpoint = format!("{base}/v1/models");
    let agent = providers::external_http_agent(
        std::time::Duration::from_secs(3),
        Some(std::time::Duration::from_secs(10)),
        None,
        None,
    );
    let mut request = agent.get(&endpoint);
    if let Some(key) = key.as_deref() {
        request = if anthropic {
            request
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        } else {
            request.header("Authorization", format!("Bearer {key}"))
        };
    }
    let mut response = request
        .call()
        .map_err(|error| ProviderError::Http(error.to_string()))?;
    if !providers::status_is_accepted(response.status()) {
        return Err(map_http_response(response));
    }
    let value: Value = response
        .body_mut()
        .with_config()
        .limit(providers::MAX_PROVIDER_BODY_BYTES)
        .read_json()
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
    let connection_id = config.active_connection_id().to_string();
    let connection = config.active_connection();
    let provider_name = config.active_provider_name().to_string();
    let provider = config.active_provider_config();
    let transport = if provider_name == "anthropic" {
        "messages".to_string()
    } else {
        provider.transport.clone()
    };
    let profile = connection.map_or_else(
        || provider.credential_profile(),
        |connection| match &connection.auth {
            crate::config::ConnectionAuth::OAuth { profile } => profile.clone(),
            crate::config::ConnectionAuth::ApiKey { credential, .. } => credential
                .clone()
                .unwrap_or_else(|| connection.settings.credential_profile()),
            crate::config::ConnectionAuth::None => String::new(),
            crate::config::ConnectionAuth::Auto => provider.credential_profile(),
        },
    );
    let resolution = crate::connection::resolve(config);
    let using_oauth = resolution.as_ref().is_ok_and(|resolved| {
        matches!(resolved.auth, crate::connection::ResolvedAuth::OAuth { .. })
    });
    let credential_source = match &resolution {
        Ok(resolved) => match &resolved.auth {
            crate::connection::ResolvedAuth::ApiKey { source } => source.clone(),
            crate::connection::ResolvedAuth::OAuth { provider, profile } => {
                format!("OAuth {provider}/{profile}")
            }
            crate::connection::ResolvedAuth::None => "not required".into(),
        },
        Err(_) => "unavailable".into(),
    };
    let credential = match &resolution {
        Ok(resolved) => match &resolved.auth {
            crate::connection::ResolvedAuth::ApiKey { source } => {
                Check::pass(format!("credential available from {source}"))
            }
            crate::connection::ResolvedAuth::OAuth { provider, profile } => Check::pass(format!(
                "OAuth credential available for {provider}/{profile}"
            )),
            crate::connection::ResolvedAuth::None => {
                Check::pass("credential not required for this connection")
            }
        },
        Err(error) => Check::fail(
            crate::redact::redact(&error.to_string()),
            Some(ErrorKind::MissingCredential),
        ),
    };

    // A required credential that is already known to be absent is definitive.
    // Do not make anonymous network requests that can only add a confusing 401
    // beside the actionable missing-variable diagnosis.
    let credential_missing = credential.state == State::Fail;
    let (reachability, models, model_list) = if credential_missing {
        let detail = format!(
            "blocked because credential profile '{profile}' is unavailable for connection '{connection_id}'"
        );
        (Check::skipped(detail.clone()), None, Check::skipped(detail))
    } else if using_oauth {
        let detail = "OAuth transport is provided by managed OpenCode; use --live to validate it";
        (Check::skipped(detail), None, Check::skipped(detail))
    } else {
        let probe = providers::probe(config, &connection_id);
        let reachability = match probe.reach {
            Reach::Up(status) => Check::pass(format!("endpoint answered HTTP {status}")),
            Reach::Unauthorized(status) => Check::fail(
                format!("endpoint rejected credentials (HTTP {status})"),
                Some(ErrorKind::InvalidCredential),
            ),
            Reach::ManagedOAuth(provider) => Check::skipped(format!(
                "{provider} OAuth is validated through managed OpenCode"
            )),
            Reach::Down(error) => Check::fail(error, Some(ErrorKind::Network)),
        };
        let (models, model_list) = match list_models(config, &connection_id) {
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
        connection_id: connection_id.clone(),
        connection_identity: connection
            .map(|connection| {
                crate::connection::configured_launch_identity(&connection_id, connection)
            })
            .unwrap_or_default(),
        provider: provider_name,
        endpoint: provider.base_url.clone(),
        model: provider.model.clone(),
        models: models.unwrap_or_default(),
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

    struct ValidationGuard {
        workspace: PathBuf,
        supervisor_key: String,
    }
    impl Drop for ValidationGuard {
        fn drop(&mut self) {
            let _ = crate::backend::control::request_stop_for(&self.supervisor_key);
            let _ = std::fs::remove_dir_all(&self.workspace);
        }
    }

    let workspace = std::env::temp_dir().join(format!(
        "aishe-managed-validation-{}-{:032x}",
        std::process::id(),
        rand::random::<u128>()
    ));
    std::fs::create_dir_all(&workspace)?;
    crate::config::set_private_dir(&workspace);
    let resolved = crate::connection::resolve(config)?;
    let secret = resolved.api_key;
    let _guard = ValidationGuard {
        workspace: workspace.clone(),
        supervisor_key: resolved.launch_identity,
    };

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
    let session = client.create_session(&workspace, "AIShe setup validation", scope, network)?;
    let price = crate::usage::budget_price_for(config.active_model(), &config.pricing);
    let shell_id = format!("setup_{:032x}", rand::random::<u128>());
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
        "This is setup validation. Return the required AIShe JSON object with type answer, an empty command, and explanation setup-ok. Do not return plain text or Markdown.",
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
    let structured = if managed_suggest_contract_valid(&answer) {
        Check::pass("managed suggest contract returned valid structured output")
    } else {
        // The managed adapter cannot use OpenCode 1.18.27's broken durable
        // json_schema format, so it enforces the trusted JSON protocol in the
        // agent prompt. Model output can still be stochastic: make one bounded,
        // explicit retry before classifying a supported transport as failed.
        let retry_events = run(
            Mode::Suggest,
            "Setup validation retry. Return only the required AIShe JSON object: type answer, command empty, explanation setup-ok. Do not return plain text, Markdown, or a code fence.",
        )
        .context("managed structured-output retry")?;
        ensure_managed_turn_completed(&retry_events)?;
        if managed_suggest_contract_valid(&completed_text(&retry_events)) {
            Check::pass("managed suggest contract accepted after one bounded retry")
        } else {
            Check::fail(
                "managed suggest response did not satisfy AIShe's structured contract after one bounded retry",
                Some(ErrorKind::UnsupportedFormat),
            )
        }
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
            "managed agent did not complete the requested AIShe proxy-tool round trip",
            Some(ErrorKind::UnsupportedTools),
        )
    } else {
        Check::pass("managed AIShe proxy tool completed with credential isolation")
    };

    Ok((text, structured, tools, streaming))
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

fn managed_suggest_contract_valid(answer: &str) -> bool {
    crate::modes::extract_json(answer)
        .and_then(|json| serde_json::from_str::<Value>(&json).ok())
        .is_some_and(|value| {
            matches!(
                value.get("type").and_then(Value::as_str),
                Some("answer" | "command")
            ) && value.get("explanation").and_then(Value::as_str).is_some()
        })
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

fn map_http_response(mut response: providers::HttpResponse) -> ProviderError {
    let status = response.status().as_u16();
    let message = response
        .body_mut()
        .with_config()
        .limit(providers::MAX_PROVIDER_BODY_BYTES)
        .read_json::<Value>()
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

pub fn cache_path(report: &Report) -> Option<PathBuf> {
    let key = cache_key(
        &report.connection_identity,
        &report.endpoint,
        &report.model,
        &report.transport,
    );
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
    let connection_id = config.active_connection_id();
    let connection = config.active_connection()?;
    let provider = config.active_provider_config();
    let transport = if config.active_provider_name() == "anthropic" {
        "messages"
    } else {
        provider.transport.as_str()
    };
    let identity = crate::connection::configured_launch_identity(connection_id, connection);
    let key = cache_key(&identity, &provider.base_url, &provider.model, transport);
    let path = crate::config::data_root()?
        .join("aishe")
        .join("capabilities")
        .join(format!("{key}.json"));
    let report: Report = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    let fresh = now_ms().saturating_sub(report.checked_at_ms) <= u128::from(CACHE_TTL_SECS) * 1000;
    (report.schema_version == CACHE_SCHEMA_VERSION
        && report.connection_id == connection_id
        && report.connection_identity == identity
        && fresh)
        .then_some(report)
}

/// Configured, cached, static-catalog, and recently audited models for a single
/// connection. For OAuth connections this also asks the managed OpenCode
/// runtime (may start it) so `/model` mirrors the subscription catalog.
pub fn known_models(config: &Config, connection_id: &str) -> Result<Vec<String>> {
    let id = config.resolve_connection_id(connection_id)?;
    let mut selected = config.clone();
    selected.select_connection(&id)?;
    let connection = selected
        .connections
        .get(&id)
        .context("selected connection disappeared")?;
    let mut models = vec![connection.settings.model.clone()];
    if let Some(report) = load(&selected) {
        models.extend(report.models);
        models.push(report.model);
    }
    let endpoint = crate::provider_catalog::normalize_base_url(&connection.settings.base_url);
    models.extend(
        crate::provider_catalog::SERVICES
            .iter()
            .filter(|service| {
                !service.model.is_empty()
                    && crate::provider_catalog::normalize_base_url(service.base_url) == endpoint
            })
            .map(|service| service.model.to_string()),
    );
    let mut recent = crate::audit::read_entries(&crate::audit::default_path());
    recent.reverse();
    models.extend(
        recent
            .into_iter()
            .filter(|entry| entry.connection_id.as_deref() == Some(id.as_str()))
            .filter_map(|entry| entry.model)
            .take(20),
    );
    // Live OpenCode catalog for subscription OAuth (Codex / Grok). Fail soft so
    // a stopped runtime still leaves the configured model pickable.
    if connection.uses_oauth() {
        if let Ok(live) = list_models(config, &id) {
            models.extend(live);
        }
    }
    models.retain(|model| crate::connection::validate_model_id(model).is_ok());
    models.sort();
    models.dedup();
    if let Some(position) = models
        .iter()
        .position(|model| model == &connection.settings.model)
    {
        models.swap(0, position);
    }
    Ok(models)
}

/// Ask the managed OpenCode server for models on the active OAuth connection.
fn list_models_via_opencode(
    config: &Config,
    connection_id: &str,
    configured_model: &str,
) -> Result<Vec<String>, ProviderError> {
    let mut selected = config.clone();
    selected
        .select_connection(connection_id)
        .map_err(|error| ProviderError::Api {
            status: 0,
            message: crate::redact::redact(&error.to_string()),
        })?;
    let state = crate::backend::supervisor::ensure_running(&selected).map_err(|error| {
        ProviderError::Api {
            status: 0,
            message: crate::redact::redact(&format!(
                "managed OpenCode could not start for model discovery: {error}"
            )),
        }
    })?;
    let client = crate::backend::opencode::OpenCodeClient::new(
        state.opencode_connection(),
        state.provider_id,
        state.model_id,
    )
    .map_err(|error| ProviderError::Api {
        status: 0,
        message: crate::redact::redact(&error.to_string()),
    })?;
    let mut models = client
        .list_provider_models()
        .map_err(|error| ProviderError::Api {
            status: 0,
            message: crate::redact::redact(&error.to_string()),
        })?;
    if !configured_model.is_empty() && !models.iter().any(|model| model == configured_model) {
        models.push(configured_model.to_string());
    }
    models.retain(|model| crate::connection::validate_model_id(model).is_ok());
    models.sort();
    models.dedup();
    if let Some(position) = models.iter().position(|model| model == configured_model) {
        models.swap(0, position);
    }
    if models.is_empty() {
        return Ok(vec![configured_model.to_string()]);
    }
    Ok(models)
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

fn cache_key(identity: &str, endpoint: &str, model: &str, transport: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in identity
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(0xfd))
        .chain(endpoint.trim_end_matches('/').as_bytes().iter().copied())
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
        let a = cache_key("connection-a", "https://one.test", "m", "chat");
        let b = cache_key("connection-a", "https://two.test", "m", "chat");
        let c = cache_key("connection-a", "https://one.test", "m", "responses");
        let d = cache_key("connection-b", "https://one.test", "m", "chat");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(a, d);
        assert!(!a.contains("one"));
    }

    #[test]
    fn verified_requires_every_live_check() {
        let check = Check::pass("ok");
        let mut report = Report {
            schema_version: CACHE_SCHEMA_VERSION,
            checked_at_ms: 0,
            connection_id: "local".into(),
            connection_identity: "c-test".into(),
            provider: "openai".into(),
            endpoint: "http://localhost".into(),
            model: "m".into(),
            models: vec!["m".into()],
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
        report.reachability = Check::skipped("validated by managed live requests");
        assert!(report.verified());
        report.tools = Check::fail("no", Some(ErrorKind::UnsupportedTools));
        assert!(!report.verified());
    }

    #[test]
    fn verdict_label_separates_skipped_live_checks_from_warnings() {
        let check = Check::pass("ok");
        let mut report = Report {
            schema_version: CACHE_SCHEMA_VERSION,
            checked_at_ms: 0,
            connection_id: "local".into(),
            connection_identity: "c-test".into(),
            provider: "openai".into(),
            endpoint: "http://localhost".into(),
            model: "m".into(),
            models: vec!["m".into()],
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
        assert_eq!(report.verdict_label(), "verified");
        // Declining the paid checks is not a warning.
        for field in [
            &mut report.text,
            &mut report.structured,
            &mut report.tools,
            &mut report.streaming,
        ] {
            *field = Check::skipped("declined");
        }
        assert_eq!(
            report.verdict_label(),
            "local checks passed · live checks not run (aishe setup --verify --live)"
        );
        report.credential = Check::fail("missing", None);
        assert_eq!(
            report.verdict_label(),
            "warnings remain; run `aishe setup --verify --live`"
        );
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

    #[test]
    fn managed_validation_structured_contract_is_explicit_and_fence_tolerant() {
        assert!(managed_suggest_contract_valid(
            r#"{"type":"answer","command":"","explanation":"setup-ok"}"#
        ));
        assert!(managed_suggest_contract_valid(
            "```json\n{\"type\":\"command\",\"command\":\"pwd\",\"explanation\":\"cwd\"}\n```"
        ));
        assert!(!managed_suggest_contract_valid("setup-ok"));
        assert!(!managed_suggest_contract_valid(
            r#"{"type":"answer","command":""}"#
        ));
    }
}
