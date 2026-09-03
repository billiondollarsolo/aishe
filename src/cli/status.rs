use crate::config::Config;
use crate::providers::Provider;

pub fn command(config: &Config, json: bool) -> u8 {
    let connection_id = session_value("AISHE_CONNECTION", config.active_connection_id());
    let session = std::env::var_os("AISHE_USAGE_FILE")
        .filter(|value| !value.is_empty())
        .and_then(|path| {
            crate::usagelog::summarize_for_connection(
                std::path::Path::new(&path),
                &config.pricing,
                Some(&connection_id),
            )
        });
    let metrics = live_status_metrics();
    let model = session_value("AISHE_MODEL", config.active_model());
    let mode = session_value("AISHE_MODE", &config.aishe.mode);
    let scope = session_value("AISHE_SCOPE", &config.backend.default_scope);
    let output = session_value("AISHE_AGENT_OUTPUT", &config.backend.output);
    let reasoning = session_value("AISHE_REASONING", config.active_reasoning_effort());
    let connection = config
        .connections
        .get(&connection_id)
        .or_else(|| config.active_connection());
    let connection_label = connection
        .map(|value| value.label.clone())
        .unwrap_or_else(|| connection_id.clone());
    let provider = connection
        .map(|value| value.provider.clone())
        .unwrap_or_else(|| config.active_provider_name().to_string());
    let auth = connection
        .map(|value| value.auth_label())
        .unwrap_or_default();
    let auth_state = connection
        .map(crate::cli::connection::auth_state)
        .unwrap_or_default();
    let endpoint_host = connection
        .and_then(|value| url::Url::parse(&value.settings.base_url).ok())
        .and_then(|url| url.host_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "unknown".into());
    let backend_readiness = backend_readiness(config);
    let selection_scope = if crate::connection::selection_is_shell_local() {
        "this shell"
    } else {
        "default"
    };
    let status_position = if config.aishe.status_line {
        config.aishe.status_line_position.as_str()
    } else {
        "off"
    };
    let (audit_enabled, _) = crate::cli::history::resolve_audit(
        config,
        std::env::var("AISHE_LOG").ok().as_deref(),
        std::env::var("AISHE_LOG_FILE").ok().as_deref(),
    );
    let audit_path = crate::cli::history::audit_log_path(config);
    let environment = crate::environment::inspect(
        config,
        &std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
    );

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_version": 1,
                "model": model,
                "connection": {
                    "id": connection_id,
                    "label": connection_label,
                    "provider": provider,
                    "endpoint_host": endpoint_host,
                    "auth": auth,
                    "auth_state": auth_state,
                    "selection_scope": selection_scope,
                },
                "mode": mode,
                "backend": {
                    "engine": config.backend.engine,
                    "readiness": backend_readiness,
                },
                "scope": scope,
                "network": config.backend.workspace_network,
                "output": output,
                "reasoning_effort": reasoning,
                "status_line": status_position,
                "budget_usd": config.aishe.budget_usd,
                "audit": {
                    "enabled": audit_enabled,
                    "redact": config.logging.redact,
                    "path": audit_path,
                },
                "session": session,
                "metrics": metrics,
                "environment": environment,
            }))
            .expect("serializing a serde_json::Value to String cannot fail")
        );
        return 0;
    }

    println!("AIShe status");
    // The label already carries the provider brand; the id belongs with auth.
    println!(
        "  connection: {} · {} · {}",
        crate::commands::display_safe(&connection_label),
        crate::commands::display_safe(&endpoint_host),
        selection_scope,
    );
    println!(
        "  auth: {} ({}) · {}",
        crate::commands::display_safe(&auth),
        crate::commands::display_safe(&connection_id),
        crate::commands::display_safe(&auth_state),
    );
    println!("  model: {}", crate::commands::display_safe(&model));
    println!(
        "  mode: {} · backend: {} ({}) · scope: {} · network: {}",
        crate::commands::display_safe(&mode),
        crate::commands::display_safe(&config.backend.engine),
        crate::commands::display_safe(&backend_readiness),
        crate::commands::display_safe(&scope),
        crate::commands::display_safe(&config.backend.workspace_network),
    );
    println!(
        "  output: {} · reasoning: {} · prompt status: {}",
        crate::commands::display_safe(&output),
        crate::commands::display_safe(&reasoning),
        crate::commands::display_safe(status_position),
    );
    println!(
        "  environment: {}{}{}",
        crate::commands::display_safe(&environment.label()),
        environment
            .git_head
            .as_deref()
            .map(|head| format!(" @ {head}"))
            .unwrap_or_default(),
        if environment.marker().is_empty() {
            String::new()
        } else {
            format!(" · {}", environment.marker())
        }
    );
    println!(
        "  audit: {} · redaction: {} · {}",
        if audit_enabled { "on" } else { "off" },
        if config.logging.redact { "on" } else { "off" },
        crate::commands::display_safe(&audit_path.display().to_string()),
    );
    if let Some(session) = session {
        println!(
            "  {}",
            crate::commands::display_safe(
                session.strip_prefix("aishe session: ").unwrap_or(&session)
            )
        );
    } else {
        println!("  session spend: no model calls yet");
    }
    let context = ["task", "elapsed", "context"]
        .iter()
        .filter_map(|key| metrics.get(*key))
        .map(|value| crate::commands::display_safe(value))
        .collect::<Vec<_>>();
    if !context.is_empty() {
        println!("  {}", context.join(" · "));
    }
    if config.aishe.budget_usd > 0.0 {
        println!("  budget: ${:.2}", config.aishe.budget_usd);
    } else {
        println!("  budget: unlimited");
    }
    println!("  controls: {}", crate::product_help::CONTROLS_HINT);
    0
}

fn backend_readiness(config: &Config) -> String {
    if config.backend.engine != "opencode" {
        return "native compatibility".into();
    }
    let runtime = match crate::backend::RuntimeManager::new().map(|manager| manager.status()) {
        Ok(crate::backend::RuntimeStatus::Ready { .. }) => "runtime ready",
        Ok(crate::backend::RuntimeStatus::Missing { .. }) => "runtime missing",
        Ok(crate::backend::RuntimeStatus::Invalid { .. }) => "runtime invalid",
        Err(_) => "runtime unavailable",
    };
    let instances = crate::backend::supervisor::instance_keys()
        .map(|keys| {
            keys.into_iter()
                .filter(|key| {
                    crate::backend::control::load_state_for(key)
                        .ok()
                        .flatten()
                        .is_some_and(|state| crate::backend::control::state_processes_exist(&state))
                })
                .count()
        })
        .unwrap_or(0);
    format!("{runtime}, {instances} active")
}

fn session_value(name: &str, fallback: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn live_status_metrics() -> std::collections::BTreeMap<String, String> {
    const ALLOWED: &[&str] = &[
        "task",
        "elapsed",
        "context",
        "last_tokens",
        "last_cost",
        "session_tokens",
        "session_cost",
        "budget",
        "requests",
        "network",
        "sandbox",
        "tasks",
    ];
    let Some(path) = std::env::var_os("AISHE_STATUS_FILE").filter(|value| !value.is_empty()) else {
        return std::collections::BTreeMap::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return std::collections::BTreeMap::new();
    };
    if bytes.len() > 64 * 1024 {
        return std::collections::BTreeMap::new();
    }
    let Ok(text) = String::from_utf8(bytes) else {
        return std::collections::BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(key, _)| ALLOWED.contains(key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Print the session token/cost summary (`aishe usage` / `/usage`).
/// Append this process's metered usage to the shared per-session tally named by
/// `AISHE_USAGE_FILE`, so the interactive PTY can print a one-line session-cost
/// summary on exit. No-op when the env var is unset (i.e. not under a PTY
/// session) or no model calls were made.
pub fn record_session_usage(provider: Option<&dyn Provider>, config: &Config) {
    let Ok(path) = std::env::var("AISHE_USAGE_FILE") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let Some(p) = provider else { return };
    let snap = p.meter().snapshot();
    if snap.is_empty() {
        return;
    }
    crate::usagelog::append_attributed(
        std::path::Path::new(&path),
        snap,
        config.active_model(),
        Some(config.active_connection_id()),
    );
    if let Ok(status_path) = std::env::var("AISHE_STATUS_FILE") {
        if !status_path.is_empty() {
            crate::usagelog::write_status_for_connection(
                std::path::Path::new(&status_path),
                std::path::Path::new(&path),
                &config.pricing,
                Some((snap, config.active_model())),
                &config.aishe.status_line_items,
                config.active_connection_id(),
            );
        }
    }
}

pub fn print_usage_summary(provider: Option<&dyn Provider>, config: &Config) {
    if let Some(summary) = std::env::var_os("AISHE_USAGE_FILE")
        .filter(|value| !value.is_empty())
        .and_then(|path| {
            crate::usagelog::summarize_for_connection(
                std::path::Path::new(&path),
                &config.pricing,
                Some(config.active_connection_id()),
            )
        })
    {
        println!("{summary}");
        if config.aishe.budget_usd > 0.0 {
            println!("budget: ${:.2}", config.aishe.budget_usd);
        }
        return;
    }
    match provider {
        Some(p) => {
            let snap = p.meter().snapshot();
            if snap.is_empty() {
                println!("usage: no model calls yet this session");
            } else {
                println!(
                    "usage: {}",
                    crate::usage::summary(snap, config.active_model(), &config.pricing)
                );
            }
            if config.aishe.budget_usd > 0.0 {
                println!(
                    "budget: ${:.2} (set budget_usd=0 for unlimited)",
                    config.aishe.budget_usd
                );
            }
        }
        // The managed backend has no legacy in-process provider; the usage file
        // above is the real source, so an empty one means no calls yet, not a
        // configuration problem.
        None => println!("usage: no model calls yet this session"),
    }
}
