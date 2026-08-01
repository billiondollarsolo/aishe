> **Lifecycle: Validation evidence.** Candidate: the dirty development working
> tree based on AIShe v0.6.5 commit `4a2c7e4`, audited 2026-08-01 on macOS
> arm64. This report proves only the cited checkout and executions; it is not a
> release-readiness decision.

# Next product UX and reliability implementation report

This report maps every implementation story and every milestone Definition of
Done item in [the active plan](NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) to concrete
source and observed evidence. It deliberately distinguishes implementation,
local deterministic proof, and platform/release proof.

Status meanings:

- **Verified locally** — implementation exists and a relevant executable gate
  passed during this audit.
- **Implemented; external proof pending** — the code or CI gate exists, but an
  acceptance condition needs another platform, service, credential, or clean
  release candidate.
- **Partial** — a stated work or acceptance item is still missing.
- **Decision complete** — the requested research decision was recorded; it does
  not imply that the researched platform or feature is supported.

## Audit identity and observed gates

The checkout was based on `4a2c7e4` but had more than 140 modified/untracked paths while
parallel implementation lanes were active. A binary that reports that commit
therefore cannot identify the exact uncommitted tree by itself. Current release
qualification must rebuild after the tree is frozen and record an artifact
digest.

| Observed gate | Result in this audit |
| --- | --- |
| `cargo test --all-targets --all-features --locked` | Initial parallel run failed three bridge lease tests. Each passed serially. After the instance-local TTL remediation, the final unloaded run passed 570 library tests plus every integration target; the bridge subset then passed 20/20 parallel stress repetitions. Preserve the initial failure as regression evidence rather than erasing it. |
| No-default-feature Rust path | `cargo test --all-targets --no-default-features --locked` passed 569 library tests plus every integration target, and strict no-default-feature Clippy passed. |
| Harness and docs-contract unit suites | 59/59 passed across `harness_identity_test.py`, `live_contract_test.py`, `bash_hook_test.py`, `terminal_compat_test.py`, `performance_benchmark_test.py`, `qualify_test.py`, `advisory_policy_test.py`, and `docs_contract_test.py`. |
| Agent UI PTY | `tests/agent_ui_pty.py` passed long questions, reconnect, resize, bounds, and zero-ESC assertions. |
| Shell hygiene | `shellcheck install.sh tests/*.sh`, generated `zsh -n`, and generated `bash -n` passed. |
| Public JSON contract | The static inventory passed 3/3 for all 23 paths, prior-minor/API compatibility passed 6/6, CLI contracts passed 50/50, audit reader/writer tests passed 8/8, and the unauthenticated provider JSON process check passed. |
| Deterministic release profile (pre-commit freeze) | 34 passed, 0 failed, 3 platform-not-applicable skips on macOS arm64 in 507 seconds. It covered performance, lazy loading, runtime install/live verify, installer transaction, PTY/fuzz/signals, 20-turn soak, 8-way concurrency, durable resume, and admin validation. The report correctly marks the source dirty, so a post-commit identity run remains required. |
| Post-extraction compatibility matrix | macOS and isolated Ubuntu passed 35/35 integration tests, generated/asset shell syntax, native PTY/tmux/attached Screen, Bash 5 Tier B 18/18, and candidate-selected nested SSH. macOS Bash 3.2 Tier B- passed 13 core cases plus exactly five declared alternatives. |

The first all-target failure names were:
`backend::bridge::tests::lease_routes_one_call_and_replays_completed_result`,
`backend::bridge::tests::started_call_becomes_outcome_unknown_after_foreground_loss`,
and
`backend::bridge::tests::provider_budget_reserves_caps_and_deduplicates_child_usage`.
They failed with lease-expired/foreground-unavailable errors because a short
test TTL was process-global. `Bridge` now owns the test TTL per instance and a
parallel-isolation regression covers the remediation. A final unloaded root
qualification remains the release authority.

## Story evidence

### Truth, qualification, command surface, and routing

| Story | Status | Implementation and observed validation |
| --- | --- | --- |
| UX-001 | **Verified locally** | Lifecycle catalog and per-document banners are in [README.md](README.md); `tests/docs_contract_test.py` rejects missing labels/index entries and invalid repository-relative paths/GitHub-style anchors. Its 2/2 tests passed and CI/qualification require it. |
| UX-002 | **Verified locally** | `docs/architecture.md`, `docs/commands.md`, and `docs/development.md` now describe schema 7, the `src/cli` seam, registry/tombstones, and current front ends. `product_help::tests::commands_markdown_matches_the_generated_registry_block` and command-surface tests passed. Reedline appears only in explicitly historical/compatibility context. |
| QA-001 | **Verified locally** | `tests/harness_identity.py` is used by external binary harnesses; `tests/qualify.py` builds and verifies before harness execution. Four identity-helper and 14 qualification-driver tests passed. |
| QA-002 | **Verified locally** | `.gitignore` covers Python caches; CI runs shellcheck and generated hook syntax. Shellcheck, generated zsh syntax, and generated Bash syntax passed locally. Clean-checkout status cannot be proven from this intentionally dirty collaborative tree. |
| QA-003 | **Verified locally** | `tests/admin_validation.py` uses registry/tombstone semantics and `corpus_version`; semantic command tests do not depend on brand prose. The final pre-freeze release profile recorded 461/461 deterministic checks on macOS with the paid model suite explicitly skipped. |
| CMD-001 | **Verified locally** | `src/command_surface.rs` defines identity, aliases, help, arguments, surfaces, output, side effects, handoff, lifecycle, and tombstones. Registry uniqueness/completeness tests passed. |
| CMD-002 | **Verified locally** | `src/dispatcher.rs` and `src/cli/runtime.rs` query the registry. `tests/command_surface.rs` proved built-in reservation, absolute paths, custom-command precedence, unavailable guidance, and tombstones. |
| CMD-003 | **Verified locally** | `src/integration.rs` renders hook dispatch from registry identities/actions. Generated coverage, no-manual-case-table, syntax, zsh, and Bash tests passed. |
| CMD-004 | **Verified locally** | `src/product_help.rs`, completion generation, `docs/commands.md`, and registry metadata are conformance-tested. Seven command-surface tests passed. |
| ROUTE-001 | **Verified locally** | `src/dispatcher.rs` defines `RouteKind`, `RouteReason`, `RouteSource`, and bounded `RouteDecision`/diagnostics while retaining the compatibility adapter. Dispatcher reason/property tests passed. |
| ROUTE-002 | **Verified with local and point-in-time Linux evidence** | `tests/fixtures/routing/v1.json` separates normative/research cases with platform and criticality metadata; Rust/generated-zsh corpus gates passed locally and the post-extraction candidate's Linux integration/PTY matrix passed. A post-commit Linux rerun remains release evidence rather than missing implementation. |
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
| UI-006 | **Verified locally** | `src/agent/renderer.rs` distinguishes phases, recovered attempts, terminal failure, reconnect, changed files, and waiting. Renderer and acceptance tests proved truthful recovered/final states and status clearing. |
| UI-007 | **Verified locally** | Streaming prints one authorship structure for short/long responses and pipes receive one body; destructive rerender is capability-gated. Suggest streaming and agent acceptance tests passed without duplication. |
| UI-008 | **Verified locally** | Config schema 7 adds theme/depth/Unicode/motion, settings/provenance, dark/light highlighting, ANSI degradation, and migration backup. Config/CLI migration and palette contrast tests passed; `docs/accessibility.md` records the maintained contrast review. |
| A11Y-001 | **Verified locally** | Semantic snapshots and ASCII glyph tests retain textual/glyph distinctions for route, mode, selection, proposal, approval, warning, danger, success, diff, and error after ANSI stripping. |
| A11Y-002 | **Verified locally** | Static motion and ASCII policies are implemented in UI, promptui, renderer, logo, and Doctor reporting. Policy/layout tests prove no ANSI/symbol dependence, and the static picker I/O acceptance proves a cursor-motion-free interaction path. |
| A11Y-003 | **Verified locally** | `ui::tests::terminal_layout_matrix_bounds_wide_combining_and_hostile_text`, picker viewport tests, renderer tests, `tests/agent_ui_pty.py`, and terminal transport tests cover narrow/wide content, control text, and resize without panic or invisible focus. Emulator-specific manual checks remain separate and `not_run`. |
| A11Y-004 | **Verified locally** | `/help session`, `docs/shell-integration.md`, `docs/accessibility.md`, and Doctor key-conflict diagnostics document active alternatives and Meta/Option caveats. Help/diagnostic/picker-key tests passed. |
| INT-001 | **Verified with local and point-in-time Linux evidence** | Generated zsh stages suggestions into `BUFFER`; Bash 5 offers `READLINE_LINE`/recall with declared 3.2 differences. The post-extraction candidate passed 65/65 macOS PTY scenarios, native/multiplexer Linux staging, Bash 5 Tier B 18/18 on both hosts, and the declared Bash 3.2 alternatives. |
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
| COMP-002 | **Verified with macOS/Linux candidate evidence** | `tests/bash_hook.py` is a real isolated interactive harness with 18 declared rows and strict 3.2 differences; 17 harness-unit tests passed. Bash 5.3.9 passed Tier B 18/18 on macOS and isolated Ubuntu; macOS Bash 3.2.57 passed 13 core cases and exactly five declared/exercised Tier B- alternatives. |
| COMP-003 | **Implemented; external proof pending** | `.github/workflows/ci.yml` contains a blocking bounded `macos-latest` PTY job for terminal latency/resize, routing, picker, status, setup, and signals. Configuration is inspectable, but this report cannot claim a remote CI execution of the final tree. |
| COMP-004 | **Verified with point-in-time local/remote evidence** | `tests/terminal_compat.py` records pass/fail/limitation/unsupported. Point evidence passes local, tmux, and screen on macOS plus the opt-in SSH contract to one authorized Ubuntu 26.04 target. Emulator rows remain `not_run`, never pass, and one SSH target does not establish universal SSH compatibility. |
| COMP-005 | **Decision complete** | `FISH_INTEGRATION_DECISION.md` declines a native Fish hook this milestone; `WSL_COMPATIBILITY_DECISION.md` declines a support/artifact claim before real WSL2 qualification. User docs preserve those boundaries. |
| ARCH-001 | **Verified locally** | `src/main.rs` is 804 lines; binary-owned Clap remains in `src/cli/args.rs`, while status, connection, session, history, backend, settings, runtime, hints, JSON/error contracts are bounded `src/cli` modules. Architecture/CLI tests passed. |
| ARCH-002 | **Verified locally** | Pure terminal primitives live in `src/ui.rs`/`src/ui/render.rs`; promptui is a compatibility facade. The architecture test rejects business/prompt dependencies and renderer snapshots passed. |
| ARCH-003 | **Verified locally and on Linux** | `src/integration.rs` is a 69-line orchestrator over six reviewed assets plus focused `registry`, `templates`, and `tests` modules. Exact pre-extraction SHA-256 snapshots, typed exact-one substitution, registry fragments, shellcheck, generated/asset syntax, and 35 focused integration tests passed locally and in the isolated Ubuntu fixture. |
| ARCH-004 | **Verified locally** | `docs/legacy-compatibility.md`, schema-7 migration, tombstones, backups, and legacy-task support windows inventory/remove reedline-era fields deliberately. Migration/backup, help, tombstone, and persisted-task tests passed. |
| SAFE-001 | **Verified locally** | `!` emits a non-color one-line `shell override` cue, route diagnostics state the safety bypass, and CLI/route tests prove one-line non-sticky behavior and no model route. |
| SAFE-002 | **Verified locally** | Approval views show mode, effective scope, network, sandbox implementation, command/effect/reason, and safe negative default; Linux isolated, Linux host, and macOS policy-only text remain distinct without color. Approval snapshots/tests passed. |
| SAFE-003 | **Implemented; external proof pending** | `SECURITY.md` carries threat-model version/date and limitations; `tests/qualify.py` records it with runtime/plugin/corpus digests. Driver tests passed, but only a frozen release qualification can prove a release against that model. |
| SAFE-004 | **Verified locally** | `docs/data-retention.md`, category-specific uninstall selection/preview, Doctor retention checks, bounds/rotation, and preservation defaults cover local state. Diagnostics, uninstall, CLI, audit, undo, history, and task tests passed. |
| SAFE-005 | **Verified locally** | `tests/boundary_fuzz.rs`, route/safety corpora, runtime archive seeds, shell-selection parsing, terminal/error sanitization, and SSE/JSON bounds are deterministic regression gates; boundary tests passed. |
| DEP-001 | **Verified locally** | Cargo metadata and `tests/advisory_policy_test.py` require one `crossterm` version; the observed graph contains only 0.29.0. |
| DEP-002 | **Implemented; external proof pending** | `Cargo.toml`/`Cargo.lock` contain only `ureq` 3.3.0 and the ureq-2 advisory exception is gone. Deterministic provider/client tests passed; fresh Linux/macOS live-contract qualification remains a release gate. |
| DEP-003 | **Verified locally** | `deny.toml` records owner, added/review dates, target removal, and rationale; the advisory-policy test rejects missing/expired metadata and passed. |
| PERF-001 | **Verified locally; host metrics informational** | `tests/performance_benchmark.py` and `examples/performance_probe.rs` emit schema-v1 shell/route/picker/render/RSS/size/backend evidence with enforced stable thresholds. Unit tests passed; point-in-time `test-results/performance-focused.json` passed on macOS. Host-sensitive values are correctly labeled informational. |
| PERF-002 | **Verified locally** | `tests/lazy_loading_test.py` instruments provider construction, backend state, listeners, and network attempts for shell/help/route/status. Unit support tests passed and point-in-time `test-results/lazy-loading-focused.json` records macOS evidence. |
| REL-001 | **Verified locally** | `tests/qualify.py` defines quick/local-full/linux-full/release/paid-live profiles, builds and verifies identity first, records commands/durations/skips/digests/corpora, and never calls required skips pass. Fourteen driver tests passed. The pre-commit deterministic release profile passed 34 gates with zero failures and three explicit Darwin-inapplicable skips; its dirty-source flag correctly prevents treating it as frozen evidence. |
| REL-002 | **Verified locally as policy** | `docs/release-readiness.md` defines pass/fail/not_run/waiver/hold, required evidence, binary/state rollback, and failed rollout response. Qualification unit tests enforce incomplete required skips. Actual release readiness remains false until external gates close. |
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
| F-003 — Rust/zsh routing duplication and Bash difference | **Resolved with declared tiers; final platform rerun pending** | ROUTE-001 through ROUTE-003 and COMP-001/002; declarative route rules/corpus in `src/dispatcher.rs`, generated predicates in `src/integration`, `tests/routing_corpus.rs`, and the real Bash tier harness. |
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
| F-017 — compatibility claims exceed tested parity | **Resolved as tiered product truth; final CI evidence pending** | COMP-001 through COMP-003; Doctor/front-end/Bash docs, strict real Bash harness, bounded macOS PTY job, and explicit B/B- differences. |
| F-018 — public JSON is inconsistently versioned | **Resolved for inventoried public surfaces** | API-001 through API-003; [automation inventory](../automation.md), versioned status/suggest/error/route contracts, prior-minor fixtures, and `tests/api_compat.rs`. |
| F-019 — main/integration ownership concentration | **Resolved locally** | ARCH-001/003; `src/main.rs` is 804 lines and `src/integration.rs` 69 lines over focused CLI modules, six reviewed shell assets, templates/registry modules, byte snapshots, syntax, and architecture tests. |
| F-020 — long streamed answers have inconsistent anatomy | **Resolved locally** | UI-004/UI-007; one answer boundary/body contract for short, long, and piped responses with no duplicate text in suggest and agent acceptance tests. |
| F-021 — accessibility preferences are implicit | **Resolved locally** | UI-008 and A11Y-001/002; explicit theme/depth/Unicode/motion config, static/ASCII paths, Doctor reporting, settings provenance, and non-color/static tests. |
| F-022 — autonomous risk state lacks clarity | **Resolved locally** | UI-005/UI-006 and SAFE-002; approval/activity views expose effective mode/scope/network/sandbox/effect/default, distinguish policy-only/isolated/host authority, and render truthful recovery/failure. |
| F-023 — duplicate dependencies and unowned exceptions | **Resolved locally; live transport rerun remains a release gate** | DEP-001 through DEP-003; single crossterm 0.29 and ureq 3.3 stacks, owned/dated syntect advisory exception, and `tests/advisory_policy_test.py`. |
| F-024 — Python cache hygiene omission | **Resolved locally** | QA-002; `.gitignore` covers Python/pytest/coverage/virtualenv artifacts and CI also enforces shellcheck/generated-shell syntax. |

## Definition of Done mapping

| Milestone condition | State | Concrete evidence or residual |
| --- | --- | --- |
| P0 findings F-001 through F-006 resolved | **Locally proven** | Viewport, registry, route contract/conformance, semantic plain output, corrected active docs/lifecycle/link gate, and stale-binary rejection all have passing deterministic tests. |
| One command registry drives/verifies every supported surface | **Locally proven** | `src/command_surface.rs`, generated hook fragments, product-help/doc/completion conformance, and command-surface tests. |
| RouteDecision and v1 corpus live, explainable, cross-surface | **Locally proven with point-in-time Linux evidence** | Rust/zsh local corpus and route CLI passed; the post-extraction isolated Linux integration, Bash, PTY, multiplexer, and SSH matrix passed. Post-commit rerun remains a release-evidence step. |
| Final answers and proposals distinguishable without color | **Locally proven** | UI render snapshots and agent acceptance/PTY tests. |
| NO_COLOR/plain/JSON has zero escape bytes | **Locally proven, release matrix pending** | UI/error/API/raw PTY assertions passed locally; supported-platform release matrix has not run on a frozen candidate. |
| Picker viewport, key conflicts, display width fixed | **Locally proven** | Promptui viewport/key/ranking, static picker I/O acceptance, and UI grapheme/cell tests passed. |
| Compatibility tiers and Doctor reporting published | **Locally proven** | Front-end/Bash/terminal docs and diagnostic tests. |
| Real Bash tier harness and deterministic macOS PTY CI subset | **Locally/remote proven; CI commit proof pending** | Harness/unit tests and CI jobs exist; the post-extraction macOS Bash 3.2/5 and isolated Linux Bash 5 matrices passed, while hosted CI for the final commit remains external evidence. |
| All 23 public JSON/JSONL paths versioned with fixtures | **Locally proven** | `src/cli/json_contract.rs` reports zero unversioned surfaces; static inventory 3/3, API compatibility 6/6, CLI contracts 50/50, and audit tests 8/8 passed. Prior v0.5/v0.6 roots and explicit migrations remain fixture-gated. |
| Active docs current; old plans bannered | **Locally proven after current corrections** | Lifecycle/index check plus command-doc conformance and corrected schema/module seams. |
| Main/integration extraction started only behind characterization and stays stable | **Locally/Linux proven** | Main is 804 lines behind CLI/architecture tests; integration is a 69-line orchestrator over reviewed assets/modules with exact pre-extraction byte snapshots, syntax checks, and 35 focused tests on both hosts. |
| Required quality, performance, security, installer, release gates pass | **Pre-commit deterministic proof; frozen/external disposition pending** | The macOS deterministic release profile passed 34/34 applicable gates, including runtime install/live verify, installer transaction, performance/lazy loading, 20-turn soak, concurrency, resume, fuzz, signals, and admin validation. The lease remediation also passed a final unloaded suite and 20 parallel stresses. Frozen-commit Linux install/upgrade/sandbox/package/provenance, hosted macOS CI, paid-live-or-waiver, and final rollback disposition remain. |
| Release notes explain visible changes/migrations | **Implemented, release decision pending** | `CHANGELOG.md` Unreleased section covers registry/routing, UI, JSON, compatibility, migration, dependencies, hints, and safety changes. |

The milestone is therefore **not Definition-of-Done complete** in this report.
The blocking evidence is a frozen-candidate release qualification across the
supported platforms and release/security/installer gates. The remaining
concrete product-work gap is ONB-001's recorded new-user evaluation.

## Platform, remote, and point-in-time evidence

These artifacts are useful but must not be generalized to the final dirty tree:

| Evidence | What it proves | Limitation |
| --- | --- | --- |
| `test-results/qualification-release-precommit.json` | Deterministic macOS release profile: 34 pass, 0 fail, 3 platform-inapplicable skips; artifact/corpus/threat-model digests recorded | Source is intentionally marked dirty and the binary still reports baseline HEAD; post-commit identity remains required. |
| `test-results/validation-20260801T055413Z.md` | macOS deterministic admin corpus 2: 461/461, model suite skipped | Point-in-time pre-commit binary; no paid model calls. |
| `test-results/bash-hook-macos-arch003-2026-08-01.json` | Post-extraction macOS Bash 5.3.9 Tier B 18/18 and Bash 3.2.57 Tier B- 13 core plus five alternatives | Candidate-specific; hosted macOS CI for the final commit remains external. |
| `test-results/bash-hook-linux-arch003-2026-08-01.json` | Isolated Ubuntu Bash 5.3.9 Tier B 18/18 | Candidate-specific pre-commit snapshot; Bash 3.2 is not expected on Linux. |
| `test-results/terminal-compat-macos-arch003-2026-08-01.json` | Post-extraction macOS local/tmux/screen transport pass at 300 ms escape latency and resize | Named terminal emulators remain `not_run`; this is candidate-specific. |
| `test-results/terminal-compat-linux-arch003-2026-08-01.json` | Isolated Ubuntu native/tmux/attached GNU Screen transport pass with staged review, 300 ms split escape, and resize | Candidate-specific pre-commit snapshot. |
| `test-results/terminal-compat-ssh-linux-arch003-2026-08-01.json` | Opt-in SSH PTY contract passed to the authorized Ubuntu target with isolated candidate selection, fake provider, split escape input, and resize | Proves only that candidate, target, authentication path, and transport configuration; target and identity-file paths are redacted. |
| `test-results/performance-focused.json` | macOS stable performance thresholds passed; backend timing/RSS measured | Host-sensitive size/RSS/render/backend metrics are informational. |
| `test-results/lazy-loading-focused.json` | macOS local shell/help/route/status isolation probe | Needs final-candidate and Linux repeat for release evidence. |
| `.github/workflows/ci.yml` | Linux and macOS gates are configured, including bounded macOS PTY and Linux Bash/terminal jobs | Configuration is not evidence that a particular commit/job passed. |

Explicitly not proven here: paid provider behavior, Linux package/provenance
gates for the frozen commit, a fresh hosted macOS CI run, WSL, native Windows,
Fish integration, authorized SSH configurations beyond the one recorded target,
or the manual
terminal-emulator/contrast matrix beyond the recorded generic palette
calculations.

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

1. Freeze the tree, rebuild, and run `tests/qualify.py release`; retain its JSON,
   artifact digest, and human decision. Re-run the full Rust suite unloaded and
   stress the bridge tests in parallel to close the observed TTL regression.
2. Obtain required Linux and macOS CI evidence. Run Linux bubblewrap,
   installer/upgrade, Bash Tier B, tmux/screen, and package/provenance gates.
3. Record paid-live and changed-surface soak as pass or as an explicit release
   waiver with owner, rationale, accepted risk, and expiry. A missing credential
   is `not_run`, never pass.
4. Record the ONB-001 new-user timed walkthrough or explicitly waive the
   research/evaluation portion with an owner and follow-up date.

No waiver is granted by this report. Emulator checks remain `not_run`; the
single authorized SSH pass remains narrowly candidate/target-specific; and the
WSL/Fish/native-Windows states remain the documented research or no-build
decisions above.
