<p align="center">
  <img src="assets/aishe-logo.svg" alt="Aishe" width="128">
</p>

# aishe — an AI-driven shell

**It is your real shell, with an agent built into the command line.** Aishe runs
an actual interactive zsh rather than emulating one, so aliases, plugins,
completion, job control, history, and ordinary commands keep their native
behavior. Input that is not a command becomes a plain-English request to the AI.

The agent layer uses a private, compatibility-pinned OpenCode SDK/runtime to
provide reasoning, tool orchestration, durable conversations, compaction, and
subagents. Aishe remains the control plane: it owns command routing, execution
scope, sandbox policy, approvals, credentials, budgets, terminal rendering, and
the audit trail. The result is an AI-driven systems shell, not a chatbot placed
beside a terminal.

> Aishe is pre-1.0. Autonomous host access can make irreversible changes; use
> workspace scope and functional Linux isolation for untrusted work, review
> [the safety model](docs/safety.md), and keep backups.

```
~/projects/app ❯ git status            # runs exactly like zsh
~/projects/app ❯ whats eating my disk  # LLM suggests: du -sh * | sort -rh | head
```

## Get productive in 60 seconds

```sh
# 1. Install (Linux/macOS; downloads the right prebuilt binary + verifies it)
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh

# 2. Authenticate with an API key or a named provider subscription profile
aishe auth set anthropic                   # hidden API-key prompt
aishe auth login openai --profile work     # isolated ChatGPT Plus/Pro OAuth

# 3. Configure, validate, and take the optional guided tour
aishe setup
aishe doctor --probe
aishe tour

# 4. Use it right away, no shell hook needed
aishe                                      # launches your real zsh with aishe active
aishe -c "turn the logs directory into a tarball"   # prints a command to run
aishe suggest --json "list files by size" | jq -r .command   # scriptable output

# 5. (Optional) make every new terminal AI-aware without launching `aishe`
echo 'eval "$(aishe init zsh)"' >> ~/.zshrc   # or: aishe init bash  >> ~/.bashrc
```

`aishe --help` lists every subcommand; `man aishe` has the full reference.

A request whose first word happens to be a real binary (`compress …`, `find …`,
`make …`) runs as that command instead of going to the model — that is the point
of a shell. Prefix it with `?` to force the natural-language route
(`aishe -c "?compress the logs into a tarball"`). See
[docs/getting-started.md](docs/getting-started.md#6-force-a-route-when-needed).

## Features

- **Your real zsh, untouched.** Aishe wraps your actual interactive zsh, so
  your plugins, completions, prompt, aliases, key bindings, and job control all
  work unmodified — it's not a shell reimplementation.
- **A real agent engine, still one shell.** Every AI turn uses Aishe's
  private, exact-version-pinned OpenCode backend for durable conversations,
  reasoning, compaction, and subagents. It starts lazily on authenticated
  loopback, never opens another TUI, and never touches direct zsh commands.
- **Plain English to commands.** Anything that isn't a real command is routed to
  the model. A leading `?` forces a question; a leading `!` forces a raw command.
- **Three modes.** `suggest` (propose, you confirm), `auto` (run safe commands,
  confirm risky ones), and `yolo` (an agentic loop that runs, reads output, and
  iterates). Cycle them live with Shift-Tab.
- **A safety gate you control.** A deterministic, quote/subshell/substitution-aware,
  path-aware screen flags destructive commands (`rm -rf /`, recursive `chmod`/`chown`
  on system paths, danger hidden in `$(…)` or `<(…)`, …) and asks before running them —
  and when it can't tell what a line would actually run, it asks instead of assuming.
  It's a best-effort screen for mistakes, not a security boundary; the sandbox below
  is the real isolation.
- **Reversible.** Built-in file edits are journaled (`aishe undo`), and — on
  **Linux with bubblewrap** — you can preview a command or a whole agentic session
  against a throwaway copy (`aishe dry-run "<cmd>"`, `yolo_dry_run`) to see the exact
  diff, then apply or discard. On macOS the sandbox/overlay isn't available, so yolo
  falls back to the best-effort policy gate (`aishe doctor` shows what's active).
- **Semantic history.** Recall past commands by meaning, not substring:
  `aishe history search "the docker run with the prometheus volume"` (or **Ctrl-X
  Ctrl-R** in the shell). Embeddings go to an OpenAI-compatible `/v1/embeddings`
  endpoint, and Ollama serves embedding models on that same route — so
  `ollama pull nomic-embed-text` plus `embedding_model = "nomic-embed-text"`
  keeps the whole feature on your machine. The index is a local file either way.
  See [docs/providers.md](docs/providers.md#embeddings-fully-offline-semantic-history).
- **Persistent shell history.** Aishe preserves your existing zsh/Oh My Zsh
  history setup. On minimal accounts without one, its own timestamped history
  log backs native Up-arrow, `Ctrl-R`, and history expansion across concurrent
  sessions, restarts, and binary upgrades.
- **Fix the last command.** When a command fails, **Ctrl-X Ctrl-F** asks the
  model for a correction (optionally re-running the failed read-only command to
  read its real error) and pre-fills it for review.
- **Providers, validated.** Anthropic and OpenAI-compatible endpoints —
  OpenAI, xAI/Grok, Groq, Ollama (local), OpenRouter, Together — with live model discovery
  and capability checks. A managed turn is never duplicated after admission or
  a side effect; interruption is durable and resumable.
- **Named accounts, one switcher.** Keep multiple API keys or OAuth accounts for
  the same provider as distinct named connections. `/model` filters across
  connection, authentication label, and known models; Enter changes only this
  shell, while `d` explicitly saves the highlighted choice as the default.
- **Agentic tools.** In yolo, the model edits files precisely, fetches web pages,
  and calls your [MCP servers](docs/mcp.md) and [skills](docs/custom-commands-and-skills.md).
- **Cost-aware.** Per-call and whole-session token/cost metering with an optional
  hard budget cap, plus a configurable right-prompt or below-prompt status line.
  Setup asks for exact prices when the selected model is unknown.
- **Durable AI tasks.** Agentic sessions checkpoint before and after tool calls,
  so `aishe sessions` and `aishe resume` can recover interrupted work without
  blindly repeating a command that may already have run.
- **Managed and recoverable.** Setup installs the exact checksum-pinned
  OpenCode runtime, offers consent-gated bubblewrap installation on Linux, and
  verifies the whole path end to end. Runtime repair/rollback and a
  state-preserving category-based uninstaller are built in.
- **Guided setup and diagnostics.** `aishe setup`, `aishe settings`, and
  `aishe tour` are resumable interactive flows; `aishe doctor --json`,
  `--fix`, `--bundle`, and `--live` make problems actionable.
- **Private by default.** API keys live in a separate mode-`0600` shared
  credentials file (or a higher-precedence environment override). OpenAI and
  xAI OAuth tokens use complete, profile-isolated OpenCode HOME/XDG roots.
  Neither appears in ordinary config, tool environments, status, or audit output.
- **Works anywhere.** `aishe -c '<line>'`, piped stdin, and a bash hook
  (`aishe init bash`) all work without launching the interactive shell.

## Built for systems work

Aishe is especially useful to sysadmins, SREs, infrastructure engineers, and
operators who already live in a shell. Ask it to inspect a failing service,
correlate logs, find disk pressure, verify ports and DNS, operate containers,
edit configuration, or carry a deployment through its validation checks. The
agent sees command results and can iterate instead of stopping after one guess.

That power stays visible and governable. Aishe shows the command associated
with each action in its compact status, expands the full transcript with Ctrl-O
or `/details`, exposes live spend and policy with `/status`, and can record
prompts, responses, tool calls, approvals, file changes, outputs, usage, and
errors in a redacted JSONL audit trail. Start in `suggest`, use `auto` for
approval-gated work, and grant `yolo` host scope only when the task truly needs
unrestricted system access.

## Install

**One line on Linux or macOS** (downloads and verifies the right Aishe binary
and exact managed agent runtime, then ensures `zsh` is present):

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh
```

<details>
<summary>Other ways to install (packages, cargo, from source)</summary>

```sh
cargo binstall aishe                       # prebuilt binary via cargo-binstall
sudo apt install ./aishe_<ver>_amd64.deb   # Debian/Ubuntu (.deb from the release)
sudo dnf install ./aishe-<ver>.x86_64.rpm  # Fedora/RHEL  (.rpm from the release)
brew install --formula ./packaging/aishe.rb
cargo install --path .                     # from a checkout (needs Rust 1.88+)
```

Every tagged release attaches per-platform tarballs (`aishe-<target>.tar.gz` +
`.sha256`) for Linux x86_64/arm64 (gnu and static musl) and macOS arm64/x86_64,
plus `.deb`/`.rpm` packages and the Homebrew formula in
[`packaging/`](packaging/aishe.rb). Full guide, completions, and uninstalling in
[docs/installation.md](docs/installation.md).
</details>

**Requirements:** `zsh` on your `PATH` for the interactive shell (the installer
can add it); `bash` is enough for `aishe -c …` and piped input. On Linux,
functional `bubblewrap` is the supported OS-isolation boundary for autonomous
workspace actions; Setup detects it and asks before installing a system
package. Prebuilt binaries target macOS (arm64 / x86_64) and Linux (x86_64 /
arm64) — no Rust toolchain or separate OpenCode installation is needed.

## Quickstart

```sh
aishe auth set anthropic                    # API key; hidden and saved privately
# or: aishe auth login openai --profile work
# or: aishe auth login xai --profile prod
aishe setup                           # guided, resumable configuration
aishe                                 # launch your real zsh with aishe active
```

Then type real commands as usual, or type a request in plain English. Not sure
your setup is right? Run `aishe doctor --probe`. Setup configures aishe; it
cannot modify the already-running parent shell, so run `aishe` afterward (or
install the optional shell hook). A full walkthrough is in
[docs/getting-started.md](docs/getting-started.md).

**Where aishe keeps its files.** It follows each platform's convention, so the
config directory is **not** `~/.config/aishe` on macOS:

| | Linux | macOS |
|---|---|---|
| Config, commands, skills | `~/.config/aishe/` | `~/Library/Application Support/aishe/` |
| History, logs, undo journal | `~/.local/share/aishe/` | `~/Library/Application Support/aishe/` |

A file put in the wrong directory is silently ignored, so **`aishe doctor` prints
the paths actually in use** — check it rather than guessing. `AISHE_CONFIG_DIR`
and `AISHE_DATA_DIR` override the base directory on every platform. Full table:
[docs/configuration.md](docs/configuration.md#file-locations). The rest of this
README writes these paths in their Linux form.

## Contents

- [Built for systems work](#built-for-systems-work)
- [Modes](#modes)
- [Front-ends](#front-ends)
- [Providers](#providers)
- [Reasoning effort](#reasoning-effort)
- [Commands and settings](#commands-and-settings)
- [Custom commands and skills](#custom-commands-and-skills)
- [Token usage and cost](#token-usage-and-cost)
- [Safety gate](#safety-gate)
- [Startup file (.aishrc)](#startup-file-aishrc)
- [Configuration](#configuration)
- [Documentation](#documentation)
- [Development](#development)

## Modes

| Mode      | Glyph | Behavior                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | Default and least privileged. The model answers or proposes a command; it cannot invoke agent tools. You review before anything runs. |
| `auto`    |  `»`  | Approval-gated agent. Safe actions can run; risky, unresolved, or broader actions stop for confirmation. |
| `yolo`    |  `*`  | Autonomous agent loop. After one scope grant for the shell, it runs tools, reads results, and iterates until done or interrupted. |

The gate has **three** outcomes, not two. *Safe* (nothing matched) runs. *Could
not verify* — the gate couldn't work out what a segment would actually run —
fails closed with a yellow panel and a plain `[y/N]`. *Dangerous* prints a red
panel and requires typing the full word `yes`. Details in
[docs/safety.md](docs/safety.md#three-outcomes).

In yolo, beyond running commands the model can call built-in tools to edit files
precisely (`read_file`/`write_file`/`edit_file`/`list_dir`, `file_tools`) and read
the web (`fetch_url`, `web_tool`) instead of fighting `sed`/heredocs/`curl`; both
are on by default. Configure [MCP servers](docs/mcp.md) under `[mcp_servers]` and
their tools join the loop too (`aishe mcp` lists them).

Switch at any time with `aishe mode auto`, or start with `aishe --mode yolo`. See
[docs/modes.md](docs/modes.md) for streaming, structured output, and input
prefixes.

### Input prefixes

- `?<text>` forces natural-language, for example `?how do I find large files`
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`

After a command fails, type `?` on the next line to ask the LLM to diagnose the
error.

## Front-ends

There are three ways to use aishe. Details in
[docs/front-ends.md](docs/front-ends.md).

1. **zsh-PTY (the interactive shell).** aishe runs your real interactive zsh
   inside a pseudo-terminal, so your full `~/.zshrc` and every plugin you already
   use (zsh-autosuggestions, zsh-syntax-highlighting, fzf-tab, powerlevel10k,
   oh-my-zsh, completions) work unmodified, including job control. A
   `command_not_found_handler` routes natural language to the LLM. **This requires
   zsh**; without it, aishe tells you to install it. On a minimal account with no
   syntax-highlighting plugin, aishe highlights complete command-shaped input
   green and natural-language questions magenta. It evaluates the whole buffer:
   `what --version` remains a command, while `what is my IP address?` changes to
   the LLM color even though `what` is an installed command. A real zsh
   highlighting plugin automatically takes precedence.

   ```sh
   aishe        # launch your zsh under aishe
   aishe zsh    # the same, explicitly
   ```

2. **Native zsh/bash hook.** Add `eval "$(aishe init zsh)"` to your `~/.zshrc`
   (or `~/.bashrc` with `init bash`) to keep your own shell session while routing
   unknown input to aishe. The bash hook is how to use aishe interactively without
   zsh. See [docs/shell-integration.md](docs/shell-integration.md).

3. **Non-interactive.** `aishe -c '<line>'` and piped stdin run lines through the
   in-process executor (zsh, falling back to bash) with no interactive terminal.

## Providers

Aishe remains the source of truth for named connection, provider, model,
credential/OAuth profile, prices, and budgets, then generates an isolated provider configuration for its
managed agent engine. It supports the **Anthropic Messages API**, OpenAI and
xAI **Responses APIs**, and **OpenAI-compatible Chat Completions APIs**. The
official OpenAI and xAI URLs use Responses; Groq, Ollama, OpenRouter, Together,
and other custom `base_url` values use Chat Completions.

```toml
[aishe]
connection = "openai-work"

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-work.auth]
type = "oauth"
profile = "work"

[connections.openai-api]
provider = "openai"
label = "OpenAI API key"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-api.auth]
type = "api_key"
credential = "openai-team"
api_key_env = "OPENAI_API_KEY"
```

API keys are read from the named private credential profile; the environment
variable remains a higher-precedence override. They are never stored in
`config.toml`, backend config, session files, tool journals, or model-controlled
tool environments. OpenAI and xAI can instead use provider subscription OAuth:

```sh
aishe auth login openai --profile work
aishe auth login openai --profile personal --headless
aishe auth status openai --profile work
aishe auth logout openai --profile personal
```

OAuth is endpoint-bound (`api.openai.com` or `api.x.ai`) and available only to
the managed OpenCode transport. Authentication is explicit per connection:
`api_key` never consumes OAuth, `oauth` never consumes an ambient API key,
`none` performs no lookup, and migrated `auto` connections retain the old
key-first compatibility order. See [docs/providers.md](docs/providers.md) for per-provider
setup and [managed agent backend](docs/managed-agent-backend.md) for the process
boundary.

## Reasoning effort

Reasoning depth is independent of transcript detail. Ctrl-O and `/details`
change what Aishe displays; `aishe reasoning` changes how much reasoning the
provider is asked to perform. `auto` is the default and leaves the choice to the
model. An explicit setting is passed through the managed OpenCode model options:

```sh
aishe reasoning                 # show the current value
aishe reasoning low             # favor latency and lower reasoning use
aishe reasoning medium          # balanced
aishe reasoning max             # deepest supported effort
aishe reasoning auto            # return control to the model/provider
aishe reasoning high --default  # save it for the selected connection
```

Accepted values are `auto`, `none`, `low`, `medium`, `high`, `xhigh`, and
`max`. Support is model-dependent. OpenAI's Luna, Terra, and Sol GPT-5.6 models
support this range; other providers may ignore or reject levels they do not
implement. Inside an Aishe shell, `/reasoning LEVEL` changes only that shell;
`--default` persists it for the connection. `/status` includes the active value with
the model, mode, scope, output density, spend, and audit state.

## Commands and settings

aishe's subcommands:

```
aishe                  launch the interactive zsh-PTY shell
aishe zsh              the same, explicitly
aishe -c '<line>'      run one line non-interactively and exit
aishe setup            guided/resumable configuration; --verify checks only
aishe settings         interactive settings hub with source/provenance
aishe auth ...         API-key and OpenAI/xAI OAuth login/status/logout
aishe connection ...   list/add/edit/remove/use/show named provider accounts
aishe tour             resumable first-session walkthrough
aishe init zsh|bash    print the shell-hook snippet (for ~/.zshrc / ~/.bashrc)
aishe doctor           diagnostics; --probe/--live/--json/--fix/--bundle
aishe backend ...      managed runtime status/install/verify/repair/rollback/logs
aishe uninstall        previewable removal; user state preserved by default
aishe completions ...  print a shell completion script for aishe itself
aishe trust [PATH]     trust this repo's .aishe/config.toml, or one project file
aishe trust --list     list every trusted file
aishe untrust [PATH]   drop trust for this repo (or one file); --all for every one

aishe dry-run '<cmd>' [--apply]     preview a command's file changes, then keep/discard
aishe undo [--list]                 revert the last AI/dry-run file change set
aishe history search '<q>'|index    semantic recall over your shell history
aishe log | usage                   read the audit log / summarize token cost
aishe runbook | context             export a session as a runbook / preview context
aishe sessions | session ...        list/inspect/rename/delete durable AI tasks
aishe resume [ID]                    safely resume an interrupted task
aishe price list|set|remove          manage exact per-model price overrides
aishe profile [VALUE] | readiness    inspect safety profile/autonomy readiness
aishe models [--connection ID]       list models for exactly one connection
aishe scope [workspace|host]         set the next agent execution scope
aishe network [allow|deny]           set workspace-agent network capability
aishe output [focus|compact|detailed] set persistent agent transcript density
aishe reasoning [LEVEL] [--default] set shell-local or saved reasoning effort
aishe status [--json]                show active session settings and spend

aishe model [MODEL] [--connection ID] [--default]  shell-local or saved selection
aishe mode|provider [VALUE]         show or set a persistent setting
aishe config|mcp|commands|skills    print the active config / registries
```

`aishe mode` and legacy `provider` show or save durable values. `aishe reasoning`
uses the same shell-local/default distinction as model selection. `aishe model` opens the unified
connection/model picker in a terminal; Enter is shell-local, `d` saves the
default, and `aishe model default` restores that default. You can also override
per session with the `--mode`/`--model`/`--provider` flags or `$AISHE_MODE`, and
in the interactive shell **Shift-Tab** cycles the mode. Full reference in
[docs/commands.md](docs/commands.md).

Aishe advertises `/help` when its standalone shell starts. The primary set is
`/model`, `/provider`, `/auth`, `/status`, `/usage`, `/log`, `/reasoning`, `/details`, `/settings`, `/reset`, and
`/commands`; the last one also lists installed custom commands. See
[docs/commands.md](docs/commands.md#primary-slash-commands).

Agent output defaults to `focus`: one transient, width-bounded row shows the
current command; a bounded digest preserves up to three executed commands, then
one activity summary and the final answer remain in scrollback. `compact` keeps
one line per completed action. Press Ctrl-O or type `/details` for full tool
activity on following turns. Persist a preference with `aishe output
focus|compact|detailed`.

Environment variables worth knowing: `$AISHE_MODE` sets the mode for the shell
hook, and **`AISHE_CONFIG_DIR` / `AISHE_DATA_DIR`** relocate aishe's config and
state directories on any platform (each takes a base directory; aishe appends
`aishe/`). They are the quickest way to try a throwaway setup, or to sidestep the
Linux/macOS path difference entirely:

```sh
AISHE_CONFIG_DIR=/tmp/try AISHE_DATA_DIR=/tmp/try aishe doctor
```

## Conversation memory

aishe remembers natural-language turns so follow-ups have context: after
"create alpha.txt containing apple", a follow-up "now do the same for beta.txt"
knows what "the same" means. Managed conversations are durable per
shell/workspace and survive backend restarts and Aishe upgrades; private session
records are never included in support bundles. `aishe sessions` lists them,
`aishe resume ses_...` reconnects one, and `reset`/`aishe reset` starts fresh
without deleting the previous session.
The native compatibility memory remains capped and can be disabled with
`memory = false`. See [docs/modes.md](docs/modes.md).

## Custom commands and skills

Drop Markdown files into `~/.config/aishe/commands/` (user — on macOS that is
`~/Library/Application Support/aishe/commands/`; `aishe doctor` prints the real
path) or `<project>/.aishe/commands/` (project) to add your own `/commands`. The file name
is the command (`bigfiles.md` becomes `/bigfiles`). If both directories define the
same name, your user command wins — a project you cloned cannot shadow it. Project
commands that run shell are also gated the same way project config is by
`aishe trust`: aishe shows the resolved command and asks before running it.

```md
---
description: Suggest a command to find the biggest files
mode: suggest            # suggest | auto | yolo (NL commands); default = current mode
# shell: true            # run the body as a shell command instead of an NL request
---
Show the 10 largest files under $ARGUMENTS, human-readable, largest first.
```

**Skills** are the model-invoked counterpart. Put skill files in
`~/.config/aishe/skills/` (same macOS caveat) as `<name>/SKILL.md`. In yolo mode
aishe advertises each skill's `name` and `description`; when your request matches,
the model pulls the skill's full instructions into context on demand. A skill or
`shell: true` command that comes from a *project* is trust-gated: enable it with
`aishe trust <path-to-file>`.

The formats match Claude Code, so real Agent Skills from
[anthropics/skills](https://github.com/anthropics/skills) and slash commands from
collections like [wshobson/commands](https://github.com/wshobson/commands) drop
in unchanged. Ready-made examples live in [examples/commands/](examples/commands/)
and [examples/skills/](examples/skills/). Full guide:
[docs/custom-commands-and-skills.md](docs/custom-commands-and-skills.md).

## Token usage and cost

aishe meters every model call. The status line can live in the right prompt,
under the prompt, or be disabled, and its items are configurable. It can show
the compact safe connection/provider/auth/model identity, shell-local/default
state, last-call tokens and cost, connection-scoped session tokens/cost, request
count, and task state. `aishe usage --by connection` shows persisted totals and
`--connection ID` filters them when audit logging is enabled.

Costs come from a built-in price table (USD per 1M tokens) or an exact user
override. Setup asks for input/output prices when it cannot price the selected
model; it never guesses. Use `aishe price list`, `aishe price set MODEL
--input ... --output ...`, or `aishe settings` later. Set `budget_usd` to stop
calling the model once the estimated session cost reaches a limit.

```toml
[aishe]
budget_usd = 0.50          # stop past ~$0.50 per session (0 = unlimited)

[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.60
```

More in [docs/usage-and-cost.md](docs/usage-and-cost.md).

## Safety gate

Before running an LLM-proposed command in suggest or auto, Aishe screens for
irreversible operations:
`rm -rf`, `dd of=/dev/...`, `mkfs`, fork bombs, `curl ... | sh`, `git push --force`
to main, `shutdown` and `reboot`, and more. Dangerous commands print a red panel
and require you to type the full word `yes`.

The screen is quote-, subshell-, and substitution-aware: *literal* danger written
inside `$(…)`, backticks, or `<(…)` is caught, so `cat <(rm -rf /etc)` is flagged.
The caveat is that this only reaches text the gate can read — a shell fed a process
substitution whose contents don't exist until run time (`bash <(curl -sL https://x.sh)`,
`source <(…)`) is **not** caught. It is also path-aware for `rm -rf`: an in-tree
relative path like `rm -rf node_modules` runs without fuss, while absolute, home,
variable, glob, or escaping targets are flagged. When the gate cannot resolve what a
command would actually run, it says so and asks instead of assuming safe (`aishe
suggest --json` reports `"risk": "unknown"`; the exit code stays `20`).

It is a pattern matcher, so `safe` means "nothing matched", not "this is safe". It
can only judge text it can resolve, which leaves whole classes it does not see
into: a wrapper or runner binary outside its built-in table, execution that happens
on another machine, a payload handed to a non-shell interpreter, and content piped
into a shell from a source it cannot read. Those classes, and why they are hard,
are in [docs/safety.md](docs/safety.md#what-the-gate-does-not-catch).

The real control for autonomous or untrusted work is isolation, not the gate:
managed yolo asks once per new shell for `workspace` or `host` scope, then does
not interrupt with per-action approvals. On Linux, `[sandbox]
linux_backend = "bwrap"` provides the OS boundary for workspace scope; use
`aishe dry-run` / `yolo_dry_run` to preview changes you can apply or discard
(`aishe undo` reverts any AI edit). macOS is explicitly policy-only. The gate
does not apply to commands you type yourself or to `!`-forced lines. Details in
[docs/safety.md](docs/safety.md).

## Logging and privacy

aishe sends an environment context block (including your recent commands) with
each request. To avoid leaking credentials, it **redacts likely secrets**
(tokens, passwords, URL credentials) from that block before sending. This is on
by default (`redact_secrets`).

An optional **audit log** records a bounded, redacted managed-agent trail as
JSONL: prompts, visible responses and provider-exposed reasoning, tool arguments
and results, exact commands, approvals, file diffs, lifecycle/recovery events,
durable identities, timing, usage, and cost. It is off by default; enable it with
`[logging] enabled = true` or `AISHE_LOG=1`. Use `/log` for the latest events,
`aishe log --json` for the raw records, and `/status` to confirm the resolved
path and redaction state. See [docs/logging.md](docs/logging.md).

Every new event is also attributed to the safe connection ID/label, provider,
auth type/profile label, model, and reasoning level. Raw keys, access tokens,
refresh tokens, local control credentials, and OAuth payloads are never identity
fields.

## Startup file (.aishrc)

aishe sources `~/.aishrc` and an `aishrc` in its config directory (in that order)
into every delegated command, so aliases, functions, and exports you define there
are available everywhere and recognized at the prompt. `~/.aishrc` is the same on
every platform, so it is the portable place to put these.

```sh
# ~/.aishrc
alias gs='git status'
alias ll='ls -lah'
export EDITOR=nvim
gco() { git checkout "$@"; }
```

A ready-to-copy example is at [examples/aishrc](examples/aishrc).

## Configuration

The config file is `config.toml` in aishe's config directory
(`~/.config/aishe/` on Linux, `~/Library/Application Support/aishe/` on macOS;
`aishe doctor` prints the resolved path). A fully annotated example is at
[examples/config.toml](examples/config.toml), and every field is documented in
[docs/configuration.md](docs/configuration.md).

```toml
[aishe]
mode = "suggest"               # suggest | auto | yolo
connection = "openai-work"     # durable default named connection
pty_prompt = true              # branded prompt in the zsh-PTY shell
structured = "schema"          # schema | json | prompt
reasoning_effort = "auto"      # auto | none | low | medium | high | xhigh | max
stream = false                 # stream answers token-by-token
show_usage = true              # print token/cost after each model call
budget_usd = 0.0               # 0 = unlimited
memory = true                  # remember recent turns
redact_secrets = true          # scrub secrets from the model context
auto_pushd = false             # zsh AUTO_PUSHD for in-process cd
# many more fields: see examples/config.toml and docs/configuration.md

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-work.auth]
type = "oauth"
profile = "work"

[backend]
engine = "opencode"
fallback = "native"             # only before prompt admission
default_scope = "workspace"
workspace_network = "deny"
max_instances = 8               # bounded isolated connection runtimes

[sandbox]
linux_backend = "bwrap"
require_functional = false
allow_host_yolo = true
```

## Documentation

The [docs/](docs/) directory has the full user guide:

- [Installation](docs/installation.md)
- [Getting started](docs/getting-started.md)
- [Modes](docs/modes.md)
- [Front-ends](docs/front-ends.md)
- [Providers](docs/providers.md)
- [Managed agent backend](docs/managed-agent-backend.md)
- [Configuration reference](docs/configuration.md)
- [Commands and slash-commands](docs/commands.md)
- [Custom commands and skills](docs/custom-commands-and-skills.md)
- [Token usage and cost](docs/usage-and-cost.md)
- [Safety gate](docs/safety.md)
- [Logging and privacy](docs/logging.md)
- [Shell integration and .aishrc](docs/shell-integration.md)
- [Per-project config and trust](docs/project-config.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Architecture (for contributors)](docs/architecture.md)
- [Roadmap](docs/ROADMAP.md)

## Development

```sh
cargo build --locked
cargo test --all-targets --locked  # integration tests spawn a real shell
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check

# end-to-end validation harness (no API key needed for the deterministic suites)
cargo build --release --locked && python3 tests/admin_validation.py
```

See [docs/development.md](docs/development.md) for the test layout and how the
validation harness works, and [docs/architecture.md](docs/architecture.md) for a
contributor's map of the codebase (routing, front-ends, provider/tool/MCP layers).

## License

See [LICENSE](LICENSE).
