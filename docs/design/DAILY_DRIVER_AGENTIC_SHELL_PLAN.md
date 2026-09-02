> **Lifecycle: Active.**  
> **Branch:** `codex/daily-driver-elite`  
> **Baseline:** `v0.7.0` / `f26ee09`  
> **Audience:** maintainers, reviewers, release engineers, and security owners

# AIShe Daily-Driver Agentic Shell Plan

## 1. Executive intent

AIShe should feel like zsh first and an agent second: ordinary commands must stay
instant, predictable, and native, while agentic work gains the context,
background execution, isolation, review, and lifecycle controls expected from a
serious coding agent. The product must not become a full-screen terminal IDE or
replace shell primitives that zsh, git, and the operating system already solve.

This plan delivers twenty connected enhancements. Existing AIShe machinery is
the foundation: real-zsh PTY integration, deterministic routing, managed
OpenCode sessions, native fallback providers, scoped tools, project trust,
context inspection, staged overlays, undo journals, usage accounting, MCP,
semantic history, and versioned JSON contracts. New work extends those seams.

The release is successful when a user can install AIShe on a clean machine,
work normally in zsh, invoke AI without changing mental modes, send a long task
to the background, inspect its exact authority and context, review isolated
changes, recover from interruption, and upgrade or roll back without losing
state.

## 2. Non-negotiable product rules

1. A known shell command never waits for a model, backend, index, or network.
2. Tab, job control, aliases, functions, history, and plugins remain native zsh.
3. AI completion is explicit; it never steals ordinary completion or Enter.
4. Every write has a visible scope and a recoverable or isolated path.
5. Approval defaults are narrow, expiring, and inspectable.
6. Secrets are never serialized into prompts, logs, task files, command lines,
   or support bundles.
7. Background work never interleaves arbitrary output with the editable prompt.
8. Context is bounded, attributable, and controllable by the user.
9. Machine output is schema-versioned, plain, and stable.
10. Unicode and color improve presentation but are never functional
    dependencies. ASCII, `NO_COLOR`, `TERM=dumb`, SSH, and narrow widths work.
11. Project configuration may narrow authority without trust; widening it
    requires content-hash trust.
12. No daemon is added when the existing managed supervisor or a short-lived
    process can own the lifecycle.

## 3. Current baseline and gaps

| Area | Existing foundation | Required delta |
|---|---|---|
| Shell editing | real zsh/ZLE; suggested commands can be staged | operate on the current buffer explicitly |
| Sessions | managed and legacy durable records; resume/reset | branch identity, background lifecycle, checkpoints |
| Safety | modes, scope, network, confirmation tiers, trust | task-scoped expiring approval leases and resource caps |
| File changes | built-in undo and whole-tree dry-run overlay | worktree isolation and hunk-level apply/reject |
| Context | bounded sections plus explain/include/exclude | attachments, pin/drop controls, repo index attribution |
| Failure recovery | last-command fix shortcut | durable bounded failure capsule and explain/retry actions |
| Models | connections, reasoning, fallback providers | explicit role routing with visible effective selection |
| MCP | stdio/HTTP client and inventory | transactional CRUD, auth references, health, permissions |
| Environment | host facts and Kubernetes context | production-risk identity and protected-context gates |
| Usage | token/cost accounting and session budget | per-task time/tool/file/download/network budgets |
| Audit | structured log, session export, replay | unified last-turn trace and reproducibility manifest |
| Lifecycle | installer, managed-runtime repair/rollback | binary update check/apply/rollback and profile backup |

## 4. Target interaction model

### 4.1 Foreground shell

- A normal line is handled exactly as it is today.
- `Ctrl-X A` sends the current buffer to the buffer copilot and replaces the
  buffer only after a valid single-command result returns.
- `Ctrl-X F` opens the last failure capsule: fix is the default action, while
  explain and safe retry are explicit alternatives.
- `Ctrl-X Space` opens a generated command palette; it does not reserve
  printable keys or replace slash commands.
- `@file`, `@dir`, `@diff`, and `@clipboard` are expanded only on agent-routed
  lines and are shown in context metadata before provider dispatch.

### 4.2 Background work

```text
aishe task start "upgrade the parser and run its tests"
aishe task list
aishe task show TASK
aishe task tail TASK
aishe task cancel TASK
aishe task resume TASK
aishe task review TASK
aishe task apply TASK [--hunk N ...]
aishe task discard TASK
```

The starter returns after durable state and the child process are established.
Output goes to a private task log. The live statusline shows only a bounded task
count/state summary. Cancel is idempotent; a dead process becomes interrupted,
never silently successful. Writable git tasks use an isolated worktree by
default. Non-git directories use the existing copy/overlay path where supported
and otherwise require explicit foreground execution.

### 4.3 Agent result routing

```text
aishe ask "summarize the API"                 # human answer
aishe ask --json "extract endpoints"         # versioned document
aishe ask --schema schema.json "extract…"     # validated JSON payload
aishe ask --insert "write the curl command"   # shell-hook buffer handoff
```

Human output never becomes executable merely because it resembles shell. Buffer
insertion and stdout capture are explicit actions.

## 5. Enhancement specifications

### E01 — Current-buffer copilot

**Outcome:** edit the live shell buffer with AI while retaining native ZLE.

- Add a hidden `--edit-line` transport that accepts the current buffer and an
  optional instruction, uses the existing suggest provider path, and emits one
  control-safe command on stdout.
- Bind `Ctrl-X A` by default through the existing conflict-aware keybinding
  diagnostics. Preserve cursor position where possible and never execute.
- Empty buffers, multiline buffers, provider errors, cancellation, and unsafe
  results leave the original buffer byte-for-byte unchanged.
- Display explanation on stderr using the existing proposal renderer.

**Acceptance:** a real-zsh PTY test proves replace, cancel, error preservation,
multiline handling, plugin coexistence, and no execution.

### E02 — Background agent task controller

**Outcome:** long agent work continues while the shell remains usable.

- Add `task start/list/show/tail/cancel/resume/review/apply/discard` commands.
- Store a schema-versioned private record, bounded redacted log, PID plus process
  start identity, timestamps, exit state, workspace, branch, budgets, and
  isolated-change location.
- Spawn the current executable without a shell, remove inherited credential
  variables, and use the established provider credential resolution inside the
  child.
- Reconcile stale PIDs before every mutation. Never signal a reused PID.
- Use atomic writes and a per-task advisory lock.

**Acceptance:** lifecycle tests cover completion, failure, cancellation,
process death, concurrent listing, stale PID defense, log bounds, and secrets.

### E03 — Worktree-isolated writable agents

**Outcome:** an autonomous task cannot dirty the user's active checkout.

- For a clean or dirty git repository, create a detached worktree from `HEAD`
  beneath AIShe's private task root and run the agent there.
- Record the source repository, base commit, source branch, worktree path, and
  resulting patch identity. Never copy `.git` or follow repository symlinks.
- `review` compares the task worktree with the recorded base. `apply` uses git's
  native three-way application into the current tree and refuses unresolved
  conflicts. `discard` removes only the validated AIShe-owned worktree.
- If git is missing or the cwd is not a worktree, use the existing bounded
  overlay only when its platform requirements pass.

**Acceptance:** dirty source trees stay untouched; create/modify/delete/rename,
conflict, cancellation, and discard are exercised in temporary repositories.

### E04 — Task-scoped approval leases

**Outcome:** reduce approval fatigue without widening global authority.

- An approval may be one-shot or scoped to exact tool, normalized command head,
  workspace root, network policy, and task ID.
- Leases live only for the active foreground turn or durable task, expire at
  task completion/cancellation and after a bounded wall time, and are never
  imported from project config.
- Shell metacharacters, interpreter payloads, host scope, package publishing,
  credential tools, and destructive classifications cannot use a broad lease.
- Every approval panel shows the effective lease and a direct revoke action.

**Acceptance:** table-driven tests prove that argument, cwd, scope, task,
timeout, and danger-class changes invalidate the lease.

### E05 — Context cockpit

**Outcome:** make context size and provenance understandable before spending.

- Extend the existing `context` command with an interactive picker when no
  mutation flags are supplied on a TTY.
- Show each section's source, inclusion reason, character/token estimate, trust
  state, freshness, and redaction count without echoing secret-bearing content.
- Support session-only pin/drop and existing durable include/exclude controls.
- Show total estimated context against the configured/model limit and reserve
  response capacity before dispatch.

**Acceptance:** terminal and JSON snapshots cover empty, oversized, untrusted,
redacted, attachment, and narrow-terminal cases.

### E06 — Native attachment grammar

**Outcome:** attach local context without pasting it into natural language.

- Parse `@file:path`, `@dir:path`, `@diff`, and `@clipboard` only after a line is
  conclusively agent-routed. Plain shell arguments beginning with `@` remain
  untouched.
- Resolve relative paths under the request cwd, reject parent/symlink escapes
  for workspace scope, honor ignore files, and cap file count, per-file bytes,
  aggregate bytes, and directory depth.
- Binary inputs become metadata unless an explicitly supported image type is
  used. Clipboard access is opt-in and never attempted in noninteractive mode.
- The prompt receives labeled bounded sections; audit stores metadata only.

**Acceptance:** parser fuzzing covers quoting, spaces, Unicode, symlinks,
devices, FIFOs, huge trees, binary files, and shell-route noninterference.

### E07 — Durable failure capsule

**Outcome:** explain or repair a failure without rerunning it to gather context.

- The zsh hook records the last failed command, exit status, cwd, duration, and
  bounded sanitized tail of captured output in a mode-0600 per-shell file.
- Add `last show|explain|fix|retry|clear`; `Ctrl-X F` invokes `last fix`.
- Retry is offered only when the existing read-only rerun classifier approves;
  otherwise it stages the command for review.
- A new successful command replaces the capsule; shutdown removes ephemeral
  shell capsules unless retention is explicitly enabled.

**Acceptance:** tests cover invalid UTF-8, ANSI/bidi controls, large output,
pipelines, secret values, interrupted commands, and safe/unsafe retry.

### E08 — Checkpointed plans

**Outcome:** make long autonomous work inspectable and resumable by step.

- Persist a bounded ordered checklist with stable step IDs, state, evidence,
  and optional user-edited text alongside the task/session.
- Add `task plan`, `task step`, and `task replan`; foreground rendering shows
  the current step and a bounded completed/total count.
- A checkpoint is truthful only when tool evidence is complete. Provider prose
  cannot mark a step complete by itself.
- Replanning retains completed evidence and records the superseded plan.

**Acceptance:** schema compatibility and state-machine tests cover reordering,
resume, interruption, duplicate events, and impossible transitions.

### E09 — Incremental repository index

**Outcome:** retrieve relevant code without sending or scanning the whole tree.

- Index tracked, non-ignored text files by content hash. Store path, language,
  symbol-like headings, bounded chunks, and optional embeddings under the data
  root keyed by repository identity.
- Use `git ls-files` when available; otherwise use the existing bounded walker.
- `aishe index [--rebuild|--status]` is explicit. Agent retrieval is opt-in and
  records selected chunk paths and hashes in context metadata.
- Reuse the configured embedding provider and vector math from semantic
  history. No new database or daemon.

**Acceptance:** incremental add/change/delete/rename, ignore rules, worktrees,
subdirectories, corruption recovery, offline embeddings, and secret redaction.

### E10 — Explicit AI command composer

**Outcome:** generate a command quickly without making Tab network-dependent.

- Build on E01 with a command-focused action and a short latency budget.
- If the budget expires, preserve the buffer and print one quiet hint; do not
  continue repainting after the user types.
- Cache by normalized buffer, cwd context hash, connection, and model for the
  current shell only.
- Never auto-accept or execute a generated command.

**Acceptance:** direct shell and Tab latency remain at baseline; cancellation
and stale-result tests prove old completions never overwrite newer input.

### E11 — Role-based model routing

**Outcome:** use the right cost/latency tier for compose, answer, build, review,
and embeddings.

- Add optional role bindings that reference existing named connections plus
  model/reasoning overrides; missing bindings fall back to the active selection.
- Resolve once per turn and show the effective role/connection/model before the
  first paid request and in usage attribution.
- Role routing never changes authentication identity implicitly and never
  crosses a connection allowlist.

**Acceptance:** precedence tests cover shell-local selection, project narrowing,
policy allowlists, unavailable roles, fallback chains, and cost attribution.

### E12 — MCP control plane

**Outcome:** manage tools without hand-editing TOML.

- Add `mcp list/show/add/edit/remove/enable/disable/test` with JSON forms.
- Support stdio argv arrays or HTTP URLs plus references to named credential
  profiles; never accept secret values on the command line.
- Changes are transactional, preserve unrelated config/comments when feasible,
  and expose tool/prompt/resource inventory plus capability health.
- Project MCP definitions retain the existing trust boundary.

**Acceptance:** fake stdio/HTTP servers cover handshake, timeout, auth reference,
name collision, rollback, disabled state, and redacted output.

### E13 — Environment identity and production guardrails

**Outcome:** make dangerous target context unmistakable.

- Resolve hostname, SSH, container, git branch, detached HEAD, Kubernetes
  context/namespace, and common cloud account/profile identifiers locally.
- Classify production only from explicit configurable patterns; never infer a
  destructive permission from the classification.
- Show a persistent `PROD`/remote marker in status and approval panels. Require
  fresh typed confirmation for host-scope writes in a protected context.
- Store only safe identifiers in task records and audit.

**Acceptance:** fixtures cover absent CLIs/config, hostile control characters,
pattern boundaries, project attempts to disable protection, and JSON output.

### E14 — Secret broker

**Outcome:** let approved commands use credentials without exposing values to
the model or long-lived agent environment.

- Tools request a named profile and target environment variable; the model sees
  only profile identity and availability.
- Resolve the value immediately before spawning the exact child, pass it via a
  private environment map, and drop it immediately afterward.
- Deny secret delivery to shell interpreters, host scope, arbitrary environment
  names, untrusted MCP servers, logs, previews, and background task metadata.
- Record only profile, consumer, reason, and result in audit.

**Acceptance:** process inspection, audit, support bundle, crash, cancellation,
and child-output tests prove the secret is absent everywhere except the child.

### E15 — Per-task resource budgets

**Outcome:** bound runaway autonomy independently of a session-wide cost cap.

- Add optional limits for cost, input/output tokens, wall time, provider turns,
  tool calls, changed files/bytes, subprocess output, downloads, and network
  calls.
- Snapshot limits into each task so later config changes cannot widen it.
- Check before an effect, reserve where needed, reconcile authoritative usage,
  and stop with a stable reason before crossing a hard limit.
- Extending a limit requires an interactive explicit action and is audited.

**Acceptance:** boundary tests cover exact limits, concurrent reservations,
unknown pricing, retries, child agents, resume, and clock-independent tests.

### E16 — Hunk-level patch review

**Outcome:** accept useful portions of an isolated task without taking all edits.

- Generate a stable patch from E03 and divide text changes into numbered hunks;
  binary/create/delete operations remain file-level.
- `task review` renders bounded diffs; `task apply --hunk` constructs a selected
  patch and uses git's native apply/check path.
- Record selected and rejected hunk identities plus resulting commit/tree
  identity. Applying twice is rejected cleanly.

**Acceptance:** tests cover adjacent hunks, new/deleted files, mode changes,
renames, binary files, whitespace, conflict, partial apply, and undo.

### E17 — Branch-aware conversations

**Outcome:** prevent one branch's assumptions from silently contaminating
another branch.

- Add repository identity, branch, and HEAD to managed session bindings.
- When identity changes, prompt to continue, fork, or start fresh; noninteractive
  operation defaults to a new session.
- `session fork` creates a new binding while retaining a bounded provenance link
  and generated summary, not a copy of secret-bearing raw records.
- Detached HEAD and non-git workspaces have explicit stable identities.

**Acceptance:** tests cover checkout, rename, worktree, rebase, detached HEAD,
deleted branch, resume with replacement cwd, and project trust changes.

### E18 — Structured result capture

**Outcome:** safely compose agents with Unix automation.

- Add `ask` with human, `--json`, `--schema`, and hidden `--insert` forms.
- Schema validation is local, bounded, and fail-closed; invalid output never
  reaches stdout in a success state.
- Human diagnostics use stderr; machine stdout contains one versioned document.
- Buffer insertion is restricted to the active shell handoff file and does not
  execute.

**Acceptance:** contract fixtures cover valid/invalid schema, streamed output,
provider failure, control characters, pipe closure, and shell insertion.

### E19 — Unified command palette

**Outcome:** make the growing capability surface discoverable without memorizing
commands.

- Generate entries from `CommandSpec`, plus settings sections, sessions,
  connections, models, and keybindings.
- Bind `Ctrl-X Space` when available. Selecting an effectful action stages its
  command in `BUFFER`; read-only actions may execute after selection.
- Use the existing filter picker, terminal capability system, and command
  metadata. No second registry.

**Acceptance:** registry conformance proves every entry has identity, label,
effect classification, and valid invocation; PTY tests cover conflicts and
ASCII/static modes.

### E20 — Binary lifecycle and profile portability

**Outcome:** upgrades become one safe workflow with an immediate recovery path.

- Add `update check|apply|rollback` around signed/checksummed release artifacts.
  Reuse installer platform resolution and managed-runtime compatibility checks.
- Keep exactly one verified prior binary. Do not roll config backward
  automatically; enforce the documented schema compatibility policy.
- Add `profile export/import` for non-secret configuration and optional
  separately encrypted credential material. Default export contains no secrets.
- Preview every path and version before mutation; use atomic replacement.

**Acceptance:** local fixture releases cover no update, upgrade, checksum/signature
failure, interrupted activation, rollback, incompatible config, and secret-free
export/import. Live network qualification remains a release gate, not a unit
test dependency.

## 6. Shared architecture

### 6.1 New modules

Keep modules narrow and reuse current contracts:

| Module | Owns | Reuses |
|---|---|---|
| `attachments` | agent-only `@` parsing and bounded reads | context, trust, redaction |
| `background` | task process records and lifecycle | tasks, backend sessions, audit |
| `failure` | last-failure capsule | fix, display-safe, atomic files |
| `repo_index` | content-hash code chunks and retrieval | semhist embedding math |
| `environment` | safe target identity and protection | context host facts |
| `roles` | workload-specific model selection | named connections and policy |
| `mcp_config` | transactional MCP management | existing MCP registry |
| `palette` | generated searchable actions | command registry and picker |
| `lifecycle` | binary/profile transactions | config atomic-write and HTTP patterns |

Worktree ownership, scoped approvals, and budget enforcement stay in the
existing background/tool-worker paths; separate one-implementation abstraction
modules would add no boundary.

Do not add a generic plugin/event framework, database, async runtime, TUI
framework, font package, or second agent backend abstraction.

### 6.2 Persistent state

All new state lives below the existing config/data roots:

```text
$XDG_DATA_HOME/aishe/
  background-tasks/<id>/record.json
  background-tasks/<id>/request
  background-tasks/<id>/activity.log
  background-tasks/<id>/worktree/
  repo-index/<worktree-id>/index.json
  failures/<shell-id-hash>.json
  updates/previous-aishe
```

Directories are `0700`, files are `0600`, symlinks are rejected at ownership
boundaries, records are bounded, and every durable JSON shape has
`schema_version`. Task deletion and worktree discard require exact validated
ownership. Uninstall preserves all user state unless its existing explicit
category flags select removal.

### 6.3 Statusline

The editable command remains the first visual row. The statusline is always a
non-editable row below it and contains a width-aware subset of:

```text
connection/model · mode/scope · branch/environment · task step · cost · jobs
```

It yields temporarily when another AIShe ZLE widget uses `POSTDISPLAY` to retain
submitted text, then returns at the next prompt. It never becomes `PROMPT` or
`RPROMPT`, never executes prompt substitution, and degrades to ASCII/plain text.

### 6.4 Security boundaries

- CLI strings are data; subprocesses use argv, never constructed shell command
  lines, except when the requested artifact is explicitly shell code.
- PID operations verify both ownership record and process start identity.
- Worktree removal validates canonical containment and git registration.
- Attachment and index readers reject devices, sockets, FIFOs, and symlink
  escapes and cap work before allocation.
- Task logs and provider-visible content pass through redaction independently.
- Resource limits fail before effects and remain narrow after resume.
- Project config cannot define approval leases, secret delivery, update sources,
  or weaker protected-environment behavior.

## 7. Delivery phases and dependencies

### Phase 0 — Current reliability baseline

- Finish transactional install/setup changes.
- Make below-input status placement the only enabled behavior.
- Complete Unicode/ASCII prompt fallback without a font dependency.
- Freeze clean tests before adding feature surface.

### Phase 1 — Foreground ergonomics

E01 buffer copilot → E07 failure capsule → E18 structured capture → E19 palette.
These share ZLE handoff, safe stdout/stderr separation, and command metadata.

### Phase 2 — Context intelligence

E06 attachments → E05 context cockpit → E09 repository index → E11 model roles.
Every source becomes attributable and budgeted before retrieval grows.

### Phase 3 — Durable autonomous work

E15 resource budgets → E02 background controller → E03 worktrees → E08 plans →
E16 hunk review → E17 branch-aware sessions. Budgets and identity precede
background writes.

### Phase 4 — Trust and ecosystem

E04 approval leases → E13 environment guardrails → E14 secret broker → E12 MCP
management. Secret delivery lands only after consumers and authority are
inspectable.

### Phase 5 — Lifecycle

E20 update/rollback/profile portability, documentation, migration fixtures,
release evidence, and rollback rehearsal.

## 8. Test and qualification matrix

Every phase must pass the normal fast suite plus its targeted checks. The final
branch must pass:

1. `cargo fmt --check`
2. `cargo clippy --all-targets --all-features --locked -- -D warnings`
3. `cargo test --all-targets --locked`
4. documentation and command-surface contracts
5. shellcheck plus generated zsh/bash syntax checks
6. direct-shell routing and latency benchmark
7. real-zsh feature, routing, signal, resize, statusline, keybinding, and plugin
   PTY suites
8. setup/settings/tour PTY matrix at 40/80/120/200 columns
9. ASCII, Unicode, `NO_COLOR`, `TERM=dumb`, static motion, redirected stdout,
   invalid UTF-8, combining, CJK, emoji, and hostile control text
10. task lifecycle tests under normal completion, failure, SIGINT, SIGTERM,
    parent death, stale PID, concurrent list/show, and restart
11. git worktree tests for clean/dirty repositories, conflict, partial apply,
    branch changes, and cleanup
12. attachment/index fuzzing and size-bound tests
13. approval, protected-environment, secret-broker, and budget boundary tables
14. MCP stdio/HTTP fake-server transactions
15. installer/runtime/update transactional fault injection
16. secret scan of logs, state, support bundles, argv, and exported profiles
17. Linux/macOS release qualification, with unsupported platform behavior stated
    rather than inferred

No paid provider call is required for deterministic CI. Live provider,
subscription OAuth, real SSH/tmux, and release-server update checks are explicit
release-candidate gates with recorded binary identity and cost.

## 9. Compatibility and migration

- Preserve existing commands and JSON v1 shapes; new documents start at v1 and
  old documents remain additive.
- `status_line_position = "right"` loads as `below`; save writes `below`.
- Existing managed sessions without branch identity remain resumable and gain
  identity only when explicitly rebound.
- Legacy task records remain readable under their current commands; background
  task schema-v1 records use a separate directory and cannot shadow them.
- Existing MCP TOML remains valid. CRUD commands write the same schema.
- Missing new config fields use conservative defaults: no background action,
  no role override, no repo retrieval, no secret delivery, and finite task
  budgets derived from existing limits.
- Downgrade never deletes unknown state. A prior binary may ignore new stores;
  config schema changes require the established backup and rollback policy.

## 10. Documentation deliverables

- Root README: one-minute install, first command, buffer copilot, background
  task, review/apply, and recovery path.
- Getting started: foreground versus background journey.
- Commands: generated command surface and exact machine-output contracts.
- Safety: leases, worktrees, protected environments, budgets, and secrets.
- Context: attachments, index provenance, pin/drop behavior, and data limits.
- MCP: transactional management and authentication references.
- Sessions: branches, plans, interruption, resume, retention, and deletion.
- Installation: update/rollback/profile portability and font-free terminal
  compatibility.
- Troubleshooting: one stable code and recovery action for every new failure.
- Architecture: ownership, storage, data flow, and threat boundaries.

## 11. Release and rollback

1. Land behind conservative defaults where provider spend, background effects,
   secret delivery, or repository indexing is involved.
2. Run migration tests from the prior two minor releases.
3. Produce signed/checksummed artifacts, SBOM, notices, and provenance.
4. Install the candidate over the prior release, exercise setup reuse, run a
   background task, apply a partial patch, then roll the binary and runtime back.
5. Confirm config, credentials, history, sessions, tasks, undo, indexes, and
   audit remain present and private.
6. Publish only after macOS and Linux flagship evidence is attached to the
   release candidate.

## 12. Definition of done

The initiative is complete only when all twenty enhancement acceptance clauses
are implemented, documented, and exercised; direct command latency remains
within the existing budget; no new secret exposure or unbounded state path is
introduced; background and foreground tasks recover truthfully after process
loss; all writable background work is isolated or explicitly refused; every
new command has help, completion, machine-output, error, and compatibility
coverage; the installer, updater, and rollback paths preserve user state; and
the entire qualification matrix passes from the exact candidate binary.

## 13. Branch implementation record

The `codex/daily-driver-elite` branch implements the releasable v1 slice of all
twenty enhancements without introducing a daemon, database, terminal UI
framework, or font dependency:

| ID | Implemented surface | Conservative v1 boundary |
|---|---|---|
| E01/E10 | `--edit-line`, `Ctrl-X Ctrl-A`, strict one-command parsing, syntax check, existing timeout/cache | synchronous ZLE request; no network work on Tab |
| E02 | `task start/list/show/tail/cancel/resume`, private atomic record/request/log, process-group and start-identity checks | git isolation required unless `--no-isolation` is explicit |
| E03/E16 | detached worktree, binary patch, numbered hunks, selective/whole `git apply --3way`, owned discard | unresolved conflicts remain for the user; no custom merge engine |
| E04 | exact normalized approval reuse already owned by one live tool worker and authority tuple | expires with the worker/turn; no persisted or project-defined lease |
| E05 | TTY context cockpit plus existing explain/preview/JSON/include/exclude | cockpit changes the durable config so behavior is predictable across prompt subprocesses |
| E06 | explicit file/directory/diff/clipboard grammar with scope, type, depth, count, and byte limits | binary content becomes metadata; no image preprocessing |
| E07 | per-shell failure capsule and `last show/explain/fix/retry/clear` | output interception is intentionally absent; command metadata is reliable and non-invasive |
| E08 | durable task plan/replan/step state and revision | explicit CLI checkpoints; provider prose cannot mark evidence complete |
| E09 | incremental tracked-text content-hash index and bounded local search | lexical ranking only; no extra embedding service or database |
| E11 | compose/answer/build/review/embed bindings with CLI-over-role precedence | unavailable roles fall back to the active selection |
| E12/E14 | MCP CRUD/test with environment-name references resolved at consumer launch; tool commands strip credential variables | no CLI secret values and no general-purpose secret-injection language |
| E13 | safe environment identity, status marker, configurable protected patterns, fresh host-yolo confirmation | classification never grants authority |
| E15 | snapshotted task limits for time, turns, cost, tool/network calls, and changed files/bytes | provider token enforcement remains covered by the existing output/session caps; fetches retain their existing per-call cap |
| E17 | session mapping schema 3 with common-repository, branch, and detached-HEAD identity; schema 1/2 migration | a branch switch safely starts a separate binding noninteractively; explicit provenance-copy/fork UX is deferred |
| E18 | human/JSON/schema `ask` plus private `--insert` handoff | bounded local JSON Schema subset rather than a new validator dependency |
| E19 | registry-derived command/connection palette and `Ctrl-X Space` buffer handoff | selected actions are staged, never auto-executed |
| E20 | update check/apply/rollback and secret-free profile export/import | client verifies HTTPS, SHA-256, archive shape, binary format, and self-test; release workflow owns GitHub provenance attestation |

The limits above are intentional product boundaries, not silent claims of a
stronger sandbox. Revisit them only when usage evidence justifies the added
state or dependency. User operation and exact ceilings are documented in
[`docs/daily-driver.md`](../daily-driver.md).
