# Getting started

This page walks through your first session with aishe.

## 1. Authenticate a provider

Aishe keeps API keys out of ordinary config using an AWS CLI-style private
credentials file. Pick the provider you want and enter its key through the
hidden prompt:

```sh
aishe auth set anthropic
# or
aishe auth set openai
```

OpenAI ChatGPT Plus/Pro and xAI SuperGrok subscriptions can use OAuth instead:

```sh
aishe auth login openai             # device authorization is automatic over SSH
# or
aishe auth login xai
```

`aishe setup` can do this step for you. Environment variables remain supported
for CI and one-process overrides and take precedence without overwriting the
saved key. For Groq, Ollama, OpenRouter, and others, see
[Providers](providers.md).

## 2. Run guided setup

Run the interactive, resumable setup before starting your first shell:

```sh
aishe setup
```

That directory is `~/.config/aishe/` on Linux but
`~/Library/Application Support/aishe/` on macOS — aishe follows each platform's
own convention, and a file left in the wrong one is silently ignored. Run
`aishe doctor` to see the path actually in use, or set `AISHE_CONFIG_DIR` (and
`AISHE_DATA_DIR`) to pick your own. Full table:
[File locations](configuration.md#file-locations). The docs write these paths in
their Linux form for brevity.

It asks for:

- existing-install discovery and organization-policy constraints,
- the backing shell and platform capabilities,
- installation and live verification of Aishe's exact managed OpenCode runtime,
- on Linux, a bubblewrap functional check and an explicit offer to install the
  package when it is missing,
- the provider/service and safety profile,
- for an OpenAI-compatible provider, the **service** (OpenAI, xAI, Groq, OpenRouter,
  Together, Ollama, or a custom endpoint) and the **API endpoint (base URL)**,
  pre-filled from the chosen service,
- a saved credential profile, hidden key entry, or environment-only workflow,
- a current model catalog from the endpoint and a validated model selection,
- per-million-token input/output prices when that exact model has no known price,
- suggest/auto/yolo behavior, workspace/host scope, and workspace network policy,
- status-line position, density, and ordered contents, and
- end-to-end backend/provider/tool/sandbox validation plus a configuration
  review before saving.

The endpoint prompt is what lets you point at Groq, Ollama, or any other
OpenAI-compatible service instead of OpenAI; pick the service and the base URL
are filled in for you (editable). After the credential step, Setup calls
`GET /v1/models`. A successful response verifies the endpoint and key without
using tokens and supplies the model picker. You can also type any model ID:
Setup checks the full returned catalog, then makes one clearly disclosed
minimal generation request only when the ID was not listed. Credential,
permission, network, and model-not-found failures stay in Setup with retry/back
choices instead of silently accepting an unverified value.

OpenCode is entirely managed by Aishe. Setup downloads the exact version pinned
by this Aishe build, verifies its size/checksum/version/notices, launches it with
private HOME/XDG directories on authenticated loopback ports, and verifies the
trusted Aishe plugin and tool restrictions. It never reuses an arbitrary
`opencode` on `PATH` and never opens a second TUI. Ordinary zsh commands remain
independent of this runtime.

On Linux, selecting isolated workspace-agent behavior requires a bubblewrap
self-test, not just the presence of a `bwrap` executable. Setup shows the exact
package-manager command and asks before sudo. If namespaces are unavailable in a
container/kernel, Setup labels that condition accurately and lets you choose a
compatible policy where organization rules allow. macOS is clearly labeled
policy-only.

Setup writes nothing until you apply the review. If interrupted, rerun `aishe
setup --resume`; use `--restart` to discard only its draft. In a pipe or CI,
setup exits instead of inventing defaults; use `aishe setup
--non-interactive` with explicit flags. Its interactive color and focus
treatment adapts to terminal width and honors `NO_COLOR`.

Setup does not and cannot activate aishe in the parent shell that launched it.
After setup, run:

```sh
aishe
```

That launches your real interactive zsh under aishe. Alternatively, install the
native hook printed by `aishe init zsh`. You can revisit settings with `aishe
settings`, rerun setup, or edit the non-secret config file directly. Use
`aishe auth` for keys. Existing config, credentials, history, task records, and
other state are preserved by binary/runtime upgrades. A fully annotated example
config is at [examples/config.toml](../examples/config.toml).

## 3. Run real commands

Anything aishe recognizes as a command runs exactly like it would in zsh:

```
~/projects/app ❯ git status
~/projects/app ❯ ls -la | grep .rs
~/projects/app ❯ for f in *.txt; do wc -l "$f"; done
```

Pipes, globs, redirection, subshells, control structures, and interactive
programs like `vim`, `ssh`, and `top` all work, because aishe hands shell lines
to your real shell.

## 4. Ask in plain English

Type a request that is not a command, and the LLM proposes one:

```
~/projects/app ❯ whats eating my disk
  du -sh * | sort -rh | head
  [Enter] run   [e] edit   [n] cancel
```

Press Enter to run it, `e` to edit it first, or `n` to cancel. This is suggest
mode, the default.

## 5. Try the other modes

```sh
aishe mode auto     # run safe commands immediately, ask about the rest
aishe mode yolo     # let the model run a multi-step task on its own
```

In `auto`, the safety gate has three outcomes: a command it finds safe runs
straight away, one it flags as dangerous stops and makes you type the full word
`yes`, and one it *could not resolve* stops with a yellow "could not verify"
panel and a plain `[y/N]`. Nothing unverified ever runs on its own. See
[Safety gate](safety.md#three-outcomes).

The managed agent adds a separate execution **scope**. `workspace` confines
agent effects to the selected project (with bubblewrap on supported Linux
systems); `host` grants host-wide agent authority. Entering yolo asks once for
the scope in each new shell. After acceptance, yolo runs without per-action
approval prompts; a new shell asks again because acceptance is never persisted.
Use `aishe scope workspace|host` and `aishe network allow|deny` to change the
next turn's selection.

Or set the mode for a single session at launch:

```sh
aishe --mode yolo
```

See [Modes](modes.md) for the full behavior of each.

## 6. Force a route when needed

Sometimes a request happens to start with a real command name (for example
"find all large files"), so it would run as a command. Use the prefixes:

- `?<text>` forces natural-language: `?find all large files`
- `!<cmd>` forces shell and skips the safety gate: `!rm -rf build`

The built-in highlighter uses the same full-line routing grammar. Thus
`what --version` stays green as a command, while `what is the capital of France?`
changes to the natural-language color even if a binary named `what` exists.
Ambiguous phrasing cannot be perfect, so `?` and `!` remain the explicit escape
hatches.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## 7. Check your setup any time

```sh
aishe doctor --probe
```

This reports your backing shell, config path, resolved front-end, provider,
credential source, managed runtime version/hash, authenticated server, trusted
plugin/tool restrictions, credential isolation, session journals, and sandbox
state. Add `--live` for the pinned runtime/server and minimal provider feature
probes; `--json` for automation; `--fix` for safe local repairs; or `--bundle
PATH` for a redacted support bundle.

## Where to go next

- [Modes](modes.md) for streaming and structured output.
- [Front-ends](front-ends.md) for the zsh-PTY shell, the native hook, and `-c`.
- [Managed agent backend](managed-agent-backend.md) for runtime, sessions,
  security ownership, offline installs, and recovery.
- [Custom commands and skills](custom-commands-and-skills.md) to add your own
  `/commands`.
- [Configuration reference](configuration.md) for every setting.
