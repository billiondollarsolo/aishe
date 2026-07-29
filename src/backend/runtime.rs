use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use flate2::read::GzDecoder;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::manifest::{RuntimeAsset, RuntimeManifest};

const DOWNLOAD_SLACK: u64 = 1024 * 1024;
const EXTRACTED_LIMIT: u64 = 300 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 4096;

#[derive(Clone, Debug)]
pub enum InstallSource {
    Default,
    Local(PathBuf),
    Url(String),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeStatus {
    Missing {
        expected_version: String,
    },
    Ready {
        version: String,
        binary: PathBuf,
        sha256: String,
    },
    Invalid {
        expected_version: String,
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct InstallMetadata {
    schema_version: u32,
    runtime: String,
    version: String,
    platform: String,
    asset: String,
    archive_sha256: String,
    binary_sha256: String,
    installed_at_ms: u128,
}

#[derive(Clone, Debug)]
pub struct RuntimeManager {
    root: PathBuf,
    manifest: RuntimeManifest,
}

impl RuntimeManager {
    pub fn new() -> Result<Self> {
        if let Some(root) = std::env::var_os("AISHE_RUNTIME_DIR").filter(|value| !value.is_empty())
        {
            return Self::with_root(PathBuf::from(root));
        }
        let data = crate::config::data_root().context("cannot resolve Aishe data directory")?;
        Self::with_root(data.join("aishe").join("runtime"))
    }

    pub fn with_root(root: PathBuf) -> Result<Self> {
        Ok(Self {
            root,
            manifest: RuntimeManifest::embedded()?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }

    pub fn version_dir(&self) -> PathBuf {
        self.root
            .join("opencode")
            .join(self.manifest.version.as_str())
    }

    pub fn binary_path(&self) -> PathBuf {
        self.version_dir().join(if cfg!(windows) {
            "opencode.exe"
        } else {
            "opencode"
        })
    }

    fn previous_dir(&self) -> PathBuf {
        self.root
            .join("opencode")
            .join(format!(".previous-{}", self.manifest.version))
    }

    pub fn status(&self) -> RuntimeStatus {
        let binary = self.binary_path();
        if !binary.is_file() {
            return RuntimeStatus::Missing {
                expected_version: self.manifest.version.clone(),
            };
        }
        match self.verify_install(&binary) {
            Ok(binary_sha256) => RuntimeStatus::Ready {
                version: self.manifest.version.clone(),
                binary,
                sha256: binary_sha256,
            },
            Err(error) => RuntimeStatus::Invalid {
                expected_version: self.manifest.version.clone(),
                reason: crate::redact::redact(&error.to_string()),
            },
        }
    }

    pub fn install(&self, source: InstallSource, force: bool) -> Result<RuntimeStatus> {
        if !force {
            if let RuntimeStatus::Ready { .. } = self.status() {
                return Ok(self.status());
            }
        }
        self.create_private_dir(&self.root)?;
        self.create_private_dir(&self.root.join("opencode"))?;

        let asset = self.manifest.asset_for_current_platform()?.clone();
        let nonce = random_hex(12);
        let archive = self.root.join(format!(".download-{nonce}"));
        let staging = self.root.join(format!(".staging-{nonce}"));
        self.create_private_dir(&staging)?;

        let result = (|| {
            self.obtain_archive(&source, &asset, &archive)?;
            let archive_sha = sha256_file(&archive)?;
            if !archive_sha.eq_ignore_ascii_case(&asset.sha256) {
                anyhow::bail!(
                    "OpenCode runtime checksum mismatch for {} (expected {}, got {})",
                    asset.name,
                    asset.sha256,
                    archive_sha
                );
            }
            self.extract_archive(&archive, &staging, &asset)?;
            let located = find_binary(&staging)?;
            let canonical_binary = staging.join("opencode");
            if located != canonical_binary {
                fs::rename(&located, &canonical_binary).with_context(|| {
                    format!(
                        "moving extracted OpenCode binary from {}",
                        located.display()
                    )
                })?;
            }
            set_executable(&canonical_binary)?;
            let binary_sha256 = self.verify_binary(&canonical_binary)?;
            crate::config::write_atomic(
                &staging.join("LICENSE"),
                include_bytes!("../../assets/backend/opencode/LICENSE"),
            )?;
            crate::config::write_atomic(
                &staging.join("THIRD_PARTY_NOTICES.md"),
                include_bytes!("../../assets/backend/opencode/THIRD_PARTY_NOTICES.md"),
            )?;
            let metadata = InstallMetadata {
                schema_version: 1,
                runtime: "opencode".into(),
                version: self.manifest.version.clone(),
                platform: asset.platform.clone(),
                asset: asset.name.clone(),
                archive_sha256: archive_sha,
                binary_sha256,
                installed_at_ms: now_ms(),
            };
            crate::config::write_atomic(
                &staging.join("install.json"),
                serde_json::to_vec_pretty(&metadata)?.as_slice(),
            )?;

            let destination = self.version_dir();
            if destination.exists() {
                let replaced = self.previous_dir();
                let retired_previous = self.root.join(format!(".retired-previous-{nonce}"));
                if replaced.exists() {
                    fs::rename(&replaced, &retired_previous).with_context(|| {
                        format!("staging prior rollback runtime {}", replaced.display())
                    })?;
                }
                if let Err(error) = fs::rename(&destination, &replaced) {
                    if retired_previous.exists() {
                        let _ = fs::rename(&retired_previous, &replaced);
                    }
                    return Err(error).with_context(|| {
                        format!("staging existing runtime {}", destination.display())
                    });
                }
                if let Err(error) = fs::rename(&staging, &destination) {
                    let _ = fs::rename(&replaced, &destination);
                    if retired_previous.exists() {
                        let _ = fs::rename(&retired_previous, &replaced);
                    }
                    return Err(error).context("activating verified OpenCode runtime");
                }
                if retired_previous.exists() {
                    let _ = fs::remove_dir_all(retired_previous);
                }
            } else {
                fs::rename(&staging, &destination)
                    .context("activating verified OpenCode runtime")?;
            }
            crate::config::write_atomic(
                &self.root.join("current"),
                format!("{}\n", self.manifest.version).as_bytes(),
            )?;
            Ok(())
        })();

        let _ = fs::remove_file(&archive);
        if staging.exists() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
        let status = self.status();
        if !matches!(status, RuntimeStatus::Ready { .. }) {
            anyhow::bail!("installed OpenCode runtime did not pass final verification");
        }
        Ok(status)
    }

    pub fn verify(&self) -> Result<RuntimeStatus> {
        let status = self.status();
        match status {
            ready @ RuntimeStatus::Ready { .. } => Ok(ready),
            RuntimeStatus::Missing { expected_version } => {
                anyhow::bail!("OpenCode {expected_version} is not installed")
            }
            RuntimeStatus::Invalid { reason, .. } => anyhow::bail!("{reason}"),
        }
    }

    /// Atomically swap the current runtime with the immediately previous
    /// checksum-verified install of the same compatibility-pinned version.
    pub fn rollback(&self) -> Result<RuntimeStatus> {
        let current = self.version_dir();
        let previous = self.previous_dir();
        if !previous.is_dir() {
            anyhow::bail!(
                "no prior compatible OpenCode {} runtime is available",
                self.manifest.version
            );
        }
        // Verify the candidate before moving the working runtime.
        let candidate = previous.join(if cfg!(windows) {
            "opencode.exe"
        } else {
            "opencode"
        });
        self.verify_install(&candidate)
            .context("previous runtime failed compatibility verification")?;

        let nonce = random_hex(12);
        let swap = self.root.join(format!(".rollback-swap-{nonce}"));
        fs::rename(&current, &swap)
            .with_context(|| format!("staging current runtime {}", current.display()))?;
        if let Err(error) = fs::rename(&previous, &current) {
            let _ = fs::rename(&swap, &current);
            return Err(error).context("activating previous runtime");
        }
        if let Err(error) = fs::rename(&swap, &previous) {
            // The candidate is already active and verified. Preserve the prior
            // current runtime at the bounded swap path for manual recovery.
            return Err(error).context("retaining replaced runtime for reverse rollback");
        }
        crate::config::write_atomic(
            &self.root.join("current"),
            format!("{}\n", self.manifest.version).as_bytes(),
        )?;
        self.verify()
    }

    /// Remove only interrupted private staging/download/replacement entries.
    /// Version directories are intentionally retained for explicit rollback.
    pub fn garbage_collect(&self, dry_run: bool) -> Result<Vec<PathBuf>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let canonical_root = fs::canonicalize(&self.root)?;
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let path = entry?.path();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if ![
                ".download-",
                ".staging-",
                ".replaced-",
                ".retired-previous-",
                ".rollback-swap-",
            ]
            .iter()
            .any(|prefix| name.starts_with(prefix))
            {
                continue;
            }
            let parent = path.parent().and_then(|value| fs::canonicalize(value).ok());
            if parent.as_deref() != Some(canonical_root.as_path()) {
                anyhow::bail!("refusing runtime cleanup outside {}", self.root.display());
            }
            reject_symlink(&path)?;
            candidates.push(path);
        }
        if !dry_run {
            for path in &candidates {
                if path.is_dir() {
                    fs::remove_dir_all(path)?;
                } else {
                    fs::remove_file(path)?;
                }
            }
        }
        Ok(candidates)
    }

    fn obtain_archive(
        &self,
        source: &InstallSource,
        asset: &RuntimeAsset,
        destination: &Path,
    ) -> Result<()> {
        match source {
            InstallSource::Local(path) => {
                let mut input = File::open(path)
                    .with_context(|| format!("opening runtime archive {}", path.display()))?;
                copy_bounded(&mut input, destination, asset.size + DOWNLOAD_SLACK)?;
            }
            InstallSource::Url(url) => self.download(url, destination, asset.size)?,
            InstallSource::Default => {
                let override_base = std::env::var("AISHE_RUNTIME_BASE_URL").ok();
                let url = self.manifest.source_url(asset, override_base.as_deref());
                self.download(&url, destination, asset.size)?;
            }
        }
        let actual = fs::metadata(destination)?.len();
        if actual != asset.size {
            anyhow::bail!(
                "OpenCode runtime archive size mismatch for {} (expected {}, got {})",
                asset.name,
                asset.size,
                actual
            );
        }
        Ok(())
    }

    fn download(&self, url: &str, destination: &Path, expected: u64) -> Result<()> {
        let parsed = url::Url::parse(url).context("invalid OpenCode runtime URL")?;
        if !matches!(parsed.scheme(), "https" | "http") {
            anyhow::bail!("runtime URL must use HTTP(S)");
        }
        if parsed.scheme() == "http"
            && !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
        {
            anyhow::bail!("unencrypted runtime downloads are allowed only from loopback");
        }
        let response = ureq::get(url)
            .timeout(std::time::Duration::from_secs(300))
            .call()
            .with_context(|| format!("downloading OpenCode runtime from {url}"))?;
        if let Some(length) = response
            .header("content-length")
            .and_then(|value| value.parse::<u64>().ok())
        {
            if length != expected {
                anyhow::bail!(
                    "runtime server reported unexpected content length {length} (expected {expected})"
                );
            }
        }
        copy_bounded(
            &mut response.into_reader(),
            destination,
            expected + DOWNLOAD_SLACK,
        )
    }

    fn extract_archive(
        &self,
        archive_path: &Path,
        staging: &Path,
        asset: &RuntimeAsset,
    ) -> Result<()> {
        match asset.format.as_str() {
            "tar_gz" => extract_tar_gz(archive_path, staging),
            "zip" => extract_zip(archive_path, staging),
            other => anyhow::bail!("unsupported OpenCode archive format {other}"),
        }
    }

    fn verify_binary(&self, binary: &Path) -> Result<String> {
        reject_symlink(binary)?;
        let output = Command::new(binary)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .map_err(|error| {
                let platform = RuntimeManifest::platform_key().unwrap_or("unknown");
                anyhow::anyhow!(
                    "cannot start {} --version for {platform}: {error}; \
                     verify that this host's libc loader can execute the selected runtime",
                    binary.display()
                )
            })?;
        if !output.status.success() {
            anyhow::bail!(
                "OpenCode runtime version probe failed with {}",
                output.status
            );
        }
        let version = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if !version.contains(&self.manifest.version) {
            anyhow::bail!(
                "OpenCode runtime version mismatch: expected {}, reported {}",
                self.manifest.version,
                crate::commands::display_safe(version.trim())
            );
        }
        sha256_file(binary)
    }

    fn verify_install(&self, binary: &Path) -> Result<String> {
        let binary_sha = self.verify_binary(binary)?;
        let metadata_path = self.version_dir().join("install.json");
        reject_symlink(&metadata_path)?;
        let metadata: InstallMetadata = serde_json::from_slice(
            &fs::read(&metadata_path)
                .with_context(|| format!("reading {}", metadata_path.display()))?,
        )
        .context("parsing OpenCode install metadata")?;
        let expected_asset = self.manifest.asset_for_current_platform()?;
        if metadata.schema_version != 1
            || metadata.runtime != "opencode"
            || metadata.version != self.manifest.version
            || metadata.platform != expected_asset.platform
            || metadata.asset != expected_asset.name
            || !metadata
                .archive_sha256
                .eq_ignore_ascii_case(&expected_asset.sha256)
            || !metadata.binary_sha256.eq_ignore_ascii_case(&binary_sha)
        {
            anyhow::bail!("OpenCode install metadata does not match the verified runtime");
        }
        Ok(binary_sha)
    }

    fn create_private_dir(&self, path: &Path) -> Result<()> {
        fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
        crate::config::set_private_dir(path);
        Ok(())
    }
}

fn copy_bounded(reader: &mut dyn Read, destination: &Path, limit: u64) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options
        .open(destination)
        .with_context(|| format!("creating {}", destination.display()))?;
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > limit {
            anyhow::bail!("OpenCode runtime archive exceeds the download limit");
        }
        output.write_all(&buffer[..read])?;
    }
    output.flush()?;
    output.sync_all()?;
    Ok(())
}

fn extract_tar_gz(archive_path: &Path, staging: &Path) -> Result<()> {
    let input = File::open(archive_path)?;
    let mut archive = tar::Archive::new(GzDecoder::new(input));
    let mut entries = 0usize;
    let mut extracted = 0u64;
    for item in archive.entries()? {
        let mut entry = item?;
        entries += 1;
        if entries > MAX_ARCHIVE_ENTRIES {
            anyhow::bail!("OpenCode archive has too many entries");
        }
        let path = entry.path()?.into_owned();
        validate_archive_path(&path)?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            anyhow::bail!("OpenCode archive contains a link or special file");
        }
        extracted = extracted.saturating_add(entry.header().size()?);
        if extracted > EXTRACTED_LIMIT {
            anyhow::bail!("OpenCode archive exceeds the extraction limit");
        }
        entry
            .unpack_in(staging)
            .with_context(|| format!("extracting {}", path.display()))?;
    }
    Ok(())
}

fn extract_zip(archive_path: &Path, staging: &Path) -> Result<()> {
    let input = File::open(archive_path)?;
    let mut archive = zip::ZipArchive::new(input)?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        anyhow::bail!("OpenCode archive has too many entries");
    }
    let mut extracted = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let relative = entry
            .enclosed_name()
            .context("OpenCode zip contains an unsafe path")?;
        validate_archive_path(&relative)?;
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o040000 && kind != 0o100000 {
                anyhow::bail!("OpenCode zip contains a link or special file");
            }
        }
        extracted = extracted.saturating_add(entry.size());
        if extracted > EXTRACTED_LIMIT {
            anyhow::bail!("OpenCode archive exceeds the extraction limit");
        }
        let output_path = staging.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output_path)?;
        std::io::copy(&mut entry, &mut output)?;
    }
    Ok(())
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        anyhow::bail!("OpenCode archive contains an absolute or empty path");
    }
    for component in path.components() {
        if !matches!(component, Component::Normal(_)) {
            anyhow::bail!("OpenCode archive contains a path traversal");
        }
    }
    Ok(())
}

fn find_binary(root: &Path) -> Result<PathBuf> {
    let direct = root.join("opencode");
    if direct.is_file() {
        reject_symlink(&direct)?;
        return Ok(direct);
    }
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        reject_symlink(&path)?;
        if path.is_dir() {
            let nested = path.join("opencode");
            if nested.is_file() {
                reject_symlink(&nested)?;
                return Ok(nested);
            }
        }
    }
    anyhow::bail!("verified archive does not contain an OpenCode binary")
}

fn reject_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        anyhow::bail!("refusing symlink at {}", path.display());
    }
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "aishe-runtime-test-{label}-{}-{}",
            std::process::id(),
            random_hex(6)
        ))
    }

    #[test]
    fn absent_runtime_is_reported_without_creating_state() {
        let root = temp_root("missing");
        let manager = RuntimeManager::with_root(root.clone()).unwrap();
        assert!(matches!(
            manager.status(),
            RuntimeStatus::Missing { expected_version } if expected_version == "1.18.9"
        ));
        assert!(!root.exists());
    }

    #[test]
    fn archive_paths_fail_closed() {
        for value in ["../opencode", "/tmp/opencode", "a/../../opencode", "."] {
            assert!(validate_archive_path(Path::new(value)).is_err(), "{value}");
        }
        assert!(validate_archive_path(Path::new("package/opencode")).is_ok());
    }

    #[test]
    fn bounded_copy_removes_no_existing_target() {
        let root = temp_root("copy");
        fs::create_dir_all(&root).unwrap();
        let target = root.join("archive");
        let mut oversized: &[u8] = b"12345";
        assert!(copy_bounded(&mut oversized, &target, 4).is_err());
        assert!(fs::read(&target).unwrap().is_empty());
        // Installation owns and removes this private temporary file on failure;
        // the primitive itself never overwrites an existing path.
        let mut retry: &[u8] = b"x";
        assert!(copy_bounded(&mut retry, &target, 4).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    fn synthetic_archive(root: &Path) -> PathBuf {
        synthetic_archive_version(root, "1.18.9")
    }

    #[cfg(unix)]
    fn synthetic_archive_version(root: &Path, version: &str) -> PathBuf {
        use flate2::write::GzEncoder;
        use flate2::Compression;

        fs::create_dir_all(root).unwrap();
        let path = root.join("opencode-test.tar.gz");
        let file = File::create(&path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut archive = tar::Builder::new(encoder);
        let script = format!("#!/bin/sh\nprintf 'opencode {version}\\n'\n");
        let mut header = tar::Header::new_gnu();
        header.set_size(script.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        archive
            .append_data(&mut header, "package/opencode", script.as_bytes())
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        path
    }

    #[cfg(unix)]
    fn manager_for_archive(runtime_root: PathBuf, archive: &Path) -> RuntimeManager {
        let mut manager = RuntimeManager::with_root(runtime_root).unwrap();
        manager.manifest.assets = vec![RuntimeAsset {
            platform: RuntimeManifest::platform_key().unwrap().into(),
            name: archive.file_name().unwrap().to_string_lossy().into_owned(),
            format: "tar_gz".into(),
            size: fs::metadata(archive).unwrap().len(),
            sha256: sha256_file(archive).unwrap(),
        }];
        manager
    }

    #[test]
    #[cfg(unix)]
    fn failed_force_install_preserves_the_verified_active_runtime() {
        let root = temp_root("transaction");
        let source_root = root.join("source");
        let archive = synthetic_archive(&source_root);
        let manager = manager_for_archive(root.join("runtime"), &archive);
        let status = manager
            .install(InstallSource::Local(archive.clone()), false)
            .unwrap();
        assert!(matches!(status, RuntimeStatus::Ready { .. }));
        let before = fs::read(manager.binary_path()).unwrap();

        let corrupt = source_root.join("corrupt.tar.gz");
        fs::write(&corrupt, b"truncated").unwrap();
        let error = manager
            .install(InstallSource::Local(corrupt), true)
            .unwrap_err();
        assert!(error.to_string().contains("size mismatch"));
        assert_eq!(fs::read(manager.binary_path()).unwrap(), before);
        assert!(matches!(
            manager.verify().unwrap(),
            RuntimeStatus::Ready { .. }
        ));
        assert!(fs::read_dir(manager.root()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn checksum_version_and_extraction_faults_never_replace_active_runtime() {
        let root = temp_root("fault-matrix");
        let source_root = root.join("source");
        let archive = synthetic_archive(&source_root.join("valid"));
        let manager = manager_for_archive(root.join("runtime"), &archive);
        manager
            .install(InstallSource::Local(archive.clone()), false)
            .unwrap();
        let before = fs::read(manager.binary_path()).unwrap();

        let checksum_fault = source_root.join("checksum.tar.gz");
        let mut corrupted = fs::read(&archive).unwrap();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xff;
        fs::write(&checksum_fault, corrupted).unwrap();
        let checksum_error = manager
            .install(InstallSource::Local(checksum_fault), true)
            .unwrap_err();
        assert!(checksum_error.to_string().contains("checksum mismatch"));
        assert_eq!(fs::read(manager.binary_path()).unwrap(), before);

        let wrong_version = synthetic_archive_version(&source_root.join("wrong-version"), "1.18.8");
        let wrong_manager = manager_for_archive(manager.root().to_path_buf(), &wrong_version);
        let version_error = wrong_manager
            .install(InstallSource::Local(wrong_version), true)
            .unwrap_err();
        assert!(version_error.to_string().contains("expected"));
        assert_eq!(fs::read(manager.binary_path()).unwrap(), before);

        let invalid_archive = source_root.join("interrupted.tar.gz");
        fs::write(&invalid_archive, b"not a complete tar gzip stream").unwrap();
        let invalid_manager = manager_for_archive(manager.root().to_path_buf(), &invalid_archive);
        let extraction_error = invalid_manager
            .install(InstallSource::Local(invalid_archive), true)
            .unwrap_err();
        assert!(
            extraction_error.to_string().contains("archive")
                || extraction_error.to_string().contains("gzip")
        );
        assert_eq!(fs::read(manager.binary_path()).unwrap(), before);
        assert!(matches!(
            manager.verify().unwrap(),
            RuntimeStatus::Ready { .. }
        ));
        assert!(fs::read_dir(manager.root()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn runtime_gc_removes_only_abandoned_private_staging() {
        let root = temp_root("gc");
        let archive = synthetic_archive(&root.join("source"));
        let manager = manager_for_archive(root.join("runtime"), &archive);
        fs::create_dir_all(manager.root()).unwrap();
        let staging = manager.root().join(".staging-deadbeef");
        let version = manager.root().join("opencode").join("keep-version");
        fs::create_dir_all(&staging).unwrap();
        fs::create_dir_all(&version).unwrap();

        assert_eq!(
            manager.garbage_collect(true).unwrap(),
            vec![staging.clone()]
        );
        assert!(staging.exists());
        assert_eq!(
            manager.garbage_collect(false).unwrap(),
            vec![staging.clone()]
        );
        assert!(!staging.exists());
        assert!(version.exists());
        fs::remove_dir_all(root).unwrap();
    }
}
