//! Behavioral change-review tests against actual isolated Git worktrees.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use aishe::background::{apply_task_changes, task_change_review, ChangeKind};
use serde_json::{json, Value};

static ENV: Mutex<()> = Mutex::new(());

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    worktree: PathBuf,
    task_dir: PathBuf,
    id: String,
    saved: Vec<(&'static str, Option<OsString>)>,
    workers: Vec<Child>,
}

fn git(cwd: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        let serial = NEXT.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("aishe-selected-{}-{serial}", std::process::id()));
        let repo = root.join("repo");
        let id = format!("selected-{serial:08}");
        let task_dir = root.join("data/aishe/background-tasks").join(&id);
        let worktree = task_dir.join("worktree");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&task_dir).unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["config", "user.name", "Fixture"]);
        git(&repo, &["config", "user.email", "fixture@example.invalid"]);
        fs::write(repo.join("alpha.txt"), base_text()).unwrap();
        fs::write(repo.join("beta.txt"), "original beta\n").unwrap();
        fs::write(repo.join("binary.dat"), [0, 1, 2, 3, 255]).unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-qm", "base"]);
        let base = git(&repo, &["rev-parse", "HEAD"]);
        git(
            &repo,
            &[
                "worktree",
                "add",
                "--detach",
                "-q",
                worktree.to_str().unwrap(),
                &base,
            ],
        );
        let mut saved = Vec::new();
        for (name, value) in [
            ("AISHE_DATA_DIR", root.join("data")),
            ("AISHE_TASKS_DIR", root.join("native")),
        ] {
            saved.push((name, std::env::var_os(name)));
            std::env::set_var(name, value);
        }
        for name in [
            "AISHE_BACKGROUND_TASK_ID",
            "AISHE_TASK_MAX_MINUTES",
            "AISHE_TASK_MAX_TOOL_CALLS",
            "AISHE_TASK_MAX_PROVIDER_TURNS",
        ] {
            saved.push((name, std::env::var_os(name)));
            std::env::remove_var(name);
        }
        let record = json!({
            "schema_version":1,"id":id,"objective":"Review two files and record checks",
            "source_cwd":repo,"run_cwd":worktree,"source_repo":repo,"worktree":worktree,
            "base_head":base,"source_branch":null,"created_at_ms":1,"updated_at_ms":2,
            "state":"completed","result_revision":1,"pid":null,"process_start":null,
            "exit_code":0,"budget":{"max_minutes":5,"max_provider_turns":20,"max_cost_usd":1.0},
            "applied_patch_sha256":null,"error":null
        });
        fs::write(
            task_dir.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        Self {
            root,
            repo,
            worktree,
            task_dir,
            id,
            saved,
            workers: Vec::new(),
        }
    }

    fn record(&self) -> Value {
        serde_json::from_slice(&fs::read(self.task_dir.join("record.json")).unwrap()).unwrap()
    }

    fn edit_record(&self, edit: impl FnOnce(&mut Value)) {
        let mut record = self.record();
        edit(&mut record);
        fs::write(
            self.task_dir.join("record.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }

    fn two_files(&self) {
        fs::write(self.worktree.join("alpha.txt"), changed_text()).unwrap();
        fs::write(self.worktree.join("beta.txt"), "updated beta\n").unwrap();
    }

    fn live_worker(&mut self) -> (u32, String) {
        let child = Command::new("sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        self.workers.push(child);
        #[cfg(target_os = "linux")]
        let identity = {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
            let ticks = stat
                .rsplit_once(") ")
                .unwrap()
                .1
                .split_ascii_whitespace()
                .nth(19)
                .unwrap();
            format!("proc:{ticks}")
        };
        #[cfg(not(target_os = "linux"))]
        let identity = {
            let output = Command::new("ps")
                .args(["-o", "lstart=", "-p", &pid.to_string()])
                .env("TZ", "UTC")
                .env("LC_ALL", "C")
                .output()
                .unwrap();
            assert!(output.status.success());
            format!("utc:{}", String::from_utf8(output.stdout).unwrap().trim())
        };
        (pid, identity)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for worker in &mut self.workers {
            let _ = worker.kill();
            let _ = worker.wait();
        }
        for (name, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn base_text() -> String {
    (1..=40).map(|line| format!("line {line}\n")).collect()
}
fn changed_text() -> String {
    base_text()
        .replace("line 2\n", "changed two\n")
        .replace("line 32\n", "changed thirty-two\n")
}

#[test]
fn partial_application_changes_only_selected_hunks_and_leaves_remaining_reviewable() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    fixture.two_files();
    let review = task_change_review(&fixture.id).unwrap();
    assert_eq!(review.files.len(), 2);
    assert_eq!(review.files[0].hunks.len(), 2);
    let entry = aishe::background::task_details(&fixture.id).unwrap().entry;
    aishe::background::mark_task_reviewed(&entry).unwrap();
    let original_index = git(&fixture.repo, &["ls-files", "--stage"]);
    let original_head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let applied = apply_task_changes(&fixture.id, &review.revision, &[], &[1]).unwrap();
    assert_eq!(applied.remaining_hunks, 2);
    assert!(!applied.complete);
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        base_text().replace("line 2\n", "changed two\n")
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        "original beta\n"
    );
    assert_eq!(fixture.record()["state"], "completed");
    assert!(
        aishe::background::task_details(&fixture.id)
            .unwrap()
            .entry
            .reviewed
    );
    assert_eq!(git(&fixture.repo, &["ls-files", "--stage"]), original_index);
    assert_eq!(git(&fixture.repo, &["rev-parse", "HEAD"]), original_head);
    let remaining = task_change_review(&fixture.id).unwrap();
    assert!(remaining.files[0].hunks[0].applied);
    assert!(!remaining.files[0].hunks[1].applied);
    assert_ne!(remaining.revision, review.revision);
    assert!(apply_task_changes(&fixture.id, &review.revision, &[], &[2]).is_err());
    assert!(apply_task_changes(&fixture.id, &remaining.revision, &[], &[1]).is_err());
    apply_task_changes(&fixture.id, &remaining.revision, &[], &[2]).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        changed_text()
    );
    let last = task_change_review(&fixture.id).unwrap();
    apply_task_changes(&fixture.id, &last.revision, &[2], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        "updated beta\n"
    );
    assert_eq!(fixture.record()["state"], "applied");
    assert!(!task_change_review(&fixture.id).unwrap().can_apply);
}

#[test]
fn task_and_source_changes_invalidate_review_and_duplicate_unknown_selections_are_rejected() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut fixture = Fixture::new();
    fixture.two_files();
    let review = task_change_review(&fixture.id).unwrap();
    for ids in [&[0][..], &[99][..], &[1, 1][..]] {
        assert!(apply_task_changes(&fixture.id, &review.revision, &[], ids).is_err());
    }
    fs::write(fixture.repo.join("alpha.txt"), "user edit\n").unwrap();
    assert!(apply_task_changes(&fixture.id, &review.revision, &[2], &[]).is_err());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        "original beta\n"
    );
    fs::write(fixture.repo.join("alpha.txt"), base_text()).unwrap();
    let next = task_change_review(&fixture.id).unwrap();
    fs::write(fixture.worktree.join("beta.txt"), "task edited again\n").unwrap();
    assert!(apply_task_changes(&fixture.id, &next.revision, &[2], &[]).is_err());
    let (pid, identity) = fixture.live_worker();
    fixture.edit_record(|record| {
        record["state"] = "running".into();
        record["pid"] = pid.into();
        record["process_start"] = identity.into();
    });
    let live = task_change_review(&fixture.id).unwrap();
    assert!(!live.can_apply);
    assert!(apply_task_changes(&fixture.id, &live.revision, &[], &[]).is_err());
    assert_eq!(fixture.record()["state"], "running");
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        base_text()
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        "original beta\n"
    );
}

#[test]
fn binary_rename_and_untracked_creation_are_atomic_whole_file_selections() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    fs::write(fixture.worktree.join("binary.dat"), [0, 9, 8, 7, 255]).unwrap();
    fs::rename(
        fixture.worktree.join("beta.txt"),
        fixture.worktree.join("renamed.txt"),
    )
    .unwrap();
    git(&fixture.worktree, &["add", "beta.txt", "renamed.txt"]);
    fs::write(fixture.worktree.join("untracked.txt"), "new file\n").unwrap();
    let review = task_change_review(&fixture.id).unwrap();
    let binary = review
        .files
        .iter()
        .find(|file| file.kind == ChangeKind::Binary)
        .unwrap();
    let renamed = review
        .files
        .iter()
        .find(|file| file.kind == ChangeKind::Renamed)
        .unwrap();
    assert!(binary.hunks.iter().all(|hunk| !hunk.selectable));
    assert!(renamed.hunks.iter().all(|hunk| !hunk.selectable));
    assert!(apply_task_changes(&fixture.id, &review.revision, &[], &[binary.hunks[0].id]).is_err());
    apply_task_changes(&fixture.id, &review.revision, &[binary.id], &[]).unwrap();
    assert_eq!(
        fs::read(fixture.repo.join("binary.dat")).unwrap(),
        [0, 9, 8, 7, 255]
    );
    assert!(fixture.repo.join("beta.txt").exists());
    assert!(!fixture.repo.join("renamed.txt").exists());
    let remaining = task_change_review(&fixture.id).unwrap();
    apply_task_changes(&fixture.id, &remaining.revision, &[], &[]).unwrap();
    assert!(!fixture.repo.join("beta.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("renamed.txt")).unwrap(),
        "original beta\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("untracked.txt")).unwrap(),
        "new file\n"
    );
}

#[test]
fn recorded_check_results_are_observed_and_display_redaction_preserves_applied_bytes() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    let secret = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789";
    fs::write(fixture.worktree.join("beta.txt"), format!("{secret}\n")).unwrap();
    let seed = aishe::tasks::Active::start(
        &aishe::config::Config::default(),
        &fixture.worktree,
        "Run a recorded check",
    );
    // Integration harnesses deliberately disable automatic foreground task
    // persistence. Enter the durable continuation path explicitly so this
    // fixture exercises the real checkpoint writer without a background grant.
    let mut task = aishe::tasks::Active::resume(seed.record().clone());
    let command = "test -s beta.txt";
    task.begin_execution_evidence(
        "check-call",
        command,
        &fixture.worktree,
        aishe::tasks::ExecutionKind::Check,
    )
    .unwrap();
    let actual = Command::new("sh")
        .args(["-c", command])
        .current_dir(&fixture.worktree)
        .output()
        .unwrap();
    task.finish_execution_evidence("check-call", actual.status.code().unwrap(), "", 1, false)
        .unwrap();
    assert_eq!(
        aishe::tasks::load(task.id())
            .unwrap()
            .check_summary()
            .passed,
        1
    );
    fixture.edit_record(|record| record["native_task_id"] = task.id().into());
    let review = task_change_review(&fixture.id).unwrap();
    assert_eq!(review.check_summary.passed, 1);
    assert_eq!(review.checks[0].exit_code, Some(0));
    assert_eq!(review.checks[0].command, command);
    assert!(!serde_json::to_string(&review).unwrap().contains(secret));
    apply_task_changes(&fixture.id, &review.revision, &[], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        format!("{secret}\n")
    );
}

#[test]
fn interrupted_application_journal_and_modified_patch_after_partial_apply_block_replay() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    fixture.two_files();
    let review = task_change_review(&fixture.id).unwrap();
    apply_task_changes(&fixture.id, &review.revision, &[], &[1]).unwrap();
    fs::write(fixture.worktree.join("beta.txt"), "new task content\n").unwrap();
    let changed = task_change_review(&fixture.id).unwrap();
    assert!(!changed.can_apply);
    assert!(changed
        .unresolved
        .iter()
        .any(|issue| issue.contains("changed after")));
    assert!(apply_task_changes(&fixture.id, &changed.revision, &[], &[]).is_err());
    fixture.two_files();
    let path = fixture.task_dir.join("changes-v1.json");
    let mut ledger: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ledger["pending"] = json!({"hunks":[2],"selection_sha256":"unknown","source_before":"unknown"});
    fs::write(path, serde_json::to_vec(&ledger).unwrap()).unwrap();
    let uncertain = task_change_review(&fixture.id).unwrap();
    assert!(!uncertain.can_apply);
    assert!(uncertain
        .unresolved
        .iter()
        .any(|issue| issue.contains("uncertain")));
    assert!(apply_task_changes(&fixture.id, &uncertain.revision, &[], &[]).is_err());
}

#[test]
fn external_diff_textconv_filters_and_fsmonitor_never_run_during_review_or_apply() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    fixture.two_files();
    let marker = fixture.root.join("helper-ran");
    let helper = format!("sh -c 'touch {}; exit 1'", marker.display());
    for (name, value) in [
        ("diff.external", helper.as_str()),
        ("diff.spy.textconv", helper.as_str()),
        ("filter.spy.clean", helper.as_str()),
        ("filter.spy.smudge", helper.as_str()),
        ("filter.spy.process", helper.as_str()),
        ("core.fsmonitor", helper.as_str()),
    ] {
        git(&fixture.repo, &["config", name, value]);
    }
    fs::write(
        fixture.repo.join(".git/info/attributes"),
        "*.txt filter=spy diff=spy\n",
    )
    .unwrap();
    let review = task_change_review(&fixture.id).unwrap();
    assert!(!marker.exists());
    apply_task_changes(&fixture.id, &review.revision, &[], &[]).unwrap();
    assert!(!marker.exists());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        changed_text()
    );
}

#[test]
fn workflow_leaf_reviews_and_applies_cumulative_dependency_changes() {
    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let fixture = Fixture::new();
    let source_base = git(&fixture.repo, &["rev-parse", "HEAD"]);
    fs::write(fixture.worktree.join("alpha.txt"), changed_text()).unwrap();
    git(&fixture.worktree, &["add", "alpha.txt"]);
    git(
        &fixture.worktree,
        &["commit", "-qm", "implementation snapshot"],
    );
    let dependency_base = git(&fixture.worktree, &["rev-parse", "HEAD"]);
    fs::write(fixture.worktree.join("beta.txt"), "review-stage edit\n").unwrap();
    let run_id = "workflow-00000001";
    let run_dir = fixture.root.join("data/aishe/workflows/runs").join(run_id);
    fs::create_dir_all(&run_dir).unwrap();
    fs::write(
        run_dir.join("record.json"),
        serde_json::to_vec(&json!({
            "schema_version":1,"id":run_id,"name":"implement-test-review",
            "source_cwd":fixture.repo,"source_repo":fixture.repo,"base_head":source_base,
            "created_at_ms":1,"updated_at_ms":2,"max_parallel":1,"state":"completed",
            "stages":[{"key":"review","name":"Review","task_id":fixture.id,"depends_on":[]}]
        }))
        .unwrap(),
    )
    .unwrap();
    fixture.edit_record(|record| {
        record["base_head"] = dependency_base.into();
        record["workflow"] = json!({"run_id":run_id,"stage_key":"review","stage_name":"Review","dependencies":[],"required_checks":[]});
    });
    let review = task_change_review(&fixture.id).unwrap();
    assert_eq!(
        review.files.len(),
        2,
        "implementation dependency disappeared from leaf review"
    );
    apply_task_changes(&fixture.id, &review.revision, &[], &[]).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        changed_text()
    );
    assert_eq!(
        fs::read_to_string(fixture.repo.join("beta.txt")).unwrap(),
        "review-stage edit\n"
    );
    assert_eq!(git(&fixture.repo, &["rev-parse", "HEAD"]), source_base);
}

#[test]
#[cfg(unix)]
fn failure_after_application_starts_becomes_durable_attention_and_never_replays() {
    use std::os::unix::fs::PermissionsExt;

    let _env = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut fixture = Fixture::new();
    fixture.two_files();
    let inspected = aishe::background::task_details(&fixture.id).unwrap().entry;
    aishe::background::mark_task_reviewed(&inspected).unwrap();
    let review = task_change_review(&fixture.id).unwrap();
    let bin = fixture.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let wrapper = bin.join("git");
    fs::write(&wrapper, "#!/bin/sh\napply=no\ncheck=no\nfor argument do\n  case \"$argument\" in apply) apply=yes;; --check) check=yes;; esac\ndone\nif [ \"$apply\" = yes ] && [ \"$check\" = no ]; then\n  printf '%s\\n' 'fixture application result missing' >&2\n  exit 23\nfi\nexec /usr/bin/git \"$@\"\n").unwrap();
    fs::set_permissions(wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    fixture.saved.push(("PATH", std::env::var_os("PATH")));
    std::env::set_var(
        "PATH",
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap(),
    );
    assert!(apply_task_changes(&fixture.id, &review.revision, &[], &[1]).is_err());
    assert_eq!(fixture.record()["state"], "failed");
    assert!(fixture.record()["error"]
        .as_str()
        .unwrap()
        .contains("uncertain"));
    let details = aishe::background::task_details(&fixture.id).unwrap();
    assert!(!details.entry.reviewed);
    assert_eq!(
        details.entry.attention,
        aishe::background::TaskAttention::Attention
    );
    let uncertain = task_change_review(&fixture.id).unwrap();
    assert!(!uncertain.can_apply);
    assert!(apply_task_changes(&fixture.id, &uncertain.revision, &[], &[1]).is_err());
    assert_eq!(
        fs::read_to_string(fixture.repo.join("alpha.txt")).unwrap(),
        base_text()
    );
}
