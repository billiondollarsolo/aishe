//! Transient live-shell execution environment. This is deliberately not
//! serialized with sessions, audit records, model requests, or task checkpoints.

use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

use anyhow::{bail, Context, Result};

pub const MAX_STATE_BYTES: usize = 256 * 1024;
pub const MAX_ENVIRONMENT_VARIABLES: usize = 512;
pub const MAX_ENVIRONMENT_VALUE_BYTES: usize = 64 * 1024;
pub const MAX_ENVIRONMENT_NAME_BYTES: usize = 128;
const HEADER: &[u8] = b"AISHE_ENV_V1\0";

/// Consume one atomic snapshot from the shell's private directory. Traversal
/// opens each directory component without following symlinks; the containing
/// directory and regular file must belong to this user and be private. The
/// file is unlinked before reading/parsing, including malformed/oversized data.
/// No error includes names or values from the environment.
pub fn consume_environment(
    path: &Path,
    denied_environment: &HashSet<String>,
) -> Result<HashMap<String, String>> {
    let parent = path.parent().context("execution state has no directory")?;
    let name = path
        .file_name()
        .context("execution state has no filename")?;
    let directory = open_directory_without_symlinks(parent)?;
    let directory_metadata = directory.metadata()?;
    let owner = unsafe { libc::geteuid() };
    if directory_metadata.uid() != owner || directory_metadata.mode() & 0o777 != 0o700 {
        bail!("execution state directory is not private");
    }
    let name = CString::new(name.as_bytes()).context("invalid execution state filename")?;
    // SAFETY: directory is an owned, open directory descriptor; name is NUL
    // terminated. O_NOFOLLOW refuses symlinks and O_NONBLOCK avoids FIFO hangs.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error()).context("opening live execution state");
    }
    // SAFETY: openat returned a fresh descriptor owned by this File.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != owner
        || metadata.mode() & 0o777 != 0o600
        || metadata.nlink() != 1
    {
        bail!("execution state file is not a private regular file");
    }
    // SAFETY: the same private directory/name were opened above. unlinkat does
    // not follow a replaced symlink; bytes are read only from the opened file.
    if unsafe { libc::unlinkat(directory.as_raw_fd(), name.as_ptr(), 0) } != 0 {
        return Err(std::io::Error::last_os_error()).context("consuming live execution state");
    }
    if metadata.len() > MAX_STATE_BYTES as u64 {
        bail!("live execution state exceeds the size limit");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_STATE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .context("reading live execution state")?;
    parse_environment(&bytes, denied_environment)
}

fn open_directory_without_symlinks(path: &Path) -> Result<File> {
    if !path.is_absolute() {
        bail!("execution state directory must be absolute");
    }
    let mut directory = File::open("/")?;
    for component in path.components() {
        let name = match component {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            _ => bail!("invalid execution state directory"),
        };
        let name = CString::new(name.as_bytes()).context("invalid execution state directory")?;
        // SAFETY: directory stays alive through openat; every normal component
        // is opened independently so no parent symlink can bypass O_NOFOLLOW.
        let descriptor = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            )
        };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error())
                .context("opening execution state directory");
        }
        // SAFETY: openat returned a fresh owned descriptor.
        directory = unsafe { File::from_raw_fd(descriptor) };
    }
    Ok(directory)
}

fn parse_environment(
    bytes: &[u8],
    denied_environment: &HashSet<String>,
) -> Result<HashMap<String, String>> {
    if bytes.len() > MAX_STATE_BYTES {
        bail!("live execution state exceeds the size limit");
    }
    let body = bytes
        .strip_prefix(HEADER)
        .context("invalid live execution state header")?;
    if body.is_empty() {
        return Ok(HashMap::new());
    }
    if !body.ends_with(&[0]) {
        bail!("incomplete live execution state");
    }
    let mut fields = body[..body.len() - 1].split(|byte| *byte == 0);
    let mut result = HashMap::new();
    let mut seen = HashSet::new();
    let mut count = 0;
    while let Some(name) = fields.next() {
        count += 1;
        if count > MAX_ENVIRONMENT_VARIABLES {
            bail!("live execution state has too many variables");
        }
        let value = fields
            .next()
            .context("incomplete live execution state record")?;
        if name.is_empty()
            || name.len() > MAX_ENVIRONMENT_NAME_BYTES
            || !matches!(name[0], b'A'..=b'Z' | b'a'..=b'z' | b'_')
            || !name
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            bail!("invalid live execution state variable");
        }
        if value.len() > MAX_ENVIRONMENT_VALUE_BYTES {
            bail!("live execution state value exceeds the size limit");
        }
        let name = std::str::from_utf8(name).context("invalid live execution state variable")?;
        if !seen.insert(name) {
            bail!("duplicate live execution state variable");
        }
        // Filter before allocating values. Configured credentials, runtime
        // authentication and control variables can never become child state.
        if crate::executor::agent_environment_allowed(name, denied_environment) {
            let value = std::str::from_utf8(value).context("invalid live execution state value")?;
            result.insert(name.to_owned(), value.to_owned());
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PrivateDirectory(std::path::PathBuf);
    impl PrivateDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "aishe-state-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn write(&self, bytes: &[u8]) -> std::path::PathBuf {
            let path = self.0.join("state");
            std::fs::write(&path, bytes).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            path
        }
    }
    impl Drop for PrivateDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn snapshot_preserves_exact_execution_values_and_excludes_credentials() {
        let directory = PrivateDirectory::new();
        let path = directory.write(b"AISHE_ENV_V1\0PATH\0/private/venv/bin:/usr/bin\0VIRTUAL_ENV\0/private/venv\0MULTILINE\0one\ntwo=three\0CUSTOM_LOGIN\0configured-secret\0AISHE_GRANT\0agent-host\0API_TOKEN\0runtime-secret\0");
        let denied = HashSet::from(["CUSTOM_LOGIN".into()]);
        let environment = consume_environment(&path, &denied).unwrap();
        assert_eq!(environment["PATH"], "/private/venv/bin:/usr/bin");
        assert_eq!(environment["VIRTUAL_ENV"], "/private/venv");
        assert_eq!(environment["MULTILINE"], "one\ntwo=three");
        assert_eq!(environment.len(), 3);
        assert!(!path.exists(), "environment values must be transient");
    }

    #[test]
    fn complete_empty_snapshot_is_valid() {
        assert!(parse_environment(HEADER, &HashSet::new())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn malformed_snapshots_are_consumed_and_never_report_values() {
        for bytes in [
            &b"AISHE_ENV_V1\0VALUE\0private-value"[..],
            &b"AISHE_ENV_V1\0VALUE\0private-value\0VALUE\0private-value\0"[..],
            &b"AISHE_ENV_V1\0bad=name\0private-value\0"[..],
            &b"AISHE_ENV_V1\0VALUE\0private-value\0orphan\0"[..],
            &b"UNKNOWN\0VALUE\0private-value\0"[..],
        ] {
            let directory = PrivateDirectory::new();
            let path = directory.write(bytes);
            let error = consume_environment(&path, &HashSet::new())
                .unwrap_err()
                .to_string();
            assert!(!error.contains("private-value"));
            assert!(!path.exists());
        }
    }

    #[test]
    fn bounds_are_enforced_before_large_values_can_be_used() {
        let mut oversized_value = HEADER.to_vec();
        oversized_value.extend_from_slice(b"VALUE\0");
        oversized_value.extend(std::iter::repeat_n(b'x', MAX_ENVIRONMENT_VALUE_BYTES + 1));
        oversized_value.push(0);
        assert!(parse_environment(&oversized_value, &HashSet::new()).is_err());
        let mut too_many = HEADER.to_vec();
        for index in 0..=MAX_ENVIRONMENT_VARIABLES {
            too_many.extend_from_slice(format!("VALUE{index}\0\0").as_bytes());
        }
        assert!(parse_environment(&too_many, &HashSet::new()).is_err());
        let directory = PrivateDirectory::new();
        let path = directory.write(&vec![b'x'; MAX_STATE_BYTES + 1]);
        assert!(consume_environment(&path, &HashSet::new()).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn symlink_files_and_ancestors_are_refused() {
        let directory = PrivateDirectory::new();
        let path = directory.write(b"AISHE_ENV_V1\0");
        let link = directory.0.join("link");
        symlink(&path, &link).unwrap();
        assert!(consume_environment(&link, &HashSet::new()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), HEADER);
        let other = PrivateDirectory::new();
        let parent_link = other.0.join("linked-directory");
        symlink(&directory.0, &parent_link).unwrap();
        assert!(consume_environment(&parent_link.join("state"), &HashSet::new()).is_err());
        assert!(path.exists());
    }

    #[test]
    fn public_files_and_hardlinks_are_refused() {
        let directory = PrivateDirectory::new();
        let path = directory.write(HEADER);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(consume_environment(&path, &HashSet::new()).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::hard_link(&path, directory.0.join("duplicate")).unwrap();
        assert!(consume_environment(&path, &HashSet::new()).is_err());
        std::fs::remove_file(directory.0.join("duplicate")).unwrap();
        std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(consume_environment(&path, &HashSet::new()).is_err());
    }
}
