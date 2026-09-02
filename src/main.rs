//! aishe — a natural-language-aware shell.
//!
//! Behaves like zsh for recognizable commands; anything else is treated as a
//! natural-language request handled by an LLM (suggest or yolo mode).

#[path = "cli/args.rs"]
mod args;

use args::*;

use std::io::IsTerminal;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

use aishe::commands::CommandRegistry;
use aishe::config::Config;
use aishe::dispatcher::{self, CommandCache};
use aishe::executor::Executor;
use aishe::providers::{self, Provider};
use aishe::skills::SkillRegistry;
use aishe::{context, integration};

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let public = aishe::user_error::UserError::from_error(error.as_ref());
            if aishe::ui::machine_output() {
                match public.render_json() {
                    Ok(document) => eprintln!("{document}"),
                    Err(_) => eprintln!(
                        "{{\"schema_version\":1,\"code\":\"internal.serialization_failed\",\"message\":\"AIShe could not serialize the error.\",\"retryable\":false,\"exit_code\":1,\"next_action\":\"Run `aishe doctor` and retry.\",\"detail\":null}}"
                    ),
                }
            } else {
                eprintln!("{}", public.render_text());
            }
            ExitCode::from(public.exit_code())
        }
    }
}

fn run() -> Result<u8> {
    let args = Args::parse();
    aishe::ui::set_machine_output(args.machine_output());

    if matches!(args.cmd, Some(Cmd::BackendSupervisor)) {
        return aishe::backend::supervisor::run_supervisor();
    }
    if let Some(command) = args.record_failure.as_deref() {
        return aishe::failure::record_from_env(command);
    }
    if let Some(Cmd::Last { cmd }) = &args.cmd {
        match cmd {
            LastCmd::Show { json } => return aishe::failure::show(*json),
            LastCmd::Retry { execute } => return aishe::failure::retry(*execute),
            LastCmd::Clear => return aishe::failure::clear(),
            LastCmd::Explain | LastCmd::Fix => {}
        }
    }

    // Route inspection is deliberately resolved before config, policy,
    // provider, plugin, MCP, or managed-backend initialization. It uses only
    // deterministic grammar plus local PATH/builtin evidence.
    if let Some(Cmd::Route { json, line }) = &args.cmd {
        return aishe::cli::backend::route(line, *json);
    }

    // Ordinary one-shot shell commands must remain ordinary shell commands:
    // prove the route before loading config/policy/providers/plugins or touching
    // the managed backend. Ambiguous input falls through to the full classifier.
    if let Some(command) = args
        .command
        .as_deref()
        .and_then(dispatcher::fast_shell_line)
    {
        if args
            .command
            .as_deref()
            .is_some_and(|line| line.trim().starts_with('!'))
        {
            aishe::cli::runtime::print_forced_shell_cue();
        }
        let mut executor = Executor::new()?;
        executor.set_history_log(aishe::cli::history::fast_history_log()?);
        return Ok(executor.run(&command) as u8);
    }

    // Setup is deliberately handled before ordinary config loading: its job is
    // to create, repair, or verify the config without invoking a legacy wizard.
    if let Some(Cmd::Setup(setup)) = &args.cmd {
        let outcome = match aishe::setup::run(aishe::setup::Options {
            resume: setup.resume,
            restart: setup.restart,
            verify_only: setup.verify,
            non_interactive: setup.non_interactive,
            service: setup.service.clone(),
            base_url: setup.base_url.clone(),
            key_env: setup.key_env.clone(),
            credential_profile: setup.credential_profile.clone(),
            model: setup.model.clone(),
            transport: setup.transport.clone(),
            profile: setup
                .profile
                .as_deref()
                .and_then(aishe::profiles::Profile::parse),
            input_price: setup.input_price,
            output_price: setup.output_price,
            live: setup.live,
            backend: setup.backend.clone(),
            install_backend: setup.install_backend,
            runtime_file: setup.runtime_file.clone(),
            runtime_base_url: setup.runtime_base_url.clone(),
            sandbox: setup.sandbox.clone(),
            install_system_deps: setup.install_system_deps,
            default_scope: setup.default_scope.clone(),
            network: setup.network.clone(),
            output: setup.output.clone(),
            json: setup.json,
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                let code = aishe::setup::exit_code(&error);
                let message = aishe::redact::redact(&error.to_string());
                if setup.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "schema_version": 1,
                            "applied": false,
                            "exit_code": code,
                            "error": message,
                        }))?
                    );
                } else {
                    aishe::cli::error_contract::emit_classified(
                        aishe::user_error::ErrorNamespace::Config,
                        "setup_failed",
                        "AIShe setup could not complete.",
                        "Run `aishe setup --verify`; repair the reported item, then retry.",
                        Some(&message),
                    );
                }
                return Ok(code);
            }
        };
        return Ok(outcome.exit_code);
    }

    if let Some(Cmd::Settings { json }) = &args.cmd {
        if *json {
            let (_, provenance) = aishe::settings::provenance()?;
            aishe::cli::json_contract::print_object(&provenance)?;
        } else {
            aishe::settings::run()?;
        }
        return Ok(0);
    }

    // Credential management deliberately uses only user config. A project
    // overlay can never redirect a write to a different saved profile.
    if let Some(Cmd::Auth { cmd }) = &args.cmd {
        return aishe::auth::run(cmd);
    }

    if let Some(Cmd::Tour {
        restart,
        non_interactive,
    }) = &args.cmd
    {
        aishe::tour::run(aishe::tour::Options {
            restart: *restart,
            non_interactive: *non_interactive,
        })?;
        return Ok(0);
    }

    // `doctor` inspects the environment without loading/initializing config.
    if let Some(Cmd::Doctor {
        probe,
        live,
        json,
        fix,
        bundle,
    }) = &args.cmd
    {
        let report = aishe::diagnostics::inspect(
            VERSION,
            &aishe::diagnostics::Options {
                probe: *probe || *live,
                live: *live,
                fix: *fix,
            },
        );
        if let Some(path) = bundle {
            let config = Config::load_quiet().ok().flatten();
            aishe::diagnostics::write_bundle(path, &report, config.as_ref())?;
            if !*json {
                eprintln!("aishe: wrote redacted support bundle to {}", path.display());
            }
        }
        if *json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", aishe::diagnostics::render_text(&report));
        }
        return Ok(if report.critical_ok() { 0 } else { 1 });
    }

    if let Some(Cmd::Backend { cmd }) = &args.cmd {
        return aishe::cli::backend::command(&backend_action(cmd));
    }
    if let Some(Cmd::Update { cmd }) = &args.cmd {
        return match cmd {
            UpdateCmd::Check { json } => aishe::lifecycle::update_check(*json),
            UpdateCmd::Apply { yes } => aishe::lifecycle::update_apply(*yes),
            UpdateCmd::Rollback { yes } => aishe::lifecycle::update_rollback(*yes),
        };
    }

    // `completions <shell>` prints a completion script and exits.
    if let Some(Cmd::Completions { shell }) = args.cmd {
        use clap::CommandFactory;
        clap_complete::generate(shell, &mut Args::command(), "aishe", &mut std::io::stdout());
        return Ok(0);
    }

    // `man` prints a roff man page generated from the clap command tree.
    if matches!(args.cmd, Some(Cmd::Man)) {
        use clap::CommandFactory;
        let man = clap_mangen::Man::new(Args::command());
        let mut out = Vec::new();
        man.render(&mut out).ok();
        use std::io::Write;
        let _ = std::io::stdout().write_all(&out);
        return Ok(0);
    }

    if let Some(Cmd::Uninstall {
        binary,
        runtime,
        sessions,
        config,
        history,
        audit_undo,
        all,
        dry_run,
        yes,
    }) = &args.cmd
    {
        return aishe::cli::backend::uninstall(
            aishe::uninstall::Selection {
                binary: *binary || *all,
                runtime: *runtime || *all,
                sessions: *sessions || *all,
                config: *config || *all,
                history: *history || *all,
                audit_undo: *audit_undo || *all,
            },
            *dry_run,
            *yes,
        );
    }

    // `trust` / `untrust` manage the project-config trust store; no config load.
    if let Some(Cmd::Trust { list, path }) = &args.cmd {
        return Ok(aishe::cli::settings::trust(*list, path.as_deref()));
    }
    if let Some(Cmd::Untrust { all, path }) = &args.cmd {
        return Ok(aishe::cli::settings::untrust(*all, path.as_deref()));
    }

    if let Some(Cmd::Sessions { json }) = &args.cmd {
        return Ok(aishe::cli::session::list(*json));
    }
    if let Some(Cmd::Session { cmd }) = &args.cmd {
        return aishe::cli::session::command(&session_action(cmd));
    }

    // `undo` reverts AI file changes from the journal; no config or provider.
    if let Some(Cmd::Undo { list }) = &args.cmd {
        return Ok(aishe::cli::settings::undo(*list));
    }

    // `init <shell>` needs no config or provider.
    if let Some(Cmd::Init { shell }) = &args.cmd {
        return match integration::script(shell) {
            Some(s) => {
                print!("{s}");
                Ok(0)
            }
            None => {
                eprintln!(
                    "aishe: no integration for '{shell}' (supported: {})",
                    integration::SUPPORTED.join(", ")
                );
                Ok(1)
            }
        };
    }

    // Audit inspection/export is useful for recovery and support even before
    // provider setup. Load existing pricing/log preferences when available,
    // but never launch setup or materialize a default config as a side effect.
    if matches!(
        args.cmd,
        Some(Cmd::Log { .. } | Cmd::Usage { .. } | Cmd::Runbook { .. })
    ) {
        let mut config = Config::load_quiet()?.unwrap_or_default();
        let _project_overlay = std::env::current_dir()
            .ok()
            .and_then(|cwd| config.apply_project_overlay(&cwd));
        let _ = aishe::connection::apply_shell_selection(&mut config);
        config.apply_overrides(
            args.mode.as_deref(),
            args.provider.as_deref(),
            args.model.as_deref(),
        )?;
        aishe::cli::connection::apply_flag(
            &mut config,
            args.connection.as_deref(),
            args.model.as_deref(),
        )?;
        return match &args.cmd {
            Some(Cmd::Log {
                session,
                action,
                model,
                since,
                limit,
                json,
            }) => Ok(aishe::cli::history::log(
                &config,
                session.as_deref(),
                action.as_deref(),
                model.as_deref(),
                since.as_deref(),
                *limit,
                *json,
            )),
            Some(Cmd::Usage {
                by,
                since,
                connection,
            }) => Ok(aishe::cli::history::usage(
                &config,
                by.as_deref(),
                since.as_deref(),
                connection.as_deref(),
            )),
            Some(Cmd::Runbook {
                session,
                out,
                replay,
            }) => {
                aishe::cli::history::runbook(&config, session.as_deref(), out.as_deref(), *replay)
            }
            _ => unreachable!("matched audit command"),
        };
    }

    let mut config = Config::load_or_init()?;
    // A project-local `.aishe/config.toml` overrides the user config (safe keys
    // always; sensitive keys only when the file is trusted). Applied before flags
    // so precedence is: CLI flags > project overlay > user config > defaults.
    let project_overlay = std::env::current_dir()
        .ok()
        .and_then(|cwd| config.apply_project_overlay(&cwd));
    aishe::connection::apply_shell_selection(&mut config)?;
    // CLI flags win over the config file (which wins over compiled defaults).
    config.apply_overrides(
        args.mode.as_deref(),
        args.provider.as_deref(),
        args.model.as_deref(),
    )?;
    aishe::cli::connection::apply_flag(
        &mut config,
        args.connection.as_deref(),
        args.model.as_deref(),
    )?;
    let request_role = if args.edit_line.is_some()
        || args.suggest_line.is_some()
        || args.auto_line.is_some()
        || args.fix_line.is_some()
        || matches!(
            args.cmd,
            Some(Cmd::Suggest { .. }) | Some(Cmd::Last { cmd: LastCmd::Fix })
        ) {
        Some("compose")
    } else if matches!(
        args.cmd,
        Some(Cmd::Ask { .. })
            | Some(Cmd::Last {
                cmd: LastCmd::Explain
            })
    ) {
        Some("answer")
    } else if args.yolo_line.is_some() || args.background_task.is_some() {
        Some("build")
    } else {
        None
    };
    let active_role = aishe::roles::apply(
        &mut config,
        request_role,
        args.connection.is_some(),
        args.model.is_some(),
    )?;
    if let Some(role) = active_role {
        std::env::set_var("AISHE_ROLE", role);
    }
    // Administrator policy is the final, read-only constraint layer. It can
    // reduce authority or reject a provider/model, never inject credentials.
    aishe::policy::constrain(&mut config)?;
    let background_request = if let Some(id) = args.background_task.as_deref() {
        let (objective, budget) = aishe::background::request(id)?;
        config.aishe.max_yolo_iterations = config
            .aishe
            .max_yolo_iterations
            .min(budget.max_provider_turns);
        if budget.max_cost_usd > 0.0 {
            config.aishe.budget_usd = if config.aishe.budget_usd > 0.0 {
                config.aishe.budget_usd.min(budget.max_cost_usd)
            } else {
                budget.max_cost_usd
            };
        }
        aishe::background::arm_deadline(budget.max_minutes);
        Some((id.to_string(), objective))
    } else {
        None
    };
    aishe::ui::configure(&config.ui);
    if args.accept_yolo {
        aishe::cli::history::init_audit(&config);
        return match aishe::cli::runtime::ensure_yolo_acceptance(&config) {
            Ok(()) => Ok(0),
            Err(error) => {
                aishe::cli::error_contract::emit_from(error.as_ref());
                Ok(1)
            }
        };
    }

    // First-class inspection / settings subcommands. They print (or persist a
    // setting) and exit, so they work the same in the zsh-PTY, a bare shell, or a
    // script. The inspectors show the *effective* config (project overlay + flags
    // applied); the setters write to the user config file (see `set_or_show`).
    match &args.cmd {
        Some(Cmd::Config { effective, json }) => {
            if *effective {
                let (effective_config, provenance) = aishe::settings::provenance()?;
                if *json {
                    aishe::cli::json_contract::print_object(&serde_json::json!({
                        "config": effective_config,
                        "provenance": provenance,
                    }))?;
                } else {
                    aishe::settings::print_provenance(&provenance);
                }
            } else if *json {
                aishe::cli::json_contract::print_envelope("config", &config)?;
            } else {
                println!("config file: {}", Config::path().display());
                match toml::to_string_pretty(&config) {
                    Ok(t) => println!("\n{t}"),
                    Err(e) => eprintln!("aishe: {e}"),
                }
            }
            return Ok(0);
        }
        Some(Cmd::Mcp { cmd }) => {
            return match cmd {
                None => {
                    aishe::cli::runtime::print_mcp_listing(&aishe::mcp::McpRegistry::connect(
                        &config.mcp_servers,
                    ));
                    Ok(0)
                }
                Some(McpCmd::List { json }) => aishe::mcp_config::list(&config, *json),
                Some(McpCmd::Show { name, json }) => aishe::mcp_config::show(&config, name, *json),
                Some(McpCmd::Add { name, input }) => {
                    aishe::mcp_config::put(name, mcp_input(input), false, false)
                }
                Some(McpCmd::Edit { name, input }) => {
                    aishe::mcp_config::put(name, mcp_input(input), true, true)
                }
                Some(McpCmd::Remove { name }) => aishe::mcp_config::remove(name),
                Some(McpCmd::Enable { name }) => aishe::mcp_config::enable(name, true),
                Some(McpCmd::Disable { name }) => aishe::mcp_config::enable(name, false),
                Some(McpCmd::Test { name, json }) => aishe::mcp_config::test(&config, name, *json),
            };
        }
        Some(Cmd::Commands { topic }) => {
            aishe::cli::runtime::print_help_command(topic.as_deref());
            return Ok(0);
        }
        Some(Cmd::Palette { query, json }) => {
            return aishe::palette::command(&config, query.as_deref(), *json);
        }
        Some(Cmd::Status { json }) => return Ok(aishe::cli::status::command(&config, *json)),
        Some(Cmd::Hints { cmd }) => {
            let action = match cmd {
                HintsCmd::Status { json } => aishe::cli::hints::Action::Status { json: *json },
                HintsCmd::Reset => aishe::cli::hints::Action::Reset,
            };
            return aishe::cli::hints::command(&config, action);
        }
        Some(Cmd::Skills) => {
            let skills = SkillRegistry::load();
            if skills.is_empty() {
                println!(
                    "no skills (add <name>/SKILL.md files to {})",
                    aishe::skills::user_dir().unwrap_or_default().display()
                );
            } else {
                println!("model-invoked skills (yolo mode):");
                for (name, desc) in skills.list() {
                    println!("\x20 {name}  —  {desc}");
                }
            }
            aishe::cli::runtime::warn_untrusted_skills(&skills);
            return Ok(0);
        }
        Some(Cmd::Task { cmd }) => {
            return aishe::background::command(&config, background_task_action(cmd));
        }
        Some(Cmd::Role { cmd }) => {
            return match cmd {
                RoleCmd::List { json } => aishe::roles::list(&config, *json),
                RoleCmd::Set {
                    name,
                    connection,
                    model,
                    reasoning,
                } => aishe::roles::set(
                    name,
                    aishe::roles::RoleConfig {
                        connection: connection.clone(),
                        model: model.clone(),
                        reasoning: reasoning.clone(),
                    },
                ),
                RoleCmd::Remove { name } => aishe::roles::remove(name),
            };
        }
        Some(Cmd::Index {
            rebuild,
            status,
            query,
            limit,
            json,
        }) => {
            let cwd = std::env::current_dir()?;
            let action = if *status {
                aishe::repo_index::Action::Status { json: *json }
            } else if let Some(query) = query {
                aishe::repo_index::Action::Search {
                    query,
                    limit: *limit,
                    json: *json,
                }
            } else {
                aishe::repo_index::Action::Build {
                    rebuild: *rebuild,
                    json: *json,
                }
            };
            return aishe::repo_index::command(&cwd, action);
        }
        Some(Cmd::Mode { value }) => {
            return Ok(aishe::cli::connection::set_or_show(
                "mode",
                value.as_deref(),
                &config,
            ))
        }
        Some(Cmd::Scope { value }) => {
            return Ok(aishe::cli::connection::set_or_show(
                "scope",
                value.as_deref(),
                &config,
            ))
        }
        Some(Cmd::Network { value }) => {
            return Ok(aishe::cli::connection::set_or_show(
                "network",
                value.as_deref(),
                &config,
            ))
        }
        Some(Cmd::Output { value }) => {
            return Ok(aishe::cli::connection::set_or_show(
                "output",
                value.as_deref(),
                &config,
            ))
        }
        Some(Cmd::Reasoning { value, default }) => {
            return Ok(aishe::cli::connection::reasoning(
                &config,
                value.as_deref(),
                *default,
            ))
        }
        Some(Cmd::Model {
            value,
            connection,
            default,
        }) => {
            return Ok(aishe::cli::connection::model(
                &config,
                value.as_deref(),
                connection.as_deref(),
                *default,
            ))
        }
        Some(Cmd::Connection { cmd }) => {
            return aishe::cli::connection::command(&config, &connection_action(cmd))
        }
        Some(Cmd::Provider { value, live, json }) => {
            if value.as_deref() == Some("test") {
                let report = aishe::capabilities::validate(&config, *live);
                if *json {
                    println!("{}", serde_json::to_string_pretty(&report)?);
                } else {
                    aishe::cli::settings::print_capability_report(&report);
                }
                return Ok(
                    if report.credential.state == aishe::capabilities::State::Fail {
                        1
                    } else {
                        0
                    },
                );
            }
            if *live || *json {
                aishe::cli::error_contract::emit_classified(
                    aishe::user_error::ErrorNamespace::Cli,
                    "invalid_provider_flags",
                    "Provider validation flags require the `test` action.",
                    "Run `aishe provider test --live` or `aishe provider test --json`.",
                    None,
                );
                return Ok(1);
            }
            return Ok(aishe::cli::connection::set_or_show(
                "provider",
                value.as_deref(),
                &config,
            ));
        }
        Some(Cmd::Models {
            provider,
            connection,
            refresh,
            json,
        }) => {
            if *refresh {
                let _ = aishe::capabilities::clear();
            }
            return Ok(aishe::cli::settings::models(
                &config,
                connection
                    .as_deref()
                    .or(provider.as_deref())
                    .unwrap_or_else(|| config.active_connection_id()),
                *json,
            ));
        }
        Some(Cmd::Profile { action, path, yes }) => {
            return match action.as_deref() {
                Some("export") => aishe::lifecycle::profile_export(
                    path.as_deref().context("profile export requires PATH")?,
                ),
                Some("import") => aishe::lifecycle::profile_import(
                    path.as_deref().context("profile import requires PATH")?,
                    *yes,
                ),
                _ if path.is_some() || *yes => {
                    anyhow::bail!("profile PATH/--yes are valid only with export or import")
                }
                _ => Ok(aishe::cli::settings::profile(&config, action.as_deref())),
            };
        }
        Some(Cmd::Readiness { json }) => {
            let report = aishe::profiles::readiness(&config);
            if *json {
                aishe::cli::json_contract::print_object(&report)?;
            } else {
                println!(
                    "autonomous readiness: {}",
                    if report.ready { "ready" } else { "not ready" }
                );
                for check in report.checks {
                    println!(
                        "  {} {}: {}",
                        if check.ready {
                            "✓"
                        } else if check.required {
                            "✗"
                        } else {
                            "!"
                        },
                        check.id,
                        check.detail
                    );
                }
            }
            return Ok(if report.ready { 0 } else { 1 });
        }
        Some(Cmd::Price { cmd }) => {
            return Ok(aishe::cli::settings::price(&config, &price_action(cmd)))
        }
        Some(Cmd::Reset) => return aishe::cli::session::reset(&config),
        Some(Cmd::Resume { id, cwd }) => {
            return aishe::cli::session::resume(&config, id.as_deref(), cwd.as_deref())
        }
        Some(Cmd::Context {
            explain,
            preview,
            json,
            exclude,
            include,
        }) => {
            return aishe::cli::settings::context(
                config,
                *explain,
                preview.as_deref(),
                *json,
                exclude,
                include,
            );
        }
        Some(Cmd::History { cmd }) => {
            return aishe::cli::history::command(&config, &history_action(cmd));
        }
        Some(Cmd::DryRun { command, apply }) => {
            return aishe::cli::history::dry_run(command, *apply);
        }
        Some(
            Cmd::Setup(_)
            | Cmd::Settings { .. }
            | Cmd::Tour { .. }
            | Cmd::Sessions { .. }
            | Cmd::Session { .. }
            | Cmd::Log { .. }
            | Cmd::Usage { .. }
            | Cmd::Runbook { .. }
            | Cmd::Backend { .. }
            | Cmd::BackendSupervisor,
        ) => {
            unreachable!("handled before config load")
        }
        _ => {}
    }

    // Tell an interactive user what a project config did (and how to trust it).
    let interactive_entry = args.command.is_none()
        && args.suggest_line.is_none()
        && args.yolo_line.is_none()
        && args.auto_line.is_none()
        && args.fix_line.is_none()
        && args.edit_line.is_none()
        && args.background_task.is_none()
        && args.record_failure.is_none()
        && !args.accept_yolo
        && std::io::stdin().is_terminal();
    if interactive_entry {
        aishe::cli::settings::notify_project_overlay(&project_overlay);
    }

    // Initialize the audit log (off unless enabled in config or via $AISHE_LOG).
    aishe::cli::history::init_audit(&config);

    // Non-interactive invocations (`-c` and the shell-hook helpers) never use
    // the PTY front-end — they need the in-process executor/provider, not a
    // wrapped interactive zsh.
    let non_interactive = args.command.is_some()
        || args.suggest_line.is_some()
        || args.yolo_line.is_some()
        || args.auto_line.is_some()
        || args.fix_line.is_some()
        || args.edit_line.is_some()
        || args.background_task.is_some()
        || args.record_failure.is_some()
        || args.accept_yolo
        || matches!(args.cmd, Some(Cmd::Suggest { .. } | Cmd::Ask { .. }));

    // The interactive shell is the zsh-PTY front-end: it drives the user's real
    // zsh, with the AI injected via a command_not_found hook, so zsh is required.
    // Piped (non-tty) stdin with no `-c`: read commands from stdin instead of
    // launching the interactive shell. An explicit `aishe zsh` always launches it.
    let explicit_zsh = matches!(args.cmd, Some(Cmd::Zsh));
    let piped_stdin = !non_interactive && !explicit_zsh && !std::io::stdin().is_terminal();
    let want_pty = !non_interactive && !piped_stdin;

    if want_pty {
        if aishe::executor::which("zsh").is_none() {
            aishe::cli::error_contract::emit_classified(
                aishe::user_error::ErrorNamespace::Cli,
                "interactive_shell_missing",
                "The interactive AIShe shell requires zsh, but zsh is not on PATH.",
                "Install zsh, rerun AIShe, or use `aishe -c`; Bash users can evaluate `aishe init bash`.",
                None,
            );
            return Ok(1);
        }
        return aishe::pty::run_zsh(&config, &aishe::cli::history::history_paths(&config).1);
    }

    let mut executor = Executor::new()?;
    context::init(executor.shell());
    // The `history` builtin reads the timestamped log (also available in `-c`).
    executor.set_history_log(aishe::cli::history::history_paths(&config).1);

    let cache = CommandCache::new();
    cache.build(executor.shell());

    // Hidden hook invocations get one conservative local typo cue per command
    // head and live shell. Intercept before constructing anything capable of a
    // provider request, managed-backend start, tool execution, or MCP traffic.
    let hook_line = args
        .suggest_line
        .as_deref()
        .or(args.auto_line.as_deref())
        .or(args.yolo_line.as_deref());
    if let Some(line) = hook_line {
        if aishe::cli::runtime::intercept_hook_typo(line, &cache)? {
            return Ok(0);
        }
    }

    // Build the provider, but keep the shell fully usable without it. We do NOT
    // warn here: local commands shouldn't print LLM noise, and the NL paths
    // (REPL, -c, hooks) each report a missing provider at the point of use.
    let mut provider: Option<Arc<dyn Provider>> = providers::make(&config).ok();

    // Install a non-fatal SIGINT handler (see INTERRUPTED docs).
    aishe::cli::runtime::install_sigint_handler();

    // User-defined slash-commands and model-invoked skills (plugins).
    let commands = CommandRegistry::load();
    let skills = aishe::skills::SkillRegistry::load();
    // Deliberately NOT warning about untrusted project skills here: this runs
    // for every invocation, so it printed on plain shell pass-through
    // (`aishe -c 'free -m'`) in any repo carrying a skill file, polluting
    // stderr for commands that never consult a skill. The warning belongs where
    // skills are actually relevant — `aishe skills`, `aishe doctor`, and the
    // yolo loop that can invoke them.
    // MCP servers (extra yolo tools). Empty/instant unless `[mcp_servers]` is set.
    let mcp = aishe::mcp::McpRegistry::connect(&config.mcp_servers);

    if let Some((id, objective)) = background_request {
        let result = aishe::cli::runtime::one_shot(
            &format!("? {objective}"),
            &mut executor,
            &mut provider,
            &config,
            &cache,
            &commands,
            &skills,
            &mcp,
        );
        aishe::background::finish(&id, &result);
        return result;
    }

    // Public scripting interface: `aishe suggest "<nl>" [--json]`.
    if let Some(Cmd::Suggest { query, json }) = &args.cmd {
        let q = query.join(" ");
        let code = aishe::cli::runtime::suggest_command(
            &q,
            *json,
            &mut executor,
            provider.as_deref(),
            &config,
        )?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(Cmd::Ask {
        query,
        json,
        schema,
        insert,
    }) = &args.cmd
    {
        let code = if *insert {
            aishe::cli::runtime::ask_insert(
                &query.join(" "),
                &mut executor,
                provider.as_deref(),
                &config,
            )?
        } else {
            aishe::cli::runtime::ask_command(
                &query.join(" "),
                *json,
                schema.as_deref(),
                &executor,
                provider.as_deref(),
                &config,
            )?
        };
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(Cmd::Last { cmd }) = &args.cmd {
        let capsule = aishe::failure::current()?;
        let code = match cmd {
            LastCmd::Explain => aishe::cli::runtime::ask_command(
                &format!(
                    "Explain why this shell command failed with exit status {} and suggest safe next steps. Do not execute anything.\nCommand: {}",
                    capsule.exit_status, capsule.command
                ),
                false,
                None,
                &executor,
                provider.as_deref(),
                &config,
            )?,
            LastCmd::Fix => aishe::cli::runtime::fix_command(
                &capsule.command,
                &capsule.exit_status.to_string(),
                &mut executor,
                provider.as_deref(),
                &config,
            )?,
            _ => unreachable!("handled before config load"),
        };
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }

    // Shell-hook helpers (called by `aishe init` integration). Each is its own
    // process under the interactive PTY, so after it runs we append its metered
    // usage to the shared session tally (a no-op outside a PTY session) for the
    // one-line summary the PTY prints on exit.
    if let Some(line) = args.suggest_line {
        let code =
            aishe::cli::runtime::suggest_line(&line, &mut executor, provider.as_deref(), &config)?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(line) = args.yolo_line {
        let code = aishe::cli::runtime::yolo_line(
            &line,
            &mut executor,
            provider.as_deref(),
            &config,
            &skills,
            &mcp,
        )?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(line) = args.auto_line {
        let code =
            aishe::cli::runtime::auto_line(&line, &mut executor, provider.as_deref(), &config)?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(cmd) = args.fix_line {
        let code =
            aishe::cli::runtime::fix_line(&cmd, &mut executor, provider.as_deref(), &config)?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }
    if let Some(line) = args.edit_line {
        let code =
            aishe::cli::runtime::edit_line(&line, &mut executor, provider.as_deref(), &config)?;
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
        return Ok(code);
    }

    // Non-interactive single-shot mode (-c).
    if let Some(input) = args.command {
        return aishe::cli::runtime::one_shot(
            &input,
            &mut executor,
            &mut provider,
            &config,
            &cache,
            &commands,
            &skills,
            &mcp,
        );
    }

    // Pipe/script mode: run each line of piped stdin like a `-c` invocation.
    if piped_stdin {
        let mut last = 0u8;
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    last = aishe::cli::runtime::one_shot(
                        trimmed,
                        &mut executor,
                        &mut provider,
                        &config,
                        &cache,
                        &commands,
                        &skills,
                        &mcp,
                    )?;
                }
                Err(_) => break,
            }
        }
        return Ok(last);
    }

    // Every interactive session is handled by the zsh-PTY branch above, and every
    // non-interactive path (hooks, `-c`, piped stdin) returns before here.
    Ok(0)
}
