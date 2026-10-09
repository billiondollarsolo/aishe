# Front-ends

aishe runs as your real zsh with an AI layered on top. There is one interactive
front-end (the zsh-PTY wrapper), plus a hook you can add to your own shell, plus
the non-interactive paths (`-c` and piped stdin).

## zsh-PTY front-end (the interactive shell)

Running `aishe` (or `aishe zsh`) launches interactive zsh inside a
pseudo-terminal with the native agent runtime. The shell profile is independent
of the agent engine:

```sh
aishe                              # clean, isolated zsh configuration
AISHE_ZSH_PROFILE=personal aishe    # your zsh configuration and plugins
```

The default `clean` profile skips personal and global interactive rc files.
The `personal` profile uses normal `zsh -i` startup and sources your `.zshenv`
and `.zshrc` from the real `ZDOTDIR` (or home), including a `ZDOTDIR` changed by
`.zshenv`. This does not enable OpenCode. Job control, pipelines, history
expansion, emacs/vi editing, and shell commands remain zsh's responsibility.

AIShe wraps Enter and slash completion while chaining the existing Enter/Tab
widgets for each emacs, vi insert, and vi command keymap. Personal custom
shortcuts win over optional AIShe bindings unless an `AISHE_*_KEY` explicitly
selects that shortcut. A late native rc can override or extend installed AIShe
widgets. Plugin behavior depends on its own configuration and load order;
AIShe does not emulate plugins.

Both profiles load `~/.aishe/leanrc` (or `AISHE_LEANRC`) before installing their
completion and widgets, then `~/.aishe/leanrc.post` (or `AISHE_LEANRC_POST`)
afterward. Put exports and completion paths in the early file, and widget or
key-binding overrides in the late file. Clean-profile completion dumps persist
under `$XDG_CACHE_HOME/aishe/zsh` or `~/.cache/aishe/zsh`, separated by zsh version
and completion search path. An unsafe cache directory falls back to temporary
storage.

The personal profile preserves `HISTFILE` (including an unset value),
`HISTSIZE`, `SAVEHIST`, and `SHARE_HISTORY`. Set `AISHE_MANAGE_HISTORY=1` to opt
into AIShe's history policy instead. The clean profile uses AIShe's timestamped
history log, so Up-arrow and `Ctrl-R` persist across sessions and binary
upgrades. With `share_history = true`, clean-profile shells exchange entries.

**It requires zsh.** If zsh is not installed, aishe tells you to install it
(rather than falling back to a lesser editor). Without zsh you can still use the
non-interactive paths (`aishe -c …`, piped stdin) and the bash hook below.

Natural-language input is routed to the already-running native parent; ordinary
commands stay in the child zsh. No OpenCode supervisor starts on this path.
Suggested commands pre-fill your next prompt. Use `/mode ask`, `/mode allow`,
`/mode agent`, or `/mode agent-host`; the legacy names `suggest`, `auto`, and
`yolo` remain aliases. Allow and agent require a grant for this live shell.
Workspace grants remain tied to the accepted canonical directory.

The clean profile supplies a bounded mode/scope prompt and configurable right
status. The personal profile preserves both `PROMPT` and `RPROMPT`. AIShe
refreshes `AISHE_MODE_INDICATOR` and `AISHE_BACKGROUND_INDICATOR` for a theme
to display, or you can opt into a mode/scope and background-work suffix on the
right prompt with `AISHE_PERSONAL_INDICATOR=1`:

```sh
AISHE_ZSH_PROFILE=personal AISHE_PERSONAL_INDICATOR=1 aishe
```

Use `AISHE_PTY_PROMPT=force` to request AIShe's full prompt in the personal
profile, or `pty_prompt = false` to suppress it. `/status` remains available
regardless of the prompt theme. AIShe's prompt labels honor `NO_COLOR` and ASCII
preferences and do not evaluate model or connection names as prompt code.

On a minimal account with no syntax-highlighting plugin, aishe supplies a
route-aware fallback: complete command-shaped input is green and recognized
natural-language questions are magenta. It evaluates the full buffer, so
`what --version` stays a command while `what is the capital of France?` changes
to the LLM route/color even if `what` is installed. It automatically gets out
of the way when zsh-syntax-highlighting or fast-syntax-highlighting is loaded.
Set `AISHE_COMMAND_HIGHLIGHT=0` to disable the fallback.

Color is not the only route signal. Press **Ctrl-X ?** to display
`aishe route: agent` or `aishe route: shell/local` for the current zsh buffer
without submitting or replacing it.

The branded prompt also has a configurable live status chip in zsh's native
right prompt; `off` hides it. Choose its ordered fields during setup or in
`aishe settings`. Fields include model, mode, backend, scope,
network, sandbox, task, elapsed time, latest context tokens, call/session
tokens/cost, budget, and request count.

The router recognizes a conservative set of full-line question forms beginning
with collision-prone commands such as `what`, `where`, and `who`. Ambiguous
imperatives such as `find large files` or `install kubectl please` remain
**shell** commands when the first word is a real binary (`install` is
`/usr/bin/install`). To force any line to the AI, **start it with `?`** (e.g.
`? install kubectl please`); the sigil is stripped before zsh sees it. The
zsh/Rust `#` spelling is a deprecated compatibility alias and remains an
ordinary comment in Bash. `?` is the reliable path on every supported tier.

Optional force-NL key: Meta/Alt+Return on zsh (`AISHE_NL_KEY`, default `^[^M`).
On Mac that is **Option+Return**, only if the terminal treats Option as Meta
(iTerm: Option → Esc+; Terminal.app: “Use Option as Meta key”). Prefer `?` if
keys are unreliable. Details:
[Shell integration — force-NL](shell-integration.md#force-nl-and-input-prefixes).
**Shift-Tab** (or `AISHE_MODE_KEY`) cycles mode
(`ask -> allow -> agent`) on empty input; nonempty input retains reverse
completion. Personal custom bindings may keep their original action; use
`/mode` or explicitly set `AISHE_MODE_KEY` in that case.

When a command **fails**, press **Ctrl-X Ctrl-F** (or `AISHE_FIX_KEY`) to ask the
model for a corrected command — it is pre-filled on your line for review, never
run automatically. Set `AISHE_AUTODIAGNOSE=1` to also print a one-line hint after
any failure pointing at the fix key. Bash key support is version-dependent and
must pass the [Tier-B matrix](bash-compatibility.md).

### Agent execution and live shell state

Before an AI, fix, or slash request, the native shell sends a private transient
snapshot of its exported environment. Agent commands use current `PATH`,
virtual environment exports, ordinary exported values, and unsets. Known shell
commands and individual keystrokes do not send snapshots. The snapshot is
consumed and removed; this bridge does not put environment values in task
checkpoints, audit records, or model prompts. Configured credential names,
common secret names, AIShe/backend control variables, and loader/startup
variables are filtered from tool execution.

The agent command shell prefers `dash -c`, with `zsh -f -c` as fallback. It does
not replay your aliases or functions or source personal startup files. Typed
commands still run in the real interactive zsh. Exporting a virtual environment
does not widen the accepted workspace or mount its external directories; a
runtime under a masked home directory can remain unavailable in workspace
scope. Use a runtime inside the workspace, or explicitly authorized host scope
where appropriate. Background workers retain a filtered launch environment
rather than continuously following later exports in the interactive shell;
resume does not save environment values.

Foreground, CLI, and background native agent turns share execution admission
and typed outcomes. See [native agent execution](configuration.md#native-agent-execution)
for exit codes, durable resume, budget semantics, and cancellation limits.

### Background work

Background work stays out of your command output. In the clean profile a small
prompt badge appears only when tasks are running or a result or problem has not
been seen. It counts work across all projects: for example, `2 running · 1 ready`.
**Needs you** marks a paused agent question or action approval. `attention`
marks failed, interrupted, or stopped work; `ready` marks an unseen result.
`queued` counts workflow stages waiting for dependencies or a worker slot.
Empty activity has no badge. Requests stay visible until you respond, while
reviewed results stay quiet across shell sessions.

Press **Ctrl-X b** to open the task browser, inspect a task, and return to the
same editable buffer and cursor position. An existing binding in the personal
profile is preserved; set `AISHE_BACKGROUND_KEY` to explicitly choose a
shortcut. `/tasks` opens the same browser. Outside the shell, use
`aishe task browse` or `aishe task browse TASK_ID`; add `--all` to include other
projects. `/inbox` and `aishe inbox` open **Needs you** across all projects.

The browser starts with the current project, or all projects if no local tasks
exist. Type to search, use the arrow keys to select a task, and **Enter** to
inspect it. **Tab** toggles the current project and all projects; **Ctrl-R**
refreshes. **Ctrl-V** chooses Current work, Needs you, or Archived history.
Details expose the result, recorded-check summary, checkpoint counters, and
limits. Press **Enter** or **u** to respond to a request, **f** to send a
follow-up, **e** for recorded checks, **l** for activity, **t** for the typed
timeline, **p** for read-only changes, or **?** for the searchable Task actions
menu. **Ctrl-F** filters the timeline by event type. **Esc** goes back or closes
the browser; **Ctrl-C** closes it from any page. Resume, stop, rework, apply,
and discard are explicit actions with confirmation;
opening a task or patch does not apply its changes. **a** opens file/hunk
selection with actual checks and a separate, revision-bound Apply decision.
The review states that a selected subset has not been checked separately.
Existing `aishe task`
start, list, tail, review, and lifecycle commands remain available. Tasks that
run in the source directory still offer confirmed stop, resume, and rework
controls. Changes, review, and apply require an isolated git worktree.

Opening finished details marks exactly that result reviewed across shells;
listing tasks does not. The task and its changes stay available, and a later
attempt or result appears in the counts again. Choose rename, pin, or archive
in Task actions; **n**, **i**, and **h** provide the same controls directly from
details. Names are display labels; the original objective stays intact. Archiving
keeps results and workspaces available in Archived history. Running tasks and
tasks waiting for your response cannot be archived.

During a native turn, **Ctrl-X d** queues background handoff at the next safe
provider/tool boundary. In task details, **g** offers to bring the same checkpoint
into this terminal with its saved authority and remaining allowances. Queued and
received handoffs remain visible in details. `/tasks fg ID` provides the same
continuation directly. `/workflow` inspects reusable task graphs and their
isolated stage tree. For commands, template examples, and execution boundaries,
see [Agentic task workflows](agentic-workflows.md).

#### Answer a request without losing your place

An agent can pause when missing information blocks useful work or when a
specific action needs approval. Open `/inbox`, select the task, and respond.
Questions can offer choices or accept a written answer. Approval shows the
proposed action and offers **Approve this exact action**, **Deny and continue**,
or **Leave for later**. Leaving it for later is the default; opening the inbox
does not approve anything. Denial returns the decision to the agent so it can
adapt its plan.

The paused worker exits after saving its request. A response continues the same
task with its saved transcript, model, scope, and remaining allowances; time
waiting for you does not consume active execution time. Approval is one-shot
and applies only to the exact action and execution context. It does not widen
workspace or network access. A file edit approval also binds the target's
current contents, so an intervening change cannot use the old decision.

#### Steer running work

Press **f** in task details to send a follow-up. It starts as **queued** and
becomes **received** when the worker saves it in the task transcript. The worker
reads follow-ups at safe boundaries, including before another tool action;
an already running command or provider request can finish first. Received
means delivered, not that the agent has completed the instruction.

Details show queued and received messages. Press **q**, or choose queued
follow-ups in Task actions, to edit or remove a queued message before the worker
starts delivering it. Once it is
being delivered or received, send a new follow-up instead. Follow-ups do not
answer a pending question or approve an action; use the request's response
control for that. Use rework when the task has already finished.
If a follow-up arrives just as the agent finishes, the task keeps it queued and
shows attention; resume the saved task to deliver it.

#### Inspect recorded checks

Press **e** for the commands actually run as checks, with their directory,
duration, observed exit status, and bounded output. The summary distinguishes
passed, failed, cancelled, not run, uncertain, and stale checks, and lists
unresolved items. Planned checks and the agent's own claims do not count.
Subsequent command activity, another check, edits, or opaque MCP calls mark
earlier checks stale; the original result remains visible. This records task
activity rather than monitoring every change made by another process.

Personal prompts keep their `RPROMPT` by default. Themes can display
`AISHE_BACKGROUND_INDICATOR`, or `AISHE_PERSONAL_INDICATOR=1` adds the optional
AIShe suffix. Activity uses a bounded local cache and shell FIFO updates;
the parent checks it every two seconds and redraws only when the counts change.
This does local work, while individual keystrokes do not launch a task-scanning
process. `AISHE_BACKGROUND_INDICATOR_ENABLED=0` hides the badge and clears the
theme variable. Logs remain in the browser rather than streaming across
your input line.

A native task's completion means the model finished its turn. Review its
result, changes, recorded checks, and unresolved items before applying. A
passing recorded command supports that check; it does not independently
certify every requirement of the task.

### Managed legacy shell

`AISHE_LEGACY_OPENCODE=1 aishe` selects the historical managed shell and backend.
Subscription OAuth setup discloses when that transport is required. Its
standalone hook and managed-session behavior are described in
[Shell integration](shell-integration.md) and
[Managed agent backend](managed-agent-backend.md); choosing the personal native
profile does not select this legacy path.

## Native zsh/bash hook

If you prefer to keep your own shell session rather than launching aishe, add the
hook to your shell startup instead:

```sh
# ~/.zshrc
eval "$(aishe init zsh)"

# ~/.bashrc
eval "$(aishe init bash)"
```

The standalone zsh hook uses the compatibility integration and routing contract.
When a personal native rc already evaluates `aishe init zsh`, AIShe avoids
installing that compatibility hook a second time and continues loading the rc.
The Bash hook is Tier B on Bash 5.x and reduced Tier B- on Bash 3.2: `#` remains
a Bash comment, its command-not-found and Readline facilities differ by version,
and it must pass its declared interactive matrix before a release claims
support. Separate
hook processes share one durable managed conversation through the
shell/workspace mapping. The Bash hook is the way to use AIShe interactively
without zsh, subject to the tested version matrix. Full details, including how
state changes persist across the subshell handoff, are in
[Shell integration](shell-integration.md); test scope and current evidence are
in [Native Bash hook compatibility](bash-compatibility.md).

## Non-interactive

`aishe -c '<line>'` runs a single line and exits, and piped stdin (`echo … |
aishe`) runs each line like a `-c` invocation. These use aishe's in-process
executor and dispatcher (zsh, falling back to bash), so they work without an
interactive terminal and without zsh present. Natural-language lines are answered
or, in ask mode, printed as a proposed command. Explicit autonomous work uses
`aishe agent 'objective'` or `aishe task start 'objective'`. An agent mode saved
in configuration alone does not grant an unattended natural-language turn
authority to run tools. Direct shell lines never start a model or backend.
Managed transport remains an explicit compatibility choice.
