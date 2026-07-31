# Commands and slash-commands

> **Alpha (pre-1.0).** The command surface can still change; prefer `/help` in a
> live shell for the current task-oriented index. Overview:
> [docs index](README.md) · [root README](../README.md).

aishe's interactive shell is your real zsh; aishe adds a small set of
subcommands, a few inspection commands, and input prefixes that control routing.

## Subcommands

```
aishe                  launch the interactive zsh-PTY shell
aishe zsh              the same, explicitly
aishe -c '<line>'      run one line non-interactively and exit
aishe setup            guided/resumable configuration and verification
aishe settings         interactive settings hub with value provenance
aishe auth ...         manage private named credential profiles
aishe connection ...   manage named provider/authentication connections
aishe tour             resumable guided first-session tour
aishe init zsh|bash    print the shell-hook snippet (for ~/.zshrc / ~/.bashrc)
aishe doctor           diagnostics; --probe/--live/--json/--fix/--bundle
aishe backend ...      status/install/verify/repair/rollback/stop/logs/gc
aishe uninstall        category-based removal; state preserved by default
aishe completions ...  print a shell completion script for aishe itself
aishe trust [PATH]     trust this repo's .aishe/config.toml, or one project file
aishe trust --list     list every trusted file
aishe untrust [PATH]   drop trust for this repo (or one file); --all for every one

aishe mode [suggest|auto|yolo]      show or set the interaction mode
aishe scope [workspace|host]        show or set the next agent execution scope
aishe network [allow|deny]          show or set workspace-agent network access
aishe output [focus|compact|detailed]  show or set agent transcript density
aishe reasoning [LEVEL] [--default]   shell-local or saved reasoning effort
aishe model [NAME] [--connection ID] [--default]  models for the active connection (OAuth via OpenCode)
aishe connection pick [ID] [--default]            switch account/connection
aishe provider [NAME]               select a unique provider connection (legacy form)
aishe provider test [--live] [--json]  validate the active provider
aishe models [--connection ID]      list models returned for one connection
aishe profile [VALUE]               show/apply a transparent safety profile
aishe readiness [--json]            check autonomous-mode readiness
aishe price list|set|remove         manage exact model price overrides
aishe config                        print the active configuration
aishe mcp                           list the MCP tools offered to yolo
aishe commands                      list primary and custom slash-commands
aishe status [--json]               show active session settings and spend
aishe skills                        list model-invoked skills
aishe undo [--list]                 revert the most recent AI file change
aishe log [filters]                 show the audit log of AI calls and actions
aishe usage [--by model|connection|day|session] [--connection ID]  audit token/cost totals
aishe context                       print the context block sent to the model
aishe runbook [--session ID|-o DIR|--replay]  export a session as a script + runbook
aishe sessions [--json]             list managed conversations and legacy tasks
aishe session show|rename|delete    inspect/manage exactly one session/task
aishe resume [ID] [--cwd PATH]      resume/bind a durable conversation or task
aishe reset                         start fresh; retain the prior conversation
```

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
/help              overview — accounts, models, session, config
/help accounts     add/switch accounts, OAuth vs API key, brands
/help models       models for the active connection only
/help session      status, usage, reset, reasoning, mode keys
/help config       setup, settings, doctor, tour, backend
```

`aishe commands` and `/commands` share the same help surface.

```text
/connection   switch account (provider + auth + endpoint)
/model        list/set models for the *active* connection only
/provider     alias for /connection
/auth         active connection authentication state and remediation
/status       connection, model, mode, scope, spend/plan, audit
/usage        live token and cost totals for this shell
/log          last 20 audit events and tool actions
/reasoning [LEVEL]  show or set shell-local reasoning effort
/details      expand/shrink agent work for following turns
/settings     interactive settings editor
/reset        fresh conversation; old session is retained
/commands     same as /help
```

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

- **↑ / ↓** or **j / k** — move selection
- type to filter
- **Enter** — apply for this shell only
- **d** (or the post-Enter “make this the default?” prompt when the choice
  differs from config) — save as durable default
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
## Prompt-only meta commands

A few settings are toggled by **meta commands at the aishe prompt**. Type them
inside the interactive shell, bare or with a leading `/`:

```
~/projects/app ❯ rehash            # or /rehash — rebuild the command cache
~/projects/app ❯ sandbox on        # yolo_sandbox
~/projects/app ❯ plan on           # yolo_plan (plan-first dry run)
~/projects/app ❯ cache off         # suggest-response cache
~/projects/app ❯ reset             # fresh conversation; old session is retained
~/projects/app ❯ details           # toggle focus/detailed output for this shell
```

Others in the same family: `editor`, `frontend`, `stream`, `structured`,
`theme`, `ghost`, `help`. `mode`, `model`, `provider`, `config`, `mcp`,
`commands`, `skills`, `usage`, `trust`, and `untrust` are *both* — real
subcommands and meta commands — so those work in either place.

`reset` is also a real `aishe reset` subcommand. Neither `/details` nor Ctrl-O
changes the saved `output` preference. The toggle affects following turns; an
inline shell cannot safely erase and redraw arbitrary historical scrollback.
Use `aishe output focus|compact|detailed` for a persistent choice.

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
shell-local by default and is saved only with `d`, `--default`, or
`aishe connection use ID --default`. Reasoning follows the same rule inside an
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
  command name, for example `?find the largest files`.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## Custom slash-commands

You can define your own `/commands` as Markdown files, plus model-invoked skills.
They run via the hook interactively and in the `-c` form. See
[Custom commands and skills](custom-commands-and-skills.md).

## Exiting

Exit with `exit`, `quit`, or `Ctrl-D`.
