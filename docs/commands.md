# Commands and slash-commands

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
aishe model [NAME]                  show or set the model (for the active provider)
aishe provider [anthropic|openai]   show or set the provider
aishe provider test [--live] [--json]  validate the active provider
aishe models [--provider NAME]      list models returned by an endpoint
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
aishe usage [--by model|day|session]  token/cost totals from the audit log
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

They always operate on the exact OpenCode version embedded in this Aishe build.
`--from` supports offline installation but does not bypass checksum, archive
size, executable-version, or license/notices verification.
`backend status --json` is schema-versioned and separates runtime state from a
sanitized running/stopped/stale supervisor summary; it never exposes local
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

## Primary slash commands

The standalone Aishe shell prints a one-line `/help` hint at startup. `/help`
and `aishe commands` show the same compact index:

```text
/help       command index
/status     model, mode, scope, output, and live spend
/usage      live token and cost totals for this shell
/details    expand/shrink agent work for following turns
/settings   interactive settings editor
/reset      fresh conversation; old session is retained
/commands   primary and installed custom slash-commands
```

Ctrl-O is the keyboard equivalent of `/details`; Shift-Tab cycles
`suggest -> auto -> yolo`.

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

`aishe mode`, `aishe scope`, `aishe network`, `aishe output`, `aishe model`,
and `aishe provider` show the current value with no argument, or save a new one
to your user config with an argument
(`~/.config/aishe/config.toml` on Linux, `~/Library/Application
Support/aishe/config.toml` on macOS — `aishe doctor` prints the resolved path;
see [File locations](configuration.md#file-locations)):

```sh
aishe mode auto         # persist the default mode
aishe scope workspace   # confine the next managed agent turn
aishe network deny      # no workspace-agent network capability
aishe output focus      # final responses; transient agent activity
aishe provider openai   # switch provider...
aishe model gpt-4o      # ...then set that provider's model
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

Each shell/workspace pair maps to one OpenCode conversation, so follow-ups keep
context across prompt processes, supervisor restarts, and Aishe upgrades.
`aishe sessions` presents those mappings together with legacy native task
records. `aishe resume ses_...` inside Aishe rebinds the live shell. When run
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
