# Development

## Build and check

Building needs **Rust 1.88 or newer** (some normal dependencies pulled in via
`syntect` require it); the prebuilt release binaries need no toolchain.

```sh
cargo build --locked
cargo test --all-targets --locked  # integration tests spawn a real shell
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
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
    `tests/durable_task_resume.py`: real-PTY setup/settings transactions,
    right/below/off live status, and kill/resume without duplicate tool
    execution.
  - `tests/installer_upgrade.sh`: hermetic replacement install that hashes
    config, shared history, tasks, and unrelated state before/after.
  - `tests/provider_unauthenticated.py`: real local HTTP endpoint proving
    loopback providers can list/validate models with no dummy API key.
  - `tests/pty_smoke.py`: PTY smoke test for the zsh-PTY front-end.
  - `tests/real_model.py`: opt-in command-vs-answer classification corpus against a
    live model. **Skips** unless `AISHE_REALTEST_KEY` is set (default endpoint: Groq
    `openai/gpt-oss-120b`; override with `AISHE_REALTEST_BASE_URL`/`_MODEL`).
  - `tests/real_fuzz.py`: opt-in **real-model robustness fuzz** — generated inputs
    (questions, tasks, prompt-injection, metacharacter lines) through
    the machine-readable `aishe suggest --json` contract against the live model,
    checking response-independent invariants (valid JSON, no crash/parse-leak,
    valid command syntax, risk/exit-code agreement, dangerous suggestions never
    greenlit). Same `AISHE_REALTEST_KEY` gate; scale with a multiplier arg
    (`real_fuzz.py <bin> 2`). Real API calls cost money and hit rate limits —
    keep the scale modest.
  - `tests/admin_validation.py`: the end-to-end validation harness.

## Validation harness

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
4. Plugins, slash-commands, and skills (deterministic): meta listings, custom
   command discovery, `shell` and `$ARGUMENTS` templating, no-frontmatter
   discovery, and name-collision precedence (a same-named project command does not
   shadow the user's — the user's command wins).
5. Dispatch classification: asserts each input routes to shell or natural language
   independent of output (including the `/usage`, `/reset`, `/ghost` meta slashes).
6. Config and meta robustness: a config exercising every newer field round-trips
   through `/config`, `aishe doctor` passes, the repo example config parses, and
   the new meta commands behave. No model needed.
7. MCP handshake/list/call coverage with a real local stdio server and
   best-effort installed servers.

`tests/pty_smoke.py` drives the zsh-PTY front-end as part of the suite too.

Run it:

```sh
cargo build --release --locked
python3 tests/admin_validation.py            # deterministic suites need no key
```

To run the natural-language suite, provide an API key in the environment (the
harness reads `GROQ_API_KEY` or a local secrets file it documents at the top).

The deterministic suites are the pass gate; the natural-language suite is
informational because model output varies.

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

GitHub Actions runs the cross-platform Rust tests plus the deterministic
pseudo-terminal suites (`pty_scenarios.py`, `pty_fuzz.py`, `zsh_features.py`)
with the fake provider on every push and PR; `real_model.py` runs only when the
`AISHE_REALTEST_KEY` secret is present. Tagged releases build the binaries,
`.deb`/`.rpm` packages, and the man page (see `.github/workflows/release.yml`).

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
