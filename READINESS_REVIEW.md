> **Lifecycle: Historical.** Baseline: v0.2.23 (`62654ef`). This review records
> its original release-readiness findings; use the
> [post-0.6.5 plan](docs/design/NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) for current
> requirements and priorities.

# aishe — CLI Shell-Tool Readiness Review

**Reviewer:** Fable 5 · **Date:** 2026-06-13 · **Commit:** `62654ef` (v0.2.23)
**Lens:** "Can any engineer easily install `aishe` and leverage it as a command-line shell tool?"

This document is a **task list**. Each task is self-contained: problem, evidence,
proposed fix, files, and acceptance criteria.

---

## Implementation status (updated 2026-06-13)

**All 14 tasks implemented and validated**, except two that require maintainer
credentials/infra (noted below).

| Task | Status |
|---|---|
| T-01 stop committing `test-results/` | ✅ done (`.gitignore` + `git rm --cached`) |
| T-02 enforce MSRV 1.80 in CI | ✅ done (new `msrv` job) |
| T-03 wire `real_fuzz.py` into CI + docs | ✅ done (CI step + `development.md`) |
| T-04 man page | ✅ done (`aishe man` via clap_mangen; wired into install.sh, release, nfpm, brew) |
| T-05 automation interface | ✅ done (`aishe suggest [--json]`, stable exit-code contract, tests) |
| T-06 macOS parity explicit | ✅ done (README/safety/modes qualifiers; runtime notices already present) |
| T-07 crates.io / Homebrew | ⚙️ partial — metadata added, `cargo publish --dry-run` passes, formula installs the man page; **actual `cargo publish` + creating the `homebrew-tap` repo require maintainer credentials** |
| T-08 README quickstart | ✅ done ("Get productive in 60 seconds") |
| T-09 panic/`unwrap` audit | ✅ done (dispatcher/executor/fallback lock-poison recovery; fallback `unreachable!` → graceful error) |
| T-10 bound `history.ext` growth | ✅ done (trim-on-append past 4 MB) |
| T-11 probe embedding endpoint | ✅ done (`doctor --probe` shows `embedding (...)`) |
| T-12 filter trivial commands from index | ✅ done (`is_trivial`) |
| T-13 `CONTRIBUTING.md` | ✅ done |
| T-14 tidy planning docs | ✅ done (PRD/PLAN → `docs/design/`) |

**Remaining maintainer actions (T-07):** run `cargo publish` with a crates.io
token, and create a `billiondollarsolo/homebrew-tap` repo (then optionally automate
the formula push in `release.yml`). The crate is verified publishable
(`cargo publish --dry-run` succeeds) and the formula is ready to drop into a tap.

---

## Verdict

**Foundation is strong; a handful of well-scoped fixes stand between "works for the
author" and "any engineer installs it and is productive in 5 minutes."**

The install story (one-liner + `.deb`/`.rpm` + `cargo binstall` + musl-static
Linux binary + checksum verification), the `--help`/`doctor`/completions UX, the
non-blocking non-interactive fallback, and the test coverage are all genuinely
good. The gaps are mostly **repo hygiene, MSRV/CI enforcement, discoverability
(man page, crates.io), a documented machine-readable interface for automation, and
macOS parity clarity** — not core functionality.

**What's already solid (do NOT redo):**
- `aishe --help` lists all subcommands with clear descriptions; `--version` embeds
  the git SHA + date.
- `aishe doctor` gives a colored, actionable environment readout (shell, config,
  provider, key, dry-run availability, audit, MCP, history) with fix hints.
- First-run in a non-interactive context **writes defaults and proceeds** with a
  clear message (does not block on a wizard) — good for CI/scripts.
- Exit codes are correct: shell commands propagate (`!false` → 1); NL with no
  provider → 1; success → 0.
- `install.sh` verifies the release checksum, prefers the static musl build, and
  ensures `zsh` is present.
- Shell completions generate for zsh/bash/fish (`aishe completions <shell>`).
- CI runs fmt + clippy `-D warnings` + build + test on **ubuntu & macos**, plus a
  full PTY suite (smoke/scenarios/fuzz/features/signals) and admin validation.
- `Cargo.lock` is committed; build is offline and reproducible; binary is 8.2 MB.

---

## Priority legend
- **P0** — adoption blocker or repo-hygiene issue; do first.
- **P1** — meaningfully improves how easily an engineer adopts/integrates it.
- **P2** — polish / nice-to-have.

---

## P0 — Adoption blockers & hygiene

### T-01 · Stop committing `test-results/` run artifacts (P0)
**Problem:** 62 test-run report files are committed to the repo (`validation-*.md`,
`fuzz-*.md`, `real-model-*.md`, `real-fuzz-*.md`). These are generated artifacts —
they bloat the repo, create noise in every diff/PR, and grow unbounded.
**Evidence:** `git ls-files test-results/ | wc -l` → **62**; `.gitignore` does not
list `test-results/`.
**Fix:**
1. Add `test-results/` (or `/test-results/`) to `.gitignore`.
2. `git rm -r --cached test-results/` and commit the removal.
3. Have the test harnesses keep writing there locally (unchanged) — just untracked.
4. If a report ever needs to be shared, attach it to a release or PR instead of
   committing it.
**Files:** `.gitignore`, `test-results/*` (remove from index), any tooling/hook that
demanded committing them.
**Acceptance:** `git status` is clean after a full test run; `git ls-files
test-results/` is empty.

### T-02 · Enforce the declared MSRV (1.80) in CI (P0)
**Problem:** `Cargo.toml` declares `rust-version = "1.80"`, but CI builds with
`dtolnay/rust-toolchain@stable`. A newer-than-1.80 API can be introduced without CI
catching it (this already happened once — `is_none_or` is 1.82). Engineers on an
older pinned toolchain would hit a build failure the project claims not to have.
**Evidence:** `.github/workflows/ci.yml` uses `@stable`; no `1.80` job.
**Fix:** Add a dedicated CI job that installs `1.80` and runs `cargo build`
(optionally `cargo test`) — `dtolnay/rust-toolchain@1.80`. Keep the stable job for
fmt/clippy. Optionally add a `clippy::incompatible_msrv` check.
**Files:** `.github/workflows/ci.yml`.
**Acceptance:** CI fails if a >1.80 API is used; green on 1.80 today.

### T-03 · Wire `real_fuzz.py` into CI and document the real-model harnesses (P1→P0 for confidence)
**Problem:** The new `tests/real_fuzz.py` (real-model robustness/injection fuzz) is
not referenced by CI, so it silently rots. `real_model.py` is wired but only via a
secret.
**Evidence:** `.github/workflows/ci.yml` `pty-smoke` job lists `real_model.py` but
not `real_fuzz.py`.
**Fix:** Add a `real_fuzz.py` step (opt-in via the same `AISHE_REALTEST_KEY`
secret; it SKIPs cleanly without it). Add a one-paragraph "Real-model tests" note in
`docs/development.md` describing both harnesses, the env vars, and the rate-limit
caveat.
**Files:** `.github/workflows/ci.yml`, `docs/development.md`.
**Acceptance:** CI invokes `real_fuzz.py`; it SKIPs without the secret and runs with it.

---

## P1 — Smooth engineer adoption & integration

### T-04 · Ship a man page (P1)
**Problem:** No `man aishe`. Engineers on Linux/macOS expect one; packagers expect a
`.1` to install.
**Evidence:** `find . -name '*.1'` → none (only git tag refs).
**Fix:** Generate a man page from the clap `Command` using `clap_mangen` (build
script or an `xtask`/`aishe completions`-style subcommand, e.g. `aishe man`).
Install it via `install.sh`, the `.deb`/`.rpm` (`nfpm.yaml`), and the Homebrew
formula (`man1.install`).
**Files:** `build.rs` or a new gen path, `Cargo.toml` (dev-dep `clap_mangen`),
`nfpm.yaml`, `packaging/aishe.rb`, `install.sh`.
**Acceptance:** `man aishe` renders after a package install; `aishe man`
(or the build) emits valid roff.

### T-05 · Document a stable machine-readable interface for automation (P1)
**Problem:** For scripting aishe (get a suggested command programmatically), the only
paths are the **hidden** hook flags (`--suggest-line`, `--auto-line`) which are
`hide = true` and undocumented as a public contract. There's no documented stable
output an engineer can rely on for automation (e.g. `aishe suggest "..." --json`).
**Evidence:** `--help` shows no `--json`/`--quiet`; `--suggest-line` is
`#[arg(long, hide = true)]`; no "scripting/automation" doc page.
**Fix (pick one, document it as stable):**
- Add a public `aishe suggest "<nl>" [--json]` subcommand that prints the
  suggested command (and, with `--json`, `{command, explanation, risk}`), with a
  documented exit-code contract (0 = safe/answer, non-zero = flagged). Reuse the
  existing `--auto-line` logic.
- Write `docs/automation.md`: exit codes, `-c` semantics, env-var configuration for
  non-interactive/CI use (`AISHE_MODE`, provider/key), and the stable output shape.
**Files:** `src/main.rs` (new subcommand), `docs/automation.md`, README (link it).
**Acceptance:** An engineer can do `cmd=$(aishe suggest "..." --json | jq -r .command)`
and rely on it across minor versions.

### T-06 · Make the macOS parity gap explicit and first-class (P1)
**Problem:** The reversibility/isolation net (`bwrap` sandbox, `aishe dry-run`,
`yolo_dry_run`) is **Linux-only**. On macOS these degrade to the best-effort policy
gate. An engineer on a Mac who reads the "reversible autonomy" pitch may assume the
sandbox is active and it is not.
**Evidence:** `sandbox.rs`/`overlay.rs` require `bwrap`; `doctor` shows
`dry-run: needs bubblewrap` without it; docs mention Linux-only but the README
Features bullets don't flag it.
**Fix:**
1. In README Features (the ↩️ Reversible bullet) and `docs/safety.md`, add an
   explicit "(Linux; needs bubblewrap)" qualifier next to `dry-run`/`yolo_dry_run`.
2. On macOS, have `aishe dry-run` / `yolo_dry_run` print a one-line notice that the
   sandbox is unavailable and it's running with the policy gate only (or refuse,
   configurable).
3. (Optional, larger) Investigate a macOS isolation path (`sandbox-exec`/Seatbelt)
   as a future proposal — note only.
**Files:** `README.md`, `docs/safety.md`, `docs/modes.md`, `src/main.rs`
(dry-run notice), `src/modes/yolo.rs` (yolo_dry_run notice).
**Acceptance:** A macOS engineer is never misled about whether isolation is active.

### T-07 · Publish to crates.io and/or a Homebrew tap (P1)
**Problem:** `cargo install aishe` (from the registry) and `brew install aishe`
(from a tap) don't work — the Homebrew formula is a **template** in `packaging/`, and
the crate isn't published. These are the two paths many engineers reach for first.
**Evidence:** `packaging/aishe.rb` is a fill-in template (SHA placeholders);
Cargo.toml has no `homepage`/`documentation`; no tap repo referenced.
**Fix:**
1. Add `homepage` and `documentation` to `[package]` in `Cargo.toml`.
2. `cargo publish` (dry-run first: `cargo publish --dry-run`); ensure `readme`,
   `license`, `keywords`, `categories` are set (they are).
3. Create/point to a `homebrew-tap` repo and have `release.yml` push the filled-in
   formula there on tag (SHA256s are already produced by the release).
**Files:** `Cargo.toml`, `.github/workflows/release.yml`, a `homebrew-tap` repo,
README install section.
**Acceptance:** `cargo install aishe` and `brew install <tap>/aishe` both work.

### T-08 · Add a copy-pasteable "Quickstart for engineers" up top (P1)
**Problem:** The README is good but the fastest path — install → export key → one
useful command → add the shell hook — is spread across sections. Engineers want a
3–4 line block that gets them value immediately.
**Evidence:** README has Features → Install → Quickstart; the shell-hook `eval` line
is under "Front-ends," not adjacent to first use.
**Fix:** Add a 6–8 line "Get productive in 60 seconds" block right after the badges:
install one-liner, `export ANTHROPIC_API_KEY=…`, `aishe -c "…"`, and
`eval "$(aishe init zsh)"` for the persistent hook — each with a one-line comment.
**Files:** `README.md`.
**Acceptance:** A new engineer copies 4 lines and gets a working suggestion + hook.

---

## P2 — Polish & robustness

### T-09 · Panic/`unwrap` audit on runtime paths (P2)
**Problem:** A long-lived shell tool should not panic on bad input/IO. There are
~200 `unwrap()/expect()/panic!` hits across `src/` (most are in `#[cfg(test)]` or on
compile-time constants like `Regex::new(...).expect(...)`, which are fine), but a few
sit in runtime modules and deserve a look.
**Evidence:** Non-test hotspots: `src/dispatcher.rs` (9), `src/providers/fallback.rs`
(4), `src/executor.rs` (2), `src/redact.rs` (2), `src/safety.rs` (2 — constant regex,
OK).
**Fix:** Audit the non-constant, non-test `unwrap()/expect()` in `dispatcher.rs`,
`fallback.rs`, `executor.rs`, `redact.rs`. Convert any that touch runtime data
(env, IO, parsing, model output) to graceful handling. Add
`#![deny(clippy::unwrap_used)]` scoped to the runtime modules (allow in tests) if
practical, or a documented convention.
**Files:** `src/dispatcher.rs`, `src/providers/fallback.rs`, `src/executor.rs`,
`src/redact.rs`.
**Acceptance:** No runtime-data `unwrap()` in the audited modules; a fuzz/edge input
never panics (rc 101).

### T-10 · Bound the aishe history log growth outside the PTY exit path (P2)
**Problem:** `history.ext` is capped only on interactive zsh exit (via the zshexit
hook). Heavy `-c`/hook usage appends without a cap between interactive sessions.
**Evidence:** `src/integration.rs` `aishe_zshexit` caps to 10k on exit; the executor
`histlog::append` path has no cap.
**Fix:** Cap on write in `histlog::append` (e.g. opportunistically trim when the file
exceeds ~20k lines), or on read in `histlog::read`. Keep it cheap and race-tolerant.
**Files:** `src/histlog.rs`.
**Acceptance:** `history.ext` stays bounded under sustained `-c` usage without an
interactive session.

### T-11 · `aishe doctor --probe` should also check the embedding provider (P2)
**Problem:** When `semantic_history` is on, the embedding endpoint may differ from
the chat provider, but `doctor --probe` only probes the chat chain.
**Evidence:** `providers::chain_names` (chat) is probed; `embedding_provider` is not.
**Fix:** If `semantic_history` is enabled, also probe the embedding provider/endpoint
and report it in the `--probe` section.
**Files:** `src/main.rs` (doctor), `src/providers/mod.rs`.
**Acceptance:** `doctor --probe` reports embedding-endpoint reachability when
semantic history is enabled.

### T-12 · Filter trivial commands from semantic-history indexing (P2)
**Problem:** `exit`, `cd`, `ls`, `clear`, etc. get embedded into the semantic index,
diluting recall quality and spending embedding tokens on noise.
**Evidence:** `semhist::candidates` only excludes `aishe history …`; a manual PTY run
indexed `exit`.
**Fix:** Extend `is_history_mgmt`/`candidates` to skip a small set of trivial no-value
commands (configurable or a fixed short list: `exit logout clear cd ls pwd`).
**Files:** `src/semhist.rs`.
**Acceptance:** Trivial commands don't appear in `aishe history search` results.

### T-13 · Add `CONTRIBUTING.md` and a "for contributors/integrators" entry point (P2)
**Problem:** `docs/development.md` and `docs/architecture.md` exist, but there's no
root `CONTRIBUTING.md` — the conventional first stop for an engineer wanting to build,
test, or extend the tool.
**Evidence:** No `CONTRIBUTING.md` at root; `SECURITY.md` exists (good).
**Fix:** Add `CONTRIBUTING.md`: build (`cargo build`), the full gate (fmt/clippy/test
+ the python PTY harnesses + how to run the real-model tests), MSRV, module map (link
`docs/architecture.md`), and the release process.
**Files:** `CONTRIBUTING.md`, README (link it).
**Acceptance:** A contributor can go from clone → green local gate using only
`CONTRIBUTING.md`.

### T-14 · Trim/clarify root-level planning docs (P2)
**Problem:** `PRD.md`, `docs/PLAN.md`, `docs/ROADMAP.md`, and `docs/proposals.md`
overlap and some are internal planning noise for a consumer of the tool.
**Evidence:** Four planning-ish docs at root/docs.
**Fix:** Keep `docs/proposals.md` (now a shipped-status record) and `docs/ROADMAP.md`;
move `PRD.md`/`PLAN.md` under a `docs/design/` folder or mark them clearly as
historical, so the docs index an engineer sees is product-focused.
**Files:** `PRD.md`, `docs/PLAN.md`, `README.md` docs list.
**Acceptance:** The docs list a new engineer sees contains only user/integration docs.

---

## Suggested implementation order
1. **T-01, T-02, T-03** (hygiene + CI correctness) — small, high-signal, unblock trust.
2. **T-08, T-06** (README quickstart + macOS honesty) — pure docs, immediate adoption lift.
3. **T-05, T-04** (automation interface + man page) — the two biggest "leverage as a
   CLI tool" enablers.
4. **T-07** (crates.io / Homebrew tap) — broad install reach.
5. **T-09 … T-14** (robustness + polish) as capacity allows.

## Not in scope here (already handled or intentionally deferred)
- Core features (modes, safety gate, sandbox, dry-run, undo, semantic history,
  runbooks) — shipped and tested.
- N2 editable plan checklist — deferred by design (advisory plan ≠ executed contract).
- Broad security audit of the gate/sandbox — recommended before a 1.0/GA claim, but a
  separate effort from CLI-tool readiness.
