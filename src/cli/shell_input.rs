//! Conventional shell input admission, independent of AI initialization.

use std::ffi::OsString;
use std::io::IsTerminal;

use anyhow::{Context, Result};

use crate::config::Config;
use crate::dispatcher;

/// Parsed shell intent; the binary supplies only the facts needed for admission.
pub struct Options<'a> {
    pub subcommand: bool,
    pub hook: bool,
    pub interactive: bool,
    pub login: bool,
    pub stdin_script: bool,
    pub agent_lines: bool,
    pub ai_selection: bool,
    pub command: Option<&'a str>,
    pub shell_arguments: &'a [OsString],
}

/// Execute conventional input before providers and interactive setup are loaded.
/// `None` leaves an explicit AIShe invocation to the binary's normal dispatch.
pub fn run(options: Options<'_>) -> Result<Option<u8>> {
    let shell_options =
        options.interactive || options.login || options.stdin_script || options.agent_lines;
    if options.subcommand {
        if shell_options || !options.shell_arguments.is_empty() {
            return Err(crate::user_error::UserFacing::cli(
                "shell_options_with_subcommand",
                "Shell launch options cannot be combined with an AIShe subcommand.",
                "Use `aishe -i`, `aishe -l`, or the subcommand separately.",
            ));
        }
        return Ok(None);
    }
    if options.hook {
        if shell_options || !options.shell_arguments.is_empty() {
            return Err(crate::user_error::UserFacing::cli(
                "shell_options_with_hook",
                "Shell launch options cannot be combined with an AIShe hook invocation.",
                "Run the shell command and AIShe hook separately.",
            ));
        }
        return Ok(None);
    }
    if options.agent_lines {
        if !options.shell_arguments.is_empty() {
            return Err(crate::user_error::UserFacing::cli(
                "agent_lines_arguments",
                "The --agent-lines protocol does not accept a script filename or arguments.",
                "Pipe AIShe input lines to `aishe --agent-lines`.",
            ));
        }
        return Ok(None);
    }
    if options.interactive {
        if !options.shell_arguments.is_empty() {
            return Err(crate::user_error::UserFacing::cli(
                "interactive_script",
                "An interactive AIShe session does not accept a script filename.",
                "Use `aishe -i` for a session or `aishe SCRIPT` for a script.",
            ));
        }
        return Ok(None);
    }
    let script_command =
        options.command.is_some() && (options.login || !options.shell_arguments.is_empty());
    let script_file =
        options.command.is_none() && !options.stdin_script && !options.shell_arguments.is_empty();
    let script_stdin = options.command.is_none()
        && !script_file
        && (options.stdin_script || !std::io::stdin().is_terminal());
    if !script_command && !script_file && !script_stdin {
        return Ok(None);
    }
    if options.ai_selection {
        return Err(crate::user_error::UserFacing::cli(
            "ai_options_with_script",
            "AI selection flags do not apply to conventional shell scripts.",
            "Use `aishe -c LINE` or `aishe --agent-lines` for agent routing.",
        ));
    }
    // A saved Bash adoption choice also controls the script interpreter. A bad
    // AI configuration must not take away otherwise ordinary script execution.
    let profile = std::env::var("AISHE_ZSH_PROFILE")
        .ok()
        .filter(|profile| matches!(profile.as_str(), "clean" | "personal" | "bash"))
        .or_else(|| {
            Config::load_quiet()
                .ok()
                .flatten()
                .map(|config| config.aishe.shell_profile)
        });
    let shell = if profile.as_deref() == Some("bash") {
        crate::executor::which("bash").context("the saved Bash profile requires Bash on PATH")?
    } else {
        crate::executor::which("zsh")
            .or_else(|| crate::executor::which("bash"))
            .context("shell scripts require zsh or Bash on PATH")?
    };
    let mut command = std::process::Command::new(shell);
    if options.login {
        command.arg("-l");
    }
    if script_command {
        let line = options.command.expect("script command is present");
        if !options.login && dispatcher::fast_shell_line(line).is_none() {
            return Err(crate::user_error::UserFacing::cli(
                "agent_positional_arguments",
                "Positional shell arguments require an unambiguous shell command.",
                "Use `aishe -lc SCRIPT NAME ARG...` for conventional shell execution.",
            ));
        }
        let line = if options.login {
            line.to_owned()
        } else {
            dispatcher::fast_shell_line(line).expect("shell route was checked")
        };
        command.arg("-c").arg(line).args(options.shell_arguments);
    } else if script_file {
        let file = std::path::Path::new(&options.shell_arguments[0]);
        if !file.is_file() {
            return Err(crate::user_error::UserFacing::cli(
                "script_not_found",
                format!("Shell script does not exist: {}", file.display()),
                "Use `aishe --help` for commands, or provide an existing script filename.",
            ));
        }
        command.arg("--").args(options.shell_arguments);
    } else {
        command.arg("-s").arg("--").args(options.shell_arguments);
    }
    let status = command.status().context("cannot launch the script shell")?;
    use std::os::unix::process::ExitStatusExt;
    Ok(Some(
        status
            .code()
            .unwrap_or_else(|| 128 + status.signal().unwrap_or(1)) as u8,
    ))
}
