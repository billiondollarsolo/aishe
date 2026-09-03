# Commands and slash-commands

> **Alpha (pre-1.0).** The command surface can still change; prefer `/help` in a
> live shell for the current task-oriented index. Overview:
> [docs index](README.md) · [root README](../README.md).

aishe's interactive shell is your real zsh; aishe adds a small set of
subcommands, a few inspection commands, and input prefixes that control routing.

## Subcommands

<!-- BEGIN GENERATED CLI SURFACE -->
```
aishe setup            Configure and verify AIShe interactively, or provision it with flags
aishe settings         Edit the current configuration through an interactive section hub
aishe auth ...         Manage provider API keys and OAuth subscriptions in AIShe's private stores
aishe tour             Run the resumable guided first-session tour
aishe init             Print a shell integration snippet: `eval "$(aishe init zsh)"`
aishe zsh              Launch your real interactive zsh (with all native plugins) under aishe
aishe doctor           Check your environment: shell, config, front-end, provider, API key
aishe backend ...      Manage AIShe's private, compatibility-pinned agent runtime
aishe update ...       Check, apply, or roll back the AIShe binary itself
aishe completions      Print a shell completion script for `aishe` itself (bash/zsh/fish/...)
aishe man              Print a roff man page for `aishe` (e.g. `aishe man > /usr/share/man/man1/aishe.1`)
aishe uninstall        Remove AIShe components by category; user state is preserved by default
aishe trust            Trust the current project's `.aishe/config.toml` so its sensitive keys (provider/endpoint, MCP servers, audit logging, safety toggles, `yolo`) apply. Safe cosmetic keys apply without trust
aishe untrust          Drop trust for the current project's `.aishe/config.toml`, or for a specific project file
aishe mode             Show or set the interaction mode for this shell; `--default` also saves it
aishe scope            Show or set the agent execution scope for future turns
aishe network          Show or set network access for workspace-scoped agent turns
aishe output           Show or set persistent agent transcript density
aishe reasoning        Show or set reasoning effort for this shell; `auto` uses the model default
aishe role ...         Configure workload-specific connection/model/reasoning overrides
aishe model            Select a model for the active connection (this shell, or default for new shells)
aishe connection ...   Manage named provider/authentication connections
aishe profile          Show/apply a safety profile, or export/import portable non-secret config
aishe readiness        Check whether autonomous mode is ready for real work
aishe price ...        Manage per-model token prices used for estimates and budgets
aishe config           Print the active configuration
aishe mcp ...          List or transactionally manage MCP servers and their capabilities
aishe commands         List primary slash-commands, or show task-oriented help
aishe palette          Search or open the generated command palette
aishe agent            Launch a foreground or isolated background agent with one coherent set of controls
aishe inbox            Show agent work that needs attention and act on it interactively
aishe capabilities     Show the active model's cached capability evidence
aishe test             Run local checks and optional paid live model/tool validation
aishe route            Explain whether a line will run in the shell, reach the agent, or invoke a builtin
aishe status           Show model, mode, scope, output, live spend, and audit-log state
aishe hints ...        Inspect or reset local one-time discovery-hint seen-state
aishe skills           List model-invoked skills
aishe undo             Undo the most recent AI file change (from the built-in file tools)
aishe log              Show the audit log of AI calls and actions (needs logging enabled)
aishe usage            Summarize token usage and estimated cost from the audit log
aishe suggest          Turn a natural-language request into a shell command (for scripting). Prints the command to stdout; exit 0 = safe/answer, 20 = flagged (either `dangerous`, or `unknown` when the gate cannot tell what the command runs — the command is still printed for review), 1 = no provider or no query. Use `--json` for structured output
aishe ask              Ask for a non-executing answer suitable for humans or Unix automation
aishe last ...         Inspect, explain, fix, safely retry, or clear the last shell failure
aishe index            Build, inspect, or search a bounded index of tracked repository text
aishe history ...      Semantic search over your shell history (opt-in; needs an embedder)
aishe sessions         List durable AI task sessions
aishe reset            Start a fresh conversation in this AIShe shell without deleting the previous managed session
aishe session ...      Inspect or manage one durable AI task session
aishe task ...         Run and manage durable background agent tasks
aishe resume           Resume the most recent interrupted task, or a specific task ID
aishe dry-run          Preview a command's file changes against a throwaway copy of the working tree (read-only system, no network via bubblewrap), then keep or discard
aishe context          Inspect or configure the environment context sent to the model
aishe plan             Interactively create or inspect a durable background-task plan
aishe runbook          Generate a runnable script + markdown runbook from a recorded session
```
<!-- END GENERATED CLI SURFACE -->

These are real subcommands, so they work the same in the interactive zsh-PTY
shell, a plain shell, or a script.

Managed runtime operations:

```sh
aishe backend status --json
aishe backend install [--from ARCHIVE] [--force]
aishe backend verify --live
aishe backend repair [--from ARCHIVE]
aishe backend rollback
aishe backend stop
aishe backend logs --tail 200
aishe backend gc --dry-run
```

They always operate on the exact OpenCode version embedded in this AIShe build.
`--from` supports offline installation but does not bypass checksum, archive
size, executable-version, or license/notices verification.
`backend status --json` is schema-versioned and separates runtime state from a
sanitized running/stopped/stale supervisor-pool summary; it never exposes local
authentication tokens, passwords, nonces, or listener URLs.

Uninstall is previewable and category-based:

```sh
aishe uninstall --dry-run
aishe uninstall                         # replaceable binary/runtime layers
aishe uninstall --sessions --dry-run
aishe uninstall --config --history --audit-undo
aishe uninstall --all --dry-run
```

Plain uninstall preserves config, credentials, history, sessions, audit, and
undo. Any selected user-state category is marked permanent and requires
targeted confirmation; use `--yes` only after reviewing the same plan with
`--dry-run`.

Credential commands follow the AWS CLI-style shared-file workflow:

```sh
aishe auth set openai              # hidden prompt; no key in shell history
printf '%s\n' "$KEY" | aishe auth set openai --stdin
aishe auth status openai [--json]  # source/provenance, never the value
aishe auth list [--json]
aishe auth remove openai [--yes]
aishe auth path
```

When the profile is omitted, the active provider's user-config profile is used.
Project overlays never choose a credential-writing target.

OpenAI and xAI subscription OAuth uses AIShe's isolated, pinned OpenCode
runtime and complete profile-specific private HOME/XDG roots:

```sh
aishe auth login openai --profile work
aishe auth login openai --profile personal --headless
aishe auth login --connection openai-work
aishe auth status openai --profile work --json
aishe auth status --connection openai-work
aishe auth logout openai --profile personal [--yes]
```

Explicit API-key and OAuth connections never fall through to one another.
Migrated `auto` connections retain the legacy key-first behavior. OAuth is
accepted only for the exact official `api.openai.com` and `api.x.ai` endpoints.

Named connection management is non-secret and exact-targeted:

```sh
aishe connection list [--json]
aishe connection show [ID] [--json]
aishe connection add ID --provider openai --auth oauth --profile work
aishe connection edit ID --model MODEL --auth api-key --credential PROFILE
aishe connection use ID [--model MODEL] [--default]
aishe connection remove ID [--yes]   # credentials are preserved
```

Removing or editing one connection invalidates only its managed runtime. Two
same-provider connections may remain active concurrently up to
`backend.max_instances`.

## Primary slash commands

The standalone AIShe shell prints a one-line `/help` hint at startup. `/help`
is **task-first** (not a raw dump of every flag). Topics:

```text
/help              task-first overview and the keys
/help accounts     add/switch accounts, OAuth vs API key, brands
/help models       models for the active connection only
/help agent        foreground and background agent work
/help session      status, usage, reset, reasoning, mode keys
/help config       setup, settings, doctor, tour, backend
/help routing      explain shell-command vs natural-language routing
/help all          every visible slash command with its effect
```

`aishe commands` and `/commands` share the same help surface.

The following block is rendered from the same command registry as zsh/bash
dispatch and the top-level CLI, and is guarded by an exact-conformance test.
The State/effect column uses one six-value vocabulary:

| Effect | Meaning |
|---|---|
| read-only | prints and exits |
| this shell | changes this shell session only |
| this shell · --default saves | changes this shell; `--default` also writes `config.toml` |
| saves config | writes `config.toml` |
| conversation | changes the active conversation or its sessions |
| runs agent / edits files | runs an agent, executes commands, or edits files |

<!-- BEGIN GENERATED COMMAND SURFACE -->
| Slash command | Purpose | State/effect |
|---|---|---|
| `/help, /commands [TOPIC]` | Show task-oriented AIShe help | read-only |
| `/connection, /provider [ID_OR_LABEL]` | Inspect or switch the active account connection | this shell · --default saves |
| `/auth` | Show authentication state for the active connection | read-only |
| `/model [MODEL]` | Inspect or select a model on the active connection | this shell · --default saves |
| `/mode [MODE [--default]…]` | Inspect or select suggest, auto, or yolo mode | this shell |
| `/scope [SCOPE]` | Inspect or select workspace or host agent scope | saves config |
| `/network [allow\|deny]` | Inspect or select workspace-agent network policy | saves config |
| `/reasoning [LEVEL]` | Inspect or select reasoning effort | this shell · --default saves |
| `/details` | Cycle agent transcript density for this shell | this shell |
| `/status [OPTIONS…]` | Show the effective connection, model, mode, scope, and usage | read-only |
| `/usage` | Show token and estimated-cost usage | read-only |
| `/log [OPTIONS…]` | Show recent audit events and agent actions | read-only |
| `/reset` | Start a fresh conversation without deleting the prior session | conversation |
| `/settings` | Open the transactional settings editor | saves config |
| `/output [DENSITY]` | Inspect or save persistent agent transcript density | saves config |
| `/config [OPTIONS…]` | Print the active AIShe configuration | read-only |
| `/skills` | List model-invoked skills | read-only |
| `/mcp` | List configured MCP tools and prompts | read-only |
| `/palette` | Search AIShe actions from one focused menu | runs agent / edits files |
| `/agent [OPTIONS OBJECTIVE…]` | Launch a controlled foreground or background agent | runs agent / edits files |
| `/inbox` | Review agent work that needs attention | runs agent / edits files |
| `/sessions` | Browse, resume, inspect, or fork AI sessions | conversation |
| `/resume [ID]` | Resume the latest interrupted task or a session by ID | conversation |
| `/fork [SESSION_ID]` | Fork a managed conversation and switch this shell to it | conversation |
| `/task [ACTION OPTIONS…]` | Start and manage isolated background agent tasks | runs agent / edits files |
| `/plan [TASK_ID]` | Inspect or edit a durable agent checklist | conversation |
| `/replan [TASK_ID]` | Revise a checklist while retaining completed evidence | conversation |
| `/context [OPTIONS…]` | Inspect exact model-visible local context and token estimates | saves config |
| `/last [ACTION…]` | Explain, fix, retry, or clear the last shell failure | runs agent / edits files |
| `/role [ACTION OPTIONS…]` | Inspect or configure workload model roles | saves config |
| `/ask [OPTIONS QUESTION…]` | Ask a non-executing question with optional structured output | runs agent / edits files |
| `/index [OPTIONS…]` | Build or search the bounded repository index | runs agent / edits files |
| `/capabilities` | Show capability evidence for the active model | read-only |
| `/test [--live]` | Validate local UX or run paid live model/tool checks | runs agent / edits files |
| `/demo` | Run the safe guided first-session demonstration | conversation |
| `/undo` | Undo the most recent journaled AI file change | runs agent / edits files |
| `/trust [PATH]` | Trust a project AIShe configuration, command, or skill | saves config |
| `/untrust [PATH]` | Remove trust from a project AIShe file | saves config |

Top-level CLI-only commands (no slash or hook form):

| CLI command | Purpose | State/effect |
|---|---|---|
| `aishe hints [status [--json] \| reset]` | Inspect or reset local discovery-hint seen-state | read-only |

Removed names remain reserved for one compatibility window:

| Removed slash command | Local guidance |
|---|---|
<!-- END GENERATED COMMAND SURFACE -->

### `/connection` vs `/model`

| Command | Changes | Does not |
|---------|---------|----------|
| `/connection` | Active **account** (connection ID, auth, endpoint, default model for that account) | — |
| `/model` | **Model** on the current connection only | Login / credential / other accounts |

This split is intentional: picking a model must never quietly switch OAuth
profiles. After `aishe auth login openai|xai [--profile …]`, AIShe creates a
matching connection when none exists (e.g. `xai-work` / **Grok - OAuth · work**)
so `/connection` lists it immediately.

### Picker controls

Both `/connection` and `/model` use the same filterable picker when run on a
TTY:

- **↑ / ↓** or **Ctrl-P / Ctrl-N** — move selection
- **Home / End** — jump to the first or last match
- **Page Up / Page Down** — move by one visible page
- type any printable character to filter, including `d`, `j`, and `k`
- **Enter** — apply for this shell; when the choice differs from config, the
  following `[y/N]` prompt offers to make it the default for new shells
- **Esc** — cancel

Direct forms: `/connection ID`, `/model NAME`,
`aishe model NAME --connection ID`, `aishe connection use ID --default`.

OAuth model lists for **Codex - OAuth** and **Grok - OAuth** come from the
managed OpenCode runtime (`GET /config/providers`), not public
`GET /v1/models`. API-key connections use the endpoint catalog. See
[Providers](providers.md).

Ctrl-O is the keyboard equivalent of `/details`; Shift-Tab cycles
`suggest -> auto -> yolo`. Ask product how-to questions in natural language
(“how do I add a Codex OAuth account?”) — answers use the built-in
`aishe-product` skill.
## This shell versus defaults for new shells

The slash commands in the generated table are the supported interactive
surface. Hidden aliases (`/commands`, `/usage`, `/provider`, `/output`, and the
maintenance commands) still dispatch and tab-complete without appearing there.
Configuration field names not present in the table do not automatically become
prompt commands.

Use `aishe settings` for the fields it exposes, or edit `config.toml` for other
fields without a dedicated CLI command. `/reset` and `aishe reset` start
a fresh retained conversation. `/details` and Ctrl-O change transcript
density for following turns in the current shell; use
`aishe output focus|compact|detailed` to change the default for new shells.

## Reversible AI file edits

Every change the built-in file tools (`write_file` / `edit_file`) make in yolo is
shown as a diff and recorded to a journal, so you can take it back:

```sh
aishe undo          # revert the most recent AI file change (a whole run, in reverse)
aishe undo --list   # show recorded change sets and whether each is still active
```

All edits made in one aishe run share a batch, so a single `aishe undo` reverts
that run as a unit — a file the model created and then edited ends up removed, back
to its original state. The journal lives at `undo.jsonl` in aishe's
[data directory](configuration.md#file-locations) (override with
`$AISHE_UNDO_JOURNAL`). Journaling is best-effort and never blocks a write. See
[Reversible edits](modes.md#reversible-edits) for details.

## Changing settings

`aishe mode`, `aishe scope`, `aishe network`, `aishe output`, and legacy
`aishe provider` show or save durable settings. Account and model selection is
shell-local by default and is saved only through the post-selection default
prompt, `--default`, or `aishe connection use ID --default`. Reasoning follows
the same rule inside an
AIShe shell: `/reasoning high` is local and `aishe reasoning high --default`
saves it for the selected connection. The durable file is
`~/.config/aishe/config.toml` on Linux and `~/Library/Application
Support/aishe/config.toml` on macOS — `aishe doctor` prints the resolved path;
see [File locations](configuration.md#file-locations):

```sh
aishe mode auto         # persist the default mode
aishe scope workspace   # confine the next managed agent turn
aishe network deny      # no workspace-agent network capability
aishe output focus      # final responses; transient agent activity
aishe connection pick   # account picker (or /connection in shell)
aishe model             # models for the *active* connection
aishe model gpt-5.6-terra --connection openai-work
aishe reasoning high --default
aishe connection use openai-work --default
```
Use `aishe settings` for the interactive editor. It shows whether each effective
value came from defaults, the user config, a trusted project overlay, or a
session override; changes are staged and written only when you apply them.

The saved value goes to your user config (a project overlay or a `--mode`/
`--provider` flag on the same command is not baked in). You can also set these
per session with the `--mode`/`--model`/`--provider` flags or `$AISHE_MODE`, and
in the interactive shell **Shift-Tab** (or `$AISHE_MODE_KEY`) cycles the mode
`suggest -> auto -> yolo`. Every field is in
[Configuration reference](configuration.md).

Yolo **acceptance** is different from the saved default scope. Each new shell
asks once before granting workspace or host agent authority. Acceptance is
never written to config. Once accepted, yolo does not show per-action approval
prompts; auto remains action-gated.

## Durable managed sessions

Each shell/workspace/connection/model/mode/scope/network tuple maps to one
OpenCode conversation, so follow-ups keep
context across prompt processes, supervisor restarts, and AIShe upgrades.
`aishe sessions` presents those mappings together with legacy native task
records. `aishe resume ses_...` inside AIShe rebinds the live shell. When run
from a normal TTY, it changes to the recorded workspace and launches the real
zsh already bound to that conversation. It never blindly repeats an effect with
an unknown outcome.

## Inspecting things

`aishe status`, `aishe config`, `aishe mcp`, `aishe commands`, and `aishe skills`
print the live shell state, active config, and registries. They also work as
slash-commands in the `-c` form (`aishe -c '/status'`, `aishe -c '/usage'`, ...).

## Input prefixes

These are not commands; they control routing of a single line, and work in the
interactive shell and in `-c`:

- `?<text>` forces natural-language. Use it when your request starts with a real
  command name, for example `?find the largest files`. This is canonical on
  every front-end; the legacy `#` alias is deprecated and planned for removal
  in AIShe 0.9.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## Custom slash-commands

You can define your own `/commands` as Markdown files, plus model-invoked skills.
They run via the hook interactively and in the `-c` form. See
[Custom commands and skills](custom-commands-and-skills.md).

## Exiting

Exit with `exit`, `quit`, or `Ctrl-D`.
