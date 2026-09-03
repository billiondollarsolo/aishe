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
    // Rust starts with SIGPIPE ignored, so `aishe log | head` panicked on the
    // closed pipe. Restore the Unix default: the process ends quietly (141).
    #[cfg(unix)]
    // SAFETY: setting a signal disposition before any thread is spawned.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
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

    if let Some(values) = args.hook_cli.as_deref() {
        return aishe::integration::dispatch_hook_cli(&values[0], &values[1]);
    }

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
            launch_follows: false,
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
        if !setup.json {
            let path_check = aishe::diagnostics::path_binary_check();
            if path_check.status == aishe::diagnostics::Status::Warn {
                eprintln!("aishe: {}", path_check.summary);
                eprintln!("  {}", path_check.detail);
            }
        }
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

    if let Some(
        Cmd::Tour {
            restart,
            non_interactive,
        }
        | Cmd::Demo {
            restart,
            non_interactive,
        },
    ) = &args.cmd
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
            // Unreachable: clap validates the shell name. Kept as a guard so
            // adding a value_parser entry without an asset fails loudly.
            None => {
                aishe::cli::error_contract::emit_classified(
                    aishe::user_error::ErrorNamespace::Cli,
                    "unsupported_shell",
                    format!("No shell integration for '{shell}'."),
                    "Run `aishe init zsh` or `aishe init bash`.",
                    None,
                );
                Ok(aishe::user_error::ErrorNamespace::Cli.exit_code())
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
    let agent_request = match &args.cmd {
        Some(Cmd::Agent(options)) => match resolve_agent(options, &config)? {
            Some(request) => Some(request),
            None => return Ok(0),
        },
        _ => None,
    };
    let background_role = args
        .background_task
        .as_ref()
        .and_then(|_| std::env::var("AISHE_TASK_ROLE").ok())
        .filter(|role| aishe::roles::NAMES.contains(&role.as_str()));
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
    } else if let Some(request) = &agent_request {
        Some(request.role.as_str())
    } else if args.yolo_line.is_some()
        || matches!(
            &args.cmd,
            Some(Cmd::Task {
                cmd: BackgroundTaskCmd::Start { .. }
            })
        )
    {
        Some("build")
    } else if args.background_task.is_some() {
        background_role.as_deref().or(Some("build"))
    } else {
        None
    };
    let active_role = aishe::roles::apply(
        &mut config,
        request_role,
        args.connection.is_some()
            || agent_request
                .as_ref()
                .is_some_and(|request| request.connection.is_some()),
        args.model.is_some()
            || agent_request
                .as_ref()
                .is_some_and(|request| request.model.is_some()),
    )?;
    if let Some(role) = active_role {
        std::env::set_var("AISHE_ROLE", role);
    }
    if let Some(request) = &agent_request {
        aishe::cli::connection::apply_flag(
            &mut config,
            request.connection.as_deref(),
            request.model.as_deref(),
        )?;
        config.backend.default_scope.clone_from(&request.scope);
        config.aishe.mode = "yolo".into();
        if let Some(cap) = request.max_cost {
            config.aishe.budget_usd = if config.aishe.budget_usd > 0.0 {
                config.aishe.budget_usd.min(cap)
            } else {
                cap
            };
        }
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
        if let Ok(scope) = std::env::var("AISHE_TASK_SCOPE") {
            if matches!(scope.as_str(), "workspace" | "host") {
                config.backend.default_scope = scope;
            }
        }
        Some((id.to_string(), objective))
    } else {
        None
    };
    aishe::ui::configure(&config.ui);
    if args.accept_yolo {
        aishe::cli::history::init_audit(&config);
        return match aishe::cli::runtime::ensure_yolo_acceptance(&config) {
            Ok(aishe::cli::runtime::YoloAcceptance::Accepted) => Ok(0),
            Ok(aishe::cli::runtime::YoloAcceptance::Declined) => Ok(1),
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
            return Ok(aishe::cli::runtime::print_help_command(topic.as_deref()));
        }
        Some(Cmd::Palette { query, json }) => {
            return aishe::palette::command(&config, query.as_deref(), *json);
        }
        Some(Cmd::Capabilities { json }) => {
            return aishe::cli::settings::capabilities(&config, *json);
        }
        Some(Cmd::Test { live, json }) => {
            return aishe::cli::settings::self_test(&config, *live, *json);
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
                    println!("  {name}  —  {desc}");
                }
            }
            aishe::cli::runtime::warn_untrusted_skills(&skills);
            return Ok(0);
        }
        Some(Cmd::Task { cmd }) => {
            return aishe::background::command(&config, background_task_action(cmd));
        }
        Some(Cmd::Inbox { json }) => return aishe::background::inbox(&config, *json),
        Some(Cmd::Plan { id }) => return aishe::background::edit_plan(id.as_deref(), false),
        Some(Cmd::Replan { id }) => return aishe::background::edit_plan(id.as_deref(), true),
        Some(Cmd::Sessions { json }) => return aishe::cli::session::browse(&config, *json),
        Some(Cmd::Session { cmd }) => {
            return aishe::cli::session::command(&config, &session_action(cmd));
        }
        Some(Cmd::Agent(_))
            if agent_request
                .as_ref()
                .is_some_and(|request| request.background) =>
        {
            let request = agent_request.as_ref().expect("resolved agent request");
            return aishe::background::command(
                &config,
                aishe::background::Action::Start {
                    objective: request.objective.clone(),
                    no_isolation: request.no_isolation,
                    max_minutes: request.max_minutes,
                    max_turns: request.max_turns,
                    max_cost: request.max_cost,
                    max_tool_calls: 200,
                    max_changed_files: 100,
                    max_changed_bytes: 10_485_760,
                    max_network_calls: 50,
                },
            );
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
        Some(Cmd::Mode { value, default }) => {
            return Ok(aishe::cli::connection::mode(
                &config,
                value.as_deref(),
                *default,
            ));
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
        Some(Cmd::Provider { value }) => {
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
            show,
        }) => {
            return aishe::cli::settings::context(
                config,
                *explain,
                preview.as_deref(),
                *json,
                exclude,
                include,
                *show,
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
            | Cmd::Demo { .. }
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
        || matches!(
            args.cmd,
            Some(Cmd::Suggest { .. } | Cmd::Ask { .. } | Cmd::Agent(_))
        );

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
            return Ok(aishe::user_error::ErrorNamespace::Cli.exit_code());
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

    if matches!(args.cmd, Some(Cmd::Agent(_))) {
        let request = agent_request
            .as_ref()
            .context("agent request was not resolved")?;
        let result = aishe::cli::runtime::one_shot(
            &format!("? {}", request.objective),
            &mut executor,
            &mut provider,
            &config,
            &cache,
            &commands,
            &skills,
            &mcp,
        );
        aishe::cli::status::record_session_usage(provider.as_deref(), &config);
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

#[derive(Clone, Debug)]
struct ResolvedAgent {
    objective: String,
    background: bool,
    role: String,
    connection: Option<String>,
    model: Option<String>,
    scope: String,
    no_isolation: bool,
    max_minutes: u32,
    max_turns: u32,
    max_cost: Option<f64>,
}

fn resolve_agent(options: &AgentArgs, config: &Config) -> Result<Option<ResolvedAgent>> {
    let guided = options.objective.is_empty();
    let objective = if guided {
        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            anyhow::bail!("agent objective is required outside an interactive terminal");
        }
        aishe::promptui::header(
            "launch an AIShe agent",
            "Choose the work, authority, model role, and execution style in one place.",
            "Workspace scope and isolated background worktrees are the safe defaults.",
        );
        let Some(value) = aishe::promptui::text(
            "Objective",
            "inspect this repository and recommend the next improvement",
            |value| {
                if value.trim().is_empty() || value.len() > 64 * 1024 {
                    anyhow::bail!("objective must contain 1..=65536 bytes")
                }
                Ok(())
            },
        )?
        else {
            return Ok(None);
        };
        if value == ":back" {
            return Ok(None);
        }
        value
    } else {
        options.objective.join(" ")
    };
    let background = if guided {
        let choices = vec![
            "Foreground · stream progress in this terminal".into(),
            "Background · isolated git worktree and inbox".into(),
        ];
        let aishe::promptui::PickerResult::Use(index) =
            aishe::promptui::filter_picker("Execution", &choices, usize::from(options.background))?
        else {
            return Ok(None);
        };
        index == 1
    } else {
        options.background
    };
    let role = if guided && options.role.is_none() {
        let choices = aishe::roles::NAMES
            .iter()
            .map(|role| format!("{role} · workload-specific connection/model/reasoning"))
            .collect::<Vec<_>>();
        let default = aishe::roles::NAMES
            .iter()
            .position(|role| *role == "build")
            .unwrap_or(0);
        let aishe::promptui::PickerResult::Use(index) =
            aishe::promptui::filter_picker("Model role", &choices, default)?
        else {
            return Ok(None);
        };
        aishe::roles::NAMES[index].to_string()
    } else {
        options.role.clone().unwrap_or_else(|| "build".into())
    };
    let scope = if guided && options.scope.is_none() {
        let choices = vec![
            "workspace · project-bound authority".into(),
            "host · explicit whole-machine authority".into(),
        ];
        let default = usize::from(config.backend.default_scope == "host");
        let aishe::promptui::PickerResult::Use(index) =
            aishe::promptui::filter_picker("Authority", &choices, default)?
        else {
            return Ok(None);
        };
        if index == 1 {
            "host".into()
        } else {
            "workspace".into()
        }
    } else {
        options
            .scope
            .clone()
            .unwrap_or_else(|| config.backend.default_scope.clone())
    };
    if options
        .max_cost
        .is_some_and(|value| !value.is_finite() || value < 0.0)
    {
        anyhow::bail!("--max-cost must be a finite non-negative number");
    }
    let mut objective = objective.trim().to_string();
    for path in &options.file {
        objective.push(' ');
        objective.push_str(&attachment_reference("file", path)?);
    }
    for path in &options.dir {
        objective.push(' ');
        objective.push_str(&attachment_reference("dir", path)?);
    }
    if options.diff {
        objective.push_str(" @diff");
    }
    if options.clipboard {
        objective.push_str(" @clipboard");
    }
    Ok(Some(ResolvedAgent {
        objective,
        background,
        role,
        connection: options.connection.clone(),
        model: options.model.clone(),
        scope,
        no_isolation: options.no_isolation,
        max_minutes: options.max_minutes,
        max_turns: options.max_turns,
        max_cost: options.max_cost,
    }))
}

fn attachment_reference(kind: &str, path: &std::path::Path) -> Result<String> {
    let value = path.to_str().context("attachment path is not UTF-8")?;
    if value.is_empty() || value.chars().any(char::is_control) {
        anyhow::bail!("attachment path is empty or contains control characters");
    }
    if !value.contains('"') {
        Ok(format!("@{kind}:\"{value}\""))
    } else if !value.contains('\'') {
        Ok(format!("@{kind}:'{value}'"))
    } else {
        anyhow::bail!("attachment paths containing both quote styles are not supported")
    }
}
