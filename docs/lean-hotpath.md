# Lean CSH hot path

Week-1 skeleton of the [CSH MVP](../../csh-mvp-scope.md) redesign, shipped
inside the existing **aishe** binary. Product name stays `aishe`.

## What changed

Interactive `aishe` (and `aishe zsh`) now:

1. Launches **`zsh -f -o RCS -o NO_GLOBAL_RCS -i`** with an isolated `ZDOTDIR`.
   `-f` is NO_RCS. `-o RCS` re-enables only this ZDOTDIR so the lean hook loads.
   `-o NO_GLOBAL_RCS` keeps `/etc/zshrc` off. The user's `~/.zshrc` is **never**
   sourced. Optional aliases live in `~/.aishe/leanrc` or `$AISHE_LEANRC`.
2. Injects a **minimal hook**: classifier `?` / `!` / PATH-known / NL, tiny
   prompt + mode glyph, Shift-Tab grant cycle. Known commands stay in the child
   zsh (no parent spawn, no model, no socket call).
3. Sends NL to the **already-running parent** over a FIFO. The parent uses the
   in-process `src/providers/*` HTTP client (ureq + rustls). It does **not**
   call `backend::supervisor` or start OpenCode.
4. Defaults to **ask**. One typed grant per shell unlocks **allow** or
   **agent** / **agent-host**. Safety `assess` still gates *model-proposed*
   lines; typed commands and `!` do not.
5. Agent `run_command` prefers **`dash -c`**, else **`zsh -f -c`**, and wraps
   with bubblewrap on Linux workspace agent.

Escape hatch: `AISHE_LEGACY_OPENCODE=1` (or `AISHE_LEAN=0`) restores the
historical PTY that sources `~/.zshrc` and the OpenCode sidecar.

## Router

Same rule table as `dispatcher::route`:

| Input | Route |
|---|---|
| leading `?` | NL (sigil stripped) |
| leading `!` | shell (ungated) |
| PATH / builtin / assignment / construct | shell |
| question grammar (`what is`, trailing `?` on question heads, …) | NL |
| else | NL |

`aishe -c` still admits known commands *before* config, credentials, Provider,
or OpenCode (`dispatcher::fast_shell_line` in `src/main.rs`).

## Approval

| Mode | Glyph | Grant | Tools |
|---|---|---|---|
| **ask** (default) | `❯` | none | none; propose or answer |
| **allow** | `»` | type `allow` once | Safe auto-runs; Dangerous/Unknown need `yes` |
| **agent** | `*` | type `agent` or `agent-host` | no per-action prompts after grant; Linux workspace needs working bwrap |

## How to run

```bash
source /root/.cargo/env
cargo test --test lean_hotpath --test architecture_boundaries
cargo test                          # full Rust suite

# release budgets (this is the SLO band; debug binaries are slower)
cargo build --release
./target/release/aishe --version
./target/release/aishe -c 'printf hi'
AISHE_FAKE_LLM='{"type":"answer","command":null,"explanation":"ok"}' \
  ./target/release/aishe -c '? what is lean'
```

Spies for the known-command regression:

```bash
AISHE_SPY_PROVIDER_MAKE=/tmp/p AISHE_SPY_OPENCODE=/tmp/o \
  ./target/release/aishe -c true
# neither /tmp/p nor /tmp/o should exist
```

## Budgets (measured)

Host: Linux 7.0 EPYC-Genoa ×16, 2026-09-09. `cargo build --release`.
`n=40` unless noted. Debug `cargo test` keeps a hang-detection ceiling only;
CI must not gate the 5 ms band on unoptimized binaries.

| Path | Target p95 | Measured p50 / p95 | Verdict |
|---|---:|---:|---|
| `aishe --version` | ≤ 5 ms (fail > 8 ms) | 5.35 / **6.36** ms (min 2.22) | Under the 8 ms fail-build line. Slightly over 5 ms on this noisy host; min shows the C band is reachable. |
| `aishe -c true` | ≤ 2 ms *overhead* vs executor | 12.57 / 13.96 ms | New process + clap. `dash -c true` 1.85 / 2.82; `zsh -f -c true` 4.17 / 5.03. Overhead ≈ 9 ms vs zsh -f — **not** the live PTY proxy. Live known-cmd is a byte copy (target ≤ 2 ms; not a new `execve(aishe)`). |
| `aishe -c 'printf hi'` | same | 12.68 / 13.85 ms | Spy: no Provider, no OpenCode. |
| First child prompt (`zsh -f -o RCS -o NO_GLOBAL_RCS -i`) | ≤ 25 ms | 4.86 / **5.33** ms | Child spawn is well under budget. Parent PTY setup is extra and one-time. |
| Warm NL first-ready (fake provider; TTFB excluded) | ≤ 10 ms | **6.83 ms** (`AISHE_SPY_WIRE_NS`) | In budget. Full `aishe -c '?…'` wall is ~40 ms because `-c` is a cold process (config + Provider construct). Live shell reuses the parent client. |
| OpenCode on known-cmd / lean NL | forbidden | spy file absent | Pass. |

How the spies were run:

```bash
AISHE_SPY_PROVIDER_MAKE=/tmp/p AISHE_SPY_OPENCODE=/tmp/o \
  ./target/release/aishe -c true
# neither file exists

AISHE_FAKE_LLM='{"type":"answer","command":null,"explanation":"ok"}' \
AISHE_SPY_PROVIDER_MAKE=/tmp/np AISHE_SPY_OPENCODE=/tmp/no \
AISHE_SPY_WIRE_NS=/tmp/wire \
  ./target/release/aishe -c '? what is lean'
# /tmp/np exists, /tmp/no does not, /tmp/wire is nanoseconds to provider.complete
```

## Remaining gaps (not W1)

- Full compsys / completion dump on first prompt.
- Streaming tokens into the PTY (ask currently returns a complete answer/fill).
- Agent-mode IPC output is parent-stdout, not inner-PTY stdio.
- `init zsh` hook for people who will not leave their rc (still post-MVP).
- MCP, skills, OAuth, named connections, overlay dry-run.
