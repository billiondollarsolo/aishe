//! Foreground tool leases and durable call idempotency.
//!
//! OpenCode can request work, but only a live foreground AIShe process holding
//! the matching lease can execute it. The journal never stores credentials and
//! redacts model/tool content before every atomic write.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{ExecutionScope, Mode, NetworkPolicy};

const JOURNAL_SCHEMA_VERSION: u32 = 1;
const MAX_JOURNAL_BYTES: u64 = 16 * 1024 * 1024;
/// Default lease lifetime without a heartbeat. Long multi-tool yolo turns rely
/// on the foreground worker's keepalive thread (see `ToolWorker`) so this can
/// stay relatively short for fail-closed cleanup of abandoned shells.
const LEASE_TTL: Duration = Duration::from_secs(120);
const USAGE_RECONCILIATION_GRACE: Duration = Duration::from_secs(30);
const BUDGET_RESERVATION_TTL: Duration = Duration::from_secs(10 * 60);
const TOOL_WAIT: Duration = Duration::from_secs(60 * 60);

/// Override for tests only (`0` = use [`LEASE_TTL`]).
#[cfg(test)]
static TEST_LEASE_TTL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn lease_ttl() -> Duration {
    #[cfg(test)]
    {
        let ms = TEST_LEASE_TTL_MS.load(std::sync::atomic::Ordering::Relaxed);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    LEASE_TTL
}

/// How often the foreground tool worker should renew the lease while a turn is
/// live (including during long `run_command` executions).
pub const LEASE_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);

#[cfg(test)]
pub fn set_test_lease_ttl_ms(ms: u64) {
    TEST_LEASE_TTL_MS.store(ms, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseRegistration {
    pub aishe_shell_id: String,
    pub backend_session_id: String,
    pub workspace: PathBuf,
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    pub interactive: bool,
    /// Hard session budget only when AIShe has an exact trusted price.
    pub budget_usd: Option<f64>,
    pub price: Option<crate::usage::Price>,
    /// Authoritative cost already present in the resumed backend session.
    pub baseline_spent_usd: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseIdentity {
    pub lease_id: String,
    pub backend_session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildRegistration {
    pub parent_session_id: String,
    pub child_session_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTurnRequest {
    pub session_id: String,
    pub requested_max_output_tokens: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderTurnDecision {
    pub max_output_tokens: u64,
    pub remaining_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderUsageReport {
    pub session_id: String,
    pub message_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginToolRequest {
    pub tool: String,
    pub args: Value,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub agent: String,
    pub directory: PathBuf,
    pub worktree: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolWork {
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub tool: String,
    pub args: Value,
    pub workspace: PathBuf,
    pub mode: Mode,
    pub scope: ExecutionScope,
    pub network: NetworkPolicy,
    pub interactive: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolStarted {
    pub lease_id: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCompletion {
    pub lease_id: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub success: bool,
    pub output: Value,
    pub exit_code: Option<i32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub success: bool,
    pub output: Value,
    pub exit_code: Option<i32>,
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BridgeFailure {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for BridgeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for BridgeFailure {}

#[derive(Clone)]
struct ToolLease {
    lease_id: String,
    registration: LeaseRegistration,
    workspace: PathBuf,
    expires_at: Instant,
    queue: VecDeque<CallKey>,
    pending_budget_reservations: HashMap<String, VecDeque<BudgetReservation>>,
}

#[derive(Clone)]
struct BudgetReservation {
    amount_usd: f64,
    expires_at: Instant,
}

/// A closed foreground lease has no tool or provider-turn authority, but keeps
/// just enough non-secret accounting state for OpenCode's asynchronous
/// `message.updated` plugin callback to reconcile the provider usage that
/// completed immediately before unregister.
struct RetiredUsageLease {
    price: Option<crate::usage::Price>,
    pending_budget_reservations: HashMap<String, VecDeque<BudgetReservation>>,
    expires_at: Instant,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
struct CallKey {
    session_id: String,
    message_id: String,
    call_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CallStatus {
    Admitted,
    Dispatched,
    Started,
    Completed,
    OutcomeUnknown,
    Cancelled,
}

#[derive(Clone, Serialize, Deserialize)]
struct DurableCall {
    key: CallKey,
    #[serde(default)]
    owner_session_id: String,
    tool: String,
    args: Value,
    status: CallStatus,
    result: Option<ToolOutcome>,
    created_at_ms: u128,
    updated_at_ms: u128,
}

#[derive(Clone, Serialize, Deserialize)]
struct DurableUsage {
    session_id: String,
    message_id: String,
    input_tokens: u64,
    output_tokens: u64,
    cost_usd: f64,
}

struct ActiveCall {
    durable: DurableCall,
    request: Option<PluginToolRequest>,
    full_result: Option<ToolOutcome>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    schema_version: u32,
    #[serde(default)]
    children: HashMap<String, String>,
    calls: Vec<DurableCall>,
    #[serde(default)]
    usage: Vec<DurableUsage>,
}

#[derive(Default)]
struct BridgeState {
    leases: HashMap<String, ToolLease>,
    retired_usage_leases: HashMap<String, RetiredUsageLease>,
    children: HashMap<String, String>,
    calls: HashMap<CallKey, ActiveCall>,
    usage: HashMap<String, DurableUsage>,
}

pub struct Bridge {
    journal_path: PathBuf,
    state: Mutex<BridgeState>,
    changed: Condvar,
}

impl Bridge {
    pub fn open_default() -> Result<Self> {
        Self::open(
            super::supervisor::backend_root()?
                .join("journal")
                .join("tool-calls.json"),
        )
    }

    pub fn open(journal_path: PathBuf) -> Result<Self> {
        let journal = load_journal(&journal_path)?;
        let calls = journal
            .calls
            .into_iter()
            .map(|mut durable| {
                if durable.owner_session_id.is_empty() {
                    durable.owner_session_id.clone_from(&durable.key.session_id);
                }
                if matches!(durable.status, CallStatus::Started | CallStatus::Dispatched) {
                    durable.status = CallStatus::OutcomeUnknown;
                    durable.updated_at_ms = now_ms();
                }
                (
                    durable.key.clone(),
                    ActiveCall {
                        durable,
                        request: None,
                        full_result: None,
                    },
                )
            })
            .collect();
        let usage = journal
            .usage
            .into_iter()
            .map(|record| (record.message_id.clone(), record))
            .collect();
        let bridge = Self {
            journal_path,
            state: Mutex::new(BridgeState {
                leases: HashMap::new(),
                retired_usage_leases: HashMap::new(),
                children: journal.children,
                calls,
                usage,
            }),
            changed: Condvar::new(),
        };
        bridge.persist()?;
        Ok(bridge)
    }

    pub fn active_lease_count(&self) -> Result<usize> {
        let mut state = self.lock()?;
        cleanup_expired(&mut state);
        Ok(state.leases.len())
    }

    pub fn register(&self, registration: LeaseRegistration) -> Result<LeaseIdentity> {
        validate_shell_id(&registration.aishe_shell_id)?;
        validate_id(&registration.backend_session_id, "ses_")?;
        let workspace = registration.workspace.canonicalize().with_context(|| {
            format!(
                "canonicalizing lease workspace {}",
                registration.workspace.display()
            )
        })?;
        validate_budget_registration(&registration)?;
        let lease_id = random_hex(32);
        let session_id = registration.backend_session_id.clone();
        let mut state = self.lock()?;
        state.retired_usage_leases.remove(&session_id);
        state.leases.insert(
            session_id.clone(),
            ToolLease {
                lease_id: lease_id.clone(),
                registration,
                workspace,
                expires_at: Instant::now() + lease_ttl(),
                queue: VecDeque::new(),
                pending_budget_reservations: HashMap::new(),
            },
        );
        self.changed.notify_all();
        Ok(LeaseIdentity {
            lease_id,
            backend_session_id: session_id,
        })
    }

    pub fn heartbeat(&self, identity: &LeaseIdentity) -> Result<(), BridgeFailure> {
        let mut state = self.bridge_lock()?;
        let lease = lease_for_identity(&mut state, identity)?;
        lease.expires_at = Instant::now() + lease_ttl();
        Ok(())
    }

    pub fn unregister(&self, identity: &LeaseIdentity) -> Result<(), BridgeFailure> {
        let mut state = self.bridge_lock()?;
        let lease = lease_for_identity(&mut state, identity)?;
        let session = lease.registration.backend_session_id.clone();
        let lease = state
            .leases
            .remove(&session)
            .expect("validated foreground lease exists");
        state.retired_usage_leases.insert(
            session,
            RetiredUsageLease {
                price: lease.registration.price,
                pending_budget_reservations: lease.pending_budget_reservations,
                expires_at: Instant::now() + USAGE_RECONCILIATION_GRACE,
            },
        );
        self.changed.notify_all();
        Ok(())
    }

    pub fn authorize_session(&self, session_id: &str) -> Result<(), BridgeFailure> {
        validate_id(session_id, "ses_").map_err(invalid_failure)?;
        let mut state = self.bridge_lock()?;
        cleanup_expired(&mut state);
        let owner = lease_owner(&state, session_id)?;
        if state
            .leases
            .get(&owner)
            .is_some_and(|lease| lease.expires_at > Instant::now())
        {
            Ok(())
        } else {
            Err(failure(
                503,
                "foreground_unavailable",
                "No authenticated foreground lease owns this provider turn",
            ))
        }
    }

    pub fn authorize_provider_turn(
        &self,
        request: &ProviderTurnRequest,
    ) -> Result<ProviderTurnDecision, BridgeFailure> {
        validate_id(&request.session_id, "ses_").map_err(invalid_failure)?;
        let mut state = self.bridge_lock()?;
        cleanup_expired(&mut state);
        let owner = lease_owner(&state, &request.session_id)?;
        let durable_spent = state
            .usage
            .values()
            .filter(|usage| {
                lease_owner(&state, &usage.session_id).ok().as_deref() == Some(owner.as_str())
            })
            .map(|usage| usage.cost_usd)
            .sum::<f64>();
        let lease = state.leases.get_mut(&owner).ok_or_else(|| {
            failure(
                503,
                "foreground_unavailable",
                "No authenticated foreground lease owns this provider turn",
            )
        })?;
        if lease.expires_at <= Instant::now() {
            return Err(failure(
                503,
                "lease_expired",
                "The foreground provider-turn lease expired",
            ));
        }
        lease.expires_at = Instant::now() + lease_ttl();
        let requested = request
            .requested_max_output_tokens
            .unwrap_or(16_384)
            .clamp(1, 1_000_000);
        let Some(budget) = lease.registration.budget_usd else {
            return Ok(ProviderTurnDecision {
                max_output_tokens: requested,
                remaining_usd: None,
            });
        };
        let Some(price) = lease.registration.price else {
            return Err(failure(
                500,
                "budget_price_missing",
                "A hard budget lease is missing its trusted model price",
            ));
        };
        prune_budget_reservations(&mut lease.pending_budget_reservations);
        let reserved = lease
            .pending_budget_reservations
            .values()
            .flatten()
            .map(|reservation| reservation.amount_usd)
            .sum::<f64>();
        let spent = lease.registration.baseline_spent_usd.max(durable_spent) + reserved;
        let remaining = (budget - spent).max(0.0);
        if remaining <= f64::EPSILON {
            return Err(failure(
                402,
                "budget_exhausted",
                "The configured AIShe session budget is exhausted",
            ));
        }
        let affordable = if price.output > 0.0 {
            ((remaining * 1_000_000.0) / price.output).floor() as u64
        } else {
            requested
        };
        let cap = requested.min(affordable);
        if cap == 0 {
            return Err(failure(
                402,
                "budget_exhausted",
                "The remaining AIShe budget cannot authorize another output token",
            ));
        }
        let reservation = cap as f64 / 1_000_000.0 * price.output;
        lease
            .pending_budget_reservations
            .entry(request.session_id.clone())
            .or_default()
            .push_back(BudgetReservation {
                amount_usd: reservation,
                expires_at: Instant::now() + BUDGET_RESERVATION_TTL,
            });
        Ok(ProviderTurnDecision {
            max_output_tokens: cap,
            remaining_usd: Some(remaining),
        })
    }

    pub fn record_provider_usage(&self, report: ProviderUsageReport) -> Result<(), BridgeFailure> {
        validate_id(&report.session_id, "ses_").map_err(invalid_failure)?;
        validate_id(&report.message_id, "msg_").map_err(invalid_failure)?;
        if report
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(failure(
                400,
                "invalid_usage",
                "Provider usage cost is invalid",
            ));
        }
        let mut state = self.bridge_lock()?;
        if state.usage.contains_key(&report.message_id) {
            return Ok(());
        }
        cleanup_expired(&mut state);
        let owner = lease_owner(&state, &report.session_id)?;
        let cost = if let Some(lease) = state.leases.get_mut(&owner) {
            prune_budget_reservations(&mut lease.pending_budget_reservations);
            let cost = report.cost_usd.or_else(|| {
                lease.registration.price.map(|price| {
                    crate::usage::cost(
                        crate::usage::Usage {
                            input: report.input_tokens,
                            output: report.output_tokens,
                            requests: 1,
                        },
                        price,
                    )
                })
            });
            if let Some(queue) = lease
                .pending_budget_reservations
                .get_mut(&report.session_id)
            {
                queue.pop_front();
            }
            cost
        } else if let Some(lease) = state.retired_usage_leases.get_mut(&owner) {
            prune_budget_reservations(&mut lease.pending_budget_reservations);
            let cost = report.cost_usd.or_else(|| {
                lease.price.map(|price| {
                    crate::usage::cost(
                        crate::usage::Usage {
                            input: report.input_tokens,
                            output: report.output_tokens,
                            requests: 1,
                        },
                        price,
                    )
                })
            });
            if let Some(queue) = lease
                .pending_budget_reservations
                .get_mut(&report.session_id)
            {
                queue.pop_front();
            }
            cost
        } else {
            return Err(failure(
                503,
                "foreground_unavailable",
                "No live or recently completed foreground lease owns this usage report",
            ));
        };
        // Unknown-price sessions cannot enforce a hard budget, but still
        // accept and de-duplicate their usage event.
        let cost = cost.unwrap_or(0.0);
        state.usage.insert(
            report.message_id.clone(),
            DurableUsage {
                session_id: report.session_id,
                message_id: report.message_id,
                input_tokens: report.input_tokens,
                output_tokens: report.output_tokens,
                cost_usd: cost,
            },
        );
        drop(state);
        self.persist().map_err(internal_failure)
    }

    /// Attach an OpenCode child session to the same authority as its parent.
    /// The ancestry is durable, but the foreground lease itself is not: after
    /// restart a live AIShe client must still re-register the root session.
    pub fn register_child(&self, child: ChildRegistration) -> Result<(), BridgeFailure> {
        validate_id(&child.parent_session_id, "ses_").map_err(invalid_failure)?;
        validate_id(&child.child_session_id, "ses_").map_err(invalid_failure)?;
        if child.parent_session_id == child.child_session_id {
            return Err(failure(
                400,
                "invalid_ancestry",
                "A child session cannot be its own parent",
            ));
        }
        let mut state = self.bridge_lock()?;
        cleanup_expired(&mut state);
        let owner = lease_owner(&state, &child.parent_session_id)?;
        if !state
            .leases
            .get(&owner)
            .is_some_and(|lease| lease.expires_at > Instant::now())
        {
            return Err(failure(
                503,
                "foreground_unavailable",
                "No authenticated foreground lease owns the parent session",
            ));
        }
        if let Some(existing) = state.children.get(&child.child_session_id) {
            return if existing == &child.parent_session_id {
                Ok(())
            } else {
                Err(failure(
                    409,
                    "ancestry_conflict",
                    "Child session is already attached to another parent",
                ))
            };
        }
        let mut cursor = child.parent_session_id.as_str();
        for _ in 0..32 {
            if cursor == child.child_session_id {
                return Err(failure(
                    400,
                    "ancestry_cycle",
                    "Child session ancestry would form a cycle",
                ));
            }
            match state.children.get(cursor) {
                Some(parent) => cursor = parent,
                None => break,
            }
        }
        state
            .children
            .insert(child.child_session_id, child.parent_session_id);
        drop(state);
        self.persist().map_err(internal_failure)?;
        self.changed.notify_all();
        Ok(())
    }

    pub fn next(
        &self,
        identity: &LeaseIdentity,
        wait: Duration,
    ) -> Result<Option<ToolWork>, BridgeFailure> {
        let deadline = Instant::now() + wait.min(Duration::from_secs(25));
        let mut state = self.bridge_lock()?;
        loop {
            let lease = lease_for_identity(&mut state, identity)?;
            lease.expires_at = Instant::now() + lease_ttl();
            if let Some(key) = lease.queue.pop_front() {
                let workspace = lease.workspace.clone();
                let registration = lease.registration.clone();
                let call = state.calls.get_mut(&key).ok_or_else(|| {
                    failure(
                        500,
                        "journal_inconsistent",
                        "Tool journal lost an admitted request",
                    )
                })?;
                call.durable.status = CallStatus::Dispatched;
                call.durable.updated_at_ms = now_ms();
                let request = call.request.as_ref().ok_or_else(|| {
                    failure(
                        409,
                        "outcome_unknown",
                        "Tool request was recovered without executable arguments",
                    )
                })?;
                let work = ToolWork {
                    session_id: key.session_id,
                    message_id: key.message_id,
                    call_id: key.call_id,
                    tool: request.tool.clone(),
                    args: request.args.clone(),
                    workspace,
                    mode: registration.mode,
                    scope: registration.scope,
                    network: registration.network,
                    interactive: registration.interactive,
                };
                drop(state);
                self.persist().map_err(internal_failure)?;
                return Ok(Some(work));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let duration = deadline.saturating_duration_since(now);
            let (next, _) = self
                .changed
                .wait_timeout(state, duration)
                .map_err(|_| internal_failure(anyhow::anyhow!("bridge state is poisoned")))?;
            state = next;
        }
    }

    pub fn started(&self, started: &ToolStarted) -> Result<(), BridgeFailure> {
        let key = key_from_started(started)?;
        let mut state = self.bridge_lock()?;
        validate_call_owner(&mut state, &started.lease_id, &key)?;
        // Renew while a tool is in flight so multi-minute run_command does not
        // drop provider-turn authority before the next model step.
        renew_lease_by_id(&mut state, &started.lease_id);
        let call = state
            .calls
            .get_mut(&key)
            .ok_or_else(|| failure(404, "call_not_found", "Tool call is not admitted"))?;
        if call.durable.status != CallStatus::Dispatched {
            return Err(failure(
                409,
                "invalid_call_state",
                "Tool call cannot enter started state",
            ));
        }
        call.durable.status = CallStatus::Started;
        call.durable.updated_at_ms = now_ms();
        drop(state);
        self.persist().map_err(internal_failure)
    }

    pub fn complete(&self, completion: ToolCompletion) -> Result<(), BridgeFailure> {
        let key = key_from_completion(&completion)?;
        validate_output(&completion.output)?;
        let mut state = self.bridge_lock()?;
        validate_call_owner(&mut state, &completion.lease_id, &key)?;
        renew_lease_by_id(&mut state, &completion.lease_id);
        let call = state
            .calls
            .get_mut(&key)
            .ok_or_else(|| failure(404, "call_not_found", "Tool call is not admitted"))?;
        if call.durable.status == CallStatus::Completed {
            return Ok(());
        }
        if call.durable.status != CallStatus::Started {
            return Err(failure(
                409,
                "invalid_call_state",
                "Tool call must be started before completion",
            ));
        }
        let full = ToolOutcome {
            success: completion.success,
            output: completion.output,
            exit_code: completion.exit_code,
            replayed: false,
        };
        call.full_result = Some(full.clone());
        call.durable.result = Some(ToolOutcome {
            success: full.success,
            output: redact_value(&full.output),
            exit_code: full.exit_code,
            replayed: false,
        });
        call.durable.status = CallStatus::Completed;
        call.durable.updated_at_ms = now_ms();
        drop(state);
        self.persist().map_err(internal_failure)?;
        self.changed.notify_all();
        Ok(())
    }

    pub fn admit_and_wait(
        &self,
        mut request: PluginToolRequest,
    ) -> Result<ToolOutcome, BridgeFailure> {
        validate_plugin_request(&request)?;
        if request.args.get("_aishe_call_id").and_then(Value::as_str)
            != Some(request.call_id.as_str())
        {
            return Err(failure(
                400,
                "call_identity_mismatch",
                "Tool call identity did not originate from OpenCode",
            ));
        }
        if let Some(args) = request.args.as_object_mut() {
            args.remove("_aishe_call_id");
        }
        validate_tool_args(&request.tool, &request.args)?;
        let key = CallKey {
            session_id: request.session_id.clone(),
            message_id: request.message_id.clone(),
            call_id: request.call_id.clone(),
        };
        let deadline = Instant::now() + TOOL_WAIT;
        let mut state = self.bridge_lock()?;
        cleanup_expired(&mut state);
        let owner_session = lease_owner(&state, &request.session_id)?;

        if let Some(call) = state.calls.get(&key) {
            match call.durable.status {
                CallStatus::Completed => {
                    let mut result = call
                        .full_result
                        .clone()
                        .or_else(|| call.durable.result.clone())
                        .ok_or_else(|| {
                            failure(
                                500,
                                "journal_inconsistent",
                                "Completed tool call has no result",
                            )
                        })?;
                    result.replayed = true;
                    return Ok(result);
                }
                CallStatus::Started | CallStatus::OutcomeUnknown => {
                    return Err(failure(
                        409,
                        "outcome_unknown",
                        "A prior execution started but its outcome is unknown; inspect state before retrying",
                    ))
                }
                CallStatus::Cancelled => {
                    return Err(failure(409, "call_cancelled", "Tool call was cancelled"))
                }
                CallStatus::Admitted if call.request.is_none() => {
                    return Err(failure(
                        409,
                        "outcome_unknown",
                        "A recovered tool request has no executable arguments; inspect state before retrying",
                    ))
                }
                CallStatus::Admitted | CallStatus::Dispatched => {}
            }
        } else {
            let lease = state.leases.get_mut(&owner_session).ok_or_else(|| {
                failure(
                    503,
                    "foreground_unavailable",
                    "No authenticated foreground lease owns this session",
                )
            })?;
            if lease.expires_at <= Instant::now() {
                return Err(failure(
                    503,
                    "lease_expired",
                    "The foreground tool lease expired",
                ));
            }
            validate_request_workspace(&request, lease)?;
            let now = now_ms();
            state.calls.insert(
                key.clone(),
                ActiveCall {
                    durable: DurableCall {
                        key: key.clone(),
                        owner_session_id: owner_session.clone(),
                        tool: request.tool.clone(),
                        args: redact_value(&request.args),
                        status: CallStatus::Admitted,
                        result: None,
                        created_at_ms: now,
                        updated_at_ms: now,
                    },
                    request: Some(request),
                    full_result: None,
                },
            );
            state
                .leases
                .get_mut(&owner_session)
                .expect("lease validated above")
                .queue
                .push_back(key.clone());
            drop(state);
            self.persist().map_err(internal_failure)?;
            self.changed.notify_all();
            state = self.bridge_lock()?;
        }

        loop {
            let call = state
                .calls
                .get(&key)
                .ok_or_else(|| failure(500, "journal_inconsistent", "Tool call disappeared"))?;
            match call.durable.status {
                CallStatus::Completed => {
                    return call
                        .full_result
                        .clone()
                        .or_else(|| call.durable.result.clone())
                        .ok_or_else(|| {
                            failure(
                                500,
                                "journal_inconsistent",
                                "Completed tool call has no result",
                            )
                        })
                }
                CallStatus::OutcomeUnknown => {
                    return Err(failure(
                        409,
                        "outcome_unknown",
                        "Tool execution outcome is unknown",
                    ))
                }
                CallStatus::Cancelled => {
                    return Err(failure(409, "call_cancelled", "Tool call was cancelled"))
                }
                _ => {}
            }
            if Instant::now() >= deadline {
                return Err(failure(
                    504,
                    "tool_timeout",
                    "Timed out waiting for the foreground tool result",
                ));
            }
            if !state
                .leases
                .get(&owner_session)
                .is_some_and(|lease| lease.expires_at > Instant::now())
            {
                return Err(failure(
                    503,
                    "foreground_unavailable",
                    "Foreground tool lease was lost before completion",
                ));
            }
            let (next, _) = self
                .changed
                .wait_timeout(state, Duration::from_secs(1))
                .map_err(|_| internal_failure(anyhow::anyhow!("bridge state is poisoned")))?;
            state = next;
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, BridgeState>> {
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("bridge state is poisoned"))
    }

    fn bridge_lock(&self) -> Result<std::sync::MutexGuard<'_, BridgeState>, BridgeFailure> {
        self.lock().map_err(internal_failure)
    }

    fn persist(&self) -> Result<()> {
        let state = self.lock()?;
        let mut calls: Vec<_> = state
            .calls
            .values()
            .map(|call| call.durable.clone())
            .collect();
        calls.sort_by_key(|call| call.created_at_ms);
        if calls.len() > 10_000 {
            calls.drain(..calls.len() - 10_000);
        }
        let journal = Journal {
            schema_version: JOURNAL_SCHEMA_VERSION,
            children: state.children.clone(),
            calls,
            usage: {
                let mut usage = state.usage.values().cloned().collect::<Vec<_>>();
                usage.sort_by(|left, right| left.message_id.cmp(&right.message_id));
                if usage.len() > 20_000 {
                    usage.drain(..usage.len() - 20_000);
                }
                usage
            },
        };
        let parent = self.journal_path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        crate::config::set_private_dir(parent);
        let bytes = serde_json::to_vec_pretty(&journal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            anyhow::bail!("tool journal exceeds the 16 MiB limit");
        }
        crate::config::write_atomic(&self.journal_path, &bytes)?;
        crate::config::set_private_file(&self.journal_path);
        Ok(())
    }
}

fn load_journal(path: &Path) -> Result<Journal> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Journal {
                schema_version: JOURNAL_SCHEMA_VERSION,
                children: HashMap::new(),
                calls: Vec::new(),
                usage: Vec::new(),
            })
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        anyhow::bail!("tool journal is not a bounded regular file");
    }
    let bytes = std::fs::read(path)?;
    let journal: Journal = serde_json::from_slice(&bytes).context("tool journal is invalid")?;
    if journal.schema_version != JOURNAL_SCHEMA_VERSION {
        anyhow::bail!("tool journal schema mismatch");
    }
    Ok(journal)
}

fn validate_budget_registration(registration: &LeaseRegistration) -> Result<()> {
    if !registration.baseline_spent_usd.is_finite() || registration.baseline_spent_usd < 0.0 {
        anyhow::bail!("baseline budget spend is invalid");
    }
    if registration
        .budget_usd
        .is_some_and(|budget| !budget.is_finite() || budget <= 0.0)
    {
        anyhow::bail!("hard budget must be finite and positive");
    }
    if let Some(price) = registration.price {
        if !price.input.is_finite()
            || !price.output.is_finite()
            || price.input < 0.0
            || price.output < 0.0
        {
            anyhow::bail!("budget price is invalid");
        }
    }
    if registration.budget_usd.is_some() && registration.price.is_none() {
        anyhow::bail!("a hard budget requires an exact trusted price");
    }
    Ok(())
}

fn lease_owner(state: &BridgeState, session_id: &str) -> Result<String, BridgeFailure> {
    let mut cursor = session_id;
    for _ in 0..32 {
        match state.children.get(cursor) {
            Some(parent) => cursor = parent,
            None => return Ok(cursor.to_string()),
        }
    }
    Err(failure(
        409,
        "ancestry_cycle",
        "Session ancestry exceeds the supported depth",
    ))
}

fn lease_for_identity<'a>(
    state: &'a mut BridgeState,
    identity: &LeaseIdentity,
) -> Result<&'a mut ToolLease, BridgeFailure> {
    let lease = state
        .leases
        .get_mut(&identity.backend_session_id)
        .ok_or_else(|| failure(404, "lease_not_found", "Foreground lease does not exist"))?;
    if !constant_time_eq(lease.lease_id.as_bytes(), identity.lease_id.as_bytes()) {
        return Err(failure(
            401,
            "invalid_lease",
            "Foreground lease identity failed",
        ));
    }
    if lease.expires_at <= Instant::now() {
        return Err(failure(410, "lease_expired", "Foreground lease expired"));
    }
    Ok(lease)
}

/// Extend the lease matching `lease_id` if it is still live. Used on tool
/// start/complete so long host work does not race the next model authorize.
fn renew_lease_by_id(state: &mut BridgeState, lease_id: &str) {
    let now = Instant::now();
    if let Some(lease) = state
        .leases
        .values_mut()
        .find(|lease| constant_time_eq(lease.lease_id.as_bytes(), lease_id.as_bytes()))
    {
        if lease.expires_at > now {
            lease.expires_at = now + lease_ttl();
        }
    }
}

fn validate_call_owner(
    state: &mut BridgeState,
    lease_id: &str,
    key: &CallKey,
) -> Result<(), BridgeFailure> {
    let owner_session_id = state
        .calls
        .get(key)
        .map(|call| call.durable.owner_session_id.clone())
        .ok_or_else(|| failure(404, "call_not_found", "Tool call is not admitted"))?;
    let identity = LeaseIdentity {
        lease_id: lease_id.to_string(),
        backend_session_id: owner_session_id,
    };
    lease_for_identity(state, &identity)?;
    Ok(())
}

fn key_from_started(value: &ToolStarted) -> Result<CallKey, BridgeFailure> {
    validate_id(&value.session_id, "ses_").map_err(invalid_failure)?;
    validate_id(&value.message_id, "msg_").map_err(invalid_failure)?;
    validate_nonempty_id(&value.call_id).map_err(invalid_failure)?;
    Ok(CallKey {
        session_id: value.session_id.clone(),
        message_id: value.message_id.clone(),
        call_id: value.call_id.clone(),
    })
}

fn key_from_completion(value: &ToolCompletion) -> Result<CallKey, BridgeFailure> {
    validate_id(&value.session_id, "ses_").map_err(invalid_failure)?;
    validate_id(&value.message_id, "msg_").map_err(invalid_failure)?;
    validate_nonempty_id(&value.call_id).map_err(invalid_failure)?;
    Ok(CallKey {
        session_id: value.session_id.clone(),
        message_id: value.message_id.clone(),
        call_id: value.call_id.clone(),
    })
}

fn validate_plugin_request(request: &PluginToolRequest) -> Result<(), BridgeFailure> {
    validate_id(&request.session_id, "ses_").map_err(invalid_failure)?;
    validate_id(&request.message_id, "msg_").map_err(invalid_failure)?;
    validate_nonempty_id(&request.call_id).map_err(invalid_failure)?;
    if request.agent.len() > 128 || request.agent.chars().any(char::is_control) {
        return Err(failure(400, "invalid_agent", "Invalid agent identity"));
    }
    Ok(())
}

fn validate_request_workspace(
    request: &PluginToolRequest,
    lease: &ToolLease,
) -> Result<(), BridgeFailure> {
    let directory = request.directory.canonicalize().map_err(|_| {
        failure(
            400,
            "invalid_directory",
            "Tool directory does not resolve canonically",
        )
    })?;
    let worktree = request.worktree.canonicalize().map_err(|_| {
        failure(
            400,
            "invalid_worktree",
            "Tool worktree does not resolve canonically",
        )
    })?;
    // OpenCode deliberately reports `/` as `worktree` for a directory that is
    // not backed by version control. That sentinel must never become AIShe
    // authority: the exact request directory still has to match the canonical
    // foreground lease, and ToolWork always carries the lease workspace.
    let global_non_vcs_worktree = worktree == Path::new("/");
    if directory != lease.workspace || (worktree != lease.workspace && !global_non_vcs_worktree) {
        return Err(failure(
            403,
            "workspace_mismatch",
            "OpenCode tool context does not match the foreground lease",
        ));
    }
    Ok(())
}

fn validate_tool_args(tool: &str, args: &Value) -> Result<(), BridgeFailure> {
    let object = args
        .as_object()
        .ok_or_else(|| failure(400, "invalid_tool_args", "Tool arguments must be an object"))?;
    let allowed: &[&str] = match tool {
        "run_command" => &["command", "cwd", "timeout_secs", "interactive"],
        "read_file" | "list_dir" => &["path"],
        "write_file" => &["path", "content"],
        "edit_file" => &["path", "old", "new"],
        "apply_patch" => &["patch"],
        "search_files" => &["query", "path"],
        "fetch_url" => &["url"],
        "use_skill" => &["name"],
        "mcp_call" => &["server", "tool", "arguments"],
        "ask_user" => &["prompt"],
        _ => return Err(failure(400, "unknown_tool", "Unknown AIShe proxy tool")),
    };
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(failure(
            400,
            "invalid_tool_args",
            "Tool arguments contain an unknown field",
        ));
    }
    let required: &[&str] = match tool {
        "run_command" => &["command"],
        "read_file" | "write_file" | "edit_file" | "list_dir" => &["path"],
        "apply_patch" => &["patch"],
        "search_files" => &["query"],
        "fetch_url" => &["url"],
        "use_skill" => &["name"],
        "mcp_call" => &["server", "tool", "arguments"],
        "ask_user" => &["prompt"],
        _ => &[],
    };
    if required.iter().any(|key| !object.contains_key(*key)) {
        return Err(failure(
            400,
            "invalid_tool_args",
            "Tool arguments omit a required field",
        ));
    }
    for (key, value) in object {
        if matches!(key.as_str(), "timeout_secs") {
            if !value
                .as_u64()
                .is_some_and(|number| (1..=3600).contains(&number))
            {
                return Err(failure(400, "invalid_tool_args", "Tool timeout is invalid"));
            }
            continue;
        }
        if matches!(key.as_str(), "interactive") {
            if !value.is_boolean() {
                return Err(failure(
                    400,
                    "invalid_tool_args",
                    "Tool interactive flag is invalid",
                ));
            }
            continue;
        }
        if matches!(key.as_str(), "arguments") {
            if !value.is_object() {
                return Err(failure(
                    400,
                    "invalid_tool_args",
                    "MCP arguments must be an object",
                ));
            }
            continue;
        }
        let text = value
            .as_str()
            .ok_or_else(|| failure(400, "invalid_tool_args", "Tool string argument is invalid"))?;
        let maximum = if matches!(key.as_str(), "content" | "old" | "new" | "patch") {
            4 * 1024 * 1024
        } else if key == "command" {
            65_536
        } else if key == "prompt" {
            16_384
        } else {
            8_192
        };
        if text.len() > maximum || text.chars().any(|character| character == '\0') {
            return Err(failure(
                400,
                "invalid_tool_args",
                "Tool string argument exceeds its safety bound",
            ));
        }
    }
    Ok(())
}

fn validate_output(value: &Value) -> Result<(), BridgeFailure> {
    let bytes = serde_json::to_vec(value).map_err(|error| internal_failure(error.into()))?;
    if bytes.len() > 1024 * 1024 {
        return Err(failure(
            400,
            "tool_output_too_large",
            "Foreground tool output exceeds 1 MiB",
        ));
    }
    Ok(())
}

fn cleanup_expired(state: &mut BridgeState) {
    let now = Instant::now();
    state.leases.retain(|_, lease| lease.expires_at > now);
    state
        .retired_usage_leases
        .retain(|_, lease| lease.expires_at > now);
}

fn prune_budget_reservations(reservations: &mut HashMap<String, VecDeque<BudgetReservation>>) {
    let now = Instant::now();
    reservations.retain(|_, queue| {
        queue.retain(|reservation| reservation.expires_at > now);
        !queue.is_empty()
    });
}

fn validate_shell_id(value: &str) -> Result<()> {
    if value.len() < 16
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid AIShe shell identity");
    }
    Ok(())
}

fn validate_id(value: &str, prefix: &str) -> Result<()> {
    if !value.starts_with(prefix) {
        anyhow::bail!("identity has the wrong prefix");
    }
    validate_nonempty_id(value)
}

fn validate_nonempty_id(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        anyhow::bail!("invalid identity");
    }
    Ok(())
}

fn redact_value(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(crate::redact::redact(text)),
        Value::Array(items) => Value::Array(items.iter().map(redact_value).collect()),
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| {
                    let lower = key.to_ascii_lowercase();
                    let value = if ["token", "password", "secret", "api_key", "authorization"]
                        .iter()
                        .any(|needle| lower.contains(needle))
                    {
                        Value::String("[REDACTED]".into())
                    } else {
                        redact_value(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

fn failure(status: u16, code: &'static str, message: impl Into<String>) -> BridgeFailure {
    BridgeFailure {
        status,
        code,
        message: message.into(),
    }
}

fn invalid_failure(error: anyhow::Error) -> BridgeFailure {
    failure(400, "invalid_identity", error.to_string())
}

fn internal_failure(error: anyhow::Error) -> BridgeFailure {
    failure(500, "bridge_internal", error.to_string())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    super::control::constant_time_eq(left, right)
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut value);
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bridge(name: &str) -> (Bridge, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "aishe-bridge-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();
        (
            Bridge::open(root.join("journal.json")).unwrap(),
            root,
            workspace,
        )
    }

    fn registration(workspace: &Path) -> LeaseRegistration {
        LeaseRegistration {
            aishe_shell_id: "0123456789abcdef".into(),
            backend_session_id: "ses_test".into(),
            workspace: workspace.into(),
            mode: Mode::Yolo,
            scope: ExecutionScope::Workspace,
            network: NetworkPolicy::Deny,
            interactive: false,
            budget_usd: None,
            price: None,
            baseline_spent_usd: 0.0,
        }
    }

    fn request(workspace: &Path) -> PluginToolRequest {
        PluginToolRequest {
            tool: "run_command".into(),
            args: serde_json::json!({"command":"pwd","_aishe_call_id":"call_test"}),
            session_id: "ses_test".into(),
            message_id: "msg_test".into(),
            call_id: "call_test".into(),
            agent: "build".into(),
            directory: workspace.into(),
            worktree: workspace.into(),
        }
    }

    #[test]
    fn run_command_interactive_flag_is_strictly_boolean() {
        assert!(validate_tool_args(
            "run_command",
            &serde_json::json!({
                "command":"sudo -v",
                "interactive":true,
                "timeout_secs":30
            })
        )
        .is_ok());
        assert_eq!(
            validate_tool_args(
                "run_command",
                &serde_json::json!({"command":"sudo -v","interactive":"yes"})
            )
            .unwrap_err()
            .code,
            "invalid_tool_args"
        );
    }

    #[test]
    fn lease_routes_one_call_and_replays_completed_result() {
        let (bridge, root, workspace) = bridge("roundtrip");
        let lease = bridge.register(registration(&workspace)).unwrap();
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| {
                let work = bridge
                    .next(&lease, Duration::from_secs(2))
                    .unwrap()
                    .unwrap();
                bridge
                    .started(&ToolStarted {
                        lease_id: lease.lease_id.clone(),
                        session_id: work.session_id.clone(),
                        message_id: work.message_id.clone(),
                        call_id: work.call_id.clone(),
                    })
                    .unwrap();
                bridge
                    .complete(ToolCompletion {
                        lease_id: lease.lease_id.clone(),
                        session_id: work.session_id,
                        message_id: work.message_id,
                        call_id: work.call_id,
                        success: true,
                        output: Value::String("ok".into()),
                        exit_code: Some(0),
                    })
                    .unwrap();
            });
            let outcome = bridge.admit_and_wait(request(&workspace)).unwrap();
            assert_eq!(outcome.output, "ok");
            worker.join().unwrap();
        });
        let replay = bridge.admit_and_wait(request(&workspace)).unwrap();
        assert!(replay.replayed);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn heartbeat_keeps_provider_authority_past_lease_ttl() {
        set_test_lease_ttl_ms(80);
        let (bridge, root, workspace) = bridge("lease-keepalive");
        let _expired = bridge.register(registration(&workspace)).unwrap();
        // Without heartbeats the short TTL expires and authorize fails closed.
        std::thread::sleep(Duration::from_millis(120));
        let dead = bridge.authorize_session("ses_test").unwrap_err();
        assert_eq!(dead.code, "foreground_unavailable");

        let lease = bridge.register(registration(&workspace)).unwrap();
        // Keepalive every 30ms while TTL is 80ms — mirrors production worker.
        for _ in 0..6 {
            std::thread::sleep(Duration::from_millis(30));
            bridge.heartbeat(&lease).unwrap();
        }
        bridge.authorize_session("ses_test").unwrap();

        // started/complete also renew during long tools so mid-tool waits work.
        std::thread::scope(|scope| {
            let caller = scope.spawn(|| bridge.admit_and_wait(request(&workspace)));
            let work = bridge
                .next(&lease, Duration::from_secs(2))
                .unwrap()
                .unwrap();
            bridge
                .started(&ToolStarted {
                    lease_id: lease.lease_id.clone(),
                    session_id: work.session_id.clone(),
                    message_id: work.message_id.clone(),
                    call_id: work.call_id.clone(),
                })
                .unwrap();
            // Simulate a long host command: keepalive heartbeats (worker thread)
            // while the tool runs past several TTL windows.
            for _ in 0..4 {
                std::thread::sleep(Duration::from_millis(50));
                bridge.heartbeat(&lease).unwrap();
            }
            bridge.authorize_session("ses_test").unwrap();
            bridge
                .complete(ToolCompletion {
                    lease_id: lease.lease_id.clone(),
                    session_id: work.session_id,
                    message_id: work.message_id,
                    call_id: work.call_id,
                    success: true,
                    output: Value::String("ok".into()),
                    exit_code: Some(0),
                })
                .unwrap();
            // complete renews; next model authorize still works.
            bridge.authorize_session("ses_test").unwrap();
            assert!(caller.join().unwrap().is_ok());
        });
        set_test_lease_ttl_ms(0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn started_call_becomes_outcome_unknown_after_foreground_loss() {
        let (bridge, root, workspace) = bridge("cancel-outcome");
        let lease = bridge.register(registration(&workspace)).unwrap();
        std::thread::scope(|scope| {
            let caller = scope.spawn(|| bridge.admit_and_wait(request(&workspace)));
            let work = bridge
                .next(&lease, Duration::from_secs(2))
                .unwrap()
                .unwrap();
            bridge
                .started(&ToolStarted {
                    lease_id: lease.lease_id.clone(),
                    session_id: work.session_id,
                    message_id: work.message_id,
                    call_id: work.call_id,
                })
                .unwrap();
            bridge.unregister(&lease).unwrap();
            assert_eq!(
                caller.join().unwrap().unwrap_err().code,
                "foreground_unavailable"
            );
        });
        drop(bridge);

        let reopened = Bridge::open(root.join("journal.json")).unwrap();
        reopened.register(registration(&workspace)).unwrap();
        let error = reopened.admit_and_wait(request(&workspace)).unwrap_err();
        assert_eq!(error.code, "outcome_unknown");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_lease_or_forged_workspace_fails_closed_and_journal_redacts() {
        let (bridge, root, workspace) = bridge("security");
        let error = bridge.admit_and_wait(request(&workspace)).unwrap_err();
        assert_eq!(error.code, "foreground_unavailable");
        let _lease = bridge.register(registration(&workspace)).unwrap();
        let other = root.join("other");
        std::fs::create_dir_all(&other).unwrap();
        let mut forged = request(&workspace);
        forged.directory = other;
        let error = bridge.admit_and_wait(forged).unwrap_err();
        assert_eq!(error.code, "workspace_mismatch");

        let mut redacted = request(&workspace);
        redacted.call_id = "call_secret".into();
        redacted.args = serde_json::json!({
            "command":"echo sk-proj-abcdefghijklmnopqrstuvwxyz123456",
            "_aishe_call_id":"call_secret"
        });
        // Admission persists before waiting; losing the lease interrupts it.
        let identity = bridge
            .state
            .lock()
            .unwrap()
            .leases
            .get("ses_test")
            .map(|lease| LeaseIdentity {
                lease_id: lease.lease_id.clone(),
                backend_session_id: "ses_test".into(),
            })
            .unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                bridge.unregister(&identity).unwrap();
            });
            let _ = bridge.admit_and_wait(redacted);
        });
        let raw = std::fs::read_to_string(root.join("journal.json")).unwrap();
        assert!(!raw.contains("sk-proj-"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_vcs_worktree_sentinel_does_not_expand_the_lease() {
        let (bridge, root, workspace) = bridge("non-vcs");
        let identity = bridge.register(registration(&workspace)).unwrap();
        let mut candidate = request(&workspace);
        candidate.worktree = PathBuf::from("/");
        let state = bridge.state.lock().unwrap();
        let lease = state
            .leases
            .get(&identity.backend_session_id)
            .expect("registered lease");
        assert!(validate_request_workspace(&candidate, lease).is_ok());

        candidate.directory = root.clone();
        assert_eq!(
            validate_request_workspace(&candidate, lease)
                .unwrap_err()
                .code,
            "workspace_mismatch"
        );
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn child_session_inherits_only_its_live_parent_lease() {
        let (bridge, root, workspace) = bridge("child");
        let lease = bridge.register(registration(&workspace)).unwrap();
        bridge
            .register_child(ChildRegistration {
                parent_session_id: "ses_test".into(),
                child_session_id: "ses_child".into(),
            })
            .unwrap();
        let mut child = request(&workspace);
        child.session_id = "ses_child".into();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let work = bridge
                    .next(&lease, Duration::from_secs(2))
                    .unwrap()
                    .unwrap();
                assert_eq!(work.session_id, "ses_child");
                bridge
                    .started(&ToolStarted {
                        lease_id: lease.lease_id.clone(),
                        session_id: work.session_id.clone(),
                        message_id: work.message_id.clone(),
                        call_id: work.call_id.clone(),
                    })
                    .unwrap();
                bridge
                    .complete(ToolCompletion {
                        lease_id: lease.lease_id.clone(),
                        session_id: work.session_id,
                        message_id: work.message_id,
                        call_id: work.call_id,
                        success: true,
                        output: Value::String("child-ok".into()),
                        exit_code: Some(0),
                    })
                    .unwrap();
            });
            assert_eq!(
                bridge.admit_and_wait(child).unwrap().output,
                Value::String("child-ok".into())
            );
        });
        bridge.unregister(&lease).unwrap();
        assert_eq!(
            bridge.authorize_session("ses_child").unwrap_err().code,
            "foreground_unavailable"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provider_budget_reserves_caps_and_deduplicates_child_usage() {
        let (bridge, root, workspace) = bridge("budget");
        let mut configured = registration(&workspace);
        configured.budget_usd = Some(0.001);
        configured.price = Some(crate::usage::Price {
            input: 0.0,
            output: 100.0,
        });
        let lease = bridge.register(configured).unwrap();
        let first = bridge
            .authorize_provider_turn(&ProviderTurnRequest {
                session_id: "ses_test".into(),
                requested_max_output_tokens: Some(100),
            })
            .unwrap();
        assert_eq!(first.max_output_tokens, 10);
        assert_eq!(
            bridge
                .authorize_provider_turn(&ProviderTurnRequest {
                    session_id: "ses_test".into(),
                    requested_max_output_tokens: Some(100),
                })
                .unwrap_err()
                .code,
            "budget_exhausted"
        );
        bridge
            .record_provider_usage(ProviderUsageReport {
                session_id: "ses_test".into(),
                message_id: "msg_budget_root".into(),
                input_tokens: 1,
                output_tokens: 5,
                cost_usd: Some(0.0005),
            })
            .unwrap();
        // Replay does not spend twice.
        bridge
            .record_provider_usage(ProviderUsageReport {
                session_id: "ses_test".into(),
                message_id: "msg_budget_root".into(),
                input_tokens: 1,
                output_tokens: 5,
                cost_usd: Some(0.0005),
            })
            .unwrap();
        bridge
            .register_child(ChildRegistration {
                parent_session_id: "ses_test".into(),
                child_session_id: "ses_budget_child".into(),
            })
            .unwrap();
        let child = bridge
            .authorize_provider_turn(&ProviderTurnRequest {
                session_id: "ses_budget_child".into(),
                requested_max_output_tokens: Some(100),
            })
            .unwrap();
        assert_eq!(child.max_output_tokens, 5);
        bridge.unregister(&lease).unwrap();
        assert_eq!(
            bridge
                .authorize_provider_turn(&ProviderTurnRequest {
                    session_id: "ses_test".into(),
                    requested_max_output_tokens: Some(1),
                })
                .unwrap_err()
                .code,
            "foreground_unavailable"
        );
        // OpenCode publishes the completed message before its plugin callback
        // necessarily reaches AIShe. Accounting is accepted during a short
        // post-lease grace without restoring provider/tool authority.
        bridge
            .record_provider_usage(ProviderUsageReport {
                session_id: "ses_budget_child".into(),
                message_id: "msg_budget_child".into(),
                input_tokens: 1,
                output_tokens: 0,
                cost_usd: Some(0.0),
            })
            .unwrap();

        // Durable usage survives a bridge reopen and remains secret-free.
        drop(bridge);
        let reopened = Bridge::open(root.join("journal.json")).unwrap();
        let mut configured = registration(&workspace);
        configured.budget_usd = Some(0.001);
        configured.price = Some(crate::usage::Price {
            input: 0.0,
            output: 100.0,
        });
        reopened.register(configured).unwrap();
        let remaining = reopened
            .authorize_provider_turn(&ProviderTurnRequest {
                session_id: "ses_test".into(),
                requested_max_output_tokens: Some(100),
            })
            .unwrap();
        assert_eq!(remaining.max_output_tokens, 5);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_provider_turn_reservations_expire_without_spending_budget() {
        let (bridge, root, workspace) = bridge("budget-expiry");
        let mut configured = registration(&workspace);
        configured.budget_usd = Some(0.001);
        configured.price = Some(crate::usage::Price {
            input: 0.0,
            output: 100.0,
        });
        bridge.register(configured).unwrap();
        assert_eq!(
            bridge
                .authorize_provider_turn(&ProviderTurnRequest {
                    session_id: "ses_test".into(),
                    requested_max_output_tokens: Some(100),
                })
                .unwrap()
                .max_output_tokens,
            10
        );
        {
            let mut state = bridge.state.lock().unwrap();
            let reservation = state
                .leases
                .get_mut("ses_test")
                .unwrap()
                .pending_budget_reservations
                .get_mut("ses_test")
                .unwrap()
                .front_mut()
                .unwrap();
            reservation.expires_at = Instant::now() - Duration::from_millis(1);
        }
        assert_eq!(
            bridge
                .authorize_provider_turn(&ProviderTurnRequest {
                    session_id: "ses_test".into(),
                    requested_max_output_tokens: Some(100),
                })
                .unwrap()
                .max_output_tokens,
            10
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
