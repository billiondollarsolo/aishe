> **Lifecycle: Validation evidence.** Current paid-live Linux candidate: AIShe
> v0.6.5 commit `a1b4f4cb52de16c254e95df80168680acbae64d5`, audited from a
> clean Linux x86_64 checkout on 2026-08-02 against `gpt-5.6-luna`. The earlier
> deterministic macOS/Linux foundation remains recorded at `35297d0`; the
> current tree also passed local macOS all-feature and minimal-feature matrices.
> A later documentation-only report commit does not change the qualified
> product binary. This report is evidence for a release decision; it is not
> itself that decision.

# Next product UX and reliability implementation report

This report maps every implementation story and every milestone Definition of
Done item in [the active plan](NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) to concrete
source and observed evidence. It deliberately distinguishes implementation,
local deterministic proof, and platform/release proof.

Status meanings:

- **Verified locally** — implementation exists and a relevant executable gate
  passed during this audit.
- **Verified on candidate** — a clean exact-commit qualification or external
  capability gate passed and recorded source/binary identity.
- **Implemented; external proof pending** — the code or CI gate exists, but an
  acceptance condition needs another platform, service, credential, or clean
  release candidate.
- **Partial** — a stated work or acceptance item is still missing.
- **Decision complete** — the requested research decision was recorded; it does
  not imply that the researched platform or feature is supported.

## Audit identity and observed gates

The deterministic foundation is commit
`35297d04769f4167ea589b5e49998537ee42a51b`. Its macOS and Linux qualification
drivers recorded that full source commit, `dirty: false`, binary identity
`35297d0`, matching checkout verification, the Cargo.lock digest, the pinned
OpenCode 1.18.9 manifest/plugin digests, threat-model version `2026-07-31.1`,
and all corpus digests.

Paid-live investigation changed both product and harness code. The resulting
clean Linux candidate is
`a1b4f4cb52de16c254e95df80168680acbae64d5`. Its report records `dirty: false`,
binary identity `a1b4f4cb`, exact-checkout verification, release binary SHA-256
`3f20f327d48380dd207b97f2df1eb3289bffa651c8bdea70eec6c146bd0390dd`,
the Cargo.lock/corpus digests, and the same pinned OpenCode 1.18.9 runtime and
trusted plugin identities. The credential is represented only by the name
`AISHE_REALTEST_KEY`; copied reports were scanned and contain no configured
secret-pattern material.

| Observed gate | Result in this audit |
| --- | --- |
| Clean macOS release profile | **35 pass, 0 fail, 3 non-required platform skips** in 577,769 ms. Source and binary both identify `35297d0`; binary SHA-256 is `5edf7fbca34a1d70cec8d335964ec36eadf743abccc28bb815cbe34cbdd70f76`. The skips are Linux installer upgrade, Linux credentials, and Linux multiplexer gates. |
| Clean Linux-full profile | **38 pass, 0 fail, 2 non-required paid-live skips** in 898,894 ms on Linux x86_64 with functional `/usr/bin/bwrap`. Source and binary both identify `35297d0`; binary SHA-256 is `05927ad8a09e4d83d6f85a2f608a2a890debb24adc08867986e76a94ba31db94`. |
| Clean Linux paid-live profile | **41 pass, 0 fail, 0 skip** in 2,495,559 ms at exact commit `a1b4f4cb`. It passed every deterministic Linux gate plus paid classification, 84-case standalone real fuzz, and the end-to-end release gate with a real YOLO tool effect and 280-case fuzz. |
| Paid model behavior | Standalone labelled classification passed 19/20 (minimum 16) with zero contract breaches; the independent release-gate repeat passed 20/20. Standalone fuzz passed 84/84 and release fuzz passed 280/280. Provider text/structured/tools/streaming, capability cache, live Doctor, and answer/command JSON contracts passed. |
| Current local macOS feature matrices | At `a1b4f4cb`, all-feature passed 572 library tests plus every integration target; no-default passed 571 plus every target. Strict minimal-feature Clippy, formatting, 66 Python tests, and Ruff passed. This is strong changed-surface proof but not a replacement for a fresh clean macOS release-profile artifact. |
| Rust and dependency paths | The paid Linux profile passed all-target Rust tests, strict all-target/all-feature Clippy, Cargo deny, exact release build, and release identity. Linux also executed the new functional nested-home bubblewrap regression. |
| Python/docs/shell contracts | 66/66 Python unit/contract tests, Ruff, compileall, docs lifecycle/links, generated Bash/Zsh syntax, shellcheck, and the consolidated shell contract passed. |
| Runtime, safety, and resilience | Both profiles passed runtime install/live verify, transactional installer checks, provider contracts, host-scope authority, 20-turn/3-cold-cycle soak, 8-session concurrency, durable resume, PTY scenarios/fuzz/signals, and admin validation. Linux also passed installer upgrade, credential modes, and required functional bubblewrap. |
| Terminal compatibility | Exact-commit macOS and Linux native PTY gates passed; Linux tmux/screen passed. The independent opt-in SSH contract passed to the authorized Linux target with exact remote identity, staged review, 300 ms split escape input, resize propagation, and `TERM=xterm-256color`. |
| Native Bash | The exact Linux Bash 5.3.9 Tier-B report passed all 18 cases. The exact macOS release gate passed its current Bash family; focused macOS Bash 3.2 Tier-B- qualification passed 13 core cases and exactly five declared alternatives. |
| Performance and lazy loading | All enforced thresholds passed. On macOS: direct-shell p95 overhead 4.292 ms (limit 10), route p95 0.000695 ms (limit 1), 1,000-row picker rank p95 0.198618 ms (limit 25), frame p95 0.013235 ms (limit 1), maximum observed RSS 6,544 KiB, default binary 11,223,472 bytes. Lazy shell/help/route/status isolation passed on both profiles. |

The first all-target failure names were:
`backend::bridge::tests::lease_routes_one_call_and_replays_completed_result`,
`backend::bridge::tests::started_call_becomes_outcome_unknown_after_foreground_loss`,
and
`backend::bridge::tests::provider_budget_reserves_caps_and_deduplicates_child_usage`.
They failed with lease-expired/foreground-unavailable errors because a short
test TTL was process-global. `Bridge` now owns the test TTL per instance and a
parallel-isolation regression covers the remediation. The final unloaded local
suite, repeated bridge stress, and both frozen qualification profiles passed.

Linux qualification also found and closed two issues before the final pass:
Rust 1.93's stricter `nonminimal_bool` lint required MSRV-compatible
`Option::is_none_or` lease checks, and an architecture test was reading a real
macOS user config instead of creating hermetic AIShe config/data roots. The
final clean Linux run proves both remediations. Native Bash 5.3 additionally
exposed real job-control/EIO races around child provider calls; the hook now
isolates child monitor mode and returns suggestions through private files for
parent-shell consumption, with Bash-family-aware PTY qualification.

## Paid-live defect discovery and closure

The first paid profile at documentation commit `8dca510` was intentionally
retained as negative evidence: **40 pass, 1 fail, 0 skip**. It proved that the
real model classifier (20 prompts) and 84-case fuzz were already healthy, then
failed the final gate because the noninteractive harness had not created the
per-shell workspace-acceptance marker required by the product. Subsequent
focused live reruns exposed additional layers that deterministic mocks did not:

| Discovery | Type | Resolution and proof |
| --- | --- | --- |
| Live harness omitted workspace authorization | Harness fidelity | `tests/live_contract.py` now creates an exact-shell, private, fsynced workspace marker and `tests/live_release.py` binds its shell identity. The final live YOLO gate passed. |
| Managed structured-capability probe simultaneously requested a plain word and JSON | Product/provider compatibility | `src/capabilities.rs` now gives one unambiguous JSON contract and permits one bounded explicit retry only for malformed structured output. Focused unit tests and live provider/Doctor checks passed. |
| YOLO live prompt named the old native `run_command` tool while the managed default exposes `aishe_run_command` | Harness/API drift | The release prompt now names the managed proxy exactly and reports the sanitized transcript on a missing effect. The real model selected the proxy tool. |
| Linux workspace execution failed when the workspace was nested under `$HOME` | **Product sandbox defect** | `src/sandbox.rs` no longer uses a namespace-created path as a later bubblewrap bind source. It masks home, recreates the mountpoint, and directly rebinds the host workspace. Argument-shape and functional Linux tests prove the workspace is writable while sibling home credentials remain hidden; the paid real tool effect passed. |
| Harness forced a seven-character checkout SHA while Git emitted an eight-character unambiguous binary SHA | Release-integrity harness defect | Identity verification now compares a validated 7–40 lowercase hexadecimal binary prefix to the full checkout commit. Tests accept legitimate 7/8/full prefixes and reject malformed or incorrect values. Exact `a1b4f4cb` qualification passed. |

Two findings remain planning inputs rather than hidden pass conditions:

- The standalone labelled run classified “what is my current working
  directory” as an answer (19/20), while the independent release repeat
  returned `pwd` (20/20). This is action-versus-answer nondeterminism inside an
  already-natural-language request, not a failure of the local
  shell-versus-natural-language router. The gate passed its declared 16/20
  threshold, but repeated critical ambiguity coverage should be strengthened.
- During sandbox diagnosis the real model replied `done` after a failed tool
  result, and focus output called the failed action a “recovered attempt” merely
  because the turn produced a final answer. The harness correctly rejected the
  absent marker, but product completion semantics should not infer recovery
  without a later successful effect. This is the highest-priority open product
  hardening item in the active plan.

## Story evidence

### Truth, qualification, command surface, and routing

| Story | Status | Implementation and observed validation |
| --- | --- | --- |
| UX-001 | **Verified locally** | Lifecycle catalog and per-document banners are in [README.md](README.md); `tests/docs_contract_test.py` rejects missing labels/index entries and invalid repository-relative paths/GitHub-style anchors. Its 2/2 tests passed and CI/qualification require it. |
| UX-002 | **Verified locally** | `docs/architecture.md`, `docs/commands.md`, and `docs/development.md` now describe schema 7, the `src/cli` seam, registry/tombstones, and current front ends. `product_help::tests::commands_markdown_matches_the_generated_registry_block` and command-surface tests passed. Reedline appears only in explicitly historical/compatibility context. |
| QA-001 | **Verified on candidate** | `tests/harness_identity.py` is used by external binary harnesses; `tests/qualify.py` builds and verifies before harness execution. It now validates a variable-length 7–40 hexadecimal binary prefix against the full checkout commit, avoiding both stale binaries and legitimate short-SHA collisions. The paid profile recorded matching source/binary identity, clean state, and artifact digest. |
| QA-002 | **Verified on candidate** | `.gitignore` covers Python caches; CI and qualification run shellcheck and generated hook syntax. The consolidated shell contract passed on both clean candidate checkouts. |
| QA-003 | **Verified on candidate** | `tests/admin_validation.py` uses registry/tombstone semantics and `corpus_version`; semantic command tests do not depend on brand prose. Admin validation passed in deterministic and paid profiles. Paid classification passed 19/20 standalone and 20/20 on repeat with zero contract breaches; live fuzz passed 84/84 and 280/280. |
| CMD-001 | **Verified locally** | `src/command_surface.rs` defines identity, aliases, help, arguments, surfaces, output, side effects, handoff, lifecycle, and tombstones. Registry uniqueness/completeness tests passed. |
| CMD-002 | **Verified locally** | `src/dispatcher.rs` and `src/cli/runtime.rs` query the registry. `tests/command_surface.rs` proved built-in reservation, absolute paths, custom-command precedence, unavailable guidance, and tombstones. |
| CMD-003 | **Verified locally** | `src/integration.rs` renders hook dispatch from registry identities/actions. Generated coverage, no-manual-case-table, syntax, zsh, and Bash tests passed. |
| CMD-004 | **Verified locally** | `src/product_help.rs`, completion generation, `docs/commands.md`, and registry metadata are conformance-tested. Seven command-surface tests passed. |
| ROUTE-001 | **Verified locally** | `src/dispatcher.rs` defines `RouteKind`, `RouteReason`, `RouteSource`, and bounded `RouteDecision`/diagnostics while retaining the compatibility adapter. Dispatcher reason/property tests passed. |
| ROUTE-002 | **Verified on candidate** | `tests/fixtures/routing/v1.json` separates normative/research cases with platform and criticality metadata. Rust/generated-zsh corpus gates, admin validation, native PTY, Linux multiplexer, and SSH paths passed on the exact clean candidate. |
| ROUTE-003 | **Verified locally** | A declarative question table drives the generated predicate in `src/integration.rs`; Rust tests prove shared highlight/submit use and execute the versioned corpus against generated zsh. No model call exists in the predicate. |
| ROUTE-004 | **Verified locally** | `aishe route` is implemented by `src/cli/backend.rs` over bounded route diagnostics. Corpus tests proved known-command, forced-agent, forced-shell, stable schema-v1 JSON, no ANSI, and no config materialization. |
| ROUTE-005 | **Verified locally** | `tests/fixtures/routing/typo-assistance-v1.json`, `dispatcher::typo_assistance`, and `cli::runtime::intercept_hook_typo` implement a local, non-executing, rate-limited cue before provider/backend/MCP construction. Corpus and hidden-hook integration tests passed; the documented false-positive budget is 1%. |
| ROUTE-006 | **Verified locally** | `docs/route-prefixes.md` makes `?` canonical and gives `#` a documented removal window; corpus, CLI, and generated-zsh tests cover comment/legacy behavior and bounded notice. |

### Picker, terminal UI, accessibility, and interaction

| Story | Status | Implementation and observed validation |
| --- | --- | --- |
| PICK-001 | **Verified locally** | `src/promptui.rs` follows selection with a terminal-row viewport and position/match count. Tests cover 0/1/20/21/100/1,000-row behavior and visible focus. |
| PICK-002 | **Verified locally** | Printable `d`, `j`, and `k` filter; arrows, Ctrl-P/N, Home/End, and page keys navigate; promotion remains a separate safe-default prompt. Picker key/help tests passed. |
| PICK-003 | **Verified locally** | `src/ui.rs` centralizes grapheme/cell width, wrapping, and truncation. Wide, combining, emoji, ANSI, hostile-control, and target-width tests passed. |
| PICK-004 | **Verified locally** | `promptui::static_filter_picker_io` provides injectable numbered paging/filtering with direct-form non-TTY guidance. Its I/O acceptance passed page navigation, filtering, default/number selection, cancel, EOF, bounds, and absence of cursor controls. |
| PICK-005 | **Verified locally** | Production picker ranking uses exact/token-prefix/substring/fuzzy stages with stable identity/ties; ranking and 1,000-row production-path tests passed. |
| UI-001 | **Verified locally** | `src/ui.rs` owns TTY, NO_COLOR, TERM=dumb, depth, background, Unicode, motion, dimensions, and machine-output policy. Resolution/precedence tests passed; production NO_COLOR reads are confined to UI or shell-template boundaries. |
| UI-002 | **Verified locally** | `StyleToken`, `Glyphs`, depth/theme palettes, and semantic render helpers live under `src/ui.rs` and `src/ui/render.rs`. Palette/depth/plain snapshots and ANSI-stripped distinction tests passed. |
| UI-003 | **Verified locally** | Direct Rust styling routes through semantic capabilities; machine output forces plain/static. UI, CLI error, route JSON, API fixture, setup/PTY, and agent-output tests provide raw-byte coverage; the agent UI PTY emitted zero ESC bytes under NO_COLOR/TERM=dumb. |
| UI-004 | **Verified locally** | Shared answer headers are used by native suggest and managed renderer paths. `tests/agent_ui_acceptance.rs` proved one monochrome boundary, copyable code/body, density parity, and one piped body. |
| UI-005 | **Verified locally** | Pure `ProposalView`/`ApprovalView` renderers drive suggest and tool approval panels with safety, authority, defaults, bounds, and escaped model text. Snapshot and terminal cancel/EOF/resize tests passed. |
| UI-006 | **Partial after live contradiction** | `src/agent/renderer.rs` distinguishes phases, failures, reconnect, changed files, and waiting, and deterministic renderer tests pass. Paid diagnosis showed that a failed tool followed only by a model final answer is currently relabelled “recovered” even when no later effect succeeded. AGENT-001 in the active plan must close this truthfulness gap. |
| UI-007 | **Verified locally** | Streaming prints one authorship structure for short/long responses and pipes receive one body; destructive rerender is capability-gated. Suggest streaming and agent acceptance tests passed without duplication. |
| UI-008 | **Verified locally** | Config schema 7 adds theme/depth/Unicode/motion, settings/provenance, dark/light highlighting, ANSI degradation, and migration backup. Config/CLI migration and palette contrast tests passed; `docs/accessibility.md` records the maintained contrast review. |
| A11Y-001 | **Verified locally** | Semantic snapshots and ASCII glyph tests retain textual/glyph distinctions for route, mode, selection, proposal, approval, warning, danger, success, diff, and error after ANSI stripping. |
| A11Y-002 | **Verified locally** | Static motion and ASCII policies are implemented in UI, promptui, renderer, logo, and Doctor reporting. Policy/layout tests prove no ANSI/symbol dependence, and the static picker I/O acceptance proves a cursor-motion-free interaction path. |
| A11Y-003 | **Verified locally** | `ui::tests::terminal_layout_matrix_bounds_wide_combining_and_hostile_text`, picker viewport tests, renderer tests, `tests/agent_ui_pty.py`, and terminal transport tests cover narrow/wide content, control text, and resize without panic or invisible focus. Emulator-specific manual checks remain separate and `not_run`. |
| A11Y-004 | **Verified locally** | `/help session`, `docs/shell-integration.md`, `docs/accessibility.md`, and Doctor key-conflict diagnostics document active alternatives and Meta/Option caveats. Help/diagnostic/picker-key tests passed. |
| INT-001 | **Verified on candidate** | Generated zsh stages suggestions into `BUFFER`; Bash 5 offers `READLINE_LINE`/recall with declared 3.2 differences. Exact-commit macOS/Linux PTY scenarios, Linux tmux/screen, SSH staging, Linux Bash 5 Tier B 18/18, and the declared Bash 3.2 alternatives passed. Linux 5.3 job-control/EIO regressions are now explicit harness cases. |
| INT-002 | **Verified locally** | Shared bounded waiting panels include task/agent/shell state and restore status. Rust tests plus `tests/agent_ui_pty.py` passed long/multiple questions, reconnect, no color, resize, EOF/Esc/Ctrl-C fail-closed behavior. |
| INT-003 | **Verified locally** | `src/user_error.rs`, provider recovery mapping, `src/cli/error_contract.rs`, and failure-hint paths provide stable bounded/redacted recovery. Error/provider/UI snapshots and bare-route diagnostics passed; ONB-004 separately proves the troubleshooting index is complete. |
| INT-004 | **Verified locally** | Renderer phases are connecting/planning/acting/waiting/recovering/finalizing with bounded focus output and truthful command/file/recovery/usage summaries. Renderer and acceptance tests passed. |

### Errors, automation, compatibility, architecture, and hardening

| Story | Status | Implementation and observed validation |
| --- | --- | --- |
| ERR-001 | **Verified locally** | `src/user_error.rs` defines schema-v1 codes, namespace exit mapping, retryability, next action, redaction/bounds, text/JSON rendering, and the agent bridge. Unit snapshots and API compatibility fixtures passed. |
| ERR-002 | **Verified locally** | `src/cli/error_contract.rs` inventories 20/20 structured common fatal paths (100%) and exactly two owned internal-only exclusions with static source evidence. Focused and full tests passed. |
| API-001 | **Verified locally** | `src/cli/json_contract.rs` and `docs/automation.md` authoritatively inventory all 23 public JSON/JSONL command paths, including schema, format, stdout/stderr, exits, bounds/privacy, migration, and ownership. The static gate counts every Clap JSON declaration and rejects missing, duplicate, zero-version, or misrouted entries. |
| API-002 | **Verified locally** | All 23 public paths now emit explicit versions. Existing v1/v2 documents retain their meaning; backend lifecycle, models, config/settings, connection/auth, readiness, and audit replay use additive versions or named v1 wrappers. Suggest retains 0/20/1 behavior, machine streams stay ANSI-free, and legacy audit objects normalize to v1 while invalid/unsupported rows are skipped. |
| API-003 | **Verified locally** | `tests/fixtures/api/{v0.5,v0.6,v1}` preserve both prior-minor status/suggest and every legacy inspection-family root shape. `tests/api_compat.rs` passed 6/6, the 23-path inventory passed 3/3, CLI contracts passed 50/50, and migration notes name every wrapper field. |
| COMP-001 | **Verified locally** | `docs/front-ends.md`, `docs/shell-integration.md`, `docs/bash-compatibility.md`, and Doctor report Tier A/B/B- and effective terminal/sandbox behavior. Diagnostic tests passed. |
| COMP-002 | **Verified on candidate** | `tests/bash_hook.py` is a real isolated interactive harness with 18 declared rows and strict 3.2 differences. Linux Bash 5.3.9 passed Tier B 18/18 at exact commit `35297d0`; the macOS release gate passed its current family, and focused Bash 3.2.57 evidence passed 13 core cases plus exactly five declared Tier-B- alternatives. The harness uses separate legacy/modern PTY drivers and exercises job-control recovery. |
| COMP-003 | **Implemented; external proof pending** | `.github/workflows/ci.yml` contains a blocking bounded `macos-latest` PTY job for terminal latency/resize, routing, picker, status, setup, and signals. Configuration is inspectable, but this report cannot claim a remote CI execution of the final tree. |
| COMP-004 | **Verified on candidate within declared scope** | `tests/terminal_compat.py` records pass/fail/limitation/unsupported. Candidate evidence passes local macOS/Linux, Linux tmux/screen, and the opt-in SSH contract to one authorized Linux target. Named emulator rows remain `not_run`, never pass, and one target does not establish universal SSH compatibility. |
| COMP-005 | **Decision complete** | `FISH_INTEGRATION_DECISION.md` declines a native Fish hook this milestone; `WSL_COMPATIBILITY_DECISION.md` declines a support/artifact claim before real WSL2 qualification. User docs preserve those boundaries. |
| ARCH-001 | **Verified locally** | `src/main.rs` is 804 lines; binary-owned Clap remains in `src/cli/args.rs`, while status, connection, session, history, backend, settings, runtime, hints, JSON/error contracts are bounded `src/cli` modules. Architecture/CLI tests passed. |
| ARCH-002 | **Verified locally** | Pure terminal primitives live in `src/ui.rs`/`src/ui/render.rs`; promptui is a compatibility facade. The architecture test rejects business/prompt dependencies and renderer snapshots passed. |
| ARCH-003 | **Verified locally and on Linux** | `src/integration.rs` is a 69-line orchestrator over six reviewed assets plus focused `registry`, `templates`, and `tests` modules. Exact pre-extraction SHA-256 snapshots, typed exact-one substitution, registry fragments, shellcheck, generated/asset syntax, and 35 focused integration tests passed locally and in the isolated Ubuntu fixture. |
| ARCH-004 | **Verified locally** | `docs/legacy-compatibility.md`, schema-7 migration, tombstones, backups, and legacy-task support windows inventory/remove reedline-era fields deliberately. Migration/backup, help, tombstone, and persisted-task tests passed. |
| SAFE-001 | **Verified locally** | `!` emits a non-color one-line `shell override` cue, route diagnostics state the safety bypass, and CLI/route tests prove one-line non-sticky behavior and no model route. |
| SAFE-002 | **Verified on paid Linux candidate** | Approval views show mode, effective scope, network, sandbox implementation, command/effect/reason, and safe negative default. Linux nested-home bubblewrap now mounts the workspace correctly while masking sibling home state; functional sandbox tests and a paid real YOLO effect passed. Linux host and macOS policy-only text remain distinct without color. |
| SAFE-003 | **Verified on candidate** | `SECURITY.md` carries threat-model version/date and limitations; both clean profiles record its digest, runtime/plugin/corpus digests, sandbox facts, and the functional Linux evidence gates against threat-model version `2026-07-31.1`. |
| SAFE-004 | **Verified locally** | `docs/data-retention.md`, category-specific uninstall selection/preview, Doctor retention checks, bounds/rotation, and preservation defaults cover local state. Diagnostics, uninstall, CLI, audit, undo, history, and task tests passed. |
| SAFE-005 | **Verified locally** | `tests/boundary_fuzz.rs`, route/safety corpora, runtime archive seeds, shell-selection parsing, terminal/error sanitization, and SSE/JSON bounds are deterministic regression gates; boundary tests passed. |
| DEP-001 | **Verified locally** | Cargo metadata and `tests/advisory_policy_test.py` require one `crossterm` version; the observed graph contains only 0.29.0. |
| DEP-002 | **Verified on candidate** | `Cargo.toml`/`Cargo.lock` contain only `ureq` 3.3.0 and the ureq-2 advisory exception is gone. Deterministic provider/client tests and fresh macOS/Linux runtime/provider/live-contract qualification passed. |
| DEP-003 | **Verified locally** | `deny.toml` records owner, added/review dates, target removal, and rationale; the advisory-policy test rejects missing/expired metadata and passed. |
| PERF-001 | **Verified on candidate; host metrics informational** | `tests/performance_benchmark.py` and `examples/performance_probe.rs` emit schema-v1 shell/route/picker/render/RSS/size/backend evidence. Both profiles passed; final macOS enforced values were 4.292 ms direct-shell p95 overhead, 0.000695 ms route p95, 0.198618 ms 1,000-row rank p95, and 0.013235 ms frame p95. Host-sensitive values remain correctly labeled informational. |
| PERF-002 | **Verified on candidate** | `tests/lazy_loading_test.py` instruments provider construction, backend state, listeners, and network attempts for shell/help/route/status. The required gate passed in both final profiles. |
| REL-001 | **Verified on candidate** | `tests/qualify.py` defines quick/local-full/linux-full/release/paid-live profiles, builds and verifies identity first, records commands/durations/skips/digests/corpora, and never calls required skips pass. Final paid Linux was 41/0/0 at exact clean commit `a1b4f4cb`; earlier macOS release was 35/0/3 and Linux-full 38/0/2. |
| REL-002 | **Verified locally as policy; release hold narrowed** | `docs/release-readiness.md` defines pass/fail/not_run/waiver/hold, required evidence, binary/state rollback, and failed rollout response. Paid-live is now pass rather than `not_run`. A fresh clean macOS release artifact for the changed commit, hosted CI/package provenance, and ONB-001 evidence or explicit dispositions remain external release inputs. |
| ONB-001 | **Partial** | `src/setup.rs` has a verified-connection short path, remaining-step UI, and explicit token/cost disclosure; setup tests cover transactional behavior. No recorded new-user timed walkthrough/usability evaluation proves the story's evaluation work or first-answer journey. |
| ONB-002 | **Verified locally** | Connection/model/reasoning/mode/output copy consistently distinguishes “this shell” from “default for new shells”; one post-selection promotion remains. Picker PTY point evidence and CLI/config tests prove no accidental rewrite. |
| ONB-003 | **Verified locally** | `src/hints.rs` stores only schema/booleans in atomic mode-0600 local state; config/settings expose disable/reset. Launch, first-answer, failure, and typo cues are bounded/rate-limited. Unit, architecture, and discovery reset/privacy tests passed. |
| ONB-004 | **Verified locally** | `docs/troubleshooting.md` maps every common ERR-002 code to one exact recovery row and keeps support-bundle privacy guidance visible. `cli::error_contract::tests::every_common_public_error_code_has_one_troubleshooting_entry` and the Markdown path/anchor gate passed. |

## Findings F-001 through F-024 disposition

Each audited finding appears exactly once below. “Resolved locally” means the
implementation and cited deterministic contract passed; it does not replace
the separate frozen-candidate/platform release evidence described later.

| Finding | Current disposition | Implementing stories and evidence |
| --- | --- | --- |
| F-001 — picker selection can move off-screen | **Resolved locally** | PICK-001; `src/promptui.rs` selection-following viewport plus required-count/terminal-budget tests, including 20/21/100/1,000 rows. |
| F-002 — slash-command truth is split | **Resolved locally** | CMD-001 through CMD-004; `src/command_surface.rs`, generated hook registry fragments, `src/product_help.rs`, and `tests/command_surface.rs` reserve/tombstone/conformance tests. |
| F-003 — Rust/zsh routing duplication and Bash difference | **Resolved on candidate with declared tiers** | ROUTE-001 through ROUTE-003 and COMP-001/002; declarative route rules/corpus in `src/dispatcher.rs`, generated predicates in `src/integration`, `tests/routing_corpus.rs`, and exact-commit macOS/Linux Bash and PTY qualification. |
| F-004 — NO_COLOR/TERM=dumb ANSI leaks | **Resolved locally** | UI-001 through UI-003; centralized `src/ui.rs`, semantic output paths, raw-byte CLI/API/agent assertions, and zero-ESC `tests/agent_ui_pty.py`. |
| F-005 — stale active-looking plans/docs | **Resolved locally** | UX-001/002; lifecycle [index](README.md), corrected architecture/commands docs, and `tests/docs_contract_test.py` lifecycle/path/anchor gate. |
| F-006 — stale binary can enter qualification | **Resolved locally** | QA-001 and REL-001; `tests/harness_identity.py` plus build-before-identity ordering and stale/mismatch rejection in `tests/qualify_test.py`. |
| F-007 — final answers blend into scrollback | **Resolved locally** | UI-004/UI-007; shared answer boundary in UI/native/managed paths and `tests/agent_ui_acceptance.rs` short/long/plain/piped assertions. |
| F-008 — no semantic terminal design system | **Resolved locally** | UI-001/UI-002 and ARCH-002; `StyleToken`, `Glyphs`, capability palettes, pure `src/ui/render.rs` views, and palette/plain snapshots. |
| F-009 — dark truecolor-only syntax highlighting | **Resolved locally; emulator review remains manual** | UI-008; schema-7 theme/depth preferences, dark/light/plain code paths in `src/modes/mod.rs`, settings/provenance, palette tests, and recorded contrast review in `docs/accessibility.md`. |
| F-010 — scalar-count width errors | **Resolved locally** | PICK-003; grapheme/cell-aware wrap/truncate/measurement in `src/ui.rs` with combining/CJK/emoji/ANSI/narrow-layout tests. |
| F-011 — printable picker keys conflict with filtering | **Resolved locally** | PICK-002; promptui key contract makes printable `d/j/k` filter and uses arrows/Ctrl-P/N/Home/End/Page keys; key/help tests passed. |
| F-012 — suggest edit abandons native shell editing | **Resolved for declared tiers** | INT-001; zsh `BUFFER` staging and Bash `READLINE_LINE`/recall in `src/integration/assets`, characterized by `tests/pty_scenarios.py` and `tests/bash_hook.py`. |
| F-013 — routing lacks explanation/correction | **Resolved locally** | ROUTE-004/005; schema-v1 `aishe route`, bounded reason/override evidence, typo corpus, non-executing local cue, and route/architecture tests. |
| F-014 — hidden `#` force-agent conflicts with comments | **Resolved by explicit lifecycle** | ROUTE-006; [prefix policy](../route-prefixes.md), corpus/CLI/generated-zsh migration tests, canonical `?`, and documented removal window while Bash keeps comment syntax. |
| F-015 — help is not generated from executable capability | **Resolved locally** | CMD-004; registry-backed `src/product_help.rs`, docs fragment conformance, completions/root-help checks, and tombstone guidance tests. |
| F-016 — error presentation is partly normalized | **Resolved locally for milestone common paths** | ERR-001/002, INT-003, ONB-004; `src/user_error.rs`, 20/20 common fatal inventory, owned internal allowlist, recovery snapshots, and exactly-one troubleshooting-row test. |
| F-017 — compatibility claims exceed tested parity | **Resolved as tiered product truth** | COMP-001 through COMP-003; Doctor/front-end/Bash docs, strict real Bash harness, exact candidate macOS/Linux/SSH evidence, bounded macOS PTY CI definition, and explicit B/B- differences. Hosted CI execution remains a release-process input, not an overclaimed compatibility fact. |
| F-018 — public JSON is inconsistently versioned | **Resolved for inventoried public surfaces** | API-001 through API-003; [automation inventory](../automation.md), versioned status/suggest/error/route contracts, prior-minor fixtures, and `tests/api_compat.rs`. |
| F-019 — main/integration ownership concentration | **Resolved locally** | ARCH-001/003; `src/main.rs` is 804 lines and `src/integration.rs` 69 lines over focused CLI modules, six reviewed shell assets, templates/registry modules, byte snapshots, syntax, and architecture tests. |
| F-020 — long streamed answers have inconsistent anatomy | **Resolved locally** | UI-004/UI-007; one answer boundary/body contract for short, long, and piped responses with no duplicate text in suggest and agent acceptance tests. |
| F-021 — accessibility preferences are implicit | **Resolved locally** | UI-008 and A11Y-001/002; explicit theme/depth/Unicode/motion config, static/ASCII paths, Doctor reporting, settings provenance, and non-color/static tests. |
| F-022 — autonomous risk state lacks clarity | **Mostly resolved; completion truth follow-up open** | UI-005 and SAFE-002 expose effective mode/scope/network/sandbox/effect/default and distinguish policy-only/isolated/host authority. Paid diagnosis contradicted the deterministic recovery assumption: final prose after a failed action is not proof of recovery. AGENT-001 owns the remaining outcome-truth fix. |
| F-023 — duplicate dependencies and unowned exceptions | **Resolved on candidate** | DEP-001 through DEP-003; single crossterm 0.29 and ureq 3.3 stacks, owned/dated syntect advisory exception, advisory-policy gate, dependency policy, and macOS/Linux live runtime/provider qualification. |
| F-024 — Python cache hygiene omission | **Resolved locally** | QA-002; `.gitignore` covers Python/pytest/coverage/virtualenv artifacts and CI also enforces shellcheck/generated-shell syntax. |

## Definition of Done mapping

| Milestone condition | State | Concrete evidence or residual |
| --- | --- | --- |
| P0 findings F-001 through F-006 resolved | **Candidate proven** | Viewport, registry, route contract/conformance, semantic plain output, corrected active docs/lifecycle/link gate, and stale-binary rejection passed in clean qualification. |
| One command registry drives/verifies every supported surface | **Candidate proven** | `src/command_surface.rs`, generated hook fragments, product-help/doc/completion conformance, and command-surface tests passed on both platforms. |
| RouteDecision and v1 corpus live, explainable, cross-surface | **Candidate proven** | Rust/generated-zsh corpus, route CLI, native Bash, Linux multiplexer, and opt-in SSH paths passed at exact deterministic commit `35297d0`; routing/admin/PTY gates passed again in the paid `a1b4f4cb` Linux profile. |
| Final answers and proposals distinguishable without color | **Candidate proven** | UI snapshots, agent acceptance, focus-output, NO_COLOR, PTY, host-scope, and SSH staged-review contracts passed. |
| NO_COLOR/plain/JSON has zero escape bytes | **Candidate proven** | UI/error/API/raw PTY assertions, JSON fixture inventory, and both platform profiles passed. |
| Picker viewport, key conflicts, display width fixed | **Candidate proven** | Promptui viewport/key/ranking, static picker I/O, grapheme/cell tests, and final picker PTY gate passed. |
| Compatibility tiers and Doctor reporting published | **Candidate proven** | Front-end/Bash/terminal docs, diagnostic tests, exact Bash/Linux transport reports, and platform profiles passed. |
| Real Bash tier harness and deterministic macOS PTY CI subset | **Implemented and candidate-proven** | Real Bash 5.3 Tier B passed 18/18 on Linux, macOS current-family and focused 3.2 Tier-B- gates passed, and the bounded CI subset is configured. Hosted CI execution remains an external release-process input. |
| All 23 public JSON/JSONL paths versioned with fixtures | **Candidate proven** | Static inventory 3/3, API compatibility 6/6, CLI contracts 50/50, audit tests, provider JSON process, and both profiles passed. |
| Active docs current; old plans bannered | **Candidate proven** | Lifecycle/link checks and command-doc conformance passed in both profiles. |
| Main/integration extraction started only behind characterization and stays stable | **Candidate proven** | Main is 804 lines; integration is a 69-line orchestrator over reviewed assets/modules. Architecture, snapshots, shell contract, Bash, and platform qualification passed. |
| Required quality, performance, security, installer, release gates pass | **Paid Linux candidate proven; current macOS release artifact pending** | Paid Linux passed 41/0/0 with clean identity, runtime/install/upgrade, functional sandbox, performance, soak/concurrency/resume, fuzz/signals, 19/20 then 20/20 labelled classification, 84/84 standalone fuzz, and 280/280 release fuzz. Current macOS all/minimal feature matrices pass, but a fresh clean release-profile artifact for the changed candidate remains a release-process input. |
| Release notes explain visible changes/migrations | **Implemented** | `CHANGELOG.md` Unreleased covers registry/routing, UI, JSON, compatibility, migration, dependencies, hints, and safety changes. |

The original engineering milestone and paid Linux gate are complete for the
current functional candidate. **Release readiness remains HOLD**, but the
paid-live hold is closed. Remaining release-process evidence is a fresh clean
macOS release profile for the changed commit, hosted CI/package provenance, and
an ONB-001 new-user observation or explicit maintainer disposition. Named
terminal-emulator rows remain `not_run` unless broader emulator-specific claims
are desired. AGENT-001 is also a known product-hardening decision: a failed tool
must not be labelled recovered solely because the model later emits a final
answer. No waiver is implied.

## Platform, remote, and point-in-time evidence

The final candidate artifacts below are exact-commit evidence. Files under
`test-results/` are intentionally ignored working artifacts; retain them in the
release evidence store if this candidate advances.

| Evidence | What it proves | Limitation |
| --- | --- | --- |
| `test-results/qualification-release-35297d0-macos.json` | Clean macOS arm64 release: 35 pass, 0 fail, 3 Linux-inapplicable skips; exact identity/digests recorded | Does not substitute for Linux-only or paid-live gates. |
| `test-results/qualification-linux-35297d0.json` | Clean Linux x86_64 full: 38 pass, 0 fail, 2 paid-live skips; installer upgrade, credentials, functional bubblewrap, Bash, tmux/screen all passed | Paid model and model-fuzz were `not_run`; test host is one Linux distribution/kernel. |
| `test-results/qualification-paid-live-a1b4f4cb-linux.json` | Clean current Linux x86_64 paid-live profile: 41 pass, 0 fail, 0 skip; exact source/binary/runtime/plugin/corpus identities recorded | One authorized Linux host and one provider/model configuration; not a fresh current macOS or hosted-package run. |
| `test-results/live-release-20260802T175505Z.md` | End-to-end live release: provider capabilities, cache, Doctor, answer/command contract, actual sandboxed YOLO tool effect, 20 labelled classifications, and 280/280 fuzz cases | Model behavior remains probabilistic; retain the structured qualification JSON as the release authority. |
| `test-results/real-model-20260802T172922Z.md` and `test-results/real-model-20260802T173632Z.md` | Independent labelled runs scored 19/20 and 20/20 with zero contract breaches, exposing one repeat-sensitive current-machine-state ambiguity | The declared gate minimum is 16/20; passing does not erase the LLM-001 consistency task. |
| `test-results/real-fuzz-20260802T173448Z.md` and `test-results/real-fuzz-20260802T175505Z.md` | Standalone 84/84 and release-scale 280/280 adversarial live fuzz passed | Seeded corpus on one model/endpoint, not universal provider proof. |
| `test-results/qualification-paid-live-8dca510-linux-failed.json` | Retained negative paid evidence: 40 pass, 1 final-gate fail due to missing per-shell workspace acceptance | Superseded by the current 41/0/0 report; retained to preserve the diagnostic chain. |
| `test-results/bash-hook-linux-35297d0.json` | Exact Linux Bash 5.3.9 Tier B 18/18, including SIGINT/SIGTSTP, history, staging, ERR-trap chaining, and cleanup | Bash 3.2 is not expected on this Linux host; its declared B- behavior was tested on macOS. |
| `test-results/terminal-compat-ssh-35297d0.json` | Exact opt-in SSH PTY contract: identity, staged Enter, 300 ms split escape, 80x24→120x40 resize, and TERM propagation | Proves only the authorized target/authentication/transport configuration used. Identity-file and target details are not embedded as capability evidence. |
| `test-results/performance.json` | Exact macOS performance profile with all enforced thresholds passing | RSS, binary size, long render, initial prompt, and backend host metrics remain informational by policy. |
| `test-results/qualification-linux-13ad479-failed.json` | Retained negative evidence from the first frozen Linux attempt: missing host prerequisites plus real picker/Bash/harness issues | Superseded as release authority by the clean final pass; retained to show failures were diagnosed rather than erased. |
| `.github/workflows/ci.yml` | Linux and macOS gates are configured, including bounded macOS PTY and Linux Bash/terminal jobs | Configuration is not evidence that a particular commit/job passed. |

Explicitly not proven here: a fresh clean current-commit macOS release profile,
hosted macOS/Linux CI and package/provenance execution, other paid providers or
models, WSL, native Windows, Fish integration, authorized SSH configurations
beyond the one recorded target, an actual new-user ONB-001 observation, or the
named manual terminal-emulator matrix beyond generic palette/layout and
transport automation.

## ONB-001 timed walkthrough protocol and record template

This protocol is a template, not an observation. Do not mark ONB-001 complete
until a filled record identifies whether the run was an actual first-time user
observation or only a maintainer dry run. Do not retain credentials or an
unredacted transcript.

### Candidate and participant record

| Field | Required recorded value |
| --- | --- |
| Observation status | `not_run`, `maintainer_dry_run`, or `new_user_observation` |
| Candidate identity | `aishe --version`, git commit/dirty state, release-binary SHA-256 |
| Environment | OS/architecture, terminal, shell/version, width, theme/color policy |
| Participant | anonymous participant code; prior AIShe use `yes/no`; terminal experience `new/intermediate/expert` |
| Connection path | `already_authenticated` or `new_authentication`; provider/auth kind without credential value |
| Researcher help | timestamped prompts/hints given; use `none` when no help was needed |
| Privacy | confirm isolated XDG roots, no command echo containing a credential, transcript redacted/deleted |

### Reproducible preparation

1. Freeze and build the exact release candidate; pass the binary-identity gate
   before observation. Record its digest.
2. Create isolated empty HOME, XDG config/data/cache, runtime, and history
   roots. Do not reuse the maintainer's AIShe state. Preserve only the redacted
   result record afterward.
3. For the `already_authenticated` path, make one supported credential available
   through its documented environment/profile mechanism before first launch;
   never paste it into the transcript. For the `new_authentication` path, let
   the participant follow the normal setup/auth UI.
4. Start screen/terminal recording only if the participant consents. Otherwise
   record checkpoint times and observations manually. In either case scan and
   redact before retention.

### Timed tasks

Start the clock immediately before the participant first runs AIShe. Do not
point them to the README unless they choose help or become blocked.

| Checkpoint | Record |
| --- | --- |
| First launch | start time; whether setup purpose and remaining steps were understood |
| Connection selected | elapsed time; path chosen; unexpected choices or backtracking |
| Credential accepted | elapsed time; confirm the value was never echoed or guessed |
| Quick verification offered | whether an already-authenticated path was visible and understood |
| Live validation decision | whether token/cost disclosure was noticed before consent/decline |
| Setup committed | elapsed time; verify resumability/rollback was not needed or record exact failure |
| Shell ready | elapsed time; whether the participant recognized where to type a request |
| First request submitted | exact non-sensitive request; whether `?`/route guidance was needed |
| First verified answer visible | elapsed time; provider success; AIShe authorship boundary and next action recognized |

### Completion criteria and record

Record each item as `pass`, `fail`, or `not_observed`, with one bounded note:

- reached a real verified first agent answer without reading the full README;
- never exposed, inferred, or persisted a credential outside the documented
  credential boundary;
- remaining steps and the short already-authenticated route were intelligible;
- live-validation token/cost impact was understood before any paid probe;
- setup could be resumed after an intentional cancel, or an existing
  deterministic cancel/resume gate was explicitly cited instead of asking the
  participant to repeat destructive work;
- first-answer authorship and next-action hint were understood without relying
  on color.

Also record total time to first answer, participant questions, wrong turns,
errors/codes, recovery commands, whether researcher intervention was required,
and the resulting action owner/date. A maintainer dry run can validate the
procedure and candidate but cannot by itself satisfy the plan's new-user
evaluation work.

## Residual actions and waivers

1. Retain the exact macOS/Linux/paid-live qualification JSON, live-model/fuzz
   reports, Bash report, SSH report, artifact digests, negative reports, and the
   human release decision in the release evidence store. The ignored local
   copies are not a durable archive by themselves.
2. Run a fresh clean macOS release profile for the changed candidate, then
   obtain hosted Linux/macOS CI evidence for the final release commit, including
   package/provenance jobs, without reclassifying configured-but-unrun jobs as
   pass. The current local feature matrices and authorized-host paid Linux gate
   are already green.
3. Paid-live model and changed-surface fuzz are **pass** for `a1b4f4cb`: 41/0/0
   overall, 19/20 then 20/20 labelled classification, 84/84 standalone fuzz,
   280/280 release fuzz, and a successful real YOLO tool effect. Preserve this
   point-in-time evidence; do not generalize it to other models/providers.
4. Implement or explicitly disposition AGENT-001 so completion UI never calls
   an effect recovered merely because a model emitted a final answer after a
   failed tool result. Implement LLM-001 repeated ambiguity trials or record the
   current 19/20 threshold as accepted probabilistic behavior.
5. Record the ONB-001 new-user timed walkthrough or explicitly waive the
   research/evaluation portion with an owner and follow-up date.
6. If broader terminal claims are desired, execute and retain the named emulator
   matrix. Otherwise keep those rows `not_run` and preserve the current generic
   terminal/transport scope.

No waiver is granted by this report. Emulator checks remain `not_run`; the
single authorized SSH pass remains narrowly candidate/target-specific; and the
WSL/Fish/native-Windows states remain the documented research or no-build
decisions above.
