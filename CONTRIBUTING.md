# Contributing to aishe

Thanks for helping build aishe. This is the fast path from clone to a green local
gate, plus the conventions the project follows.

## Prerequisites
- **Rust** ≥ 1.88 (the declared MSRV — CI builds on both 1.88 and stable).
- **zsh** on `$PATH` (the interactive front-end drives your real zsh; the PTY test
  harnesses need it).
- **Python 3** (the PTY/real-model test harnesses are Python scripts).
- **bubblewrap** (`bwrap`), optional — enables the sandbox / `dry-run` tests on
  Linux; those tests skip cleanly without it.

## Build
```sh
cargo build            # debug
cargo build --release  # what the PTY/admin harnesses run against
```
`Cargo.lock` is committed and the build is fully offline.

## The full local gate
Run this before opening a PR — it mirrors CI:
```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test                                   # unit + integration (spawns a real shell)

# PTY harnesses (need zsh; use the release binary)
cargo build --release
python3 tests/pty_smoke.py     target/release/aishe
python3 tests/pty_scenarios.py target/release/aishe
python3 tests/pty_fuzz.py      target/release/aishe        # add a multiplier to scale: `... 5`
python3 tests/zsh_features.py  target/release/aishe
python3 tests/pty_signals.py   target/release/aishe
python3 tests/admin_validation.py target/release/aishe
```

### Real-model tests (opt-in)
Two harnesses exercise a *live* model (default: Groq `openai/gpt-oss-120b`). They
**skip** unless `AISHE_REALTEST_KEY` is set; no key is ever written to the repo.
```sh
export AISHE_REALTEST_KEY=...        # set once
# optional: AISHE_REALTEST_BASE_URL, AISHE_REALTEST_MODEL
python3 tests/real_model.py target/release/aishe    # command-vs-answer classification
python3 tests/real_fuzz.py  target/release/aishe 2  # robustness + prompt-injection under a real model
```
These make real API calls (cost + rate limits) — keep the scale modest.

## Conventions
- **Formatting/lints:** `cargo fmt`; clippy must be clean under `-D warnings`.
- **MSRV:** don't use APIs newer than Rust 1.88 (CI's 1.88 job will catch it).
- **No panics on runtime data:** avoid `unwrap()/expect()` on env/IO/parsing/model
  output in runtime paths; recover or propagate. `expect()` on compile-time
  constants (e.g. a static regex) is fine.
- **Tests:** every behavior change ships with a test. Safety-gate changes must keep
  `tests/safety_corpus.rs` green (add new cases rather than loosening it).
- **Docs:** user-facing changes update the relevant `docs/*.md` and `CHANGELOG.md`.

## Module map
See [docs/architecture.md](docs/architecture.md) for the module-by-module tour
(dispatcher, executor, providers, safety, sandbox, overlay, modes, …).

## Releases
Releases are cut from `main` via the `release.yml` workflow (`workflow_dispatch`
with a version). It builds the cross-platform binaries + `.deb`/`.rpm`, attaches
them with checksums, and tags `vX.Y.Z`. Bump `Cargo.toml`, `packaging/aishe.rb`,
and `CHANGELOG.md` in the release commit.

## Security
Please report vulnerabilities per [SECURITY.md](SECURITY.md), not via public issues.
