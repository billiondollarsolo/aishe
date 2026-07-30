//! Platform dependency discovery, transparent installation plans, and
//! functional verification.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BubblewrapState {
    Unsupported,
    Missing,
    InstalledButUnusable { reason: String },
    Usable { path: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InstallPlan {
    pub manager: String,
    pub program: String,
    pub args: Vec<String>,
    pub display: String,
    pub needs_privilege: bool,
}

pub fn bubblewrap_probe() -> BubblewrapState {
    if !cfg!(target_os = "linux") {
        return BubblewrapState::Unsupported;
    }
    let Some(binary) = crate::executor::which("bwrap") else {
        return BubblewrapState::Missing;
    };
    match functional_probe(&binary) {
        Ok(()) => BubblewrapState::Usable { path: binary },
        Err(error) => BubblewrapState::InstalledButUnusable {
            reason: crate::redact::redact(&error.to_string()),
        },
    }
}

fn functional_probe(binary: &Path) -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "aishe-bwrap-probe-{}-{}",
        std::process::id(),
        random_hex(6)
    ));
    std::fs::create_dir_all(&root)?;
    crate::config::set_private_dir(&root);
    let marker = ".aishe-probe-writable";
    let touch = crate::executor::which("touch")
        .context("touch is required for the bubblewrap functional test")?;
    let denied = PathBuf::from(format!(
        "/etc/.aishe-bwrap-must-not-write-{}-{}",
        std::process::id(),
        random_hex(6)
    ));
    // The denied write must run as an external command. A redirection failure
    // on the `:` special builtin is fatal in some /bin/sh implementations even
    // when it appears as an `if` condition, which makes a correctly read-only
    // /etc look like a broken sandbox.
    let script = "set -eu; : > \"$1\"; \
                  if \"$2\" \"$3\" 2>/dev/null; then exit 41; fi; \
                  test -r /etc/passwd";
    let output = Command::new(binary)
        .args([
            "--ro-bind",
            "/",
            "/",
            "--tmpfs",
            "/tmp",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--bind",
        ])
        .arg(&root)
        .arg(&root)
        .args(["--chdir"])
        .arg(&root)
        .args([
            "--unshare-net",
            "--die-with-parent",
            "--",
            "/bin/sh",
            "-c",
            script,
            "aishe-bwrap-probe",
            marker,
        ])
        .arg(&touch)
        .arg(&denied)
        .stdin(Stdio::null())
        .output();
    let result = match output {
        Ok(output)
            if output.status.success()
                && root.join(marker).is_file()
                && !denied.try_exists().unwrap_or(true) =>
        {
            Ok(())
        }
        Ok(output) => anyhow::bail!(
            "bubblewrap functional test failed ({}): {}",
            output.status,
            crate::commands::display_safe(String::from_utf8_lossy(&output.stderr).trim())
        ),
        Err(error) => Err(error).context("starting bubblewrap functional test"),
    };
    let _ = std::fs::remove_file(&denied);
    let _ = std::fs::remove_dir_all(&root);
    result
}

pub fn bubblewrap_install_plan() -> Result<InstallPlan> {
    package_install_plan("bubblewrap")
}

pub fn zsh_install_plan() -> Result<InstallPlan> {
    package_install_plan("zsh")
}

fn package_install_plan(package: &str) -> Result<InstallPlan> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("{package} package installation is supported only on Linux");
    }
    let manager = detect_package_manager().context(
        "no supported package manager found (apt-get, dnf, yum, zypper, pacman, or apk)",
    )?;
    let binary = crate::executor::which(&manager)
        .with_context(|| format!("{manager} disappeared while constructing its install plan"))?;
    let is_root = unsafe { libc::geteuid() } == 0;
    let sudo = if is_root {
        None
    } else {
        Some(crate::executor::which("sudo").context(format!(
            "{package} installation needs administrator access but sudo is unavailable"
        ))?)
    };
    build_package_install_plan(package, &manager, binary, sudo, is_root)
}

fn build_package_install_plan(
    package: &str,
    manager: &str,
    binary: PathBuf,
    sudo: Option<PathBuf>,
    is_root: bool,
) -> Result<InstallPlan> {
    let (manager_name, mut package_args): (&str, Vec<String>) = match manager {
        "apt-get" => ("apt", vec!["install".into(), "-y".into(), package.into()]),
        "dnf" => ("dnf", vec!["install".into(), "-y".into(), package.into()]),
        "yum" => ("yum", vec!["install".into(), "-y".into(), package.into()]),
        "zypper" => (
            "zypper",
            vec!["--non-interactive".into(), "install".into(), package.into()],
        ),
        "pacman" => (
            "pacman",
            vec!["-S".into(), "--noconfirm".into(), package.into()],
        ),
        "apk" => ("apk", vec!["add".into(), package.into()]),
        _ => anyhow::bail!("unsupported package manager '{manager}'"),
    };
    let mut args = Vec::new();
    let (program, needs_privilege) = if is_root {
        (binary.display().to_string(), false)
    } else {
        let sudo = sudo.context(format!(
            "{package} installation needs administrator access but sudo is unavailable"
        ))?;
        args.push(binary.display().to_string());
        (sudo.display().to_string(), true)
    };
    args.append(&mut package_args);
    let display = std::iter::once(program.as_str())
        .chain(args.iter().map(String::as_str))
        .map(shell_display_arg)
        .collect::<Vec<_>>()
        .join(" ");
    Ok(InstallPlan {
        manager: manager_name.into(),
        program,
        args,
        display,
        needs_privilege,
    })
}

pub fn install_bubblewrap(plan: &InstallPlan, consent: bool) -> Result<BubblewrapState> {
    run_install_plan(plan, consent, "bubblewrap")?;
    let state = bubblewrap_probe();
    if !matches!(state, BubblewrapState::Usable { .. }) {
        anyhow::bail!("bubblewrap installed but its functional self-test failed");
    }
    Ok(state)
}

pub fn install_zsh(plan: &InstallPlan, consent: bool) -> Result<PathBuf> {
    run_install_plan(plan, consent, "zsh")?;
    let binary = crate::executor::which("zsh").context("zsh installed but is not on PATH")?;
    let output = Command::new(&binary)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .context("running zsh --version")?;
    if !output.status.success() {
        anyhow::bail!("zsh installed but its version self-test failed");
    }
    Ok(binary)
}

fn run_install_plan(plan: &InstallPlan, consent: bool, label: &str) -> Result<()> {
    if !consent {
        anyhow::bail!("{label} installation was not authorized");
    }
    let status = Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("running {}", plan.display))?;
    if !status.success() {
        anyhow::bail!("{label} package installation failed with {status}");
    }
    Ok(())
}

fn detect_package_manager() -> Option<String> {
    let os_release = std::fs::read_to_string("/etc/os-release")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let preferred: &[&str] = if os_release.contains("id=alpine") {
        &["apk", "apt-get", "dnf", "yum", "zypper", "pacman"]
    } else if os_release.contains("id=arch") || os_release.contains("id_like=arch") {
        &["pacman", "apt-get", "dnf", "yum", "zypper", "apk"]
    } else if os_release.contains("id_like=suse") || os_release.contains("id=opensuse") {
        &["zypper", "dnf", "yum", "apt-get", "pacman", "apk"]
    } else if os_release.contains("id_like=\"rhel")
        || os_release.contains("id_like=rhel")
        || os_release.contains("id=fedora")
    {
        &["dnf", "yum", "apt-get", "zypper", "pacman", "apk"]
    } else {
        &["apt-get", "dnf", "yum", "zypper", "pacman", "apk"]
    };
    preferred
        .iter()
        .find(|name| crate::executor::which(name).is_some())
        .map(|name| (*name).to_string())
}

fn shell_display_arg(value: &str) -> String {
    if value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_quoting_is_inert() {
        assert_eq!(shell_display_arg("/usr/bin/apt-get"), "/usr/bin/apt-get");
        assert_eq!(shell_display_arg("a b"), "'a b'");
        assert_eq!(shell_display_arg("a'b"), "'a'\\''b'");
    }

    #[test]
    fn non_linux_probe_is_explicit() {
        if !cfg!(target_os = "linux") {
            assert_eq!(bubblewrap_probe(), BubblewrapState::Unsupported);
        }
    }

    #[test]
    fn installed_bubblewrap_is_reported_truthfully() {
        if cfg!(target_os = "linux") && crate::executor::which("bwrap").is_some() {
            match bubblewrap_probe() {
                BubblewrapState::Usable { path } => assert!(path.is_absolute()),
                BubblewrapState::InstalledButUnusable { reason } => {
                    assert!(!reason.trim().is_empty())
                }
                state => panic!("installed bubblewrap was misclassified as {state:?}"),
            }
        }
    }

    #[test]
    fn package_manager_plans_use_exact_argv_without_a_shell() {
        let cases = [
            ("apt-get", &["install", "-y", "bubblewrap"][..]),
            ("dnf", &["install", "-y", "bubblewrap"]),
            ("yum", &["install", "-y", "bubblewrap"]),
            ("zypper", &["--non-interactive", "install", "bubblewrap"]),
            ("pacman", &["-S", "--noconfirm", "bubblewrap"]),
            ("apk", &["add", "bubblewrap"]),
        ];
        for (manager, expected) in cases {
            let binary = PathBuf::from(format!("/private/test/{manager}"));
            let root =
                build_package_install_plan("bubblewrap", manager, binary.clone(), None, true)
                    .unwrap();
            assert_eq!(root.program, binary.display().to_string());
            assert_eq!(root.args, expected);
            assert!(!root.needs_privilege);

            let user = build_package_install_plan(
                "bubblewrap",
                manager,
                binary.clone(),
                Some(PathBuf::from("/private/test/sudo")),
                false,
            )
            .unwrap();
            assert_eq!(user.program, "/private/test/sudo");
            assert_eq!(
                user.args.first().map(String::as_str),
                Some(binary.to_str().unwrap())
            );
            assert_eq!(
                &user.args[1..],
                expected,
                "package argv changed behind sudo for {manager}"
            );
            assert!(user.needs_privilege);
        }
    }

    #[test]
    fn package_install_execution_requires_explicit_consent() {
        let plan = InstallPlan {
            manager: "test".into(),
            program: "/path/that/must/not/be/executed".into(),
            args: vec!["install".into()],
            display: "test install".into(),
            needs_privilege: true,
        };
        let error = run_install_plan(&plan, false, "bubblewrap").unwrap_err();
        assert!(error.to_string().contains("was not authorized"));
    }
}
