use std::io::IsTerminal;

use anyhow::Result;

use crate::dispatcher::{self, CommandCache};

/// Parsed managed-backend action transferred from the binary's Clap surface.
#[derive(Clone, Debug)]
pub enum Action {
    Status {
        json: bool,
    },
    Install {
        from: Option<std::path::PathBuf>,
        force: bool,
        json: bool,
    },
    Verify {
        live: bool,
        json: bool,
    },
    Repair {
        from: Option<std::path::PathBuf>,
        json: bool,
    },
    Rollback,
    Stop,
    Logs {
        tail: usize,
    },
    Gc {
        dry_run: bool,
    },
}

pub fn command(command: &Action) -> Result<u8> {
    use crate::backend::{InstallSource, RuntimeManager, RuntimeStatus};

    let manager = RuntimeManager::new()?;
    match command {
        Action::Status { json } => {
            let status = manager.status();
            let (supervisor, supervisor_lines) = backend_instance_status()?;
            if *json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": 1,
                        "runtime": &status,
                        "supervisor": supervisor
                    }))?
                );
            } else {
                match &status {
                    RuntimeStatus::Ready {
                        version,
                        binary,
                        sha256,
                    } => {
                        println!("agent runtime: OpenCode {version} · ready");
                        println!("binary: {}", binary.display());
                        println!("sha256: {sha256}");
                    }
                    RuntimeStatus::Missing { expected_version } => {
                        println!("agent runtime: OpenCode {expected_version} · not installed");
                        println!("next: aishe backend install");
                    }
                    RuntimeStatus::Invalid {
                        expected_version,
                        reason,
                    } => {
                        println!("agent runtime: OpenCode {expected_version} · invalid");
                        println!("reason: {reason}");
                        println!("next: aishe backend repair");
                    }
                }
                for line in supervisor_lines {
                    println!("{line}");
                }
            }
            Ok(if matches!(status, RuntimeStatus::Ready { .. }) {
                0
            } else {
                1
            })
        }
        Action::Install { from, force, json } => {
            let source = from
                .clone()
                .map(InstallSource::Local)
                .unwrap_or(InstallSource::Default);
            let status = manager.install(source, *force)?;
            if *json {
                crate::cli::json_contract::print_envelope("runtime", &status)?;
            } else if let RuntimeStatus::Ready {
                version, binary, ..
            } = status
            {
                println!("✓ installed OpenCode {version}");
                println!("  {}", binary.display());
            }
            Ok(0)
        }
        Action::Verify { live, json } => {
            let status = manager.verify()?;
            if *live {
                // The supervisor health probe is intentionally distinct from
                // `--version`; until a provider is needed it starts with no key.
                crate::backend::supervisor::smoke_test(&manager)?;
            }
            if *json {
                crate::cli::json_contract::print_object(&serde_json::json!({
                    "runtime": status,
                    "live": live,
                }))?;
            } else {
                println!(
                    "✓ managed runtime{} verified",
                    if *live { " and server" } else { "" }
                );
            }
            Ok(0)
        }
        Action::Repair { from, json } => {
            let source = from
                .clone()
                .map(InstallSource::Local)
                .unwrap_or(InstallSource::Default);
            let status = manager.install(source, true)?;
            if *json {
                crate::cli::json_contract::print_envelope("runtime", &status)?;
            } else {
                println!("✓ managed OpenCode runtime repaired");
            }
            Ok(0)
        }
        Action::Rollback => {
            let _ = crate::backend::supervisor::request_stop();
            let status = manager.rollback()?;
            if let RuntimeStatus::Ready {
                version, sha256, ..
            } = status
            {
                println!("✓ rolled back to the prior verified OpenCode {version} install");
                println!("  sha256: {sha256}");
            }
            Ok(0)
        }
        Action::Stop => crate::backend::supervisor::request_stop(),
        Action::Logs { tail } => {
            crate::backend::supervisor::print_logs(*tail)?;
            Ok(0)
        }
        Action::Gc { dry_run } => {
            let removed = manager.garbage_collect(*dry_run)?;
            for path in &removed {
                println!(
                    "{} {}",
                    if *dry_run { "would remove" } else { "removed" },
                    path.display()
                );
            }
            if removed.is_empty() {
                println!("runtime cache is clean");
            }
            Ok(0)
        }
    }
}

fn backend_instance_status() -> Result<(serde_json::Value, Vec<String>)> {
    let mut rows = Vec::new();
    let mut lines = Vec::new();
    let keys = crate::backend::supervisor::instance_keys()?;
    for key in &keys {
        let loaded = crate::backend::control::load_state_for(key);
        match crate::backend::control::verified_state_for(key) {
            Ok(Some(state)) => {
                let loopback = state.control_url.starts_with("http://127.0.0.1:")
                    && state.opencode_url.starts_with("http://127.0.0.1:");
                let connection = if state.connection_id.is_empty() {
                    "legacy"
                } else {
                    state.connection_id.as_str()
                };
                rows.push(serde_json::json!({
                    "key": key,
                    "state": "running",
                    "connection_id": state.connection_id,
                    "supervisor_pid": state.supervisor_pid,
                    "opencode_pid": state.opencode_pid,
                    "runtime_version": state.runtime_version,
                    "plugin_sha256": state.plugin_sha256,
                    "provider_id": state.provider_id,
                    "model_id": state.model_id,
                    "started_at_ms": state.started_at_ms,
                    "loopback": loopback
                }));
                lines.push(format!(
                    "supervisor: running · connection {} · pid {} · OpenCode pid {} · {}/{}",
                    crate::commands::display_safe(connection),
                    state.supervisor_pid,
                    state.opencode_pid,
                    crate::commands::display_safe(&state.provider_id),
                    crate::commands::display_safe(&state.model_id)
                ));
            }
            Ok(None) => match loaded {
                Ok(Some(state)) => {
                    rows.push(serde_json::json!({
                        "key": key,
                        "state": "stale",
                        "connection_id": state.connection_id,
                        "provider_id": state.provider_id,
                        "model_id": state.model_id,
                    }));
                    lines.push(format!(
                        "supervisor: stale · connection {} (Doctor can repair it)",
                        crate::commands::display_safe(if state.connection_id.is_empty() {
                            "legacy"
                        } else {
                            &state.connection_id
                        })
                    ));
                }
                Ok(None) => {}
                Err(error) => {
                    let detail = crate::redact::redact(&error.to_string());
                    rows.push(serde_json::json!({"key":key,"state":"invalid","detail":detail}));
                    lines.push(format!("supervisor: invalid · {detail}"));
                }
            },
            Err(error) => {
                let detail = crate::redact::redact(&error.to_string());
                rows.push(serde_json::json!({"key":key,"state":"invalid","detail":detail}));
                lines.push(format!("supervisor: invalid · {detail}"));
            }
        }
    }

    if keys.is_empty() {
        match (
            crate::backend::control::load_state(),
            crate::backend::control::verified_state(),
        ) {
            (_, Ok(Some(state))) => {
                rows.push(serde_json::json!({
                    "state":"running",
                    "connection_id":state.connection_id,
                    "provider_id":state.provider_id,
                    "model_id":state.model_id,
                    "supervisor_pid":state.supervisor_pid,
                    "opencode_pid":state.opencode_pid,
                    "legacy_layout":true,
                }));
                lines.push("supervisor: running in legacy layout".into());
            }
            (Ok(Some(_)), Ok(None)) => {
                rows.push(serde_json::json!({"state":"stale","legacy_layout":true}));
                lines.push("supervisor: stale legacy state (Doctor can repair it)".into());
            }
            (Ok(None), Ok(None)) => {
                lines.push("supervisor: stopped (starts on the next AI turn)".into());
            }
            (Err(error), _) | (_, Err(error)) => {
                let detail = crate::redact::redact(&error.to_string());
                rows.push(
                    serde_json::json!({"state":"invalid","detail":detail,"legacy_layout":true}),
                );
                lines.push(format!("supervisor: invalid state · {detail}"));
            }
        }
    }
    let running = rows
        .iter()
        .filter(|row| row.get("state").and_then(serde_json::Value::as_str) == Some("running"))
        .count();
    let overall = if running > 0 {
        "running"
    } else if rows.is_empty() {
        "stopped"
    } else {
        "stale"
    };
    Ok((
        serde_json::json!({"state":overall,"active_instances":running,"instances":rows}),
        lines,
    ))
}

pub fn uninstall(selection: crate::uninstall::Selection, dry_run: bool, yes: bool) -> Result<u8> {
    let plan = crate::uninstall::Plan::discover(selection)?;
    let existing = plan.existing_targets();
    println!(
        "{}",
        if dry_run {
            "AIShe uninstall preview"
        } else {
            "AIShe uninstall plan"
        }
    );
    println!("User state is preserved unless its category was explicitly selected.");
    let selected_categories = [
        (plan.selection.binary, "binary/completions/man"),
        (plan.selection.runtime, "managed runtime/cache"),
        (plan.selection.sessions, "AI sessions/tool journals"),
        (plan.selection.config, "config/credentials"),
        (plan.selection.history, "shell history"),
        (plan.selection.audit_undo, "audit/undo data"),
    ]
    .into_iter()
    .filter_map(|(selected, name)| selected.then_some(name))
    .collect::<Vec<_>>();
    println!("Selected: {}", selected_categories.join(", "));
    if existing.is_empty() {
        println!("Nothing selected is currently installed.");
        return Ok(0);
    }
    let mut previous = "";
    for target in &existing {
        if target.category != previous {
            println!("\n{}:", target.category);
            previous = target.category;
        }
        println!(
            "  {} {}",
            if target.recoverable {
                "replaceable"
            } else {
                "permanent"
            },
            target.path.display()
        );
    }
    if dry_run {
        println!("\nNo files were changed.");
        return Ok(0);
    }

    if !yes {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!(
                "confirmation required; review `aishe uninstall --dry-run` and rerun with --yes"
            );
        }
        use std::io::Write;
        if plan.selection.includes_user_state() {
            print!(
                "\nSelected user state cannot be recovered by AIShe. Type `delete` to continue: "
            );
        } else {
            print!("\nRemove the selected replaceable components? [y/N] ");
        }
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        let confirmed = if plan.selection.includes_user_state() {
            answer.trim() == "delete"
        } else {
            matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        };
        if !confirmed {
            println!("Cancelled; no files were changed.");
            return Ok(2);
        }
    }

    let removed = plan.apply()?;
    println!("\nRemoved {} selected path(s).", removed.len());
    if plan.selection.includes_user_state() {
        println!("Selected user state was permanently removed and is not recoverable by AIShe.");
    } else {
        println!("Config, credentials, history, sessions, audit, and undo data were preserved.");
    }
    Ok(0)
}

pub fn route(words: &[String], json: bool) -> Result<u8> {
    let line = words.join(" ");
    if line.trim().is_empty() {
        eprintln!("aishe: route needs a non-empty line after `--`");
        return Ok(2);
    }
    let cache = CommandCache::new();
    cache.discover_local();
    let decision = dispatcher::route(&line, &cache);

    if json {
        println!("{}", serde_json::to_string_pretty(&decision.diagnostic())?);
        return Ok(0);
    }

    println!("route: {}", decision.kind);
    println!("reason: {}", decision.reason);
    match decision.head.as_deref() {
        Some(head) => println!(
            "effective head: {} (known command: {})",
            dispatcher::safe_diagnostic_text(head),
            if decision.known_command { "yes" } else { "no" }
        ),
        None => println!("effective head: none (known command: no)"),
    }
    println!(
        "ambiguous command phrase: {}",
        if decision.ambiguous { "yes" } else { "no" }
    );
    println!("source: {}", decision.source);
    if line.trim_start().starts_with('#') {
        println!("migration: # is a deprecated agent prefix; use ? (planned removal: AIShe 0.9)");
    }
    match decision.kind {
        dispatcher::RouteKind::Shell => {
            println!("action: pass this line directly to the shell");
        }
        dispatcher::RouteKind::NaturalLanguage => {
            println!("action: send this line to the configured agent");
        }
        dispatcher::RouteKind::Builtin => {
            println!("action: invoke an AIShe builtin locally");
        }
    }
    if decision.reason == dispatcher::RouteReason::ForcedShell {
        println!("safety: ! applies to this line only and bypasses the AI safety gate");
    }
    let opposite = decision.opposite_route_override();
    println!("override: {}", opposite.guidance);
    Ok(0)
}
