# Modes

aishe has three interaction modes for natural-language input. Real commands run
the same way regardless of mode.

| Mode      | Glyph | Behavior                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | Default. The LLM proposes a command; you confirm with `[Enter] / [e]dit / [n]`. |
| `auto`    |  `»`  | Commands the safety gate deems safe run immediately; anything it flags or cannot resolve stops and asks. |
| `yolo`    |  `⚡`  | Agentic loop: the model runs commands, reads output, and iterates until done. |

Switch at any time:

```sh
aishe mode auto
```

Or set the mode for a single session:

```sh
aishe --mode yolo
```

## suggest

The model returns a single command and a short explanation. aishe shows it and
waits:

```
~/projects/app ❯ whats eating my disk
  du -sh * | sort -rh | head
  [Enter] run   [e] edit   [n] cancel
```

If the request is a question rather than a task, the model answers in prose
instead of proposing a command.

## auto

Same proposal step, but a command the safety gate considers safe runs
immediately. The gate has three outcomes, so two of them still stop:

- **safe** — nothing matched and every segment resolved: runs straight away.
- **could not verify** — the gate could not work out what a segment would run
  (an unresolvable head such as `$(which rm)`): a yellow panel and a plain
  `[y/N]`. It fails closed; nothing unverified auto-runs.
- **dangerous** — a rule matched a destructive shape: a red panel, and you must
  type the full word `yes`.

This keeps the convenience of one-step execution while protecting against
destructive operations. See [Safety gate](safety.md#three-outcomes).

## yolo

Yolo uses Aishe's managed OpenCode engine to plan, call tools, inspect results,
compact context, create subagents where appropriate, and continue until the task
is done. OpenCode never executes a host tool directly: its built-ins are
hidden/denied and every effect crosses Aishe's authenticated foreground bridge.

### One acceptance, not approval spam

The first yolo turn in every new shell asks you to accept one scope:

- **workspace** — effects are confined to the canonical workspace and any
  explicitly configured roots. Workspace network is separately `allow` or
  `deny`. On Linux, bubblewrap is the supported OS boundary.
- **host** — explicit host-wide agent authority for this shell. The warning
  names the consequences; organization policy may disable this choice.

After acceptance, yolo does not ask again for each command or edit. That is the
mode's contract. Acceptance is in memory only, never config, so opening another
shell asks again. `aishe scope workspace|host` changes the default selection;
`aishe network allow|deny` changes the workspace network capability.

`yolo_confirm` and `yolo_confirm_dangerous` remain readable for the temporary
native compatibility backend and legacy tasks. They do not reintroduce
per-action approval into an accepted managed yolo session.

### Tools and ownership

- `run_command` executes through Aishe with bounded output, timeout/process-group
  cancellation, scope checks, sandbox policy, redaction, and audit.
- `read_file`, `write_file`, `edit_file`, and `list_dir` stay path-confined;
  writes produce diffs and undo records.
- `fetch_url` obeys the workspace network capability and bounded response rules.
- Configured [MCP servers](mcp.md) remain namespaced
  `mcp__<server>__<tool>` and are invoked by Aishe.
- [Skills](custom-commands-and-skills.md) are progressively disclosed by Aishe;
  project trust and organization policy still apply.

Provider keys are available only to the managed provider process. Every
model-controlled command/tool environment starts from an explicit sanitized
snapshot with provider variables, `AISHE_*`, `OPENCODE_*`, and likely secret
names removed.

### Isolation, durability, and budget

On Linux, `[sandbox] linux_backend = "bwrap"` gives workspace tools a read-only
host, writable project/private `/tmp`, and explicit network profile. Setup and
Doctor run a functional namespace test; merely finding the executable is not
enough. macOS is clearly labeled policy-only. `aishe dry-run` and legacy
`yolo_dry_run` remain available for throwaway-copy previews.

Every provider turn is authorized against Aishe's exact price/budget before the
request. Usage is accepted once per provider message, including child sessions,
and updates the statusline. An exhausted budget denies the next turn without
destroying the conversation.

Every tool call is durably journaled before execution. A duplicate completed
call replays its result; a call interrupted after start is marked
outcome-unknown and is never silently repeated. Use:

```sh
aishe sessions
aishe session show ses_...
aishe resume ses_...
```

Use `aishe profile conservative|balanced|autonomous` to apply a transparent
settings bundle. `aishe readiness` reports the provider, runtime, tool,
sandbox, and policy prerequisites that have actually been validated.

## Reversible edits

Every file the built-in tools (`write_file` / `edit_file`) change in yolo is shown
as a colored unified diff as it happens, and its prior contents are recorded to a
journal — so you can take any AI edit back:

```sh
aishe undo          # revert the most recent AI file change
aishe undo --list   # list recorded change sets (active / reverted)
```

All edits in one aishe run share a *batch*, so a single `aishe undo` reverts that
run as a unit and in reverse order: a file the model created and then edited is
removed entirely (back to not existing), and a file it overwrote is restored to its
original bytes. Reverting marks the batch done, so a second `aishe undo` moves on to
the previous run.

This is a safety net, not a sandbox: it covers the built-in **file tools** only,
not arbitrary `run_command` side effects (use workspace scope/bubblewrap or
`aishe dry-run` for those — see [Safety gate](safety.md)). The journal is JSONL at `undo.jsonl` in
aishe's [data directory](configuration.md#file-locations) (override with
`$AISHE_UNDO_JOURNAL`); journaling is best-effort and never blocks or fails a
write.

## Streaming

Enable token streaming so answers render live as they arrive:

Type this at the aishe prompt (`stream` is a
[prompt-only meta command](commands.md#prompt-only-meta-commands), not an `aishe`
subcommand):

```
~/projects/app ❯ stream on        # or stream = true in config
```

In suggest and auto mode, an answer streams to the screen as it is generated.
Once the model commits to a command instead of prose, aishe falls back to the
normal confirm or run flow, so a command is never half-printed. When a prose
answer finishes streaming, aishe re-renders it as markdown in place.

In yolo mode, the agentic loop streams the model's text live too, so long runs no
longer look frozen: you see the reasoning and the final answer as they arrive,
interleaved with the tool-call lines. When a turn is the final answer, aishe
re-renders the streamed text as markdown in place once it completes, so headers,
lists, emphasis, and code blocks look right. If the streamed answer was taller
than the screen, the raw text is kept as-is (it cannot be safely re-rendered after
scrolling).

Streaming works with both providers over SSE, including streamed tool calls.
Endpoints without SSE simply deliver the whole answer at once (aishe falls back
automatically). Streaming is not used for the scriptable suggest `-c` path.

## Rendering and syntax highlighting

Model answers are rendered as markdown: headers, lists, emphasis, tables, inline
code, and fenced code blocks. Fenced code blocks are syntax-highlighted by
language (for example ```python or ```rust), using a bundled set of syntaxes and
a dark theme. Blocks get a subdued language label and closing rule without
prefixing copied source lines. This applies to both native and managed OpenCode
answers, whether streamed or buffered. Redirected output, `TERM=dumb`, and
`NO_COLOR` preserve the original Markdown structure without ANSI styling.

Highlighting is built in by default. To build a smaller binary without the
bundled syntaxes, compile with `--no-default-features`; code blocks then render as
plain styled blocks. See [Installation](installation.md).

## Structured output

To get dependable, actionable results, suggest mode asks the model for a strict
JSON schema by default (`structured = "schema"`) on providers that support it.
This guarantees the `{type, command, explanation}` shape. If a provider rejects
the schema, aishe automatically steps down to a plain JSON object, then to
prompt-only, and always parses defensively so unrecognized output becomes a plain
answer rather than a crash.

Also a prompt-only meta command, so type it at the aishe prompt (or set
`structured` in your config):

```
~/projects/app ❯ structured schema     # strict schema (default)
~/projects/app ❯ structured json       # any JSON object
~/projects/app ❯ structured prompt     # unconstrained text
```

yolo mode uses tool calling for the same reliability. Regardless of what the
model returns, the deterministic [safety gate](safety.md) decides what actually
runs. The model's output is never trusted to be safe.

## Conversation memory

Managed suggest, auto, and yolo turns use one durable OpenCode conversation per
Aishe shell/canonical workspace. A follow-up from the hook's next short-lived
process therefore retains the same request, response, tool, and compaction
context. It also survives an idle supervisor exit and an Aishe binary/runtime
upgrade.

- `aishe sessions` lists managed mappings and legacy native task records.
- `aishe resume ses_...` rebinds the current Aishe shell; from a normal TTY it
  opens the real zsh in the recorded workspace already bound to that session.
- A changed workspace gets a different conversation by default.
- `reset`/`/reset` at the prompt, or `aishe reset`, detaches the current
  conversation without deleting it. The command prints its resumable session ID.
- Focus output is the default: routine tool/reasoning activity is transient and
  only the final response remains in scrollback. Ctrl-O or `details` toggles
  detailed output for this shell; `aishe output focus|compact|detailed` persists
  a preference.
- Private backend/session contents are excluded from support bundles.
- `memory = false` controls the capped native compatibility transcript during
  the transition; managed conversation durability is part of the backend
  session contract.

Long conversations are compacted by the agent engine. The `context` statusline
field reports the latest authoritative input-token count rather than inventing a
percentage. See [Token usage and cost](usage-and-cost.md).

## Input prefixes

- `?<text>` forces natural-language, for example `?how do I find large files`.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error using the failed command and its output.
