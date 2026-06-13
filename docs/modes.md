# Modes

aishe has three interaction modes for natural-language input. Real commands run
the same way regardless of mode.

| Mode      | Glyph | Behavior                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | Default. The LLM proposes a command; you confirm with `[Enter] / [e]dit / [n]`. |
| `auto`    |  `»`  | Commands the safety gate deems safe run immediately; dangerous ones still require typing `yes`. |
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
immediately. A command the gate flags as dangerous still stops and asks you to
type the full word `yes`. This keeps the convenience of one-step execution while
protecting against destructive operations.

## yolo

yolo is an agentic loop. The model is given tools, calls one, reads the result,
and decides what to do next, repeating until the task is done or it hits
`max_yolo_iterations`. Key points:

- The core tool is `run_command`: it runs a shell command with stdin closed
  (non-interactive flags), the captured output is shown to you and truncated for
  the model, and each command times out after a fixed limit.
- **File tools** (`file_tools = true`, on by default): the model can also call
  `read_file`, `write_file`, `edit_file`, and `list_dir` to work with files
  precisely, instead of round-tripping through `cat`/`sed`/heredocs (which it
  gets wrong more often). A write or edit to a path outside the working tree
  (absolute, `~`, or `..`-escaping) is confirmed whenever the confirmation tier
  is not `"never"` (see `yolo_confirm` below). Every such edit is shown as a diff
  and is **reversible** — see [Reversible edits](#reversible-edits).
- **Web tool** (`web_tool = true`, on by default): the model can call `fetch_url`
  to read a page or docs (HTTP GET over http/https only; HTML is stripped to
  readable text, the body is byte-capped while reading and char-capped before it
  goes to the model). Use this instead of `curl`/`wget` for reading the web.
- **MCP tools**: if you configure [MCP servers](mcp.md), their tools are offered
  too, namespaced `mcp__<server>__<tool>`, and proxied to the server when called.
- **Plan-first (dry run)** (`yolo_plan = true`, off by default): before the loop
  runs anything, the model lays out its intended steps and you approve them
  (`Proceed with this plan? [Y/n]`). Costs one extra planning call and applies
  only interactively (a piped/`-c` run has no one to approve, so it proceeds).
  Toggle live with `aishe plan on`.
- **Preview-first file edits** (`yolo_preview = true`, off by default): when the
  loop uses a built-in `write_file` or `edit_file`, show the diff and ask
  `apply this write/edit to <path>? [y/N]` before touching the file, instead of
  writing first and showing the diff after. Applies to the file tools only (use
  `yolo_confirm` / `yolo_sandbox` for arbitrary `run_command` side effects). As
  elsewhere, a piped/`-c` run has no one to answer, so it applies automatically.
  Every applied change is still journaled for `aishe undo`.
- **Confirmation tiers** (`yolo_confirm`): control when the loop pauses to
  confirm a `run_command` call:
  - `"never"` runs everything without asking.
  - `"dangerous"` (the default) confirms only commands the
    [safety gate](safety.md) flags.
  - `"writes"` confirms dangerous commands and any command that modifies state
    (anything not recognized as a read-only command such as `ls`/`cat`/`grep`/
    `git status`).
  - `"all"` confirms every command.
  The older `yolo_confirm_dangerous` boolean still works: it only takes effect
  when `yolo_confirm` is left at its default, where `yolo_confirm_dangerous =
  false` means the same as `"never"`. Otherwise `yolo_confirm` wins. As with the
  rest of aishe, a piped/`-c` run has no one to answer, so a confirm proceeds
  automatically there.
- **Sandbox** (`yolo_sandbox = true`, off by default): a policy-based restricted
  exec. When on, a `run_command` that reaches the network or writes outside the
  working tree is refused before it runs (the reason is fed back to the model so
  it can adapt), rather than executed. It is best-effort, not a kernel sandbox;
  see [Safety](safety.md#sandbox-policy-based-best-effort). Toggle live with
  `aishe sandbox on`.
- If skills are present, the model can pull a skill's instructions into context
  on demand. See [Custom commands and skills](custom-commands-and-skills.md).
- Every tool call is recorded in the [audit log](logging.md) (`run_command` as an
  `action`, the built-in tools as `yolo:read_file` / `yolo:fetch_url` etc.).

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
not arbitrary `run_command` side effects (use `yolo_confirm` / `yolo_sandbox` for
those — see [Safety gate](safety.md)). The journal is JSONL at
`$XDG_DATA_HOME/aishe/undo.jsonl` (override with `$AISHE_UNDO_JOURNAL`); journaling
is best-effort and never blocks or fails a write.

## Streaming

Enable token streaming so answers render live as they arrive:

```sh
aishe stream on        # or stream = true in config
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
a dark theme. This applies to both streamed and non-streamed answers.

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

```sh
aishe structured schema     # strict schema (default)
aishe structured json       # any JSON object
aishe structured prompt     # unconstrained text
```

yolo mode uses tool calling for the same reliability. Regardless of what the
model returns, the deterministic [safety gate](safety.md) decides what actually
runs. The model's output is never trusted to be safe.

## Conversation memory

In the interactive REPL, aishe remembers recent natural-language turns so
follow-ups have context. After "create a file alpha.txt containing apple", a
follow-up like "now do the same for beta.txt" knows that "the same" means
"containing apple".

- Memory applies across suggest, auto, and yolo turns in one session.
- It stores your requests and the assistant's replies (a suggested command or
  answer, or a yolo run's final summary), not the full tool-by-tool transcript,
  so it stays small. It is capped by an approximate size budget and is never
  written to disk.
- It lives only for the interactive process. One-shot `-c` runs and the shell
  hook do not carry memory between invocations.
- Clear it any time with `aishe reset` (or `/reset`).
- Turn it off with `memory = false` in config. Note that more history means more
  input tokens per request; see [Token usage and cost](usage-and-cost.md).

## Input prefixes

- `?<text>` forces natural-language, for example `?how do I find large files`.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error using the failed command and its output.
