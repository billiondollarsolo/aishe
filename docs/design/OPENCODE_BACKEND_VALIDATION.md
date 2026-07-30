---
title: "Aishe 0.5.0 Managed OpenCode Backend Validation"
date: "2026-07-30"
candidate_commit: "b388ee3"
aishe_version: "0.5.0"
opencode_version: "1.18.9"
status: "deterministic-qualification-pass-release-holds-pending"
---

# Aishe 0.5.0 Managed OpenCode Backend Validation

## Verdict

The implementation in
[`OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md`](OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md)
is complete at candidate commit `b388ee3`. The deterministic macOS and
disposable-Ubuntu gates pass against the exact OpenCode 1.18.9 runtime. The
candidate is not authorized for public release until every item under
[Release gates and rollout obligations](#release-gates-and-rollout-obligations)
that is labeled **blocking** is resolved and reviewed.

No external paid provider request was made during this qualification. A
credential previously pasted into a conversation is exposed and was
deliberately not reused.

## Candidate identity

| Component | Identity |
| --- | --- |
| Source | branch `feature/opencode-backend`, commit `b388ee3` |
| Aishe | `0.5.0` |
| Managed engine | OpenCode `1.18.9` |
| Runtime manifest SHA-256 | `5234a037fc7f51f7a2049b4ddc0c671086507ba1f36677f231c2193164f625e8` |
| Trusted plugin SHA-256 | `6a5800b1c85a156d27be168ad339176a6d4ff73f03780a9f3730ff6d436da718` |
| macOS candidate binary SHA-256 | `cf701d57a09c45796c625cd244717fcb83181e77c5572e30d8184edd6a28cc57` |
| Linux candidate binary SHA-256 | `9eaef6ead025dca06d19ad11f0a2b82fa8497b60bf90c96571c144695f90e425` |
| Linux installed OpenCode SHA-256 | `7c4d91c84d2bfdeabb59257e3490c5e5acb08f2aacb3e42f3ddc296a1c3f1aca` |

The macOS binary reports `aishe 0.5.0 (b388ee3, 2026-07-30)`. The disposable
Linux checkout intentionally excludes `.git`, so that binary reports
`(unknown, 2026-07-30)`; its source tree and binary digest are recorded above.

## Test environments

| Environment | Details |
| --- | --- |
| macOS | macOS 14.6.1 arm64, Darwin 23.6.0, zsh 5.9, Rust 1.97.1, declared MSRV Rust 1.88.0 |
| Linux | Ubuntu 26.04 LTS x86_64 glibc, kernel 7.0.0-15, zsh 5.9, bubblewrap 0.11.1, Rust 1.93.1 |
| Linux access | Disposable SSH node supplied for Aishe validation |
| Provider contract | Deterministic authenticated loopback HTTP provider; synthetic canary credential only |
| Runtime | Aishe-managed OpenCode 1.18.9 selected from the embedded OS/architecture/libc manifest |

## Gate results

| Gate | macOS | Linux | Evidence |
| --- | :---: | :---: | --- |
| Format | pass | CI-equivalent source | `cargo fmt --all -- --check` |
| Strict Clippy | pass | CI-equivalent source | all targets/features, warnings denied |
| Rust tests | 408 library + 2 main + 42 CLI + integration suites | 409 library + 2 main + 41 CLI + integration suites | all passed; Linux includes functional bubblewrap test |
| Declared MSRV | pass | covered by CI configuration | Rust 1.88.0 `cargo check --all-targets --locked` |
| Dependency policy | pass | same lockfile | advisories, bans, licenses, sources |
| Release build | pass | pass | locked release profile |
| Cargo package | pass | n/a | 169 files; notices, manifest, plugin, license, and backend docs present |
| Installer transaction | covered by Rust/macOS source suite | pass | corrupt runtime/binary, exact argv, atomic activation, state hash preservation |
| Setup/settings/tour PTY | pass | pass | colors, focus, 40/58/80/120/200 columns, `NO_COLOR`, cancel/resume/restart/apply |
| Provider/model setup | pass | pass | bad key rejected; `/models`; listed and manually typed model validation; pricing |
| Pinned OpenCode contract | pass | pass | real runtime, fake provider, tools, usage, session, egress, credential isolation |
| Accepted yolo host scope | pass | pass | one authenticated lease wrote outside workspace with no per-action prompt |
| Durable interrupted resume | pass | pass | pending tool was not repeated |
| Routing/scrollback PTY | 38/38 | 38/38 | sigils, `what/where/who`, prompt remains visible, yolo acceptance |
| Native zsh | 44/44 | 44/44 | expansions, control flow, redirection, job control, history |
| Signal/terminal | 7/7 | 7/7 | resize, Ctrl-C, Ctrl-Z, process continuation |
| Statusline | 4/4 | 4/4 | right/below/off, inert prompt substitution |
| Generated PTY fuzz | 339/339 | 339/339 | commands, NL punctuation/metacharacters, adversarial model output |
| Administrative validation | n/a | 455/455 | shell, file operations, routing, config, commands, skills, MCP |
| Direct shell differential | pass | pass | 1,000 exact commands, backend absent |
| Managed soak | n/a | pass | real runtime, 1,000 turns, 40 cold samples, 39 reconnects, 12 lifecycle cycles |
| Concurrent sessions | n/a | pass | 100 isolated shell/workspace sessions in 72.926 s |
| Real host reboot | local two-phase smoke | pass | byte hashes, history, session identity, and prior provider context survived |

## Performance

### Direct commands

Each harness ran 1,000 commands through raw zsh and `aishe -c`, checked exact
stdout/stderr/exit status, and proved that no backend state or process appeared.

| Host | Raw zsh p95 | Aishe p95 | Added p95 | Limit | Result |
| --- | ---: | ---: | ---: | ---: | :---: |
| macOS arm64 | 3.838 ms | 8.673 ms | 4.835 ms | 10.000 ms | pass |
| Ubuntu x86_64 | 2.744 ms | 5.533 ms | 2.788 ms | 10.000 ms | pass |

### Managed backend

The Linux soak ran the real OpenCode 1.18.9 binary with a deterministic
loopback provider. It admitted 1,000 turns, forced a supervisor restart every
25 turns, and then exercised repeated stop/start lifecycle transitions.

| Metric | Result |
| --- | ---: |
| Managed turns | 1,000 |
| Forced supervisor reconnects | 39 |
| Lifecycle stop/start cycles | 12 over 74.7 s |
| Exact authenticated provider requests | 1,012 for 1,000 turns + 12 lifecycle turns |
| Cold-ready p95 | 2,229.0 ms |
| Cold full-turn p95 | 3,493.1 ms |
| Full live-verify p95 | 3,661.8 ms |
| Warm authenticated-health p95 | 25.4 ms |
| Full managed-turn p50 | 410.3 ms |
| Full managed-turn p95 | 3,276.3 ms |
| Supervisor RSS maximum | 13,476 KiB |
| OpenCode RSS maximum | 750,616 KiB |
| OpenCode warm-sample-to-final growth | 45,040 KiB |

The full-turn p95 deliberately includes forced cold reconnects and a provider
request whose retained conversation grows through 1,000 turns; it is not the
adapter-only processing measurement. Cold authenticated readiness passes the
2.5-second target, and warm control health passes the 100-millisecond target.

The separate concurrency harness admitted 100 overlapping Aishe processes into
100 different workspace/shell identities. It completed in 72.926 seconds and
proved exactly one provider request and one durable session for every identity,
with no mixed prompt or workspace.

## Security and authority results

- The exact managed runtime is size-bounded, SHA-256 verified,
  version-attested, safely extracted, staged, atomically activated, retained for
  rollback, and selected by OS/architecture/Linux libc.
- A system `opencode` executable, existing public server, user/project OpenCode
  config, plugin, auth, cache, and model refresh cannot enter the managed
  process.
- The embedded plugin is dependency-free and build-hash verified. The runtime
  loader has no package-install dependency or bootstrap egress.
- OpenCode's host-effecting built-ins are denied for primary and child agents.
  Model effects require a live, authenticated Aishe foreground lease bound to
  shell, workspace, mode, scope, network, session, message, and stable call ID.
- Wrong credentials, forged workspace or ancestry, stale sessions, duplicate
  effects, and unregistered calls fail closed. A started effect whose client
  disappears becomes `outcome_unknown` and is never blindly replayed.
- Provider credentials are delivered to OpenCode through the private bootstrap
  channel but are absent from command, skill, MCP, file-tool, support-bundle,
  journal, diagnostic, and model-visible environments.
- The Linux bubblewrap functional test proves writable workspace, read-only host
  root, hidden HOME/config/state, private `/tmp`, symlink-escape denial, and
  network allow/deny.
- macOS correctly identifies workspace mode as policy-only and requires one
  unsandboxed-risk acceptance per shell.
- Setup credential input is hidden, remains in memory until Apply, never enters
  the resumable draft, and is removed on cancel or failed transaction.

## Shell and UI results

- Direct zsh commands never require or start OpenCode.
- Questions, suggestions, and agent tasks use the managed backend by default
  without launching an alternate-screen TUI.
- Submitted prompts remain visible in scrollback. Background updates do not
  erase the accepted line or corrupt a live ZLE buffer.
- A valid first command word does not lock the whole line green: full-buffer
  `what is`, `where is`, and `who am` questions change to the AI route and use
  the identical grammar on Enter.
- Unknown `/name` input cannot bypass the custom-command registry through the
  direct-shell fast path. A real executable absolute path remains fast.
- Yolo requests one scope acceptance per shell and emits no per-action approval
  after acceptance. The host-scope contract performs a real reversible
  outside-workspace effect through Aishe, not through an OpenCode built-in.
- Setup uses visible color/focus, word wrapping, stable left alignment,
  responsive widths, `NO_COLOR`, explicit progress, actionable errors, a review
  screen, and transactional Apply.

## State and operational results

- Installer and runtime operations preserve exact hashes for config,
  credentials, history, pricing, trust, sessions, tasks, audit, undo, and
  unrelated data.
- Default uninstall removes only replaceable binary/runtime integration and
  preserves user state. Destructive categories are separate and require
  explicit confirmation.
- Shared Aishe history persists across concurrent shells and upgrades.
- A real Linux reboot changed the kernel boot ID while preserving byte-exact
  config/history/session/backend state. After reconnect, the same managed
  session sent the new turn with its pre-reboot conversation in provider
  context. Boot ID changed from
  `3c117f37-9e99-4821-a0c8-5526f2db9f6b` to
  `2b8e7c56-5013-48d3-a78b-afdeb67db395`.
- Doctor detects and repairs private layout, runtime, plugin, supervisor,
  credential, and sandbox problems. A second repair is idempotent and reports no
  false changes.
- `aishe backend status --json` is schema-versioned and omits control URLs,
  passwords, tokens, and nonces.
- OpenCode and legacy sessions are listed together; legacy sessions/tasks remain
  readable and resumable.

## Commands executed

Core local gates:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
rustup run 1.88.0 cargo check --all-targets --locked
cargo deny check advisories bans licenses sources
cargo build --release --locked
cargo package --locked --allow-dirty --no-verify
```

Platform and contract gates:

```sh
python3 tests/setup_pty.py target/release/aishe
python3 tests/pty_scenarios.py target/release/aishe
python3 tests/statusline_pty.py target/release/aishe
python3 tests/pty_smoke.py target/release/aishe
python3 tests/pty_signals.py target/release/aishe
python3 tests/zsh_features.py target/release/aishe
python3 tests/pty_fuzz.py target/release/aishe 1
python3 tests/opencode_runtime_contract.py target/release/aishe
python3 tests/opencode_host_scope.py target/release/aishe
python3 tests/durable_task_resume.py target/release/aishe
python3 tests/direct_shell_benchmark.py target/release/aishe \
  --commands 1000 --warmup 20
python3 tests/admin_validation.py target/release/aishe
```

Final Linux scale gates:

```sh
python3 tests/opencode_soak.py target/release/aishe \
  --turns 1000 --cold-cycles 40 --warm-probes 200 \
  --reconnect-every 25 --lifecycle-hours 0.02 --lifecycle-interval 1
python3 tests/opencode_concurrency.py target/release/aishe --sessions 100

python3 tests/reboot_persistence.py prepare \
  target/release/aishe /root/aishe-reboot-v0.5.0
# reboot and reconnect to the disposable node
python3 tests/reboot_persistence.py verify \
  target/release/aishe /root/aishe-reboot-v0.5.0
```

## Release gates and rollout obligations

These are unresolved by design and must not be reported as passes. Items 1–3
block publishing v0.5.0; items 4–5 are ongoing rollout obligations and do not
prevent an otherwise qualified v0.5.0 publication.

1. **Blocking — fresh-secret live-provider matrix.** Run OpenAI Responses,
   Anthropic, and one OpenAI-compatible provider on a disposable node with a
   newly issued, never-published credential, explicit request/dollar caps, and
   redacted logs. The previously pasted key must be rotated, not reused.
2. **Blocking — literal 24-hour lifecycle soak.** The same harness supports
   `--lifecycle-hours 24`; the final candidate has an accelerated lifecycle
   run, but elapsed wall-clock time cannot be simulated honestly.
3. **Blocking — release evidence review and publication.** No tag or GitHub
   release is created by this implementation task.
4. **Ongoing — fallback retention.** `[backend] engine = "native"` and runtime
   rollback are implemented and tested, but they must remain shipped for at
   least the next two minor releases before the temporal obligation can be
   closed.
5. **Ongoing — optional distro expansion.** Ubuntu/Debian-family Linux is
   qualified. Fedora/RHEL-family validation remains recommended when a
   disposable node is available; it is not claimed here.

## Release-owner continuation

After injecting fresh credentials through the environment—not config, source,
arguments, or shell history—run the bounded live suites documented in
`docs/development.md`. Then run the literal lifecycle gate:

```sh
AISHE_RUNTIME_DIR=/approved/runtime \
python3 tests/opencode_soak.py target/release/aishe \
  --turns 1000 --cold-cycles 40 --warm-probes 200 \
  --reconnect-every 25 --lifecycle-hours 24 --lifecycle-interval 60
```

Review the generated Markdown/JSON, support-bundle canary scan, actual provider
costs, and CI artifact/SBOM/provenance results before creating a release tag.
