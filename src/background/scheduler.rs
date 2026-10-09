//! One detached, restart-safe orchestrator per workflow. Workers still own all
//! model/tool effects and normal admission; the scheduler only creates isolated
//! Git snapshots and releases dependencies.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;

use super::workflows::{self, WorkflowRun, WorkflowState};
use super::{Record, State};
use crate::config::Config;

/// Called by a shell's existing cache watcher, at most once a minute. Recovery
/// wakes the durable orchestrator and never reconstructs interrupted effects.
pub fn wake_pending_workflows() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_SCAN: AtomicU64 = AtomicU64::new(0);
    let now = super::now_ms().min(u128::from(u64::MAX)) as u64;
    let previous = LAST_SCAN.load(Ordering::Relaxed);
    if now.saturating_sub(previous) < 60_000
        || LAST_SCAN
            .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
    {
        return;
    }
    if let Ok(runs) = workflows::list_runs() {
        for run in runs
            .into_iter()
            .filter(|run| matches!(run.state, WorkflowState::Running | WorkflowState::Waiting))
            .take(64)
        {
            if !run
                .scheduler_pid
                .is_some_and(|pid| super::same_process(pid, run.scheduler_process_start.as_deref()))
            {
                let _ = ensure_running(&run.id);
            }
        }
    }
}

pub fn ensure_running(id: &str) -> Result<()> {
    let run = workflows::load_run(id)?;
    if run.state == WorkflowState::Cancelled {
        return Ok(());
    }
    let path = workflows::run_dir(id)?.join("scheduler.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    super::set_private(&path, 0o600);
    if lock.try_lock_exclusive().is_err() {
        return Ok(());
    }
    let log_path = workflows::run_dir(id)?.join("scheduler.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    super::set_private(&log_path, 0o600);
    let mut child = Command::new(std::env::current_exe()?);
    let config = Config::load_quiet()?.unwrap_or_default();
    super::restrict_background_environment(&mut child, &config);
    child
        .args(["--background-workflow", id])
        .current_dir(&run.source_cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    super::detach_worker(&mut child);
    // Release before spawn: the child's lock arbitrates duplicate wakeups.
    FileExt::unlock(&lock)?;
    child.spawn().context("starting workflow scheduler")?;
    Ok(())
}

pub fn run(config: &Config, id: &str) -> Result<u8> {
    let path = workflows::run_dir(id)?.join("scheduler.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    super::set_private(&path, 0o600);
    if lock.try_lock_exclusive().is_err() {
        return Ok(0);
    }
    let mut run = workflows::load_run(id)?;
    run.scheduler_pid = Some(std::process::id());
    run.scheduler_process_start = super::process_start(std::process::id());
    workflows::update_run(id, |fresh| {
        if fresh.state != WorkflowState::Cancelled {
            fresh.scheduler_pid = run.scheduler_pid;
            fresh.scheduler_process_start = run.scheduler_process_start.clone();
        }
        Ok(())
    })?;
    loop {
        // Cancellation is an external durable command; reload it before every
        // release, and never save our older Running snapshot over it.
        let fresh = workflows::load_run(id)?;
        if fresh.state == WorkflowState::Cancelled {
            break;
        }
        run = fresh;
        tick(config, &mut run)?;
        if !matches!(run.state, WorkflowState::Running | WorkflowState::Waiting) {
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // Do not overwrite state after releasing work; only the owner clears PID.
    workflows::update_run(id, |fresh| {
        if fresh.scheduler_pid == Some(std::process::id()) {
            fresh.scheduler_pid = None;
            fresh.scheduler_process_start = None;
        }
        Ok(())
    })?;
    Ok(0)
}

fn tick(config: &Config, run: &mut WorkflowRun) -> Result<()> {
    let mut records = BTreeMap::new();
    for stage in &run.stages {
        let mut record = super::load(&stage.task_id)?;
        super::reconcile(&mut record)?;
        records.insert(stage.key.clone(), record);
    }
    // Capture completed results exactly once. A checkpoint with missing,
    // failed, uncertain or stale required evidence never releases successors.
    for stage in &mut run.stages {
        let record = &records[&stage.key];
        if !matches!(record.state, State::Completed | State::Applied)
            || (record.state == State::Completed
                && stage.snapshot_revision != record.result_revision)
        {
            stage.snapshot_head = None;
        }
        if matches!(record.state, State::Completed | State::Applied)
            && stage.snapshot_head.is_none()
        {
            let result = required_evidence(record).and_then(|_| {
                super::update(&record.id, |fresh| {
                    if !matches!(fresh.state, State::Completed | State::Applied)
                        || fresh.result_revision != record.result_revision
                    {
                        anyhow::bail!("stage changed before its dependency snapshot");
                    }
                    fresh.workflow_frozen = true;
                    Ok(())
                })?;
                snapshot(record)
            });
            match result {
                Ok(head) => {
                    stage.snapshot_head = Some(head);
                    stage.snapshot_revision = record.result_revision;
                }
                Err(error) => {
                    let detail = crate::redact::redact(&error.to_string());
                    run.error = Some(detail.clone());
                    super::update(&record.id, |fresh| {
                        fresh.workflow_frozen = false;
                        if fresh.state == State::Completed {
                            fresh.state = State::Failed;
                            fresh.exit_code = Some(1);
                            fresh.error = Some(detail.clone());
                        } else if fresh.state == State::Applied {
                            fresh.error = Some(detail.clone());
                        }
                        Ok(())
                    })?;
                    records.insert(stage.key.clone(), super::load(&stage.task_id)?);
                }
            }
        }
    }
    let mut active = records
        .values()
        .filter(|r| matches!(r.state, State::Starting | State::Running | State::Waiting))
        .count();
    let available = run
        .stages
        .iter()
        .filter(|s| matches!(records[&s.key].state, State::Completed | State::Applied))
        .filter_map(|s| {
            s.snapshot_head
                .as_ref()
                .map(|head| (s.key.clone(), head.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    for stage in &run.stages {
        let record = &records[&stage.key];
        if record.state != State::Blocked {
            continue;
        }
        let failed = stage.depends_on.iter().find_map(|key| {
            let parent = &records[key];
            matches!(
                parent.state,
                State::Failed | State::Cancelled | State::Interrupted | State::Discarded
            )
            .then(|| {
                format!(
                    "dependency {} is {:?}; resolve or resume that stage before continuing",
                    parent.id, parent.state
                )
            })
        });
        if let Some(detail) = failed {
            set_blocked(&record.id, &detail)?;
            continue;
        }
        if active >= run.max_parallel
            || !stage
                .depends_on
                .iter()
                .all(|key| available.contains_key(key))
        {
            continue;
        }
        // Check the cancellation journal immediately before preparing/claiming.
        if workflows::load_run(&run.id)?.state == WorkflowState::Cancelled {
            return Ok(());
        }
        let heads = stage
            .depends_on
            .iter()
            .map(|key| available[key].clone())
            .collect::<Vec<_>>();
        let result = prepare_stage(run, record, &heads, &records)
            .and_then(|_| super::launch_stage(config, &record.id));
        if let Err(error) = result {
            super::update(&record.id, |fresh| {
                if matches!(fresh.state, State::Blocked | State::Starting) {
                    fresh.state = State::Blocked;
                    fresh.attempt_started_at_ms = None;
                    fresh.error = Some(crate::redact::redact(&error.to_string()));
                }
                Ok(())
            })?;
        } else {
            active += 1;
        }
    }
    for stage in &mut run.stages {
        stage.state = Some(super::load(&stage.task_id)?.state);
    }
    let completed = run.stages.iter().all(|s| s.snapshot_head.is_some());
    let states = run
        .stages
        .iter()
        .filter_map(|s| s.state)
        .collect::<Vec<_>>();
    run.state = if completed {
        WorkflowState::Completed
    } else if run.stages.iter().any(|s| {
        matches!(s.state, Some(State::Starting | State::Running))
            || (s.state == Some(State::Completed) && s.snapshot_head.is_none())
    }) {
        WorkflowState::Running
    } else if states.contains(&State::Waiting) {
        WorkflowState::Waiting
    } else {
        WorkflowState::Blocked
    };
    run.updated_at_ms = super::now_ms();
    // A user can cancel while Git builds a snapshot. Preserve that decision.
    workflows::update_run(&run.id, |fresh| {
        if fresh.state != WorkflowState::Cancelled {
            fresh.state = run.state;
            fresh.stages = run.stages.clone();
            fresh.error = run.error.clone();
        }
        Ok(())
    })?;
    Ok(())
}

fn set_blocked(id: &str, detail: &str) -> Result<()> {
    let snapshot = super::load(id)?;
    if snapshot.error.as_deref() == Some(detail) {
        return Ok(());
    }
    super::update(id, |record| {
        if record.state == State::Blocked {
            record.error = Some(detail.into());
        }
        Ok(())
    })
}

pub(super) fn required_evidence(record: &Record) -> Result<()> {
    let Some(link) = &record.workflow else {
        return Ok(());
    };
    if link.required_checks.is_empty() {
        return Ok(());
    }
    let native = crate::tasks::load(
        record
            .native_task_id
            .as_deref()
            .context("required checks have no native checkpoint")?,
    )?;
    for required in &link.required_checks {
        let observed = native
            .evidence
            .iter()
            .rev()
            .find(|e| e.kind == crate::tasks::ExecutionKind::Check && e.command == *required);
        if !observed.is_some_and(|e| {
            e.outcome == crate::tasks::ExecutionOutcome::Passed
                && !e.stale(native.workspace_revision)
        }) {
            anyhow::bail!(
                "required check has no fresh recorded pass: {}",
                crate::commands::display_safe(required)
            );
        }
    }
    Ok(())
}

fn prepare_stage(
    run: &WorkflowRun,
    record: &Record,
    heads: &[String],
    records: &BTreeMap<String, Record>,
) -> Result<()> {
    let mut base = if let Some(first) = heads.first() {
        first.clone()
    } else {
        run.base_head.clone()
    };
    for other in heads.iter().skip(1) {
        if other == &base {
            continue;
        }
        let tree = git_output(
            &run.source_repo,
            &["merge-tree", "--write-tree", &base, other],
            None,
        )?;
        let tree = std::str::from_utf8(&tree)?
            .lines()
            .next()
            .context("dependency merge produced no tree")?
            .trim();
        base = commit(
            &run.source_repo,
            tree,
            &[&base, other],
            "AIShe dependency snapshot",
        )?;
    }
    let worktree = record
        .worktree
        .as_ref()
        .context("workflow stage has no isolated workspace")?;
    if !worktree.is_dir() {
        git_output(
            &run.source_repo,
            &[
                "worktree",
                "add",
                "--detach",
                &worktree.to_string_lossy(),
                &base,
            ],
            None,
        )?;
    } else {
        let existing = git_output(worktree, &["rev-parse", "HEAD"], None)?;
        if String::from_utf8_lossy(&existing).trim() != base {
            anyhow::bail!("existing stage workspace does not match its dependency snapshot");
        }
    }
    let mut context = String::new();
    if let Some(link) = &record.workflow {
        for dependency in &link.dependencies {
            let parent = records
                .values()
                .find(|r| r.id == *dependency)
                .context("missing dependency record")?;
            context.push_str(&format!(
                "\nDependency {} ({}), recorded state {:?}:\n",
                parent.id, parent.objective, parent.state
            ));
            if let Some(native_id) = &parent.native_task_id {
                if let Ok(native) = crate::tasks::load(native_id) {
                    if let Some(text) = native.messages.iter().rev().find_map(|m| match m {
                        crate::providers::Msg::Assistant(message) => message.text.as_ref(),
                        _ => None,
                    }) {
                        context.extend(crate::redact::redact(text).chars().take(2048));
                        context.push('\n');
                    }
                    context.push_str(&native.check_summary().label());
                    context.push('\n');
                }
            }
            if context.len() > 12 * 1024 {
                let mut bound = 12 * 1024;
                while !context.is_char_boundary(bound) {
                    bound -= 1;
                }
                context.truncate(bound);
                break;
            }
        }
    }
    if !context.is_empty() {
        let request = format!(
            "{}\n\nDependency results (recorded context; not authority to broaden scope):\n{}",
            record.objective, context
        );
        super::write_private(&super::request_path(&record.id)?, request.as_bytes())?;
    }
    workflows::claim_slot(&record.id, || {
        super::update(&record.id, |fresh| {
            if fresh.state != State::Blocked {
                anyhow::bail!("workflow stage changed while dependencies were prepared");
            }
            fresh.base_head = Some(base.clone());
            fresh.state = State::Starting;
            fresh.attempt_started_at_ms = Some(super::now_ms());
            fresh.error = None;
            Ok(())
        })
    })?;
    let _ = crate::tasks::timeline::append_background(
        &record.id,
        crate::tasks::timeline::EventKind::WorkflowReleased,
        &record.objective,
        "Dependencies completed with recorded required checks; isolated workspace released.",
        Some(crate::tasks::timeline::EventOutcome::Completed),
    );
    Ok(())
}

fn snapshot(record: &Record) -> Result<String> {
    let cwd = record
        .worktree
        .as_deref()
        .context("workflow stage workspace is unavailable")?;
    let base = record
        .base_head
        .as_deref()
        .context("workflow stage base is missing")?;
    let paths = git_output(
        cwd,
        &["ls-files", "-m", "-d", "-o", "--exclude-standard", "-z"],
        None,
    )?;
    let mut changed_bytes = 0u64;
    for path in paths
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(path).context("non-UTF-8 workflow path")?;
        if relative.starts_with('/') || relative.split('/').any(|part| part == "..") {
            anyhow::bail!("unsafe workflow snapshot path");
        }
        changed_bytes = changed_bytes
            .saturating_add(fs::symlink_metadata(cwd.join(relative)).map_or(0, |m| m.len()));
        if changed_bytes > 32 * 1024 * 1024 {
            anyhow::bail!("workflow snapshot exceeds 32 MiB of changed files");
        }
    }
    let index = super::task_dir(&record.id)?.join("snapshot.index");
    // An alternate index leaves the worker's own Git staging untouched. Clean
    // filters, merge drivers, hooks and fsmonitor are disabled by git_command.
    let operation = (|| {
        let mut read = git_command(cwd)?;
        read.env("GIT_INDEX_FILE", &index).args(["read-tree", base]);
        output(read)?;
        let mut add = git_command(cwd)?;
        add.env("GIT_INDEX_FILE", &index)
            .args(["add", "-A", "--", "."]);
        output(add)?;
        let mut tree = git_command(cwd)?;
        tree.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = String::from_utf8(output(tree)?)?;
        let head = commit(cwd, tree.trim(), &[base], "AIShe completed stage snapshot")?;
        let link = record
            .workflow
            .as_ref()
            .context("workflow snapshot has no owner")?;
        let reference = format!("refs/aishe/workflows/{}/{}", link.run_id, link.stage_key);
        git_output(cwd, &["update-ref", &reference, &head], None)?;
        Ok(head)
    })();
    let _ = fs::remove_file(index);
    operation
}

fn commit(repo: &Path, tree: &str, parents: &[&str], message: &str) -> Result<String> {
    if !is_object_id(tree) || parents.iter().any(|p| !is_object_id(p)) {
        anyhow::bail!("invalid workflow Git snapshot identity");
    }
    let mut command = git_command(repo)?;
    command.args(["commit-tree", tree]);
    for parent in parents {
        command.args(["-p", parent]);
    }
    command
        .args(["-m", message])
        .env("GIT_AUTHOR_NAME", "AIShe workflow")
        .env("GIT_AUTHOR_EMAIL", "workflow@aishe.local")
        .env("GIT_COMMITTER_NAME", "AIShe workflow")
        .env("GIT_COMMITTER_EMAIL", "workflow@aishe.local");
    Ok(String::from_utf8(output(command)?)?.trim().into())
}
fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|c| c.is_ascii_hexdigit())
}

fn git_command(repo: &Path) -> Result<Command> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "commit.gpgSign=false",
            "-c",
            "diff.external=",
        ])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env_remove("GIT_EXTERNAL_DIFF");
    let mut query = Command::new("git");
    query.arg("-C").arg(repo).args([
        "-c",
        "core.fsmonitor=false",
        "config",
        "--name-only",
        "--get-regexp",
        r"^(filter\..*\.(clean|smudge|process|required)|merge\..*\.driver)$",
    ]);
    clean_git_environment(&mut query);
    let config = super::changes::command_output(
        &mut query,
        None,
        64 * 1024,
        true,
        Instant::now() + Duration::from_secs(10),
    )?;
    let config = std::str::from_utf8(&config).context("invalid Git helper names")?;
    if config.lines().count() > 512 {
        anyhow::bail!("too many configured Git helpers");
    }
    for key in config.lines() {
        if key.len() > 512
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
        {
            anyhow::bail!("invalid configured Git helper");
        }
        let value = if key.ends_with(".required") || key.starts_with("merge.") {
            "false"
        } else {
            ""
        };
        command.arg("-c").arg(format!("{key}={value}"));
    }
    clean_git_environment(&mut command);
    Ok(command)
}

fn clean_git_environment(command: &mut Command) {
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_CONFIG_") {
            command.env_remove(name);
        }
    }
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
    command
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
}

fn git_output(repo: &Path, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut command = git_command(repo)?;
    command.args(args);
    super::changes::command_output(
        &mut command,
        input,
        1024 * 1024,
        false,
        Instant::now() + Duration::from_secs(10),
    )
}

fn output(mut command: Command) -> Result<Vec<u8>> {
    super::changes::command_output(
        &mut command,
        None,
        1024 * 1024,
        false,
        Instant::now() + Duration::from_secs(10),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Repo(std::path::PathBuf);
    impl Repo {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "aishe-workflow-snapshots-{}",
                super::super::new_id()
            ));
            fs::create_dir_all(&path).unwrap();
            git_output(&path, &["init", "."], None).unwrap();
            Self(path)
        }
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tree_with_file(
        repo: &Repo,
        base: &str,
        name: &str,
        value: &[u8],
        index_name: &str,
    ) -> String {
        let index = repo.0.join(index_name);
        let mut read = git_command(&repo.0).unwrap();
        read.env("GIT_INDEX_FILE", &index).args(["read-tree", base]);
        output(read).unwrap();
        let blob = String::from_utf8(
            git_output(&repo.0, &["hash-object", "-w", "--stdin"], Some(value)).unwrap(),
        )
        .unwrap();
        let mut change = git_command(&repo.0).unwrap();
        change.env("GIT_INDEX_FILE", &index).args([
            "update-index",
            "--add",
            "--cacheinfo",
            "100644",
            blob.trim(),
            name,
        ]);
        output(change).unwrap();
        let mut tree = git_command(&repo.0).unwrap();
        tree.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = String::from_utf8(output(tree).unwrap()).unwrap();
        commit(&repo.0, tree.trim(), &[base], "fixture stage").unwrap()
    }

    #[test]
    fn independent_stage_snapshots_merge_without_changing_source_and_conflicts_fail_closed() {
        let repo = Repo::new();
        fs::write(repo.0.join("first.txt"), "original first\n").unwrap();
        fs::write(repo.0.join("second.txt"), "original second\n").unwrap();
        git_output(&repo.0, &["add", "-A"], None).unwrap();
        let tree = String::from_utf8(git_output(&repo.0, &["write-tree"], None).unwrap()).unwrap();
        let base = commit(&repo.0, tree.trim(), &[], "fixture source").unwrap();
        git_output(&repo.0, &["update-ref", "HEAD", &base], None).unwrap();
        let first = tree_with_file(&repo, &base, "first.txt", b"changed first\n", "first.index");
        let second = tree_with_file(
            &repo,
            &base,
            "second.txt",
            b"changed second\n",
            "second.index",
        );
        let merged = String::from_utf8(
            git_output(
                &repo.0,
                &["merge-tree", "--write-tree", &first, &second],
                None,
            )
            .unwrap(),
        )
        .unwrap();
        let merged = commit(
            &repo.0,
            merged.lines().next().unwrap(),
            &[&first, &second],
            "fixture merge",
        )
        .unwrap();
        assert_eq!(
            git_output(&repo.0, &["show", &format!("{merged}:first.txt")], None).unwrap(),
            b"changed first\n"
        );
        assert_eq!(
            git_output(&repo.0, &["show", &format!("{merged}:second.txt")], None).unwrap(),
            b"changed second\n"
        );
        assert_eq!(
            String::from_utf8(git_output(&repo.0, &["rev-parse", "HEAD"], None).unwrap())
                .unwrap()
                .trim(),
            base
        );
        assert_eq!(
            fs::read(repo.0.join("first.txt")).unwrap(),
            b"original first\n"
        );
        let conflict = tree_with_file(
            &repo,
            &base,
            "first.txt",
            b"conflicting first\n",
            "conflict.index",
        );
        assert!(git_output(
            &repo.0,
            &["merge-tree", "--write-tree", &first, &conflict],
            None
        )
        .is_err());
    }

    #[test]
    fn workflow_snapshot_disables_repository_clean_filters() {
        let repo = Repo::new();
        let witness = repo.0.join("unexpected-filter-effect");
        git_output(
            &repo.0,
            &[
                "config",
                "filter.fixture.clean",
                &format!("touch {}; cat", witness.display()),
            ],
            None,
        )
        .unwrap();
        git_output(
            &repo.0,
            &["config", "filter.fixture.required", "true"],
            None,
        )
        .unwrap();
        fs::write(repo.0.join(".gitattributes"), "*.txt filter=fixture\n").unwrap();
        fs::write(repo.0.join("source.txt"), "literal source\n").unwrap();
        git_output(&repo.0, &["add", "-A"], None).unwrap();
        assert!(!witness.exists());
        let value = git_output(&repo.0, &["show", ":source.txt"], None).unwrap();
        assert_eq!(value, b"literal source\n");
    }

    #[test]
    fn snapshot_ids_cannot_inject_git_options() {
        assert!(!is_object_id("--exec=bad"));
        assert!(is_object_id(&"a".repeat(40)));
        assert!(!is_object_id(&"x".repeat(40)));
    }
}
