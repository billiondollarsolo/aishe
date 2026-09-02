<p align="center">
  <img src="assets/aishe-banner.png" alt="AIShe — AI Shell" width="640">
</p>

# AIShe — AI Shell

**AIShe** is **AI Shell**: your real shell, with an agent built into the command
line. The CLI package name is `aishe`.

> **Alpha (pre-1.0).** The product is usable day to day, but APIs, config shape,
> and UX can still change. Autonomous host access can make irreversible changes —
> prefer workspace scope and Linux isolation for untrusted work, read
> [the safety model](docs/safety.md), and keep backups.

AIShe runs an actual interactive zsh rather than emulating one, so aliases,
plugins, completion, job control, history, and ordinary commands keep their
native behavior. Input that is not a command becomes a plain-English request to
the AI.

The agent layer uses a private, compatibility-pinned OpenCode SDK/runtime for
reasoning, tools, durable conversations, compaction, and subagents. AIShe stays
the control plane: routing, execution scope, sandbox policy, approvals,
credentials, budgets, terminal rendering, and the audit trail. The result is an
AI-driven systems shell — not a chatbot parked next to a terminal.

```
~/projects/app ❯ git status            # runs exactly like zsh
~/projects/app ❯ whats eating my disk  # LLM suggests: du -sh * | sort -rh | head
```

**Full user guide:** [docs/](docs/README.md) · start with
[Getting started](docs/getting-started.md) · [Commands](docs/commands.md) ·
[Providers](docs/providers.md)

---

## Get productive in 60 seconds

```sh
# 1. Install + guided setup (includes API-key or subscription OAuth sign-in)
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh -s -- --setup

# 2. Use it — no shell hook required
aishe                                      # real zsh with aishe active
aishe -c "turn the logs directory into a tarball"
aishe suggest --json "list files by size" | jq -r .command

# 3. (Optional) make every new terminal AI-aware
echo 'eval "$(aishe init zsh)"' >> ~/.zshrc   # or: aishe init bash
```

### Everyday controls (in the shell)

| Do this | How |
|--------|-----|
| Help | **`/help`** · topics: `/help accounts` · `models` · `session` · `config` |
| Switch account / model | **`/connection`** · **`/model`** (model is *this account only*) |
| Cycle mode | **Shift-Tab** → suggest `❯` · auto `»` · yolo `*` |
| **Force English to the AI** | Start with **`?`** — e.g. `? install kubectl please` |
| Force raw shell | Start with **`!`** (bypasses the safety gate) |

**Common trap:** lines whose **first word is a real binary** run as shell — even
if the rest is English. `install` is `/usr/bin/install` on every Mac/Linux box,
so `install kubectl please` is **not** “please install the package”; use
`? install kubectl please`. Optional highlighting uses green for shell and
magenta for agent input; color is never required. Press **Ctrl-X ?** in zsh or
run `aishe route -- '<line>'` to read the route and reason as plain text.

Full routing, Option/Alt+Return, and Mac terminal Meta settings:
[Getting started §6](docs/getting-started.md#6-force-a-route-when-needed) ·
[Shell integration](docs/shell-integration.md#force-nl-and-input-prefixes).

---

## Features

- **Your real zsh, untouched.** Plugins, completions, prompt, aliases, key
  bindings, and job control work unmodified — not a shell reimplementation.
- **A real agent engine, still one shell.** Every AI turn uses AIShe's
  version-pinned OpenCode backend for durable conversations, reasoning,
  compaction, and subagents. It starts lazily on authenticated loopback, never
  opens a second TUI, and never hijacks direct zsh commands.
- **Plain English to commands.** Non-commands go to the model. **`?`** forces
  natural language when the first word is also a real binary (e.g. `install`);
  **`!`** forces raw shell past the safety gate.
- **Three modes.** `suggest` (propose, you confirm), `auto` (run safe commands,
  confirm risky ones), `yolo` (agent loop that runs, reads output, iterates).
  Cycle live with **Shift-Tab**.
- **Accounts vs models, deliberately split.** `/connection` switches the
  account (provider + auth + endpoint). `/model` lists models for the *active*
  connection only — changing a model never quietly changes logins. Brands:
  **Codex - API** / **Codex - OAuth · {profile}**, **Grok - API** /
  **Grok - OAuth · {profile}**. After `auth login`, a connection is created if
  missing so the new account shows up immediately.
- **OAuth model catalogs from OpenCode.** Subscription OAuth (`Codex - OAuth`,
  `Grok - OAuth`) discovers models via the managed runtime; API-key connections
  use the endpoint `GET /v1/models`.
- **A safety gate you control.** Quote/subshell/path-aware screening of
  destructive patterns, plus real isolation on Linux with bubblewrap for
  workspace-scoped agent work. Best-effort gate ≠ security boundary — see
  [safety](docs/safety.md).
- **Reversible.** Built-in file edits are journaled (`aishe undo`). On Linux
  with bubblewrap, preview commands/sessions against a throwaway copy
  (`aishe dry-run`, `yolo_dry_run`).
- **Semantic history.** Recall past commands by meaning (`aishe history search`
  or **Ctrl-X Ctrl-R**). Works fully offline with Ollama embeddings.
- **Fix the last command.** **Ctrl-X Ctrl-F** asks the model for a correction
  after a failure.
- **Edit without leaving the prompt.** **Ctrl-X Ctrl-A** improves the current
  buffer and **Ctrl-X Space** opens a generated command palette; both fill the
  line for review and never press Enter for you.
- **Isolated background agents.** `aishe task start '…'` runs long work in a
  detached git worktree with finite time/tool/network/change budgets, durable
  state, cancellation/resume, numbered hunk review, and three-way apply.
- **Explicit context and automation.** Agent-only `@file`, `@dir`, `@diff`, and
  `@clipboard` attachments are bounded; `aishe index` searches tracked code
  locally; `aishe ask --json|--schema` produces validated machine output.
- **Cost-aware.** Per-call and session metering, optional hard budget, status
  line below the editable command. OAuth status prefers a `plan` marker over fake
  dollar spend when prices are subscription-based.
- **Durable AI tasks.** Checkpointed agent sessions; `aishe sessions` /
  `aishe resume` recover interrupted work without blind re-execution.
- **Private by default.** API keys in a mode-`0600` credentials file; OAuth in
  profile-isolated OpenCode HOME/XDG roots. Neither leaks into config, status,
  or audit identity fields.
- **Multiple front-ends.** Interactive zsh-PTY, optional zsh hook, qualified
  [Bash 5.x Tier B / Bash 3.2 Tier B- hook](docs/bash-compatibility.md),
  `aishe -c`, and pipes.

## Built for systems work

AIShe is built for sysadmins, SREs, infrastructure engineers, and operators who
already live in a shell. Inspect failing services, correlate logs, find disk
pressure, verify ports and DNS, operate containers, edit configuration, or carry
a deployment through validation — the agent sees command results and can iterate.

That power stays visible: compact status for the active command, full transcript
with Ctrl-O / `/details`, live spend and policy with `/status`, optional redacted
JSONL audit. Start in `suggest`, use `auto` for approval-gated work, grant `yolo`
host scope only when the task truly needs unrestricted system access.

---

## Install

**One line on Linux or macOS** (binary, exact managed agent runtime, and guided
setup; ensures `zsh` is present when needed):

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh -s -- --setup
```

<details>
<summary>Other ways to install (packages, cargo, from source)</summary>

```sh
cargo binstall aishe                       # prebuilt binary via cargo-binstall
sudo apt install ./aishe_<ver>_amd64.deb   # Debian/Ubuntu (.deb from the release)
sudo dnf install ./aishe-<ver>-1.x86_64.rpm # Fedora/RHEL (.rpm from the release)
cargo install --path .                     # from a checkout (needs Rust 1.88+)
```

Every tagged release attaches per-platform tarballs (`aishe-<target>.tar.gz` +
`.sha256`) for Linux x86_64/arm64 (gnu and static musl) and macOS arm64/x86_64,
plus `.deb`/`.rpm` packages. Full guide: [docs/installation.md](docs/installation.md).
</details>

**Requirements:** `zsh` on `PATH` for the interactive shell (installer can add
it); `bash` is enough for `aishe -c` and pipes. On Linux, functional
`bubblewrap` is the supported OS-isolation boundary for autonomous workspace
actions. Prebuilt binaries target macOS and Linux — no Rust toolchain or
separate OpenCode install required.

## Quickstart

```sh
aishe setup                                 # guided setup includes authentication
aishe                                       # launch real zsh with aishe active
```

Then type real commands or plain English. Validate with `aishe doctor --probe`.
Setup cannot modify the already-running parent shell — run `aishe` afterward (or
install the optional hook). Walkthrough:
[docs/getting-started.md](docs/getting-started.md).

**Where aishe keeps its files** (not always `~/.config/aishe` on macOS):

| | Linux | macOS |
|---|---|---|
| Config, commands, skills | `~/.config/aishe/` | `~/Library/Application Support/aishe/` |
| History, logs, undo journal | `~/.local/share/aishe/` | `~/Library/Application Support/aishe/` |

`aishe doctor` prints the paths in use. Overrides: `AISHE_CONFIG_DIR`,
`AISHE_DATA_DIR`. Full table: [configuration](docs/configuration.md#file-locations).

---

## Modes

| Mode      | Glyph | Behavior |
|-----------|:-----:|----------|
| `suggest` |  `❯`  | Default. Model answers or proposes a command; no agent tools. You review before anything runs. |
| `auto`    |  `»`  | Approval-gated agent. Safe actions run; risky or unresolved actions stop for confirmation. |
| `yolo`    |  `*`  | Autonomous loop after one scope grant per shell. Tools, results, iteration until done. |

Input prefixes: **`?…` force NL** (prefer this over Option/Alt+Return on Mac) ·
`!…` force shell (bypass safety gate) · bare `?` after a failed command asks for
diagnosis. Details: [docs/getting-started.md#6-force-a-route-when-needed](docs/getting-started.md#6-force-a-route-when-needed),
[docs/modes.md](docs/modes.md), [docs/safety.md](docs/safety.md).

## Accounts, providers, and models

AIShe is authoritative for **named connections** (account + endpoint + auth +
default model), then generates an isolated provider config for the managed
engine. Supported shapes: Anthropic Messages, OpenAI/xAI **Responses**, and
OpenAI-compatible Chat Completions (Groq, Ollama, OpenRouter, Together, …).

```sh
# Switch account (Enter is shell-local; the following prompt can save a default)
/connection
aishe connection use openai-work --default

# Change model on the *active* connection only
/model
aishe model gpt-5.6-luna

# Sign in
aishe auth login openai --profile work     # → Codex - OAuth · work
aishe auth login xai --profile work        # → Grok - OAuth · work
aishe auth set anthropic                   # API key
```

Setup includes top shortcuts for **ChatGPT / Codex OAuth** and **Grok OAuth**.
Full recipes: [docs/providers.md](docs/providers.md) ·
[docs/commands.md](docs/commands.md#primary-slash-commands).

## Commands

### CLI (selected)

```
aishe                  launch interactive zsh-PTY shell
aishe -c '<line>'      one-shot non-interactive line
aishe setup            guided configuration (--verify checks only)
aishe settings         interactive settings hub
aishe auth ...         API keys + OpenAI/xAI OAuth login/status/logout
aishe connection ...   list/add/edit/remove/use/show/pick named accounts
aishe tour|demo        safe first-session walkthrough
aishe init zsh|bash    shell-hook snippet for ~/.zshrc / ~/.bashrc
aishe doctor           diagnostics (--probe / --live / --json / --fix / --bundle)
aishe backend ...      managed OpenCode install/verify/repair/rollback/logs
aishe model [NAME]     shell-local model on active (or --connection) account
aishe models           list models for a connection
aishe mode|scope|network|output|reasoning|status|config|mcp|role|…
aishe agent            guided/scriptable foreground or isolated background agent
aishe inbox            review, resume, rework, or inspect background work
aishe capabilities     cached evidence for text/JSON/tools/streaming
aishe test [--live]    offline health check; --live makes minimal paid probes
aishe task|plan|context|last|index|palette|ask|sessions|resume|reset|undo|…
```

`aishe --help` and `man aishe` list the full surface. Complete reference:
**[docs/commands.md](docs/commands.md)**.
Daily-driver examples and safety boundaries:
**[docs/daily-driver.md](docs/daily-driver.md)**.

### In-shell slash commands

```
/help [topic]   task-first help  (accounts · models · session · config)
/connection     switch account   (↑/↓ · type to filter · Enter this shell)
/model          models for *active* connection only
/provider       alias for /connection
/auth           auth state for the active connection
/status         connection, model, mode, scope, spend/plan, audit
/usage          live token/cost for this shell
/log            recent audit events
/reasoning      shell-local reasoning effort
/details        expand agent transcript (Ctrl-O)
/               fuzzy command/session/task/model/MCP palette
/agent          guided foreground/background agent launcher
/inbox          agent work needing attention
/sessions       browse/resume/fork durable conversations
/context        inspect model-visible local context and token estimates
/capabilities   show active-model capability evidence
/test [--live]  offline checks or explicit paid end-to-end validation
/settings       interactive settings editor
/reset          fresh conversation (prior session retained)
/commands       same as /help
```

**Shift-Tab** cycles modes · **Ctrl-O** toggles details · **`?`** forces NL ·
ask naturally (“how do I add a Codex OAuth account?”) — product answers use the
built-in `aishe-product` skill.

## Front-ends

1. **zsh-PTY** — `aishe` wraps your real interactive zsh (full `~/.zshrc`,
   plugins, job control). Natural language routes via `command_not_found_handler`.
2. **Native hook** — `eval "$(aishe init zsh)"` (or `bash`) keeps *your* session.
3. **Non-interactive** — `aishe -c '…'` and pipes.

Details: [docs/front-ends.md](docs/front-ends.md) ·
[docs/shell-integration.md](docs/shell-integration.md) ·
[Bash compatibility](docs/bash-compatibility.md) ·
[terminal/transport compatibility](docs/terminal-compatibility.md).

## Safety, cost, logging

- **Safety gate** and sandbox: [docs/safety.md](docs/safety.md)
- **Token usage and budgets:** [docs/usage-and-cost.md](docs/usage-and-cost.md)
- **Audit log and redaction:** [docs/logging.md](docs/logging.md)
- **Reversible edits / dry-run:** `aishe undo`, `aishe dry-run` (Linux + bwrap)

## Configuration

Config lives in `config.toml` under the platform config directory
(`aishe doctor` prints the path). Annotated example:
[examples/config.toml](examples/config.toml). Every field:
[docs/configuration.md](docs/configuration.md).

```toml
[aishe]
mode = "suggest"
connection = "openai-work"
reasoning_effort = "auto"
budget_usd = 0.0

[connections.openai-work]
provider = "openai"
label = "Codex - OAuth · work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-work.auth]
type = "oauth"
profile = "work"

[backend]
engine = "opencode"
default_scope = "workspace"
workspace_network = "deny"

[sandbox]
linux_backend = "bwrap"
```

Startup aliases for delegated commands: `~/.aishrc` (portable) — see
[examples/aishrc](examples/aishrc).

---

## Documentation

| Topic | Doc |
|-------|-----|
| Install | [docs/installation.md](docs/installation.md) |
| First session | [docs/getting-started.md](docs/getting-started.md) |
| CLI + slash commands | [docs/commands.md](docs/commands.md) |
| Providers & OAuth | [docs/providers.md](docs/providers.md) |
| Modes | [docs/modes.md](docs/modes.md) |
| Front-ends & hooks | [docs/front-ends.md](docs/front-ends.md) · [shell-integration](docs/shell-integration.md) |
| Managed OpenCode backend | [docs/managed-agent-backend.md](docs/managed-agent-backend.md) |
| Configuration | [docs/configuration.md](docs/configuration.md) |
| Custom commands & skills | [docs/custom-commands-and-skills.md](docs/custom-commands-and-skills.md) |
| MCP | [docs/mcp.md](docs/mcp.md) |
| Safety | [docs/safety.md](docs/safety.md) |
| Usage & cost | [docs/usage-and-cost.md](docs/usage-and-cost.md) |
| Logging | [docs/logging.md](docs/logging.md) |
| Troubleshooting | [docs/troubleshooting.md](docs/troubleshooting.md) |
| Architecture | [docs/architecture.md](docs/architecture.md) |
| Product plan | [v0.7.0 release record](docs/releases/v0.7.0.md) · [implementation evidence and next queue](docs/design/NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) · [design lifecycle index](docs/design/README.md) |
| **Index** | **[docs/README.md](docs/README.md)** |

## Development

```sh
cargo build --locked
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
cargo build --release --locked && python3 tests/admin_validation.py
```

See [docs/development.md](docs/development.md) and
[docs/architecture.md](docs/architecture.md).

## License

See [LICENSE](LICENSE).
