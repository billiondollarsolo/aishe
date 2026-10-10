> **Lifecycle: Active.** Baseline: main `9219190b663c12b5a402aeab11f32c48e63d868c`,
> reviewed 2026-10-10. This is the implementation contract for the v1.1.0
> adoption and release-readiness milestone. Current behavior remains documented
> in [Getting started](../getting-started.md), [Commands](../commands.md), and
> [Release readiness](../release-readiness.md).

# AIShe adoption and release-readiness plan

## Outcome

Someone coming from Bash or zsh should install AIShe, open a usable shell,
recognize their commands and preferences, and discover agent features without
first buying or connecting a model. Background work should stay quiet until it
needs attention; its permissions, progress, answers, and recorded checks should
remain inspectable. The release must prove ordinary shell performance and
supported behavior on its actual source commit.

This milestone prepares v1.1.0; it does not publish a tag, upload a public
release, change a user's login shell, or claim unperformed provider or terminal
qualification. Release publication is a later operation after its gates pass.

## Baseline and findings

The baseline already has a real interactive zsh, native provider transports,
lazy agent initialization, Ask/Allow/Agent mode grants, persistent background
tasks, Needs you questions and specific approvals, queued/received follow-ups,
recorded checks, revision-bound selective review/apply, workflow dependencies,
budgets, durable interruption/resume, names, pins, and archives. Retain these
features and their permission boundaries throughout this work.

The adoption review found the following concrete problems:

| ID | Observed problem | Consequence |
| --- | --- | --- |
| AD-01 | Fresh shell and ordinary direct commands invoke account setup | Users cannot try the shell independently of AI credentials |
| AD-02 | Personal zsh requires an environment variable; adoption choice is not persistent | Existing prompts, aliases, functions, history, and widgets are hard to keep |
| AD-03 | Setup's recommended profile chooses Allow while documentation promises Ask | First-use behavior and permission expectations disagree |
| AD-04 | First-shell discovery state is consumed without a visible hint | Users never discover help, the slash picker, or mode cycling |
| AD-05 | Bare-slash completion prints approximately thirty catalogue rows | Useful controls disappear above short terminal viewports |
| AD-06 | Unpriced task usage is shown as recorded cost $0.0000 | Users cannot distinguish free work from unknown spending |
| AD-07 | Long task prose precedes authority and evidence; scrolling is undiscoverable | Users cannot readily assess permissions or actual validation |
| AD-08 | Opening details marks work Reviewed immediately | Seen work and deliberately reviewed work are indistinguishable |
| AD-09 | Settings preview promises fields omitted by the native prompt | Configuration previews do not describe the actual shell |
| AD-10 | Installer always installs the optional managed runtime | Native API/local-model adoption incurs an unnecessary roughly 63 MB dependency |
| AD-11 | PATH/setup completion is unclear; a registry installation is advertised without a published crate | New installs can end with an unusable next command |
| AD-12 | Piped multiline scripts are split into requests and CLI shell flags are unsupported | Shell syntax can accidentally become AI input |
| AD-13 | Historical managed integration and the native frontend are presented as interchangeable | Users cannot tell which experience they activated |
| AD-14 | macOS ordinary-command startup exceeds the strict added-overhead SLO | The candidate fails an existing release requirement |
| AD-15 | Release publication has no exact-source functional/performance qualification dependency | A packaged but unqualified commit can become public |
| AD-16 | Public v1.0.0 predates the merged native/task work; package version remains 1.0.0 | Users download a different product from the one reviewed |
| AD-17 | Missing provider token reports still appear as zero cost in native session prompts and historical usage | Task details alone cannot establish trustworthy spending or budget enforcement |
| AD-18 | Untrusted project overlays can switch shell profiles or use the canonical autonomous mode alias | A cloned project can change the user's interpreter/startup expectations without an explicit trust decision |
| AD-19 | Last-command fix/explain helpers are omitted from noninteractive classification | An interactive shortcut can capture a nested shell instead of a correction |
| AD-20 | Absolute-path installer launch leaves internal shell controls dependent on PATH | `/setup` can silently fail before the new executable directory is configured |
| AD-21 | Setup's saved next-step examples use fixed aligned columns | At 32 columns, explanations split into isolated letters and unreadable fragments |
| AD-22 | Settings immediately redraws its menu after showing a prompt preview | On an 18-row terminal, the preview scrolls away before the user can inspect it |
| AD-23 | A specific-action approval prints its full request above an unbounded decision menu | At 32 columns and 18 rows, the exact command scrolls away while the approval choices remain visible |

### Existing evidence and its limits

The review used isolated HOME/config/data/runtime fixtures and actual PTYs at
100, 58, and 32 columns. It did not alter user dotfiles or call paid providers.
The source was clean at the baseline commit. Workspace records are retained at
`/workspace/scratch/aishe-adoption-readiness-review-main921.json`, with hashes
and references to first-install, shell contract, and UI capture evidence.
These paths describe this development session, not portable release artifacts.

Merged-main CI run `37937501671` failed macOS native startup twice. With the
unchanged 100 measured commands and ten warmups, added p95 overhead was
14.780 ms and 13.901 ms against a limit of less than 10 ms. Functional tests
passed. Linux passed the startup requirement. Diagnostic profiles are useful
for locating cost but cannot replace the original gate. The older results
remain historical evidence; new source must get new qualification.

## Product and implementation decisions

### Start with a shell; connect AI when needed

Fresh interactive launch creates or reads safe shell configuration and opens
the ordinary shell. Account connection is optional. Explicit Setup offers
Connect later and explains where to connect later. An AI request without a
working connection returns a short actionable connection message; it does not
pretend to have completed a task. No-provider direct commands remain local and
do not initialize providers, plugins, MCP, or the managed backend.

The recommended first-use mode is Ask. Mode cycling retains its existing
per-shell grants and never treats a stored default as a fresh host permission.
Technical runtime, sandbox, and endpoint diagnostics stay available but do not
precede the basic account/adoption choices on the first setup screen.

### Choose a shell experience explicitly

Persist a shell-profile preference in user configuration and expose it in
Setup and Settings. Offer Keep my zsh, Clean AIShe, and Bash integration with
accurate descriptions. Keep my zsh retains user startup configuration, prompt,
aliases/functions, history, widgets, and keymaps. Clean AIShe retains the
purpose-built prompt and avoids personal interactive startup configuration.
Bash integration advertises its reduced feature tier and uses actual Bash.
An explicit environment override remains available for tests and temporary use.

Activation must be inspectable and reversible. Provide a clear command and
instructions for native terminal startup, with a removable bounded block or
equivalent explicit configuration. Preserve original dotfiles and detect
conflicting/duplicate activation. Do not silently call chsh or add AIShe to
system shell lists. Describe any remaining login-shell limitations precisely.

### Discovery without persistent clutter

Render a short one-time hint with help, slash discovery, and mode cycling.
Persist its seen state only after the shell confirms the hint was displayed.
Failed spawning or suppressed display must leave the hint available.

Bare `/` followed by Tab opens a bounded searchable picker, respecting narrow
and short terminals. Enter stages a selection; it does not execute it. Escape
restores the previous input. Prefix, path, and argument completion continue
through the native shell. Prompt previews must use the fields actually
supported by the native prompt, including honest unavailable usage/cost values.

### Background work that earns trust

Retain quiet task counts and the Needs you inbox. Questions must show which
task needs the answer. Approvals remain bound to the exact action and default
to leaving work waiting. Full action arguments and effective scope must remain
inspectable through a bounded, scrollable review at narrow and short sizes;
choosing to continue that review must not grant permission. Follow-ups show
queued and received states and remain
deliverable to a running task without disrupting the foreground command line.

Task details prioritize status, task identity, model/connection, effective
scope, workspace, budgets/limits, outstanding interactions, and checks actually
recorded before long response prose. Results are bounded or scrollable, with
visible keyboard guidance and a continuation indicator. A model's completion
claim never upgrades failed, stale, or unrun checks.

Record cost coverage, not just a numeric total: known, partially priced, or
unknown. Unknown pricing is n/a; partial totals explicitly identify missing
coverage. Migrate older records without manufacturing price provenance. A
configured money budget cannot silently treat unverified historical spend as
zero when resuming a task. Token/request limits remain independently enforced.
Apply the same coverage distinction to native prompt totals, session tallies,
the content-free usage ledger, and historical usage reports. Missing token
reports differ from explicit reported zero usage. Legacy session/ledger records
keep their observed counts but do not acquire invented coverage. Positive
native session money budgets require verifiable usage and pricing before
admitting another request.
Enforce that requirement within a turn before further provider/tool work as
well as at turn admission. Reject invalid/nonfinite money caps. Accepted
streaming attempts and metered provider failures stay in the content-free
ledger; a parse or read error cannot erase consumption. Native planning
bypasses the suggestion-response cache so reserved task turns retain actual
usage provenance.

An untrusted project overlay cannot change the saved shell profile or select
autonomous Agent mode through either its canonical name or its legacy alias.
The existing explicit trust and per-shell grant checks remain separate.

Opening a result acknowledges it as Seen and may quiet its notification.
Reviewed is an explicit action. Preserve older acknowledged revisions as Seen
when their explicit-review provenance is unavailable; keep names/pins/archives
and workspace isolation. New attention or new result
revisions must become visible again according to the existing task lifecycle.

### Ordinary shell and script semantics

Piped multiline input and explicitly supplied scripts must be executed as
shell programs, preserving control flow, exit status, positional arguments,
and signals. Shell-program input must never be routed line by line to AI.
Keep natural-language requests explicit through documented AIShe request
interfaces. Parse conventional shell flags deliberately; mixed or invalid
arguments must not bypass CLI validation through a fast path. Qualify supported
interactive/login forms before advertising them as replacement behavior.

Keep native AIShe distinct from the historical managed conversation hook in
instructions. Bash's declared Tier B/B- limits, actual tested platform matrix,
and unperformed named terminal/plugin qualifications remain visible.

### Native installation and trustworthy releases

Install the native binary by default. Managed OpenCode is an explicit runtime
choice for OAuth or legacy workflows and can be installed later. Preserve
checksums, private runtime directories, existing runtime upgrades, transactional
rollback, and explicit offline/mirror controls. Make the executable location,
PATH remedy, setup, and launch next steps unambiguous. Advertise only available
distribution channels.

Prepare a distinct 1.1.0 package version, lockfile entry, changelog, and release
notes containing both the merged native/task features and this adoption pass.
Existing public tags and historical evidence are immutable.

Release publication requires successful CI for the exact release source,
including native Linux/macOS functional and original startup gates. Record
manual/live qualification as pass, fail, or not_run; require an explicit scoped
owner/date/expiry disposition for permitted omissions. A missing record or
failed mandatory gate blocks publication. Package-only success is insufficient.

## Work breakdown and acceptance criteria

Tasks are complete only when implementation and their applicable checks pass.
The checkbox status will be reconciled with actual candidate evidence.

### A. Shell-first setup and adoption

- [ ] A1 Remove provider setup from fresh ordinary launch and direct history logging.
- [ ] A2 Add Connect later and an actionable disconnected-AI experience.
- [ ] A3 Persist and expose Clean/Personal/Bash shell choice with temporary overrides.
  Keep shell-profile project overrides behind explicit trust.
- [ ] A4 Align recommended first-use mode with Ask and existing grant boundaries.
- [ ] A5 Simplify first setup screen and retain detailed diagnostics on demand.
- [ ] A6 Add inspectable reversible native activation and conflict/duplicate handling.
- [ ] A7 Qualify isolated fresh-home shell launch, providerless direct commands,
  personal startup preservation, Bash selection, reconnect/setup, settings persistence,
  and internal controls when the executable is outside PATH.
- [ ] A8 Wrap saved setup examples by terminal width while keeping commands and their meaning readable.

### B. Native discovery and prompt consistency

- [ ] B1 Render and acknowledge a real one-time discovery hint.
- [ ] B2 Replace overflowing bare-slash listing with a bounded searchable picker.
- [ ] B3 Preserve staged input, cancellation, prefix/path/argument completion, and concurrent-shell isolation.
- [ ] B4 Align actual prompt fields and Settings previews; indicate unknown usage honestly.
- [ ] B5 Capture real PTY behavior at normal, narrow, and short viewports.
- [ ] B6 Keep prompt previews visible until the user explicitly returns to Settings.

### C. Honest and usable task results

- [ ] C1 Persist price coverage and migrate older task records without false zero-cost claims.
  Cover provider reports, prompt/session tallies, the usage ledger and historical reports.
- [ ] C2 Fail safely on unknown historical spend under a money budget.
- [ ] C3 Show authority/context and recorded check status before long result prose.
- [ ] C4 Add discoverable scrolling and continuation indicators at narrow sizes.
- [ ] C5 Separate Seen from explicit Reviewed while retaining old decisions and quiet archives.
- [ ] C6 Qualify unknown/partial pricing, long results, explicit review, new attention,
  questions/approvals, follow-up receipts, and recorded-check failure/staleness.
- [ ] C7 Keep the full exact-action approval request inspectable at 32/58 columns
  and 18 rows, with explicit continuation, cancellation, and a safe default decision.

### D. Script and command compatibility

- [ ] D1 Execute piped multiline programs as a single ordinary shell program.
- [ ] D2 Support and document explicit scripts/positional arguments and conventional forms within tested scope.
- [ ] D3 Preserve fast-path argument validation, cwd/env/rc, output/status, and signal contracts.
- [ ] D4 Distinguish native frontend, Bash tier, and historical managed integration in documentation.
- [ ] D5 Add meaningful multiline/control-flow/error/argv/no-AI regression checks.
  Qualify last-command fix/explain helpers as noninteractive operations.

### E. Installation and release pipeline

- [ ] E1 Make optional managed runtime installation explicit and preserve transaction safety.
- [ ] E2 Clarify executable/PATH/setup/launch completion and remove unavailable distribution claims.
- [ ] E3 Add native-default/explicit-runtime/offline/fault/rollback installation checks in isolated fixtures.
- [ ] E4 Require exact-source CI and explicit candidate qualification before release publication.
- [ ] E5 Prepare v1.1.0 version, changelog, comprehensive release notes, and user documentation.

### F. Startup and candidate qualification

- [ ] F1 Implement a measured production optimization, preserving shell/security/history behavior.
- [ ] F2 Run the original 100-command/ten-warmup startup gate with no backend start on Linux and macOS.
- [ ] F3 Pass fmt, strict all-target/all-feature clippy, locked MSRV/no-default build, Rust tests,
  shell/installer/docs/reporting contracts, and native/task compatibility regressions.
- [ ] F4 Collect actual candidate commit/tree/version/binary identity with statuses and evidence paths.
- [ ] F5 Review the final diff for permission regressions, stale claims, migrations, and startup behavior.
- [ ] F6 Open reviewable PR work and qualify the resulting main commit after authorized merging.
- [ ] F7 Leave public release publication pending with a concrete qualification record.

### R. Publication qualification and release decision

These tasks remain separate from completion of the A–F implementation milestone.

- [ ] R1 Record source-bound paid-provider qualification, or an explicitly
  accepted owner/reason/risk/expiry disposition. Missing credentials or budget
  remain `not_run`; deterministic supported-provider failures are not passes.
- [ ] R2 Record named graphical terminal/version/OS manual interaction evidence,
  or accepted scoped owner/reason/risk/expiry dispositions. Recorded synthetic
  PTYs do not establish manual emulator passes.
- [ ] R3 Record final-source long-soak evidence, or an accepted
  owner/reason/risk/expiry disposition. Bounded runtime CI does not establish
  a long-duration soak pass.
- [ ] R4 Reconcile the release owner's decision and source-bound record; require
  successful maintained push CI and unexpired Linux/macOS evidence for the
  exact full release commit, including later documentation/decision changes.
  Keep `decision: hold` and `published: false` until publication requirements
  are met. Run the release workflow only for a later authorized publication
  and record its actual outcome separately.

### Implementation and evidence map

| Work | Main implementation | Required evidence |
| --- | --- | --- |
| Fresh shell, profile, setup and settings | `src/config.rs`, `src/setup.rs`, `src/settings.rs`, `src/pty.rs` | `tests/adoption_pty.py`, `tests/setup_pty.py`, `tests/settings_pty.py`, `tests/native_zsh_profile_pty.py` |
| Discovery, slash picker and prompt fields | `src/lean/assets/hook.zsh`, `src/lean/slash.rs`, `src/usagelog.rs`, `src/cli/hints.rs` | `tests/native_discovery_pty.py`, picker/mode/prompt PTYs, real narrow-screen captures |
| Task presentation and interaction truth | `src/cli/taskui.rs`, `src/background/presentation.rs`, `src/background/interactions.rs`, `src/tasks.rs` | Background/interactions/workflow PTYs, saved-revision review and recorded-check regressions |
| Usage and budget provenance | `src/usage.rs`, `src/usagelog.rs`, `src/audit.rs`, `src/lean/nl.rs`, `src/agent/native.rs`, provider transports | Rust budget/coverage/error tests, `tests/native_runtime.rs`, HTTP-backed prompt/task usage scenarios, managed-runtime contracts |
| Scripts and activation | `src/cli/args.rs`, `src/cli/shell_input.rs`, `src/cli/agent_launch.rs`, `src/activation.rs`, command dispatch and executor | `tests/shell_adoption.py`, `tests/direct_shell_startup_test.py`, CLI and signal contracts |
| Installer and publication boundary | `install.sh`, release workflow, `tests/release_gate.py`, `tests/release_evidence.py` | Transaction/fault fixtures; source-bound CI, artifact identity, dispositions and gate regression tests |
| Startup hot path | executor one-shot route and `src/histlog.rs` | Original `tests/direct_shell_benchmark.py` on both platforms, launcher contracts, history permission/concurrency checks |

The exact source commit, clean tree, executable version and SHA-256 must be
recorded with candidate evidence. Local diagnostics and earlier candidate
passes remain labeled with their original source; they are never substituted
for final hosted gates. Named graphical terminals, paid-provider behavior and
long-soak operation are separate publication qualifications.

## Delivery sequence and ownership

The implementation branch is `feat/adoption-release-readiness`. Parallel owners
cover adoption/config/setup, task presentation/pricing, native discovery,
installer/release workflow, startup execution, and script/activation semantics.
The integration owner maintains this plan, coordinates shared interfaces and
Cargo builds, reconciles documentation, reviews the assembled change, and
records candidate qualification. Shared files have explicit ownership; all
work is reviewed together before publication.

Sequence: establish the plan; implement independent work with focused
regressions; integrate shared profile/hint/prompt/cost APIs; build and resolve
failures; run complete applicable qualification; commit and publish PR work;
run exact-source hosted gates; merge only qualified work; validate main; prepare
the publication handoff. Failed checks stay recorded and block readiness until
fixed by a new source change. An unchanged retry is not a fix.

## Completion and release handoff record

At completion append the actual PR/commit, validation manifest, platform
results, remaining release dispositions, and publication status here or link
a candidate-specific qualification record. Do not replace the baseline's
failed evidence with a later pass. Mark this design Implemented only when its
implementation has landed; separate implementation completion from approval
to publish and from actual distribution availability.

### Candidate review iterations

Review work is available in [PR #10](https://github.com/billiondollarsolo/aishe/pull/10).
The first hosted candidate, branch commit
`3253b914d9ef4a8e2e23a58f4cba925b52dfcd81`, was tested as synthetic merge
`e43176bfc7bdb524109900026fa1c487c880307f`; their trees matched exactly.
CI run `38017643029` is failed evidence, not release qualification. Linux native
startup passed with 1.673 ms added p95; macOS failed at 12.628 ms.
The local compile attempt also exhausted generated debug-cache space before
the Rust tests ran. Its manifest retains that failure. Generated caches were
cleared and local debug-info/incremental storage bounded for the next build;
hosted standard-toolchain checks remain mandatory.

This iteration exposed a stale reviewed-wrapper fingerprint, a native task
metadata fixture without its private journal, the nested last-command helper
bug, and an obsolete admin pipe test using implicit per-line force-shell
sigils. Each has a concrete source or contract correction. Further review
closed missing session/ledger cost provenance, project-profile/mode trust,
activation-marker preservation, and durable task connection display gaps.
The next candidate adds a measured history hot-path optimization: assemble one
append record, avoid unnecessary permission writes, and reuse opened-file
metadata while retaining permission repair and trimming. The original macOS
startup requirement must pass on that new source.

Branch candidate `620caea86bb7436724c4d9cf5f8b0b5afb1c31da` was exercised
as synthetic merge `d1ee69047ba9ca71154b2da4adc6da9ef9357a0f`, with equal
trees, in CI run `38019521259`. The original startup tests passed on Linux
(1.717 ms added p95) and macOS (5.702 ms added p95), using the unchanged
100 commands and ten warmups with no managed backend start. This establishes
measured improvement, but the run remains failed functional evidence.
Integration exposed outdated connection-switch and post-response-budget test
expectations, a managed usage-log reader that did not understand v3 coverage,
and a native-session fixture invoking the background-task inspector. Correct
those contracts and qualify the complete next candidate; startup success alone
does not approve release publication.

The full local integration pass also retains the existing orchestration guard:
shell-program admission and guided agent launch belong in CLI domain modules,
keeping `main.rs` below its unchanged 1,500-line limit. Explicit private task
directories must enable real checkpoint persistence in integration harnesses;
an absent directory override or explicit disabling must not write to a user's
default task directory. Activation owns a versioned public JSON contract.
Current-format usage fixtures prove verified subtotals, and legacy fixtures
separately prove unavailable coverage. UI qualification reads plain-text
offsets and explicitly scrolls beyond authority fields to reach long results.

The clean local `8a7df84b56abd77a9890c4b3d77283f1ae57a40a` candidate
passed all twelve core gates, including 1,114 Rust tests across 37 targets.
Its original Linux startup gate passed at 1.841 ms added p95. Actual short-screen
captures then exposed AD-21 and AD-22; those observations require production
UI corrections and new-source qualification. The native run also stopped at a
local harness arrangement error: its copied binary lacked the adjacent release
library needed to compile prompt fixtures. Retain that failed manifest; the
next immutable test profile includes the matching library and dependencies.
These intermediate passes do not qualify the final release source.

The subsequent hosted `a09c705` source retained a macOS startup failure:
11.145 ms added p95 against 10.000 ms, using the original sample and threshold.
Its diagnostic child CPU cost for direct-command overhead (3.936 ms) nearly
equaled executable startup via `--version` (3.962 ms); inheriting active history
and avoiding a config read saved only 0.165 ms. This supports a further
production build change: full link-time optimization and one code-generation
unit, retaining unwinding and platform TLS trust. Measure the resulting binary
size and qualify its original platform startup gates; do not assume a gain or
replace failed evidence with a quieter diagnostic run.
The `--version` diagnostic also constructs the CLI parser; it is not a pure
loader measurement and does not establish a loader-only cost or cause.
Inspection of the actual `8a7df84` Linux release assembly also found an
11,896-byte rich-orchestration stack frame and two page probes allocated before
ordinary-command admission. Keep the same admission logic in a small entry
function and call the rich orchestration only after a miss, preventing that
unnecessary allocation. This is observed instruction-level work removed, not
a claim about its macOS wall-time gain; the unchanged end-to-end gate decides.

The clean `9388e7ee3615c57e4affd3ad1fad29f0875f5fc7` local candidate
passed all recorded core checks (1,114 Rust tests across 37 targets) and all
37 native command checks. Its original Linux startup gate measured 1.312 ms
added p95, with 100 commands, ten warmups and no backend start. Actual setup,
settings and discovery captures confirmed AD-21 and AD-22 were corrected.
An additional focused approval probe then exposed AD-23: at 32 columns and
18 rows, the exact command left the visible decision viewport. The safe default
still left the action waiting, but that is insufficient for usable approval
review. Keep those source-bound observations and qualify the bounded-action
review correction on new source; automated passes alone do not erase this
observed UI failure.

Hosted `9388e7e` CI run `38022810493` completed with ten of eleven jobs
passing. Linux passed the original startup gate at 1.616 ms added p95. macOS
passed its functional checks but failed startup at 10.319 ms against the
unchanged 10 ms limit, so its release identity manifest correctly refused
qualification. The later diagnostic measured roughly 4.850 ms of additional
child CPU; avoiding the config read saved only 0.201 ms. This does not provide
comfortable release headroom. The next production build uses size optimization
(`opt-level = "s"`) with full LTO and one code-generation unit, preserving
unwinding, native TLS trust and the single-file distribution. Record actual
binary size and unchanged platform gate results before claiming improvement.

Candidate `a98a3bc270520ff771de98b57bbe4140e7d38a45` also retains its
earlier compile failure in CI run `38022690647`: the Settings path accessed
`glyphs` as a field rather than calling `glyphs()`. Compilation stopped before
startup measurement; that candidate has no recorded startup pass.
