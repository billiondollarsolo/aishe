//! Read-only, revision-bound change review and explicit selective application.
//!
//! The display is redacted independently of the exact bytes Git receives. A
//! private application journal makes an interrupted application uncertain rather
//! than silently replaying it. Partial applications retain stable original IDs.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Record, State};

const MAX_PATCH: usize = 8 * 1024 * 1024;
const MAX_FILES: usize = 256;
const MAX_HUNKS: usize = 4096;
const MAX_PREIMAGES: usize = 32 * 1024 * 1024;
const MAX_PREVIEW: usize = 256 * 1024;
const MAX_LEDGER: usize = 128 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Modified,
    Created,
    Deleted,
    Renamed,
    Binary,
    Mode,
    Unsupported,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeHunk {
    pub id: usize,
    pub header: String,
    pub lines: Vec<String>,
    pub applied: bool,
    pub selectable: bool,
    pub clipped: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeFile {
    pub id: usize,
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub hunks: Vec<ChangeHunk>,
    pub applied: bool,
    pub selectable: bool,
    pub limitation: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChangeReview {
    pub task_id: String,
    pub revision: String,
    pub patch_sha256: String,
    pub files: Vec<ChangeFile>,
    pub check_summary: crate::tasks::CheckSummary,
    pub checks: Vec<crate::tasks::CheckEvidence>,
    pub evidence_workspace_revision: u64,
    pub evidence_caveat: String,
    pub remaining_hunks: usize,
    pub applied_hunks: usize,
    pub can_apply: bool,
    pub unresolved: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyChanges {
    pub task_id: String,
    pub applied_hunks: Vec<usize>,
    pub remaining_hunks: usize,
    pub complete: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Ledger {
    #[serde(default)]
    patch_sha256: String,
    #[serde(default)]
    applied: BTreeSet<usize>,
    #[serde(default)]
    pending: Option<PendingApply>,
}

#[derive(Clone, Serialize, Deserialize)]
struct PendingApply {
    hunks: BTreeSet<usize>,
    selection_sha256: String,
    source_before: String,
}

struct RawFile {
    header: Vec<u8>,
    hunks: Vec<Vec<u8>>,
    old_path: Option<PathBuf>,
    path: PathBuf,
    kind: ChangeKind,
    atomic: bool,
    limitation: Option<String>,
}

struct Capture {
    patch: Vec<u8>,
    files: Vec<RawFile>,
    ledger: Ledger,
    source_stamp: String,
    review: ChangeReview,
}

/// Captures exact patch/source identities and actual recorded check outcomes.
/// This never executes a provider, external diff, filter, textconv, or Git hook.
pub fn task_change_review(id: &str) -> Result<ChangeReview> {
    let mut record = super::load(id)?;
    if record.id != id {
        anyhow::bail!("task record identity does not match the selected task");
    }
    super::reconcile(&mut record)?;
    Ok(capture(&record)?.review)
}

/// Apply a selection only if it still describes the exact reviewed source and
/// task patch. Empty selectors mean all remaining changes. File/hunk IDs must
/// be unique, positive, known, and not already applied.
pub fn apply_task_changes(
    id: &str,
    revision: &str,
    files: &[usize],
    hunks: &[usize],
) -> Result<ApplyChanges> {
    if revision.len() != 64 || !revision.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("an exact change-review revision is required");
    }
    let mut result = None;
    let mut application_error = None;
    super::update(id, |record| {
        if record.id != id {
            anyhow::bail!("task record identity does not match the selected task");
        }
        let repo = record
            .source_repo
            .as_deref()
            .context("task has no source git repository")?;
        let _source_lock = source_lock(repo)?;
        let mut captured = capture(record)?;
        if captured.review.revision != revision {
            anyhow::bail!(
                "changes or source files changed since review; refresh the review before applying"
            );
        }
        if !captured.review.can_apply {
            anyhow::bail!(
                "task changes cannot be applied: {}",
                captured.review.unresolved.join(" ")
            );
        }
        let selected = selection(&captured.files, &captured.ledger, files, hunks)?;
        let bytes = selected_patch(&captured.files, &selected);
        if bytes.is_empty() {
            anyhow::bail!("there are no remaining selected changes");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let filters = configured_filters(repo, deadline)?;
        let mut check = git_command(repo, &filters);
        check.args(["apply", "--check", "--whitespace=nowarn", "-"]);
        command_output(&mut check, Some(&bytes), 64 * 1024, false, deadline)
            .context("selected changes do not apply cleanly; source files remain unchanged")?;
        // Recheck after validation, immediately before writing. The source lock
        // serializes aishe applications; manual changes also invalidate this.
        if source_stamp(repo, &captured.files, deadline)? != captured.source_stamp {
            anyhow::bail!(
                "source files changed while validating the selection; refresh the review"
            );
        }
        captured.ledger.pending = Some(PendingApply {
            hunks: selected.clone(),
            selection_sha256: digest(&bytes),
            source_before: captured.source_stamp.clone(),
        });
        write_ledger(id, &captured.ledger)?;
        let mut command = git_command(repo, &filters);
        command.args(["apply", "--whitespace=nowarn", "-"]);
        // Any failure after the intent is durable remains uncertain. A killed
        // process may have written files even if no result reached this process.
        let execution = command_output(&mut command, Some(&bytes), 64 * 1024, false, deadline)
            .context(
                "application result is uncertain; inspect source files before further task actions",
            );
        if let Err(error) = execution {
            record.state = State::Failed;
            record.error = Some(safe_text(&format!("{error:#}")));
            application_error = Some(error);
            // Publish attention while preserving the pending journal. Returning
            // an error from the closure would suppress this authoritative save.
            return Ok(());
        }
        captured.ledger.applied.extend(selected.iter().copied());
        captured.ledger.pending = None;
        if let Err(error) = write_ledger(id, &captured.ledger).context("changes were applied but their journal could not be saved; inspect source files before further task actions") {
            record.state = State::Failed;
            record.error = Some(safe_text(&format!("{error:#}")));
            application_error = Some(error);
            return Ok(());
        }
        let count = captured
            .files
            .iter()
            .map(|file| file.hunks.len())
            .sum::<usize>();
        let remaining = count.saturating_sub(captured.ledger.applied.len());
        record.applied_hunks = captured.ledger.applied.iter().copied().collect();
        if remaining == 0 {
            record.state = State::Applied;
            record.applied_patch_sha256 = Some(digest(&captured.patch));
        }
        result = Some(ApplyChanges {
            task_id: id.into(),
            applied_hunks: selected.into_iter().collect(),
            remaining_hunks: remaining,
            complete: remaining == 0,
        });
        Ok(())
    })?;
    if let Some(error) = application_error {
        return Err(error);
    }
    result.context("application produced no result")
}

/// Continuing an already partially applied workspace could replay old changes.
/// Start a new task instead; the unapplied original selection stays reviewable.
pub(super) fn ensure_no_partial_apply(record: &Record) -> Result<()> {
    let ledger = read_ledger(&record.id)?;
    if ledger.pending.is_some() || !ledger.applied.is_empty() {
        anyhow::bail!("task changes were already applied or application is uncertain; review remaining changes or start a new task");
    }
    Ok(())
}

fn capture(record: &Record) -> Result<Capture> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let (patch, files) = raw_patch(record, deadline)?;
    let patch_sha256 = digest(&patch);
    let mut ledger = read_ledger(&record.id)?;
    if ledger.patch_sha256.is_empty() {
        ledger.patch_sha256 = patch_sha256.clone();
    }
    let repo = record
        .source_repo
        .as_deref()
        .context("task has no source git repository")?;
    let source_stamp = source_stamp(repo, &files, deadline)?;
    let ledger_bytes = serde_json::to_vec(&ledger)?;
    let mut stamp = Sha256::new();
    for part in [
        record.id.as_bytes(),
        patch_sha256.as_bytes(),
        source_stamp.as_bytes(),
        &ledger_bytes,
    ] {
        stamp.update((part.len() as u64).to_le_bytes());
        stamp.update(part);
    }
    stamp.update(format!(
        "{:?}:{}:{}",
        record.state, record.result_revision, record.budget_exceeded
    ));
    let revision = format!("{:x}", stamp.finalize());
    let mut unresolved = Vec::new();
    if ledger.patch_sha256 != patch_sha256
        && (!ledger.applied.is_empty() || ledger.pending.is_some())
    {
        unresolved.push(
            "The task patch changed after changes were applied; automatic application is blocked."
                .into(),
        );
    }
    if ledger.pending.is_some() {
        unresolved.push("A previous application has an uncertain result; inspect the source files before continuing.".into());
    }
    if record.budget_exceeded {
        unresolved.push("The task exceeded its change budget.".into());
    }
    if !matches!(
        record.state,
        State::Completed | State::Failed | State::Interrupted
    ) {
        unresolved.push(format!(
            "Changes cannot be applied while the task is {:?}.",
            record.state
        ));
    }
    let checkpoint = record.native_task_id.as_deref().and_then(|id| {
        super::task_details(&record.id)
            .ok()
            .and_then(|details| details.checkpoint)
            .filter(|checkpoint| checkpoint.id == id)
    });
    let check_summary = checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.check_summary.clone())
        .unwrap_or_else(|| crate::tasks::CheckSummary {
            unresolved: vec!["No explicit checks have been recorded.".into()],
            ..Default::default()
        });
    let checks = checkpoint
        .as_ref()
        .map(|checkpoint| {
            checkpoint
                .evidence
                .iter()
                .filter(|entry| entry.kind == crate::tasks::ExecutionKind::Check)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let evidence_workspace_revision =
        checkpoint.map_or(0, |checkpoint| checkpoint.workspace_revision);
    let mut hunk_id = 0;
    let mut preview_left = MAX_PREVIEW;
    let displayed = files.iter().enumerate().map(|(index, file)| {
        let hunks = file.hunks.iter().map(|hunk| {
            hunk_id += 1;
            let text = crate::commands::display_safe_multiline(&crate::redact::redact(&String::from_utf8_lossy(hunk)));
            let header = text.lines().next().map(str::to_string).unwrap_or_else(|| format!("{:?} whole file change", file.kind));
            let mut clipped = false;
            let lines = text.lines().take(1000).filter_map(|line| {
                if preview_left == 0 { clipped = true; return None; }
                let mut line = line.chars().take(2048).collect::<String>();
                if line.len() > preview_left {
                    let mut end = preview_left;
                    while !line.is_char_boundary(end) { end -= 1; }
                    line.truncate(end);
                    clipped = true;
                }
                preview_left = preview_left.saturating_sub(line.len() + 1);
                Some(line)
            }).collect();
            ChangeHunk { id: hunk_id, header, lines, applied: ledger.applied.contains(&hunk_id), selectable: !file.atomic && file.limitation.is_none() && !ledger.applied.contains(&hunk_id), clipped }
        }).collect::<Vec<_>>();
        ChangeFile { id: index + 1, path: safe_text(&file.path.to_string_lossy()), old_path: file.old_path.as_ref().map(|path| safe_text(&path.to_string_lossy())), kind: file.kind, applied: hunks.iter().all(|hunk| hunk.applied), selectable: file.limitation.is_none() && hunks.iter().any(|hunk| !hunk.applied), limitation: file.limitation.clone().or_else(|| file.atomic.then(|| "Select the whole file to preserve this binary, rename, creation, deletion, or mode change.".into())), hunks }
    }).collect();
    let applied_hunks = ledger.applied.len();
    let remaining_hunks = hunk_id.saturating_sub(applied_hunks);
    let can_apply = unresolved.is_empty() && remaining_hunks > 0;
    Ok(Capture { patch, files, ledger, source_stamp, review: ChangeReview { task_id: record.id.clone(), revision, patch_sha256, files: displayed, check_summary, checks, evidence_workspace_revision, evidence_caveat: "Any recorded check evidence belongs to the task workspace. A selected subset has not been checked separately; freshness covers recorded task effects.".into(), remaining_hunks, applied_hunks, can_apply, unresolved } })
}

fn raw_patch(record: &Record, deadline: Instant) -> Result<(Vec<u8>, Vec<RawFile>)> {
    let worktree = record
        .worktree
        .as_deref()
        .context("task has no isolated worktree")?;
    let workflow = record
        .workflow
        .as_ref()
        .map(|link| super::workflows::load_run(&link.run_id))
        .transpose()?;
    if let (Some(workflow), Some(link)) = (&workflow, &record.workflow) {
        if record.source_repo.as_ref() != Some(&workflow.source_repo)
            || record.source_cwd != workflow.source_cwd
            || !workflow
                .stages
                .iter()
                .any(|stage| stage.task_id == record.id && stage.key == link.stage_key)
        {
            anyhow::bail!("workflow task does not match its recorded source and stage");
        }
    }
    // Execution budgets compare against a dependency snapshot. Reviewing a
    // workflow leaf instead compares against the original source so inherited
    // implementation/test changes remain visible and can be applied together.
    let base = workflow
        .as_ref()
        .map(|workflow| workflow.base_head.as_str())
        .or(record.base_head.as_deref())
        .context("task has no recorded base commit")?;
    if !matches!(base.len(), 40 | 64) || !base.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        anyhow::bail!("task base commit is not an object ID");
    }
    let expected = super::task_dir(&record.id)?.join("worktree");
    if worktree != expected || fs::symlink_metadata(worktree)?.file_type().is_symlink() {
        anyhow::bail!("task worktree is not an owned isolated directory");
    }
    let filters = configured_filters(worktree, deadline)?;
    let diff_args = [
        "diff",
        "--binary",
        "--find-renames",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        base,
        "--",
    ];
    let mut diff = git_command(worktree, &filters);
    let mut patch = command_output(diff.args(diff_args), None, MAX_PATCH, true, deadline)?;
    let mut names = git_command(worktree, &filters);
    let names = command_output(
        names.args([
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            "--no-ext-diff",
            "--no-textconv",
            base,
            "--",
        ]),
        None,
        MAX_PATCH,
        true,
        deadline,
    )?;
    let paths = parse_names(&names)?;
    let mut files = parse_sections(&patch, &paths)?;
    let mut again = git_command(worktree, &filters);
    if command_output(again.args(diff_args), None, MAX_PATCH, true, deadline)? != patch {
        anyhow::bail!("task files changed during review; refresh the review");
    }
    let mut list = git_command(worktree, &filters);
    let untracked = command_output(
        list.args(["ls-files", "--others", "--exclude-standard", "-z"]),
        None,
        256 * 1024,
        false,
        deadline,
    )?;
    for raw in untracked
        .split(|byte| *byte == 0)
        .filter(|raw| !raw.is_empty())
    {
        let path = PathBuf::from(
            std::str::from_utf8(raw).context("non-UTF-8 task paths require manual review")?,
        );
        validate_path(&path)?;
        let mut extra = git_command(worktree, &filters);
        let bytes = command_output(
            extra
                .current_dir(worktree)
                .args([
                    "diff",
                    "--no-index",
                    "--binary",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-color",
                    "--",
                ])
                .arg("/dev/null")
                .arg(&path),
            None,
            MAX_PATCH.saturating_sub(patch.len()),
            true,
            deadline,
        )?;
        let mut sections = parse_sections(&bytes, &[(ChangeKind::Created, None, path)])?;
        files.append(&mut sections);
        patch.extend(bytes);
    }
    if files.len() > MAX_FILES
        || files.iter().map(|file| file.hunks.len()).sum::<usize>() > MAX_HUNKS
    {
        anyhow::bail!("task changes exceed the bounded file/hunk review limit; review manually");
    }
    for file in &mut files {
        for path in file.old_path.iter().chain(std::iter::once(&file.path)) {
            validate_path(path)?;
            if let Err(error) = validate_ancestors(worktree, path) {
                file.limitation = Some(error.to_string());
            }
        }
        if file.header.windows(7).any(|bytes| bytes == b"120000\n")
            || file.header.windows(7).any(|bytes| bytes == b"160000\n")
            || file.header.windows(7).any(|bytes| bytes == b"160000 ")
        {
            file.kind = ChangeKind::Unsupported;
            file.limitation = Some(
                "Symlinks and submodule changes require manual review and application.".into(),
            );
        }
    }
    Ok((patch, files))
}

fn parse_names(bytes: &[u8]) -> Result<Vec<(ChangeKind, Option<PathBuf>, PathBuf)>> {
    let mut parts = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let mut result = Vec::new();
    while let Some(status) = parts.next() {
        let kind = match status.first() {
            Some(b'M') => ChangeKind::Modified,
            Some(b'A') => ChangeKind::Created,
            Some(b'D') => ChangeKind::Deleted,
            Some(b'R') => ChangeKind::Renamed,
            _ => ChangeKind::Unsupported,
        };
        let first = PathBuf::from(
            std::str::from_utf8(parts.next().context("missing diff path")?)
                .context("non-UTF-8 task paths require manual review")?,
        );
        let (old, path) = if kind == ChangeKind::Renamed {
            (
                Some(first),
                PathBuf::from(
                    std::str::from_utf8(parts.next().context("missing rename path")?)
                        .context("non-UTF-8 task paths require manual review")?,
                ),
            )
        } else {
            (None, first)
        };
        result.push((kind, old, path));
    }
    Ok(result)
}

fn parse_sections(
    bytes: &[u8],
    paths: &[(ChangeKind, Option<PathBuf>, PathBuf)],
) -> Result<Vec<RawFile>> {
    let text = std::str::from_utf8(bytes).context("non-UTF-8 patch text requires manual review")?;
    let mut sections = Vec::new();
    let mut current = String::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") && !current.is_empty() {
            sections.push(std::mem::take(&mut current));
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        sections.push(current);
    }
    if sections.len() != paths.len() {
        anyhow::bail!("task patch and file names changed during review");
    }
    sections
        .into_iter()
        .zip(paths)
        .map(|(section, (kind, old_path, path))| {
            let mut header = Vec::new();
            let mut hunks = Vec::new();
            let mut current = Vec::new();
            for line in section.split_inclusive('\n') {
                if line.starts_with("@@ ") {
                    if !current.is_empty() {
                        hunks.push(std::mem::take(&mut current));
                    }
                    current.extend(line.as_bytes());
                } else if current.is_empty() {
                    header.extend(line.as_bytes());
                } else {
                    current.extend(line.as_bytes());
                }
            }
            if !current.is_empty() {
                hunks.push(current);
            }
            if hunks.is_empty() {
                hunks.push(Vec::new());
            }
            let binary =
                section.contains("GIT binary patch\n") || section.contains("Binary files ");
            let mode = header.windows(9).any(|bytes| bytes == b"old mode ")
                || header.windows(9).any(|bytes| bytes == b"new mode ");
            let kind = if binary {
                ChangeKind::Binary
            } else if mode && *kind == ChangeKind::Modified {
                ChangeKind::Mode
            } else {
                *kind
            };
            Ok(RawFile {
                header,
                hunks,
                old_path: old_path.clone(),
                path: path.clone(),
                kind,
                atomic: kind != ChangeKind::Modified,
                limitation: (kind == ChangeKind::Unsupported)
                    .then(|| "This Git change requires manual review and application.".into()),
            })
        })
        .collect()
}

fn selection(
    files: &[RawFile],
    ledger: &Ledger,
    file_ids: &[usize],
    hunk_ids: &[usize],
) -> Result<BTreeSet<usize>> {
    let file_ids = unique_ids(file_ids, "file")?;
    let hunk_ids = unique_ids(hunk_ids, "hunk")?;
    if file_ids.iter().any(|id| *id > files.len()) {
        anyhow::bail!("unknown file number");
    }
    let count = files.iter().map(|file| file.hunks.len()).sum::<usize>();
    if hunk_ids.iter().any(|id| *id > count) {
        anyhow::bail!("unknown hunk number");
    }
    let all = file_ids.is_empty() && hunk_ids.is_empty();
    let mut selected = BTreeSet::new();
    let mut hunk_id = 0;
    for (index, file) in files.iter().enumerate() {
        let mut chosen = Vec::new();
        for _ in &file.hunks {
            hunk_id += 1;
            if hunk_ids.contains(&hunk_id)
                || file_ids.contains(&(index + 1))
                || (all && !ledger.applied.contains(&hunk_id))
            {
                chosen.push(hunk_id);
            }
        }
        if chosen.is_empty() {
            continue;
        }
        if let Some(limitation) = &file.limitation {
            anyhow::bail!("{}: {limitation}", safe_text(&file.path.to_string_lossy()));
        }
        if chosen.iter().any(|id| ledger.applied.contains(id)) {
            anyhow::bail!("selected changes were already applied; choose remaining changes");
        }
        if file.atomic && chosen.len() != file.hunks.len() {
            anyhow::bail!("binary, rename, creation, deletion, and mode changes must be selected as a whole file");
        }
        if file.atomic && !all && !file_ids.contains(&(index + 1)) {
            anyhow::bail!("binary, rename, creation, deletion, and mode changes require a whole-file selector");
        }
        selected.extend(chosen);
    }
    Ok(selected)
}

fn unique_ids(ids: &[usize], kind: &str) -> Result<BTreeSet<usize>> {
    let set = ids.iter().copied().collect::<BTreeSet<_>>();
    if set.len() != ids.len() || set.contains(&0) {
        anyhow::bail!("{kind} numbers must be unique positive integers");
    }
    Ok(set)
}

fn selected_patch(files: &[RawFile], selected: &BTreeSet<usize>) -> Vec<u8> {
    let mut output = Vec::new();
    let mut id = 0;
    for file in files {
        let mut body = Vec::<u8>::new();
        let mut any = false;
        for hunk in &file.hunks {
            id += 1;
            if selected.contains(&id) {
                any = true;
                body.extend(hunk);
            }
        }
        if any {
            output.extend(&file.header);
            output.extend(body);
        }
    }
    output
}

fn source_stamp(repo: &Path, files: &[RawFile], deadline: Instant) -> Result<String> {
    let filters = configured_filters(repo, deadline)?;
    let mut digest = Sha256::new();
    let mut head = git_command(repo, &filters);
    digest.update(command_output(
        head.args(["rev-parse", "HEAD"]),
        None,
        4096,
        false,
        deadline,
    )?);
    let mut paths = BTreeSet::new();
    for file in files {
        paths.insert(file.path.clone());
        paths.extend(file.old_path.clone());
    }
    let mut index = git_command(repo, &filters);
    index.args(["ls-files", "--stage", "-z", "--"]).args(&paths);
    digest.update(command_output(
        &mut index, None, MAX_PATCH, false, deadline,
    )?);
    let mut remaining = MAX_PREIMAGES;
    for path in paths {
        validate_path(&path)?;
        validate_ancestors(repo, &path)?;
        digest.update((path.as_os_str().len() as u64).to_le_bytes());
        digest.update(path.to_string_lossy().as_bytes());
        let full = repo.join(&path);
        let metadata = match fs::symlink_metadata(&full) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                digest.update(b"missing");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            digest.update(b"symlink");
            digest.update(fs::read_link(&full)?.to_string_lossy().as_bytes());
            continue;
        }
        if metadata.is_dir() {
            digest.update(b"directory");
            continue;
        }
        if !metadata.is_file() || metadata.len() as usize > remaining {
            anyhow::bail!(
                "source preimages exceed the bounded review limit or contain a special file"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            digest.update(metadata.permissions().mode().to_le_bytes());
        }
        let mut options = OpenOptions::new();
        options.read(true);
        no_follow(&mut options);
        let mut bytes = Vec::new();
        options
            .open(&full)?
            .take(remaining as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > remaining || Instant::now() >= deadline {
            anyhow::bail!("source preimages exceed the bounded review limit");
        }
        remaining -= bytes.len();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn validate_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|part| match part {
            Component::Normal(part) => part.to_string_lossy().eq_ignore_ascii_case(".git"),
            _ => true,
        })
    {
        anyhow::bail!("unsafe task patch path");
    }
    Ok(())
}

fn validate_ancestors(root: &Path, path: &Path) -> Result<()> {
    let mut current = root.to_path_buf();
    let parts = path.components().collect::<Vec<_>>();
    for component in parts.iter().take(parts.len().saturating_sub(1)) {
        current.push(component.as_os_str());
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => (),
            Ok(_) => {
                anyhow::bail!("task path has a symlink or special-file ancestor; apply manually")
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn read_ledger(id: &str) -> Result<Ledger> {
    let path = super::task_dir(id)?.join("changes-v1.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Ledger::default()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() || metadata.len() as usize > MAX_LEDGER {
        anyhow::bail!("invalid task application journal");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    no_follow(&mut options);
    let mut bytes = Vec::new();
    options
        .open(path)?
        .take(MAX_LEDGER as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_LEDGER {
        anyhow::bail!("task application journal exceeds its limit");
    }
    let ledger: Ledger = serde_json::from_slice(&bytes)?;
    if ledger.applied.len() > MAX_HUNKS
        || ledger.applied.contains(&0)
        || (!ledger.patch_sha256.is_empty()
            && (ledger.patch_sha256.len() != 64
                || !ledger
                    .patch_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())))
    {
        anyhow::bail!("invalid task application journal");
    }
    Ok(ledger)
}

fn write_ledger(id: &str, ledger: &Ledger) -> Result<()> {
    let bytes = serde_json::to_vec(ledger)?;
    if bytes.len() > MAX_LEDGER {
        anyhow::bail!("task application journal exceeds its limit");
    }
    super::write_private(&super::task_dir(id)?.join("changes-v1.json"), &bytes)
}

fn source_lock(repo: &Path) -> Result<File> {
    let path = super::task_root()?.join(format!(
        "apply-{}.lock",
        digest(repo.canonicalize()?.to_string_lossy().as_bytes())
    ));
    let mut options = OpenOptions::new();
    options.create(true).write(true).truncate(false);
    no_follow(&mut options);
    let lock = options.open(&path)?;
    super::set_private(&path, 0o600);
    lock.lock_exclusive()?;
    Ok(lock)
}

fn configured_filters(repo: &Path, deadline: Instant) -> Result<Vec<String>> {
    let mut command = git_command(repo, &[]);
    let bytes = command_output(
        command.args([
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ]),
        None,
        64 * 1024,
        true,
        deadline,
    )?;
    let mut filters = BTreeSet::new();
    for key in bytes.split(|byte| *byte == 0).filter(|key| !key.is_empty()) {
        let key = std::str::from_utf8(key).context("invalid Git filter configuration")?;
        let driver = key
            .strip_prefix("filter.")
            .and_then(|key| key.rsplit_once('.').map(|(driver, _)| driver))
            .context("invalid Git filter name")?;
        if driver.len() > 256
            || !driver
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            anyhow::bail!("unsupported Git filter name");
        }
        filters.insert(driver.into());
    }
    Ok(filters.into_iter().collect())
}

fn git_command(repo: &Path, filters: &[String]) -> Command {
    let mut command = Command::new("git");
    command.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "diff.external=",
        "-c",
        "core.pager=cat",
        "-c",
        "diff.renames=true",
    ]);
    for filter in filters {
        for suffix in ["clean=", "smudge=", "process=", "required=false"] {
            command.arg("-c").arg(format!("filter.{filter}.{suffix}"));
        }
    }
    command
        .arg("-C")
        .arg(repo)
        .arg("--work-tree")
        .arg(repo)
        .env("GIT_OPTIONAL_LOCKS", "0");
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_NAMESPACE",
    ] {
        command.env_remove(name);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    command
}

pub(super) fn command_output(
    command: &mut Command,
    input: Option<&[u8]>,
    limit: usize,
    diff_exit: bool,
    deadline: Instant,
) -> Result<Vec<u8>> {
    if Instant::now() >= deadline {
        anyhow::bail!("bounded Git operation timed out");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().context("missing Git stdout")?;
    let stderr = child.stderr.take().context("missing Git stderr")?;
    let (sender, receiver) = std::sync::mpsc::channel();
    let readers: [(Box<dyn Read + Send>, bool); 2] =
        [(Box::new(stdout), false), (Box::new(stderr), true)];
    for (reader, is_error) in readers {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = reader
                .take(limit as u64 + 1)
                .read_to_end(&mut bytes)
                .map(|_| (is_error, bytes));
            let _ = sender.send(result);
        });
    }
    drop(sender);
    if let Some(input) = input {
        let mut stdin = child.stdin.take().context("missing Git stdin")?;
        let input = input.to_vec();
        std::thread::spawn(move || {
            let _ = stdin.write_all(&input);
        });
    }
    let mut output = Vec::new();
    let mut errors = Vec::new();
    for _ in 0..2 {
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok((is_error, bytes))) if bytes.len() <= limit => {
                if is_error {
                    errors = bytes;
                } else {
                    output = bytes;
                }
            }
            _ => {
                terminate(&mut child);
                anyhow::bail!("Git operation exceeded its time or output limit");
            }
        }
    }
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                terminate(&mut child);
                anyhow::bail!("Git operation timed out");
            }
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    };
    if !(status.success() || diff_exit && status.code() == Some(1)) {
        anyhow::bail!(
            "Git operation failed: {}",
            safe_text(&String::from_utf8_lossy(&errors))
        );
    }
    Ok(output)
}

fn terminate(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
}

fn safe_text(text: &str) -> String {
    crate::commands::display_safe(&crate::redact::redact(text))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
