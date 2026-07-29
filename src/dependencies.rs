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
    let script = format!(
        "set -eu; : > {marker}; \
         if : > /etc/.aishe-bwrap-must-not-write 2>/dev/null; then exit 41; fi; \
         test -r /etc/passwd"
    );
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
            &script,
        ])
        .stdin(Stdio::null())
        .output();
    let result = match output {
        Ok(output) if output.status.success() && root.join(marker).is_file() => Ok(()),
        Ok(output) => anyhow::bail!(
            "bubblewrap functional test failed ({}): {}",
            output.status,
            crate::commands::display_safe(String::from_utf8_lossy(&output.stderr).trim())
        ),
        Err(error) => Err(error).context("starting bubblewrap functional test"),
    };
    let _ = std::fs::remove_dir_all(&root);
    result
}

pub fn bubblewrap_install_plan() -> Result<InstallPlan> {
    if !cfg!(target_os = "linux") {
        anyhow::bail!("bubblewrap package installation is supported only on Linux");
    }
    let manager = detect_package_manager().context(
        "no supported package manager found (apt-get, dnf, yum, zypper, pacman, or apk)",
    )?;
    let is_root = unsafe { libc::geteuid() } == 0;
    let (manager_name, binary, package_args): (&str, PathBuf, &[&str]) = match manager.as_str() {
        "apt-get" => (
            "apt",
            crate::executor::which("apt-get").unwrap(),
            &["install", "-y", "bubblewrap"],
        ),
        "dnf" => (
            "dnf",
            crate::executor::which("dnf").unwrap(),
            &["install", "-y", "bubblewrap"],
        ),
        "yum" => (
            "yum",
            crate::executor::which("yum").unwrap(),
            &["install", "-y", "bubblewrap"],
        ),
        "zypper" => (
            "zypper",
            crate::executor::which("zypper").unwrap(),
            &["--non-interactive", "install", "bubblewrap"],
        ),
        "pacman" => (
            "pacman",
            crate::executor::which("pacman").unwrap(),
            &["-S", "--noconfirm", "bubblewrap"],
        ),
        "apk" => (
            "apk",
            crate::executor::which("apk").unwrap(),
            &["add", "bubblewrap"],
        ),
        _ => unreachable!("package-manager detector returned an unknown manager"),
    };
    let mut args = Vec::new();
    let (program, needs_privilege) = if is_root {
        (binary.display().to_string(), false)
    } else {
        let sudo = crate::executor::which("sudo").context(
            "bubblewrap installation needs administrator access but sudo is unavailable",
        )?;
        args.push(binary.display().to_string());
        (sudo.display().to_string(), true)
    };
    args.extend(package_args.iter().map(|value| value.to_string()));
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
    if !consent {
        anyhow::bail!("bubblewrap installation was not authorized");
    }
    let status = Command::new(&plan.program)
        .args(&plan.args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .with_context(|| format!("running {}", plan.display))?;
    if !status.success() {
        anyhow::bail!("bubblewrap package installation failed with {status}");
    }
    let state = bubblewrap_probe();
    if !matches!(state, BubblewrapState::Usable { .. }) {
        anyhow::bail!("bubblewrap installed but its functional self-test failed");
    }
    Ok(state)
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
}
