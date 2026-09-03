# Development

## Build and check

Building needs **Rust 1.88 or newer** (some normal dependencies pulled in via
`syntect` require it); the prebuilt release binaries need no toolchain.

```sh
cargo build --locked
cargo test --all-targets --locked  # integration tests spawn a real shell
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
# Lint maintained shell sources and syntax-check exactly what `aishe init` emits.
python3 tests/shell_contract.py target/debug/aishe
```

## Test layout

- Rust unit tests live inline in each module under `src/`.
- Integration tests are in `tests/`:
  - `tests/cli.rs`, `tests/dispatcher.rs`, `tests/executor.rs`, `tests/modes.rs`,
    `tests/providers.rs`, `tests/safety.rs`, `tests/safety_corpus.rs`,
    `tests/mcp.rs`, `tests/mcp_http.rs`: Rust integration tests.
  - `tests/pty_scenarios.py`, `tests/pty_fuzz.py`, `tests/zsh_features.py`,
    `tests/pty_signals.py`: deterministic pseudo-terminal suites for the zsh-PTY
    front-end, driven by a fake provider (no key). `pty_signals.py` covers
    Ctrl-C / Ctrl-Z / window resize / multi-line continuation; the fuzz and
    feature suites write Markdown reports under `test-results/`.
  - `tests/setup_pty.py`, `tests/statusline_pty.py`, and
    `tests/model_picker_pty.py`, and `tests/durable_task_resume.py`: real-PTY setup/settings transactions,
    native right-prompt/off live status, and kill/resume without duplicate tool
    execution. The model picker suite covers two concurrent shells, filtering,
    shell-local selection, save-default, cancellation, rollback, and plain
    no-emoji output.
  - `tests/installer_upgrade.sh`: hermetic replacement install that hashes
    config, shared history, tasks, and unrelated state before/after.
  - `tests/opencode_runtime_contract.py`: launches the exact pinned OpenCode
    runtime against a deterministic fake provider, real trusted plugin, real
    authenticated foreground bridge, and isolated environment. It proves
    multi-turn continuity, suggest/auto tool policy, usage, idempotency journal,
    credential non-leakage/non-persistence, no runtime plugin dependency tree,
    and no install-time or public-catalog egress.
  - `tests/opencode_host_scope.py`: runs a real pinned-runtime yolo tool loop
    with a synthetic per-shell host acceptance, writes to a disposable path
    outside the workspace, and proves the effect crossed one authenticated
    AIShe lease without a second per-action approval.
  - `tests/opencode_connection_isolation.py`: launches two same-provider,
    same-model API-key connections concurrently and proves distinct provider
    credentials, runtimes, sessions, audit identities, and secret-free state.
  - `tests/opencode_soak.py` and `tests/opencode_concurrency.py`: measure cold
    and warm lifecycle latency/RSS, repeated reconnect continuity, optional
    24-hour stop/start behavior, exact provider-request counts, and concurrent
    shell/session isolation.
  - `tests/direct_shell_benchmark.py`: compares raw zsh and `aishe -c`, checks
    exact output across 1,000 direct commands, enforces the direct-command p95
    startup regression SLO, and proves that no managed-backend state is created.
  - `tests/reboot_persistence.py`: two-phase disposable-node gate that hashes
    config/history/session state before a real reboot, then verifies the bytes,
    managed-session identity, and prior provider context after reconnect.
  - `tests/fixtures/opencode/v1.18.27/`: frozen OpenAPI endpoint and normalized
    event fixtures for the supported compatibility surface.
  - `tests/provider_unauthenticated.py`: real local HTTP endpoint proving
    loopback providers can list/validate models with no dummy API key.
  - `tests/pty_smoke.py`: PTY smoke test for the zsh-PTY front-end.
  - `tests/bash_hook.py`: real interactive native-Bash matrix with isolated
    config/data/history, fake provider, exact Bash 5.x Tier B and Bash 3.2 Tier
    B- reporting, signals/job control, and machine-verifiable alternatives.
    `tests/bash_hook_test.py` covers version and report semantics.
  - `tests/terminal_compat.py`: deterministic local PTY, tmux, screen, and
    opt-in SSH transport contract. It proves suggestion staging, 300 ms split
    escape-sequence history recall, resize propagation, and records explicit
    pass/fail/limitation/unsupported JSON. `tests/terminal_compat_test.py`
    covers report and remote-fixture semantics.
  - `tests/real_model.py`: opt-in command-vs-answer classification corpus against a
    live model through the `suggest --json` contract. **Skips** unless
    `AISHE_REALTEST_KEY` is set (default endpoint: Groq
    `openai/gpt-oss-120b`; override with `AISHE_REALTEST_BASE_URL`/`_MODEL`).
    `tests/live_contract_test.py` tests the shared response validator without a
    key.
  - `tests/real_fuzz.py`: opt-in **real-model robustness fuzz** — generated inputs
    (questions, tasks, prompt-injection, metacharacter lines) through
    the machine-readable `aishe suggest --json` contract against the live model,
    checking response-independent invariants (valid JSON, no crash/parse-leak,
    valid command syntax, risk/exit-code agreement, dangerous suggestions never
    greenlit). Same `AISHE_REALTEST_KEY` gate; scale with a multiplier arg
    (`real_fuzz.py <bin> 2`). Real API calls cost money and hit rate limits —
    keep the scale modest. `AISHE_REALTEST_TIMEOUT` controls the outer
    per-process deadline (300 seconds by default, outside AIShe's internal retry
    envelope).
  - `tests/live_release.py`: paid, isolated release-candidate matrix combining
    live provider/Doctor capability checks, answer/command contracts, a real
    yolo function-tool round trip, classification, and scaled fuzz. It defaults
    to GPT-5.6 Luna and accepts the credential only through
    `AISHE_REALTEST_KEY`.
  - `tests/admin_validation.py`: the end-to-end validation harness.

## Validation harness

Use the unified driver for normal qualification. It always builds and verifies
the release binary before an external harness and writes one versioned evidence
report with commands, durations, skips, host/runtime/threat-model identity, and
artifact digests:

```sh
python3 tests/qualify.py --list
python3 tests/qualify.py quick --output test-results/qualification-quick.json
python3 tests/qualify.py local-full --output test-results/qualification-local.json
python3 tests/qualify.py linux-full --output test-results/qualification-linux.json
python3 tests/qualify.py release --output test-results/qualification-release.json
AISHE_REALTEST_KEY=... python3 tests/qualify.py paid-live \
  --output test-results/qualification-paid-live.json
```

`linux-full` is for a Linux host and requires the Linux-specific installer,
credential-isolation, tmux, and screen gates. `release` records non-applicable
platform gates explicitly. `paid-live` makes every credentialed gate required;
a missing credential yields `incomplete`, never pass. The individual commands
below remain useful for focused development and diagnosis.

`tests/admin_validation.py` is a repeatable harness that exercises a large
surface area and writes a timestamped Markdown report under `test-results/`. It
has seven suites:

1. Shell pass-through: runs many commands and shell constructs through `aishe -c`
   and compares output to the raw shell. Anything misrouted to the LLM shows up as
   a mismatch. Run with no API key.
2. Admin file operations: create, edit, move, permission, archive, and delete
   files, verifying on-disk state.
3. Natural-language behavior when a real-model credential is provided; skipped
   otherwise.
4. Plugins, slash-commands, and skills (deterministic): registered built-in
   listings, custom command discovery, `shell` and `$ARGUMENTS` templating,
   no-frontmatter discovery, and name-collision precedence (a same-named project
   command does not shadow the user's — the user's command wins).
5. Dispatch classification: asserts each input routes to shell or natural
   language independently of output, including supported slash commands and
   explicit route prefixes.
6. Config and command robustness: a config exercising every newer field
   round-trips through the configuration surfaces, `aishe doctor` passes, the
   repo example config parses, and supported shell controls behave. No model
   needed.
7. MCP handshake/list/call coverage with a real local stdio server and
   best-effort installed servers.

`tests/pty_smoke.py` drives the zsh-PTY front-end as part of the suite too.

Run it:

```sh
cargo build --release --locked
python3 tests/admin_validation.py            # deterministic suites need no key
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_runtime_contract.py target/release/aishe
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_host_scope.py target/release/aishe
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_connection_isolation.py target/release/aishe
python3 tests/model_picker_pty.py target/release/aishe
# In-shell regressions from the 2026-09-03 daily-driver review
python3 tests/in_shell_menus_pty.py target/release/aishe
python3 tests/yolo_consent_pty.py target/release/aishe
python3 tests/palette_pty.py target/release/aishe
python3 tests/mode_handoff_pty.py target/release/aishe
python3 tests/bare_words_pty.py target/release/aishe
python3 tests/theme_prompt_pty.py target/release/aishe
python3 tests/keys_pty.py target/release/aishe
python3 tests/picker_arrows_pty.py target/release/aishe
python3 tests/statusline_width_pty.py target/release/aishe
python3 tests/docs_cli_block_test.py target/release/aishe
python3 tests/direct_shell_benchmark.py target/release/aishe
python3 tests/bash_hook.py target/release/aishe --bash bash --require-current-family
python3 tests/terminal_compat_test.py
python3 tests/terminal_compat.py target/release/aishe \
  --json test-results/terminal-compat.json

# Full fake-provider release qualifications:
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_soak.py target/release/aishe \
    --turns 1000 --cold-cycles 40 --warm-probes 200 --reconnect-every 25
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_soak.py target/release/aishe \
    --turns 20 --cold-cycles 3 --warm-probes 20 \
    --lifecycle-hours 24 --lifecycle-interval 300
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/opencode_concurrency.py target/release/aishe --sessions 100

# On a disposable node only:
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/reboot_persistence.py prepare \
    target/release/aishe /persistent/aishe-reboot-fixture
# reboot and reconnect
AISHE_RUNTIME_DIR=/path/to/test-runtime \
  python3 tests/reboot_persistence.py verify \
    target/release/aishe /persistent/aishe-reboot-fixture
```

To run the natural-language suite, provide an API key in the environment (the
harness reads `GROQ_API_KEY` or a local secrets file it documents at the top).

The deterministic suites are the pass gate; the natural-language suite is
informational because model output varies.

For an authorized SSH target with the candidate installed, qualify the remote
PTY explicitly. Omitting the target records a limitation rather than a pass:

```sh
python3 tests/terminal_compat.py target/release/aishe \
  --capability ssh --require-capability ssh \
  --ssh-target user@qualification-host \
  --ssh-identity ~/.ssh/qualification-key --ssh-binary /path/to/aishe
```

For live official-OpenAI coverage, the opt-in classification and robustness
suites accept the endpoint and model explicitly:

```sh
export AISHE_REALTEST_KEY="$OPENAI_API_KEY"
export AISHE_REALTEST_BASE_URL="https://api.openai.com"
export AISHE_REALTEST_MODEL="gpt-5.6-luna"

python3 tests/real_model.py target/release/aishe
python3 tests/real_fuzz.py target/release/aishe 10  # hundreds of paid API calls
```

Run a high-scale live fuzz only on a disposable test node, with a cost/rate-limit
budget. The deterministic PTY fuzzer remains the preferred high-volume gate;
live requests add provider-contract and model-behavior coverage, not perfectly
repeatable assertions.

## CI

GitHub Actions runs the cross-platform Rust tests, a bounded blocking macOS PTY
job, the pinned OpenCode runtime contract, plus the deterministic
pseudo-terminal suites (`pty_scenarios.py`, `pty_fuzz.py`, `zsh_features.py`)
and the current Linux Bash 5 hook matrix. Linux also requires the local, tmux,
and screen terminal contract; macOS requires the local 300 ms latency/resize
contract plus routing, picker, statusline, setup, and signal PTY suites. All use
the fake provider on every push and PR; `real_model.py` runs only when the
`AISHE_REALTEST_KEY` secret is present. A candidate must also satisfy the
[release-readiness and rollback policy](release-readiness.md); deterministic
platform skips are holds, and paid/live skips require an owned disposition.
Tagged releases build the binaries,
`.deb`/`.rpm` packages, man page, pinned runtime assets, license/notices, SBOM,
checksums, and provenance (see `.github/workflows/release.yml`).

For a contributor's map of the codebase, see [architecture.md](architecture.md).

## Adding to the harness

The harness is designed to grow. Add rows to the case lists at the top of
`tests/admin_validation.py` (`SHELL_CASES`, `FILE_OPS`, `DISPATCH_SHELL`,
`DISPATCH_NL`, `NL_SUGGEST`), or add command and skill fixtures in
`install_plugins`.

## Coding conventions

- Match the style of the surrounding code.
- Keep clippy and rustfmt clean.
- Prefer the deterministic test suites for anything that does not strictly need a
  live model.
