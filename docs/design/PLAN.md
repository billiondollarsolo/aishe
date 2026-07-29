# aishe master plan

A detailed, forward-looking plan for taking aishe from a feature-rich pre-1.0
tool to a stable, installable, trustworthy "AI shell". This is the long-form
companion to [ROADMAP.md](ROADMAP.md): the roadmap is the checklist, this is the
reasoning, sequencing, and acceptance criteria behind it.

> **Current agent architecture:** the managed OpenCode milestone is specified
> and tracked in
> [OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md](OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md).
> That document supersedes this older plan anywhere it discusses a single
> process, direct provider/tool loops, setup, packaging, or agent-session
> behavior. Direct shell execution remains service-free and lazy.

Each work item lists: **Goal**, **Why**, **Approach** (files to touch),
**Acceptance** (how we know it is done), **Effort** (S/M/L/XL), **Priority**
(P0 ship-blocking, P1 high, P2 normal, P3 nice-to-have), and **Status**.

Last updated: June 2026.

---

## 0. Where we are

aishe is a natural-language-aware shell. It behaves like zsh for real commands
and routes anything else to an LLM in one of three modes (suggest / auto / yolo).
Already shipped and validated:

- **One interactive front-end.** A zsh-PTY wrapper that drives the user's real
  interactive zsh with all native plugins (a `command_not_found` hook plus
  `precmd`/accept-line wrappers route natural language to the model). It requires
  zsh; the non-interactive paths (`aishe -c …`, piped stdin) and the bash hook
  work without it. The former opt-in reedline editor was removed (the
  architectural decision, section 2, resolved as Option B and shipped).
- **Providers.** Anthropic Messages API, official OpenAI Responses API, and
  custom OpenAI-compatible Chat Completions APIs, with SSE streaming, tool use,
  token/cost metering, a budget cap, and retry with exponential backoff + jitter
  + `Retry-After`.
- **Agentic yolo loop.** `run_command` plus built-in file tools, a `fetch_url`
  web tool, MCP tools, skills (progressive disclosure), plan-first dry run, and
  policy sandbox / confirmation tiers.
- **MCP client.** stdio and Streamable HTTP transports; consumes tools,
  resources, and prompts; namespaced `mcp__<server>__<tool>`; verified against
  real `npx`/`uvx` servers.
- **zsh parity (removed with reedline).** Completion (commands, paths, env vars,
  subcommands, `--help` flags), multi-line editing, history expansion, autocd,
  dir stack,
  spelling correction, named dirs, AUTO_PUSHD, cdpath, a rich async git prompt,
  background job control, and a timestamped cross-session `history`.
- **Trust and safety.** Path-aware safety gate with an adversarial corpus, secret
  redaction, audit logging, sandbox/confirm tiers.
- **Distribution.** Release CI (tarballs + checksums), `cargo binstall` metadata,
  a Homebrew formula template, `aishe completions`, build metadata in
  `--version`, a richer `aishe doctor`.

Quality bar today: ~215 Rust tests, a Python validation harness (~483 checks
across 7 suites, including real model and real MCP servers), a PTY smoke test,
CI on Linux + macOS, clippy `-D warnings` and rustfmt clean.

The gaps that remain are mostly **depth, polish, robustness, and the one big
architectural decision** below.

---

## 1. Guiding principles

1. **Linux still works like Linux.** Real commands must always behave exactly as
   the underlying shell would. The harness's Suite 1 (300+ command diffs) is the
   guardrail; it must stay green.
2. **The model is opt-in and bounded.** No surprise spend, no surprise actions.
   Budget caps, confirmation, redaction, and audit logging are first-class.
3. **Single binary, no services.** No daemon, no database, minimal C deps. Easy
   to install, copy, and reason about.
4. **Best-effort, never blocking.** Anything that talks to the network or a slow
   subprocess (git status, `--help` parsing, the command cache) runs off the hot path or with a
   timeout; the prompt never hangs.
5. **Documented and tested as we go.** Every feature ships with docs, a config
   toggle where relevant, unit tests, and harness coverage.

---

## 2. The architectural decision (resolve first)

**The fork:** the reedline front-end simulates shell state by running each line
as a fresh `zsh -c` and replaying `cd`/`export`/aliases/functions from a
generated rc. This is why job control, `Ctrl-Z`, process groups, and some subtle
state interactions are hard there, while the zsh-PTY front-end gets them free.

We have three coherent end states:

- **Option A - reedline as flagship.** Keep investing in reedline parity. Pro:
  full control of the UX (ghost text, inline AI, custom completion, prompts).
  Con: we are slowly reimplementing a shell; job control and exotic state will
  always lag. Cost: XL, ongoing.
- **Option B - zsh-PTY as flagship, reedline as the lightweight AI command
  runner.** Position zsh-pty as "your real shell, now with AI" and reedline as a
  fast keyless command runner / scripting front-end. Pro: real shell semantics
  for free; less reinvention. Con: the AI UX in a PTY is constrained (we inject a
  `command_not_found` hook rather than owning the line editor). Cost: M to
  reposition, then most parity items become "not our problem".
- **Option C - persistent backing shell for reedline.** Replace fresh `zsh -c`
  with one long-lived `zsh` we feed commands to (PTY or coprocess), so reedline
  keeps its UX but gains real state and job control. Pro: best of both. Con: the
  hardest engineering (synchronizing prompts, capturing output boundaries,
  signal handling); essentially building a terminal multiplexer. Cost: XL.

**Decision: Option B, fully executed.** The zsh-PTY front-end is *the*
interactive shell ("your real shell, now with AI"); the binary requires zsh for
it and prints an install hint when it is missing. The built-in reedline editor
has been **removed entirely** (the `completer`/`highlight`/`ghost`/`prompt`/
`theme`/`validator`/`history_expand`/`histfilter` modules, the `reedline`
dependency, the `--pty`/`--no-pty` flags, and the reedline-only config keys all
gone). Non-interactive use (`aishe -c …`, piped stdin) and the bash hook still
work without zsh. **Status: RESOLVED and shipped.** Every `(reedline)` parity
item below is therefore moot/dropped. Option C (a persistent backing shell)
remains a stretch/moonshot, not planned work.

---

## 3. Milestones toward 1.0

### Phase 1 - Stabilize and make it real (P0/P1)
Get to a confidently installable, documented 0.2 / 0.3.
- Resolve the architectural decision (section 2).
- Man page; finish the Homebrew tap and verify a real tagged release end to end.
- Reconcile the repo identity (`Cargo.toml` `repository`, binstall, Homebrew URLs
  vs the actual GitHub remote). **P0** for binstall/Homebrew to actually work.
- Config precedence + migration tests; a documented config-stability promise.
- Fill the remaining test-surface gaps (interactive PTY behaviors, large/binary
  output, schema step-down).

### Phase 2 - Depth (P1/P2)
- The big AI-shell differentiators that are not yet built (section 4.A).
- Completion depth round 2 (zsh compsys-level), suffix/global aliases.
- A real sandbox (container/namespace) behind a feature flag.

### Phase 3 - Ecosystem and scale (P2/P3)
- Plugin/skill distribution, a command/skill registry, MCP server presets.
- Local-model ergonomics, response/embedding caches, per-project profiles.
- Telemetry stance + optional opt-in metrics.

---

## 4. Detailed work items

### A. AI-shell features

**A1. True dry-run / batch approval for yolo.**
- Goal: beyond plan-first, let the user approve or edit the *concrete* command
  batch before each tool round, with per-command accept/skip/edit.
- Why: trust for destructive multi-step runs.
- Approach: extend `src/modes/yolo.rs` to surface each planned `run_command`
  before execution with `[y]/[n]/[e]/[a]ll` controls; reuse the safety-gate UX.
- Acceptance: a yolo run pauses per command at the chosen tier; "all" disables
  further prompts for the run; harness + a PTY check.
- Effort: M. Priority: P2. Status: open.

**A2. Per-project profiles (`.aishe/config.toml`).**
- Goal: a project-local config overlay (mode, model, mcp servers, budget) merged
  over the user config, discovered like `.aishe/context.md`.
- Why: different repos want different models/tools/budgets.
- Approach: `src/config.rs` gains a layered load (user then project), walking up
  from cwd; document precedence; `aishe doctor` shows the active layers.
- Acceptance: a project `.aishe/config.toml` overrides specific keys only;
  round-trips and precedence are tested.
- Effort: M. Priority: P1. Status: DONE. `Config::apply_project_overlay` discovers
  `.aishe/config.toml` walking up from cwd and deep-merges it between the user
  config and CLI flags. Tiered trust (`src/trust.rs`): safe keys (cosmetics, mode
  for suggest/auto, per-provider `model`) apply automatically; sensitive keys
  (provider/endpoint, `[mcp_servers]`, logging, safety toggles, `mode = "yolo"`)
  apply only after `aishe trust`. `aishe doctor` shows the active layer + trust
  state. Unit + E2E tested; documented in
  [docs/project-config.md](project-config.md).

**A3. Response + embedding cache on disk.**
- Goal: persist the suggest-mode cache (currently in-memory only) across
  sessions, optionally keyed by an embedding similarity rather than exact match.
- Why: more hits, lower latency/cost.
- Approach: extend `src/cache.rs` with an optional on-disk store (JSONL under the
  data dir, TTL'd); exact-match first, semantic later (needs an embeddings
  provider abstraction).
- Acceptance: a repeat across processes is served from disk; cache is bounded and
  prunes expired entries; opt-in via config.
- Effort: M (exact) / L (semantic). Priority: P2. Status: open.

**A4. Multi-step task memory and "continue".**
- Goal: a durable, resumable task log so a long yolo task can be paused/resumed
  and referenced ("continue where we left off").
- Why: real work spans sessions.
- Approach: persist the yolo transcript (redacted) under the data dir; an `aishe
  resume` command.
- Acceptance: a yolo task survives a restart and resumes with context.
- Effort: L. Priority: P3. Status: open.

**A5. Inline-AI polish.** ~~Ghost-text live repaint during pause; accept-word vs
accept-line; a "why" affordance to expand the suggestion rationale.~~
- Status: DROPPED. Ghost text was a reedline feature and was removed with it.

**A6. Output understanding.** Let the model see (truncated, redacted) output of
the *last real command* on `?` follow-ups automatically, not just on yolo tool
results.
- Approach: `src/context.rs` already includes recent commands; add an opt-in
  "last output" capture for the immediately preceding command.
- Acceptance: `<cmd>` then `?why did that fail` includes the captured output.
- Effort: M. Priority: P2. Status: partial (recent commands only).

### B. Trust and safety

**B1. Real sandbox (container/namespace).**
- Goal: an actually enforced restricted exec for yolo, not just policy refusal:
  no network, a scratch dir, read-only mounts.
- Why: policy-based `yolo_sandbox` is best-effort; a real boundary is the ask.
- Approach: behind a feature flag, run tool commands via
  `bubblewrap`/`unshare`/`firejail` on Linux and `sandbox-exec` on macOS when
  present; fall back to the policy sandbox. New `src/sandbox.rs` backends.
- Acceptance: a sandboxed yolo run cannot reach the network or write outside the
  scratch dir, verified on Linux CI with `bwrap`.
- Effort: L. Priority: P2. Status: open (policy tier shipped).

**B2. Confirmation tiers UX + audit of refusals.**
- Goal: log sandbox refusals and tier prompts to the audit log; a per-session
  "what did the AI try" summary.
- Effort: S. Priority: P2. Status: open.

**B3. Safety-gate corpus growth + fuzzing.**
- Goal: keep `tests/safety_corpus.rs` ahead of bypass techniques; add a small
  fuzz target over the gate's tokenizer/unwrapper.
- Effort: M. Priority: P2. Status: open.

**B4. Redaction coverage.** Expand `src/redact.rs` for cloud creds (GCP, Azure),
JWTs, private-key blocks; add a redaction self-test corpus.
- Effort: S. Priority: P2. Status: open.

### C. zsh parity (DROPPED - reedline removed per section 2)

> The interactive shell is now the user's real zsh, which has all of this
> natively, so this whole backlog is dropped. Retained for historical context.

**C1. Full job control** (only if Option A/C). `Ctrl-Z`/`SIGTSTP`, process
groups, `fg`/`bg` of suspended jobs. Requires a persistent backing shell.
- Effort: XL. Priority: P2 (or DROP under Option B). Status: partial (background
  jobs only).

**C2. Completion depth round 2.** zsh compsys-level: glob qualifiers, completion
from man pages, per-argument context (e.g. `git checkout <branch|file>`),
descriptions for more tools, `kill <pid>` / `ssh <host>` completers.
- Approach: extend `src/completer.rs`; consider a small declarative spec for
  per-command argument kinds.
- Effort: L. Priority: P2. Status: partial (`--help` flags shipped).

**C3. Global and suffix aliases.** zsh `alias -g` / `alias -s`.
- Effort: M. Priority: P3. Status: open.

**C4. History timestamps in up-arrow + cross-session live import.** Surface the
EXTENDED_HISTORY timestamps in the Ctrl-R menu; live-import other sessions'
commands (reedline `sync`).
- Effort: M. Priority: P3. Status: partial (`history` builtin shipped).

**C5. Async vcs_info live repaint.** Repaint the prompt when the background git
status lands, instead of showing it one prompt later.
- Approach: needs a reedline repaint signal or a custom event loop tick.
- Effort: M. Priority: P3. Status: partial (async compute shipped).

### D. Front-end and UX

**D1. Right-prompt / transient prompt polish**, configurable segments, a
powerlevel-style preset.
- Effort: M. Priority: P3. Status: open.

**D2. A `--script <file>` runner** (beyond pipe mode): run a file of aishe lines
with proper `$?` and error handling, shebang-friendly (`#!/usr/bin/env aishe`).
- Effort: S. Priority: P2. Status: partial (pipe/stdin mode shipped).

**D3. TUI dashboard (`aishe top`?)** for session usage/cost, audit tail, MCP
status. Stretch.
- Effort: L. Priority: P3. Status: open.

### E. Providers and model layer

**E1. Schema -> json -> prompt step-down test.** The defensive structured-output
fallback path is untested end to end.
- Approach: mock a provider that rejects `json_schema` then `json_object`; assert
  the step-down in `tests/providers.rs`.
- Effort: S. Priority: P1. Status: DONE. `tests/providers.rs` covers the full
  chain (`openai_steps_down_all_the_way_to_text`) and the terminal give-up
  (`openai_gives_up_when_even_text_is_rejected`); the pure `step_down` /
  `is_format_error` helpers are unit-tested in `openai_compat.rs`.

**E2. More providers / presets.** First-class presets for Groq, Ollama,
OpenRouter, Together, Azure OpenAI, Bedrock (auth differences); `aishe` wizard
offers them.
- Effort: M. Priority: P2. Status: partial (OpenAI-compatible base works).

**E3. Local-model ergonomics.** Detect Ollama, suggest small models, handle
no-tool-support gracefully (already falls back), tune timeouts for slow local
inference.
- Effort: M. Priority: P2. Status: open.

**E4. Prompt-cache / token-efficiency.** Use Anthropic prompt caching and trim
the context block adaptively to the model's window.
- Effort: M. Priority: P2. Status: open.

**E5. Embeddings provider abstraction** (enables A3 semantic cache, future RAG
over `.aishe/`).
- Effort: M. Priority: P3. Status: open.

### F. MCP and extensibility

**F1. MCP server presets and `aishe mcp add`.** A curated list (filesystem, git,
fetch, sqlite) and a command to scaffold `[mcp_servers]` entries.
- Effort: S. Priority: P2. Status: open.

**F2. MCP roots, sampling, and elicitation.** Support more of the MCP spec:
expose roots, answer `sampling/createMessage` (server asks our model), handle
`elicitation`. Today server-initiated requests are ignored.
- Effort: L. Priority: P3. Status: open (tools/resources/prompts shipped).

**F3. MCP lifecycle robustness.** Reconnect on a dropped stdio server, health in
`aishe doctor`/`aishe mcp`, per-server enable/disable at runtime.
- Effort: M. Priority: P2. Status: open.

**F4. Skill/command sharing.** A simple way to install community skills/commands
(`aishe skill add <url|name>`), with the existing Claude-Code-compatible format.
- Effort: M. Priority: P3. Status: open.

### G. Testing and QA

**G1. Interactive PTY behavior tests.** `Ctrl-C` mid-command, `Ctrl-Z`, window
resize, completion-menu navigation, multi-line editing. Extend the smoke harness
into a small expect-style suite.
- Effort: M. Priority: P1. Status: DONE. `tests/pty_signals.py` (in CI) drives the
  real wrapped zsh through Ctrl-C mid-command (shell survives), Ctrl-C on an empty
  line, Ctrl-Z job suspension, window resize (SIGWINCH propagation updates
  `$COLUMNS`), and multi-line for-loop continuation. Completion-menu navigation is
  native zsh ZLE (depends on the user's setup) and is left untouched.

**G2. I/O edge tests.** Large captured output truncation, binary output handling,
Unicode/emoji line editing, very long lines.
- Effort: M. Priority: P2. Status: partial (pipe mode tested).

**G3. Config precedence + migration tests.** env vs `--flags` vs file vs project
overlay; the legacy `llmsh` migration path.
- Effort: S. Priority: P1. Status: DONE for the layers that exist today.
  `--flags > file > defaults` is covered by `Config::apply_overrides` unit tests
  plus an E2E (`tests/cli.rs::cli_flags_are_accepted_over_config`); the audit
  env-vs-file precedence by `resolve_audit` unit tests; and the legacy `llmsh`
  migration by `tests/cli.rs::legacy_llmsh_config_is_migrated_on_run`. The
  per-project `.aishe/config.toml` overlay (A2) adds the layer between user config
  and flags, with its own precedence/tiering tests.

**G4. Harness as CI gate.** Run the deterministic suites (no key) in CI on every
PR; nightly run with a key (and real MCP) in a protected workflow.
- Effort: S. Priority: P1. Status: DONE for the per-PR gate. `ci.yml` runs the
  deterministic PTY suites (`pty_scenarios.py`, `pty_fuzz.py`, `zsh_features.py`)
  with the fake provider on every push/PR; `real_model.py` runs only when the
  `AISHE_REALTEST_KEY` secret is present. A scheduled nightly with real MCP is
  still open.

**G5. Coverage + a fuzz target** for the dispatcher and safety tokenizer.
- Effort: M. Priority: P2. Status: open.

**G6. Cross-shell tests.** Validate the bash backend (not just zsh) in the
harness and smokes.
- Effort: M. Priority: P2. Status: open.

### H. Distribution and packaging

**H1. Real release dry-run.** Cut a `v0.2.0` tag, confirm the release workflow
produces working tarballs + checksums on all three targets, and that
`cargo binstall aishe` resolves them. **Blocked on H2.**
- Effort: S. Priority: P0. Status: open.

**H2. Repo identity reconciliation.** Align `Cargo.toml` `repository`, the
binstall `pkg-url`, the Homebrew formula URLs, and the README badges with the
actual GitHub remote. Decide canonical owner.
- Effort: S. Priority: P0. Status: DONE. Canonical owner is
  `billiondollarsolo/aishe` (the actual remote); `Cargo.toml`, the binstall
  `pkg-url`, the Homebrew formula (now incl. aarch64-linux), and the docs all
  resolve there.

**H3. Man page.** Generate `aishe.1` (clap_mangen via build.rs or an `aishe man`
command) and install it from Homebrew + the tarball.
- Effort: S. Priority: P1. Status: open.

**H4. aarch64-linux + musl static builds.** Add `aarch64-unknown-linux-gnu` and a
`x86_64-unknown-linux-musl` fully-static target to the release matrix (cross or
`cargo-zigbuild`).
- Effort: M. Priority: P2. Status: DONE. The release matrix now builds
  `x86_64`/`aarch64` for both `-gnu` and static `-musl` via `cargo-zigbuild`.

**H5. Publish to crates.io.** Ensure metadata, license, and a clean
`cargo publish --dry-run`; decide whether to publish the lib.
- Effort: S. Priority: P2. Status: open.

**H6. Linux packages.** A `.deb`/`.rpm` (nfpm) and an AUR/Nix expression.
- Effort: M. Priority: P3. Status: partial. `.deb`/`.rpm` ship from the release
  workflow (nfpm, `nfpm.yaml`) for amd64/arm64 with completions + man page; a
  `curl | sh` `install.sh` is also added. AUR/Nix expressions still open.

### I. Documentation

**I1. Quickstart GIF / asciinema** on the README; a 60-second tour.
- Effort: S. Priority: P2. Status: open.

**I2. Recipes/cookbook** doc: common yolo tasks, MCP setups, safety config,
per-project profiles.
- Effort: M. Priority: P2. Status: open.

**I3. Architecture doc** for contributors: dispatcher decision order, the two
front-ends, the provider/tool/MCP layering, the harness.
- Effort: M. Priority: P1. Status: DONE. [docs/architecture.md](architecture.md)
  covers the design principles, crate/module map, the routing decision order +
  command cache, the zsh-PTY hook handoff, the
  provider/`ResponseFormat`/step-down layer, modes/safety/sandbox, tools/MCP/skills,
  config precedence, and the test layout. Linked from the README and development.md.

**I4. Security/privacy statement** consolidated: what is sent, when, redaction
guarantees and limits, audit log contents, telemetry stance.
- Effort: S. Priority: P1. Status: partial (logging.md, safety.md).

**I5. Keep docs/ROADMAP, CHANGELOG, examples/config.toml in lockstep** with every
feature (already the practice; document it in development.md).
- Effort: S. Priority: P2. Status: ongoing.

### J. Performance and reliability

**J1. Startup latency budget.** Measure and bound cold start (config load,
command-cache build, provider construction, MCP connect). MCP connect is the
likely tall pole; make it lazy/parallel.
- Approach: connect MCP servers in parallel threads; defer non-essential work
  until first use; add a `--time-startup` debug.
- Acceptance: cold start under a documented budget with N MCP servers.
- Effort: M. Priority: P2. Status: open.

**J2. Command-cache freshness.** Background rehash on `$PATH`/rc changes; today
it is built once + `rehash`.
- Effort: S. Priority: P3. Status: open.

**J3. Memory bounds.** Cap session memory, captured output, and the disk cache
explicitly; document the limits.
- Effort: S. Priority: P2. Status: partial.

### K. Observability

**K1. Structured `aishe usage --json`** and an audit-log query/summary command.
- Effort: S. Priority: P2. Status: open.

**K2. Optional, off-by-default, anonymized metrics** (counts only, no content),
with a clear consent prompt. Decide the stance first (I4).
- Effort: M. Priority: P3. Status: open (decision needed).

### L. Technical debt and refactors

**L1. Consolidate the three retry loops.** Done in spirit (shared policy), but
the per-provider POST loops still duplicate structure; extract a single
`request_with_retry` helper taking a closure.
- Effort: S. Priority: P3. Status: partial.

**L2. `main.rs` is large.** Split the REPL, the one-shot/pipe path, the meta
commands, and the doctor into modules.
- Effort: M. Priority: P2. Status: open.

**L3. Error taxonomy.** A unified error type for user-facing messages vs internal
failures; consistent exit codes.
- Effort: M. Priority: P3. Status: open.

**L4. Config schema versioning.** A `version` key + forward/back-compat strategy
so future renames migrate cleanly (we already migrated `llmsh` -> `aishe`).
- Effort: S. Priority: P2. Status: open.

### M. Community and governance

**M1. CONTRIBUTING.md, issue/PR templates, a code of conduct.**
- Effort: S. Priority: P2. Status: open.

**M2. Security policy (SECURITY.md)** and a disclosure contact.
- Effort: S. Priority: P1. Status: DONE. `SECURITY.md` covers private reporting
  (GitHub advisories + mj@alphabravo.io), supported versions, the security model
  (deterministic gate, confirmation tiers, best-effort sandbox, prompt-injection
  threats), data handling/privacy, and hardening recommendations.

**M3. A public roadmap board / labels** mapping to this plan.
- Effort: S. Priority: P3. Status: open.

---

## 5. Cross-cutting concerns

- **Privacy by default.** Redaction on, audit off, no telemetry. Any change here
  needs explicit consent UX. Document in one place (I4).
- **Config stability.** After 0.2, config keys are additive-only within a minor;
  removals/renames go through a migration (L4) and a CHANGELOG "Changed" note.
- **No surprise spend.** Budget cap, usage line, and (future) per-project budgets
  must make cost legible at all times.
- **Reproducible builds + provenance.** Checksums today; consider signing and
  SLSA provenance for releases later.

---

## 6. Stretch / moonshots

- A persistent backing shell (Option C) turning reedline into a full terminal.
- Multi-agent yolo (a planner + workers) with a shared scratch workspace.
- RAG over the repo (`.aishe/` + code index) for grounded suggestions.
- A plugin marketplace for skills/commands/MCP presets.
- A local, fully-offline mode with a bundled small model.
- Windows support (today unsupported; would need a backing shell story).

---

## 7. Definition of done (per feature)

A feature is "done" only when all of:
1. Code matches the surrounding style; clippy `-D warnings` and rustfmt clean.
2. Unit tests for the logic; harness coverage for user-visible behavior; a PTY
   smoke step if it is interactive.
3. A config toggle (where relevant) with a sensible default.
4. Docs updated: the feature doc, `docs/configuration.md`, `examples/config.toml`,
   `CHANGELOG.md`, and `docs/ROADMAP.md`.
5. Verified live where it touches a provider/MCP/PTY.
6. No em dashes in any committed artifact; secrets never committed.

---

## 8. Open decisions (need a human)

1. ~~**Architectural direction** (section 2): Option A, B, or C.~~ **Resolved:
   Option B** (zsh-PTY the one interactive shell; reedline removed). `(reedline)` parity work is now
   P3.
2. ~~**Canonical repository identity** (H2): which GitHub owner is canonical, so
   binstall/Homebrew/release URLs resolve.~~ **Resolved: `billiondollarsolo/aishe`.**
3. **Telemetry stance** (K2/I4): none, or opt-in anonymized counts.
4. **crates.io publishing** (H5): publish the binary and/or the library.
5. **Minimum supported platforms** and whether Windows is ever in scope.

---

## 9. Suggested next 10 (a concrete starting order)

Done (struck through) are kept for the record; the live order continues below.

1. ~~H2 repo identity + H1 real release dry-run.~~ DONE (repo identity reconciled;
   release workflow ships gnu/musl/aarch64 tarballs + checksums, `.deb`/`.rpm`,
   and `install.sh`).
2. ~~Section 2 architectural decision.~~ DONE (Option B).
3. ~~H3 man page.~~ DONE (help2man `aishe.1` ships from the release/package jobs).
4. ~~G4 harness in CI~~ + ~~G3 config precedence tests~~. DONE (apply_overrides /
   resolve_audit / legacy-migration tests).
5. ~~E1 schema step-down test.~~ DONE (full chain + give-up, in `tests/providers.rs`).
6. ~~I3 architecture doc~~ + ~~M2 SECURITY.md~~. DONE.
7. ~~A2 per-project profiles.~~ DONE (`.aishe/config.toml` overlay + `aishe trust`).
8. ~~G1 interactive PTY tests.~~ DONE (`tests/pty_signals.py`: Ctrl-C/Ctrl-Z/
   resize/multi-line, in CI).
9. **B1 real sandbox behind a flag** (P2, the headline safety upgrade).
10. **C2 completion depth round 2** (P2, daily-use polish).

The P1 next-10 is cleared. Live shortlist (P2): B1 real sandbox, C2 completion
depth round 2.
