# Architecture

A contributor's map of how aishe is put together: how one line of input becomes
either a shell command or an LLM action, how the front-end works, how the
provider / tool / MCP layers stack up, and where the tests live. For user-facing
behavior see [front-ends.md](front-ends.md), [modes.md](modes.md), and
[safety.md](safety.md); for build/test commands see
[development.md](development.md).

## Design principles

1. **Do not reimplement a shell.** Real shell lines are handed to `zsh -c`
   (fallback `bash -c`), or run directly inside the user's zsh. aishe owns the
   *routing* decision and the AI, not the shell grammar.
2. **The model is never trusted to decide what runs.** Every command the model
   proposes passes a separate, deterministic safety gate before it can execute
   (`src/safety.rs`). Model output is untrusted input, not authorization.
3. **Shell path stays direct and service-free.** A valid shell line never starts
   the agent backend or performs network work. AI turns lazy-start a private,
   exact-version-pinned OpenCode process behind an Aishe-owned supervisor and
   authenticated foreground tool bridge.
4. **OpenCode orchestrates; Aishe authorizes.** OpenCode owns conversation and
   provider orchestration. It never receives direct host-effecting tools.
   Aishe owns scope, policy, tools, sandbox, credentials, budget, audit, and UI.
5. **Best-effort, never blocking.** Anything that hits the network or a slow
   subprocess (git status, `--help` parsing, the command cache) runs
   off the hot path or under a timeout. The prompt never hangs.

## Crate layout

The crate is a library (`src/lib.rs`) with a thin binary driver
(`src/main.rs`). Everything testable lives in the library so the integration
tests in `tests/` can exercise internals directly.

| Module | Responsibility |
| --- | --- |
| `dispatcher` | Classify a line: shell / natural language / intercepted builtin. Owns the command cache. |
| `executor` | Run shell lines via `zsh -c`/`bash -c`; intercept state-mutating builtins (`cd`, `export`, ...); capture output; job table. |
| `pty` | The flagship front-end: run the user's real interactive zsh inside a PTY. |
| `integration` | The zsh/bash hook scripts (`aishe init zsh`) injected as `command_not_found_handler` + `precmd` + ZLE widgets. |
| `setup` / `promptui` | Resumable setup state machine and reusable interactive menus/text prompts. |
| `config` / `credentials` / `auth` | Schema/versioned ordinary config, the separate private shared store/resolver, and credential-management CLI. |
| `settings` / `profiles` | Transactional settings with provenance; named safety bundles and readiness checks. |
| `agent` | Backend-neutral session/prompt/event/scope/usage contract plus the foreground turn controller/renderer. |
| `backend::runtime` / `manifest` | Exact-version platform manifest, bounded download/extraction, SHA/version verification, atomic install/repair/rollback/GC. |
| `backend::supervisor` / `control` | Private loopback process lifecycle, isolated HOME/XDG layout, authenticated control protocol, and health/reconciliation. |
| `backend::opencode` | Narrow OpenCode v1 REST/SSE adapter, event normalization, session mapping, prompt abort/snapshot recovery. |
| `backend::bridge` | Foreground leases, child ancestry, provider budget authorization, durable call idempotency, and usage deduplication. |
| `policy` / `dependencies` | Administrator constraints and consent-gated zsh/bubblewrap discovery, installation plans, and functional self-tests. |
| `provider_catalog` / `capabilities` | Service presets, transport/auth policy, model listing, live validation, and capability cache. |
| `diagnostics` | Structured Doctor checks, safe repairs, JSON, and redacted support bundles. |
| `providers` | Native compatibility provider trait/implementations plus setup/model/capability helpers. Managed AI turns route provider work through OpenCode. |
| `modes` | Native compatibility suggest/yolo loops and shared output/safety helpers. |
| `tools` | Built-in agentic tools (`read_file`/`write_file`/`edit_file`/`list_dir`, `fetch_url`). |
| `mcp` | Minimal MCP client (stdio + Streamable HTTP); exposes server tools to yolo as `mcp__<server>__<tool>`. |
| `safety` / `sandbox` | Destructive-command gate; best-effort policy sandbox (network / out-of-tree writes) for yolo. |
| `context` | The environment context block (cwd, dir listing, recent history, project context) prepended to LLM requests. |
| `tasks` | Private durable yolo checkpoints, interrupted-task lifecycle, and safe resume. |
| `tour` | Resumable guided first-session lessons in an isolated workspace. |
| `uninstall` | Exact category/path uninstall planning with state-preserving defaults. |
| `usagelog` | Cross-process PTY usage aggregation and live status rendering data. |
| `session` | In-session conversation memory, persisted to a per-session file for the hook front-ends (whose NL calls are separate processes). |
| `config` | Versioned config schema, atomic migration/save, CLI override and project-overlay precedence. |
| `trust` | Trust store for project `.aishe/config.toml` overlays (`aishe trust`). |
| `cache` | Short-TTL response cache wrapping a `Provider` for identical suggest repeats. |
| `redact` | Best-effort secret scrubbing of the context block. |
| `audit` | Optional JSONL audit log of prompts, responses, and AI-initiated actions. |
| `usage` | Token meter, pricing table, cost/budget math. |
| `skills` | Progressive-disclosure skill registry (Claude-Code-compatible). |
| `commands` | User-defined slash-commands. |
| `histlog` | History storage/format (zsh EXTENDED_HISTORY) for the `history` builtin. |
| `fuzzy` | Fuzzy matching + spelling correction for the command cache. |

## The routing decision (the heart of it)

`dispatcher::dispatch(line, cache) -> Dispatch` decides what a line *is*, and is
the single most important function to understand. It returns
`Shell` / `NaturalLanguage` / `Builtin`. The order is deliberate; earlier rules
win:

1. **Forced LLM:** a leading `?` or `#` always routes to the model, even if the
   rest is a real command (`? who wrote zsh`). The sigil is stripped.
2. **Forced shell:** a leading `!` routes to the shell and is exempt from the
   safety gate. The sigil is stripped.
3. **Slash-commands:** `/<meta> ...` is an alias for `aishe <meta> ...`, but only
   if `<meta>` is a known subcommand, so `/usr/bin/x` stays a path.
4. **Intercepted builtins:** `cd`, `export`, `unset`, `source`, `exit`, `aishe`,
   the dir-stack and job builtins, `history` (state that must persist in-process).
5. **Shell-syntax signals:** lines starting with `./`, `/`, `~/`, `$(`, `(`;
   function definitions (`name() { ... }`); control-structure heads (`if`, `for`,
   `[[`, `((`, `{`, ...); env-assignment heads (`FOO=bar ...`, `arr=(a b c)`,
   `m[k]=v`).
6. **Pipelines / compound lines:** split quote- and paren-aware on top-level
   `|`/`;`/`&&`/`||`. Shell only if *every* segment's head is a known command or a
   reserved word, else natural language. (So `grep -E 'a|b'` stays one segment and
   `find junk | wat` falls through to the model.)
7. **Question grammar:** a conservative full-buffer grammar routes common
   question forms whose first word collides with a real command (`what is`,
   `where is`, `who am`, ...). Bare/option/path forms remain shell.
8. **Cache hit:** if the effective head (after skipping `K=V` prefixes) is in the
   command cache, it is shell.
9. **Otherwise:** natural language.

### The command cache

`CommandCache` backs rule 7. `build()` scans `$PATH` synchronously (so builtins
and PATH commands are recognized on the very first prompt) and then fetches zsh
builtins, aliases, and functions on a background thread (so a slow `.zshrc`
doesn't block startup). A hardcoded `FALLBACK_BUILTINS` set covers the window
before the async fetch lands and the case where querying zsh fails. `aishe
rehash` rebuilds it synchronously.

The classification logic is pure and exhaustively unit-tested at the bottom of
`src/dispatcher.rs` and in `tests/dispatcher.rs`; the deterministic PTY suites
(below) prove the same routing holds through a real shell.

## Front-ends

The interactive shell is the zsh-PTY wrapper; the same hook also works as a pure
shell integration, and there are the non-interactive `-c`/pipe paths. There is no
built-in line editor (the architectural decision, PLAN section 2, committed to
zsh; the former reedline front-end was removed).

### zsh-PTY (`src/pty.rs` + `src/integration.rs`) - the interactive shell

Runs the user's genuine interactive zsh (`zsh -i`) inside a pseudo-terminal, so
every zsh extension (autosuggestions, syntax-highlighting, fzf-tab, powerlevel10k,
oh-my-zsh) works unmodified because it *is* the user's zsh. aishe only proxies the
terminal bytes and injects its hook via an isolated `ZDOTDIR` that sources the
user's real config and then appends `aishe init zsh`.

The hook (in `src/integration.rs`) is the subtle part:

- zsh runs `command_not_found_handler` in a **subshell**, so it cannot touch the
  line editor or shell state (`cd`/`export` there would be discarded). The handler
  therefore writes the intended action to a per-shell temp file
  (`$AISHE_PENDING_FILE`).
- A `precmd` hook runs in the **main** shell before the next prompt and acts on
  that file: in `suggest` it `print -z`s the command onto the buffer to confirm or
  edit; in `auto` it `eval`s a safe command (so `cd`/`export` persist and it lands
  in history) or holds a dangerous one for review; in `yolo` the handler runs
  `aishe --yolo-line` inline.
- A ZLE accept-line wrapper strips the `?`/`#` sigil before zsh parses the line
  (so the metacharacters never reach zsh's grammar), a force-NL widget
  (default Alt-Enter) rewrites the current buffer with an LLM suggestion, and a
  mode-cycle widget (default Shift-Tab) rotates `AISHE_MODE` and repaints the
  prompt glyph.

The same `init` mechanism also works as a pure hook in the user's own shell
without the PTY (`eval "$(aishe init zsh)"` in `.zshrc`); the PTY wrapper is just
the turnkey path that needs no rc edit. bash has an equivalent script (the way to
use aishe interactively without zsh).

### Non-interactive (`-c`, piped stdin)

`aishe -c '<line>'` and piped stdin drive the in-process `executor` + `dispatcher`
+ `modes` directly (no PTY, no zsh required - `zsh -c` with a `bash -c` fallback).
These share the engine with the shell hooks; they never touch an interactive
front-end.

## The managed AI path

When `dispatch` returns `NaturalLanguage`, `agent::controller` resolves a
backend-neutral `PromptRequest` containing the shell/workspace identity, mode,
scope, network capability, model configuration, context, output policy, and
budget. OpenCode is the default `AgentBackend`; the native provider/mode path is
available only as a pre-admission repair and legacy-resume compatibility layer.

### Runtime and supervisor

`backend::manifest` embeds the exact OpenCode version, per-platform asset names,
archive sizes, and SHA-256 values. `backend::runtime` performs bounded
download/copy, safe tar/zip extraction (entry count, expanded bytes, traversal,
link/special-file rejection), executable version verification, private license
and metadata installation, atomic activation, compatible rollback, and bounded
staging GC.

`backend::supervisor` owns one private per-user topology:

```text
foreground Aishe
  -> authenticated control server (random 127.0.0.1 port)
     -> OpenCode server (separate random 127.0.0.1 port + Basic Auth)
     -> durable bridge/session journals
```

It launches OpenCode with an isolated HOME/XDG tree, explicit environment, one
embedded hash-verified plugin, and the exact verified executable. State files
carry schema/protocol/runtime/plugin identity and private startup credentials;
clients validate all of them and the owning processes before connecting.
Supervisor state, control requests, SSE frames, outputs, and logs are bounded.
The process exits after a configurable idle timeout.

### OpenCode adapter and event recovery

`backend::opencode::client` implements only the pinned v1 REST/SSE surface. It
subscribes before posting a prompt, snapshots message IDs, lets OpenCode assign
its monotonic user-message ID, and binds events to the exact new user/assistant
turn. `mapper` normalizes text, reasoning, tool lifecycle, todo, diff, child
session, compaction, usage, error, and idle events into `AgentEvent`.

Early part events are buffered until the assistant ID is proven; stale idle and
unrelated-session events are ignored. A disconnect is repaired with bounded
message/session snapshots rather than assuming replay. Prompt abort is a
first-class backend operation. The fixture suite freezes the v1.18.9 endpoints
and every normalized event class.

`backend::opencode::session` atomically maps `(aishe shell ID, canonical
workspace)` to the durable OpenCode session. That is why separate hook processes
and supervisor restarts share conversation context. Managed mappings and legacy
native task records appear in one `aishe sessions` view.

### Trusted plugin and foreground bridge

The dependency-free plugin in `assets/backend/opencode/aishe-plugin.mjs`
generates provider configuration from Aishe's active provider/model/credential,
requires Aishe authorization before each provider turn, reports authoritative
usage, hides/denies OpenCode built-in host tools, and exposes only proxy tools.
Suggest explicitly disables every tool because permission denial alone does not
remove schemas from some OpenCode/provider requests.

`backend::bridge` registers a short-lived lease for the foreground process.
Every plugin request must match its session/message/call/agent/directory/worktree
identity and a registered parent/child ancestry. Tool state is persisted before
dispatch (`admitted -> dispatched -> started -> completed`); completed calls
replay the prior result, while an interrupted started call becomes
`outcome_unknown`. Provider usage is deduplicated by message ID. Budget
reservations expire if a provider never reports a failed turn, preventing
permanent budget lockout without permitting unbounded spend.

The foreground `ToolWorker` adapts Aishe's command/file/web/MCP/skill tools.
Model-controlled child processes receive an explicit sanitized environment:
provider variables, all `AISHE_*`/`OPENCODE_*`, and likely secret names are
removed. Tool output is redacted and bounded before it crosses the bridge.

### Modes, scope, and rendering

- **Suggest** provides no tools and normalizes the final answer/command for
  review.
- **Auto** exposes Aishe proxy tools but keeps Aishe's action approval policy.
- **Yolo** requires a one-time workspace/host acceptance for each shell, then
  has no per-action Aishe or OpenCode approvals. Acceptance is in-memory and
  cannot leak into a new shell.

On Linux, `sandbox.linux_backend = "bwrap"` applies a read-only host, writable
canonical workspace/private `/tmp`, and explicit network profile. The functional
self-test distinguishes a missing binary from unusable namespaces. macOS is
explicitly policy-only. `safety.rs` remains a deterministic defense-in-depth
screen for suggest/auto and native compatibility; it is not represented as the
OS boundary.

`agent::renderer` presents normalized events inline in compact or detailed form,
honors width/color/redirected output, and keeps OpenCode implementation details
out of the UI. `usagelog` merges authoritative usage, backend/scope/network,
task, elapsed time, and context-token state into the right/below/off statusline.

### Provider and native compatibility layer

`providers` still supplies provider catalog/model discovery, setup probes,
embedding support, the deterministic fake provider, and the temporary native
compatibility engine. Its Anthropic/OpenAI wire implementations and structured
output step-down remain tested for legacy resume and pre-admission fallback.
Once OpenCode has admitted a prompt, emitted output, or requested an effect,
Aishe never falls through to native or starts a second provider request.

## Cross-cutting concerns

- **Config and credentials (`config.rs`, `credentials.rs`).** Schema-v4 config
  migration creates a private backup and atomically adds non-secret credential
  profile references. API keys live in a separate mode-`0600`, versioned,
  atomically written shared file; one resolver applies environment > staged
  setup value > saved profile precedence for every provider path. Ordinary
  config rewrites remain atomic. Precedence is `CLI flags > project overlay >
  user config > compiled defaults`. `Config::apply_overrides` applies the flag layer;
  `Config::apply_project_overlay` merges a repo's `.aishe/config.toml` under the
  tiered trust rules (safe keys always, sensitive keys only when `trust::is_trusted`),
  walking up from cwd. Audit logging resolves `AISHE_LOG`/`AISHE_LOG_FILE` over the
  file via `resolve_audit` (in `main.rs`). Missing non-TTY config is actionable and
  never creates guessed defaults; guided setup is explicit and resumable. A
  pre-rename `llmsh` config is migrated on first run. All precedence is unit/E2E
  tested.
- **Organization policy (`policy.rs`).** A root/admin file can require or
  disable the managed backend, pin a mirror/hash set, require functional
  bubblewrap, restrict scope/network/provider/model/MCP/skills, require
  audit/redaction, and cap budget/output. It only narrows authority and is
  validated before Apply and use.
- **Project trust (`trust.rs`).** A small JSON store under the data dir mapping a
  project config's absolute path to a content hash, so editing a trusted file
  drops trust. Managed with `aishe trust` / `aishe untrust`. See
  [project-config.md](project-config.md).
- **Usage and budget (`usage.rs`, `backend::bridge`).** OpenCode reports
  authoritative per-message usage through the trusted plugin. The bridge
  deduplicates it, reserves estimated cost before provider turns, caps output,
  expires abandoned reservations, and denies the next call before budget
  overrun. `usagelog.rs` combines short-lived PTY child results into ordered
  live right/below status metrics.
- **Audit and redaction.** Off by default. When on, prompts/responses/actions are
  written as JSONL; redaction applies to both the context block and the log.

## Tests and the harness

See [development.md](development.md) for commands. The shape:

- **Unit tests** live inline in each `src/` module (dispatcher classification,
  safety patterns, provider step-down helpers, config precedence, ...).
- **Rust integration tests** in `tests/` spawn the real backing shell or a mock
  HTTP server: `cli.rs`, `dispatcher.rs`, `executor.rs`, `modes.rs`,
  `providers.rs` (mockito), `safety.rs`, `mcp.rs`, `mcp_http.rs`.
- **Deterministic PTY suites** (Python, driven by the fake provider, no key, in
  CI): `tests/pty_scenarios.py` (targeted flows), `tests/pty_fuzz.py` (thousands
  of generative cases, logged to `test-results/fuzz-*.md`), and
  `tests/zsh_features.py` (a 44-case zsh feature matrix), plus
  `tests/setup_pty.py`, `tests/statusline_pty.py`, and
  `tests/durable_task_resume.py`. These prove setup cancellation, route-aware
  highlighting, prompt status, and interrupted-task recovery through real PTYs.
- **Pinned OpenCode runtime contract:** `tests/opencode_runtime_contract.py`
  launches the exact v1.18.9 runtime with a deterministic local provider and the
  real trusted plugin/bridge. It proves two-turn session continuity,
  suggest-tool absence, Aishe-only auto tools, foreground command execution,
  provider credential isolation, exact usage, durable journals, and secret
  non-persistence.
- **Frozen OpenCode fixtures:** `tests/fixtures/opencode/v1.18.9/` locks the
  supported OpenAPI endpoints and representative text/reasoning/tool/todo/diff/
  compaction/usage/idle event mapping.
- **Opt-in real-model suite:** `tests/real_model.py` runs a classification corpus
  against a live endpoint when `AISHE_REALTEST_KEY` is set.
- **Validation harness:** `tests/admin_validation.py` exercises a broad surface
  and writes a timestamped report under `test-results/`.

The deterministic suites are the pass gate; key-gated and model suites are
informational because model output varies.

## Where to start reading

- To change routing: `src/dispatcher.rs` (and its tests).
- To change how commands run: `src/executor.rs`.
- To change the interactive experience: `src/pty.rs` + `src/integration.rs` (the
  default) — the only interactive front-end.
- To change LLM behavior: `src/modes/` and `src/providers/`.
- To add a tool: `src/tools.rs` (built-in) or an MCP server in config.
