//! Verified binary update/rollback and secret-free profile portability.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use serde::Serialize;
use sha2::{Digest, Sha256};

const DOWNLOAD_LIMIT: u64 = 100 * 1024 * 1024;
const PROFILE_LIMIT: u64 = 4 * 1024 * 1024;

#[derive(Serialize)]
struct UpdateCheck {
    schema_version: u32,
    current: String,
    latest: String,
    update_available: bool,
    target: String,
}

pub fn update_check(json: bool) -> Result<u8> {
    let latest = latest_version()?;
    let update_available = version_is_newer(&latest, env!("CARGO_PKG_VERSION"))?;
    let report = UpdateCheck {
        schema_version: 1,
        current: env!("CARGO_PKG_VERSION").into(),
        update_available,
        latest,
        target: target()?.into(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if report.update_available {
        println!(
            "AIShe {} is available (current {})",
            report.latest, report.current
        );
    } else {
        println!("AIShe {} is current", report.current);
    }
    Ok(0)
}

pub fn update_apply(yes: bool) -> Result<u8> {
    let latest = latest_version()?;
    let current = std::env::current_exe()?.canonicalize()?;
    println!("update: {} -> {}", env!("CARGO_PKG_VERSION"), latest);
    println!("binary: {}", current.display());
    if !version_is_newer(&latest, env!("CARGO_PKG_VERSION"))? {
        if latest.trim_start_matches('v') != env!("CARGO_PKG_VERSION") {
            anyhow::bail!(
                "refusing to replace AIShe with older release {latest}; use rollback for recovery"
            );
        }
        println!("already current");
        return Ok(0);
    }
    confirm("replace this binary after checksum and self-test", yes)?;
    let target = target()?;
    let name = format!("aishe-{target}.tar.gz");
    let base = std::env::var("AISHE_RELEASE_BASE_URL")
        .unwrap_or_else(|_| "https://github.com/billiondollarsolo/aishe/releases".into());
    let url = format!(
        "{}/download/{}/{}",
        base.trim_end_matches('/'),
        latest,
        name
    );
    let archive = download(&url, DOWNLOAD_LIMIT)?;
    let checksum = String::from_utf8(download(&format!("{url}.sha256"), 4096)?)?;
    let expected = checksum.split_whitespace().next().unwrap_or("");
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("release checksum is malformed");
    }
    let actual = format!("{:x}", Sha256::digest(&archive));
    if !actual.eq_ignore_ascii_case(expected) {
        anyhow::bail!("release checksum mismatch; nothing was replaced");
    }
    let binary = extract_binary(&archive)?;
    verify_format(&binary)?;
    activate(&current, &binary, &latest)?;
    println!("updated AIShe to {latest}; rollback with `aishe update rollback`");
    Ok(0)
}

pub fn update_rollback(yes: bool) -> Result<u8> {
    let current = std::env::current_exe()?.canonicalize()?;
    let backup = update_root()?.join("previous-aishe");
    if !backup.is_file() {
        anyhow::bail!("no verified previous AIShe binary is available");
    }
    println!("rollback target: {}", current.display());
    confirm("restore the previous verified binary", yes)?;
    let prior = std::fs::read(&backup)?;
    verify_format(&prior)?;
    let current_bytes = std::fs::read(&current)?;
    replace_binary(&current, &prior)?;
    write_executable(&backup, &current_bytes)?;
    println!("restored the previous AIShe binary");
    Ok(0)
}

pub fn profile_export(path: &Path) -> Result<u8> {
    let mut config = crate::config::Config::load_quiet()?.unwrap_or_default();
    for server in config.mcp_servers.values_mut() {
        server.env.retain(|_, value| value.starts_with("env:"));
        server.headers.retain(|_, value| value.starts_with("env:"));
    }
    let text = format!(
        "# AIShe non-secret profile; credential values are intentionally excluded.\n{}",
        toml::to_string_pretty(&config)?
    );
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    crate::config::write_atomic(path, text.as_bytes())?;
    private(path)?;
    println!("exported non-secret profile to {}", path.display());
    Ok(0)
}

pub fn profile_import(path: &Path, yes: bool) -> Result<u8> {
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() > PROFILE_LIMIT {
        anyhow::bail!("profile must be a regular file no larger than 4 MiB");
    }
    let text = std::fs::read_to_string(path)?;
    let config: crate::config::Config = toml::from_str(&text).context("invalid AIShe profile")?;
    for server in config.mcp_servers.values() {
        if server
            .env
            .values()
            .chain(server.headers.values())
            .any(|value| !value.starts_with("env:"))
        {
            anyhow::bail!("profile contains literal MCP secret material; import refused");
        }
    }
    let destination = crate::config::Config::path();
    println!("profile: {} -> {}", path.display(), destination.display());
    println!(
        "connections: {} · MCP servers: {} · credentials: preserved separately",
        config.connections.len(),
        config.mcp_servers.len()
    );
    confirm("replace the non-secret configuration", yes)?;
    if destination.exists() {
        let backup = destination.with_extension("toml.before-profile-import");
        crate::config::write_atomic(&backup, &std::fs::read(&destination)?)?;
        private(&backup)?;
        println!("backup: {}", backup.display());
    }
    config.save()?;
    println!("imported profile; run `aishe doctor`");
    Ok(0)
}

fn latest_version() -> Result<String> {
    if let Ok(value) = std::env::var("AISHE_UPDATE_VERSION") {
        if valid_version(&value) {
            return Ok(if value.starts_with('v') {
                value
            } else {
                format!("v{value}")
            });
        }
        anyhow::bail!("AISHE_UPDATE_VERSION is invalid");
    }
    let bytes = download(
        "https://api.github.com/repos/billiondollarsolo/aishe/releases/latest",
        1024 * 1024,
    )?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let tag = value["tag_name"]
        .as_str()
        .context("latest release has no tag_name")?;
    if !valid_version(tag) {
        anyhow::bail!("latest release tag is invalid");
    }
    Ok(tag.to_string())
}

fn valid_version(value: &str) -> bool {
    parse_version(value).is_some()
}

fn parse_version(value: &str) -> Option<(u64, u64, u64, Option<&str>)> {
    let value = value.strip_prefix('v').unwrap_or(value);
    if value.is_empty()
        || value.len() > 40
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    let (core, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(core, suffix)| (core, Some(suffix)));
    if prerelease
        .is_some_and(|suffix| suffix.is_empty() || suffix.split('.').any(|part| part.is_empty()))
    {
        return None;
    }
    let mut numbers = core.split('.');
    let parsed = (
        numbers.next()?.parse().ok()?,
        numbers.next()?.parse().ok()?,
        numbers.next()?.parse().ok()?,
        prerelease,
    );
    numbers.next().is_none().then_some(parsed)
}

fn version_is_newer(candidate: &str, current: &str) -> Result<bool> {
    let candidate = parse_version(candidate).context("release version is invalid")?;
    let current = parse_version(current).context("current AIShe version is invalid")?;
    let numeric = (candidate.0, candidate.1, candidate.2).cmp(&(current.0, current.1, current.2));
    Ok(match numeric {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => match (candidate.3, current.3) {
            (None, Some(_)) => true,
            (Some(new), Some(old)) => prerelease_cmp(new, old).is_gt(),
            _ => false,
        },
    })
}

fn prerelease_cmp(left: &str, right: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) => {
                let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                    (Ok(a), Ok(b)) => a.cmp(&b),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => a.cmp(b),
                };
                if !order.is_eq() {
                    return order;
                }
            }
        }
    }
}

fn target() -> Result<&'static str> {
    match (
        std::env::consts::ARCH,
        std::env::consts::OS,
        cfg!(target_env = "musl"),
    ) {
        ("x86_64", "linux", false) => Ok("x86_64-unknown-linux-gnu"),
        ("x86_64", "linux", true) => Ok("x86_64-unknown-linux-musl"),
        ("aarch64", "linux", false) => Ok("aarch64-unknown-linux-gnu"),
        ("aarch64", "linux", true) => Ok("aarch64-unknown-linux-musl"),
        ("x86_64", "macos", _) => Ok("x86_64-apple-darwin"),
        ("aarch64", "macos", _) => Ok("aarch64-apple-darwin"),
        _ => anyhow::bail!("no prebuilt AIShe update for this platform"),
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(url)?;
    if parsed.scheme() != "https"
        && !(parsed.scheme() == "http"
            && matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1")))
    {
        anyhow::bail!("updates require HTTPS (HTTP is allowed only for loopback tests)");
    }
    let mut response = crate::providers::external_http_agent(
        Duration::from_secs(30),
        Some(Duration::from_secs(300)),
        Some(Duration::from_secs(60)),
        Some(Duration::from_secs(300)),
    )
    .get(url)
    .header("User-Agent", "aishe-update")
    .call()
    .with_context(|| format!("downloading {url}"))?;
    if !crate::providers::status_is_accepted(response.status()) {
        anyhow::bail!("update server returned HTTP {}", response.status().as_u16());
    }
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        anyhow::bail!("update download exceeded its size limit");
    }
    Ok(bytes)
}

fn extract_binary(archive: &[u8]) -> Result<Vec<u8>> {
    let decoder = GzDecoder::new(archive);
    let mut archive = tar::Archive::new(decoder);
    let mut binary = None;
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        if path.as_ref() != Path::new("aishe") || !entry.header().entry_type().is_file() {
            anyhow::bail!("release archive contains an unexpected entry");
        }
        if binary.is_some() || entry.size() > DOWNLOAD_LIMIT {
            anyhow::bail!("release archive contains an invalid binary");
        }
        let mut bytes = Vec::new();
        entry.take(DOWNLOAD_LIMIT + 1).read_to_end(&mut bytes)?;
        binary = Some(bytes);
    }
    binary.context("release archive did not contain aishe")
}

fn activate(current: &Path, binary: &[u8], expected_version: &str) -> Result<()> {
    let root = update_root()?;
    std::fs::create_dir_all(&root)?;
    private_dir(&root)?;
    let staged = root.join("candidate-aishe");
    write_executable(&staged, binary)?;
    let output = Command::new(&staged).arg("--version").output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success()
        || !stdout.contains("aishe")
        || !stdout.contains(expected_version.trim_start_matches('v'))
    {
        anyhow::bail!("downloaded binary failed its self-test");
    }
    let backup = root.join("previous-aishe");
    write_executable(&backup, &std::fs::read(current)?)?;
    replace_binary(current, binary)?;
    std::fs::remove_file(staged).ok();
    Ok(())
}

fn replace_binary(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("binary has no parent directory")?;
    let staged = parent.join(format!(".aishe-update-{}", std::process::id()));
    write_executable(&staged, bytes)?;
    if let Err(error) = std::fs::rename(&staged, path) {
        std::fs::remove_file(&staged).ok();
        return Err(error).with_context(|| format!("activating {}", path.display()));
    }
    Ok(())
}

fn verify_format(bytes: &[u8]) -> Result<()> {
    let valid = if cfg!(target_os = "linux") {
        bytes.starts_with(b"\x7fELF")
    } else if cfg!(target_os = "macos") {
        matches!(
            bytes.get(..4),
            Some([0xcf, 0xfa, 0xed, 0xfe]) | Some([0xca, 0xfe, 0xba, 0xbe])
        )
    } else {
        false
    };
    if !valid {
        anyhow::bail!("binary format does not match this operating system");
    }
    Ok(())
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::config::write_atomic(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn update_root() -> Result<PathBuf> {
    Ok(crate::config::data_root()
        .context("no platform data directory")?
        .join("aishe/updates"))
}

fn private(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn confirm(action: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!("confirmation required; review the paths above and rerun with --yes");
    }
    use std::io::Write;
    print!("{action}? [y/N]: ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        anyhow::bail!("cancelled; nothing changed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_archives_are_strictly_bounded() {
        assert!(valid_version("v1.2.3-rc1"));
        assert!(!valid_version("../release"));
        assert!(!valid_version("v1.2.3/latest"));
        assert!(!valid_version("v1.2"));
        assert!(version_is_newer("v1.2.4", "1.2.3").unwrap());
        assert!(version_is_newer("v1.2.3", "1.2.3-rc1").unwrap());
        assert!(version_is_newer("v1.2.3-rc.10", "1.2.3-rc.2").unwrap());
        assert!(!version_is_newer("v1.2.2", "1.2.3").unwrap());

        let mut encoded = Vec::new();
        {
            let gzip = flate2::write::GzEncoder::new(&mut encoded, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(&mut header, "aishe", &b"test"[..])
                .unwrap();
            archive.finish().unwrap();
        }
        assert_eq!(extract_binary(&encoded).unwrap(), b"test");

        let mut hostile = Vec::new();
        {
            let gzip = flate2::write::GzEncoder::new(&mut hostile, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_size(4);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(&mut header, "other", &b"test"[..])
                .unwrap();
            archive.finish().unwrap();
        }
        assert!(extract_binary(&hostile).is_err());
    }
}
