# Getting started

This page walks through your first session with aishe.

## 1. Set an API key

aishe reads your API key only from an environment variable, never from the config
file. Pick the provider you want and export its key:

```sh
export ANTHROPIC_API_KEY=sk-ant-...      # Anthropic
# or
export OPENAI_API_KEY=sk-...             # OpenAI (or any OpenAI-compatible key)
```

For Groq, Ollama, OpenRouter, and others, see [Providers](providers.md).

## 2. First run and the wizard

The first time you start aishe with no config present, an interactive wizard
writes `config.toml` into aishe's config directory:

```sh
aishe
```

That directory is `~/.config/aishe/` on Linux but
`~/Library/Application Support/aishe/` on macOS — aishe follows each platform's
own convention, and a file left in the wrong one is silently ignored. Run
`aishe doctor` to see the path actually in use, or set `AISHE_CONFIG_DIR` (and
`AISHE_DATA_DIR`) to pick your own. Full table:
[File locations](configuration.md#file-locations). The docs write these paths in
their Linux form for brevity.

It asks for:

- the provider (anthropic or openai-compatible),
- for an OpenAI-compatible provider, the **service** (OpenAI, Groq, OpenRouter,
  Together, Ollama, or a custom endpoint) and the **API endpoint (base URL)**,
  pre-filled from the chosen service,
- the environment variable that holds your API key,
- the model,
- the default mode.

The endpoint prompt is what lets you point at Groq, Ollama, or any other
OpenAI-compatible service instead of OpenAI; pick the service and the base URL
and model are filled in for you (editable). When aishe is not run from a
terminal (a hook, a pipe, CI), the wizard is skipped and a default config is
written instead, so it never blocks.

You can re-run any of these later with the meta commands, or edit the config file
directly. A fully annotated example config is at
[examples/config.toml](../examples/config.toml).

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

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## 7. Check your setup any time

```sh
aishe doctor
```

This reports your backing shell, config path, resolved front-end, provider, and
whether the API key is set.

## Where to go next

- [Modes](modes.md) for streaming and structured output.
- [Front-ends](front-ends.md) for the zsh-PTY shell, the native hook, and `-c`.
- [Custom commands and skills](custom-commands-and-skills.md) to add your own
  `/commands`.
- [Configuration reference](configuration.md) for every setting.
