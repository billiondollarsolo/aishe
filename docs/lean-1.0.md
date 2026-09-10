# AISHE 1.0 — lean daily-driver acceptance

**Branch:** `feat/lean-csh-hotpath`  
**Date:** 2026-09-09  
**Crate version:** `1.0.0` (tag `v1.0.0` on `feat/lean-csh-hotpath`). Product is **1.0** on lean default; do not merge `main` from this doc alone.

Capability ≠ mechanism. Lean hot path never requires OpenCode. Grok subscription OAuth is the happy path; API keys are fallback. No secrets in docs or tests.

## Capability checklist → tests

| Cap | Feature | Lean proof | Test / gate |
|---|---|---|---|
| F01 | Known cmds in-process | child `zsh -f`, no FIFO/Provider | `lean_parity_wave5` live PTY latency |
| F02 | `?` / `!` / grammar | lean hook + Rust table | wave1–2 + hook corpus |
| F03–F05 | ask / allow / agent | FIFO NL → suggest / confirm / yolo | wave1 smoke + lib `lean::nl` + **wave7** noninteractive agent (`run_nl` + `AISHE_ACCEPTANCE_FILE` + FakeProvider) |
| F06–F07 | Tools + safety | yolo + `safety::assess` | lib + allow CONFIRM |
| F08–F09 | Grants + mode names | Shift-Tab / typed grant | wave3 aliases |
| F10–F11 | Sessions + undo | lean JSONL + `/undo` | wave2 |
| F13 | Grok OAuth | `~/.grok/auth.json` | `lean_live_xai` (gated) + doctor |
| F14 | Auth honesty | Grok OAuth + API-key; OpenAI OAuth **LEGACY** | docs + doctor next-step |
| F15 | `/connection` `/model` | FIFO list/pick | wave3 |
| F16–F17 | MCP / skills names | `/mcp` `/skills` | wave5 |
| F18 | compsys | private ZDOTDIR | wave1 |
| F19 | Token stream | `STREAM_END` + PtyOut | wave4 |
| F20–F21 | PTY + clean shell | shared PTY, no user rc | wave1 |
| F26 | Doctor lean | FIFO, grok, bwrap, leanrc | wave2 |
| F27 | Install w/o OpenCode | docs | wave3 docs |
| F28 | Lean smoke | spies green | wave1 |
| F30–F35 | Fix-last, @file, usage, details | lean-native | wave2/5 |
| F40 | Custom slash-commands | `commands.rs` → allowlist + FIFO | `lean_parity_wave6` |
| F34 | Details density | `/details` + Ctrl-O | wave5 |

## Auth (F14 honesty)

- **Happy path:** Grok CLI OAuth — `grok` login → `~/.grok/auth.json` (never printed).
- **Fallback:** API key via `aishe auth` / provider env (`XAI_API_KEY`, etc.).
- **Not claimed:** native OpenAI/Codex browser OAuth on the lean hot path. That remains **LEGACY/heavy** (`AISHE_LEGACY_OPENCODE=1` or `aishe auth` / OpenCode HOME profiles). Do not advertise it as lean-native.

First-run: `aishe doctor` warns when grok auth.json is missing and points at `grok` login (or API-key via `aishe auth`).

## Custom slash-commands (F40)

Reuse existing markdown discovery (`~/.config/aishe/commands/*.md`, project `.aishe/commands/`). No new DSL.

- Hook allowlist treats single-segment `/name` as lean slash (not PATH, not free NL).
- FIFO `SLASH` → `CommandRegistry::load` → expand → NL turn or shell (`shell:true` + safety / trust).
- `/help` and `/commands` list builtins + customs; Tab completes builtins + `AISHE_LEAN_CMDS_FILE`.

## Post-1.0 (explicitly skipped)

| ID | Item |
|---|---|
| F12 | Overlay dry-run |
| F22 | `init zsh` stay-in-rc |
| F23 | Bash hook as lean interactive |
| F24 | Palette / TUI chrome in PTY |
| F25 | Background tasks / worktrees / inbox |
| F41 | Org policy / profiles / roles |
| F42 | Semantic history / repo index |
| F14+ | Native OpenAI OAuth without OpenCode |

## Verify

```bash
export PATH=/root/.cargo/bin:$PATH
cargo test --lib
cargo test --test lean_parity_wave1 --test lean_parity_wave2 --test lean_parity_wave3 \
  --test lean_parity_wave4 --test lean_parity_wave5 --test lean_parity_wave6 --test lean_parity_wave7
# optional live:
# AISHE_LIVE_LLM=1 cargo test --test lean_live_xai -- --nocapture
```

## Daily-driver verdict

On this branch, lean is a **1.0-complete daily driver** for: typed shell, English NL (ask/allow/agent), Grok OAuth, sessions, undo, MCP/skills discovery, custom markdown slashes, doctor, and streaming answers — without OpenCode on the hot path. Post-1.0 items above remain CLI/LEGACY/opt-in.
