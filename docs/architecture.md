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
3. **Single binary, no services.** No daemon, no database, minimal C deps. The
   provider HTTP layer is hand-rolled (no vendor SDKs) to keep request/response
   shapes under our control.
4. **Best-effort, never blocking.** Anything that hits the network or a slow
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
| `providers` | The `Provider` trait and its Anthropic / OpenAI-compatible / fake implementations; retries, streaming, usage metering. |
| `modes` | `suggest` and `yolo` (agentic) modes, the safety gate wrapper, markdown rendering. |
| `tools` | Built-in agentic tools (`read_file`/`write_file`/`edit_file`/`list_dir`, `fetch_url`). |
| `mcp` | Minimal MCP client (stdio + Streamable HTTP); exposes server tools to yolo as `mcp__<server>__<tool>`. |
| `safety` / `sandbox` | Destructive-command gate; best-effort policy sandbox (network / out-of-tree writes) for yolo. |
| `context` | The environment context block (cwd, dir listing, recent history, project context) prepended to LLM requests. |
| `session` | In-session conversation memory, persisted to a per-session file for the hook front-ends (whose NL calls are separate processes). |
| `config` | Config schema, load/migrate/save, the first-run wizard, CLI-override and project-overlay precedence. |
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
7. **Cache hit:** if the effective head (after skipping `K=V` prefixes) is in the
   command cache, it is shell.
8. **Otherwise:** natural language.

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

## The LLM path

When `dispatch` returns `NaturalLanguage`, a mode handles it.

### Providers (`src/providers/`)

`Provider` is a small `Send + Sync` trait: `complete`, `complete_stream`,
`complete_with_tools`, `complete_with_tools_stream`, and `meter`. Messages are
provider-neutral (`Msg::User/Assistant/ToolResult`); Responses tool turns also
carry opaque provider items so reasoning state can be replayed. Each
implementation maps messages to its wire format.

- `anthropic.rs`: Anthropic Messages API.
- `openai_compat.rs`: OpenAI Responses for the official API URL; Chat
  Completions for custom compatible URLs (Groq, Ollama, OpenRouter, ...).
- `fake.rs`: a deterministic provider (no network/key) selected when
  `AISHE_FAKE_LLM[_FILE]` is set; the backbone of the deterministic PTY suites.

`providers::make(config)` builds the configured provider, returns the fake when
the env hook is set, and optionally wraps it in `cache::CachingProvider`. Retries
(429/5xx/connection) use shared backoff-with-jitter that honors `Retry-After`.

**Structured-output step-down.** `ResponseFormat` is `Text` / `Json` /
`JsonSchema`. Strict schema is best-effort: if an OpenAI-compatible server rejects
`json_schema` (or `json_object`) with a format-shaped 400, `complete` steps the
format down (`schema -> json -> text`) and retries, terminating if even text is
rejected so the loop can't spin. Callers always parse defensively regardless. See
`step_down`/`is_format_error` and the chain tests in `tests/providers.rs`.

### Context and memory

`context.rs` builds the block prepended to each request: cwd, a capped directory
listing, recent command history, and an optional per-project `.aishe/context.md`.
It never includes file contents. `redact.rs` scrubs likely secrets first (when
`redact_secrets` is on). `session.rs` keeps a rolling transcript so follow-ups
have context, persisted to a per-session file by the hook
front-ends (whose NL calls are separate processes).

### Modes (`src/modes/`)

- **suggest** (`suggest.rs`): one constrained completion, parsed into a
  command-or-answer. A dangerous command is flagged before you confirm. `auto` is
  the same classification but safe commands run immediately.
- **yolo** (`yolo.rs`): an agentic loop. The model is offered `run_command` plus
  (per config) the built-in file/web tools, MCP tools, and skills. Each tool call
  is gated, run, and its result fed back as a `Msg::ToolResult` until the model
  stops or `max_yolo_iterations` is hit.

### The safety gate and sandbox

`safety.rs` is a deterministic, pattern-based classifier (`Risk`). It normalizes a
line, splits on operators, strips privilege/wrapper prefixes (`sudo`, `env`,
`time`, ...) so they can't smuggle a command past the anchored patterns, and is
path-aware for `rm -rf` (an in-tree relative target is allowed). `modes::safety_gate`
wraps it; `sandbox.rs` adds the optional yolo policy sandbox (refuse network /
out-of-tree writes), fed back to the model as a tool error. Neither is a kernel
sandbox; both are documented as best-effort in [SECURITY.md](../SECURITY.md).

### Tools, MCP, and skills

`tools.rs` defines the built-in tools as `ToolDef`s (name + description + JSON
schema) and executes them, with the same out-of-tree write confirmation as the
command gate. `mcp.rs` is a minimal MCP client: it connects to `[mcp_servers]`
over stdio or Streamable HTTP, does the JSON-RPC handshake, lists tools, and
proxies `tools/call`, namespacing each tool `mcp__<server>__<tool>` so the whole
ecosystem plugs in alongside the built-ins. `skills.rs` loads progressive-disclosure
skills in the Claude-Code-compatible format.

## Cross-cutting concerns

- **Config (`config.rs`).** Precedence is `CLI flags > project overlay > user
  config > compiled defaults`. `Config::apply_overrides` applies the flag layer;
  `Config::apply_project_overlay` merges a repo's `.aishe/config.toml` under the
  tiered trust rules (safe keys always, sensitive keys only when `trust::is_trusted`),
  walking up from cwd. Audit logging resolves `AISHE_LOG`/`AISHE_LOG_FILE` over the
  file via `resolve_audit` (in `main.rs`). A missing config triggers the first-run
  wizard (only on a TTY); a pre-rename `llmsh` config is migrated on first run. All
  precedence is unit/E2E tested.
- **Project trust (`trust.rs`).** A small JSON store under the data dir mapping a
  project config's absolute path to a content hash, so editing a trusted file
  drops trust. Managed with `aishe trust` / `aishe untrust`. See
  [project-config.md](project-config.md).
- **Usage and budget (`usage.rs`).** A shared `UsageMeter` per provider records
  tokens; cost is estimated from a pricing table (overridable in config) and
  enforced against `budget_usd`.
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
  `tests/zsh_features.py` (a 44-case zsh feature matrix). These prove routing and
  the hook hold through a real zsh.
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
