# Getting started

> These instructions describe the v1.1.0 candidate. The installer downloads the
> latest published release; see its version with `aishe --version`.

This page walks through your first session with **AIShe** (**AI Shell**).

## 1. Open your shell

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh -s -- --launch
# Later, or when AIShe is already installed:
aishe
```

Ordinary shell commands work immediately without credentials or a runtime
service. A fresh install keeps your zsh configuration, prompt, plugins, and
history. Exit returns to the shell that launched AIShe.

Use `/setup` or `aishe setup` to connect an AI account. **Connect later** saves
only the shell settings you review; it does not check a provider or download a
runtime. Asking the agent before connecting gives an actionable setup cue.
Setup is interactive and resumable. Choose a shell experience first:

| Choice | What starts |
| --- | --- |
| Keep my zsh | Your `.zshenv`/`.zshrc`, theme, plugins, history, and native agent |
| Clean AIShe | Isolated native zsh with AIShe's complete prompt |
| Bash integration | Your `.bashrc`, prompt and Readline, with the reduced Bash hook |

`/settings` saves this choice for new shells. Optional `aishe activate zsh` or
`aishe activate bash` previews a reversible startup block; `--apply` writes it
with a backup, and `--remove` restores the surrounding startup file. The login
shell stays unchanged. Existing configurations without a
shell-profile setting keep their previous clean profile. `AISHE_ZSH_PROFILE`
remains an explicit one-process override for zsh.

AIShe writes its configuration to a per-platform directory: `~/.config/aishe/` on
Linux but `~/Library/Application Support/aishe/` on macOS — aishe follows each platform's
own convention, and a file left in the wrong one is silently ignored. Run
`aishe doctor` to see the path actually in use, or set `AISHE_CONFIG_DIR` (and
`AISHE_DATA_DIR`) to pick your own. Full table:
[File locations](configuration.md#file-locations). The docs write these paths in
their Linux form for brevity.

The main decisions are:

- **Account and model:** connect later, or use API keys and local endpoints with
  the included native engine. Subscription OAuth explicitly selects a managed
  legacy transport.
- **Behavior:** accept the recommended ask mode and compact status, or customize
  mode, scope, history, output, and logging. Allow and agent still require a
  separate grant for each live shell.
- **Review:** see account, behavior, readiness, and pricing before Apply.
  Detailed diagnostics and the exact configuration diff are optional actions.

Setup checks shell and workspace readiness without a runtime download on the
native path. Unknown-model pricing can be deferred. Full generation and tool
checks are opt-in, with their token use disclosed before consent.

The endpoint prompt is what lets you point at Groq, Ollama, or any other
OpenAI-compatible service instead of OpenAI; pick the service and the base URL
are filled in for you (editable). After the credential step, Setup calls
`GET /v1/models`. A successful response verifies the endpoint and key without
using tokens and supplies the model picker. You can also type any model ID:
Setup checks the full returned catalog, then makes one clearly disclosed
minimal generation request only when the ID was not listed. Credential,
permission, network, and model-not-found failures stay in Setup with retry/back
choices instead of silently accepting an unverified value.

When an existing config and credential are available, setup offers a short
path directly to validation
and the final review. Before any live checks, Setup states that the text,
structured, tool, and streaming probes consume tokens and may incur provider
charges; declining never turns an unrun live check into a pass.

When you explicitly choose managed subscription OAuth, OpenCode is managed by
AIShe. Setup downloads the exact version pinned
by this AIShe build, verifies its size/checksum/version/notices, launches it with
private HOME/XDG directories on authenticated loopback ports, and verifies the
trusted AIShe plugin and tool restrictions. It never reuses an arbitrary
`opencode` on `PATH` and never opens a second TUI. Ordinary zsh commands remain
independent of this runtime.

On Linux, selecting isolated workspace-agent behavior requires a bubblewrap
self-test, not just the presence of a `bwrap` executable. Native setup reports
whether isolation is available; install bubblewrap before choosing isolated
workspace-agent work. Managed setup shows an installation plan and asks before
sudo. If namespaces are unavailable in a container/kernel, setup explains that
condition. Host scope remains an explicit choice where policy allows it; macOS
workspace restrictions are clearly labeled policy-only.

Setup does not change `config.toml` or `credentials.toml` until Apply. The
agent runtime, any system package you approve, and an OAuth login are written
when you choose them, and the resumable draft (choices only, never credentials)
is saved after every step. If interrupted, rerun `aishe
setup --resume`; use `--restart` to discard only its draft. In a pipe or CI,
setup exits instead of inventing defaults; use `aishe setup
--non-interactive` with explicit flags. Its interactive color and focus
treatment adapts to terminal width and honors `NO_COLOR`.

Setup does not and cannot activate aishe in the parent shell that launched it.
After setup, run:

```sh
aishe
```

That opens the experience saved in setup or settings. The personal profile
loads your `.zshenv`/`.zshrc`, keeps your prompt and history policy, and chains
existing Enter/Tab widgets. Its quiet right-prompt suffix shows mode and work.
Themes can display `AISHE_MODE_INDICATOR` and `AISHE_BACKGROUND_INDICATOR`.
`AISHE_PTY_PROMPT=force` explicitly opts into AIShe's full prompt.

With Clean AIShe, optional exports, aliases and completion paths belong in
`~/.aishe/leanrc`; late widget and binding changes belong in
`~/.aishe/leanrc.post`. See [Front-ends](front-ends.md) for startup order and
shortcuts. Bash offers the [documented reduced tier](bash-compatibility.md).

Subscription OAuth setup shows the explicit
`AISHE_LEGACY_OPENCODE=1 aishe` launch command; first-run setup honors that choice
for the shell it starts. The legacy hook remains available through `aishe init
zsh`. Use `/` then Tab for described commands, `/settings` for saved defaults,
or `/setup` to revisit setup. Use
`aishe auth` for keys. Existing config, credentials, history, task records, and
other state are preserved by binary/runtime upgrades. A fully annotated example
config is at [examples/config.toml](../examples/config.toml).

### Signing in without the wizard

Setup covers this, but the credential commands work on their own. API keys go
into an AWS CLI-style private credentials file through a hidden prompt:

```sh
aishe auth set anthropic
aishe auth set openai
```

OpenAI ChatGPT Plus/Pro and xAI SuperGrok subscriptions use OAuth instead:

```sh
aishe auth login openai             # device authorization is automatic over SSH
aishe auth login xai
```

Environment variables remain supported for CI and one-process overrides, and
take precedence without overwriting the saved key. For Groq, Ollama,
OpenRouter, and others, see [Providers](providers.md).

## 2. Run real commands

Anything aishe recognizes as a command runs exactly like it would in zsh:

```
~/projects/app ❯ git status
~/projects/app ❯ ls -la | grep .rs
~/projects/app ❯ for f in *.txt; do wc -l "$f"; done
```

Pipes, globs, redirection, subshells, control structures, and interactive
programs like `vim`, `ssh`, and `top` all work, because aishe hands shell lines
to your real shell.

Agent commands also use current exported `PATH`, virtual environment variables,
and ordinary exports/unsets from this shell. For example, activating a project
virtual environment before an agent request selects that environment's tools.
This does not widen the accepted workspace: a virtual environment outside it
can remain inaccessible under isolation. Aliases and functions stay in zsh;
the agent uses a fresh command shell without personal startup code. Credential
and startup-control variables are filtered, and the live snapshot is not saved
in task or audit records.

## 3. Ask in plain English

Type a request that is not a command, and the LLM proposes one:

```
~/projects/app ❯ whats eating my disk
  du -sh * | sort -rh | head
  [Enter] run now  [e] edit first  [n/Esc] cancel
```

Press Enter to run it, `e` to edit it first, or `n` to cancel. This is ask mode, the default.

In the interactive `aishe` shell the proposal is staged on your command line
instead: the first Enter puts it there so you can edit it, and the second Enter
runs it. Nothing is ever executed without a keystroke of yours.

## 4. Try the other modes

Inside the native shell:

```text
/mode allow        # grant safe suggestions for this shell
/mode agent        # grant autonomous work inside this workspace
/mode agent-host   # explicitly grant host-wide work where policy allows
```

Shift-Tab cycles modes on an empty line. With text entered it keeps reverse
completion. Legacy `suggest`, `auto`, and `yolo` names remain accepted aliases;
`aishe mode` changes defaults for new shells rather than granting a live shell.

In `allow`, the safety gate has three outcomes: a command it finds safe runs
straight away, one it flags as dangerous stops and makes you type the full word
`yes`, and one it *could not resolve* stops with a yellow "could not verify"
panel and a plain `[y/N]`. Nothing unverified ever runs on its own. See
[Safety gate](safety.md#three-outcomes).

The native agent has a separate execution **scope**. `workspace` binds commands
and built-in file tools to the accepted project, with functional bubblewrap
required on Linux; `host` grants host-wide authority. A workspace grant stays
bound to the canonical directory shown at acceptance. Moving outside that tree
requires another grant. After acceptance, agent actions within scope run without
per-action prompts; explicit file previews retain their configured approval.
New shells ask again. Protected host environments require fresh typed
confirmation and reject unattended host execution. See the
[scope boundaries](lean-hotpath.md#default-ui-and-session-state), including MCP
and macOS limits.

Or set the mode for a single session at launch:

```sh
aishe --mode agent
```

See [Modes](modes.md) for the full behavior of each.

### Run an explicit task

For a scriptable autonomous request, use the task command rather than relying
on a saved agent mode:

```sh
aishe agent --scope workspace 'inspect and fix the failing tests'
aishe agent --background --scope workspace 'update the project documentation'
aishe task list
aishe task browse
aishe task resume TASK_ID
aishe resume NATIVE_TASK_ID
```

Foreground, CLI, and background tasks share the native execution engine.
Background work runs in an isolated git worktree by default. Resume continues
the saved transcript and scope, connection/model, workspace root, and spent
allowances; a possibly started tool is not blindly executed again. Current
organization policy still applies. Live environment values are not stored in
the checkpoint.

Completion returns exit `0`; cancellation, exhausted budgets, iteration caps,
failure, and declined approval have separate nonzero exits. A completed task
means the model supplied its final answer, so review the change, recorded
checks, and unresolved items before applying it. Background tasks can also pause
for a question or specific action approval. Exact exit codes and budget and
cancellation boundaries are in
[Native agent execution](configuration.md#native-agent-execution).

### Keep track of background work

Continue using the shell while a background task runs. The clean prompt shows
a quiet badge for running work and unseen results or problems; it disappears
when there is nothing to show. Press **Ctrl-X b** to open the task browser and
return to your current command with its cursor intact. A personal shortcut
already bound to that key stays yours; `/tasks` also opens the browser.
**Needs you** marks a paused question or action approval. Open `/inbox` to answer
those requests across all projects; leaving an approval for later is the default.

Type to search, use arrows and **Enter** for details, **Ctrl-R** to refresh, and
**Tab** to include other projects. **Ctrl-V** switches between Current work,
Needs you, and Archived history. In details, **Enter** or **u** responds to a
request, **f** sends a follow-up, **e** opens recorded checks, **l** opens
activity, **t** opens the execution timeline, **p** shows read-only changes,
**?** opens searchable Task actions,
and **Esc** goes back. **Ctrl-C** closes the browser from any page. Inspect the
result, checks, unresolved items, and limits before deciding to resume, stop,
rework, apply, or discard.
Those actions ask for confirmation. Looking at a result does not apply it.
Follow-ups show **queued** until saved in the worker's transcript, then
**received**. Press **q** in details, or choose queued follow-ups in Task actions,
to edit or remove a message before delivery. Send a new follow-up once delivery
begins. Existing commands can finish before the worker reads your message.

Opening a finished result marks that exact result seen across shells. Mark it
reviewed explicitly after checking its changes and evidence. Give
it a useful name, pin work you return to, or archive finished work from **?**.
Archive keeps the result and workspace; Archived history brings it back. A new
attempt or result becomes visible again. For a scriptable response or follow-up,
see [background task controls](commands.md#background-task-controls).

For a long native conversation, press **Ctrl-X d** while it is running to queue
background continuation. Its current operation can finish before the checkpoint
transfers. Press **g** in task details to continue with its context in your
terminal. Scope, model, and spent allowances carry forward. Press **a** to review
and select files or hunks alongside checks before applying work. Open `/workflow`
to inspect a saved, parameterized task graph. See
[Agentic task workflows](agentic-workflows.md) for examples and the limits of
handoffs, check evidence, partial application, and parallel stages.

Personal themes can use `AISHE_BACKGROUND_INDICATOR`; setting
`AISHE_PERSONAL_INDICATOR=1` opts into AIShe's right-prompt suffix.

## 5. Force a route when needed

AIShe is **shell-first**: if the first word is a real binary on `PATH`, the
line runs in zsh. That is intentional so real tools keep working — and it is
the #1 source of “why didn’t the AI hear me?” confusion.

### Prefer `?` for natural language

| You type | What happens |
|----------|----------------|
| `install kubectl please` | **Shell.** `install` is `/usr/bin/install` (copy files). Often fails with `No such file or directory`. |
| `? install kubectl please` | **AI.** The `?` is stripped; the agent gets the English request. |
| `! rm -rf build` | **Shell**, safety gate skipped (dangerous by design). |
| `!!`, `!$`, `!word` | Native zsh history expansion, not AIShe's force-shell prefix. |
| `?` alone after a failed command | Ask the model to diagnose the last failure. |

`# …` remains a deprecated compatibility spelling for force-NL and is stripped
before the model runs. New workflows should use `?`; `aishe route -- '# …'`
shows the migration notice and planned compatibility window.

### Route highlight and non-color cue

| Buffer color | Meaning |
|--------------|---------|
| **Green** | Routed as a **shell** command |
| **Magenta** | Routed as **natural language** |
| **Cyan** | A recognized AIShe `/command` |

An unrecognized `/name` keeps whatever your syntax-highlighting plugin gives an
unknown command, usually red, which is accurate: it falls through to the shell
as a path and fails.

The colors are optional accelerators, not the only signal. Press **Ctrl-X ?**
in the zsh front end to print `agent` or `shell/local` for the current buffer,
or run `aishe route -- '<line>'` anywhere to get the route, stable reason, and
opposite override without submitting the input.

There is no separate “NL mode” badge on the status line. Mode glyphs are
ask `❯` / allow `»` / agent `*`. Force-NL only changes **that one line**.

### Option / Alt + Return (optional)

A force-NL key can submit the current buffer as natural language without a
`?` prefix:

- **Default (zsh):** Meta/Alt + Return (bindkey `^[^M`)
- **Mac:** that is **⌥ Option + Return**, but only if the terminal treats
  Option as Meta (see below)
- **Bash 5.x hook:** Ctrl-G; **Bash 3.2 Tier B-:** use `?`
- Override: `export AISHE_NL_KEY='^G'` (Ctrl-G) before starting aishe

If the line is **empty**, the key does nothing. Prefer **`?`** if keys are
finicky.

**Enable Option-as-Meta (so ⌥+Return works):**

| App | Setting |
|-----|---------|
| **iTerm2** | Settings → Profiles → Keys → Left/Right Option key → **Esc+** |
| **Terminal.app** | Settings → Profiles → Keyboard → **Use Option as Meta key** |
| **VS Code / Cursor** | `"terminal.integrated.macOptionIsMeta": true` |

Full keybinding detail: [Shell integration — force-NL](shell-integration.md#force-nl-and-input-prefixes).

### Why green `install` is not a bug

The built-in highlighter uses the same routing as the shell. Thus
`what --version` stays green, while `what is the capital of France?` can go
magenta. Imperatives that start with a real binary (`install …`, `find …`,
`open …`) stay **green** unless you force NL. Ambiguous phrasing cannot be
perfect; **`?` and `!` are the reliable escape hatches.**

## 6. Check your setup any time

```sh
aishe doctor --probe
```

This reports your backing shell, config path, resolved front-end, provider,
credential source, managed runtime version/hash, authenticated server, trusted
plugin/tool restrictions, credential isolation, session journals, and sandbox
state. Add `--live` for the pinned runtime/server and minimal provider feature
probes; `--json` for automation; `--fix` for safe local repairs; or `--bundle
PATH` for a redacted support bundle.

## 7. Switch accounts and models

Inside the shell:

```
/help                # task-first index
/connection          # switch account (Codex/Grok OAuth vs API, Anthropic, …)
/model               # models for the *active* account only
/status              # connection brand, model, mode, spend/plan
```

After `aishe auth login openai --profile work` or `aishe auth login xai
--profile work`, AIShe creates a connection if one is missing so `/connection`
lists **Codex - OAuth · work** or **Grok - OAuth · work** immediately. Details:
[Commands](commands.md#primary-slash-commands) and [Providers](providers.md).

## Where to go next

- [Commands and slash-commands](commands.md) for the full CLI and `/help` topics.
- [Providers](providers.md) for Anthropic, Codex/OpenAI, Grok/xAI, OAuth, and
  OpenAI-compatible endpoints.
- [Modes](modes.md) for streaming and structured output.
- [Front-ends](front-ends.md) for the zsh-PTY shell, the native hook, and `-c`.
- [Managed agent backend](managed-agent-backend.md) for runtime, sessions,
  security ownership, offline installs, and recovery.
- [Custom commands and skills](custom-commands-and-skills.md) to add your own
  `/commands`.
- [Configuration reference](configuration.md) for every setting.
- [Root README](../README.md) for marketing overview and the 60-second start.
