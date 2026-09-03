use anyhow::Result;

use crate::config::Config;
use crate::executor::Executor;
use crate::providers;
use crate::safety::{self, Risk};
use crate::ui::SemanticStylize;

/// Parsed semantic-history action transferred from the binary's Clap surface.
#[derive(Clone, Debug)]
pub enum Action {
    Search {
        query: Vec<String>,
        limit: usize,
        bare: bool,
    },
    Index {
        rebuild: bool,
    },
}

fn data_dir() -> std::path::PathBuf {
    crate::config::data_root()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("aishe")
}

/// The reedline history file and the timestamped sidecar log paths. Shared across
/// sessions by default (zsh `SHARE_HISTORY`), or pid-suffixed per-session when
/// `share_history` is off.
pub fn history_paths(config: &Config) -> (std::path::PathBuf, std::path::PathBuf) {
    if config.aishe.share_history {
        (data_dir().join("history"), data_dir().join("history.ext"))
    } else {
        let pid = std::process::id();
        (
            data_dir().join(format!("history.{pid}")),
            data_dir().join(format!("history.{pid}.ext")),
        )
    }
}

/// History destination for the direct `-c` fast path. A parent AIShe PTY
/// exports the exact active file; standalone invocations preserve the existing
/// first-run, migration, malformed-config, and `share_history=false` contracts.
/// No provider, plugin, MCP registry, or managed backend is constructed.
pub fn fast_history_log() -> Result<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("AISHE_HISTFILE").filter(|path| !path.is_empty()) {
        return Ok(path.into());
    }
    let mut config = Config::load_or_init()?;
    let _project_overlay = std::env::current_dir()
        .ok()
        .and_then(|cwd| config.apply_project_overlay(&cwd));
    Ok(history_paths(&config).1)
}

/// The on-disk semantic-history vector store.
fn semhist_path() -> std::path::PathBuf {
    data_dir().join("history.vec")
}

/// `aishe dry-run "<cmd>"`: run the command against a throwaway copy of the
/// working tree under bubblewrap (read-only root, no network), show the file
/// changes it would make, then keep them (`--apply`) or discard them.
pub fn dry_run(command: &str, apply: bool) -> Result<u8> {
    let bubblewrap = crate::dependencies::bubblewrap_probe();
    if !matches!(
        bubblewrap,
        crate::dependencies::BubblewrapState::Usable { .. }
    ) {
        eprintln!(
            "aishe: dry-run needs functional Linux bubblewrap for safe isolation; \
             current state: {bubblewrap:?}. Run `aishe doctor` or `aishe setup`."
        );
        return Ok(1);
    }
    let cwd = std::env::current_dir()?;
    let staging = std::env::temp_dir().join(format!("aishe-dryrun-{}", std::process::id()));
    std::fs::remove_dir_all(&staging).ok();
    let _guard = TempDirGuard(staging.clone());

    if let Err(e) = crate::overlay::copy_tree(&cwd, &staging) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }

    // Run the command in the sandbox: <bwrap-argv…> -- <shell> -c <command>.
    let shell = Executor::new()
        .ok()
        .map(|e| e.shell().to_path_buf())
        .unwrap_or_else(|| std::path::PathBuf::from("/bin/sh"));
    let argv = crate::overlay::dry_run_argv(&cwd, &staging);
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .arg(&shell)
        .arg("-c")
        .arg(command)
        .status();
    let code = match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            eprintln!("aishe: failed to launch sandbox: {e}");
            return Ok(1);
        }
    };

    let changes = crate::overlay::changes(&cwd, &staging);
    println!();
    if changes.is_empty() {
        println!(
            "{} no file changes (command exit {code}).",
            "dry-run:".bold()
        );
        return Ok(code as u8);
    }
    println!(
        "{} {} file change(s) (command exit {code}):",
        "dry-run:".bold(),
        changes.len()
    );
    crate::overlay::print_changes(&changes);
    if apply {
        let failed = crate::overlay::apply_journaled(&cwd, &staging, &changes, "dry_run");
        if failed.is_empty() {
            println!(
                "\n{} applied {} change(s) to the working tree ({} to revert).",
                "✓".green(),
                changes.len(),
                "aishe undo".bold()
            );
        } else {
            println!(
                "\n{} applied with {} failure(s): {}",
                "!".yellow(),
                failed.len(),
                failed.join(", ")
            );
        }
    } else {
        println!(
            "\n{} re-run with {} to keep these changes.",
            "discarded —".dim(),
            "--apply".bold()
        );
    }
    Ok(code as u8)
}

/// Removes a directory tree on drop (best-effort), for the dry-run staging copy.
struct TempDirGuard(std::path::PathBuf);
impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Dispatch `aishe history <search|index>`.
pub fn command(config: &Config, cmd: &Action) -> Result<u8> {
    match cmd {
        Action::Index { rebuild } => history_index(config, *rebuild),
        Action::Search { query, limit, bare } => {
            history_search(config, &query.join(" "), *limit, *bare)
        }
    }
}

/// Notice + early return when the feature is off, with how to turn it on. In
/// `bare` mode the notice goes to stderr so stdout stays clean for the widget.
fn semantic_history_off_notice_bare(bare: bool) -> u8 {
    if bare {
        eprintln!("aishe: semantic history is off (set semantic_history = true).");
        return 0;
    }
    semantic_history_off_notice()
}

/// Notice + early return when the feature is off, with how to turn it on.
fn semantic_history_off_notice() -> u8 {
    println!(
        "semantic history is off. Enable it in {}:\n  \
         [aishe]\n  semantic_history = true\n  \
         embedding_provider = \"openai\"   # anthropic has no embeddings endpoint\n  \
         embedding_model = \"text-embedding-3-small\"\n\
         then run `aishe history index`.",
        Config::path().display()
    );
    0
}

/// Embed any not-yet-indexed history commands (or all, with `--rebuild`) into the
/// vector store. Reports how many were added.
fn history_index(config: &Config, rebuild: bool) -> Result<u8> {
    if !config.aishe.semantic_history {
        return Ok(semantic_history_off_notice());
    }
    let store = semhist_path();
    let hist = history_paths(config).1;
    match crate::index::reindex(config, &store, &hist, rebuild) {
        Ok(Ok(ix)) => {
            println!(
                "indexed {} command(s) ({} in the store).",
                ix.added, ix.total
            );
            Ok(0)
        }
        Ok(Err(crate::index::Skip::NoHistory)) => {
            println!("no history to index yet (run some commands first).");
            Ok(0)
        }
        Ok(Err(crate::index::Skip::UpToDate(n))) => {
            println!("semantic index is up to date ({n} commands).");
            Ok(0)
        }
        Err(e) => {
            eprintln!("aishe: {e}");
            Ok(1)
        }
    }
}

/// Embed the query and print the closest past commands by meaning. In `bare`
/// mode only the command text is printed (no score column) and every notice goes
/// to stderr, so the recall key binding can assign stdout straight to the line.
fn history_search(config: &Config, query: &str, limit: usize, bare: bool) -> Result<u8> {
    if !config.aishe.semantic_history {
        return Ok(semantic_history_off_notice_bare(bare));
    }
    if query.trim().is_empty() {
        eprintln!(
            "aishe: history search needs a query, e.g. aishe history search \"docker volume\""
        );
        return Ok(1);
    }
    let store = semhist_path();
    let entries = crate::semhist::load(&store);
    if entries.is_empty() {
        let msg = "the semantic index is empty — run `aishe history index` first.";
        if bare {
            eprintln!("aishe: {msg}");
        } else {
            println!("{msg}");
        }
        return Ok(0);
    }
    let provider = providers::embedder(config)?;
    let qv = provider.embed(&[query.to_string()], &config.aishe.embedding_model)?;
    let Some(qvec) = qv.into_iter().next() else {
        eprintln!("aishe: the embedder returned no vector for the query.");
        return Ok(1);
    };
    let hits = crate::semhist::top_k(&entries, &qvec, limit.max(1));
    if hits.is_empty() {
        if bare {
            eprintln!("aishe: no match.");
        } else {
            println!("no matches.");
        }
        return Ok(0);
    }
    for (score, cmd) in hits {
        if bare {
            println!("{cmd}");
        } else {
            println!("{}  {cmd}", format!("{score:.2}").dim());
        }
    }
    Ok(0)
}

/// `AISHE_LOG=1` forces it on, `AISHE_LOG_FILE` overrides the path.
/// Resolve the effective audit-log settings from the config file and the
/// environment. Precedence: `AISHE_LOG` *enables* logging on top of the config
/// flag (either turns it on; neither leaves it off), and `AISHE_LOG_FILE`
/// *overrides* the configured path. Pure so the precedence is unit-testable
/// without touching the process environment or the global audit state.
pub fn resolve_audit(
    config: &Config,
    env_log: Option<&str>,
    env_file: Option<&str>,
) -> (bool, Option<std::path::PathBuf>) {
    let env_on = matches!(env_log, Some("1") | Some("true") | Some("yes"));
    let enabled = config.logging.enabled || env_on;
    let path = env_file
        .map(std::path::PathBuf::from)
        .or_else(|| config.logging.file.clone().map(std::path::PathBuf::from));
    (enabled, path)
}

/// Resolve the audit log path for read-only log/usage commands without
/// initializing the writer.
pub fn audit_log_path(config: &Config) -> std::path::PathBuf {
    if let Ok(p) = std::env::var("AISHE_LOG_FILE") {
        if !p.is_empty() {
            return std::path::PathBuf::from(p);
        }
    }
    if let Some(p) = &config.logging.file {
        return std::path::PathBuf::from(p);
    }
    crate::audit::default_path()
}

/// Parse a relative `--since` like `30m`, `2h`, `3d`, `1w` into a cutoff epoch-ms.
/// A bare number means minutes. Returns `None` if unparseable.
fn parse_since(s: &str) -> Option<u64> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let n: u64 = s[..split].parse().ok()?;
    let secs = match &s[split..] {
        "" | "m" => 60,
        "s" => 1,
        "h" => 3600,
        "d" => 86_400,
        "w" => 604_800,
        _ => return None,
    };
    Some(crate::audit::now_ms_u64().saturating_sub(n * secs * 1000))
}

/// `aishe log`: print (filtered) audit entries as a table, or raw JSONL.
#[allow(clippy::too_many_arguments)]
pub fn log(
    config: &Config,
    session: Option<&str>,
    action: Option<&str>,
    model: Option<&str>,
    since: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> u8 {
    let path = audit_log_path(config);
    let mut entries = crate::audit::read_entries(&path);
    if entries.is_empty() && !path.exists() {
        eprintln!(
            "aishe: no audit log at {} (enable it in [logging] or with AISHE_LOG=1)",
            path.display()
        );
        return 0;
    }
    let cutoff = since.and_then(parse_since);
    entries.retain(|e| {
        if let Some(c) = cutoff {
            if e.ts_ms < c {
                return false;
            }
        }
        if let Some(s) = session {
            if e.session != s && e.backend_session.as_deref() != Some(s) {
                return false;
            }
        }
        if let Some(a) = action {
            if e.kind != a {
                return false;
            }
        }
        if let Some(m) = model {
            if !e.model.as_deref().is_some_and(|em| em.contains(m)) {
                return false;
            }
        }
        true
    });
    if let Some(n) = limit {
        let len = entries.len();
        if len > n {
            entries.drain(0..len - n);
        }
    }
    if entries.is_empty() {
        if json {
            eprintln!("aishe: no matching audit entries");
        } else {
            println!("no matching audit entries");
        }
        return 0;
    }
    if json {
        for e in &entries {
            println!("{}", e.raw);
        }
        return 0;
    }
    for e in &entries {
        let detail = match e.kind.as_str() {
            "session_start" => format!("── session {} ──", e.session),
            "ai_request" => format!(
                "→ ask {} ({})",
                e.model.as_deref().unwrap_or("?"),
                e.mode.as_deref().unwrap_or("?")
            ),
            "ai_response" => format!(
                "← {} · {} in / {} out{}",
                e.model.as_deref().unwrap_or("?"),
                e.tokens_in.unwrap_or(0),
                e.tokens_out.unwrap_or(0),
                e.duration_ms
                    .map(|duration| format!(" · {duration}ms"))
                    .unwrap_or_default(),
            ),
            "ai_error" => format!(
                "✗ {} {}",
                e.model.as_deref().unwrap_or("?"),
                e.text.as_deref().unwrap_or("")
            ),
            "action" => {
                let exit = e.exit.map(|c| format!(" [exit {c}]")).unwrap_or_default();
                format!(
                    "$ {}{}  ({})",
                    e.command.as_deref().unwrap_or(""),
                    exit,
                    e.source.as_deref().unwrap_or("")
                )
            }
            "tool_call" => {
                let target = e
                    .command
                    .as_deref()
                    .or_else(|| e.raw.get("path").and_then(serde_json::Value::as_str))
                    .unwrap_or("");
                format!(
                    "→ tool {} {}",
                    e.tool.as_deref().unwrap_or("?"),
                    crate::commands::display_safe(target)
                )
            }
            "tool_result" => format!(
                "← tool {} · {}{}{}",
                e.tool.as_deref().unwrap_or("?"),
                if e.success == Some(true) {
                    "ok"
                } else {
                    "failed"
                },
                e.exit
                    .map(|exit| format!(" · exit {exit}"))
                    .unwrap_or_default(),
                e.duration_ms
                    .map(|duration| format!(" · {duration}ms"))
                    .unwrap_or_default(),
            ),
            "tool_approval" => format!(
                "approval {} · {}",
                e.tool.as_deref().unwrap_or("?"),
                e.raw
                    .get("decision")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| e.raw.get("phase").and_then(serde_json::Value::as_str))
                    .unwrap_or("recorded")
            ),
            "file_change" => format!(
                "file changed {}",
                crate::commands::display_safe(
                    e.raw
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("?")
                )
            ),
            "agent_event" => format!("agent {}", e.event.as_deref().unwrap_or("event")),
            other => other.to_string(),
        };
        let colored = if e.kind == "ai_error" {
            detail.red().to_string()
        } else if e.kind == "session_start" {
            detail.dim().to_string()
        } else {
            detail
        };
        println!("{}  {}", crate::audit::fmt_utc(e.ts_ms).dim(), colored);
    }
    0
}

/// `aishe usage`: aggregate token counts and estimated cost from the audit log.
/// Ledger rows as audit-shaped entries, so one aggregation path serves both.
fn ledger_entries() -> Vec<crate::audit::Entry> {
    crate::audit::ledger::read()
        .into_iter()
        .map(|value| {
            let string = |key: &str| value.get(key).and_then(|v| v.as_str()).map(str::to_string);
            let number = |key: &str| value.get(key).and_then(serde_json::Value::as_u64);
            crate::audit::Entry {
                ts_ms: number("ts_ms").unwrap_or(0),
                session: string("session").unwrap_or_default(),
                kind: "ai_response".to_string(),
                model: string("model"),
                connection_id: string("connection_id"),
                provider: string("provider"),
                auth_type: string("auth_type"),
                mode: string("mode"),
                tokens_in: number("tokens_in"),
                tokens_out: number("tokens_out"),
                cache_read_tokens: number("cache_read_tokens"),
                cache_write_tokens: number("cache_write_tokens"),
                reasoning_tokens: number("reasoning_tokens"),
                cost_usd: value.get("cost_usd").and_then(serde_json::Value::as_f64),
                duration_ms: number("duration_ms"),
                ..Default::default()
            }
        })
        .collect()
}

/// Why a group has no dollar figure. Subscription plans have no per-token price
/// at all, which is a different situation from a priced model nobody configured.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CostBasis {
    /// Every call in the group had a known price.
    Priced,
    /// Some or all calls ran on a subscription, where tokens are not billed.
    #[default]
    Subscription,
    /// A billable model with no configured price.
    Unpriced,
}

impl CostBasis {
    fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unpriced, _) | (_, Self::Unpriced) => Self::Unpriced,
            (Self::Priced, _) | (_, Self::Priced) => Self::Priced,
            _ => Self::Subscription,
        }
    }
}

#[derive(Default)]
struct UsageAgg {
    tin: u64,
    tout: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    reqs: u64,
    errors: u64,
    duration_ms: u64,
    cost: f64,
    unpriced: u64,
    basis: Option<CostBasis>,
}

impl UsageAgg {
    fn add(&mut self, other: &UsageSample) {
        self.tin += other.tin;
        self.tout += other.tout;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.reasoning += other.reasoning;
        self.reqs += 1;
        self.duration_ms += other.duration_ms;
        self.cost += other.cost;
        if other.basis == CostBasis::Unpriced {
            self.unpriced += 1;
        }
        self.basis = Some(match self.basis {
            Some(existing) => existing.merge(other.basis),
            None => other.basis,
        });
    }

    /// Share of input tokens served from the prompt cache.
    fn cache_hit_rate(&self) -> Option<f64> {
        let offered = self.tin + self.cache_read;
        (offered > 0 && (self.cache_read > 0 || self.cache_write > 0))
            .then(|| self.cache_read as f64 * 100.0 / offered as f64)
    }

    fn cost_label(&self) -> String {
        match self.basis.unwrap_or_default() {
            CostBasis::Priced => format!("~${:.4}", self.cost),
            CostBasis::Unpriced if self.cost > 0.0 => {
                format!("~${:.4} (+{} unpriced)", self.cost, self.unpriced)
            }
            CostBasis::Unpriced => "no price set".to_string(),
            CostBasis::Subscription => "plan".to_string(),
        }
    }
}

struct UsageSample {
    tin: u64,
    tout: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    duration_ms: u64,
    cost: f64,
    basis: CostBasis,
}

/// `aishe usage`: what this shell, today, and the whole audit log have spent.
/// Everything here is read from AIShe's own audit log, so it is exact for what
/// AIShe did; plan quota is fetched from the provider and labelled as such.
pub fn usage(
    config: &Config,
    by: Option<&str>,
    since: Option<&str>,
    connection: Option<&str>,
    json: bool,
) -> u8 {
    use crate::usage::{self, Usage};
    // The ledger is content-free and always written; the audit log is opt-in and
    // carries prompts. Prefer the ledger, and fall back so history recorded
    // before the ledger existed still counts.
    let mut entries = ledger_entries();
    if entries.is_empty() {
        entries = crate::audit::read_entries(&audit_log_path(config));
    }
    if entries.is_empty() {
        println!("no model calls recorded yet");
        if let Some(plan) = plan_quota(config) {
            println!("plan: {plan} (from your provider subscription)");
        }
        return 0;
    }
    let cutoff = since.and_then(parse_since);
    let by = by.unwrap_or("model");
    let connection = connection.map(|value| {
        config
            .resolve_connection_id(value)
            .unwrap_or_else(|_| value.to_string())
    });

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as u64)
        .unwrap_or(0);
    let today = crate::audit::fmt_date(now_ms);
    let shell_session = std::env::var("AISHE_SHELL_ID")
        .ok()
        .filter(|id| !id.is_empty());
    let mut groups: std::collections::BTreeMap<String, UsageAgg> =
        std::collections::BTreeMap::new();
    let mut total = UsageAgg::default();
    let mut today_agg = UsageAgg::default();
    let mut session_agg = UsageAgg::default();
    let mut errors = 0_u64;

    for e in &entries {
        if connection
            .as_deref()
            .is_some_and(|id| e.connection_id.as_deref() != Some(id))
        {
            continue;
        }
        if let Some(c) = cutoff {
            if e.ts_ms < c {
                continue;
            }
        }
        if e.kind == "ai_error" {
            errors += 1;
            continue;
        }
        if e.kind != "ai_response" {
            continue;
        }
        let tin = e.tokens_in.unwrap_or(0);
        let tout = e.tokens_out.unwrap_or(0);
        let model = e.model.as_deref().unwrap_or("?");
        // A subscription reports no per-token price, so an absent price there is
        // correct rather than missing configuration.
        let subscription = e.auth_type.as_deref() == Some("oauth");
        let (cost, basis) = match usage::price_for(model, &config.pricing) {
            Some(price) => (
                usage::cost(
                    Usage {
                        input: tin,
                        output: tout,
                        requests: 1,
                    },
                    price,
                ),
                CostBasis::Priced,
            ),
            None if subscription => (e.cost_usd.unwrap_or(0.0), CostBasis::Subscription),
            None => (0.0, CostBasis::Unpriced),
        };
        let sample = UsageSample {
            tin,
            tout,
            cache_read: e.cache_read_tokens.unwrap_or(0),
            cache_write: e.cache_write_tokens.unwrap_or(0),
            reasoning: e.reasoning_tokens.unwrap_or(0),
            duration_ms: e.duration_ms.unwrap_or(0),
            cost,
            basis,
        };
        let key = match by {
            "connection" => e
                .connection_id
                .clone()
                .unwrap_or_else(|| "legacy/unknown".into()),
            "day" => crate::audit::fmt_date(e.ts_ms),
            "session" => e.session.clone(),
            _ => model.to_string(),
        };
        groups.entry(key).or_default().add(&sample);
        total.add(&sample);
        if crate::audit::fmt_date(e.ts_ms) == today {
            today_agg.add(&sample);
        }
        if shell_session.as_deref() == Some(e.session.as_str()) {
            session_agg.add(&sample);
        }
    }
    total.errors = errors;

    let plan = plan_quota(config);
    if json {
        return usage_json(
            config,
            by,
            &groups,
            &total,
            &today_agg,
            &session_agg,
            plan.as_deref(),
        );
    }
    print_usage_report(
        by,
        connection.as_deref(),
        &groups,
        &total,
        &today_agg,
        &session_agg,
        plan.as_deref(),
    )
}

/// Plan quota for the active connection, when the provider exposes one. Only
/// OpenAI subscriptions do today; everything else reports nothing rather than
/// guessing.
fn plan_quota(config: &Config) -> Option<String> {
    let connection = config.active_connection()?;
    let crate::config::ConnectionAuth::OAuth { profile } = &connection.auth else {
        return None;
    };
    let provider = crate::oauth::OAuthProvider::from_base_url(&connection.settings.base_url)?;
    crate::oauth::plan_usage(provider, profile).map(|usage| usage.summary)
}

fn tokens(value: u64) -> String {
    crate::usage::group(value)
}

fn print_usage_report(
    by: &str,
    connection: Option<&str>,
    groups: &std::collections::BTreeMap<String, UsageAgg>,
    total: &UsageAgg,
    today: &UsageAgg,
    session: &UsageAgg,
    plan: Option<&str>,
) -> u8 {
    if total.reqs == 0 {
        println!("no model calls recorded in the audit log");
        if let Some(plan) = plan {
            println!("plan: {plan}");
        }
        return 0;
    }
    println!("AIShe usage");
    for (label, agg) in [
        ("this shell", session),
        ("today", today),
        ("all time", total),
    ] {
        if agg.reqs == 0 {
            continue;
        }
        let mut line = format!(
            "  {label:<11} {} in · {} out · {} turns · {}",
            tokens(agg.tin),
            tokens(agg.tout),
            agg.reqs,
            agg.cost_label()
        );
        if agg.reasoning > 0 {
            line.push_str(&format!(" · {} thinking", tokens(agg.reasoning)));
        }
        if let Some(rate) = agg.cache_hit_rate() {
            line.push_str(&format!(" · {rate:.0}% cached"));
        }
        if agg.duration_ms > 0 {
            line.push_str(&format!(
                " · {:.1}s model time",
                agg.duration_ms as f64 / 1000.0
            ));
        }
        println!("{line}");
    }
    if total.errors > 0 {
        println!("  errors      {} failed turn(s)", total.errors);
    }
    if let Some(plan) = plan {
        println!("  plan        {plan} (from your provider subscription)");
    }
    println!();
    match connection {
        Some(connection) => println!("by {by} · connection {connection}:"),
        None => println!("by {by}:"),
    }
    for (key, agg) in groups {
        print_usage_row(key, agg);
    }
    print_usage_row("TOTAL", total);
    0
}

fn print_usage_row(label: &str, agg: &UsageAgg) {
    let cache = match agg.cache_hit_rate() {
        Some(rate) => format!("{rate:>3.0}% cached"),
        None => String::new(),
    };
    println!(
        "  {:<28} {:>9} in {:>9} out {:>4} req  {:<22} {}",
        label,
        tokens(agg.tin),
        tokens(agg.tout),
        agg.reqs,
        agg.cost_label(),
        cache
    );
}

fn usage_json(
    config: &Config,
    by: &str,
    groups: &std::collections::BTreeMap<String, UsageAgg>,
    total: &UsageAgg,
    today: &UsageAgg,
    session: &UsageAgg,
    plan: Option<&str>,
) -> u8 {
    let describe = |agg: &UsageAgg| {
        serde_json::json!({
            "tokens_in": agg.tin,
            "tokens_out": agg.tout,
            "cache_read_tokens": agg.cache_read,
            "cache_write_tokens": agg.cache_write,
            "reasoning_tokens": agg.reasoning,
            "requests": agg.reqs,
            "duration_ms": agg.duration_ms,
            "cost_usd": agg.cost,
            "cost_basis": match agg.basis.unwrap_or_default() {
                CostBasis::Priced => "priced",
                CostBasis::Subscription => "subscription",
                CostBasis::Unpriced => "unpriced",
            },
            "cache_hit_percent": agg.cache_hit_rate(),
        })
    };
    let document = serde_json::json!({
        "schema_version": 1,
        "group_by": by,
        "connection": config.active_connection_id(),
        "session": describe(session),
        "today": describe(today),
        "total": describe(total),
        "errors": total.errors,
        "plan": plan,
        "groups": groups
            .iter()
            .map(|(key, agg)| serde_json::json!({"key": key, "usage": describe(agg)}))
            .collect::<Vec<_>>(),
    });
    match crate::cli::json_contract::print_object(&document) {
        Ok(()) => 0,
        Err(error) => crate::cli::error_contract::emit_from(error.as_ref()),
    }
}

/// `aishe runbook`: turn a recorded session (from the audit log) into a runnable
/// `.sh` script and a human `.md` runbook — or, with `--replay`, re-run the
/// recorded commands through the safety gate (never the model).
pub fn runbook(
    config: &Config,
    session: Option<&str>,
    out: Option<&str>,
    replay: bool,
) -> Result<u8> {
    let path = audit_log_path(config);
    let entries = crate::audit::read_entries(&path);
    if entries.is_empty() {
        eprintln!(
            "aishe: no audit log at {} (enable it in [logging] or with AISHE_LOG=1)",
            path.display()
        );
        return Ok(0);
    }
    // Target session: the requested one, else the most recent recorded session.
    let session_id = match session {
        Some(s) => s.to_string(),
        None => entries
            .iter()
            .rev()
            .map(|e| e.session.clone())
            .find(|s| !s.is_empty())
            .unwrap_or_default(),
    };
    let rows: Vec<&crate::audit::Entry> =
        entries.iter().filter(|e| e.session == session_id).collect();
    if rows.is_empty() {
        eprintln!("aishe: no entries for session '{session_id}'");
        return Ok(1);
    }
    // The request that started it (first ai_request prompt), and the commands run.
    let request = rows
        .iter()
        .find(|e| e.kind == "ai_request")
        .and_then(|e| e.text.clone());
    let commands: Vec<(String, Option<i64>)> = rows
        .iter()
        .filter(|e| e.kind == "action")
        .filter_map(|e| e.command.clone().map(|c| (c, e.exit)))
        .collect();

    if replay {
        return Ok(replay_commands(&commands));
    }

    if commands.is_empty() {
        eprintln!("aishe: session '{session_id}' ran no commands to export");
        return Ok(1);
    }
    let when = rows
        .first()
        .map(|e| crate::audit::fmt_utc(e.ts_ms))
        .unwrap_or_default();
    let sh = render_runbook_sh(&session_id, &when, request.as_deref(), &commands);
    let md = render_runbook_md(&session_id, &when, request.as_deref(), &rows);

    let dir = std::path::PathBuf::from(out.unwrap_or("."));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }
    let base = format!("runbook-{}", session_id.replace(['/', ' '], "_"));
    let sh_path = dir.join(format!("{base}.sh"));
    let md_path = dir.join(format!("{base}.md"));
    if let Err(e) = std::fs::write(&sh_path, sh).and_then(|_| std::fs::write(&md_path, md)) {
        eprintln!("aishe: {e}");
        return Ok(1);
    }
    println!("{} {}", "wrote".green(), sh_path.display());
    println!("{} {}", "wrote".green(), md_path.display());
    Ok(0)
}

/// Render the runnable script for a session's commands.
fn render_runbook_sh(
    session: &str,
    when: &str,
    request: Option<&str>,
    commands: &[(String, Option<i64>)],
) -> String {
    let mut s = String::from("#!/usr/bin/env bash\n");
    s.push_str(&format!(
        "# Runbook generated by aishe from audit session {session} ({when} UTC).\n"
    ));
    if let Some(r) = request {
        s.push_str(&format!("# Request: {}\n", r.lines().next().unwrap_or(r)));
    }
    s.push_str("# Review before running — these are the commands the AI ran, in order.\n");
    s.push_str("# Secrets are already redacted in the audit log they came from.\n");
    s.push_str("set -uo pipefail\n\n");
    for (cmd, exit) in commands {
        if matches!(exit, Some(c) if *c != 0) {
            s.push_str(&format!("# (exited {} when recorded)\n", exit.unwrap()));
        }
        s.push_str(cmd);
        s.push('\n');
    }
    s
}

/// Render the human-readable markdown runbook.
fn render_runbook_md(
    session: &str,
    when: &str,
    request: Option<&str>,
    rows: &[&crate::audit::Entry],
) -> String {
    let title = request
        .and_then(|r| r.lines().next())
        .map(|l| l.to_string())
        .unwrap_or_else(|| format!("aishe session {session}"));
    let mut m = format!("# Runbook: {title}\n\n");
    m.push_str(&format!(
        "Generated by aishe from audit session `{session}` ({when} UTC).\n\n## Steps\n\n"
    ));
    let mut n = 0;
    for e in rows {
        match e.kind.as_str() {
            "action" => {
                if let Some(cmd) = &e.command {
                    n += 1;
                    let exit = e.exit.map(|c| format!(" → exit {c}")).unwrap_or_default();
                    let src = e.source.as_deref().unwrap_or("");
                    m.push_str(&format!("{n}. `{cmd}`{exit}  _({src})_\n"));
                }
            }
            "ai_response" => {
                if let Some(t) = &e.text {
                    if !t.is_empty() {
                        m.push_str(&format!("> {t}\n\n"));
                    }
                }
            }
            _ => {}
        }
    }
    m.push_str(&format!(
        "\n## Reproduce\n\n```sh\nbash runbook-{}.sh\n```\n",
        session.replace(['/', ' '], "_")
    ));
    m
}

/// `aishe runbook --replay`: re-run recorded commands through the safety gate.
/// Safe commands run; dangerous ones are skipped with a warning (the gate, not the
/// model, decides — so reproduction is deterministic and never re-prompts an LLM).
fn replay_commands(commands: &[(String, Option<i64>)]) -> u8 {
    if commands.is_empty() {
        println!("nothing to replay");
        return 0;
    }
    let mut executor = match Executor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("aishe: {e}");
            return 1;
        }
    };
    let mut last = 0u8;
    for (cmd, _) in commands {
        match safety::assess(cmd) {
            Risk::Safe => {
                println!("{} {cmd}", "›".green());
                last = executor.run(cmd) as u8;
            }
            Risk::Dangerous(reason) => {
                eprintln!("{} skipped (dangerous: {reason}): {cmd}", "!".yellow());
            }
            // Replay is non-interactive by design, so an unresolvable command is
            // skipped rather than guessed at.
            Risk::Unknown(reason) => {
                eprintln!("{} skipped (unverifiable: {reason}): {cmd}", "!".yellow());
            }
        }
    }
    last
}

pub fn init_audit(config: &Config) {
    let env_log = std::env::var("AISHE_LOG").ok();
    let env_file = std::env::var("AISHE_LOG_FILE").ok();
    let (enabled, path) = resolve_audit(config, env_log.as_deref(), env_file.as_deref());
    crate::audit::init_for_config(enabled, path, config.logging.redact, config);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_precedence_env_over_file() {
        let mut config = Config::default();
        config.logging.enabled = false;
        config.logging.file = None;
        let (enabled, path) = resolve_audit(&config, None, None);
        assert!(!enabled);
        assert!(path.is_none());

        assert!(resolve_audit(&config, Some("1"), None).0);
        assert!(!resolve_audit(&config, Some("0"), None).0);

        config.logging.enabled = true;
        assert!(resolve_audit(&config, None, None).0);

        config.logging.file = Some("/from/config.jsonl".into());
        assert_eq!(
            resolve_audit(&config, None, Some("/from/env.jsonl"))
                .1
                .unwrap(),
            std::path::PathBuf::from("/from/env.jsonl")
        );
        assert_eq!(
            resolve_audit(&config, None, None).1.unwrap(),
            std::path::PathBuf::from("/from/config.jsonl")
        );
    }
}
