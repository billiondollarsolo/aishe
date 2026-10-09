<p align="center">
  <img src="assets/aishe-banner.png" alt="AIShe — AI Shell" width="640">
</p>

# AIShe — AI Shell

**AIShe** is **AI Shell**: your real shell, with an agent built into the command
line. The CLI package name is `aishe`.

> **Alpha.** The product is usable day to day, but APIs, config shape,
> and UX can still change. Autonomous host access can make irreversible changes —
> prefer workspace scope and Linux isolation for untrusted work, read
> [the safety model](docs/safety.md), and keep backups.

AIShe runs an actual interactive zsh. The default `clean` profile uses an
isolated, lightweight configuration; `AISHE_ZSH_PROFILE=personal aishe` loads
your zsh configuration and plugins with the same native agent. Completion, job
control, history, and ordinary commands stay in zsh. Optional native startup
files are `~/.aishe/leanrc` and `~/.aishe/leanrc.post`. Input that is not a
command becomes a plain-English request to the AI.

The default agent uses pooled native provider connections and keeps the model,
execution scope, approvals, budget, and usage visible in the shell. Managed
OpenCode and its historical shell integration remain available explicitly with
`AISHE_LEGACY_OPENCODE=1`; subscription OAuth setup discloses that requirement.

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
AISHE_ZSH_PROFILE=personal aishe            # your zsh settings, native agent
aishe -c "turn the logs directory into a tarball"
aishe suggest --json "list files by size" | jq -r .command

# 3. (Optional) make every new terminal AI-aware
echo 'eval "$(aishe init zsh)"' >> ~/.zshrc   # or: aishe init bash
```

### Everyday controls (in the shell)

| Do this | How |
|--------|-----|
| Help | **`/` then Tab** · `/help` · `/help model` · `/help keys` |
| Switch account / model | **`/connection`** · **`/model`** (model is *this account only*) |
| Cycle mode | **Shift-Tab** on empty input → ask `❯` · allow `»` · agent `*` |
| **Force English to the AI** | Start with **`?`** — e.g. `? install kubectl please` |
| Force raw shell | Start with **`! command`** (bypasses the safety gate; `!!` keeps zsh history expansion) |

**Common trap:** lines whose **first word is a real binary** run as shell — even
if the rest is English. `install` is `/usr/bin/install` on every Mac/Linux box,
so `install kubectl please` is **not** “please install the package”; use
`? install kubectl please`. Optional highlighting follows the route: green for
shell, magenta for agent input, cyan for a recognized `/command`; color is never
required. Press **Ctrl-X ?** in zsh or
run `aishe route -- '<line>'` to read the route and reason as plain text.

Full routing, Option/Alt+Return, and Mac terminal Meta settings:
[Getting started §5](docs/getting-started.md#5-force-a-route-when-needed) ·
[Shell integration](docs/shell-integration.md#force-nl-and-input-prefixes).

---

## Features

- **Your real zsh, with a fast default.** Ordinary commands stay in the child
  shell. Choose the clean profile or opt into your personal zsh configuration
  without changing agent engines. Existing Enter/Tab widgets are chained;
  personal prompts and custom shortcuts are preserved. Early and late native
  startup files support further customization.
- **A native agent in the shell.** Provider connections are reused across turns;
  skills and local commands load when needed, and MCP connects only when asked.
  Foreground, CLI, and background tasks share native admission, effect budgets,
  cancellation, and explicit completion or stop outcomes.
- **The live working environment.** Agent commands use current exported
  variables, `PATH`, virtual environments, and unsets from the interactive shell.
  Credentials and startup controls are filtered; aliases and functions remain
  in zsh rather than being replayed into the agent's command shell.
- **Clean setup and settings.** Setup connects an account, chooses behavior,
  and reviews the result. Settings shows current values and unsaved changes;
  grouped review and transactional saves keep edits predictable.
- **Plain English to commands.** Non-commands go to the model. **`?`** forces
  natural language when the first word is also a real binary (e.g. `install`);
  **`!`** forces raw shell past the safety gate.
- **Three modes.** `ask` proposes or answers, `allow` runs safe suggestions and
  confirms risky ones, and `agent` runs a tool loop within an explicitly granted
  workspace or host scope. **Shift-Tab** cycles on empty input; nonempty input
  keeps reverse completion. Legacy mode names remain accepted aliases.
- **Accounts vs models, deliberately split.** `/connection` switches the
  account (provider + auth + endpoint). `/model` lists models for the *active*
  connection only — changing a model never quietly changes logins. Brands:
  **Codex - API** / **Codex - OAuth · {profile}**, **Grok - API** /
  **Grok - OAuth · {profile}**. After `auth login`, a connection is created if
  missing so the new account shows up immediately.
- **Searchable account and model choices.** Pickers filter and page without
  changing accounts implicitly. Native model choices use local configuration
  and capability cache; explicit provider checks refresh model evidence. A
  separate Yes saves a selection as the default for new shells.
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
- **Commands stay discoverable.** **`/` then Tab** browses grouped commands
  with descriptions and fills the line for review. Arguments and file completion
  keep their native behavior; selecting a command does not execute it.
- **Isolated background agents.** `aishe task start '…'` runs long work in a
  detached git worktree with finite time, provider/tool dispatch, and change
  limits, durable state, cancellation/resume, numbered hunk review, and three-way
  apply. Network-call limits count recognized network tools and MCP calls, not
  every request made inside a subprocess.
- **Quiet background awareness.** A small prompt badge shows running work and
  unseen results or problems. **Ctrl-X b** opens the task browser without losing
  your editable command; `/tasks` opens it too. Inspect results, activity, limits,
  and changes before choosing a follow-up. See
  [background work](docs/front-ends.md#background-work).
- **Explicit context and automation.** Agent-only `@file`, `@dir`, `@diff`, and
  `@clipboard` attachments are bounded; `aishe index` searches tracked code
  locally; `aishe ask --json|--schema` produces validated machine output.
- **Cost-aware.** `/usage` totals this shell across model and connection
  switches. Known prices support a cumulative budget; unknown prices stay
  visible. `aishe usage` inspects the content-free saved ledger without enabling
  the audit log.
- **Durable AI tasks.** Checkpointed agent sessions; `aishe sessions` /
  `aishe resume` recover interrupted work without blind re-execution. Saved
  connection, model, scope, network, workspace root, and effect limits carry
  forward; current organization policy can further restrict them.
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
with Ctrl-O / `/details`, live spend and policy with `/status`, token and cache
accounting with `/usage`, optional redacted JSONL audit. Start in `ask`, use `allow`
for safe suggestions, and grant `agent-host` when the task needs host access.

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
| `ask`     |  `❯`  | Default. Model answers or proposes a command; no agent tools. You review before anything runs. |
| `allow`   |  `»`  | Safe suggestions run after a shell grant; risky or unresolved actions stop for confirmation. |
| `agent`   |  `*`  | Autonomous loop after a scope grant per shell. Tools, results, and explicit completion or stop outcomes. |

Legacy aliases `suggest`, `auto`, and `yolo` remain supported.

Input prefixes: **`?…` force NL** (prefer this over Option/Alt+Return on Mac) ·
`! command` force shell (bypass safety gate) · bare `?` after a failed command asks for
diagnosis. Details: [docs/getting-started.md#5-force-a-route-when-needed](docs/getting-started.md#5-force-a-route-when-needed),
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
aishe tour             safe first-session walkthrough
aishe init zsh|bash    shell-hook snippet for ~/.zshrc / ~/.bashrc
aishe doctor           diagnostics (--probe / --live / --json / --fix / --bundle)
aishe backend ...      managed OpenCode install/verify/repair/rollback/logs
aishe model [NAME]     shell-local model on active (or --connection) account
aishe usage            tokens, cache, cost, and plan usage (--by / --since / --json)
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

```text
/ then Tab      browse commands by purpose with descriptions
/ or /help      quick guide; /help model explains one command
/commands       complete catalogue, including custom Markdown commands
/connection     searchable account picker for this shell
/model          searchable models for the active account
/mode           ask, allow, agent, or agent-host
/details        cycle focus / compact / detailed (also Ctrl-O)
/status         mode, scope, grant, model, session, usage and budget
/tasks          browse background work, results, activity and changes
/usage          cumulative tokens and cost for this shell
/settings       edit saved defaults with grouped change review
/setup          configure or resume setup
/doctor         inspect local setup and dependencies
/tour           guided first-session walkthrough
/context        inspect model-visible context
/reset          clear this conversation; retain usage and budget
/sessions       list, clear, or resume saved conversations
/undo           restore the last journaled file-change batch
/skills         list local skills
/mcp            connect configured servers and discover their tools
/backend        explain optional specialist backends
```

Pickers apply to this shell; an explicit Yes promotes a saved default. Settings
applies defaults to new shells. **Shift-Tab** cycles modes on empty input,
**Ctrl-O** cycles output detail, **Ctrl-X b** opens background work while
retaining the command line, and **`?`** forces natural language. Additional
CLI commands and the legacy shell surface are described in
[docs/commands.md](docs/commands.md).

## Front-ends

1. **Native zsh-PTY** — `aishe` launches a clean zsh; opt into your full
   `.zshenv`/`.zshrc` with `AISHE_ZSH_PROFILE=personal`. Both use the native agent.
2. **Standalone hook** — `eval "$(aishe init zsh)"` (or `bash`) keeps *your*
   existing session and its compatibility integration.
3. **Non-interactive** — `aishe -c '…'` and pipes.

`aishe agent '…'` is an explicit autonomous task request. Selecting agent mode
in configuration alone does not authorize an unattended natural-language turn.
Native tasks return distinct exit codes for completion, cancellation, exhausted
budgets, iteration limits, failure, and declined approval. Completion means the
model supplied a final answer; it is not independent verification of the result.
See [native agent execution](docs/configuration.md#native-agent-execution).

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
mode = "ask"
connection = "openai-work"
reasoning_effort = "auto"
budget_usd = 0.0

[connections.openai-work]
provider = "openai"
label = "Codex - API · work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-work.auth]
type = "api_key"
credential = "openai-work"
api_key_env = "OPENAI_API_KEY"

[backend]
engine = "native"
default_scope = "workspace"
workspace_network = "deny"

[sandbox]
linux_backend = "bwrap"
```

Native interactive startup files: `~/.aishe/leanrc` before widgets and
`~/.aishe/leanrc.post` afterward. Compatibility executor aliases use
`~/.aishrc` — see [examples/aishrc](examples/aishrc).

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
| Product plan | [v0.8.0 release record](docs/releases/v0.8.0.md) · [implementation evidence and next queue](docs/design/NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) · [design lifecycle index](docs/design/README.md) |
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
