# aishe — Feature Proposals

Detailed, self-contained engineering specs for the next wave of aishe features.
This is the design companion to [PLAN.md](PLAN.md) (sequencing/reasoning) and
[ROADMAP.md](ROADMAP.md) (the checklist): each proposal here is sized to be picked
up and built independently, with concrete file targets, tests, and a definition of
done.

Two tracks:

- **Robustness** — earn the trust to run aishe on real machines (R1–R5).
- **Noteworthy** — the differentiators that get aishe talked about (N1–N5).

> Status legend: **Proposed** (designed, not started) · **In progress** · **Done**.
> Every proposal ships behind a config flag (default off unless noted), with docs,
> unit tests, and harness coverage, per the project's guiding principles.

## Summary

| ID | Title | Track | Priority | Effort | Depends on |
|----|-------|-------|----------|--------|-----------|
| R1 | Transactional file edits + `aishe undo` | Robustness | P1 | M | — |
| R2 | Real sandbox backend for yolo | Robustness | P1 | XL | R1 (diff UX) |
| R3 | Parse-based safety gate | Robustness | P2 | L | — |
| R4 | Provider fallback chain + offline | Robustness | P2 | M | — |
| R5 | Queryable audit log + cost observability | Robustness | P2 | M | — |
| N1 | Error-driven autopilot (fix-the-last-command) | Noteworthy | P1 | M | — |
| N2 | Preview-first agentic runs (plan + diff + apply/undo) | Noteworthy | P1 | L | R1, R2 |
| N3 | Project- and host-aware context | Noteworthy | P2 | M | — |
| N4 | Semantic history search | Noteworthy | P3 | L | R4 (embeddings) |
| N5 | Replayable runbooks | Noteworthy | P3 | M | — |

A natural build order: **R1 → N1 → R5 → R4 → N3 → R3 → R2 → N2 → N5 → N4**. R1 and
N1 are the highest trust-and-stickiness per unit of effort and unlock the rest.

**Status: all ten proposals have shipped a v1** (each released as its own minor
version). The per-proposal "Status" notes below record what landed and which
refinements remain as follow-ups.

---

## R1 — Transactional file edits + `aishe undo`

**Track:** Robustness · **Priority:** P1 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — journaling, diff-on-write, and `aishe undo [--list]` landed (`src/undo.rs`); pre-apply interactive approval and blob dedup remain as follow-ups.

### Problem
In yolo, the model edits files through aishe's *own* built-in tools
(`write_file`/`edit_file` in `src/tools/`). Today those writes hit disk
immediately and irreversibly. The single biggest objection to letting aishe act
autonomously is "what if it clobbers my config?" Because these tools are ours, we
can make every AI file change **previewable and reversible** without any kernel
sandbox.

### Design
1. **Staging layer in `src/tools/`.** Introduce a `ChangeSet` that records, per
   touched path: the pre-image (original bytes or "did not exist") and the
   post-image. `write_file`/`edit_file` compute the post-image and record it in
   the active `ChangeSet` instead of (or in addition to) writing.
2. **Apply with a unified diff.** Before applying, render a colored unified diff
   (reuse the markdown/`syntect` render path already used for model output, or a
   minimal diff renderer). In interactive yolo the user approves per the
   `yolo_confirm` tier; non-interactive (`-c`) applies unless `--dry-run`.
3. **Journal for undo.** On apply, append the `ChangeSet` (pre-images + paths +
   timestamp + the originating request) to a journal at
   `$XDG_DATA_HOME/aishe/undo/<session>.jsonl`. Pre-images for large files are
   stored as content-addressed blobs under `undo/blobs/`.
4. **`aishe undo`** (new `Cmd` in `src/main.rs`): restore the most recent
   `ChangeSet` (or `aishe undo --list` / `aishe undo <id>`). Reverting is just
   writing the pre-images back; a revert is itself journaled so `undo` is
   idempotent and re-doable.
5. **Scope.** v1 covers the built-in file tools only. Arbitrary `run_command`
   side effects are out of scope here (that is R2/N2).

### Config & CLI
- `[aishe] file_tools_preview = true` (default true): show diffs before applying.
- `[aishe] undo_history = 50` (entries kept).
- `aishe undo [--list | <id>]`, `aishe diff` (show the pending/last change set).

### Tests
- **Unit (`src/tools/`):** `ChangeSet` records pre/post correctly for create,
  overwrite, and edit-in-place; applying then reverting yields byte-identical
  originals; reverting a create deletes the file; binary files round-trip.
- **Unit:** journal serialize/deserialize; blob dedup; `undo --list` ordering.
- **Integration (`tests/`):** a yolo run using the **fake provider** (existing
  `providers::fake`) that emits `write_file`/`edit_file` tool calls; assert the
  diff is produced, the file is changed on apply, and `aishe -c` + a follow-up
  `aishe undo` restores it. No network/key needed.
- **Manual:** edit a real config in yolo, `aishe undo`, confirm byte-identical.

### Definition of Done
- Every built-in file-tool write is staged, diffed, and journaled.
- `aishe undo` restores the last AI change set to byte-identical state, is itself
  undoable, and survives a fresh process (journal on disk).
- Default-on preview does not break the existing yolo file-tool tests.
- `docs/` updated (a "Reversible edits" section in `docs/modes.md` + the new
  subcommands in `docs/commands.md`); `cargo fmt`/`clippy -D warnings`/`cargo test`
  green; admin_validation green.

### Open questions
- Concurrent sessions writing the same path — last-writer-wins with a warning, or
  refuse? (Propose: warn + proceed, record both in the journal.)
- Symlinks / files outside the working tree — record realpath; never follow a
  symlink out of tree without an explicit confirm.

---

## R2 — Real sandbox backend for yolo

**Track:** Robustness · **Priority:** P1 · **Effort:** XL · **Risk:** Med · **Depends on:** R1 (diff UX) · **Status: Shipped (v1 — bwrap)** — `sandbox_backend = "bwrap"` runs each yolo `run_command` under bubblewrap with a read-only root + writable working tree (`sandbox::bwrap_wrap_argv`, an executor wrapper-argv prefix; `src/sandbox.rs`, `src/executor.rs`, `src/modes/yolo.rs`). Degrades to policy when bwrap is absent; `doctor` reports it. The **overlay/copy-on-write backend** (dry-run → diff → apply, which N2 builds on) and per-step network approval remain follow-ups.

### Problem
`yolo_sandbox` today is *best-effort policy* (`src/sandbox.rs`): the gate refuses
a `run_command` that looks like it reaches the network or writes outside the tree.
It is advisory — a determined or obfuscated command can still escape (see the
"not a sandbox" note in `docs/safety.md`). For real autonomy on a server we want
commands that **physically cannot** escape, plus the same preview-then-apply story
R1 gives file tools.

### Design
Make `yolo_sandbox` a *backend* selector rather than a boolean:

- `"off"` — current behavior.
- `"policy"` — current best-effort gate (rename of today's `true`).
- `"bwrap"` — run each `run_command` inside `bubblewrap` (`bwrap`) with: a
  read-only bind of `/`, a writable bind of the working tree only, `--unshare-net`
  (unless the step is explicitly network-approved), and a private `/tmp`. Detect
  `bwrap` via `which`; fall back to `policy` with a one-time warning if absent.
- `"overlay"` — run in the working tree but under an `overlayfs`/copy-on-write
  upper dir, so writes land in a scratch layer; after the step, **diff the upper
  layer** (reusing R1's diff/approve/journal) and apply on approval. This is the
  "dry-run on a copy, show me the diff, then commit" UX.

Implementation touches `src/executor.rs::run_captured` (wrap the command spawn in
the chosen backend) and `src/sandbox.rs` (backend selection + capability probe).
Network approval integrates with the `yolo_confirm` tier: a step that needs the
network prompts unless tier allows it.

### Config & CLI
- `[aishe] yolo_sandbox = "off" | "policy" | "bwrap" | "overlay"` (back-compat:
  `true` → `"policy"`, `false` → `"off"`).
- `aishe doctor` reports which backends are available (`bwrap`, kernel overlay).

### Tests
- **Unit:** backend selection + capability probe (mock `which`); back-compat
  bool→enum mapping in `src/config.rs`.
- **Integration (Linux CI):** with `bwrap` present, a `run_command` that tries to
  write `/etc/x` is contained (no change on the host); a network fetch is blocked
  unless approved. Gated behind a `requires bwrap` skip when unavailable (mirrors
  the real-MCP/real-model opt-in pattern).
- **Overlay:** a command writing into the tree produces a diff; approving applies
  it, declining leaves the tree untouched.
- **Manual:** `yolo "clean up build artifacts"` under `overlay` → review diff →
  apply.

### Definition of Done
- `yolo_sandbox` accepts the four modes; bool back-compat preserved and tested.
- Under `bwrap`, out-of-tree writes and un-approved network are physically
  prevented (verified in CI where available).
- Under `overlay`, no host change occurs until the user approves the diff;
  approval routes through the existing confirm tier and R1's journal (so it is
  also undoable).
- Graceful, documented fallback when the backend is unavailable; `doctor` surfaces
  it. Docs in `docs/safety.md`. Full gate green.

### Open questions
- macOS has no `bwrap`/overlayfs — document as Linux-only for the hard backends;
  `policy` remains the cross-platform default. (Sandbox-exec is a possible future
  macOS backend.)

---

## R3 — Parse-based safety gate

**Track:** Robustness · **Priority:** P2 · **Effort:** L · **Risk:** Med · **Depends on:** — · **Status: Shipped (v1)** — `split_segments` is now a quote/escape/paren/backtick-aware tokenizer (no more naive char-split), subshells are unwrapped (`unwrap_subshell`), and the existing path-aware predicates + substitution recursion run on the structurally-extracted commands (`src/safety.rs`). Fixes quoted-operator false positives and subshell evasions with zero corpus regression. A full shell-grammar AST (handling here-docs, process substitution `<()`) remains a future refinement.

### Problem
`src/safety.rs` is regex + path-aware matching over segments split only on
`; && || |` (`split_segments`). We have already had to patch evasions (command
substitution, alternative interpreters). It is whack-a-mole: each new obfuscation
needs a new pattern. A gate that *parses* the line is principled instead of
reactive.

### Design
1. Introduce a small, dependency-light shell tokenizer/AST (or reuse a vetted
   crate; evaluate `yash-syntax`/`conch-parser` — only if it doesn't bloat the
   single-binary footprint, else hand-roll a tokenizer that understands quoting,
   `$( )`, backticks, redirections, and `K=V` prefixes).
2. Walk the AST: assess **every** command node (including those inside
   substitutions, here-strings, and redirection targets) against the existing
   risk predicates (`rm_recursive_force_risk`, `move_out_of_tree_risk`,
   `recursive_perms_risk`, device/`/proc`/`/sys` writes). The regex `PATTERNS`
   become node-level checks.
3. Keep `assess(&str) -> Risk` as the stable public API so callers
   (`modes::suggest`, `modes::yolo`, `dispatcher`) are unchanged.
4. Share the parser with `src/dispatcher.rs` where useful (the dispatcher's
   `split_top_level` has similar needs) to avoid two half-parsers.

### Config & CLI
- None (internal). Optional `aishe explain "<cmd>"` (see R5/N1 tie-in) to print
  the parse + the gate verdict for auditing.

### Tests
- **Unit:** the entire existing `tests/safety_corpus.rs` adversarial corpus must
  stay green; add the evasion cases (nested `$()`, `eval`-of-string, redirection
  to `/dev/sd*` via a variable) now caught structurally.
- **Property/fuzz:** extend `tests/pty_fuzz.py` or add a Rust property test that
  random safe commands never flip to Dangerous (no new false positives) and known
  dangerous templates are always caught.
- **Differential:** for a corpus of real commands, assert the new gate's verdict
  never disagrees with the old gate *in the dangerous direction* on inputs the old
  gate already caught.

### Definition of Done
- `assess` is AST-backed; the adversarial corpus + new evasion cases all pass;
  zero regressions in the 300+ command-diff shell suite (no new false positives).
- Binary size delta documented and acceptable (single-binary principle).
- Full gate green.

### Open questions
- Crate vs hand-rolled: decide via a size/maintenance spike. A hand-rolled
  tokenizer scoped to "what the gate needs" is likely smaller and sufficient.

---

## R4 — Provider fallback chain + offline

**Track:** Robustness · **Priority:** P2 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — `provider_fallback` builds a `FallbackProvider` (`src/providers/fallback.rs`) that advances on terminal error, folds usage into one meter, and announces once; `doctor` shows the chain. The **reachability probe** shipped in 0.2.14: `aishe doctor --probe` sends a short read-only `GET /v1/models` to each chain member and reports reachable / key-rejected / unreachable (`providers::probe`), making the offline/fallback story verifiable. **Live-streaming across the chain** remains a follow-up.

### Problem
`providers::make(&config)` returns a single provider. A dead endpoint, a 5xx
storm, or a blown `budget_usd` makes the AI features fail outright. Pairs poorly
with the no-hang work already shipped — we should *degrade*, not fail.

### Design
1. Config gains an ordered fallback list, e.g.
   `[aishe] provider_fallback = ["openai", "ollama"]`, or a richer
   `[[providers.chain]]` array. `make` returns a `FallbackProvider` that wraps the
   ordered list.
2. `FallbackProvider` tries the primary; on a *terminal* failure (connect refused,
   repeated 5xx after the existing retry/backoff, budget exceeded, or the
   SIGALRM-style timeout from the hook budget) it advances to the next provider,
   emitting a dim one-line notice.
3. First-class **local/offline**: document and smooth the Ollama path
   (`base_url = http://localhost:11434`), and have `aishe doctor` report local
   model reachability so "offline-capable" is a real, testable claim.
4. Streaming and tool-use must be supported across the chain (some local models
   lack tool-use — degrade to suggest-only with a notice).

### Config & CLI
- `[aishe] provider_fallback = ["..."]` (empty = today's single-provider behavior).
- `aishe doctor` shows each chain member's reachability + tool-use capability.

### Tests
- **Unit:** `FallbackProvider` advances on terminal error, not on a successful
  call; preserves usage metering across members; respects budget.
- **Integration:** a fake primary that always errors + a fake secondary that
  succeeds → the call succeeds via the secondary and the notice is emitted
  (deterministic, no network).
- **Manual:** kill the network mid-session; confirm fallback to a local model.

### Definition of Done
- A failing primary transparently falls back; usage/cost still accounted; a clear
  notice is shown once per fallback. Single-provider configs behave exactly as
  before. `doctor` reports the chain. Docs in `docs/providers.md`. Gate green.

---

## R5 — Queryable audit log + cost observability

**Track:** Robustness · **Priority:** P2 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — `aishe log` (filters: session/action/model/since/-n/--json) and `aishe usage` (`--by model|day|session`) read the audit log read-only via `audit::read_entries` (`src/audit.rs`); cost reuses the `usage` price table. The **compact post-session summary line** shipped in 0.2.13: the PTY children append metered usage to a shared per-session tally (`AISHE_USAGE_FILE`) and the parent prints a one-line `aishe session: … reqs · ~$…` aggregate (per-model cost) on exit (`src/usagelog.rs`, `src/pty.rs`).

### Problem
`src/audit.rs` already writes a JSONL log of prompts, responses, and AI-initiated
actions, and `src/usage.rs` meters tokens/cost — but they are files, not features.
Sysadmins want "what did the AI do on this box, and what did it cost."

### Design
1. **`aishe log`** subcommand (new `Cmd` in `src/main.rs`): tail/filter the audit
   JSONL — by session, time range, action type (`command`/`file`/`tool`), or
   model. `--since`, `--session`, `--action`, `--json` flags. Pretty table by
   default.
2. **`aishe usage --history`**: aggregate cost from the audit log (per day / per
   model / per session), not just the live in-process meter.
3. **Redaction-aware:** the log already supports `redact`; `aishe log` must never
   un-redact. Add a one-line integrity note (append-only, no edit path).
4. Optional: a compact post-session summary line ("this session: 4 model calls,
   1,820 tok, $0.012, 2 commands run, 1 file changed").

### Config & CLI
- `aishe log [--since <when>] [--session <id>] [--action <kind>] [--json]`
- `aishe usage --history [--by day|model|session]`

### Tests
- **Unit:** log filtering predicates (time/session/action) over a synthetic JSONL
  fixture; cost aggregation math (reuse `usage.rs` tests).
- **Integration:** run `aishe -c` actions with audit enabled to a temp
  `AISHE_LOG_FILE`, then `aishe log`/`aishe usage --history` and assert the
  rendered rows; assert redaction holds (a planted secret never appears).
- **Manual:** a day of usage → `aishe usage --history --by model`.

### Definition of Done
- `aishe log` and `aishe usage --history` read the existing audit/meter data with
  filtering; redaction is preserved and tested; no new write path to the log from
  these read commands. Docs in `docs/logging.md`. Gate green.

---

## N1 — Error-driven autopilot (fix-the-last-command)

**Track:** Noteworthy · **Priority:** P1 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — Ctrl-X Ctrl-F fix key (zsh + bash) with last-command/exit capture and an opt-in `AISHE_AUTODIAGNOSE` hint, reusing `--suggest-line` + the hook budget (`src/integration.rs`). 0.2.18 added **stderr-tail context** (`fix_capture_stderr`): the fix key re-runs a read-only, safe failed command once to capture its real error output for the correction prompt (`src/fix.rs`, a tested `--fix-line` hook helper); destructive/network commands are never re-run. No remaining follow-ups.

### Problem
The stickiest daily-driver loop is not "translate English → command," it is *"my
command just failed — fix it."* aishe already supports typing `?` after a failure
to diagnose, but it is manual and easy to forget. Make failure recovery a
first-class, one-keystroke flow.

### Design
1. In `src/integration.rs`, the `aishe_precmd` hook already runs in the main shell
   before each prompt and has access to `$?`. On a non-zero exit (and opt-in),
   capture the failed command (from `$AISHE_LAST_CMD`, set in a `preexec` hook)
   plus a bounded tail of its stderr (best-effort, via a captured fd or a
   `fc`-based recall) and stage it.
2. A keybinding (default `Ctrl-X Ctrl-F`, override `AISHE_FIX_KEY`) calls a new
   hook helper `aishe --fix-line "<cmd>" --exit <n>` that asks the model for a
   corrected command **plus a one-line why**, and `print -z`s the fix for
   review/edit (never auto-runs — same safety posture as suggest).
3. Optional ambient mode `[aishe] autodiagnose = true`: after a failure, print a
   dim one-line hint ("aishe: press Ctrl-X Ctrl-F to fix") without spending tokens
   until the user asks.
4. Reuse the suggest path + the SIGALRM hook budget so a slow model never freezes
   the prompt.

### Config & CLI
- `[aishe] autodiagnose = false` (hint only; opt-in), `AISHE_FIX_KEY`.
- Hidden `--fix-line` hook entry point in `src/main.rs` (mirrors `--suggest-line`).

### Tests
- **Unit:** `--fix-line` prompt construction (failed cmd + exit code + stderr tail)
  via the fake provider returns a corrected command; the safety gate still applies
  to the fix.
- **PTY integration (`tests/`):** extend `tests/pty_scenarios.py` — run a failing
  command, trigger the fix key, assert the corrected command is pre-filled (uses
  the deterministic fake-provider PTY harness, no key).
- **Manual:** `gti status` → fix key → `git status` pre-filled with a reason.

### Definition of Done
- A failed command can be fixed in one keystroke; the fix is pre-filled for review,
  passes the safety gate, never auto-runs, and never hangs the prompt. Opt-in
  ambient hint works. Docs in `docs/front-ends.md` + `docs/shell-integration.md`.
  Gate + PTY suites green.

---

## N2 — Preview-first agentic runs (plan + diff + apply/undo)

**Track:** Noteworthy · **Priority:** P1 · **Effort:** L · **Risk:** Med · **Depends on:** R1, R2 · **Status: Shipped (v1 — preview-first file edits)** — `yolo_preview = true` makes the built-in `write_file`/`edit_file` tools show the unified diff and ask `apply this write/edit to <path>? [y/N]` *before* touching the file (`confirm_apply` in `src/tools.rs`; threaded from `src/modes/yolo.rs`), composing R1's `unified_diff`. Declining leaves the file untouched; applied changes are journaled for `aishe undo`. Non-interactive `-c` runs apply automatically, consistent with the rest of the confirm UX. The **editable plan checklist** and **overlay dry-run preview of arbitrary `run_command` steps** (which builds on R2's overlay/copy-on-write backend) remain follow-ups.

### Problem
The headline demo: type a goal → see the **exact commands and predicted file
diffs** → approve → apply → undo. No shell-AI does reversible, previewable
autonomy well. This composes R1 (file diffs/undo) and R2 (overlay dry-run for
arbitrary commands) into one UX.

### Design
1. Extend the existing `yolo_plan` (plan-first) flow in `src/modes/yolo.rs`: render
   the plan as an **editable checklist** (accept / skip / edit per step), not just
   a yes/no.
2. For each step, run it under R2's `overlay` backend (or stage file-tool edits via
   R1) to compute its effect *without committing*, then present a combined preview:
   commands + unified file diffs + "touches network: yes/no."
3. On approval, apply the batch (R1 journal records it); `aishe undo` reverts the
   whole batch. Decline leaves the host untouched.
4. Per-step `[y]/[n]/[e]/[a]ll` controls reuse the safety-gate confirm UX.

### Config & CLI
- `[aishe] yolo_preview = true` (when sandbox/overlay available).
- Reuses `aishe undo` from R1.

### Tests
- **Integration:** fake-provider yolo run emitting a 2-step plan (one file edit,
  one shell command) → assert the preview shows both, applying changes the tree,
  declining does not, and `undo` reverts the batch. Linux-gated for the overlay
  path.
- **Unit:** plan checklist edit/skip semantics.
- **Manual:** "set up a Python venv and install deps" → review the plan + diffs →
  apply → undo.

### Definition of Done
- yolo can run plan→preview→approve→apply→undo end-to-end with file diffs and
  per-step network disclosure; nothing touches the host before approval; the whole
  batch is undoable. Docs in `docs/modes.md`. Gate green (overlay tests gated on
  Linux/`bwrap`).

---

## N3 — Project- and host-aware context

**Track:** Noteworthy · **Priority:** P2 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — `context::build` now adds a project-tasks block (just/make/npm/composer/compose/cargo/python/CI) and a cached host-tools line; `project_tasks`/`host_profile` config toggles; `aishe context` previews the block (`src/context.rs`). Shipped in 0.2.15: **walking up to the project root** (`find_project_root`/`project_tasks_rooted` — task discovery resolves from a subdirectory, nearest task surface wins) and **richer host facts** (`host_facts`: init system + active k8s context). No remaining follow-ups.

### Problem
`src/context.rs` already sends cwd, a dir listing, and recent history. Suggestion
*correctness* is what separates a tool from a toy: "run the tests" should resolve
to *this* repo's actual command, and suggestions should use tools that are
actually installed on *this* host.

### Design
1. **Project signals:** detect and summarize the repo's task surface — `justfile`,
   `Makefile` targets, `package.json` scripts, `composer.json`, `Cargo.toml`,
   `pyproject.toml`, `docker-compose.yaml` services, `.github/workflows`. Add a
   compact, capped "project tasks" block to the context (extends the existing
   `.aishe/context.md` mechanism and the context block builder).
2. **Host signals:** a cheap, cached capability probe — package manager
   (`apt`/`dnf`/`apk`/`brew`/`pacman`), container runtime (`docker`/`podman`),
   `kubectl` context, init system — so the model proposes commands that exist here.
   Reuse the `CommandCache` (`src/dispatcher.rs`) to know what's on `$PATH`.
3. All of it is redaction-aware (`redact_secrets`) and size-capped; respects
   `project_context`.

### Config & CLI
- `[aishe] project_tasks = true`, `[aishe] host_profile = true` (both default on,
  cheap, cached).
- `aishe context` (debug): print the assembled context block (redacted).

### Tests
- **Unit:** parsers for `justfile`/`Makefile`/`package.json` task extraction
  (fixtures); host-profile probe with a mocked `PATH`/`which`.
- **Integration:** in a fixture repo with a `justfile`, a suggest call (fake
  provider asserting the prompt contains the task list) resolves "run the tests" to
  the `just test` target.
- **Manual:** same request in a Make-based vs npm-based repo yields the right
  command.

### Definition of Done
- The context block includes capped, redacted project tasks + host capabilities;
  cost (latency/size) stays within the "never block the prompt" budget (cached,
  off-hot-path); `aishe context` shows it. Docs in `docs/project-context.md`. Gate
  green.

---

## N4 — Semantic history search

**Track:** Noteworthy · **Priority:** P3 · **Effort:** L · **Risk:** Med · **Depends on:** R4 (embeddings) · **Status: Shipped (v1)** — An `embed` capability was added to the provider trait (OpenAI-compatible `/v1/embeddings` impl; a deterministic bag-of-words fake for tests). `aishe history index` embeds history-log commands into a capped, rebuildable on-disk vector store (`src/semhist.rs`, `history.vec`) — incrementally by default, `--rebuild` from scratch — and `aishe history search "<q>"` returns cosine-top-k matches. Fully offline-capable via a local Ollama `embedding_provider`; opt-in (`semantic_history`) and silent when off (no embedding without an explicit index). The **interactive pre-fill key binding** shipped in 0.2.12: a `Ctrl-X Ctrl-R` ZLE widget (`AISHE_RECALL_KEY`, `src/integration.rs`) takes the current line as the query and pre-fills the closest past command via `aishe history search --bare`. 0.2.16 fixed the **data source**: the interactive PTY now records each command to aishe's history log via a `preexec` hook (`AISHE_HISTFILE`) and the executor persists `-c`/hook commands, so indexing has real history (previously the log was only ever read, never written). 0.2.17 added **auto-indexing on exit** (`semantic_history_autoindex`, opt-in) via a shared, tested indexing core (`src/index.rs`), so the store stays fresh without a manual `aishe history index`. **No remaining follow-ups** — N4 is complete.

### Problem
aishe keeps a timestamped history (`src/histlog.rs`, zsh `EXTENDED_HISTORY`
format). "That docker run with the volume mount from last week" is a natural,
sticky query that normal `Ctrl-R` can't answer. Natural-language history search is
a genuinely novel feature.

### Design
1. Add an embeddings capability to the provider trait (Anthropic/OpenAI both expose
   embeddings; local via Ollama). Embed history entries incrementally into a small
   on-disk vector store at `$XDG_DATA_HOME/aishe/history.vec` (append-only, capped,
   rebuildable).
2. **`?`-history search:** a query like `? the docker run with the prometheus
   volume` (or a dedicated key / `aishe history search "<q>"`) returns the top-k
   semantically-closest past commands, pre-filled for recall.
3. Fully local-capable (Ollama embeddings) so privacy-conscious users keep it
   offline; opt-in because it sends history to the embedder.

### Config & CLI
- `[aishe] semantic_history = false` (opt-in; names the embedding model).
- `aishe history search "<query>"`, plus an interactive key binding.

### Tests
- **Unit:** vector store append/query/cap/rebuild; cosine top-k correctness with a
  deterministic fake embedder.
- **Integration:** seed a history, query with the fake embedder, assert the
  expected command ranks first; assert nothing is embedded when the feature is off.
- **Manual:** real embeddings, fuzzy recall of a real past command.

### Definition of Done
- Opt-in semantic search over local history returns relevant past commands and
  pre-fills them; works fully offline with a local embedder; the store is capped
  and rebuildable; off by default and silent when off. Docs in
  `docs/shell-integration.md`. Gate green.

### Open questions
- Index freshness vs cost — embed lazily on idle (a background, bounded task), not
  on the hot path.

---

## N5 — Replayable runbooks

**Track:** Noteworthy · **Priority:** P3 · **Effort:** M · **Risk:** Low · **Depends on:** — · **Status: Shipped (v1)** — `aishe runbook` renders a session (from the audit log) to a runnable `.sh` + a markdown `.md`; `--replay` re-runs recorded commands through the safety gate (not the model). Built on R5's `audit::read_entries` (`src/main.rs`, `docs/runbooks.md`).

### Problem
A successful yolo session ("set up nginx with TLS") is throwaway today. Turning it
into a committable, auditable **script + markdown runbook** makes aishe an *ops
artifact generator* — a story ops teams repeat and share.

### Design
1. yolo already runs a sequence of approved commands/tool calls (`src/modes/yolo.rs`)
   and everything is in the audit log (`src/audit.rs`). Add a recorder that, on
   request, emits two artifacts for a session:
   - `runbook-<ts>.sh` — the exact approved commands, in order, with a header
     comment and the original request.
   - `runbook-<ts>.md` — a human narrative: the goal, each step with its rationale
     and result, and a "to reproduce" section.
2. **`aishe runbook [--last | --session <id>] [-o <dir>]`** generates them from the
   audit log (so it works even after the fact). Secrets are redaction-aware.
3. Optional: a `--replay` that re-runs the script through the safety gate (not the
   model) for deterministic reproduction.

### Config & CLI
- `aishe runbook [--last | --session <id>] [-o DIR] [--replay]`

### Tests
- **Unit:** runbook generation from a synthetic audit JSONL fixture → expected
  `.sh` ordering and `.md` structure; redaction preserved.
- **Integration:** a fake-provider yolo session → `aishe runbook --last` → assert
  the script reproduces the commands; `--replay` runs them through the gate.
- **Manual:** a real multi-step setup → commit the generated runbook.

### Definition of Done
- Any yolo session can be exported to a runnable script + a readable runbook from
  the audit log; secrets stay redacted; `--replay` reproduces deterministically via
  the gate (no model). Docs: a new `docs/runbooks.md` + index link. Gate green.

---

## Cross-cutting requirements (apply to every proposal)

- **Single binary, no services** — prefer std + existing deps; justify any new
  crate against binary size.
- **Never block the prompt** — anything touching the network or a slow subprocess
  runs off the hot path or under a timeout (the SIGALRM hook budget pattern).
- **Default-safe** — new powers (sandbox backends, autorun, embeddings) are opt-in
  or confirmation-gated; the model never decides what executes.
- **Tested as we go** — unit tests in-module, deterministic integration via the
  `fake` provider and the PTY harness, and a `test-results/` report where a suite
  is involved. `cargo fmt` + `clippy -D warnings` + `cargo test` + the PTY suites +
  `admin_validation` stay green.
- **Documented** — every feature ships with a docs page/section and a config
  reference entry.
