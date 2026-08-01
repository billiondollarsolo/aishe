> **Lifecycle: Active.** Baseline: AIShe v0.6.5 (`4a2c7e4`). The implementation
> and clean deterministic macOS/Linux candidate qualification are complete at
> functional commit `35297d0`; ONB-001's actual new-user observation and the
> separate hosted-CI/paid-live release disposition remain open. See the
> [implementation report](NEXT_PRODUCT_UX_RELIABILITY_IMPLEMENTATION_REPORT.md).

[PRD]

# AIShe next product, UX, routing, compatibility, and reliability plan

| Field | Value |
| --- | --- |
| Status | Implemented and clean deterministic candidate-qualified; ONB-001 research and external release disposition remain open |
| Baseline | AIShe 0.6.5, commit 4a2c7e4 |
| Audit date | 2026-07-31, America/New_York |
| Scope | Post-0.6.5 product quality, UX, routing, terminal compatibility, maintainability, and release confidence |
| Owners | AIShe maintainers |
| Planning style | Small reviewable stories, explicit dependencies, automated acceptance evidence |
| Product constraints | Preserve the real shell, shell-first safety, lazy backend startup, local privacy, and pre-1.0 compatibility |

## 1. Document authority

This is the implementation plan for work after 0.6.5. It is intentionally not
another historical design essay.

- For future UX, routing, command-surface, compatibility, accessibility, and
  maintainability work, this document is authoritative.
- SECURITY.md remains authoritative for the security and threat model.
- docs/architecture.md should describe the implementation that exists, not a
  desired future state.
- The managed OpenCode design and validation documents remain authoritative for
  the backend protocol and its original qualification evidence.
- Older plans under docs/design are historical unless a banner says that they
  are still active. Unchecked boxes in a historical plan are not automatically
  product requirements.
- If implementation changes a decision here, update this document in the same
  pull request with the decision, evidence, and migration effect.

The first implementation epic creates a design-document index and labels every
older plan as active, implemented, superseded, or historical. That prevents the
current situation in which a v0.2 validation list, a v0.5 backend plan, a v0.6.4
brand plan, and current source can all appear to be product truth.

## 2. Executive verdict

AIShe already has an unusually strong foundation. It runs a genuine zsh in a
PTY, keeps direct commands service-free, has transactional setup and settings,
uses a pinned private agent runtime, isolates Linux workspace actions with
bubblewrap, retains durable sessions, redacts secrets, offers recovery and undo,
and ships with broad deterministic testing and supply-chain checks.

The next milestone should not replace the shell, replace the backend, or add a
large full-screen TUI. The highest-return work is to make the existing system
coherent and unmistakable:

1. Use one declarative command registry instead of conflicting Rust, zsh, bash,
   help, docs, and test lists.
2. Make routing explainable and identical at submit time across supported
   surfaces while preserving a zero-service fast path for real commands.
3. Give user input, shell output, agent activity, proposed commands, approvals,
   and final agent answers distinct semantic presentation.
4. Fix the filter picker so selection can never become invisible and printable
   characters always behave as filter text.
5. Centralize terminal capability, color, theme, Unicode, width, and motion
   policy. Today NO_COLOR and TERM=dumb are honored by some renderers but not
   every direct styling call.
6. Define honest compatibility tiers. The zsh PTY is the flagship experience;
   the zsh hook is close to it; the bash hook is materially different; fish and
   Windows are not supported.
7. Refactor the 5,800-line binary driver and embedded shell scripts only after
   characterization tests lock behavior.
8. Version every machine-readable contract and make the exact release binary,
   terminal matrix, and corpus version part of validation evidence.

The plan is ordered so correctness and product truth land before visual polish,
and characterization lands before refactoring.

## 3. Audit scope and evidence

### 3.1 What was reviewed

The review covered:

- CLI and command topology
- interactive zsh PTY and native zsh/bash hooks
- dispatcher and command cache
- suggest, auto, and yolo interaction
- agent event normalization and rendering
- setup, settings, tour, help, status, doctor, and support bundles
- named connections, model selection, OAuth/API-key boundaries
- configuration, project overlays, trust, credentials, redaction, and audit
- safety matching, approvals, scopes, network policy, sandbox, undo, and dry-run
- managed runtime, supervisor, bridge, session recovery, and output bounding
- providers, MCP, skills, custom commands, semantic history, and automation
- terminal colors, Markdown, syntax highlighting, widths, keys, and non-TTY use
- install, upgrade, packaging, release, dependency, and supply-chain workflows
- unit, integration, PTY, installer, runtime-contract, soak, fuzz, and live tests
- root documentation and all planning/design documents

At the audited baseline the repository contained roughly 61,800 lines of Rust,
8,800 lines of Python tests, and 14,600 lines of Markdown. The size is not itself
a defect, but it makes source-of-truth discipline essential.

### 3.2 Commands and observed results

All evidence below was collected from commit 4a2c7e4 after rebuilding the
release binary as AIShe 0.6.5.

| Gate | Result | Notes |
| --- | --- | --- |
| cargo fmt --all -- --check | Pass | No formatting drift |
| cargo test --all-targets --locked | Pass | 587 observed Rust test cases across all targets |
| cargo clippy --all-targets --all-features --locked -- -D warnings | Pass | Current stable toolchain |
| cargo deny check advisories bans licenses sources | Pass with duplicate-version warnings | Documented advisory exceptions remain |
| cargo build --release --locked | Pass | Rebuilt exact 0.6.5 binary |
| PTY scenario harness | Pass | 50/50 scenarios |
| model/connection picker PTY | Pass | Selection, rollback, defaults, concurrency |
| statusline PTY | Pass | 180/60/42 columns and prompt-injection safety |
| setup/settings/tour PTY | Pass | 40/80/120/200 columns, NO_COLOR, resume, rollback, secret handling |
| direct-shell benchmark | Pass | raw zsh p95 3.404 ms; AIShe p95 8.264 ms; 4.860 ms regression versus 10 ms SLO |
| deterministic admin validation | 455/456 | 318/318 shell pass-through and 46/46 route cases pass; one stale test expects old product spelling |
| shellcheck install.sh tests/*.sh | Findings | Two intentional-looking SC2016 infos and two SC1007 warnings need disposition |

The deterministic admin run found a test-governance defect, not a runtime
failure: the source correctly says “AIShe” while tests/admin_validation.py still
expects “Aishe.” The same corpus still treats removed legacy commands such as
/ghost, /plan, /cache, and /sandbox as valid merely because they return zero
without reaching a model. That is exactly why command and test registries need a
single source of truth.

### 3.3 What was not claimed

This review did not make paid provider calls, perform a literal 24-hour soak,
qualify Linux bubblewrap on the current macOS host, test every terminal emulator,
or add Windows support. Existing deterministic loopback and CI evidence was
reviewed, but those omissions remain explicit release gates where relevant.

### 3.4 External UX lens

The terminal audit also applied the relevant principles from the current
[Vercel Web Interface Guidelines](https://raw.githubusercontent.com/vercel-labs/web-interface-guidelines/main/command.md):
keyboard discoverability, visible focus, clear asynchronous state, robust long
content, non-color state cues, actionable errors, and careful destructive
actions. Browser-specific rules were not copied into a terminal product.

## 4. Product principles that must survive

### P-01: Preserve the real shell

Do not build a partial shell or replace zsh editing, history, completion, job
control, plugins, aliases, functions, signals, or terminal applications.

### P-02: Preserve the direct-command fast path

A recognized shell line must not start the managed backend, perform a provider
request, load extension servers, or add a blocking subprocess to each keystroke.
The current p95 regression budget remains at most 10 ms.

### P-03: Route deterministically and locally

The model must not decide whether input is a shell command. Routing must be
local, bounded, inspectable, and testable. A route score can inform UI but never
authorize execution.

### P-04: Prefer shell on unresolved command collisions

If a real executable, builtin, alias, or function is the effective command head
and the local grammar cannot prove a natural-language question, keep shell
semantics. This avoids silently sending or executing something different from
what an experienced shell user typed. Make the ? force-agent affordance obvious.

### P-05: Do not use color as the only state

Color may accelerate recognition, but words, glyph shape, spacing, and focus
markers must keep every state legible in monochrome and redirected output.

### P-06: Agent output must be visibly authored

Shell output stays untouched. Agent activity and answers receive an AIShe label
or boundary that does not contaminate copied commands or code blocks.

### P-07: Capabilities over platform marketing

Report the effective sandbox, shell integration, terminal, and backend behavior.
Do not call macOS policy checks a sandbox or call bash behavior identical to zsh
until tests prove it.

### P-08: Local privacy remains the default

No behavioral telemetry or phone-home analytics are introduced by this plan.
Product metrics are local test metrics or opt-in research measurements with
explicit consent.

### P-09: Refactor by seam, not by rewrite

Characterize behavior, extract one seam, keep tests green, and ship. Do not mix
large source moves with new routing or safety policy.

### P-10: Every persistent or scriptable shape is versioned

Config, JSON, journals, caches, and external command output need explicit schema
or compatibility policy before 1.0.

## 5. Current architecture and main seams

~~~text
typed line
  |
  +-- zsh PTY / zsh hook -------- local shell-script route and highlighting
  |                                  |
  |                                  +-- native zsh execution
  |                                  +-- AIShe hidden hook command
  |
  +-- bash hook ------------------ command-not-found route with reduced parity
  |
  +-- -c / piped input ----------- Rust dispatcher
                                     |
                                     +-- direct shell / builtin
                                     +-- custom command / MCP prompt
                                     +-- managed agent turn
                                            |
                                            +-- normalized AgentEvent stream
                                            +-- foreground authorization/tools
                                            +-- semantic renderer
~~~

The intended seam is clear, but product rules are duplicated across it:

- src/dispatcher.rs classifies one-shot input and carries a legacy meta list.
- src/integration.rs contains separate zsh question grammar, zsh slash cases,
  bash slash cases, highlights, handoffs, and branded prompt strings.
- src/main.rs parses the CLI, dispatches many commands, renders many errors,
  manages connections and sessions, and implements one-shot meta handling.
- src/product_help.rs, docs/commands.md, README.md, tests, completions, and old
  plans each carry another command or behavior list.
- src/promptui.rs, src/agent/renderer.rs, src/modes, src/main.rs, src/tools.rs,
  and embedded shell code each make independent styling decisions.

The target architecture makes route, command, and terminal semantics reusable
library contracts, while the PTY remains a thin byte proxy around a genuine zsh.

## 6. Personas and critical journeys

### Persona A: shell expert

Wants native zsh behavior, no surprise network work, copyable output, fast
startup, and an explicit override when natural language collides with a command.

### Persona B: developer learning terminal workflows

Wants plain English to work, proposed commands to be clearly separated and
editable, errors to explain recovery, and dangerous actions to be unmistakable.

### Persona C: autonomous operator

Wants clear scope/network/sandbox state, durable progress, concise tool activity,
reliable approvals, auditability, cost visibility, and interruption recovery.

### Persona D: automation author

Wants stable JSON, exact exit codes, no ANSI, clean stdout/stderr separation,
bounded execution, and backward-compatible schemas.

### Persona E: administrator or security reviewer

Wants effective-policy provenance, narrow credentials, sandbox truth, dependency
evidence, support bundles without secrets, and reproducible release gates.

### Critical journeys

1. Install, authenticate, configure, verify, take a tour, and start a shell.
2. Type a real command and get exactly native behavior with no backend startup.
3. Type a natural-language question and recognize its route before submission.
4. Resolve a command-name collision using an obvious local override.
5. Review, edit, run, or cancel a proposed command without losing shell editing.
6. Watch an agent work, answer a question, approve a risky action, and identify
   the final answer in scrollback.
7. Switch account/model for one shell or promote it deliberately to default.
8. Recover from a failed provider, backend, command, or interrupted session.
9. Use the product through NO_COLOR, TERM=dumb, a pipe, SSH, tmux, and narrow or
   wide terminals.
10. Consume JSON across an upgrade without silently changing its meaning.

## 7. Findings and severity

Severity describes product risk, not blame:

- P0: fix before building more behavior on the affected surface.
- P1: high-value milestone work.
- P2: important hardening or expansion after the core milestone.
- Research: investigate and decide; do not promise the feature yet.

### 7.1 P0 findings

#### F-001: picker selection can move off-screen

Evidence: picker_frame_lines in src/promptui.rs renders only matches.take(20),
while Up/Down wraps over matches.len(). With more than 20 models, selected can
refer to an undisplayed row; Enter then chooses an invisible option.

Impact: a user can select the wrong model or connection with no visible focus.

Required outcome: a viewport follows selection, shows absolute position and
match count, and never accepts a hidden selection.

#### F-002: slash-command truth is split and internally inconsistent

Evidence:

- dispatcher::is_meta_subcommand lists removed reedline-era commands such as
  editor, frontend, stream, structured, theme, rehash, ghost, plan, sandbox,
  and cache.
- It omits current primary commands including connection, auth, scope, and
  network.
- zsh and bash hooks contain different hand-written case lists.
- one_shot supports another subset.
- docs/commands.md still calls obsolete entries prompt-only commands.
- the deterministic admin suite treats obsolete entries as a pass if they print
  “interactive-only.”

Impact: custom slash-command shadowing, help, highlighting, one-shot behavior,
and shell behavior can disagree. A removed command can silently become a model
prompt.

Required outcome: one declarative registry defines availability, aliasing,
arguments, side effects, shell-local handoff, help, and tests.

#### F-003: routing logic is duplicated across Rust and zsh, while bash is not equivalent

Evidence: looks_like_question exists as Rust logic and as an embedded zsh
function. Bash primarily sees command-not-found input, so an English phrase
beginning with a real executable does not receive the same full-buffer collision
handling or route highlighting.

Impact: the same line can have different route behavior across -c, zsh, and
bash. Any grammar edit can change highlight without changing submit behavior.

Required outcome: a versioned route decision contract, shared corpus, generated
or conformance-tested shell grammar, and honest compatibility tiers.

#### F-004: NO_COLOR and TERM=dumb do not suppress every ANSI sequence

Evidence: main.rs styles fatal errors directly with crossterm Stylize. An
empirical NO_COLOR=1 run of aishe -c /reset emitted ESC [ m reset sequences.
Several modules use direct Stylize while others have their own capability check.

Impact: logs and scripts receive control bytes despite opting out. Snapshot
tests and exact error matching become fragile.

Required outcome: one terminal capability object and zero escape bytes in
NO_COLOR, TERM=dumb, non-TTY, and JSON modes.

#### F-005: planning and architecture documents contain live-looking stale claims

Examples:

- docs/architecture.md calls src/main.rs thin though it is over 5,800 lines.
- It references config schema v4 while current named connections use schema 6.
- docs/commands.md documents removed prompt commands.
- P1_P2_BRAND_SWEEP_PLAN says ready for a 0.6.4 release even though those changes
  shipped and 0.6.5 is current.
- AUDIT_FIXES_PLAN still presents old safety gaps even though current safety.rs
  and its corpus have moved substantially beyond that baseline.

Impact: maintainers and agents can reimplement removed features, test the wrong
contract, or make decisions from obsolete risks.

Required outcome: document lifecycle banners, an index, current architecture,
and generated/verified command reference.

#### F-006: local qualification can accidentally exercise a stale binary

Evidence: the pre-existing target/release/aishe initially reported 0.6.3 while
the source baseline was 0.6.5. Harnesses accept any path and do not reject a
source/version mismatch.

Impact: a green local PTY result may not validate the checked-out code.

Required outcome: the qualification driver builds first or records and verifies
the source commit/version identity.

### 7.2 P1 findings

#### F-007: final agent answers blend into shell scrollback

The normalized renderer clearly labels tool activity, but final Markdown is
printed without a persistent AIShe authorship boundary. A user returning to a
long terminal transcript may not know which prose came from a command and which
came from the agent.

#### F-008: terminal styling has no semantic design system

Raw ANSI codes, crossterm styles, termimad colors, syntect truecolor, and zsh
prompt colors are spread across many files. There is no shared meaning for
assistant, user route, proposed command, focus, warning, danger, success,
muted, diff, or policy state.

#### F-009: syntax highlighting assumes a dark theme and truecolor

The highlight feature uses base16-ocean.dark and 24-bit ANSI output. Light
terminals, 16-color terminals, and terminals with inaccurate capability
advertising can produce poor contrast.

#### F-010: terminal width calculations count Unicode scalar values, not cells

promptui and streamed-row estimation use chars().count(). Wide CJK characters,
emoji, combining marks, and grapheme clusters can wrap or truncate incorrectly.

#### F-011: picker keys conflict with filtering

At an empty filter, d saves a default and j/k navigate, so a user cannot begin a
search with those characters despite the instruction “type to filter.” The
connection picker also prints controls before filter_picker prints them again.

#### F-012: suggest-mode edit abandons native line editing

After selecting edit, AIShe uses a plain stdin line prompt. Long commands lose
cursor movement, history, completion, and the rest of the real-zsh advantage.

#### F-013: routing is correct-by-policy but not sufficiently explainable

The conservative shell-first rule intentionally routes install kubectl please
to /usr/bin/install. The behavior is documented, but there is no route-explain
command, reason code, ambiguity cue, or correction path for likely typos such as
gti status. Existing fuzzy command correction code is not integrated here.

#### F-014: # as a hidden force-agent prefix conflicts with shell expectations

The product documents ? as primary but also intercepts #. In an interactive
shell, a leading # commonly means a comment. This compatibility decision needs
an explicit deprecation or support policy rather than remaining incidental.

#### F-015: help is task-first but not generated from executable capability

product_help.rs is a useful consolidation, yet it is another manual list. Help,
CLI, slash aliases, shell-local requirements, and examples can still diverge.

#### F-016: error presentation is only partly normalized

Provider errors have stable kinds and AgentEvent has UserFacingError, but the
CLI still has many ad hoc println/eprintln/anyhow paths, raw alternate-format
errors, inconsistent color, and uneven next actions.

#### F-017: compatibility claims are broader than test parity

The flagship zsh PTY is deeply tested on Linux CI and was manually exercised on
macOS in this audit. Bash has generated-script assertions but no equivalent
interactive end-to-end harness. macOS ships Bash 3.2, which deserves an explicit
syntax/runtime gate if advertised.

#### F-018: machine-readable outputs are inconsistently versioned

Setup, doctor, sessions, tasks, and other persisted shapes often carry
schema_version. status --json and suggest --json do not. There is no central
inventory of stdout/stderr, exit-code, nullability, or compatibility promises.

#### F-019: main.rs and integration.rs concentrate unrelated responsibilities

main.rs owns clap declarations plus substantial setup, backend, connection,
session, history, trust, audit, runbook, and rendering logic. integration.rs
contains large embedded scripts and duplicated zsh/bash logic. This increases
review risk and makes small UX changes touch critical orchestration.

#### F-020: long streamed answers can remain visually inconsistent

Short streamed answers are re-rendered as Markdown by moving the cursor. If the
answer exceeds the screen, raw Markdown remains because it cannot be safely
erased. The behavior is defensible but needs a deliberate streaming style and
response boundary rather than appearance depending on response height.

#### F-021: accessibility preferences are implicit rather than configurable

There is no explicit static/screen-reader mode, reduced-motion setting, Unicode
fallback policy, or high-contrast theme. Live status redraw and Unicode glyphs
work well in common terminals but are not a universal interface.

#### F-022: risk state could be clearer during autonomous work

Mode glyphs, status fields, and acceptance panels exist, but agent answers and
tool summaries do not always carry a compact effective scope/network/sandbox
context. On macOS, policy-only autonomous work must never visually resemble a
Linux-isolated workspace turn.

#### F-023: dependency exceptions and duplicate stacks need owned retirement

cargo-deny passes, but the graph includes crossterm 0.28 and 0.29, ureq 2 and 3,
and other duplicate transitive versions. Advisory exceptions for bincode via
syntect assets and the ureq 2 maintenance advisory are well explained but have
no owner, review date, or retirement release.

#### F-024: repository hygiene misses Python cache directories

test-results is ignored, but tests/__pycache__ is currently untracked and
.gitignore does not cover Python bytecode or pytest caches.

### 7.3 P2 and research findings

- Qualify tmux, screen, SSH latency, bracketed paste, common macOS/Linux terminal
  emulators, light/dark themes, C locale, wide text, and very small terminals.
- Decide whether a fish hook is worth building; fish completions do not imply a
  fish AIShe integration.
- Research WSL as a Linux compatibility target. Native Windows remains out of
  scope until PTY, shell, filesystem, credentials, and packaging are designed.
- Add local, privacy-preserving usability measurement fixtures; do not add
  remote telemetry by default.
- Assess log/audit/session retention, rotation, export, and deletion controls.
- Measure default and no-highlight binary size, cold start, memory, backend
  startup, and long-session rendering.
- Consider fuzzy model ranking only after exact substring behavior and stable
  selection are fixed.

## 8. Goals, non-goals, and success metrics

### 8.1 Goals

- A user can identify shell, agent, command proposal, approval, tool activity,
  and final answer without depending on color.
- All supported input surfaces agree on the route for the versioned corpus.
- Every route has a stable reason code and a local explanation path.
- The command surface is defined once and cannot drift across code/docs/tests.
- A picker selection is always visible and usable with 1 to at least 1,000 rows.
- NO_COLOR, TERM=dumb, non-TTY, and JSON output contain no terminal controls.
- zsh remains native and direct-command latency remains inside the current SLO.
- Compatibility claims are tiered, tested, and shown in doctor.
- Machine-readable contracts are inventoried, versioned, and regression-tested.
- High-risk refactors reduce ownership concentration without behavior changes.

### 8.2 Quantitative success metrics

| Metric | Target |
| --- | --- |
| Critical route corpus false-shell rate | 0 |
| Route agreement across Rust, zsh submit, zsh highlight, and supported bash behavior | 100% for the declared tier |
| Direct command backend starts | 0 |
| Direct command p95 overhead versus raw zsh | At most 10 ms |
| Hidden picker selections | 0 |
| ANSI bytes under NO_COLOR, TERM=dumb, non-TTY, or JSON | 0 |
| Unversioned public JSON surfaces | 0 |
| Stale active-looking design documents | 0 |
| Shellcheck warnings in owned scripts | 0, or narrowly annotated with rationale |
| macOS and Linux flagship PTY deterministic pass rate | 100% |
| Bash declared-tier deterministic pass rate | 100% |
| Fatal user errors with a stable code and next action | At least 95%, then 100% before 1.0 |
| Default-theme contrast failures in maintained palette | 0 in recorded manual matrix |

### 8.3 Non-goals

- Replacing zsh with a custom editor or shell parser
- Sending each keystroke or route decision to a model
- Promising perfect intent inference for ambiguous English
- Treating the deterministic safety matcher as a kernel security boundary
- Claiming a macOS sandbox where none exists
- Adding remote analytics by default
- Supporting native Windows in this milestone
- Replacing the managed OpenCode backend
- Rewriting all large modules in one change
- Restyling shell command output owned by external programs

## 9. Product decisions and recommended defaults

### D-001: shell-first remains the collision default

Recommended and assumed. A real executable head stays shell unless explicit
question grammar or ? proves the agent route. This is the least surprising and
least privacy-invasive default for a shell product.

### D-002: expose route reason, not fake confidence

Use deterministic reasons such as forced_agent, forced_shell, slash_command,
shell_syntax, control_structure, assignment, question_grammar, compound_shell,
known_command, or unknown_head. An optional ambiguity flag may be shown, but do
not present a probabilistic percentage as truth.

### D-003: final answers get a compact AIShe label

Recommended default:

~~~text
AIShe · answer
The response begins here...
~~~

The label is semantic and subdued; the answer body uses normal terminal
foreground colors. Code blocks remain directly copyable. A plain output mode
may use “AIShe:” on its own line.

### D-004: remove printable picker shortcuts

Recommended default: every printable character filters. Arrow keys and
Ctrl-P/Ctrl-N move. Enter applies to the shell. If the selection differs from
the durable default, the existing post-selection y/N prompt is the only default
promotion interaction. Remove d and empty-filter j/k shortcuts.

### D-005: theme modes are auto, dark, light, mono, and none

auto selects a conservative palette and terminal color depth. none guarantees
no ANSI. mono may use attributes if safe. Explicit dark/light override unreliable
terminal background detection.

### D-006: static output is an explicit accessibility mode

ui.motion = live or static. Static mode does not erase/repaint live status lines
and emits durable phase lines instead. ui.unicode = auto, always, or ascii.

### D-007: keep # compatible temporarily, decide deprecation explicitly

Recommended: keep it functional through the first post-0.6.5 minor release,
remove it from primary onboarding in favor of ?, add tests for real comment
expectations, then decide a documented two-release deprecation. Do not remove it
silently.

### D-008: no major refactor precedes characterization

The registry, route corpus, renderer snapshots, exact binary identity, and shell
syntax gates land before main.rs or integration.rs moves.

## 10. Target interaction specification

### 10.1 Route states

Each submitted line resolves to RouteDecision:

| Field | Meaning |
| --- | --- |
| kind | shell, natural_language, or builtin |
| normalized | line after an explicit sigil is removed |
| reason | stable deterministic reason code |
| head | effective command head when available |
| known_command | whether the head exists in the current command cache |
| ambiguous | safe UI hint only; never authorization |
| source | rust, generated-zsh, generated-bash, or explicit |

Add a public diagnostic command:

~~~sh
aishe route -- explain this repository
aishe route --json -- 'install kubectl please'
~~~

Text output should say what will happen, why, and how to force the other route.
JSON receives schema_version.

### 10.2 Input presentation

- Known shell input: existing native syntax highlighting wins. AIShe fallback
  may use a shell semantic color but must not recolor strings by first token
  alone.
- Natural-language input: use an agent semantic color plus a non-color route
  cue in the right-side message or prompt state where the shell permits it.
- Forced agent: show “AI” or “agent” as a transient ZLE message after submit.
- Forced shell: show “shell override” once when ! is used because it bypasses
  the AI safety gate.
- Ambiguous known-command phrase: remain shell, but a bounded one-time cue may
  say “shell route; prefix ? for AIShe.” Never prompt on every normal command.
- Route color and submit behavior must be driven by the same conformance corpus.

### 10.3 Agent response anatomy

Focus mode:

~~~text
  working · 3 actions · 1 file
  commands: cargo test | git diff
  ✓ 3 actions · 1 file changed · 8.2s

AIShe · answer
Tests pass. I also fixed...
~~~

Compact mode may retain one line per completed action. Detailed mode may show
streamed tool output. All modes share the final answer boundary.

Rules:

- Do not tint the entire answer.
- Keep labels outside fenced code.
- Do not prefix each answer line; that harms copying.
- Success means the overall outcome, not merely “a tool call returned.”
- Recovered attempts use warning semantics, not green success semantics.
- Approval and user-question panels clear live status before taking the cursor.
- Effective mode/scope/network/sandbox should be visible in risky approval
  panels and available in the final activity summary.

### 10.4 Proposed command anatomy

~~~text
AIShe · suggested command

  rg --files | xargs du -h | sort -h

Find files, measure them, then sort by size.

[Enter] place in shell   [Esc] cancel
~~~

The preferred interactive behavior is to place the command into the real zsh
buffer for native editing and explicit execution. If a separate confirmation is
needed outside a hook, use a full line editor only if it does not duplicate the
shell; otherwise print the command and offer a copy/stage handoff.

### 10.5 Picker anatomy

~~~text
Select a model
filter: sonnet
3 matches · 2/3

    claude-3-5-sonnet
  > claude-3-7-sonnet
    claude-sonnet-4

↑/↓ move · type to filter · Enter use in this shell · Esc cancel
~~~

Behavior:

- viewport height is based on terminal rows with a safe maximum
- selected row is always inside the viewport
- count and absolute position are visible
- PageUp/PageDown/Home/End work
- resize recomputes width and viewport by the next draw
- empty results keep filter editing active
- all printable Unicode can enter the filter
- display width uses terminal cells and never splits a grapheme
- a plain/static fallback prints numbered pages and accepts a number

### 10.6 Terminal design tokens

Use semantic tokens rather than literal cyan/green/red at call sites:

- accent
- focus
- user_shell
- user_agent
- assistant_label
- activity
- proposed_command
- success
- warning
- danger
- muted
- policy
- diff_add
- diff_remove
- code_label

Every token has text/glyph behavior, not just a color. Rendering resolves tokens
through TerminalCapabilities:

- is_tty
- term
- no_color
- color_depth: none, ansi16, ansi256, truecolor
- background: unknown, dark, light
- unicode: ascii or unicode
- motion: static or live
- columns and rows

### 10.7 Errors

Every user-facing error should be representable as:

| Field | Purpose |
| --- | --- |
| code | Stable searchable identifier |
| title | One-line human summary |
| detail | Redacted bounded context |
| next_action | Exact recovery command or choice |
| retryable | Whether retry makes sense |
| docs | Optional stable documentation anchor |
| exit_code | Stable process result |

Default text:

~~~text
AIShe: model_not_found
The active connection cannot use model example-model.
Next: /model
~~~

Debug detail belongs behind a verbose flag or support bundle, not in an
unbounded alternate-format anyhow chain by default.

## 11. Compatibility policy

### 11.1 Declared tiers

| Tier | Surface | Intended guarantee |
| --- | --- | --- |
| A | AIShe zsh PTY on supported Linux/macOS | Flagship: routing, prompt, keys, status, approvals, agent rendering, native zsh |
| A- | Native zsh hook | Same routing and agent handoffs; user's prompt remains authoritative |
| B | Native bash hook | Documented reduced feature set until full-buffer parity and Bash 3.2/5.x tests exist |
| C | -c, pipes, JSON | Stable non-interactive Rust dispatcher and automation contract |
| Research | fish hook, WSL | No promise until qualification |
| Unsupported | native Windows | No current binaries or shell integration |

### 11.2 Platform matrix

Qualify:

- macOS arm64 and x86_64 artifacts
- Linux x86_64 and arm64, glibc and musl artifacts
- zsh 5.8 and 5.9 where available
- macOS Bash 3.2 and Linux Bash 5.x
- bubblewrap available, missing, and installed-but-unusable namespace cases
- local terminal, SSH PTY, tmux, and screen
- TERM=xterm-256color, screen, tmux, dumb, and unset/odd values
- NO_COLOR set to empty and non-empty values
- light, dark, 16-color, 256-color, and truecolor configurations
- columns 20, 40, 60, 80, 120, 200 and dynamic resize
- ASCII, combining text, CJK width, emoji, and control-sequence injection
- bracketed paste, multiline input, Ctrl-C, Ctrl-Z, EOF, and terminal close
- concurrent shells and long remote key latency

### 11.3 Doctor capability report

Doctor should report effective tier and caveats:

~~~text
interactive shell: Tier A · zsh 5.9 · PTY
input routing: full-buffer zsh contract
terminal UI: 256 colors · dark · Unicode · live redraw
agent isolation: workspace · network deny · bubblewrap active
~~~

On bash it should name missing parity rather than simply saying supported.

## 12. Functional requirements

### Routing

- FR-001: Return a structured RouteDecision for every non-empty line.
- FR-002: Keep explicit ? and ! prefixes deterministic and strip them once.
- FR-003: Never start backend/provider work for a shell RouteDecision.
- FR-004: Maintain a versioned corpus with platform/PATH fixtures.
- FR-005: Prove route/highlight conformance across declared surfaces.
- FR-006: Expose route explanation in text and schema-versioned JSON.
- FR-007: Preserve shell-first behavior for unresolved executable collisions.
- FR-008: Bound every parse and cache operation.

### Command surface

- FR-020: Define built-in slash commands in one registry.
- FR-021: Include aliases, help, availability, arguments, mutability, and
  shell-local handoff metadata.
- FR-022: Prevent custom commands from shadowing registered built-ins.
- FR-023: Generate or assert zsh/bash hook cases from the registry.
- FR-024: Generate help/reference tests from the registry.
- FR-025: Reject or clearly explain an unavailable command; never silently turn
  a removed built-in into a model request.

### Terminal UX

- FR-040: Resolve all styles through TerminalCapabilities and semantic tokens.
- FR-041: Emit no ANSI in none/plain/JSON/non-TTY modes.
- FR-042: Render a persistent non-color assistant boundary.
- FR-043: Keep shell program output byte-for-byte owned by the shell program.
- FR-044: Keep selected picker rows visible.
- FR-045: Measure display cells and preserve graphemes.
- FR-046: Support live and static progress modes.
- FR-047: Provide Unicode and ASCII glyph sets.
- FR-048: Select syntax themes compatible with explicit light/dark/mono modes.

### Interaction and recovery

- FR-060: Stage suggested commands into native zsh editing where possible.
- FR-061: Show route override instructions only when relevant and rate-limit
  ambient hints.
- FR-062: Normalize errors into stable codes and next actions.
- FR-063: Make approval scope, network, sandbox, and effect clear.
- FR-064: Preserve Ctrl-C and terminal restoration on every prompt/picker path.
- FR-065: Preserve setup/settings transactional rollback and secret handling.

### Compatibility and automation

- FR-080: Publish a compatibility-tier table and Doctor evidence.
- FR-081: Test Bash 3.2 and Bash 5.x at the claimed tier.
- FR-082: Add schema_version to every public JSON document.
- FR-083: Define stdout, stderr, exit codes, and ANSI policy per command.
- FR-084: Keep backward fixtures for at least two minor versions before 1.0.

### Maintainability and docs

- FR-100: Mark every design document lifecycle.
- FR-101: Split main.rs by command domain after characterization.
- FR-102: Move shell integrations to reviewable templates/modules and syntax
  check generated output.
- FR-103: Add owner/review date to dependency/advisory exceptions.
- FR-104: Ignore Python-generated caches.
- FR-105: Verify validation binary identity against the checkout.

## 13. Non-functional requirements

- NFR-001: direct shell p95 overhead stays at most 10 ms versus raw zsh.
- NFR-002: direct shell route performs no network and does not start OpenCode.
- NFR-003: route evaluation is deterministic for the same capability fixture.
- NFR-004: UI fields influenced by model/repo data are escaped and bounded.
- NFR-005: raw mode is restored after success, error, panic boundary, signal,
  terminal close, and cancellation.
- NFR-006: config and credential writes remain atomic and permission-safe.
- NFR-007: no default telemetry.
- NFR-008: JSON remains valid UTF-8, control-safe, and free of ANSI.
- NFR-009: new UI works at 40 columns; 20-column behavior remains functional.
- NFR-010: release binaries remain within an explicit measured size budget.
- NFR-011: no new ignored advisory lacks rationale, owner, and review date.
- NFR-012: MSRV 1.88 remains enforced until intentionally changed.

## 14. Implementation epics and stories

Each story is intended to be reviewable in one focused agent session or a small
pull request. “Tests” are part of the story, not follow-up work.

### Epic 0: truth, corpus, and qualification foundations

#### UX-001 — Create the document lifecycle index

**Why:** stale plans currently look actionable.

**Work:**

- Add docs/design/README.md with lifecycle definitions.
- Inventory every file under docs/design plus root plans.
- Banner each as active, implemented, superseded, historical, or validation
  evidence; include baseline and successor link.
- Point docs/README or contributor docs to the index.

**Acceptance:**

- No design file lacks a lifecycle label.
- A repository check fails if a new design file has no label.
- The index names this plan as active for its scope.

**Tests:** lightweight docs lint script plus link check.

**Dependencies:** none. **Priority:** P0. **Effort:** S.

#### UX-002 — Correct current architecture and command documentation

**Why:** current docs describe a thin driver, schema v4, and removed prompt
commands.

**Work:**

- Update docs/architecture.md to schema 6 and the actual driver/module seams.
- Remove the obsolete prompt-only meta section from docs/commands.md.
- Update contributor offline wording: Cargo.lock pins dependencies but does not
  make a fresh build offline without a populated cache/vendor tree.
- Correct test/module descriptions that still refer to reedline or legacy jobs.

**Acceptance:**

- Every documented slash command is executable on its stated surface.
- Architecture does not call main.rs thin until extraction actually makes it so.
- No active doc describes reedline as a current front end.

**Tests:** command-doc conformance introduced by CMD-004.

**Dependencies:** UX-001. **Priority:** P0. **Effort:** S.

#### QA-001 — Verify exact binary identity in every external harness

**Why:** the first audit run accidentally exercised a stale 0.6.3 binary.

**Work:**

- Add a shared Python helper that captures aishe --version once.
- Compare the reported version/commit with Cargo.toml and git HEAD when the
  checkout has git metadata.
- Add --allow-mismatched-binary only for intentional release-artifact tests.
- Print identity at the start of every PTY/admin report.
- Make the documented local gate build release before running harnesses.

**Acceptance:**

- Passing a stale binary fails before behavioral checks.
- Packaged artifact validation can opt into digest/version evidence without git.

**Tests:** helper unit test with fake version outputs.

**Dependencies:** none. **Priority:** P0. **Effort:** S.

#### QA-002 — Repair repository and shell-script hygiene

**Why:** Python caches are untracked and shellcheck findings are unresolved.

**Work:**

- Ignore __pycache__, *.pyc, .pytest_cache, coverage, and local virtualenvs.
- Decide each SC2016/SC1007 finding; fix or annotate with narrow rationale.
- Add shellcheck to the documented and CI quality gate.
- Run zsh -n and bash -n over generated hook output.

**Acceptance:**

- A test run leaves git status clean in a clean checkout.
- shellcheck exits zero.
- Generated hook scripts parse in their declared shell versions.

**Dependencies:** none. **Priority:** P0. **Effort:** S.

#### QA-003 — Replace stale meta-command assertions

**Why:** tests currently pass phantom features and fail current brand copy.

**Work:**

- Remove legacy /ghost, /plan, /cache, and /sandbox expectations unless the
  registry intentionally reintroduces them.
- Match stable error codes or structured output, not brand prose.
- Split product-copy snapshots from behavior assertions.
- Record corpus_version in generated validation reports.

**Acceptance:**

- Current deterministic admin validation is 456/456 or its intentionally revised
  total, with no phantom-command pass.
- Brand changes cannot break semantic tests.

**Dependencies:** CMD-001 for final registry; an initial correction may land
earlier. **Priority:** P0. **Effort:** S.

### Epic 1: one command surface

#### CMD-001 — Define CommandSpec and the authoritative registry

**Why:** command identity and availability are duplicated.

**Work:**

- Create src/command_surface.rs.
- Define stable id, CLI command, slash aliases, summary, help topic, argument
  policy, availability by surface, output type, side-effect class, and whether
  shell-local state/handoff is required.
- Register the actual 0.6.5 surface before adding features.
- Explicitly classify removed legacy commands as tombstones with replacement
  guidance for one compatibility window.

**Acceptance:**

- dispatcher::is_meta_subcommand is deleted or becomes a registry query.
- Registry validation rejects duplicate names/aliases and missing help.
- Unit tests enumerate every active and tombstoned entry.

**Dependencies:** QA-003 can proceed in parallel but finalizes after this.
**Priority:** P0. **Effort:** M.

#### CMD-002 — Route slash commands through the registry in Rust

**Why:** custom-command shadowing and one-shot behavior currently depend on an
incorrect list.

**Work:**

- Use the registry in dispatcher, parse_slash, try_custom_command, and one_shot.
- Return a clear unavailable/shell-only error for known commands.
- Preserve absolute paths beginning with slash.
- Add correct handling for connection, auth, scope, and network according to
  declared availability.

**Acceptance:**

- Built-ins cannot be shadowed by user/project commands.
- /usr/bin/env remains a shell path.
- Unknown custom commands and MCP prompts retain their precedence.
- Removed built-ins do not silently reach the agent.

**Tests:** table-driven unit and CLI tests for every registry entry and path
collision.

**Dependencies:** CMD-001. **Priority:** P0. **Effort:** M.

#### CMD-003 — Drive zsh and bash hook cases from the registry

**Why:** hooks carry different manual case statements.

**Work:**

- Render shell case fragments or stable lookup tables from CommandSpec.
- Keep shell-local operations explicit: connection/model/reasoning/mode/output
  may need handoff files or environment mutation.
- Syntax-check emitted hooks.
- Fail tests if a registry command has no implementation for a declared surface.

**Acceptance:**

- No hand-maintained slash command list remains in embedded zsh/bash code.
- Every declared command works in a real PTY/hook test.
- Shell-local versus durable effects are shown to the user.

**Dependencies:** CMD-001, CMD-002. **Priority:** P0. **Effort:** M.

#### CMD-004 — Generate help and docs conformance from CommandSpec

**Why:** task-first help is valuable but still drifts.

**Work:**

- Let product_help query registry metadata while preserving task-oriented prose.
- Generate a compact command reference fragment or verify hand-written docs
  against the registry.
- Add completion aliases where appropriate.
- Provide replacement guidance for tombstoned commands.

**Acceptance:**

- /help, aishe commands, CLI help references, docs/commands.md, and completion
  tests contain exactly the declared names for each surface.
- New commands cannot merge without summary/help/availability metadata.

**Dependencies:** CMD-001 through CMD-003. **Priority:** P0. **Effort:** M.

### Epic 2: canonical, explainable routing

#### ROUTE-001 — Introduce RouteDecision and stable reason codes

**Why:** Dispatch communicates outcome but not evidence.

**Work:**

- Add RouteKind and RouteReason without changing behavior.
- Capture normalized line, effective head, known-command state, and ambiguity.
- Keep Dispatch compatibility temporarily or migrate call sites atomically.
- Bound user-controlled fields in debug output.

**Acceptance:**

- Every existing dispatcher case maps to a stable reason.
- Fast shell admission still returns without loading config/backend state.
- Benchmarks show no material regression.

**Tests:** table tests for every reason plus property checks for sigil stripping.

**Dependencies:** CMD-002. **Priority:** P0. **Effort:** M.

#### ROUTE-002 — Create a versioned cross-platform route corpus

**Why:** current tests cover many shell forms but relatively few intent
collisions and do not centrally describe PATH/platform assumptions.

**Work:**

- Add tests/fixtures/routing/v1.json or TOML.
- Fields: id, input, expected kind, expected reason, platform, known commands,
  aliases/functions, notes, and criticality.
- Include Linux/macOS command-name collisions, builtins, assignments, control
  structures, quoting, operators, typos, Unicode, comments, paths, slash commands,
  and malicious control text.
- Separate normative cases from discovery/research cases.

**Acceptance:**

- Corpus runs against Rust classifier on Linux and macOS.
- Critical corpus has zero false-shell natural-language cases.
- Every production routing bug adds a fixture before its fix.

**Dependencies:** ROUTE-001. **Priority:** P0. **Effort:** M.

#### ROUTE-003 — Enforce zsh highlight/submit conformance

**Why:** duplicated question grammar can drift.

**Work:**

- Move question-pair data to a declarative Rust table or build asset.
- Generate the zsh predicate where practical; otherwise execute the corpus
  against both predicates and fail on mismatch.
- Assert that visible route color and Enter submission choose the same route.
- Preserve native syntax highlighters taking precedence.

**Acceptance:**

- 100% agreement on the zsh-declared corpus.
- A grammar change requires one source edit plus snapshot update.
- No per-keystroke backend/model call.

**Dependencies:** ROUTE-002. **Priority:** P0. **Effort:** M.

#### ROUTE-004 — Add aishe route explain/JSON

**Why:** users and support need to inspect ambiguous behavior.

**Work:**

- Add aishe route with --json and -- before the line.
- Text includes kind, reason, effective head, and the opposite-route override.
- JSON contains schema_version and stable enums.
- Doctor/support bundle may include a bounded list of user-provided route probes
  only when explicitly requested; never collect ambient typed input.

**Acceptance:**

- install kubectl please explains known_command and recommends ?.
- ? install kubectl please explains forced_agent.
- ! command explains forced_shell and safety bypass.
- JSON is ANSI-free and snapshot-tested.

**Dependencies:** ROUTE-001. **Priority:** P1. **Effort:** S.

#### ROUTE-005 — Design typo and ambiguity assistance

**Why:** unknown command typos currently become model prompts and existing fuzzy
correction code is unused.

**Work:**

- Build a labeled corpus for typos versus natural-language unknown heads.
- Prototype a local cue using CommandCache::correction without auto-execution.
- Rate-limit suggestions and never transmit input merely to disambiguate.
- Decide whether the cue belongs in zsh command-not-found, pre-submit, or
  post-failure behavior.

**Acceptance:**

- No typo suggestion executes automatically.
- Corpus false-positive threshold is documented before enabling by default.
- Direct command SLO and privacy are preserved.

**Dependencies:** ROUTE-002. **Priority:** P2. **Effort:** M. **Type:** research
then implementation.

#### ROUTE-006 — Decide the # prefix lifecycle

**Why:** it conflicts with shell comments.

**Work:**

- Add corpus/tests for interactive comments and forced-agent behavior.
- Document ? as canonical.
- Choose keep, configurable, or two-release deprecation.
- If deprecated, show a bounded local message and provide migration notes.

**Acceptance:** behavior is deliberate, tested, documented, and consistent
across surfaces.

**Dependencies:** ROUTE-002. **Priority:** P1. **Effort:** S.

### Epic 3: picker correctness and interaction

#### PICK-001 — Add a selection-following viewport

**Why:** invisible selection is a correctness bug.

**Work:**

- Compute visible start/end around selected and terminal row budget.
- Render match count and absolute position.
- Keep selected in range through filter changes and wrap/navigation.
- Test 0, 1, 20, 21, 100, and 1,000 matches.

**Acceptance:** Enter can only select the visibly marked row.

**Dependencies:** none. **Priority:** P0. **Effort:** S.

#### PICK-002 — Make printable characters always filter

**Why:** d/j/k currently conflict with stated behavior.

**Work:**

- Remove d default shortcut and empty-filter j/k navigation.
- Keep arrow keys; add Ctrl-P/Ctrl-N, PageUp/PageDown, Home/End.
- Retain one post-Enter default-promotion prompt.
- Remove duplicate connection-picker help lines.

**Acceptance:**

- Searches beginning with d, j, and k work.
- Key help exactly matches behavior.
- Connection and model picker share one interaction contract.

**Dependencies:** PICK-001. **Priority:** P0. **Effort:** S.

#### PICK-003 — Use terminal-cell and grapheme-aware layout

**Why:** chars().count() is not display width.

**Work:**

- Select a small maintained width/grapheme dependency or implement a narrowly
  reviewed helper.
- Centralize truncate, wrap, pad, and row estimation.
- Strip/escape control text before width measurement.

**Acceptance:**

- Golden tests cover combining marks, CJK, emoji, ANSI-stripped text, and narrow
  columns.
- Truncation never splits a grapheme or exceeds the target cell width.

**Dependencies:** UI-001 can provide the destination module. **Priority:** P1.
**Effort:** M.

#### PICK-004 — Add plain/static picker fallback

**Why:** raw redraw is not ideal for every terminal or assistive technology.

**Work:**

- In static mode, print a bounded numbered page and accept number/filter input.
- Preserve cancellation and terminal restoration.
- Add explicit non-TTY direct-form guidance rather than trying to interact.

**Acceptance:** all picker tasks are possible without cursor motion or color.

**Dependencies:** PICK-001, A11Y-002. **Priority:** P1. **Effort:** M.

#### PICK-005 — Improve ranking without changing identity

**Why:** substring matching becomes noisy with large model catalogs.

**Work:**

- Rank exact prefix, token prefix, substring, then fuzzy subsequence.
- Use existing fuzzy helpers where suitable.
- Keep stable tie ordering and display the exact provider model id.

**Acceptance:** deterministic ranking snapshots and no selection/default identity
regression.

**Dependencies:** PICK-001 through PICK-003. **Priority:** P2. **Effort:** S.

### Epic 4: terminal design system and response differentiation

#### UI-001 — Centralize TerminalCapabilities

**Why:** every module currently decides TTY/color differently.

**Work:**

- Add src/ui/capabilities.rs.
- Resolve TTY, NO_COLOR, TERM=dumb, color depth, background override, Unicode,
  motion, and dimensions once per interaction.
- Define environment/config precedence and test pure resolution functions.
- Make JSON force no style regardless of terminal.

**Acceptance:** no production call site needs to read NO_COLOR directly outside
the capability module and shell-template boundary.

**Dependencies:** none. **Priority:** P0. **Effort:** M.

#### UI-002 — Introduce semantic styles and glyph sets

**Why:** literal colors encode no shared meaning.

**Work:**

- Add semantic StyleToken and Renderer helpers.
- Define dark, light, mono, and none palettes for ANSI16/256/truecolor.
- Define Unicode and ASCII glyphs for success, error, warning, focus, route,
  progress, diff, and mode.
- Migrate promptui and agent renderer first.

**Acceptance:**

- Token snapshots exist for each palette/depth.
- State remains distinct after stripping ANSI.
- No whole-answer foreground tint.

**Dependencies:** UI-001. **Priority:** P0. **Effort:** M.

#### UI-003 — Eliminate ANSI leaks

**Why:** NO_COLOR currently emits reset bytes from direct styling.

**Work:**

- Replace direct Stylize in main.rs, modes, tools, overlay, and provider fallback.
- Route termimad/syntect and embedded shell styling through effective policy.
- Add raw-byte assertions for errors, warnings, setup, status, answers, pickers,
  approvals, and JSON.

**Acceptance:** zero ESC bytes under NO_COLOR, TERM=dumb, non-TTY, ui.theme=none,
and all JSON modes.

**Dependencies:** UI-001, UI-002. **Priority:** P0. **Effort:** M.

#### UI-004 — Add the final assistant boundary

**Why:** answers blend with command output.

**Work:**

- Add a shared begin_assistant_answer/end boundary.
- Use it in managed focus/compact/detailed and native compatibility paths.
- Label prose answers, not emitted scriptable commands.
- Ensure redirected/plain output remains simple and parseable.

**Acceptance:**

- A monochrome transcript makes authorship unambiguous.
- Code copied from fenced blocks has no label prefix.
- Snapshot all output density modes.

**Dependencies:** UI-002. **Priority:** P0. **Effort:** S.

#### UI-005 — Unify command proposal and approval panels

**Why:** suggest, auto, yolo, trust, and dangerous-command prompts use different
styling and information density.

**Work:**

- Define reusable proposal and approval views.
- Show command/effect, reason, safety classification, scope, network, sandbox,
  and default action as applicable.
- Use safe negative defaults for destructive/host actions.
- Keep model-controlled text escaped and bounded.

**Acceptance:** color-stripped panels retain focus and risk distinctions; PTY
tests cover Enter, Esc, Ctrl-C, EOF, and resize.

**Dependencies:** UI-002, ERR-001. **Priority:** P1. **Effort:** M.

#### UI-006 — Make tool activity state semantic

**Why:** a completed tool is not always overall success.

**Work:**

- Distinguish queued, running, completed, recovered failure, terminal failure,
  changed file, delegated work, reconnect, and waiting-for-user.
- Keep focus terse, compact one-line, detailed expandable in scrollback.
- Include a truthful final outcome and changed-file summary.

**Acceptance:** recovered failures never render as uncomplicated green success;
status clears before interactive questions.

**Dependencies:** UI-002, UI-004. **Priority:** P1. **Effort:** M.

#### UI-007 — Define one streaming contract

**Why:** response appearance currently depends on whether text fits the screen.

**Work:**

- Print the assistant boundary before streaming.
- Choose a safe live Markdown strategy: either consistently stream plain body
  then append a stable final boundary, or incrementally render only constructs
  that do not require destructive rewrites.
- Keep cursor-up rerender optional and disabled in static/uncertain terminals.
- Use grapheme/cell width for row estimation.

**Acceptance:**

- Short and long answers have the same authorship structure.
- No duplicate text after resize/scroll.
- Pipes receive exactly one body.

**Dependencies:** UI-004, PICK-003, A11Y-002. **Priority:** P1. **Effort:** L.

#### UI-008 — Add theme configuration and light-theme code highlighting

**Why:** fixed dark truecolor output is not universally legible.

**Work:**

- Add config schema migration for ui.theme, ui.color_depth override,
  ui.unicode, and ui.motion.
- Choose syntax themes for dark/light; degrade to ANSI16 or plain code.
- Add settings controls and effective-config provenance.

**Acceptance:** transactional migration/rollback tests; recorded contrast review
for maintained palettes; NO_COLOR still wins.

**Dependencies:** UI-001 through UI-003. **Priority:** P1. **Effort:** M.

### Epic 5: accessibility and terminal resilience

#### A11Y-001 — Guarantee non-color state cues

**Work:** audit every route, mode, status, approval, success, warning, danger,
diff, selection, and error state after ANSI stripping.

**Acceptance:** snapshots remain distinguishable and instructions never refer
only to a color.

**Dependencies:** UI-002. **Priority:** P0. **Effort:** S.

#### A11Y-002 — Add static motion and ASCII modes

**Work:**

- Implement ui.motion=static and ui.unicode=ascii.
- Provide a simple ASCII brand mark or omit the large mark in dumb/plain modes.
- Replace live erased status with durable phase lines in static mode.
- Make doctor report the effective policy.

**Acceptance:** TERM=dumb and ASCII modes contain no half-blocks, box drawing,
cursor motion, or ANSI; all information remains present.

**Dependencies:** UI-001, UI-002. **Priority:** P1. **Effort:** M.

#### A11Y-003 — Build the terminal layout matrix

**Work:** create golden and PTY cases for 20–200 columns, wide text, combining
text, emoji, long paths/models, resize, and injected control sequences.

**Acceptance:** no panic, unsafe control execution, invisible focus, or line
overflow in maintained layouts.

**Dependencies:** PICK-003, UI-003. **Priority:** P1. **Effort:** M.

#### A11Y-004 — Document keyboard access and collisions

**Work:**

- Put active keybindings in /help session and shell-integration docs.
- Show terminal-specific Option/Meta caveats.
- Provide rebinding examples and a conflict diagnostic.
- Ensure all picker/menu operations have non-letter alternatives.

**Acceptance:** every interactive action is keyboard reachable and documented
without assuming a particular terminal emulator.

**Dependencies:** PICK-002, CMD-004. **Priority:** P1. **Effort:** S.

### Epic 6: suggest, agent, and recovery interaction

#### INT-001 — Stage suggested commands into native shell editing

**Why:** plain stdin editing is a regression from the product premise.

**Work:**

- In zsh hook/PTY, make Enter on a suggestion place the command in BUFFER and
  return to native editing; a second Enter executes normally.
- Provide explicit “run now” only if the user chooses it.
- For -c, preserve current scriptable stdout contract.
- For bash Tier B, document or implement the closest safe READLINE_LINE handoff.

**Acceptance:** completion, cursor keys, history edits, and syntax highlighting
work on staged commands; no command runs merely because edit was chosen.

**Dependencies:** CMD-003, UI-005. **Priority:** P1. **Effort:** M.

#### INT-002 — Clarify user questions and approvals

**Work:**

- Use a shared panel for AgentEvent::WaitingForUser and approval requests.
- Show which agent/task is blocked and whether the shell is awaiting input.
- Restore prompt state after answer/cancel.
- Keep secrets hidden only for actual credential prompts, not acceptance phrases.

**Acceptance:** PTY tests cover long prompts, no color, Ctrl-C, reconnect, and
multiple questions.

**Dependencies:** UI-005, UI-006. **Priority:** P1. **Effort:** M.

#### INT-003 — Normalize failure recovery

**Work:**

- Route provider/backend/sandbox/policy/config/route/tool errors through stable
  error codes.
- Offer one primary next action and optional details.
- Keep failure hints rate-limited and avoid repetition on prompt redraw.
- Make bare ? diagnostic behavior coexist clearly with ? force-agent.

**Acceptance:** error matrix snapshots and exact recovery commands; no secret
or unbounded backend payload.

**Dependencies:** ERR-001, CMD-004. **Priority:** P1. **Effort:** M.

#### INT-004 — Improve long-task progress and completion

**Work:**

- Define phases: connecting, planning, acting, waiting, recovering, finalizing.
- Show task/session identity only when useful.
- Summarize commands, files, recovered failures, usage, elapsed, and resume path.
- Avoid spinner/motion dependence.

**Acceptance:** focus mode stays within a bounded line budget; static mode emits
bounded phase changes; completion is truthful after partial failure.

**Dependencies:** UI-006, A11Y-002. **Priority:** P1. **Effort:** M.

### Epic 7: error and automation contracts

#### ERR-001 — Create a shared UserError model

**Work:**

- Extend or reuse agent::UserFacingError for CLI domains.
- Define stable code namespaces and exit-code mapping.
- Provide text and JSON renderers with redaction and bounds.
- Migrate the top 20 most common fatal paths first.

**Acceptance:** all migrated errors have code, next action, retryable flag, and
ANSI-free JSON.

**Dependencies:** UI-001. **Priority:** P0. **Effort:** M.

#### ERR-002 — Migrate remaining ad hoc error output

**Work:** inventory println/eprintln/anyhow fatal paths, categorize, and migrate
without hiding diagnostics from verbose/support bundles.

**Acceptance:** at least 95% common-path coverage in milestone; an allowlist with
owner remains for internal-only errors.

**Dependencies:** ERR-001. **Priority:** P1. **Effort:** L, split by module.

#### API-001 — Inventory public machine-readable surfaces

**Work:** document command, schema, version, stdout/stderr, exit codes, nullable
fields, bounds, and compatibility owner for setup, doctor, status, suggest,
provider tests, sessions, tasks, backend, models, config, usage, and future route.

**Acceptance:** docs/automation.md contains the complete inventory.

**Dependencies:** CMD-001. **Priority:** P0. **Effort:** S.

#### API-002 — Version status and suggest JSON

**Work:**

- Add schema_version without changing existing field meaning.
- Define structured error output for --json failures.
- Keep suggest exit 0/20/1 compatibility and document risk enums.
- Add fixtures for prior shape and migration notes.

**Acceptance:** jq/serde fixture tests, no ANSI, stable stdout, errors on stderr
or documented JSON envelope.

**Dependencies:** API-001, ERR-001. **Priority:** P0. **Effort:** S.

#### API-003 — Add compatibility fixture gates

**Work:** store representative v1 JSON and persisted records, deserialize old
fixtures, snapshot new output, and require explicit schema bump for breaking
changes.

**Acceptance:** CI blocks unreviewed field removal/type change.

**Dependencies:** API-001, API-002. **Priority:** P1. **Effort:** M.

### Epic 8: compatibility qualification

#### COMP-001 — Publish the tier matrix and Doctor reporting

**Work:** implement section 11 in installation, front-end, shell-integration,
and Doctor docs/output.

**Acceptance:** no “same behavior” claim exceeds tested evidence.

**Dependencies:** ROUTE-003, CMD-003. **Priority:** P1. **Effort:** S.

#### COMP-002 — Build a real bash-hook harness

**Work:**

- Spawn interactive bash with an isolated rc file and fake provider.
- Test unknown NL, ? force-agent, real command collision, mode cycle, details,
  failure fix, slash commands, state handoff, history, signals, and cleanup.
- Run against Bash 3.2 and 5.x.
- Mark expected Tier B differences explicitly.

**Acceptance:** 100% declared Tier B matrix on macOS and Linux.

**Implemented evidence (2026-08-01):** `tests/bash_hook.py` and its 17 unit
tests now qualify an isolated, fake-provider, interactive Bash session. Bash
5.3.9 passed all 18 Tier B cases on macOS and Linux. macOS Bash 3.2.57 passed 13
core cases and reported exactly five declared Tier B- Readline differences,
each with its exercised alternative. Strict required-family runs exit zero;
missing families remain unavailable rather than being counted as passes. See
[Native Bash hook compatibility](../bash-compatibility.md).

**Dependencies:** CMD-003, ROUTE-002. **Priority:** P1. **Effort:** L.

#### COMP-003 — Add macOS flagship PTY CI

**Work:** run a bounded deterministic subset of pty_scenarios, picker,
statusline, setup, and signals on macos-latest, with terminal permissions and
timeouts documented.

**Acceptance:** routing and terminal behavior regressions block on both
supported OS families.

**Implemented evidence (2026-08-01):** CI now has a bounded blocking
`macOS flagship PTY gate (bounded)` job for routing, connection/model picker,
statusline, setup, signals, 300 ms escape latency, and resize. Its PTY-only
permission model and per-step/job timeouts are documented in
[Terminal compatibility](../terminal-compatibility.md). A post-extraction
candidate rerun passed 65/65 routing/interaction cases, the model picker, every
status-line placement plus control-injection safety, the full setup/settings/
tour flow, and 7/7 resize/signal/job-control cases.

**Dependencies:** QA-001. **Priority:** P1. **Effort:** M.

#### COMP-004 — Qualify terminal multiplexers and SSH

**Work:** automate tmux/screen where available and record manual checks for
common terminals; simulate 300 ms escape-sequence latency and resize.

**Acceptance:** capability matrix records pass, limitation, or unsupported; no
silent claims.

**Implemented evidence (2026-08-01):** `tests/terminal_compat.py` emits explicit
`pass`, `fail`, `limitation`, or `unsupported` JSON; Linux CI requires local,
tmux, and screen passes. SSH is opt-in and is a limitation without an authorized
target. The post-extraction candidate passed the same staged-review, 300 ms
split-escape, 80x24 to 120x40 resize, and `TERM` contract through native PTY,
tmux, and attached screen on macOS and Ubuntu, plus an authorized end-to-end SSH
PTY. The SSH fixture prepended a symlink to the explicitly requested isolated
candidate; a distinct host-installed AIShe binary was not used. Reports redact
the target address and identity-file path. Common emulator checks remain visibly
`not_run` until manually recorded.

**Dependencies:** A11Y-003. **Priority:** P2. **Effort:** M.

#### COMP-005 — Fish and WSL decision spikes

**Work:** separately assess user value, architecture, security, packaging, and
test cost. Fish integration must not be inferred from fish completion support.

**Acceptance:** decision records with build/no-build outcome and no premature
README promise.

**Implemented evidence (2026-08-01):** the separate
[Fish integration decision](FISH_INTEGRATION_DECISION.md) records no native hook
for this milestone, while the [WSL decision](WSL_COMPATIBILITY_DECISION.md)
records no dedicated artifact or support claim before genuine WSL2
qualification. Installation docs distinguish Fish completion from a hook and
WSL research from native Linux/macOS support.

**Dependencies:** core milestone complete. **Priority:** Research. **Effort:** S
each.

### Epic 9: architecture extraction

#### ARCH-001 — Extract CLI command modules from main.rs

**Why:** reduce change collision and review surface.

**Work sequence:**

1. Move pure status rendering/JSON to src/cli/status.rs.
2. Move connection/model commands to src/cli/connection.rs.
3. Move session/resume/reset to src/cli/session.rs.
4. Move history/log/usage/runbook to domain modules.
5. Leave clap parsing and top-level orchestration in the binary.

Each move is its own pull request with no behavior change.

**Acceptance:** main.rs trends below 1,500 lines, no public output diff outside
approved snapshots, all existing tests remain green.

**Dependencies:** CMD-001, API-003, ERR-001. **Priority:** P1. **Effort:** L
split into S/M stories.

#### ARCH-002 — Extract terminal UI primitives

**Work:** create src/ui with capabilities, styles, glyphs, width, answer,
proposal, approval, picker, and error renderers. Keep promptui as a compatibility
facade during migration.

**Acceptance:** no circular dependency with business logic; pure renderers are
snapshot-testable.

**Dependencies:** UI-001 through UI-005. **Priority:** P1. **Effort:** M.

#### ARCH-003 — Make shell integration reviewable

**Work:**

- Split zsh shared hook, PTY wrapper, prompt, and bash hook into assets/templates
  or focused Rust modules.
- Substitute a small typed set of generated fragments.
- Preserve exact escaping and private handoff paths.
- Run shell syntax, snapshot, and real PTY tests for every change.

**Acceptance:** integration.rs becomes orchestration rather than a 1,400-line
string store; generated scripts are reproducible.

**Implemented evidence (2026-08-01):** `src/integration.rs` is now 69 lines of
orchestration. The shared zsh hook, zsh init wrapper, PTY `.zshenv`/`.zshrc`,
branded prompt, and Bash hook are reviewable files under
`src/integration/assets/`; registry-derived fragments, typed exact-one template
substitutions, and tests live in focused sibling modules. SHA-256 snapshots pin
all four public generated artifacts to their pre-extraction bytes. All six
assets and both generated hooks pass `zsh -n`/`bash -n`; 35 integration tests,
the full macOS PTY matrix, native macOS/Linux tmux and attached-screen matrices,
Bash 3.2/5.x qualification, and the isolated Linux/SSH candidate reruns pass.

**Dependencies:** CMD-003, ROUTE-003, QA-002. **Priority:** P1. **Effort:** L.

#### ARCH-004 — Remove dead legacy front-end fields deliberately

**Work:** inventory config fields and code left only for reedline/native legacy
resume. Define compatibility lifetime, migration, and tombstone behavior before
deletion.

**Acceptance:** no active help advertises removed fields; old configs migrate
with backup; legacy tasks retain the promised support window.

**Dependencies:** UX-001, API-003. **Priority:** P2. **Effort:** M.

### Epic 10: safety, privacy, and trust UX

#### SAFE-001 — Make forced-shell bypass unmistakable

**Work:** show a local, non-color “shell override” cue for !, document that the
AI safety gate does not apply, and ensure ! does not become sticky state.

**Acceptance:** route explain and PTY tests prove one-line scope and no model
call.

**Dependencies:** ROUTE-001, UI-002. **Priority:** P1. **Effort:** S.

#### SAFE-002 — Standardize effective authority in approvals

**Work:** show mode, workspace/host, network, sandbox implementation, target
command/path, and why approval is required. Use stronger language for macOS
policy-only and Linux host scope.

**Acceptance:** reviewers can distinguish isolated workspace, policy-only
workspace, and host authority after stripping color.

**Dependencies:** UI-005. **Priority:** P1. **Effort:** M.

#### SAFE-003 — Version the threat model and qualification

**Work:** add threat-model version/date, sandbox functional-test identity,
runtime/plugin pin, and known limitations to release evidence. Keep safety
matcher language explicitly defense-in-depth.

**Acceptance:** each release can state which threat model and boundary were
qualified.

**Dependencies:** none. **Priority:** P1. **Effort:** S.

#### SAFE-004 — Add retention and deletion controls

**Work:** inventory history, audit, sessions, tasks, usage, caches, support
bundles, OAuth, credentials, and undo. Define default retention, size bounds,
rotation, export, and exact deletion commands. Keep destructive deletion
category-specific with dry preview and confirmation.

**Acceptance:** Doctor reports excessive/unbounded state; uninstall and cleanup
tests prove preservation defaults and exact deletion.

**Dependencies:** API-001. **Priority:** P2. **Effort:** L split by state type.

#### SAFE-005 — Expand parser and boundary fuzzing

**Work:** target route parser, safety fixed point, shell handoff records, terminal
escape sanitization, JSON/SSE bounds, archive extraction, and selection files.

**Acceptance:** deterministic seeds in CI; crash/hang/control-injection findings
become regression fixtures.

**Dependencies:** ROUTE-002, A11Y-003. **Priority:** P1. **Effort:** M.

### Epic 11: dependencies, performance, and release engineering

#### DEP-001 — Align crossterm versions

**Work:** evaluate upgrading direct crossterm to termimad's version, run terminal
matrix, and confirm MSRV. Measure duplicate dependency and binary-size effect.

**Acceptance:** one crossterm version if compatible; otherwise a documented
temporary exception with owner/review date.

**Dependencies:** UI snapshot foundation. **Priority:** P1. **Effort:** S.

#### DEP-002 — Complete ureq 3 migration

**Work:** port remaining external HTTPS clients in small modules, preserve native
roots, timeouts, proxy behavior, retry policy, and provider fixtures.

**Acceptance:** remove ureq 2 and RUSTSEC-2025-0134 exception; live-contract and
deterministic HTTP tests pass on Linux/macOS.

**Dependencies:** none, but avoid mixing with UI work. **Priority:** P1.
**Effort:** L split by client.

#### DEP-003 — Time-box advisory exceptions

**Work:** add owner, added date, next review date, target removal, and validation
argument for every ignore.

**Acceptance:** CI or maintenance checklist flags expired review dates.

**Dependencies:** none. **Priority:** P1. **Effort:** S.

#### PERF-001 — Expand performance budgets

**Work:** record direct shell p50/p95, initial PTY prompt, route decision,
picker 1,000-row redraw, backend cold/warm start, long-answer render, RSS, and
binary size for default/no-highlight builds.

**Acceptance:** versioned JSON benchmarks and regression thresholds where hosts
are stable; informational trends elsewhere.

**Dependencies:** QA-001. **Priority:** P1. **Effort:** M.

#### PERF-002 — Protect lazy loading with assertions

**Work:** instrument test-only backend/provider/extension startup markers and
prove shell, help, route explain, and local status paths start only intended
components.

**Acceptance:** direct shell and route classification show backend_started=false
and no network listener/provider request.

**Dependencies:** ROUTE-001. **Priority:** P0. **Effort:** S.

#### REL-001 — Build a single qualification driver

**Work:**

- Add a script/xtask that builds the correct binary and runs gates by profile:
  quick, local-full, linux-full, release, paid-live.
- Record version, commit, OS, shell, runtime pin, sandbox result, corpus versions,
  commands, durations, skips, and artifact digests.
- Never hide a skipped credentialed or platform gate as pass.

**Acceptance:** one machine-readable report and human summary; stale binary is
impossible by default.

**Dependencies:** QA-001, API-003. **Priority:** P1. **Effort:** M.

#### REL-002 — Define release readiness and rollback

**Work:** require deterministic supported-platform gates, dependency policy,
install/upgrade transaction, runtime verify/repair/rollback, schema migrations,
compatibility fixtures, and explicit paid-live/soak disposition. Document binary
rollback versus persistent-state forward compatibility.

**Acceptance:** a release candidate cannot be published with ambiguous holds.

**Dependencies:** REL-001. **Priority:** P1. **Effort:** S.

### Epic 12: onboarding, settings, and information architecture

#### ONB-001 — Simplify first successful path

**Work:** evaluate setup with a new-user timed walkthrough; keep resumability and
transactionality, but offer a short path for an already-authenticated supported
connection. Show steps remaining and why live validation costs tokens.

**Acceptance:** a new user can reach a verified first agent answer without
reading the full README; no credential is echoed or guessed.

**Dependencies:** UI-002, ERR-001. **Priority:** P1. **Effort:** M.

#### ONB-002 — Make shell-local versus durable state explicit

**Work:** use consistent copy in connection/model/reasoning/mode/output:
“this shell” versus “default for new shells.” Remove duplicate save affordances.

**Acceptance:** picker, direct CLI, status, help, and settings use the same terms;
tests prove no accidental config rewrite.

**Dependencies:** CMD-004, PICK-002. **Priority:** P0. **Effort:** S.

#### ONB-003 — Add contextual discovery without prompt spam

**Work:** keep the one-time launch hint concise, add relevant next-action hints
after ambiguity/failure/first answer, and store local seen-state with a reset.

**Acceptance:** hints are rate-limited, disableable, static-mode safe, and never
repeat on prompt redraw.

**Dependencies:** ROUTE-001, INT-003. **Priority:** P2. **Effort:** M.

#### ONB-004 — Consolidate troubleshooting

**Work:** map stable error codes to docs anchors and exact Doctor/backend/
connection commands. Keep support-bundle privacy exclusions visible.

**Acceptance:** every common error code has one maintained troubleshooting
entry; links are checked.

**Dependencies:** ERR-001, ERR-002. **Priority:** P1. **Effort:** M.

## 15. Dependency graph and recommended sequence

~~~text
Wave 0: truth and test identity
  UX-001 UX-002 QA-001 QA-002
        |
Wave 1: contracts
  CMD-001 -> CMD-002 -> CMD-003 -> CMD-004
  ROUTE-001 -> ROUTE-002 -> ROUTE-003
  UI-001 -> UI-002 -> UI-003
  ERR-001
        |
Wave 2: correctness and visible UX
  PICK-001 -> PICK-002
  UI-004 UI-005 A11Y-001 ONB-002 API-001 API-002 PERF-002
  ROUTE-004 ROUTE-006
        |
Wave 3: depth and parity
  PICK-003 PICK-004 UI-006 UI-007 UI-008
  A11Y-002 A11Y-003 INT-001 INT-002 INT-003 INT-004
  COMP-001 COMP-002 COMP-003 API-003
        |
Wave 4: extraction and hardening
  ARCH-001 ARCH-002 ARCH-003 ARCH-004
  SAFE-002 SAFE-004 SAFE-005
  DEP-001 DEP-002 DEP-003 PERF-001 REL-001 REL-002
        |
Wave 5: researched expansion
  ROUTE-005 PICK-005 COMP-004 COMP-005 ONB-003
~~~

Recommended release slicing:

- Patch stabilization: PICK-001, NO_COLOR leak fix, stale validation correction,
  Python ignore, and documentation lifecycle banners.
- First minor: command registry, RouteDecision/corpus/explain, picker key model,
  semantic capabilities/tokens, answer boundary, JSON versions.
- Second minor: static/ASCII/theme modes, unified panels/progress, bash harness,
  macOS PTY CI, terminal-width correctness.
- Following minors: architecture extraction, ureq migration, retention, broad
  terminal qualification, and research outcomes.

Do not make the patch release wait for the full visual system if the invisible
picker selection can be fixed safely first.

## 16. Validation strategy

### 16.1 Required on every pull request

~~~sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo deny check advisories bans licenses sources
shellcheck install.sh tests/*.sh
cargo build --release --locked
~~~

After QA-002, generated hook syntax checks are also mandatory.

### 16.2 Targeted gates by changed area

| Area | Required evidence |
| --- | --- |
| routing | Rust corpus, zsh conformance, bash declared-tier corpus, pty_scenarios, direct-shell benchmark |
| command registry/help | registry validation, CLI tests, real zsh/bash slash PTY, docs conformance |
| picker | unit viewport/key/width tests, model_picker_pty, setup_pty, 1,000-row benchmark |
| styles/rendering | raw-byte no-ANSI matrix, semantic snapshots, agent event fixtures, light/dark/manual contrast |
| setup/settings | setup_pty, config migration, rollback, secret-safe transcript scan |
| agent/backend | OpenCode fixtures, runtime contract, concurrency, isolation, durable resume, soak profile |
| security/sandbox | safety corpus, fuzz seeds, Linux functional bwrap, host/workspace scope |
| API/JSON | old/new fixtures, schema validation, stdout/stderr, exit codes, jq parsing |
| packaging | installer transaction/upgrade, target builds, checksums, SBOM/provenance |

### 16.3 Route corpus design

Each fixture should include:

~~~json
{
  "id": "known-install-imperative",
  "input": "install kubectl please",
  "known_commands": ["install"],
  "expected": "shell",
  "reason": "known_command",
  "critical": true,
  "notes": "shell-first collision; UI recommends ? for agent"
}
~~~

Corpus groups:

- explicit sigils and comments
- actual executables on Linux/macOS
- zsh/bash builtins, aliases, functions, reserved words
- env assignments and assignment-only input
- control structures, functions, arithmetic, substitutions, redirects
- quote/operator/path/glob cases
- pipelines with all-known and unknown segments
- English interrogatives and imperative requests
- command-name collisions: what, where, who, time, test, find, install, open,
  say, yes, false, true, read, type, source, history
- likely typos and unknown commands
- slash commands, absolute paths, custom commands, MCP prompts
- Unicode whitespace and punctuation
- terminal control and pathological length

Normative classification must not vary with the maintainer's host PATH; fixtures
declare known heads. Separate host-discovery smoke tests may use the real PATH.

### 16.4 Renderer golden matrix

For every relevant view, capture:

- themes: dark, light, mono, none
- depths: 16, 256, truecolor
- TTY: true/false
- motion: live/static
- glyphs: Unicode/ASCII
- widths: 20/40/80/120/200
- content: short, long, multiline, wide, combining, malicious control text

Views:

- shell/agent route cue
- assistant final boundary
- suggested command
- dangerous/unknown approval
- trust prompt
- waiting-for-user
- tool lifecycle and recovered failure
- diff
- error with next action
- menu/picker empty/filtered/paged
- setup/status/doctor

Golden tests assert semantic plain text first, then capability-specific ANSI.

### 16.5 Release gates

A public release requires:

- exact source/binary identity
- all deterministic gates on supported OS/architecture families
- Linux bubblewrap functional qualification for isolated claims
- macOS policy-only copy review
- install, upgrade, repair, rollback, and config migration evidence
- managed runtime and plugin digest verification
- compatibility fixture pass
- no expired dependency exception
- SBOM, checksums, provenance, and package smoke
- paid-live matrix or explicit documented hold/waiver by the maintainer
- soak gate appropriate to the changed lifecycle surface
- clean rollback instructions

## 17. Migration and backward compatibility

### Config

- Add UI fields in one schema migration with conservative defaults matching
  current appearance as closely as possible.
- Back up before migration and keep atomic write/rollback behavior.
- Unknown future fields must not be silently discarded by an older binary.
- Project overlays may set cosmetic UI fields without trust; authority-bearing
  scope/network/safety fields keep current trust/policy rules.

### Slash commands

- Registry tombstones provide replacement text for removed names.
- Do not reuse a removed built-in name for a custom command during the
  compatibility window.
- Document when a slash alias is shell-local, durable, read-only, or unavailable
  in -c.

### JSON

- Additive fields are allowed within a schema version only if consumers can
  ignore unknown fields.
- Removal, rename, type change, or semantic change requires a schema bump.
- Keep fixtures/readers for at least two minor releases before 1.0.

### Output styling

- New assistant labels are an interactive-human output change, not a scriptable
  stdout change.
- suggest command stdout and JSON remain stable.
- JSON, pipes, and explicit plain mode never gain decorative labels or ANSI.

### Prefixes and keys

- ? and ! remain stable.
- # needs the explicit ROUTE-006 decision and notice window.
- Rebinding environment variables remain supported; Doctor can report conflicts.
- Removing d/j/k picker shortcuts is acceptable because the documented durable
  direct CLI remains and the post-Enter default prompt is retained; call it out
  in release notes.

## 18. Risks and mitigations

| Risk | Consequence | Mitigation |
| --- | --- | --- |
| Route refactor changes shell behavior | Commands sent to model or NL run as shell | Characterization corpus first; shell-first invariant; exact reason snapshots |
| Hook generation breaks user shell | Startup or Enter failure | zsh -n/bash -n, isolated rc PTY, chain existing widgets/traps, staged rollout |
| Visual polish contaminates scripts | ANSI/labels in pipelines | TerminalCapabilities, raw-byte tests, explicit JSON/plain contracts |
| Width dependency raises MSRV/size | Build or release regression | evaluate small crate, MSRV CI, size benchmark, fallback reviewed helper |
| Static mode diverges | Accessibility path rots | same semantic view model, snapshot both modes |
| Main extraction causes conflicts | hard-to-review functional changes | one domain per PR, no copy change, fixtures before moves |
| Bash parity becomes endless | delays flagship UX | honest Tier B, bounded declared matrix, no false equality |
| Theme auto-detection is wrong | poor contrast | explicit dark/light override, conservative default, mono escape hatch |
| More hints create noise | degraded expert UX | rate limit, seen-state, disable setting, relevance only |
| Error codes expose internals | unstable automation | domain codes, bounded detail, verbose/support bundle separation |
| Dependency migration changes TLS | provider failures | module-by-module fixtures, native roots/proxy/timeout tests, live gate |
| Retention deletion loses state | irreversible damage | preview, categories, safe defaults, confirmation, backups where possible |

## 19. Open decisions with recommended answers

These are non-blocking because the plan carries a recommended default:

1. Should ambiguous executable-headed English ever prompt before shell?
   Recommended: no by default. Keep shell-first and show a bounded local cue.
2. Should the assistant answer label include model/account?
   Recommended: default only “AIShe · answer”; verbose/detailed may add model.
3. Should d remain a picker shortcut?
   Recommended: no. All printable input filters; use the post-Enter durable
   promotion prompt.
4. Should # remain force-agent?
   Recommended: compatibility window, primary docs use ?, then explicit
   deprecation decision.
5. Should Bash aim for zsh feature parity?
   Recommended: first make Tier B honest and tested; pursue parity only where
   Bash can implement it without fragile Enter interception.
6. Should theme auto-detection rely on COLORFGBG?
   Recommended: use it only as a hint; provide explicit override and conservative
   unknown fallback.
7. Should usability metrics leave the machine?
   Recommended: no. Use local qualification and opt-in research sessions.

## 20. Definition of done for the milestone

The milestone is done only when:

- P0 findings F-001 through F-006 are resolved.
- One command registry drives or verifies every supported surface.
- RouteDecision and the v1 corpus are live, explainable, and cross-surface.
- Final agent answers and command proposals are distinguishable without color.
- NO_COLOR/plain/JSON output has zero escape bytes.
- Picker viewport, key conflicts, and display-width handling are fixed.
- Compatibility tiers and Doctor capability reporting are published.
- Bash has a real declared-tier harness and macOS has a deterministic PTY CI
  subset.
- status and suggest JSON are versioned with compatibility fixtures.
- active docs match current code and old plans have lifecycle banners.
- main/integration extraction has started only behind characterization and leaves
  public behavior stable.
- required quality, performance, security, installer, and release gates pass.
- release notes explain visible changes and migrations.

## 21. First implementation queue

The recommended first ten reviewable changes are:

1. PICK-001: fix invisible selection with viewport tests.
2. QA-001: reject stale binaries in external harnesses.
3. QA-002: Python ignores and clean shellcheck/syntax gate.
4. UX-001 plus UX-002: lifecycle index and removal of phantom command docs.
5. UI-001 plus the narrow UI-003 fatal-error path: make NO_COLOR truly clean.
6. CMD-001: introduce the registry with current behavior only.
7. CMD-002: replace dispatcher/custom-command meta lookup.
8. ROUTE-001: add reason-bearing RouteDecision with no behavior change.
9. ROUTE-002: land the versioned corpus.
10. UI-004: add a minimal monochrome-safe AIShe final-answer boundary.

This order delivers one immediate correctness fix, makes all later evidence
trustworthy, then establishes the contracts needed for broader UX work.

## 22. What not to redo

The audit found these foundations strong and worth preserving:

- real zsh PTY rather than a shell emulator
- lazy managed backend and direct shell fast path
- checksum-pinned runtime and isolated loopback supervisor
- normalized agent events and bounded model-controlled rendering
- transactional setup/settings with drafts, backups, rollback, and secret-safe
  input
- named connection isolation and shell-local versus durable selection
- deterministic safety matcher that is candid about its limits
- Linux bubblewrap as the actual workspace boundary
- project overlay/command/skill trust model
- redaction, private credential stores, and audit off by default
- durable session recovery, tool idempotency, usage deduplication, and budgets
- undo journal and dry-run architecture
- broad Rust, PTY, runtime-contract, install/upgrade, fuzz, and release tests
- supply-chain checks, checksums, SBOM, and provenance

The next version becomes better by making those systems coherent, visible, and
maintainable—not by replacing them.

[/PRD]
