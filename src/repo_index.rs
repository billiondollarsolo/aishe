//! Bounded, incremental repository index for explicit local code retrieval.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const SCHEMA_VERSION: u32 = 1;
const MAX_FILES: usize = 10_000;
const MAX_FILE_BYTES: u64 = 256 * 1024;
const MAX_INDEX_BYTES: usize = 64 * 1024 * 1024;
const CHUNK_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub hash: String,
    pub language: String,
    pub bytes: usize,
    pub chunks: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Index {
    pub schema_version: u32,
    pub repository: PathBuf,
    pub head: String,
    pub updated_at_ms: u128,
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub schema_version: u32,
    pub repository: PathBuf,
    pub head: String,
    pub files: usize,
    pub chunks: usize,
    pub bytes: usize,
    pub updated_at_ms: u128,
}

pub enum Action<'a> {
    Build {
        rebuild: bool,
        json: bool,
    },
    Status {
        json: bool,
    },
    Search {
        query: &'a str,
        limit: usize,
        json: bool,
    },
}

pub fn command(cwd: &Path, action: Action<'_>) -> Result<u8> {
    match action {
        Action::Build { rebuild, json } => {
            let (index, changed) = build(cwd, rebuild)?;
            let summary = summary(&index);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": 1,
                        "changed_files": changed,
                        "index": summary,
                    }))?
                );
            } else {
                println!(
                    "indexed {} files / {} chunks ({} changed) at {}",
                    summary.files,
                    summary.chunks,
                    changed,
                    summary.repository.display()
                );
            }
        }
        Action::Status { json } => {
            let root = repo_root(cwd)?;
            let index = load(&root)?.context("no repository index; run `aishe index`")?;
            let summary = summary(&index);
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                println!(
                    "{} files / {} chunks / {} bytes · HEAD {}",
                    summary.files, summary.chunks, summary.bytes, summary.head
                );
            }
        }
        Action::Search { query, limit, json } => {
            let root = repo_root(cwd)?;
            let index = load(&root)?.context("no repository index; run `aishe index`")?;
            let matches = search(&index, query, limit.min(20));
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "schema_version": 1,
                        "query": query,
                        "matches": matches,
                    }))?
                );
            } else {
                for hit in matches {
                    println!("{}:{}\n{}", hit.path, hit.chunk + 1, hit.text);
                }
            }
        }
    }
    Ok(0)
}

pub fn build(cwd: &Path, rebuild: bool) -> Result<(Index, usize)> {
    let root = repo_root(cwd)?;
    let old = if rebuild { None } else { load(&root)? };
    let old_by_path: BTreeMap<&str, &FileEntry> = old
        .as_ref()
        .map(|index| {
            index
                .files
                .iter()
                .map(|file| (file.path.as_str(), file))
                .collect()
        })
        .unwrap_or_default();
    let mut files = Vec::new();
    let mut total = 0usize;
    let mut changed = 0usize;

    for relative in tracked_files(&root)?.into_iter().take(MAX_FILES + 1) {
        if files.len() == MAX_FILES {
            anyhow::bail!("repository exceeds the {MAX_FILES}-file index limit");
        }
        let path = root.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() || metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        let bytes = std::fs::read(&path)?;
        if bytes.contains(&0) {
            continue;
        }
        total = total.saturating_add(bytes.len());
        if total > MAX_INDEX_BYTES {
            anyhow::bail!("repository index exceeds the 64 MiB text limit");
        }
        let path_text = crate::commands::display_safe(&relative.to_string_lossy());
        let hash = format!("{:x}", Sha256::digest(&bytes));
        if let Some(existing) = old_by_path
            .get(path_text.as_str())
            .filter(|f| f.hash == hash)
        {
            files.push((*existing).clone());
            continue;
        }
        changed += 1;
        let text = crate::commands::display_safe(&String::from_utf8_lossy(&bytes));
        files.push(FileEntry {
            language: language(&relative),
            path: path_text,
            hash,
            bytes: bytes.len(),
            chunks: chunks(&text),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let index = Index {
        schema_version: SCHEMA_VERSION,
        repository: root.clone(),
        head: git(&root, &["rev-parse", "HEAD"])
            .unwrap_or_else(|_| "unknown".into())
            .trim()
            .to_string(),
        updated_at_ms: now_ms(),
        files,
    };
    save(&root, &index)?;
    Ok((index, changed))
}

#[derive(Clone, Debug, Serialize)]
pub struct Match {
    pub path: String,
    pub chunk: usize,
    pub score: usize,
    pub hash: String,
    pub text: String,
}

pub fn search(index: &Index, query: &str, limit: usize) -> Vec<Match> {
    let terms: Vec<String> = query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| term.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    if terms.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for file in &index.files {
        for (chunk, text) in file.chunks.iter().enumerate() {
            let haystack = format!("{}\n{text}", file.path).to_ascii_lowercase();
            let score = terms
                .iter()
                .map(|term| haystack.match_indices(term).count())
                .sum();
            if score > 0 {
                hits.push(Match {
                    path: file.path.clone(),
                    chunk,
                    score,
                    hash: file.hash.clone(),
                    text: text.clone(),
                });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(limit);
    hits
}

fn repo_root(cwd: &Path) -> Result<PathBuf> {
    let root = git(cwd, &["rev-parse", "--show-toplevel"])?;
    PathBuf::from(root.trim())
        .canonicalize()
        .context("resolving repository root")
}

fn tracked_files(root: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args(["ls-files", "-z", "--cached"])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    if !output.status.success() {
        anyhow::bail!("git ls-files failed");
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(path_from_bytes)
        .collect()
}

#[cfg(unix)]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        anyhow::bail!("git returned an unsafe tracked path");
    }
    Ok(path)
}

#[cfg(not(unix))]
fn path_from_bytes(bytes: &[u8]) -> Result<PathBuf> {
    let path =
        PathBuf::from(String::from_utf8(bytes.to_vec()).context("tracked path is not UTF-8")?);
    if path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        anyhow::bail!("git returned an unsafe tracked path");
    }
    Ok(path)
}

fn chunks(text: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if !current.is_empty() && current.len().saturating_add(line.len() + 1) > CHUNK_BYTES {
            result.push(std::mem::take(&mut current));
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}

fn language(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or("text")
        .to_ascii_lowercase()
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        anyhow::bail!("git {} failed", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn store_path(root: &Path) -> Result<PathBuf> {
    let base = crate::config::data_root().context("no platform data directory")?;
    let identity = format!(
        "{:x}",
        Sha256::digest(root.as_os_str().to_string_lossy().as_bytes())
    );
    Ok(base
        .join("aishe")
        .join("repo-index")
        .join(identity)
        .join("index.json"))
}

fn load(root: &Path) -> Result<Option<Index>> {
    let path = store_path(root)?;
    if !path.exists() {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_INDEX_BYTES as u64 * 2 {
        anyhow::bail!("repository index is invalid or oversized");
    }
    let index: Index = serde_json::from_slice(&std::fs::read(&path)?).with_context(|| {
        format!(
            "reading {}; rebuild with `aishe index --rebuild`",
            path.display()
        )
    })?;
    if index.schema_version != SCHEMA_VERSION || index.repository != root {
        anyhow::bail!("repository index identity/version mismatch; rebuild it");
    }
    Ok(Some(index))
}

fn save(root: &Path, index: &Index) -> Result<()> {
    let path = store_path(root)?;
    let parent = path.parent().context("repository index has no parent")?;
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    crate::config::write_atomic(&path, &serde_json::to_vec(index)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn summary(index: &Index) -> Summary {
    Summary {
        schema_version: SCHEMA_VERSION,
        repository: index.repository.clone(),
        head: index.head.clone(),
        files: index.files.len(),
        chunks: index.files.iter().map(|file| file.chunks.len()).sum(),
        bytes: index.files.iter().map(|file| file.bytes).sum(),
        updated_at_ms: index.updated_at_ms,
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexical_search_is_ranked_and_bounded() {
        let index = Index {
            schema_version: 1,
            repository: PathBuf::from("/tmp/example"),
            head: "abc".into(),
            updated_at_ms: 0,
            files: vec![FileEntry {
                path: "src/auth.rs".into(),
                hash: "h".into(),
                language: "rs".into(),
                bytes: 10,
                chunks: vec!["token token validation".into(), "unrelated".into()],
            }],
        };
        let hits = search(&index, "token validation", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "src/auth.rs");
        assert_eq!(hits[0].score, 3);
    }
}
