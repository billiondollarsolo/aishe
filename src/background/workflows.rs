//! Private, parameterized task graphs. Templates are data, never shell scripts.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::{Budget, Record, State};
use crate::config::Config;

const MAX_TEMPLATE_BYTES: u64 = 256 * 1024;
const MAX_STAGES: usize = 32;
const MAX_PARAMETERS: usize = 32;
fn default_parallel() -> usize {
    2
}
fn default_scope() -> String {
    "workspace".into()
}
fn default_network() -> String {
    "deny".into()
}
fn default_budget() -> Budget {
    Budget {
        max_minutes: 30,
        max_provider_turns: 40,
        max_cost_usd: 0.0,
        max_tool_calls: 200,
        max_changed_files: 100,
        max_changed_bytes: 10 * 1024 * 1024,
        max_network_calls: 50,
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowParameter {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowStage {
    pub key: String,
    pub name: String,
    pub objective: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub required_checks: Vec<String>,
    #[serde(default = "default_budget")]
    pub budget: Budget,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default = "default_network")]
    pub network: String,
    #[serde(default)]
    pub connection: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<WorkflowParameter>,
    #[serde(default = "default_parallel")]
    pub max_parallel: usize,
    pub stages: Vec<WorkflowStage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageLink {
    pub run_id: String,
    pub stage_key: String,
    pub stage_name: String,
    pub dependencies: Vec<String>,
    pub required_checks: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowState {
    Running,
    Waiting,
    Completed,
    Blocked,
    Cancelled,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StageRun {
    pub key: String,
    pub name: String,
    pub task_id: String,
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub snapshot_head: Option<String>,
    #[serde(default)]
    pub snapshot_revision: u64,
    #[serde(default)]
    pub state: Option<State>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub source_cwd: PathBuf,
    pub source_repo: PathBuf,
    pub base_head: String,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub max_parallel: usize,
    pub state: WorkflowState,
    pub stages: Vec<StageRun>,
    #[serde(default)]
    pub scheduler_pid: Option<u32>,
    #[serde(default)]
    pub scheduler_process_start: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
}

impl WorkflowTemplate {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 || !valid_name(&self.name) {
            anyhow::bail!("workflow needs schema_version 1 and a simple name of 1..=64 letters, digits, - or _");
        }
        if !(1..=MAX_STAGES).contains(&self.stages.len()) || !(1..=8).contains(&self.max_parallel) {
            anyhow::bail!("workflow needs 1..=32 stages and max_parallel 1..=8");
        }
        if self.parameters.len() > MAX_PARAMETERS || self.description.len() > 8192 {
            anyhow::bail!("workflow description or parameters exceed the bound");
        }
        let mut parameters = BTreeSet::new();
        for parameter in &self.parameters {
            if !valid_name(&parameter.name)
                || !parameters.insert(parameter.name.as_str())
                || parameter.description.len() > 4096
                || parameter.default.as_ref().is_some_and(|v| v.len() > 8192)
            {
                anyhow::bail!("workflow parameter names must be unique and bounded");
            }
        }
        let mut stages = BTreeMap::new();
        for stage in &self.stages {
            if !valid_name(&stage.key)
                || stages.insert(stage.key.as_str(), stage).is_some()
                || stage.name.trim().is_empty()
                || stage.name.len() > 256
                || stage.objective.trim().is_empty()
                || stage.objective.len() > super::MAX_OBJECTIVE_BYTES / 2
                || stage
                    .required_checks
                    .iter()
                    .any(|c| c.trim().is_empty() || c.len() > 8192)
            {
                anyhow::bail!(
                    "workflow stage names, objectives, and checks must be unique and bounded"
                );
            }
            if crate::agent::ExecutionScope::parse(&stage.scope).is_none()
                || crate::agent::NetworkPolicy::parse(&stage.network).is_none()
            {
                anyhow::bail!("workflow stage {} has invalid scope or network", stage.key);
            }
            validate_budget(&stage.budget)?;
            if stage.required_checks.len() > 1 {
                anyhow::bail!("each stage accepts one required check command; combine checks with && or use dependent check stages");
            }
            if stage
                .required_checks
                .iter()
                .any(|check| check.contains("{{"))
            {
                anyhow::bail!(
                    "required checks are literal commands; parameters belong in stage objectives"
                );
            }
            for text in std::iter::once(&stage.objective) {
                for placeholder in placeholders(text)? {
                    if !parameters.contains(placeholder.as_str()) {
                        anyhow::bail!("unknown workflow parameter {placeholder}");
                    }
                }
            }
        }
        for stage in &self.stages {
            let mut seen = BTreeSet::new();
            for dependency in &stage.depends_on {
                if dependency == &stage.key
                    || !stages.contains_key(dependency.as_str())
                    || !seen.insert(dependency)
                {
                    anyhow::bail!(
                        "workflow stage {} has an invalid dependency {dependency}",
                        stage.key
                    );
                }
            }
        }
        let mut released = BTreeSet::new();
        loop {
            let before = released.len();
            for stage in &self.stages {
                if stage
                    .depends_on
                    .iter()
                    .all(|key| released.contains(key.as_str()))
                {
                    released.insert(stage.key.as_str());
                }
            }
            if released.len() == self.stages.len() {
                break;
            }
            if released.len() == before {
                anyhow::bail!("workflow dependencies contain a cycle");
            }
        }
        if serde_json::to_vec(self)?.len() > MAX_TEMPLATE_BYTES as usize {
            anyhow::bail!("workflow template exceeds 256 KiB");
        }
        Ok(())
    }
}

fn validate_budget(b: &Budget) -> Result<()> {
    if !(1..=1440).contains(&b.max_minutes)
        || !(1..=1000).contains(&b.max_provider_turns)
        || !b.max_cost_usd.is_finite()
        || b.max_cost_usd < 0.0
        || !(1..=10_000).contains(&b.max_tool_calls)
        || !(1..=10_000).contains(&b.max_network_calls)
        || !(1..=10_000).contains(&b.max_changed_files)
        || !(1..=1024 * 1024 * 1024).contains(&b.max_changed_bytes)
    {
        anyhow::bail!("workflow budget is invalid or exceeds task bounds");
    }
    Ok(())
}

fn placeholders(text: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut tail = text;
    while let Some((_, after)) = tail.split_once("{{") {
        let (key, rest) = after
            .split_once("}}")
            .context("unterminated workflow parameter")?;
        if !valid_name(key) {
            anyhow::bail!("invalid workflow parameter {key}");
        }
        result.push(key.into());
        tail = rest;
    }
    Ok(result)
}

fn substitute(text: &str, values: &BTreeMap<String, String>) -> Result<String> {
    // One pass deliberately avoids interpreting placeholder-like parameter values.
    let mut result = String::new();
    let mut tail = text;
    while let Some((before, after)) = tail.split_once("{{") {
        result.push_str(before);
        let (key, rest) = after
            .split_once("}}")
            .context("unterminated workflow parameter")?;
        result.push_str(
            values
                .get(key)
                .with_context(|| format!("missing workflow parameter {key}"))?,
        );
        tail = rest;
    }
    result.push_str(tail);
    if result.len() > super::MAX_OBJECTIVE_BYTES / 2 {
        anyhow::bail!("expanded workflow field is too long");
    }
    Ok(crate::redact::redact(&result))
}

pub fn save_template(template: &WorkflowTemplate) -> Result<()> {
    template.validate()?;
    let bytes = serde_json::to_vec_pretty(template)?;
    let redacted: WorkflowTemplate =
        serde_json::from_str(&crate::redact::redact(&String::from_utf8(bytes)?))?;
    redacted.validate()?;
    let path = template_path(&template.name)?;
    super::write_private(&path, &serde_json::to_vec_pretty(&redacted)?)
}

pub fn save_template_file(name: &str, path: &Path) -> Result<()> {
    if fs::metadata(path)?.len() > MAX_TEMPLATE_BYTES {
        anyhow::bail!("workflow file exceeds 256 KiB");
    }
    let bytes = fs::read(path)?;
    let mut template: WorkflowTemplate = if path.extension().is_some_and(|ext| ext == "toml") {
        toml::from_str(std::str::from_utf8(&bytes)?)?
    } else {
        serde_json::from_slice(&bytes)?
    };
    template.name = name.into();
    save_template(&template)
}

pub fn load_template(name: &str) -> Result<WorkflowTemplate> {
    let path = template_path(name)?;
    if fs::metadata(&path)?.len() > MAX_TEMPLATE_BYTES {
        anyhow::bail!("workflow template exceeds 256 KiB");
    }
    let template: WorkflowTemplate = serde_json::from_slice(&fs::read(path)?)?;
    template.validate()?;
    Ok(template)
}

pub fn list_templates() -> Result<Vec<WorkflowTemplate>> {
    let mut values = Vec::new();
    for entry in fs::read_dir(root()?.join("templates"))?
        .take(1024)
        .flatten()
    {
        if let Some(name) = entry.path().file_stem().and_then(|n| n.to_str()) {
            if let Ok(value) = load_template(name) {
                values.push(value);
            }
        }
    }
    values.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(values)
}

pub fn remove_template(name: &str) -> Result<()> {
    fs::remove_file(template_path(name)?)?;
    Ok(())
}

pub fn run_template(
    config: &Config,
    name: &str,
    parameters: BTreeMap<String, String>,
    no_isolation: bool,
) -> Result<WorkflowRun> {
    if no_isolation {
        anyhow::bail!(
            "workflows require isolated git worktrees; run an individual task for --no-isolation"
        );
    }
    let template = load_template(name)?;
    let mut values = BTreeMap::new();
    for parameter in &template.parameters {
        let value = parameters
            .get(&parameter.name)
            .cloned()
            .or_else(|| parameter.default.clone())
            .with_context(|| format!("provide --param {}=VALUE", parameter.name))?;
        if value.len() > 8192 {
            anyhow::bail!("workflow parameter {} exceeds 8192 bytes", parameter.name);
        }
        values.insert(parameter.name.clone(), value);
    }
    if parameters.keys().any(|key| !values.contains_key(key)) {
        anyhow::bail!("unknown workflow parameter");
    }
    let source_cwd = std::env::current_dir()?.canonicalize()?;
    let (source_repo, base_head, branch) = super::git_identity(&source_cwd)
        .context("workflows require a git repository for isolated stage snapshots")?;
    let id = super::new_id();
    let now = super::now_ms();
    let ids: BTreeMap<_, _> = template
        .stages
        .iter()
        .map(|s| (s.key.clone(), super::new_id()))
        .collect();
    let mut prepared = Vec::new();
    for stage in &template.stages {
        let mut selected = config.clone();
        if let Some(connection) = &stage.connection {
            selected.select_connection(connection)?;
        }
        if let Some(model) = &stage.model {
            selected.set_active_model(model.clone());
        }
        // A template may narrow current authority; it cannot silently widen it.
        if stage.scope == "host" && config.backend.default_scope != "host" {
            anyhow::bail!("workflow stage {} requests host scope; select host scope explicitly before running",stage.key);
        }
        if stage.network == "allow" && config.backend.workspace_network != "allow" {
            anyhow::bail!(
                "workflow stage {} requests network access; enable it explicitly before running",
                stage.key
            );
        }
        selected.backend.default_scope = stage.scope.clone();
        selected.backend.workspace_network = stage.network.clone();
        crate::policy::constrain(&mut selected)?;
        super::validate_host_admission(&selected, &stage.scope, &source_cwd)?;
        let task_id = ids[&stage.key].clone();
        let objective = substitute(&stage.objective, &values)?;
        let checks = stage.required_checks.clone();
        let dir = super::task_dir(&task_id)?;
        let record = Record {
            schema_version: 1,
            id: task_id,
            objective: objective.clone(),
            source_cwd: source_cwd.clone(),
            run_cwd: dir.join("worktree"),
            source_repo: Some(source_repo.clone()),
            worktree: Some(dir.join("worktree")),
            base_head: Some(base_head.clone()),
            source_branch: branch.clone(),
            created_at_ms: now,
            updated_at_ms: now,
            state: State::Blocked,
            result_revision: 0,
            mailbox: super::InteractionMailbox::default(),
            workflow: Some(StageLink {
                run_id: id.clone(),
                stage_key: stage.key.clone(),
                stage_name: stage.name.clone(),
                dependencies: stage
                    .depends_on
                    .iter()
                    .map(|key| ids[key].clone())
                    .collect(),
                required_checks: checks,
            }),
            foreground: false,
            workflow_frozen: false,
            native_task_id: None,
            engine: Some("native".into()),
            connection_id: selected.active_connection_id().into(),
            provider: selected.active_provider_name().into(),
            model: selected.active_model().into(),
            connection: crate::tasks::snapshot_connection(&selected),
            role: stage.key.clone(),
            scope: stage.scope.clone(),
            network: stage.network.clone(),
            elapsed_ms: 0,
            attempt_started_at_ms: None,
            steering_revision: 0,
            steering: Vec::new(),
            pid: None,
            process_start: None,
            exit_code: None,
            budget: stage.budget.clone(),
            plan: Vec::new(),
            plan_revision: 0,
            applied_hunks: Vec::new(),
            applied_patch_sha256: None,
            budget_exceeded: false,
            error: None,
        };
        prepared.push((record, objective));
    }
    let run = WorkflowRun {
        schema_version: 1,
        id: id.clone(),
        name: template.name,
        source_cwd,
        source_repo,
        base_head,
        created_at_ms: now,
        updated_at_ms: now,
        max_parallel: template.max_parallel,
        state: WorkflowState::Running,
        stages: template
            .stages
            .iter()
            .map(|stage| StageRun {
                key: stage.key.clone(),
                name: stage.name.clone(),
                task_id: ids[&stage.key].clone(),
                depends_on: stage.depends_on.clone(),
                snapshot_head: None,
                snapshot_revision: 0,
                state: Some(State::Blocked),
            })
            .collect(),
        scheduler_pid: None,
        scheduler_process_start: None,
        error: None,
    };
    // Publish a complete graph only after every immutable stage request exists.
    fs::create_dir_all(run_dir(&id)?)?;
    super::set_private(&run_dir(&id)?, 0o700);
    for (record, objective) in prepared {
        fs::create_dir_all(super::task_dir(&record.id)?)?;
        super::set_private(&super::task_dir(&record.id)?, 0o700);
        super::write_private(&super::request_path(&record.id)?, objective.as_bytes())?;
        super::save(&record)?;
        let _ = crate::tasks::timeline::append_background(
            &record.id,
            crate::tasks::timeline::EventKind::WorkflowQueued,
            &record.objective,
            "Waiting for workflow dependencies and a worker slot.",
            Some(crate::tasks::timeline::EventOutcome::Waiting),
        );
    }
    save_run(&run)?;
    super::scheduler::ensure_running(&id)?;
    Ok(run)
}

pub fn run_details(id: &str) -> Result<WorkflowRun> {
    let mut run = load_run(id)?;
    for stage in &mut run.stages {
        stage.state = super::load(&stage.task_id).ok().map(|r| r.state);
    }
    Ok(run)
}

pub fn list_runs() -> Result<Vec<WorkflowRun>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(root()?.join("runs"))?.take(4096).flatten() {
        if let Some(id) = entry.file_name().to_str() {
            if let Ok(run) = run_details(id) {
                result.push(run);
            }
        }
    }
    result.sort_by_key(|r| std::cmp::Reverse(r.created_at_ms));
    Ok(result)
}

pub fn resume_run(id: &str) -> Result<()> {
    if load_run(id)?.state == WorkflowState::Cancelled {
        anyhow::bail!("workflow {id} was cancelled; run a new workflow");
    }
    super::scheduler::ensure_running(id)
}

/// Graph slot admission and the background record claim use one lock order:
/// workflow journal first, then the individual record. Manual continuations
/// cannot exceed the same bound the detached scheduler uses.
pub(super) fn claim_slot(id: &str, claim: impl FnOnce() -> Result<()>) -> Result<()> {
    let record = super::load(id)?;
    let Some(link) = record.workflow else {
        return claim();
    };
    let path = run_dir(&link.run_id)?.join("record.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    super::set_private(&path, 0o600);
    lock.lock_exclusive()?;
    let run = load_run(&link.run_id)?;
    if run.state == WorkflowState::Cancelled {
        anyhow::bail!("workflow {} is cancelled", run.id);
    }
    let active = run
        .stages
        .iter()
        .filter(|stage| stage.task_id != id)
        .map(|stage| super::load(&stage.task_id))
        .collect::<Result<Vec<_>>>()?
        .iter()
        .filter(|record| {
            matches!(
                record.state,
                State::Starting | State::Running | State::Waiting
            )
        })
        .count();
    if active >= run.max_parallel {
        anyhow::bail!(
            "workflow {} has all {} worker slots reserved; resume after another stage stops",
            run.id,
            run.max_parallel
        );
    }
    claim()
}

pub fn cancel_run(id: &str) -> Result<()> {
    let mut run = load_run(id)?;
    update_run(id, |fresh| {
        fresh.state = WorkflowState::Cancelled;
        Ok(())
    })?;
    run.state = WorkflowState::Cancelled;
    for stage in run.stages {
        super::cancel(&stage.task_id)?;
    }
    Ok(())
}

pub(super) fn root() -> Result<PathBuf> {
    let root = crate::config::data_root()
        .context("no data directory is available")?
        .join("aishe/workflows");
    for path in [root.clone(), root.join("templates"), root.join("runs")] {
        fs::create_dir_all(&path)?;
        super::set_private(&path, 0o700);
    }
    Ok(root)
}
fn template_path(name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        anyhow::bail!("invalid workflow name");
    }
    Ok(root()?.join("templates").join(format!("{name}.json")))
}
pub(super) fn run_dir(id: &str) -> Result<PathBuf> {
    super::validate_id(id)?;
    Ok(root()?.join("runs").join(id))
}
pub(super) fn load_run(id: &str) -> Result<WorkflowRun> {
    let path = run_dir(id)?.join("record.json");
    if fs::metadata(&path)?.len() > 1024 * 1024 {
        anyhow::bail!("workflow run exceeds its bound");
    }
    let run: WorkflowRun = serde_json::from_slice(&fs::read(path)?)?;
    if run.schema_version != 1
        || run.id != id
        || !(1..=8).contains(&run.max_parallel)
        || run.stages.len() > MAX_STAGES
    {
        anyhow::bail!("invalid workflow run");
    }
    Ok(run)
}
pub(super) fn save_run(run: &WorkflowRun) -> Result<()> {
    super::write_private(
        &run_dir(&run.id)?.join("record.json"),
        &serde_json::to_vec_pretty(run)?,
    )
}

pub(super) fn update_run(
    id: &str,
    change: impl FnOnce(&mut WorkflowRun) -> Result<()>,
) -> Result<()> {
    let path = run_dir(id)?.join("record.lock");
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)?;
    super::set_private(&path, 0o600);
    lock.lock_exclusive()?;
    let mut run = load_run(id)?;
    change(&mut run)?;
    run.updated_at_ms = super::now_ms();
    save_run(&run)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn template() -> WorkflowTemplate {
        serde_json::from_value(serde_json::json!({"schema_version":1,"name":"test","parameters":[{"name":"target"}],"stages":[{"key":"build","name":"Build","objective":"Build {{target}}"},{"key":"check","name":"Check","objective":"Check","depends_on":["build"]}]})).unwrap()
    }
    #[test]
    fn rejects_cycles_and_unknown_dependencies() {
        let mut t = template();
        t.validate().unwrap();
        t.stages[0].depends_on.push("check".into());
        assert!(t.validate().is_err());
        t.stages[0].depends_on = vec!["missing".into()];
        assert!(t.validate().is_err());
    }
    #[test]
    fn substitution_is_literal_and_single_pass() {
        let values = BTreeMap::from([("target".into(), "$(touch /tmp/unsafe) {{other}}".into())]);
        assert_eq!(
            substitute("Work on {{target}}", &values).unwrap(),
            "Work on $(touch /tmp/unsafe) {{other}}"
        );
    }
    #[test]
    fn rejects_escaping_names_and_unbounded_budgets() {
        let mut t = template();
        t.name = "../other".into();
        assert!(t.validate().is_err());
        t.name = "valid".into();
        t.stages[0].budget.max_tool_calls = 0;
        assert!(t.validate().is_err());
    }
}
