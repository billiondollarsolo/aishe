# Daily-driver agent workflows

AIShe keeps the normal shell fast and native, then adds explicit agent actions
around it. A known command, alias, function, pipeline, redirect, or shell
control structure still belongs to zsh. Agent work begins only after an
agent-routed line, `?`, a dedicated `aishe` command, or an AIShe key binding.

## Shell editing and discovery

Inside `aishe` or the zsh integration:

| Action | Default | Result |
|---|---|---|
| Improve the current command | `Ctrl-X Ctrl-A` | Replaces `BUFFER` with one syntax-checked proposal; never executes it. |
| Fix the most recent failure | `Ctrl-X Ctrl-F` | Uses the private failure capsule and fills the correction for review. |
| Open the command palette | `/` then Tab, `/` then Enter, or `Ctrl-X Space` | Searches commands and configured connections, then fills the selected invocation. |
| Force natural language | `? request` or Alt/Option-Enter | Sends the line to the agent without executing the request as shell. |
| Toggle agent detail | `Ctrl-O` | Changes transcript density without changing authority. |

Press Tab or Enter on an exact `/` to open the same palette without remembering a key
binding. It includes commands plus configured roles, connections, cached models,
MCP servers, sessions, background tasks, and review-ready work. A selection is
inserted into the editable buffer for review; it is never executed automatically.
Quoted slash-command arguments remain data on both zsh and Bash; AIShe never
uses shell `eval` to reinterpret them.

Override the three new bindings with `AISHE_EDIT_KEY`, `AISHE_FIX_KEY`, and
`AISHE_PALETTE_KEY`. AIShe reports conflicts in `aishe doctor`; it does not
replace Tab, Enter, history, completion, job control, or your plugin manager.

`aishe ask --insert "request"` is the scriptable equivalent of a command
composer when invoked from an active AIShe shell. It writes through that
shell's private handoff and leaves Enter to the user.

## Launch one capable agent

Use one launcher instead of assembling role, account, scope, context, and task
commands yourself:

```sh
aishe agent                                      # guided launcher
aishe agent 'review this branch and run tests'   # foreground managed session
aishe agent --background --role build \
  --file Cargo.toml --dir src --diff \
  --max-minutes 25 --max-turns 30 --max-cost 1.50 \
  'finish the parser change and validate it'
```

Foreground agents use AIShe's existing managed conversation, tools, audit, and
undo. Background agents use the isolated task controller below. `--scope
workspace` is the default; `--scope host` requests broader policy authority but
does not bypass policy or protected-environment confirmation. Explicit
`--connection` and `--model` win over the selected workload role.

Before real work, inspect cached evidence with `aishe capabilities`. Run `aishe
test` for local/offline checks. Only `aishe test --live` makes the small paid
text, structured-output, tool, and streaming probes.

## Attach exact context

Attachment markers are expanded only after a request has been routed to the
agent. They are inert in ordinary shell commands.

```text
? explain @file:src/main.rs
? compare @file:"docs/old design.md" with @file:docs/new.md
? review @dir:src @diff
? summarize @clipboard
```

Workspace scope rejects paths outside the current canonical workspace and
symlink escapes. Directory traversal skips `.git` and symlinks, stops after
three levels, and fails if more than 24 files are selected. A file is limited
to 64 KiB and all attachments together to 256 KiB. NUL-bearing data is labeled
binary and omitted. Secret redaction remains enabled by default. Clipboard
reading is explicit: AIShe never reads it without `@clipboard`.

For repeated code lookup, build a local tracked-file index instead:

```sh
aishe index
aishe index --status
aishe index --query 'session rotation' -n 8
aishe index --query 'session rotation' --json
aishe index --rebuild
```

The index uses `git ls-files`, content hashes, bounded local chunks, and lexical
ranking. It sends nothing to a provider by itself. The store is private,
incremental, capped at 10,000 files/64 MiB, and keyed to the canonical worktree
root.

## Long work in the background

Start writable coding work from a git repository:

```sh
aishe task start 'upgrade the parser and run focused tests'
aishe task list
aishe task show TASK_ID
aishe task tail TASK_ID
```

AIShe creates a detached worktree under its private task directory before the
agent starts. The active checkout may already be dirty; the task still begins
from its recorded `HEAD` and cannot modify that checkout. A non-git directory
is refused unless `--no-isolation` explicitly accepts work in place.

Every task snapshots finite limits. Tune them per task:

```sh
aishe task start 'bounded refactor' \
  --max-minutes 20 --max-turns 30 --max-cost 2.00 \
  --max-tool-calls 80 --max-network-calls 10 \
  --max-changed-files 30 --max-changed-bytes 500000
```

The task record stores a redacted objective, PID plus process-start identity,
source branch/HEAD, worktree, plan, limits, and terminal state. The raw request
and bounded 8 MiB log are separate private files. Background children inherit
only required runtime variables and explicitly configured credential references;
agent-spawned commands have those credential names removed again.

Lifecycle operations are explicit and idempotent where possible:

```sh
aishe task cancel TASK_ID
aishe task resume TASK_ID
aishe task plan TASK_ID 'inspect parser' 'edit parser' 'run tests'
aishe task step TASK_ID 1 completed
aishe task replan TASK_ID 'inspect parser' 'edit parser' 'run focused tests'
aishe task step TASK_ID 3 completed --evidence 'cargo test: 586 passed'
```

`aishe plan [TASK_ID]` and `aishe replan [TASK_ID]` provide the compact
interactive equivalents. Replanning retains completion and evidence only when
the step text is unchanged.

Before keeping work, review the exact patch:

```sh
aishe task review TASK_ID
aishe task apply TASK_ID                 # whole patch, git apply --3way
aishe task apply TASK_ID --hunk 2 --hunk 5
aishe task discard TASK_ID               # validates the owned worktree first
```

`aishe inbox` is the daily attention queue. It refreshes task state and offers
tail, cancel, review, resume, show, or rework. `aishe inbox --json` is stable for
scripts. The interactive review panel can apply everything, toggle selected
hunks, send bounded rework instructions, reject/discard, or leave the isolated
workspace untouched.

Review numbers every text hunk; binary and mode-only changes remain file-level.
An exceeded changed-file/byte budget blocks apply. Conflicts fail without
silently resolving or deleting the isolated worktree.

## Browse and fork conversations

`aishe sessions` opens a workspace-aware session browser in a terminal and
retains machine-readable listing behavior with `--json`. Resume the current or
named conversation with `/resume` or `aishe resume ID`. Fork the current managed
conversation with `/fork` or `aishe session fork [SESSION_ID]`; the fork keeps
history and becomes this shell's active conversation. AIShe refuses a managed
fork when the selected connection/model does not match the source session.

## See exactly what the model sees

```sh
aishe context --explain
aishe context --preview 'review @file:src/main.rs' --json
aishe context --show --preview 'review @file:src/main.rs @diff'
```

`--show` uses the real request attachment expansion and prints the exact local
context only after redaction. It does not contact the provider. The metadata
forms show sources, include/exclude decisions, redaction counts, token estimates,
and estimated input cost without revealing content.

## Recover from command failures

After an interactive command fails, AIShe stores its sanitized command, exit
status, cwd, and duration in a mode-`0600` capsule scoped to that live shell.

```sh
aishe last show
aishe last explain
aishe last fix
aishe last retry                 # preview only
aishe last retry --execute       # only for the existing read-only classifier
aishe last clear
```

Effectful, unknown, or redacted commands are printed for review and are never
executed by retry. The capsule is cleared after the next successful command and
on normal shell exit. AIShe intentionally does not intercept arbitrary command
output just to populate this feature.

## Model roles

Roles let fast composition, explanation, and longer builds use different models
without mixing account identities:

```sh
aishe role set compose --connection openai-work --model fast-model --reasoning low
aishe role set answer --model balanced-model
aishe role set build --model deep-model --reasoning high
aishe role list
aishe role remove compose
```

Supported roles are `compose`, `answer`, `build`, `review`, and `embed`. An
explicit `--connection` or `--model` wins over a role. Missing role fields use
the current selection. The selected connection still owns authentication and
organization policy remains the final constraint.

## MCP without secret-bearing TOML edits

```sh
aishe mcp add filesystem --command npx --arg -y --arg @modelcontextprotocol/server-filesystem
aishe mcp add tickets --url https://mcp.example.test \
  --header-env Authorization=TICKETS_AUTH
aishe mcp list
aishe mcp show tickets
aishe mcp test tickets
aishe mcp disable tickets
aishe mcp enable tickets
aishe mcp edit tickets --url https://mcp2.example.test \
  --header-env Authorization=TICKETS_AUTH
aishe mcp remove tickets
```

`--env TARGET=SOURCE_ENV` and `--header-env HEADER=SOURCE_ENV` store only an
`env:SOURCE_ENV` reference. AIShe never accepts the secret value as a CLI
argument. Add/edit validates the transport before atomically saving config;
show/list redact legacy literal values.

## Structured automation

```sh
aishe ask 'summarize this repository'
aishe ask --json 'summarize this repository' | jq .answer
aishe ask --schema ./result.schema.json 'extract release risks' | jq .result
```

Machine stdout is one schema-versioned JSON document. Diagnostics use stderr.
Schema mode validates a bounded JSON Schema subset locally: object/array/scalar
types, required/properties, `additionalProperties`, enum, and items. Invalid
provider output exits nonzero with no successful payload on stdout.

## Target identity and protected environments

The status and `aishe status --json` identify hostname/SSH/container, git
branch and HEAD, Kubernetes context, common cloud profile, and an explicit
protected marker. Configure token or glob matches:

```toml
[sandbox]
protected_environment_patterns = ["prod", "production-*", "customer-live"]
```

Classification does not grant authority. In a protected environment, an
interactive host-scope yolo turn requires a fresh typed `host LABEL`
acknowledgement; noninteractive host-scope autonomy fails closed. Workspace
scope remains the default.

## Status placement and terminal compatibility

AIShe's live connection, model, mode, scope, branch/environment, session cost,
request count, and background-task count are rendered through zsh's native
right prompt. Spaceship is extended through its right-prompt section API;
other themes keep their existing `RPROMPT` and AIShe appends its chip. Legacy
`status_line_position = "below"` values migrate to `right`; set `right` or
`off` in setup/settings.

Native ZLE does not provide a reserved, persistent footer row. A simulated
below-prompt footer conflicts with asynchronous themes and autosuggestions and
can leave a new line on every redraw. A guaranteed fixed footer, like Codex or
Claude Code, requires a dedicated full-screen terminal renderer. The native
right prompt stays visible while typing and yields automatically if the command
would overlap it.

The display omits lower-priority fields that do not fit, truncates any single
overlong field to terminal width, and treats provider/model text as data,
not prompt expansion. Unicode improves the glyphs, but no patched or Nerd Font
is required. `ui.unicode = "ascii"`, a missing UTF-8 locale, `TERM=dumb`, or
redirected output selects functional ASCII/plain fallbacks. Shipping a font
would add install, licensing, terminal-configuration, SSH, and remote-host
failure modes without adding capability, so AIShe deliberately does not.

## Updates and portable profiles

```sh
aishe update check
aishe update check --json
aishe update apply                 # preview + confirmation
aishe update rollback              # swaps the one previous verified binary

aishe profile export ./aishe-profile.toml
aishe profile import ./aishe-profile.toml
```

Update apply downloads the platform release archive over HTTPS, enforces size
bounds, verifies the published SHA-256, accepts exactly one platform-format
`aishe` file, executes its `--version` self-test, keeps one private previous
binary, and activates with a same-directory atomic rename. Release artifacts
also carry GitHub provenance attestations; the client currently verifies the
release checksum and executable itself. Rollback never touches config or state.

Profile export contains config and environment-variable references, never API
keys or OAuth stores. Import previews counts and paths, refuses literal MCP
secret material, preserves credentials separately, and writes a recovery copy
of the prior config.

## Data and cleanup

New stores are under `<data>/aishe/`:

- `background-tasks/` — records, requests, bounded logs, and isolated worktrees;
- `repo-index/` — content-hashed tracked-file chunks;
- `failures/` — per-live-shell failure capsules;
- `updates/` — the one rollback binary.

Run `aishe doctor` for the real platform paths. `AISHE_CONFIG_DIR` and
`AISHE_DATA_DIR` override them. See [Data retention](data-retention.md) before
manual cleanup; a task worktree should be removed with `aishe task discard` so
git worktree metadata and the owned path are validated together.
