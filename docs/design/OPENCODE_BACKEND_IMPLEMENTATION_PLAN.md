---
title: "Aishe OpenCode Agent Backend and Enterprise Setup"
created: "2026-07-29T14:55:11-04:00"
status: "implemented-release-gates-pending"
source: "hotseat-codex-plugin"
codex_goal: true
target_release: "0.5.0"
initial_opencode_version: "1.18.9"
implementation_commit: "b388ee3"
validation_report: "OPENCODE_BACKEND_VALIDATION.md"
validation_data: "OPENCODE_BACKEND_VALIDATION.json"
---

# Aishe OpenCode Agent Backend and Enterprise Setup

## Document Authority

This document is the source of truth for integrating a managed OpenCode server
behind Aishe while preserving Aishe as a native, real zsh command-line
experience. It supersedes older statements in `docs/design/PLAN.md`,
`docs/design/PRD.md`, and `docs/architecture.md` that require a single binary,
forbid a managed service, or describe Aishe's current provider/yolo loop as the
long-term AI architecture.

Those older documents remain useful history. Where they conflict with this
document, this document wins for the OpenCode milestone.

This is a planning artifact. It does not authorize deleting or rewriting
existing config, credentials, shell history, audit logs, undo journals, task
records, or project settings. All migration must be additive, versioned,
transactional, reversible, and covered by upgrade tests.

## Implementation Status

The implementation is complete on `feature/opencode-backend` at candidate
commit `b388ee3`. Deterministic macOS and disposable-Linux qualification,
including the real pinned OpenCode runtime, is recorded in
[`OPENCODE_BACKEND_VALIDATION.md`](OPENCODE_BACKEND_VALIDATION.md).

This status does not authorize publication. The validation report names the
remaining release holds explicitly: the paid live-provider matrix requires a
fresh credential that has never been published, the literal 24-hour lifecycle
soak remains a release-operation gate, and the native compatibility fallback
must remain available through two subsequent minor releases. An exposed key is
never reused merely to turn a checklist green.

## Objective

Turn Aishe into one seamless product that simultaneously behaves as:

1. A genuine interactive Linux/macOS command line backed by the user's real
   zsh, with native aliases, functions, completions, plugins, job control,
   history, prompts, and terminal applications.
2. A mature conversational and coding agent with streaming responses, durable
   sessions, reasoning and tool progress, subagents, compaction, task state,
   cost tracking, interruption, recovery, and polished inline interaction.

OpenCode will become the default engine for every AI request: questions,
command suggestions, `auto` work, and `yolo` agent work. Aishe will remain the
only visible user interface and the only component authorized to execute
model-requested host actions.

The user must never have to open the OpenCode TUI, run `opencode`, configure an
OpenCode account, or understand that a local OpenCode server exists. Advanced
diagnostics may identify the engine by name, and third-party notices must do so,
but normal operation must simply feel like Aishe.

## Background

Aishe 0.4.1 already provides important foundations that must be preserved:

- A real zsh running in a PTY, with an injected Aishe hook rather than a shell
  grammar reimplementation.
- Full-buffer command-versus-natural-language routing, including `?` and `!`
  route overrides and command-name/question disambiguation.
- Interactive, resumable, transactional setup with private credential staging.
- Private shared credentials, environment override precedence, model discovery,
  exact model validation, capability probing, pricing, and statusline setup.
- `suggest`, `auto`, and `yolo` modes.
- Deterministic command safety classification, bubblewrap dry-run support,
  policy checks, audit logging, undo journals, MCP, skills, and durable yolo
  task checkpoints.
- Persistent shared zsh history across sessions and upgrades.
- Release tarballs, checksums, Linux packages, Homebrew support, installer
  preservation tests, PTY tests, fuzz tests, and live-model validation.

The current weakness is not the shell. It is the hand-built provider and
agentic harness: provider-specific tool behavior, session/event handling,
subagents, approvals, compaction, task rendering, and recovery are expensive to
keep maturing independently.

OpenCode already exposes a headless HTTP server with an OpenAPI-described API,
SSE events, sessions, messages, tools, permissions, agents, child sessions,
todos, provider integration, compaction, and structured output. Its published
JavaScript SDK is a client that starts or connects to that server; Aishe is a
Rust application and will communicate with the HTTP/OpenAPI/SSE protocol
directly rather than embedding the JavaScript SDK.

Primary references:

- [OpenCode SDK](https://opencode.ai/docs/sdk/)
- [OpenCode server](https://opencode.ai/docs/server/)
- [OpenCode permissions](https://opencode.ai/docs/permissions/)
- [OpenCode providers](https://opencode.ai/docs/providers/)
- [OpenCode security model](https://github.com/anomalyco/opencode/blob/v1.18.9/SECURITY.md)
- [OpenCode v1.18.9 release](https://github.com/anomalyco/opencode/releases/tag/v1.18.9)

## Product Decisions

These decisions are approved and are not open implementation questions.

| Decision | Approved behavior |
| --- | --- |
| AI backend scope | OpenCode handles every AI interaction: suggest, chat, auto, and yolo. |
| Migration fallback | The current Rust-native provider engine remains a compatibility fallback during rollout. It is not a second normal mode. |
| OpenCode distribution | Aishe installs and manages an exact compatibility-pinned OpenCode runtime in Aishe's private data directory. |
| System OpenCode | Ignore unrelated `opencode` binaries on `PATH` by default. Never attach to an arbitrary server. |
| User interface | Never launch or embed the OpenCode TUI. Aishe renders all interaction inline in the current shell. |
| Tool execution | Disable OpenCode's host-effecting built-in tools. A trusted Aishe OpenCode plugin forwards tool calls to the foreground Aishe client, which applies Aishe policy and executes them. |
| Credentials | Aishe remains the sole credential source of truth. Do not ask users to configure OpenCode credentials and do not copy keys into normal config. |
| Linux sandbox setup | When bubblewrap is missing, setup offers to install it, shows the exact package-manager command, obtains explicit consent before sudo, and runs a functional self-test. |
| Auto semantics | `auto` executes actions classified as safe and asks before risky, unknown, privileged, external-write, or policy-changing actions. |
| Yolo semantics | Yolo asks once when its execution scope is entered, then never asks per action in that shell session. |
| Yolo scopes | `workspace` is the default. `host` is explicit and permits real machine administration after one session-scoped acceptance. |
| macOS yolo | Entering yolo requires one unsandboxed-risk acceptance per Aishe shell session. No later per-action prompts appear in that session. |
| OpenCode v2 | Do not build the production adapter against beta v2. Hide v1 behind an Aishe backend interface so a future adapter can replace it. |
| First release | Treat this as a minor architectural release, initially targeted at Aishe 0.5.0. |

## Goals

- Preserve normal shell behavior and latency. A valid shell command must never
  require OpenCode, start the backend, contact a provider, or change output.
- Use one OpenCode session engine for questions, follow-ups, suggestions, and
  agent tasks so users do not experience two conflicting memories.
- Render a clean, native coding-agent experience without leaving the terminal
  or entering an alternate-screen application.
- Preserve Aishe's setup, credentials, pricing, statusline, routing, audit,
  undo, history, project trust, and shell identity.
- Use OpenCode for provider adaptation, reasoning/tool loops, child agents,
  session state, compaction, todos, and normalized progress events.
- Keep model-generated effects behind an Aishe-owned execution boundary.
- Make installation, upgrade, rollback, diagnosis, and offline deployment
  predictable and supportable.
- Make every important state transition observable in a structured test,
  diagnostic, log, or user-visible status.
- Support concurrent Aishe shells without mixing sessions, approvals, scopes,
  working directories, costs, or tool results.
- Preserve interrupted work without blindly re-running a tool whose previous
  outcome is unknown.

## Non-Goals

- Do not expose the OpenCode TUI, web interface, desktop application, account
  store, updater, or sharing feature.
- Do not treat OpenCode permissions as a security sandbox. OpenCode explicitly
  documents that they are a UX mechanism, not isolation.
- Do not allow OpenCode to auto-update independently of Aishe.
- Do not inherit user or project `opencode.json`, `.opencode`, OpenCode plugins,
  OpenCode skills, OpenCode credentials, or an existing OpenCode daemon.
- Do not rewrite zsh, replace zsh's line editor, or route ordinary commands
  through an AI process.
- Do not translate legacy provider-native interrupted tasks into OpenCode
  sessions. Preserve and resume them through the compatibility engine.
- Do not promise that arbitrary shell commands are reversible. File-tool edits
  remain journaled; shell commands remain auditable but may have irreversible
  effects.
- Do not silently fall back to another engine after OpenCode has admitted a
  prompt or begun a tool loop. That could duplicate cost and side effects.
- Do not target Windows in this milestone.
- Do not make OpenCode v2 beta a production dependency.

## User Stories

### Shell user

As a shell user, I can run `git`, `ssh`, `vim`, pipelines, redirections,
functions, aliases, jobs, `cd`, `export`, and every other normal zsh feature
without the backend affecting their behavior.

### Conversational user

As a user, I can type a natural-language question directly at the shell prompt,
receive a streamed answer, ask a follow-up, and see model/cost/session status
without opening a separate application.

### Coding-agent user

As a developer, I can ask Aishe to inspect, edit, test, and explain a project.
I see concise reasoning, tool activity, command output, diffs, subagent work,
usage, and a final result inline, followed by my normal zsh prompt.

### Operations user

As an operator, I can deliberately enter `yolo · host`, accept the risk once
for that shell session, and let the agent perform real host administration
without Aishe repeatedly asking for approval.

### Safety-conscious user

As a user, I can remain in `auto` and receive clear approval prompts for risky
actions while safe reads and commands proceed.

### Enterprise administrator

As an administrator, I can mirror and pin the managed runtime, deploy an
organization policy, disable host yolo, require audit logging, restrict provider
hosts, preinstall dependencies, and validate the result non-interactively.

### Offline or restricted-network user

As a user behind a proxy or in an offline environment, I can install the exact
runtime from a mirror or local file and receive actionable validation rather
than an opaque download failure.

### Existing Aishe user

As an upgrading user, my config, credentials, history, pricing, project trust,
tasks, logs, and statusline choices remain intact. Existing interrupted tasks
remain visible and resumable.

## User Experience Contract

### One continuous command line

Expected interaction:

```text
~/project ❯ git status
On branch main
nothing to commit

~/project ❯ fix the failing authentication tests

  ● Inspecting authentication and its tests
  ├ read  src/auth.rs
  ├ read  tests/auth_test.rs
  └ ran   cargo test auth
          2 failed, 18 passed

  ● Fixing the refresh-token race
  ├ edit  src/auth.rs                              +12 -4
  ├ edit  tests/auth_test.rs                       +18 -0
  └ ran   cargo test auth
          20 passed

  Fixed the refresh-token race and added regression coverage.
  3,418 in · 621 out · 4 requests · $0.014

~/project ❯
```

OpenCode server logs, startup messages, ports, JSON, SSE frames, provider wire
formats, and plugin names must not appear in normal output.

### Routing

The existing dispatcher remains authoritative:

- Valid shell input executes in the real zsh.
- Natural-language input goes to OpenCode.
- `?<text>` forces AI routing.
- `!<command>` forces shell routing.
- Full-buffer question grammar must continue to disambiguate command-name
  collisions such as `what`, `where`, and `who`.
- Highlighting must represent the final full-buffer route, not merely the first
  token.
- The submitted prompt must remain visible after Enter.

The current route should remain visible in the prompt/status treatment when it
is useful, but route prediction must not add layout noise to narrow terminals.

### Mode behavior

| Mode | Model behavior | Tool behavior | Approval behavior |
| --- | --- | --- | --- |
| `suggest` | Answer or propose a command using structured output. | No host-mutating tools. Optional bounded read-only context tools may be enabled later. | A proposed command is placed into the shell buffer for review. |
| `auto` | Full agent loop is available. | Aishe tool bridge is enabled. | Safe actions run; risky or unknown actions ask once per action or approved pattern. |
| `yolo · workspace` | Full agent loop and subagents are available. | Actions are confined to the declared workspace policy; Linux commands run through bubblewrap. | One scope acceptance per shell session, then no per-action prompts. Disallowed scope escapes return tool errors, not prompts. |
| `yolo · host` | Full agent loop and subagents are available. | Real host actions are permitted. | One explicit host-risk acceptance per shell session, then no per-action prompts. |

Shift-Tab continues to cycle modes. Entering yolo invokes the scope acceptance
only if the current shell session has not accepted that scope. Switching out of
yolo does not erase acceptance within the same shell session. Starting a new
Aishe shell always resets yolo acceptance.

The statusline must show mode and scope distinctly:

```text
gpt-5.6-luna · auto · workspace
gpt-5.6-luna · yolo · workspace
gpt-5.6-luna · yolo · host
```

### Scope semantics

`workspace` means:

- Relative paths resolve against the registered canonical workspace.
- File writes must remain beneath allowed workspace roots after symlink
  resolution.
- Linux shell commands run in a bubblewrap profile with only declared writable
  roots.
- Host-sensitive locations are read-only or absent.
- Network is denied by default for the conservative/balanced profiles and can
  be enabled as a declared session capability. A denied network action returns
  an explanatory tool error and suggests the explicit scope/network command.
- No individual action can silently widen the scope.

`host` means:

- Commands and file tools operate with the permissions of the current user.
- `sudo` and root behavior remain OS behavior, not an Aishe privilege bypass.
- Network is available.
- Aishe still redacts secrets, audits actions, uses idempotency records, and
  journals supported file edits.
- Aishe does not display per-action approvals after session acceptance.

On macOS:

- `workspace` is enforced by Aishe path/policy checks but is not a kernel
  sandbox in this milestone.
- Both yolo scopes show a one-time per-session warning that the agent executes
  without supported OS isolation.
- After acceptance, yolo remains unprompted.

### Interrupt and resume

- First Ctrl-C during a model turn or tool action requests cancellation,
  terminates the active tool process group, calls OpenCode session abort, and
  preserves the Aishe shell.
- A second Ctrl-C forces local detachment if graceful abort does not complete
  promptly.
- A disconnected terminal must not cause a mutating tool call to be repeated.
- `aishe resume` reconnects to the durable OpenCode session or presents the
  legacy task through the compatibility engine.
- If a shell command began but its completion was not durably recorded, resume
  sends an "outcome unknown; inspect state before retrying" tool result rather
  than executing it again.

## Architecture

### High-level topology

```text
┌──────────────────────────────────────────────────────────────────────┐
│ User terminal                                                        │
│                                                                      │
│  real zsh + ZLE + user's .zshrc/plugins/history                      │
│                     │                                                │
│                     ▼                                                │
│  Aishe foreground client                                             │
│  ├─ dispatcher and route highlighting                                │
│  ├─ inline renderer and statusline handoff                            │
│  ├─ mode/scope/session lease                                          │
│  ├─ approval UI                                                       │
│  └─ Aishe tool executor                                               │
│      ├─ safety/policy                                                 │
│      ├─ bubblewrap                                                    │
│      ├─ file tools / MCP / skills                                     │
│      ├─ audit / undo / idempotency                                    │
│      └─ command PTY                                                   │
└──────────────────────┬───────────────────────────────────────────────┘
                       │ private loopback control/tool protocol
                       ▼
┌──────────────────────────────────────────────────────────────────────┐
│ Aishe backend supervisor (lazy per-user daemon)                       │
│ ├─ process lock, leases, protocol/version checks                      │
│ ├─ OpenCode lifecycle and health                                     │
│ ├─ session-to-foreground-client routing                              │
│ ├─ tool request broker                                               │
│ ├─ idempotency journal                                                │
│ └─ private runtime metadata/logs                                      │
│                       │                                               │
│                       ▼                                               │
│ Managed OpenCode v1 server                                            │
│ ├─ provider/model adapter                                             │
│ ├─ sessions/messages/compaction                                       │
│ ├─ agent loop/subagents/todos                                         │
│ ├─ structured output                                                  │
│ ├─ SSE events                                                         │
│ └─ trusted Aishe plugin ── tool request ──► supervisor ──► client     │
└──────────────────────────────────────────────────────────────────────┘
```

### Ownership matrix

| Concern | Aishe owns | OpenCode owns |
| --- | --- | --- |
| Interactive shell | Real zsh PTY, hooks, history, routing, highlighting | Nothing |
| User interface | All prompts, progress, approvals, diffs, status, errors | Emits structured state only |
| Credentials | Storage, precedence, redaction, rotation | Receives process-scoped credential material only |
| Provider protocol | Configuration and validation policy | Request adaptation and model execution |
| Sessions | User-facing mapping, leases, migration, cost aggregation | AI transcript, message parts, child sessions, compaction |
| Agent loop | Mode/scope policy and cancellation | Reasoning, tool selection, iteration, subagents |
| Tools | Authorization, execution, sandbox, audit, undo, MCP, skills | Requests tools through trusted Aishe plugin |
| Permissions | Final policy decision and UX | Built-in permission engine configured to allow only Aishe bridge tools and safe control-plane tools |
| Runtime | Download, checksum, version, process, rollback | Executes pinned server |
| Updates | Aishe release compatibility manifest | OpenCode auto-update disabled |

### Why OpenCode must not execute host tools directly

OpenCode documents that it does not provide a security sandbox. Running the
entire server inside bubblewrap is insufficient because the provider credential
and model-controlled shell/file tools would still inhabit the same process
trust domain.

The required boundary is:

1. OpenCode holds provider credentials only in its private child-process
   environment.
2. All built-in OpenCode tools capable of reading files, executing processes,
   editing files, fetching arbitrary URLs, loading external skills, or loading
   external plugins are hidden/denied.
3. A trusted Aishe plugin exposes proxy tools that carry OpenCode session,
   message, and call identity to the Aishe supervisor.
4. The supervisor routes each request to the foreground Aishe client that owns
   the registered session lease.
5. The foreground client decides and executes using Aishe policy. Tool
   subprocesses do not inherit the OpenCode child environment or provider key.

This preserves OpenCode's agent loop while keeping Aishe's security and shell
identity.

### Backend abstraction

Introduce a backend-neutral Rust interface. OpenCode v1 must not leak through
the renderer, dispatcher, modes, usage, or CLI:

```rust
pub trait AgentBackend: Send + Sync {
    fn health(&self) -> Result<BackendHealth>;
    fn ensure_session(&self, request: SessionRequest) -> Result<BackendSession>;
    fn submit(&self, request: PromptRequest) -> Result<PromptHandle>;
    fn events(&self, handle: &PromptHandle) -> Result<Box<dyn Iterator<Item = Result<AgentEvent>>>>;
    fn snapshot(&self, session: &BackendSession) -> Result<SessionSnapshot>;
    fn abort(&self, session: &BackendSession) -> Result<()>;
    fn list_sessions(&self, filter: SessionFilter) -> Result<Vec<SessionSummary>>;
    fn resume(&self, session: &BackendSession) -> Result<PromptHandle>;
}
```

The exact Rust API may use channels instead of iterators, but it must preserve
these responsibilities. `AgentEvent` is Aishe-owned and versioned independently
of OpenCode.

Implementations:

- `OpenCodeBackend`: default.
- `NativeBackend`: compatibility wrapper around the existing provider/mode
  engine.
- `FakeBackend`: deterministic tests.

### Normalized Aishe events

At minimum:

```rust
pub enum AgentEvent {
    Connected,
    SessionCreated { session_id: String },
    UserPromptAccepted { text: String },
    ReasoningStarted,
    ReasoningDelta { text: String },
    ReasoningCompleted,
    TextDelta { text: String },
    TextCompleted { text: String },
    ToolQueued { call: ToolCallView },
    ToolStarted { call: ToolCallView },
    ToolOutput { call_id: String, stream: OutputStream, chunk: String },
    ToolCompleted { call_id: String, result: ToolResultView },
    ToolFailed { call_id: String, error: UserFacingError },
    Diff { diff: DiffView },
    TodoUpdated { items: Vec<TodoItem> },
    SubagentStarted { parent: String, child: String, agent: String },
    SubagentCompleted { child: String, result: String },
    Usage { usage: UsageDelta },
    Compacted,
    WaitingForApproval { request: ApprovalRequest },
    WaitingForUser { request: UserQuestion },
    Reconnecting { attempt: u32 },
    Reconciled,
    Aborted,
    Completed { summary: String },
    Failed { error: UserFacingError },
}
```

Every OpenCode event parser must tolerate unknown fields and unknown event
types. Unknown events are debug-logged, never fatal, and never printed raw.

### OpenCode v1 API contract

Store a reviewed OpenAPI fixture for the pinned version and generate or hand
maintain only the narrow Rust types Aishe uses. Do not import the JavaScript SDK.

Required operations:

| Operation | Purpose |
| --- | --- |
| `GET /global/health` | Verify server identity and exact compatible version. |
| `GET /event` | Subscribe before prompt admission to live SSE events. |
| `POST /session` | Create a durable AI session with Aishe title and permissions. |
| `GET /session` | List sessions for migration, resume, and diagnostics. |
| `GET /session/status` | Reconcile busy/idle state. |
| `GET /session/:id` | Read session metadata and parent relationship. |
| `GET /session/:id/children` | Map subagent sessions to the owning foreground lease. |
| `GET /session/:id/message` | Rebuild state after reconnect or event loss. |
| `POST /session/:id/prompt_async` | Admit a prompt and return immediately. |
| `POST /session/:id/abort` | Cancel a running turn. |
| `POST /session/:id/summarize` | Explicit compaction command where needed. |
| `POST /session/:id/revert` and `/unrevert` | Revert conversation state only; never claim host side effects were reverted. |
| `GET /session/:id/todo` | Reconcile todo state. |
| `GET /session/:id/diff` | Optional informational diff; Aishe tool-journal diffs remain authoritative. |

The stable v1 SSE server publishes live events but does not provide durable
event replay. The client may send `Last-Event-ID`, but Aishe must not assume the
server replays missed events. On disconnect:

1. Stop applying new local UI state.
2. Reconnect with bounded exponential backoff.
3. Fetch session status, messages, children, todos, and pending local tool
   journal records.
4. Rebuild the normalized snapshot idempotently.
5. Resume rendering only after reconciliation.

### Session identity and working directory

An Aishe shell session has a random, private `aishe_shell_id`. It maintains one
active OpenCode session per workspace identity:

- Nearest VCS/worktree root when present.
- Otherwise the canonical current directory.
- Moving within one workspace keeps the same OpenCode session.
- Moving to a different workspace switches or creates a session.
- Returning to a prior workspace in the same Aishe shell resumes its session.
- Separate Aishe shells get separate primary sessions unless the user
  explicitly runs `aishe resume <id>`.

The mapping record contains no credentials:

```json
{
  "schema_version": 1,
  "aishe_shell_id": "...",
  "workspace": "/canonical/project",
  "backend": "opencode",
  "backend_session_id": "...",
  "mode": "auto",
  "scope": "workspace",
  "created_at": "...",
  "updated_at": "..."
}
```

OpenCode child/subagent sessions inherit routing through their parent chain.
The supervisor must resolve an unknown child session to the registered ancestor
before accepting a tool request.

`cd` in the user's real zsh updates the next prompt's workspace context. A `cd`
inside an agent tool command affects only that command process and must never
move the user's interactive zsh.

## Managed OpenCode Runtime

### Initial compatibility pin

The first implementation targets OpenCode `v1.18.9`, released 2026-07-28.
Before merging, rerun the compatibility suite against that exact tag. If the
implementation intentionally moves to another stable tag, update the manifest,
OpenAPI fixture, recorded source audit, test fixtures, third-party notices, and
this document in one reviewed change.

Initial upstream asset manifest:

| Aishe platform | Upstream asset | Compressed size | SHA-256 |
| --- | --- | ---: | --- |
| Linux x86_64 glibc | `opencode-linux-x64-baseline.tar.gz` | 59,311,798 | `3eddbc5423264055f2527a0abd2d3a6fc6bbca3dced6bbd85d5d4cc27beacad2` |
| Linux x86_64 musl | `opencode-linux-x64-baseline-musl.tar.gz` | 61,680,081 | `2420d3369aee94d2317ba07b6643786a0666ac9b3e8d4cd069913397f4d41697` |
| Linux aarch64 glibc | `opencode-linux-arm64.tar.gz` | 59,122,119 | `b16bd7593ea960a25d9c6849b3023bcd9b9244a6f51675341fd2052043b0670f` |
| Linux aarch64 musl | `opencode-linux-arm64-musl.tar.gz` | 61,250,947 | `8fe9da991068f9e1524c6cb34dad52806bf5927baaa4d583b6fd4ea7987210f4` |
| macOS arm64 | `opencode-darwin-arm64.zip` | 44,954,303 | `6f998b7dabb9425bb348fd0d88afeb92a14422771231cec9b0f4374b947397e6` |
| macOS x86_64 | `opencode-darwin-x64-baseline.zip` | 47,190,199 | `ee8ffb2971db99cc2d4638b9b26218e1e33484c616cc4ca9a41016f4c9424417` |

Do not fetch `latest`. Do not trust a checksum downloaded from the same
untrusted URL at runtime. Embed the reviewed asset name, exact version, size
range, and SHA-256 in an Aishe compatibility manifest committed to the repo.

### Runtime paths

Linux:

```text
~/.local/share/aishe/runtime/
├── manifest.json
├── current                 # atomic text pointer, not an unsafe symlink
└── opencode/
    ├── 1.18.9/
    │   ├── opencode
    │   ├── LICENSE
    │   ├── THIRD_PARTY_NOTICES.md
    │   └── install.json
    └── <previous-compatible-version>/
```

macOS uses Aishe's existing Application Support data root.

Permissions:

- Runtime directories: `0700`.
- Runtime binary: `0755`.
- Control state, tokens, install metadata, logs: `0600`.
- Licenses/notices: `0644`.

Keep the active and immediately previous verified runtime. Remove older
versions only through an explicit garbage-collection step after verifying they
are not active. Runtime cleanup must never traverse outside the canonical Aishe
runtime root.

### Download and installation

1. Resolve the platform only from OS/architecture, the host Linux libc loader,
   and the embedded manifest. Prefer glibc on glibc hosts and musl on native
   musl/Alpine hosts; never assume a musl-named archive is statically linked.
2. Resolve source precedence:
   - Explicit `--from <local-file>`.
   - Organization policy/local mirror.
   - `AISHE_RUNTIME_BASE_URL`.
   - Aishe release mirror.
   - Approved upstream URL as final fallback.
3. Honor standard proxy variables and native certificate roots.
4. Download to a private temporary file under the runtime root.
5. Enforce a maximum size before and during download.
6. Verify exact SHA-256 before extraction.
7. Extract into a new private staging directory with path traversal,
   absolute-path, symlink, hardlink, device, and archive-bomb protections.
8. Verify file format, executable bit, and `opencode --version`.
9. Run an isolated `serve` health smoke test with no provider credential.
10. Atomically rename the staging directory into the version directory.
11. Atomically update the `current` text pointer.
12. Retain the prior runtime for rollback.

Any failure before step 10 leaves the active runtime untouched. Any failure
after Aishe binary upgrade must produce a clear degraded state and preserve the
native compatibility backend.

### Runtime commands

Add:

```text
aishe backend status [--json]
aishe backend install [--from PATH] [--force]
aishe backend verify [--live]
aishe backend repair
aishe backend rollback
aishe backend stop
aishe backend logs [--tail N]
aishe backend gc [--dry-run]
```

`aishe backend install` installs only the compile-time compatible runtime. A
user-supplied arbitrary version is rejected unless a developer-only
compatibility override is enabled. That override must never be presented as
supported.

### Runtime update policy

- OpenCode auto-update is always disabled.
- Aishe releases change the compatible runtime manifest deliberately.
- Runtime reconciliation occurs during the Aishe installer, setup, explicit
  backend update, or first AI call after an Aishe upgrade.
- Never update a runtime silently in the middle of an active shell or agent
  task.
- New Aishe detects an older supervisor protocol, requests graceful shutdown,
  preserves interrupted sessions, and starts the new supervisor/runtime.
- If the new runtime fails verification, roll back the runtime pointer and
  retain the installed Aishe binary with a visible compatibility-backend
  warning.

## Backend Supervisor

### Lifecycle

The supervisor is an internal Aishe subcommand, not a system service:

```text
aishe __backend-supervisor
```

It is lazily started only when an AI request, backend command, or live backend
probe needs it. Direct shell startup and valid shell commands must not start it.

Requirements:

- One supervisor per Aishe data root and OS user.
- Private advisory lock prevents duplicate startup.
- Detached process with stdin closed and private rotating logs.
- It spawns the exact managed OpenCode path, never a name resolved from `PATH`.
- It records a private state file containing schema/protocol version, PIDs,
  start time, endpoints, runtime version, and random tokens.
- Every connection verifies process identity, protocol version, runtime
  version, authentication, and a startup nonce.
- Stale PID/state files are repaired without killing unrelated processes.
- Idle timeout defaults to 30 minutes after the last lease/tool/model activity.
- Active prompts or tool requests prevent idle shutdown.
- `aishe backend stop` performs graceful abort, then bounded process-group
  termination.
- Parent death, terminal death, and system shutdown cannot leave a foreground
  terminal in raw mode.

### Local authentication

- Bind OpenCode and supervisor endpoints only to `127.0.0.1`.
- Disable mDNS and do not bind IPv6 wildcard or `0.0.0.0`.
- Generate independent 256-bit random tokens for:
  - OpenCode HTTP Basic Auth.
  - Supervisor control requests.
  - Trusted plugin tool requests.
- Never print tokens or include them in support bundles.
- State/token files must be `0600` under a `0700` directory.
- Reject redirects, non-loopback endpoints, Host-header confusion, oversized
  requests, invalid content types, invalid JSON, stale leases, and protocol
  mismatches.
- Use constant-time token comparison.
- Rate-limit failed authentication and log only redacted metadata.

OpenCode `--port=0` currently prefers 4096 before an arbitrary free port.
Aishe must choose a random available high port, pass it explicitly, verify the
child's reported URL, and retry boundedly on an address race. It must never
attach to an unrelated process already listening on 4096.

### OpenCode launch environment

Launch with:

```text
HOME=<aishe-data>/backend/opencode/home
XDG_CONFIG_HOME=<aishe-data>/backend/opencode/xdg/config
XDG_DATA_HOME=<aishe-data>/backend/opencode/xdg/data
XDG_CACHE_HOME=<aishe-data>/backend/opencode/xdg/cache
XDG_STATE_HOME=<aishe-data>/backend/opencode/xdg/state
OPENCODE_CONFIG_DIR=<aishe-data>/backend/opencode/config
OPENCODE_CONFIG_CONTENT=<generated JSON>
OPENCODE_DISABLE_PROJECT_CONFIG=1
OPENCODE_DISABLE_DEFAULT_PLUGINS=1
OPENCODE_DISABLE_EXTERNAL_SKILLS=1
OPENCODE_DISABLE_AUTOUPDATE=1
OPENCODE_DISABLE_LSP_DOWNLOAD=1
OPENCODE_SERVER_USERNAME=aishe
OPENCODE_SERVER_PASSWORD=<random>
OPENCODE_CLIENT=aishe
```

Do not set `OPENCODE_PURE=1`, because the trusted Aishe plugin must load.
Isolation instead comes from the managed HOME/XDG/config roots, disabled
project config, disabled default plugins, and a generated config whose only
plugin is the checksum-verified Aishe bridge plugin.

Additional requirements:

- `share` is disabled.
- External web, shell, file, search, apply-patch, skill, and LSP tools are
  denied/hidden.
- OpenCode telemetry/export environment variables are cleared unless an
  organization policy explicitly configures an approved local exporter.
- Provider keys are added only to the OpenCode child environment, not the
  supervisor environment and not the foreground tool environment.
- Backend stdout/stderr are captured to private rotating logs.
- Startup parsing accepts only the expected
  `opencode server listening on http://127.0.0.1:<port>` form.
- Health must report exactly the compatible version before any credential is
  provided.

## Trusted Aishe OpenCode Plugin

### Packaging

Ship a reviewed, deterministic JavaScript module as an embedded Aishe asset.
At backend startup:

1. Write it atomically into the managed backend config directory.
2. Verify its compile-time SHA-256.
3. Refuse to start the OpenCode backend if the on-disk plugin differs and
   cannot be safely repaired.
4. Reference only that absolute plugin path in generated OpenCode config.

The plugin must have no third-party runtime imports and no install-time network
dependency.

### Responsibilities

- Define Aishe proxy tools.
- Receive OpenCode `ToolContext`, including session and message identity.
- Use `tool.execute.before` to capture the stable OpenCode tool call ID and
  overwrite any model-supplied internal identity field.
- POST the request to the authenticated supervisor bridge.
- Block until the owning foreground client returns a result or cancellation.
- Propagate abort signals.
- Return normalized text/metadata to OpenCode.
- Notify Aishe before each provider turn so the session budget and step policy
  can authorize or clamp the request.
- Never execute a command, read a host file, open a URL, or access Aishe's
  credential store itself.
- Never expose endpoint tokens, provider keys, config paths, or internal IDs in
  tool descriptions/results.

### Proxy tools

Initial stable tool surface:

| Tool | Purpose | Aishe implementation |
| --- | --- | --- |
| `aishe_run_command` | Run one shell command with cwd, timeout, and output policy. | Existing executor plus new PTY/stream/idempotency support. |
| `aishe_read_file` | Read a bounded text/binary-safe file representation. | Existing file tools with canonical path checks. |
| `aishe_write_file` | Create/replace a file atomically. | Existing write tool plus undo journal. |
| `aishe_edit_file` | Perform exact replacement edits. | Existing edit tool plus diff/undo. |
| `aishe_apply_patch` | Apply a validated patch transactionally. | New or refactored Aishe patch tool. |
| `aishe_list_dir` | List bounded directory metadata. | Existing list tool. |
| `aishe_search_files` | Glob/regex search with limits. | New shared search implementation using `rg` where available. |
| `aishe_fetch_url` | Fetch approved HTTP(S) content with limits/redaction. | Existing web tool through Aishe network policy. |
| `aishe_use_skill` | Load an Aishe-approved skill. | Existing progressive-disclosure skill registry. |
| `aishe_mcp_call` or namespaced tools | Call user-configured Aishe MCP servers. | Existing Aishe MCP client; OpenCode never loads user MCP config directly. |
| `aishe_ask_user` | Ask a non-approval question needed to continue. | Foreground inline question renderer. |

Tool schemas must set `additionalProperties: false`, bound string/array sizes,
and reject control characters where inappropriate.

OpenCode safe control-plane tools may remain:

- Todo state.
- Child-agent/task orchestration.
- Compaction/session metadata.

Every primary and subagent permission set must inherit a default deny and
explicitly allow only the Aishe proxy tools and approved control-plane tools.
Tests must prove that built-in shell/read/write/edit/patch/web/search/skill/LSP
tools are absent for primary and child agents.

### Tool routing and leases

The foreground client registers:

```rust
struct ToolLease {
    lease_id: SecretId,
    aishe_shell_id: String,
    backend_session_id: String,
    workspace: CanonicalPath,
    mode: Mode,
    scope: ExecutionScope,
    network: NetworkPolicy,
    interactive: bool,
    expires_at: Instant,
}
```

The model cannot choose or override mode, scope, workspace, network policy, or
lease. The supervisor looks these up by authenticated OpenCode session/ancestor
identity.

The foreground client keeps a long-lived authenticated tool-event channel to
the supervisor. When the plugin calls a proxy tool:

1. Plugin sends session ID, message ID, stable call ID, tool name, and args.
2. Supervisor validates plugin token, schema, session ancestry, and active
   lease.
3. Supervisor durably records `admitted`.
4. Supervisor sends `ToolRequested` to the correct foreground client.
5. Client independently validates tool schema, canonical cwd, mode, scope, and
   policy.
6. Client records `started`, then executes.
7. Client streams display output locally and returns bounded model output.
8. Supervisor records `completed` before acknowledging the plugin.
9. Duplicate call IDs return the durable prior result.

If no foreground lease exists, mutating tools do not execute. The plugin
receives a typed unavailable/interrupted result that OpenCode can preserve for
resume.

### Tool idempotency

Key: backend session ID + message ID + OpenCode call ID.

States:

```text
admitted -> dispatched -> started -> completed
                               \-> outcome_unknown
              \-> cancelled
```

- File writes/edits use atomic transactions and record before/after hashes.
- A duplicate completed call returns the stored redacted result.
- A duplicate admitted/dispatched call may be safely re-routed.
- A duplicate `started` mutating shell call is never executed again
  automatically.
- An unknown outcome is reported to the model as unknown; it must inspect state
  before proposing another action.
- Journal records contain redacted args/results and no provider credentials.
- Retention follows Aishe task/audit policy and is upgrade-safe.

## Provider, Model, Credential, and Cost Integration

### Aishe remains the source of truth

Keep:

- `config.toml` provider endpoint, model, transport, and credential-profile
  references.
- Private `credentials.toml`.
- Environment-over-saved credential precedence.
- Existing setup model catalog and exact validation behavior.
- User pricing overrides and unknown-price prompt.
- Fallback provider configuration.

Do not call OpenCode `auth.set` for normal API-key providers and do not adopt
`~/.local/share/opencode/auth.json`.

### Generated provider config

Map Aishe provider definitions into generated OpenCode config with stable,
Aishe-namespaced provider IDs:

- Official OpenAI/Responses: `aishe-openai`.
- Anthropic Messages: `aishe-anthropic`.
- OpenAI-compatible custom endpoint: `aishe-compatible`.
- Local Ollama-compatible endpoint: `aishe-local`.

The exact mapping must be covered by fixtures for OpenAI, Anthropic, Groq,
OpenRouter, Together, Ollama, and a generic compatible server.

For custom OpenAI-compatible services, generated config must explicitly select
the OpenAI-compatible adapter and base URL. For official OpenAI reasoning/tool
models, use OpenCode's supported Responses path. Do not infer transport from
model-name prefixes when endpoint/capability evidence exists.

The resolved API key is set only in the OpenCode child environment under the
provider variable referenced by generated config. It must be removed from
backend logs and never inherited by foreground tool processes.

Credential changes trigger a graceful backend restart after active calls
finish. A new key is validated before replacing the active working backend.

### Setup validation sequence

1. Direct catalog request validates endpoint and key without token spend where
   supported.
2. Exact selected/typed model is checked against the full catalog.
3. If unlisted, one explicitly approved minimal generation validates it.
4. Generated OpenCode config starts against an isolated local test session.
5. Live text streaming is verified.
6. Structured command/answer output is verified.
7. One safe proxy tool round trip runs in a temporary workspace.
8. Usage fields are verified and mapped.
9. Optional subagent/tool-loop validation is offered with token-cost warning.

Errors must name the failing layer: runtime, server authentication, provider
credential, provider reachability, model availability, structured output,
tools, streaming, or sandbox.

### Usage and budgets

OpenCode usage events are input to Aishe's meter; Aishe remains the displayed
and enforced cost authority.

Pricing precedence:

1. Exact user price in Aishe config.
2. Exact reviewed built-in Aishe price.
3. Compatible OpenCode/Models.dev price only when provider and model identity
   match exactly and the value is labeled as catalog-derived.
4. Unknown.

Never invent or fuzzy-match a price for budget enforcement.

Aggregate:

- Main session and child-agent token usage.
- Per-call input/output/reasoning/cache fields where available.
- Last call, current task, current shell session, and persisted AI-session
  totals.
- Costs without double-counting replayed/reconciled events.

The trusted plugin performs a budget authorization callback before every
provider turn. Aishe may clamp maximum output tokens based on remaining budget.
When the hard budget is exhausted, the plugin rejects the next provider turn
before it starts and OpenCode receives a typed budget error. Post-event abort is
only a secondary guard because it can race the next turn.

## Inline Renderer

### Rendering principles

- Do not use an alternate screen.
- Do not clear prior shell output.
- Never erase the user's submitted prompt.
- Preserve partially typed ZLE input if asynchronous status arrives.
- Use adaptive width and Unicode only when the terminal supports it.
- Respect `NO_COLOR`, dumb terminals, redirected stdout, and JSON output modes.
- Send machine-readable output only to requested stdout contracts; diagnostics
  and progress belong on stderr.
- Avoid raw ANSI from model/provider/tool output.
- Bound command output in the transcript while preserving complete output in a
  private spill file when configured.

### Visual hierarchy

- Cyan/brand accent: headings/current state.
- Green: completed/safe.
- Yellow: attention/auto approval/degraded capability.
- Red: denied/failed/destructive.
- Dim gray: metadata, model, cost, elapsed time.
- Inverse or clearly marked focus row in interactive menus.

Tool row states:

```text
  ○ queued   cargo test
  ● running  cargo test                         4.2s
  ✓ ran      cargo test                         20 passed
  ✗ failed   cargo test                         exit 101
```

Reasoning is summarized by default. Raw reasoning content is shown only when
the provider supplies displayable summaries and the user enables expanded
detail. Never claim access to hidden chain-of-thought.

### Approval UI in auto

Approval panel must show:

- What action will run.
- Canonical cwd/target.
- Why it requires approval.
- Scope and sandbox.
- Whether network/sudo/external writes are involved.
- Choices with clear keys: allow once, allow matching action for session, edit
  where supported, deny.

OpenCode's own approval UI is never shown. Aishe's decision is final.

### Yolo entry UI

Workspace on Linux:

```text
Enter yolo · workspace?

The agent may run commands and change files without asking again in this
shell session. Linux actions are confined to:
  /home/mj/project
Network: off
Sandbox: bubblewrap verified

Type yolo to continue:
```

Host:

```text
Enter yolo · host?

The agent may execute any command available to your user, use sudo, modify
system files, access the network, and make irreversible changes without asking
again in this shell session.

Type yolo-host to continue:
```

macOS additionally states that no supported OS sandbox is active. Acceptance is
never persisted across Aishe shell sessions.

### Statusline

Add optional fields:

- `backend`
- `scope`
- `task`
- `elapsed`
- `context`
- `last_tokens`
- `last_cost`
- `session_tokens`
- `session_cost`
- `requests`

Existing right/below/off placement remains. Setup offers compact, detailed, and
custom field sets with a live width-aware preview.

## Setup Experience

### Quality bar

Setup must feel like a polished product, not a sequence of `println!` calls:

- Clear title, one-sentence purpose, and progress (`Step 2 of 9`).
- One primary decision per screen.
- A visibly focused selection.
- Width-aware wrapping with consistent indentation.
- Back, help, cancel, and resume on every safe step.
- No active config change until final Apply.
- Secrets remain process-local until Apply and are never written to drafts.
- System package installation and runtime installation are explicitly labeled
  as immediate side effects because they cannot be rolled back with config.
- Every verification result explains its evidence.
- Every warning includes the next action.
- Narrow terminals down to 40 columns remain usable without horizontal
  clipping.
- `NO_COLOR` remains legible through symbols/text, not color alone.

### Setup flow

#### Step 1: Welcome and existing-state discovery

Show:

- Fresh install versus upgrade.
- Existing config path/schema.
- Existing credential profiles without values.
- Existing history/tasks/session counts.
- Current Aishe and managed runtime versions.
- Organization policy status.

Offer Resume/Restart when a draft exists. Never delete the draft, config, or
credentials without an explicit targeted choice.

#### Step 2: Shell and platform

Validate:

- OS and architecture.
- Real zsh path/version.
- PTY capability.
- terminal color/width.
- config/data directory writability and privacy.
- proxy/custom CA visibility.

If zsh is missing, offer the same transparent package-manager flow as
bubblewrap. The user may continue in non-interactive/bash-hook-only mode, but
the limitation is explicit.

#### Step 3: Agent runtime

Show:

- `OpenCode Agent Runtime 1.18.9`.
- Download size for the detected platform.
- Source/mirror hostname.
- Install path.
- Exact checksum verification statement.
- License link/notice.

Default action: Install and verify.

Alternatives:

- Use approved local file.
- Configure mirror.
- Continue shell-only and resume setup later.

Do not offer arbitrary system OpenCode selection in the normal setup.

#### Step 4: Linux sandbox

If Linux:

1. Detect `bwrap`.
2. Run a real functional self-test, not `command -v` alone.
3. If absent or unusable, explain what it protects and what remains available.
4. Detect package manager.
5. Show the exact command.
6. Ask `Install bubblewrap now?`.
7. Invoke sudo only after yes.
8. Stream package-manager output.
9. Rerun functional self-test.
10. On failure, offer retry, manual instructions, or save/resume.

Supported commands:

- Debian/Ubuntu: `sudo apt-get install -y bubblewrap`
- Fedora/RHEL: `sudo dnf install -y bubblewrap`
- Older RHEL: `sudo yum install -y bubblewrap`
- openSUSE: `sudo zypper --non-interactive install bubblewrap`
- Arch: `sudo pacman -S --noconfirm bubblewrap`
- Alpine: `sudo apk add bubblewrap`

When already root, omit sudo. Never build package-manager commands from
untrusted strings and never use `sh -c`.

Functional self-test must distinguish:

- Installed and usable.
- Installed but user namespaces/policy block it.
- Unsupported kernel/container environment.
- Missing.

If macOS, show `OS sandbox: unavailable in this release` and explain the
per-session yolo warning.

#### Step 5: Provider and credential

Preserve the current service picker, endpoint normalization, private saved-key
option, environment-only option, loopback no-auth support, and hidden secret
entry.

Explain that Aishe owns the key and provides it privately to the managed engine;
the user does not configure OpenCode.

#### Step 6: Model and pricing

- Poll the endpoint's model catalog where available.
- Visibly validate the credential through the catalog request.
- Allow filtering/selection and exact manual entry.
- Validate an unlisted model with one minimal request only after consent.
- Ask exact input/output prices when unknown.
- Allow unknown price while clearly disabling cost budgets.

#### Step 7: Behavior and scope

Choose:

- Conservative: suggest default, auto asks for effects, network off.
- Balanced: auto default, safe reads/commands automatic, workspace writes ask,
  network asks.
- Autonomous: yolo workspace available, bubblewrap required on Linux, network
  selected explicitly.
- Custom.

Do not collect a persistent yolo risk acceptance during setup. Acceptance is
per live shell session.

#### Step 8: Interface

Configure:

- Statusline placement and fields.
- Compact versus detailed agent output.
- Tool output verbosity.
- Reasoning summary visibility.
- Color/Unicode accessibility.
- Optional audit logging, with privacy explanation.

#### Step 9: End-to-end validation

Run:

- Runtime hash/version.
- Server start and authenticated health.
- Config isolation check.
- Built-in tool-denial check.
- Provider credential and model.
- Text streaming.
- Structured command/answer.
- Proxy tool call in a temporary workspace.
- Tool environment credential-leak check.
- Bubblewrap escape self-test on Linux.
- Usage/cost mapping.
- Optional paid subagent/tool-loop test.

Validation uses the draft credential in memory. It does not persist it before
Apply.

#### Step 10: Review and Apply

Review groups:

- Files that will change.
- Runtime/system dependencies already installed.
- Provider/model/key source.
- Mode/scope/network defaults.
- Statusline/output.
- Pricing/budget.
- Validation evidence.
- Warnings/degraded capabilities.

Apply transaction:

1. Validate draft schema.
2. Write credential store atomically/private.
3. Write config migration backup.
4. Write config atomically.
5. Write backend mapping/state only after config succeeds.
6. Start a clean backend with persisted credentials.
7. Re-run health.
8. If final health fails, restore prior config/credential files and keep the
   setup draft.
9. Mark setup complete and discard only the draft.

Finish with:

```text
Setup complete

✓ zsh             /usr/bin/zsh
✓ agent engine    OpenCode 1.18.9
✓ provider        OpenAI · gpt-5.6-luna
✓ sandbox         bubblewrap · workspace
✓ history         preserved

Run:
  aishe

Inside Aishe:
  git status                 runs in zsh
  explain this repository    asks the agent
```

Offer the guided tour.

### Non-interactive setup

Extend `aishe setup --non-interactive` with explicit flags:

```text
--backend opencode
--install-backend
--runtime-file PATH
--runtime-base-url URL
--sandbox bwrap|policy
--install-system-deps
--default-scope workspace|host
--network allow|deny
--output focus|compact|detailed
--json
```

Rules:

- Never invoke sudo in non-interactive mode unless
  `--install-system-deps` is explicitly supplied.
- Never accept yolo risk non-interactively as a substitute for per-session
  acceptance.
- Return structured checks and stable exit codes.
- A missing required value is an error, not an interactive fallback.
- Environment or stdin secrets must never be echoed.

### Stable setup exit codes

| Code | Meaning |
| ---: | --- |
| 0 | Applied or verification succeeded. |
| 2 | User cancelled/paused without applying. |
| 3 | Input/config validation failed. |
| 4 | Runtime installation or compatibility failed. |
| 5 | Provider credential/model validation failed. |
| 6 | Sandbox requirement failed. |
| 7 | Organization policy denied the requested configuration. |

## Installer and Packaging

### Curl installer

The curl installer should stage and verify the Aishe binary and managed
OpenCode runtime before replacing an existing installation.

Default:

- Fresh TTY install: install Aishe and the pinned runtime; print `aishe setup`.
- Upgrade: install the new binary/runtime transactionally; never run setup or
  rewrite config automatically.
- `--setup`: run setup after both are verified.

Overrides:

```text
AISHE_SKIP_ZSH=1
AISHE_SKIP_BACKEND=1
AISHE_RUNTIME_BASE_URL=...
AISHE_RUNTIME_FILE=...
AISHE_BIN_DIR=...
AISHE_CONFIG_DIR=...
AISHE_DATA_DIR=...
```

If backend download fails during an upgrade, do not replace the working binary
unless the staged Aishe version explicitly supports the currently installed
runtime or the user opted into compatibility fallback.

State inventory must continue to prove that config/data files are untouched.
Runtime version metadata may change; history, credentials, config, tasks,
sessions, audit logs, and undo data may not.

### `.deb` and `.rpm`

Package installation must not download user-specific network content in package
scripts.

- Package Aishe normally.
- Change bubblewrap from `recommends` to a clear recommended/weak dependency,
  not a hard dependency, because restricted containers may not support it.
- First `aishe setup` installs the managed runtime into the invoking user's data
  directory.
- Optionally publish a separate architecture-specific
  `aishe-opencode-runtime` package for offline enterprise repositories. If
  installed, Aishe verifies and uses it as a system-managed approved runtime
  without copying it into user data.
- Never make the package depend on an unpinned generic `opencode` package.

### Homebrew

- Do not reuse an arbitrary Homebrew OpenCode version.
- Setup installs the managed runtime in Aishe data.
- A future formula may declare the pinned OpenCode archive as an explicit
  versioned resource, but only when the formula can preserve exact compatibility
  and checksums on all supported architectures.
- Explain that macOS yolo is not OS-sandboxed in this milestone.

### Cargo/source install

`cargo install` installs the Aishe binary only. First setup installs the managed
runtime. Provide `aishe backend install --from` for offline development.

### Release workflow

Add jobs that:

1. Validate the pinned upstream tag and MIT license.
2. Download each approved upstream runtime asset.
3. Verify the committed SHA-256 and size.
4. Run `opencode --version` on native-compatible assets.
5. Mirror unchanged archives into the Aishe GitHub release or publish a signed
   Aishe runtime manifest pointing to them.
6. Publish `THIRD_PARTY_NOTICES.md`, OpenCode license, checksums, SBOM, and
   provenance.
7. Run adapter contract tests against the exact runtime before publishing.
8. Keep the Aishe release draft until every binary, package, runtime asset,
   checksum, notice, and validation result is uploaded.

The installer verifies the embedded compatibility digest even when downloading
from the Aishe mirror.

### Uninstall

`aishe uninstall` must present separate choices:

- Binary/completions/man page.
- Managed backend runtimes/cache.
- AI sessions/tool journals.
- Config and credentials.
- Shell history.
- Audit/undo data.

Default uninstall removes binaries/runtime caches but preserves config,
credentials, history, sessions, audit, and undo data. Destructive state removal
requires explicit targeted confirmation and prints whether recovery is
possible.

## Configuration and Migration

### Current schema v5

Add:

```toml
version = 5

[aishe]
mode = "suggest"

[backend]
engine = "opencode"
fallback = "native"
managed = true
idle_timeout_secs = 1800
default_scope = "workspace"
workspace_network = "deny"
output = "focus"

[sandbox]
linux_backend = "bwrap"
require_functional = false
workspace_roots = []
allow_host_yolo = true
```

Names may be refined during implementation, but the concepts and migration
behavior are required.

Runtime version and hash do not belong in editable user config. They are
defined by the Aishe compatibility manifest. An organization policy or
developer override may select an approved mirror, not an arbitrary unsupported
version.

### Migration rules

- Schema 3 loads with identical existing behavior before Apply.
- On migration, back up config atomically and add backend/sandbox defaults.
- Preserve provider, endpoint, credential profile, model, price, statusline,
  memory, context, logging, MCP, skill, history, and safety settings.
- Map current `yolo_sandbox`, `sandbox_backend`, and confirmation settings into
  the closest v4 scope/profile without enabling more authority.
- Existing `yolo` default becomes `workspace`, not `host`.
- Do not write yolo acceptance to config.
- Keep deprecated fields readable through at least two minor releases and
  report provenance/deprecation in `aishe settings`.
- Never migrate or delete private credentials merely because the backend
  changes.

### Backend fallback

During migration:

- Default backend is OpenCode.
- Native fallback is allowed only if OpenCode cannot be started before a prompt
  is admitted.
- Display one concise `agent engine unavailable; using native fallback` status.
- Do not fallback after prompt admission, during reconnect, after a partial
  response, or after any tool call.
- By default, native fallback supports suggest/chat/auto compatibility.
- Legacy yolo tasks continue through native resume.
- New yolo tasks require OpenCode unless the user explicitly selects the
  developer compatibility override.
- Record backend name in usage, sessions, logs, and support diagnostics.

### Organization policy

Add an optional root/admin-managed policy file:

- Linux: `/etc/aishe/policy.toml`
- macOS: `/Library/Application Support/Aishe/policy.toml`
- Override for tests/managed deployment: `AISHE_POLICY_FILE`

Policy may:

- Require/disable OpenCode.
- Set runtime mirror and approved hashes.
- Require bubblewrap for workspace agent actions.
- Disable host yolo.
- Restrict provider hosts/models.
- Require audit logging and redaction.
- Restrict network access.
- Disable user MCP/skills.
- Set maximum budgets and output tokens.
- Require support-bundle exclusions.

Policy constrains rather than supplies ordinary user preferences. Precedence:

```text
organization constraints
  > CLI request
  > trusted project overlay
  > user config
  > compiled defaults
```

Setup/settings must label constrained values as `Managed by organization`.

## Existing State and Feature Integration

### Shell history

- Keep `history.ext` and native zsh `HISTFILE` behavior unchanged.
- Backend runtime install/update/uninstall must never remove history.
- AI session history is separate from shell command history.
- Agent-executed commands may be recorded in audit/tool journals but must not
  pollute the user's interactive zsh history unless the user explicitly chooses
  that behavior.

### Conversation memory

- OpenCode sessions become canonical for new AI conversations.
- Existing lightweight `session.rs` JSONL memory may be imported once as
  bounded context into a new session, then retained or archived; never delete it
  automatically.
- `/reset` creates a new OpenCode session for the current workspace rather than
  deleting old session data.

### Durable tasks

- New tasks use OpenCode session state plus Aishe tool journals.
- Existing `tasks/*.json` stay valid and appear as `engine=native`.
- `aishe sessions` returns a unified list with engine, workspace, mode, scope,
  status, cost, and last activity.
- `aishe resume` selects the correct backend from the record.
- Never replay an existing native provider continuation through OpenCode.

### Audit and undo

- Aishe remains audit authority for prompts, normalized responses, approvals,
  tool actions, scope changes, backend lifecycle, and errors.
- OpenCode private logs are diagnostic and do not replace Aishe audit.
- Aishe file edits remain undoable as one agent task transaction where
  possible.
- OpenCode conversation revert must be labeled `conversation revert`; it must
  not claim to undo host actions.
- Secret redaction applies before persistent logs/tool results/support bundles.

### MCP and skills

- Existing Aishe MCP configuration remains the user-facing source.
- Aishe calls configured MCP servers through its current client and exposes
  approved tools through the trusted bridge.
- OpenCode does not load user MCP servers or a user's global OpenCode token
  store. Supported OAuth tokens are created explicitly by `aishe auth login`
  inside Aishe's isolated managed-runtime data directory.
- Existing Aishe/Claude-compatible skills remain discoverable through Aishe
  trust and progressive disclosure.
- OpenCode external skills remain disabled.
- Tool name collisions are resolved deterministically and displayed by
  `aishe mcp`/`aishe tools`.

### Project context and trust

- Aishe's `.aishe/config.toml` trust system remains authoritative.
- OpenCode project config/instructions are disabled.
- Aishe builds and redacts context, then sends it as explicit session/system
  context.
- Untrusted project files cannot install an OpenCode plugin, change endpoints,
  widen scope, enable host yolo, or alter provider credentials.

## Security Requirements

### Required invariants

- A model cannot select its execution mode or scope.
- A model cannot invoke an OpenCode built-in host tool.
- A model cannot read the OpenCode process environment through an allowed tool.
- Provider credentials are absent from foreground tool subprocess environments.
- Project files cannot configure the managed OpenCode instance.
- Tool call identity is supplied by the trusted plugin and validated by Aishe.
- Duplicate tool requests do not duplicate completed effects.
- Unknown-effect outcomes are not retried automatically.
- Workspace paths are canonicalized after symlink resolution.
- Archive extraction cannot escape runtime staging.
- Server endpoints are loopback-authenticated and version-verified.
- No runtime auto-update occurs.
- Support bundles never contain credentials, bridge tokens, raw task content,
  shell history, or unrestricted OpenCode logs by default.

### Bubblewrap execution profile

Build bubblewrap arguments as an argv vector, never a shell string.

Workspace command profile:

- `--die-with-parent`
- new mount, IPC, PID, UTS, cgroup namespaces where supported
- optional network namespace according to declared policy
- read-only system runtime paths needed for executables/libraries
- minimal `/dev`
- new `/proc`
- tmpfs `/tmp`
- declared workspace roots read-write
- other allowed context roots read-only
- private synthetic HOME unless a specific home subpath is approved
- no Aishe config/data/credential/backend directories mounted
- no SSH/GPG/cloud credentials unless explicitly granted as a session
  capability
- sanitized environment and bounded PATH
- resource/time/output limits

The bubblewrap self-test and command profile must be tested as an unprivileged
user and root. If bubblewrap exists but cannot create required namespaces,
workspace yolo is unavailable until the user chooses policy-only degradation
or host scope according to policy.

### Host tool environment

Even in host scope:

- Remove provider API keys, bridge tokens, backend passwords, and internal
  runtime variables.
- Preserve normal user environment needed by commands.
- Do not log secret prompt input from sudo/ssh/GPG/password tools.
- Interactive commands use a PTY when necessary; secret input passes directly
  to the child.
- Aishe cannot bypass OS authentication prompts. Those are not Aishe approval
  prompts and may still appear in yolo.

### Network

- OpenCode provider traffic uses the configured provider endpoint.
- Proxy tool network access is separately policy-controlled.
- Workspace network denial must not prevent OpenCode provider traffic because
  provider execution is outside the tool sandbox.
- `fetch_url` validates schemes, redirects, loopback/private-network policy,
  size, timeout, and content type.
- Enterprise provider allowlists and proxy roots are enforceable.

## Diagnostics and Observability

### Doctor checks

Add structured checks:

- `backend.engine`
- `backend.runtime.present`
- `backend.runtime.version`
- `backend.runtime.hash`
- `backend.runtime.license`
- `backend.supervisor`
- `backend.server.loopback`
- `backend.server.auth`
- `backend.server.health`
- `backend.config.isolated`
- `backend.plugin.hash`
- `backend.tools.restricted`
- `backend.events`
- `backend.provider`
- `backend.model`
- `backend.tool_bridge`
- `backend.credential_isolation`
- `sandbox.bubblewrap.present`
- `sandbox.bubblewrap.functional`
- `sandbox.workspace.escape`
- `sessions.storage`
- `sessions.migration`
- `usage.mapping`

Each check has stable ID, status, detail, remediation, fixability, and JSON
representation.

`doctor --fix` may:

- Repair private directories/files.
- Reinstall a corrupt runtime from the approved source.
- Rewrite the embedded trusted plugin.
- Remove stale state files after process verification.
- Restart a mismatched supervisor.
- Create missing journals/history files.

It may not:

- Install a system package with sudo without interactive consent.
- Replace credentials.
- Delete sessions/history/tasks.
- Widen scope or accept yolo risk.

### Logs

- Aishe supervisor log: lifecycle, versions, health, redacted errors.
- OpenCode log: private, size-limited, rotated, debug disabled by default.
- Tool journal: durable idempotency/effect state.
- Aishe audit: optional user-facing accountability.

`aishe backend logs` redacts again at read time and clearly labels source.

### Support bundle

Include:

- Version/build/runtime manifest.
- Doctor JSON.
- OS/architecture and dependency capability.
- Redacted config provenance.
- Backend process state without tokens.
- Last bounded redacted backend errors.
- Tool schema/permission summary.

Exclude:

- Credentials and credential file content.
- Bridge/server tokens.
- Prompt/session/message content.
- Shell history.
- Tool arguments/results unless user explicitly selects them after preview.
- Raw environment.

## Failure Handling

| Failure | Required behavior |
| --- | --- |
| Runtime missing | Offer `aishe backend install`; use pre-admission native fallback where allowed. |
| Runtime hash mismatch | Quarantine staged file, keep active runtime, fail closed, show source/hash remediation. |
| Runtime version mismatch | Refuse connection, repair/rollback, never "try anyway" silently. |
| Supervisor stale state | Verify PID/executable/start nonce before cleanup; start fresh. |
| Port collision | Choose another random port; never attach to unknown listener. |
| OpenCode crash before prompt | Restart once; fallback only before admission. |
| OpenCode crash during prompt | Mark interrupted, restart, reconcile durable messages/tool journal, offer resume. |
| SSE disconnect | Reconnect and rebuild snapshot; do not assume event replay. |
| Foreground client disconnects | Cancel/expire its tool lease; do not execute queued effects in daemon. |
| Tool call duplicated | Return prior completed result or prior state; never rerun completed effect. |
| Tool outcome unknown | Return unknown result; require state inspection rather than automatic repeat. |
| Bubblewrap missing | Setup/doctor offer install; workspace yolo unavailable or policy-degraded explicitly. |
| Bubblewrap unusable | Explain kernel/container restriction; never claim sandbox active. |
| Credential missing | Say which Aishe credential profile/env is missing; do not say only "LLM not configured." |
| Credential invalid | Name rejected provider/profile and repair command without printing secret. |
| Model invalid | Return to model selection and show catalog/manual validation options. |
| Budget exhausted | Deny next provider turn before request; preserve session and show usage. |
| Setup interrupted | Save non-secret draft, preserve active config, resume at correct step. |
| Upgrade interrupted | Keep prior binary/runtime pointer or recover from staged transaction. |
| OpenCode data migration fails | Back up managed backend data, roll back runtime, preserve sessions for support. |
| Provider outage | Use configured provider fallback inside the OpenCode-generated provider policy only before side effects; never duplicate an admitted turn. |

## Performance and Reliability Targets

- Valid shell commands: zero backend startup/network work.
- Added direct-shell startup overhead from this milestone: p95 under 20 ms.
- Backend startup is lazy.
- Warm backend health/lease acquisition: p95 under 100 ms locally.
- Cold backend ready time: target p95 under 2.5 seconds on supported hardware,
  measured separately from runtime download and provider latency.
- First visible streamed model output: provider-dependent, but Aishe event
  processing adds target p95 under 50 ms.
- Tool request dispatch from plugin to foreground client: p95 under 50 ms.
- Ctrl-C local acknowledgement: under 100 ms; process termination bounded.
- No unbounded in-memory output, SSE buffer, message list, or log growth.
- Backend idle memory and cold-start size are measured and published before
  release; regressions over an approved baseline require review.
- Setup remains usable at 40, 58, 80, 120, and 200 columns.
- Concurrent shell sessions must not cross-contaminate prompts, scopes, costs,
  tool calls, or status files.

## Files and Interfaces

### New Rust modules

Suggested layout:

```text
src/
  agent/
    mod.rs
    backend.rs          # AgentBackend trait and shared request/result types
    events.rs           # normalized Aishe event schema
    controller.rs       # prompt lifecycle, reconnect, cancellation
    renderer.rs         # inline native renderer
    policy.rs           # mode/scope/network decision
  backend/
    mod.rs
    native.rs           # compatibility adapter
    supervisor.rs       # process/lock/lease lifecycle
    control.rs          # authenticated local control protocol
    bridge.rs           # tool routing/idempotency
    runtime.rs          # download/hash/extract/install/rollback
    manifest.rs         # embedded compatibility manifest
    opencode/
      mod.rs
      client.rs         # narrow REST client
      sse.rs            # SSE parser/reconnect
      types.rs          # reviewed v1 wire types
      mapper.rs         # OpenCode -> AgentEvent
      config.rs         # generated isolated config
      session.rs        # session/workspace mapping
  dependencies.rs       # zsh/bubblewrap detection, install commands, self-test
```

Embedded assets:

```text
assets/backend/opencode/
  runtime-manifest.json
  aishe-plugin.mjs
  THIRD_PARTY_NOTICES.md
tests/fixtures/opencode/
  v1.18.9-openapi.json
  events/*.json
  messages/*.json
```

Exact names may change, but responsibilities must not be collapsed into
`main.rs`, `integration.rs`, or one oversized backend file.

### Existing files expected to change

- `Cargo.toml` / `Cargo.lock`: HTTP server/control, cryptographic hash/random,
  safe archive extraction, process locking, and test dependencies.
- `build.rs`: embed manifest/plugin hashes/build metadata.
- `src/main.rs`: backend/scope CLI and early setup/doctor handling.
- `src/lib.rs`: new module exports.
- `src/config.rs`: schema v4 and migration.
- `src/setup.rs`: new dependency/runtime/backend steps.
- `src/promptui.rs`: progress, panels, responsive layout, command confirmation.
- `src/diagnostics.rs`: backend/sandbox checks and repair.
- `src/dispatcher.rs`: preserve routing; add regression cases only.
- `src/pty.rs`: lazy backend/session environment and signal integration.
- `src/integration.rs`: mode/scope/session handoff and status fields.
- `src/modes/*`: migrate to backend-neutral controller; retain native adapter.
- `src/providers/*`: compatibility backend and setup probes.
- `src/tools.rs`: shared foreground proxy execution.
- `src/sandbox.rs`: scope-aware bwrap profiles and functional probe.
- `src/tasks.rs`: unified engine labels/migration.
- `src/session.rs`: legacy import/compatibility.
- `src/usage.rs` / `src/usagelog.rs`: backend/child-session usage.
- `src/audit.rs` / `src/undo.rs`: normalized backend/tool events.
- `src/mcp.rs` / `src/skills.rs`: proxy exposure.
- `install.sh`, `nfpm.yaml`, `packaging/aishe.rb`.
- `.github/workflows/ci.yml`, `.github/workflows/release.yml`.
- README, architecture, setup, install, safety, provider, modes, cost,
  troubleshooting, and runbook docs.

### Public CLI compatibility

Preserve:

- `aishe`
- `aishe -c`
- `aishe suggest`
- `aishe mode`
- `aishe model`
- `aishe models`
- `aishe setup`
- `aishe settings`
- `aishe doctor`
- `aishe sessions`
- `aishe resume`
- `aishe auth`
- `aishe dry-run`
- `aishe init zsh|bash`

Existing JSON contracts remain stable or receive a versioned additive schema.

## Implementation Workstreams

### Phase 0: Freeze contracts and fixtures

- [x] Add this document and link it from `docs/design/PLAN.md`.
- [x] Commit the reviewed OpenCode v1.18.9 runtime manifest and license.
- [x] Store the v1.18.9 OpenAPI fixture and representative event/message
      fixtures.
- [x] Define `AgentBackend`, `AgentEvent`, mode/scope, error, session, and usage
      types.
- [ ] Record baseline shell, startup, memory, setup, and live-model results.
- [x] Add a feature flag/config override so the incomplete backend cannot become
      default accidentally.

Exit: types/fixtures compile; no user behavior changes.

### Phase 1: Runtime manager and supervisor

- [x] Implement platform manifest selection.
- [x] Implement bounded download, embedded SHA-256 verification, safe
      tar.gz/zip extraction, version check, staging, atomic activation, prior
      retention, rollback, and garbage collection.
- [x] Add third-party notices.
- [x] Implement private supervisor lock/state/token generation.
- [x] Implement exact managed-process spawn, random loopback ports, Basic Auth,
      health, logs, idle lifecycle, stop/restart, and stale recovery.
- [x] Add backend CLI commands and JSON.
- [x] Add installer/runtime mirror/offline flows.
- [x] Add doctor checks.

Exit: `aishe backend install/status/verify/rollback/stop` passes without a
provider key; an arbitrary system OpenCode is ignored.

### Phase 2: OpenCode client and event normalization

- [x] Implement narrow authenticated REST client.
- [x] Implement strict bounded SSE parser.
- [x] Subscribe before prompt admission.
- [x] Map message/reasoning/tool/todo/child/usage/error/idle events.
- [x] Implement snapshot reconciliation after disconnect.
- [x] Implement cancellation and abort.
- [x] Implement workspace/session mapping and persistence.
- [x] Add fake server and fixture-driven contract tests.

Exit: local fake provider supports streamed question/answer, reconnect, abort,
resume, compaction, and child-session mapping.

### Phase 3: Trusted plugin and foreground tool bridge

- [x] Implement and embed dependency-free trusted plugin.
- [x] Implement plugin hash/config isolation.
- [x] Deny/hide OpenCode host-effecting built-ins for every agent/subagent.
- [x] Implement authenticated supervisor tool protocol.
- [x] Implement foreground leases and child-session ancestry.
- [x] Implement stable call-ID injection.
- [x] Implement durable idempotency states.
- [x] Adapt Aishe file/command/web/MCP/skill tools.
- [x] Sanitize tool environment and outputs.
- [x] Implement PTY execution for commands requiring interactive OS prompts.
- [x] Stream command output to foreground renderer while returning bounded
      output to OpenCode.
- [x] Implement Ctrl-C across plugin/supervisor/client/process group.

Exit: a real OpenCode loop can inspect/edit/test a disposable repo while a
security test proves provider keys and OpenCode built-in tools are unavailable
to the model.

### Phase 4: Modes, scopes, sandbox, and budgets

- [x] Map suggest/auto/yolo to OpenCode agents/tool policies.
- [x] Implement per-shell acceptance state.
- [x] Implement `workspace` and `host` scope commands/status.
- [x] Implement Linux bwrap command/file profiles.
- [x] Implement network capability.
- [x] Implement macOS policy-only warning.
- [x] Ensure yolo never emits per-action approvals after acceptance.
- [x] Ensure auto remains action-gated.
- [x] Implement provider-turn budget authorization in trusted plugin.
- [x] Aggregate child-session usage without duplicates.

Exit: complete mode/scope matrix passes deterministic PTY and Linux isolation
tests.

### Phase 5: Inline renderer and shell integration

- [x] Implement normalized renderer.
- [x] Preserve submitted prompts and ZLE buffers.
- [x] Render tools/diffs/todos/subagents/costs/errors.
- [x] Implement compact/detailed/no-color/JSON behavior.
- [x] Extend statusline fields.
- [x] Ensure background events do not corrupt a live prompt.
- [x] Preserve direct zsh behavior and plugin precedence.
- [x] Update guided tour.

Exit: PTY screenshots/transcripts at supported widths meet the visual contract,
and the existing shell feature matrix remains unchanged.

### Phase 6: Enterprise setup and migration

- [x] Extend draft schema and step state machine.
- [x] Add runtime install screen.
- [x] Add bubblewrap consent/install/self-test.
- [x] Add generated-backend E2E validation.
- [x] Add behavior/scope/interface screens.
- [x] Add transactional review/apply/rollback.
- [x] Add non-interactive flags and exit codes.
- [x] Add schema-v4 migration and compatibility deprecations.
- [x] Add organization policy.
- [x] Unify OpenCode and legacy session listings.
- [x] Preserve/import legacy memory and resume legacy tasks.

Exit: clean setup, interrupted setup, upgrade, rollback, offline, and managed
policy journeys pass.

### Phase 7: Packaging, release, and hardening

- [x] Extend curl installer with transactional runtime staging.
- [x] Update packages/Homebrew/source instructions.
- [x] Add mirrored runtime assets, checksums, notices, SBOM, provenance.
- [x] Add dependency and license audit.
- [x] Run fault injection, fuzz, concurrency, soak, and performance suites.
- [x] Run disposable Linux SSH validation.
- [x] Run macOS validation.
- [ ] Publish release candidate only after evidence review.
- [ ] Retain feature/config rollback to native backend for at least two minor
      releases.

Exit: release definition of done is satisfied.

## Verification Plan

### Unit tests

Add focused Rust tests for:

- Manifest platform mapping and digest parsing.
- Download bounds and content length mismatch.
- Archive traversal, absolute paths, symlinks, hardlinks, devices, duplicates,
  and decompression limits.
- Atomic runtime activation/rollback.
- Process state and stale PID identity.
- Constant-time auth/token checks.
- OpenCode wire decoding with unknown fields/events.
- SSE chunk boundaries, multiline data, UTF-8 splits, reconnect, and malformed
  frames.
- Event normalization and deduplication.
- Session/workspace identity and child ancestry.
- Tool lease authorization.
- Mode/scope policy.
- Idempotency state transitions and crash recovery.
- Path/symlink escape prevention.
- Environment secret filtering.
- Usage/cost deduplication and budget authorization.
- Config v3-to-v4 migration and provenance.
- Organization policy constraints.
- No-color/width-aware renderer layout.

### Fake OpenCode contract server

Create a deterministic Rust/Python fixture server that:

- Requires Basic Auth.
- Reports a configurable version.
- Implements the narrow session API.
- Emits realistic SSE fragmentation.
- Simulates text, reasoning summaries, tool lifecycle, todos, subagents,
  usage, compaction, errors, disconnects, and event loss.
- Simulates prompt accepted but response interrupted.
- Simulates duplicate tool calls and late events.

Use it in every CI platform. No provider key.

### Real pinned OpenCode, fake provider

Run the actual managed OpenCode v1.18.9 binary against a local fake
OpenAI/Anthropic-compatible provider:

- Text stream.
- Structured suggest answer/command.
- Tool loop through trusted plugin.
- Multiple tool iterations.
- Subagent child session.
- Compaction.
- Abort.
- Reconnect/reconciliation.
- Exact usage mapping.
- Invalid key/model/capability errors.
- Built-in tool absence.
- Project/global OpenCode config isolation.

This is the primary adapter contract gate and must not spend money.

### Tool bridge security suite

Prove:

- Wrong/missing bridge token rejected.
- Forged mode/scope/workspace ignored.
- Unregistered/stale session rejected.
- Child session routes only to registered ancestor.
- Built-in OpenCode shell/read/write/web tools unavailable.
- User/project OpenCode plugins/config cannot load.
- Provider key absent from `env`, command output, allowed file roots, support
  bundle, and foreground child process environment.
- Workspace file/symlink escapes rejected.
- Network denied when policy says denied.
- Duplicate completed effect not executed twice.
- Crash after `started` produces unknown outcome and no replay.
- Ctrl-C kills process group and aborts OpenCode.
- Output limits and binary output do not crash or corrupt terminal.

### Bubblewrap tests

On Linux CI with bubblewrap:

- Functional self-test.
- Workspace read/write.
- Host root read-only.
- Aishe config/data/credential/backend paths unavailable.
- `/tmp` private.
- Network allowed/denied profiles.
- Symlink escape.
- Nested shell/subshell/process substitution.
- Tool command timeout and process cleanup.
- Root and non-root behavior on disposable environments where feasible.

When CI kernel blocks unprivileged namespaces, assert the diagnostic reports
`installed but unusable`, not `available`.

### PTY tests

Extend existing suites:

- Direct shell never starts backend.
- Prompt text remains visible.
- Full-buffer route/highlight remains correct.
- Rich streamed answer.
- Tool rows and diff.
- Auto approval once/always/deny.
- Yolo workspace one-time acceptance and no action prompts.
- Yolo host one-time acceptance and no action prompts.
- New shell asks again.
- macOS warning path fixture.
- Ctrl-C during model/tool.
- Ctrl-Z/job control for direct shell remains native.
- Background/reconnect output preserves typed ZLE buffer.
- Concurrent Aishe shells do not mix output/state.
- 40/58/80/120/200-column setup and runtime UI.
- `NO_COLOR`, ASCII fallback, redirected output, JSON.
- Statusline right/below/off and new fields.

### Setup/install/upgrade tests

Hermetic fixtures:

- Fresh curl install with runtime mirror.
- Corrupt Aishe checksum.
- Corrupt runtime checksum.
- Wrong runtime version.
- Interrupted runtime extraction.
- Runtime rollback.
- Setup cancel/resume before/after runtime and bubblewrap steps.
- Fake package managers prove exact argv and sudo consent.
- No sudo in non-interactive setup without explicit flag.
- Existing config/credentials/history/tasks/sessions/other state hashes unchanged
  by install and upgrade.
- v0.4.1 schema/config fixture upgrades to v0.5.0.
- Missing-key setup returns credential step.
- `/models` and exact typed model behavior.
- Unknown price prompt.
- Offline/local-file/mirror/proxy flows.
- Organization policy display/enforcement.
- Uninstall default preserves user state.

### Existing regression gate

All existing gates remain:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --verbose --locked
cargo build --release --locked
sh tests/installer_upgrade.sh target/release/aishe
python3 tests/provider_unauthenticated.py target/release/aishe
python3 tests/credentials_linux.py target/release/aishe
python3 tests/live_contract_test.py
python3 tests/pty_smoke.py target/release/aishe
python3 tests/pty_scenarios.py target/release/aishe
python3 tests/statusline_pty.py target/release/aishe
python3 tests/setup_pty.py target/release/aishe
python3 tests/durable_task_resume.py target/release/aishe
python3 tests/pty_fuzz.py target/release/aishe
python3 tests/zsh_features.py target/release/aishe
python3 tests/pty_signals.py target/release/aishe
python3 tests/admin_validation.py target/release/aishe
```

No existing deterministic regression may be waived merely because the backend
changed.

### Live-model validation

Run only on a disposable test node with a new, non-published test credential,
explicit cost cap, rate limiting, and log redaction.

Matrix:

- Official OpenAI Responses reasoning/tool model.
- Anthropic tool model.
- One OpenAI-compatible provider.
- Optional local Ollama model.
- Valid and invalid credentials.
- Listed and manually typed models.
- Suggest answers versus commands.
- Auto safe/risky decisions.
- Workspace coding tasks.
- Multi-step tool loops.
- Subagents.
- Context compaction.
- Cost/budget cutoff.
- Provider rate-limit/retry.
- 100–500 varied prompts, capped by dollars and requests.

Assertions are response-independent where possible: valid protocol, no crash,
no credential leak, bounded cost, correct tool authorization, correct mode/scope
behavior, valid files/tests, and no unsafe escape.

Never reuse a credential pasted into a conversation, committed file, issue,
test result, or shell history. Rotate exposed test credentials before use.

### Disposable SSH Linux release-candidate gate

On a clean supported Ubuntu/Debian and, where possible, Fedora/RHEL-family node:

1. Snapshot or ensure the node is disposable.
2. Install the previous public Aishe release and create config, credential,
   history, task, audit, and undo fixtures.
3. Upgrade through the release-candidate installer.
4. Hash-compare preserved state.
5. Complete setup, including consented bubblewrap installation.
6. Verify runtime version/hash and backend isolation.
7. Run direct shell/zsh matrix.
8. Run fake-provider OpenCode contract suite.
9. Run budgeted live-model matrix.
10. Run workspace yolo escape tests.
11. Run concurrent sessions and resume.
12. Enter host yolo only on the disposable snapshot and perform a reversible
    administration scenario.
13. Reboot/reconnect and verify sessions/history/config remain.
14. Roll back runtime and verify compatibility path.
15. Generate a redacted support bundle and inspect it manually.

Store a timestamped Markdown/JSON report with versions, commands, pass/fail,
timings, costs, and redacted diagnostics. Do not store keys.

### Performance/soak

- 1,000 direct shell commands with backend stopped.
- 1,000 fake-provider prompts across repeated session reconnects.
- 100 concurrent/overlapping shell sessions in a constrained synthetic test
  where practical.
- 24-hour idle/start/stop supervisor soak.
- Kill supervisor/OpenCode/client at every durable transition.
- Large output, binary output, slow output, hung process, child process tree.
- Runtime disk usage and log rotation.

## Requirement-to-Test Matrix

| Requirement | Required evidence |
| --- | --- |
| Real zsh unchanged | Existing zsh feature, signal, PTY, and shell differential suites |
| No separate UI | PTY transcripts; no alternate-screen sequences/OpenCode TUI process |
| All AI defaults to OpenCode | Backend selection unit/E2E and status metadata |
| Native fallback only pre-admission | Fault-injection tests around prompt admission |
| Managed exact runtime | Manifest/hash/version/install/rollback tests |
| No system OpenCode contamination | PATH trap and existing server tests |
| Config/plugin isolation | Malicious home/project OpenCode fixture |
| Credentials remain Aishe-owned | Credential precedence and no OpenCode auth-file tests |
| Model validation | Setup catalog/manual/live tests |
| Built-in host tools unavailable | Primary/subagent tool inventory contract |
| Tool calls execute in foreground Aishe | Lease/routing/TTY tests |
| Auto prompts risky actions | PTY approval matrix |
| Yolo has no per-action prompts | Workspace/host PTY transcript assertions |
| Yolo acceptance resets per shell | Multi-session PTY test |
| Workspace sandbox | Bubblewrap escape/network/path suite |
| Host administration works | Disposable SSH host-scope scenario |
| macOS warns once per session | macOS PTY/fixture test |
| No duplicate effect on resume | Kill-at-transition/idempotency suite |
| Prompt/ZLE input preserved | Async event redraw PTY tests |
| Cost and budget accurate | Usage fixture, child aggregation, live capped run |
| Setup is polished/resumable | Width/color/cancel/resume/package-manager PTY suite |
| Upgrades preserve state | Installer hash fixture and previous-release SSH upgrade |
| Enterprise policy enforced | policy precedence/setup/doctor tests |
| Support bundles remain private | secret canary scan |

## Acceptance Criteria

### Product

- [x] Users can mix ordinary zsh commands, questions, and coding-agent tasks in
      one continuous terminal.
- [x] Normal commands behave exactly as real zsh and never require backend
      availability.
- [x] OpenCode TUI/server implementation details never appear in normal use.
- [x] Every AI interaction uses OpenCode by default and shares durable context.
- [x] Suggest, auto, yolo workspace, and yolo host match the approved semantics.
- [x] Yolo produces no per-action Aishe/OpenCode approval after session scope
      acceptance.
- [x] A new shell requires new yolo acceptance.
- [x] Submitted prompts and partially typed input are never lost.

### Architecture

- [x] OpenCode v1 is behind `AgentBackend`.
- [x] The runtime is exact-version pinned, checksum-verified, isolated,
      transactionally installed, and rollback-capable.
- [x] OpenCode host-effecting built-ins are unavailable to every primary and
      child agent.
- [x] All host effects pass through authenticated Aishe foreground leases.
- [x] Provider keys are unavailable to model-controlled tool processes.
- [x] Event loss is repaired through state reconciliation.
- [x] Tool effects are idempotent or explicitly marked outcome-unknown.

### Setup and operations

- [x] Setup installs/verifies the managed runtime.
- [x] Linux setup offers consent-gated bubblewrap installation and functional
      verification.
- [x] Setup is transactional/resumable and secrets never enter drafts.
- [x] Offline/mirror/proxy/non-interactive flows work.
- [x] Doctor identifies and repairs runtime/supervisor/plugin/sandbox faults.
- [x] Upgrade preserves all user state.
- [x] Uninstall defaults preserve user state.

### Quality

- [x] All existing deterministic tests pass.
- [x] New fake-server, real-pinned-runtime, bridge-security, sandbox, setup,
      packaging, and migration suites pass.
- [x] Disposable Linux SSH gate passes.
- [x] macOS gate passes within documented policy-only constraints.
- [ ] Live-model budgeted validation passes without credential leakage.
- [x] Performance targets are measured and accepted.
- [x] Docs, man page, completion, release notes, licenses, SBOM, and support
      runbook are complete.

## Release and Rollout

### Feature stages

1. `experimental`: explicit `backend.engine = "opencode"`; native default.
2. `preview`: setup offers OpenCode as recommended; migration prompt; rollback
   command available.
3. `default`: OpenCode is default for all AI; native fallback remains.
4. `stable`: remove preview warning after at least one successful minor release
   and published validation evidence.
5. Future: evaluate OpenCode v2 behind a separate adapter only after upstream
   declares it stable and Aishe contract tests pass.

The user requested OpenCode as the target architecture. Staging is for safe
delivery, not to reopen that product decision.

### Release blockers

- Any direct-shell regression.
- Any config/history/credential/task/session loss.
- Any provider credential accessible to proxy tool execution.
- Any OpenCode built-in host tool visible to an agent.
- Any yolo per-action approval after accepted scope.
- Any workspace bubblewrap escape in supported Linux environments.
- Any duplicate mutating tool execution after fault injection.
- Any unverified runtime download or version drift.
- Any prompt/ZLE corruption.
- Any support-bundle secret canary.
- Missing third-party license/notices.
- Missing disposable-node upgrade evidence.

### Rollback

- `backend.engine = "native"` restores the compatibility engine.
- `aishe backend rollback` selects the prior verified runtime.
- Config schema migration backups remain recoverable.
- OpenCode session data remains untouched by backend rollback.
- Legacy tasks remain native.
- Release notes explain rollback without deleting config/history.

## Documentation Deliverables

Update:

- `README.md`
- `docs/architecture.md`
- `docs/getting-started.md`
- `docs/installation.md`
- `docs/configuration.md`
- `docs/providers.md`
- `docs/modes.md`
- `docs/safety.md`
- `docs/front-ends.md`
- `docs/usage-and-cost.md`
- `docs/mcp.md`
- `docs/troubleshooting.md`
- `docs/development.md`
- `docs/runbooks.md`
- `SECURITY.md`
- `CHANGELOG.md`
- man page/completions
- `THIRD_PARTY_NOTICES.md`

User documentation must explain behavior, not ask users to operate OpenCode.
Contributor documentation must explain the exact OpenCode boundary, pin,
protocol, plugin, and threat model.

## Risks and Mitigations

| Risk | Mitigation |
| --- | --- |
| OpenCode API churn | Exact runtime pin, OpenAPI fixture, narrow adapter, health version gate, compatibility tests |
| Runtime size/download | Show size, cache by version, mirror/offline install, lazy setup, keep only current+previous |
| Backend startup latency | Lazy start, idle reuse, no backend on shell path, measured SLO |
| Config contamination | Managed HOME/XDG/config, project config disabled, only hashed trusted plugin |
| Provider key exposure | Built-in host tools denied; tools execute in foreground Aishe environment without child key |
| Double approvals | OpenCode proxy tools pre-allowed; Aishe is sole approval UI |
| Yolo semantics regress | Session-scoped acceptance tests and no per-action approval invariant |
| Two session stores | Aishe mapping/index with OpenCode canonical new sessions and labeled legacy records |
| SSE event loss | Snapshot reconciliation, event deduplication, durable tool journal |
| Duplicate side effects | Stable call ID, durable state machine, no automatic retry after started |
| Bubblewrap unavailable | Guided install/self-test, clear degradation, host scope, macOS warning |
| Daemon complexity | Narrow supervisor protocol, private lock/state, idle shutdown, fault tests |
| OpenCode subagent bypass | Default-deny permissions applied/tested for every agent and child |
| Provider-turn budget race | Trusted plugin pre-turn authorization and token clamping |
| Existing dirty/user state | Scoped edits, atomic migrations, hash-preservation tests, no cleanup outside runtime root |

## Open Questions

None that block implementation. The product choices that materially affect the
architecture were resolved during planning:

- OpenCode handles all AI interactions with native fallback during rollout.
- Aishe manages a pinned private runtime.
- Linux setup offers a consented verified bubblewrap install.
- Auto remains gated.
- Yolo acceptance is per shell session and has no later action prompts.
- Yolo has explicit workspace and host scopes.
- macOS yolo is allowed after a per-session unsandboxed warning.

If implementation discovers an upstream limitation that prevents the trusted
plugin from providing stable tool call identity, hiding built-in tools for
subagents, or authorizing provider turns before execution, stop and amend this
document with evidence before choosing a weaker boundary.

## Codex Goal Prompt

Implement the complete “Aishe OpenCode Agent Backend and Enterprise Setup”
milestone described in
`docs/design/OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md`.

Treat that document as the source of truth. Preserve Aishe's real zsh PTY,
routing, history, private credentials, setup transactionality, pricing,
statusline, audit, undo, MCP, skills, task records, installation state, and
public CLI/JSON contracts.

OpenCode must become the default backend for every AI request, but it must stay
invisible behind Aishe's native inline CLI. Install and manage the exact
compatibility-pinned OpenCode runtime transactionally. Do not use an arbitrary
system OpenCode or OpenCode auto-update/config/credentials/TUI.

Keep OpenCode behind an `AgentBackend` abstraction. Disable OpenCode's
host-effecting built-in tools for all primary agents and subagents. Ship a
checksum-verified trusted Aishe OpenCode plugin that forwards stable,
authenticated tool requests to the Aishe supervisor. Route each request to the
owning foreground Aishe client, derive mode/scope/cwd/policy from Aishe-owned
session leases, execute with Aishe's safety/bubblewrap/audit/undo/idempotency
layers, and ensure provider credentials never enter model-controlled tool
environments.

Preserve these exact semantics:

- `suggest`: answer or propose for review.
- `auto`: safe actions run and risky/unknown actions ask.
- `yolo · workspace`: one session acceptance, then no per-action prompts;
  Linux tools are workspace-confined with bubblewrap.
- `yolo · host`: one session acceptance, then no per-action prompts; real host
  administration is permitted.
- macOS yolo: one unsandboxed warning/acceptance per shell session, then no
  per-action prompts.

Build the enterprise setup/install/runtime/bubblewrap/provider/model/pricing/
status/validation/review flows, organization policy, diagnostics, rollback,
offline/mirror support, release assets, licensing, and documentation in the
plan.

Implement in the plan's phases. Add all named unit, contract, real-pinned-
runtime, tool-security, bubblewrap, PTY, migration, installer, fault-injection,
performance, live-model, and disposable-SSH validation. Do not mark the goal
complete until every acceptance criterion and release blocker is verified with
recorded evidence. Do not delete or overwrite unrelated working-tree changes.
