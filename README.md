# aishe

A natural-language-aware shell. `aishe` behaves like zsh for real commands, and
treats anything that is not a command as a natural-language request handled by an
LLM. The model either **suggests** a command for you to confirm, or **runs
autonomously** in a tool-use loop.

```
~/projects/app ❯ git status            # runs exactly like zsh
~/projects/app ❯ whats eating my disk  # LLM suggests: du -sh * | sort -rh | head
```

aishe does not reimplement a shell grammar. It owns the prompt and input loop and
delegates execution of shell lines to `zsh -c` (falling back to `bash -c`), so
pipes, globs, redirection, subshells, and interactive programs (vim, ssh, top)
all work unmodified.

## Contents

- [Install](#install)
- [Quickstart](#quickstart)
- [Modes](#modes)
- [Front-ends](#front-ends)
- [Providers](#providers)
- [Meta commands and slash-commands](#meta-commands-and-slash-commands)
- [Custom commands and skills](#custom-commands-and-skills)
- [Token usage and cost](#token-usage-and-cost)
- [Safety gate](#safety-gate)
- [Prompt and theming](#prompt-and-theming)
- [Startup file (.aishrc)](#startup-file-aishrc)
- [Configuration](#configuration)
- [Documentation](#documentation)
- [Development](#development)

## Install

Requires Rust 1.80 or newer and `zsh` or `bash` on your `PATH`. Targets macOS
(arm64 / x86_64) and Linux (x86_64 / arm64).

For now aishe is built from source with Cargo. Prebuilt packages for common
systems (Homebrew, a Linux package or tarball, cargo-binstall) are planned but
not available yet.

```sh
cargo install --path .
# or, from a checkout:
cargo build --release   # binary at target/release/aishe
```

See [docs/installation.md](docs/installation.md) for the full guide, including
uninstalling and the planned prebuilt packages.

## Quickstart

1. Set your API key in the environment. It is never written to the config file.

   ```sh
   export ANTHROPIC_API_KEY=sk-ant-...      # or OPENAI_API_KEY=...
   ```

2. Start aishe. On first run an interactive wizard writes
   `~/.config/aishe/config.toml` (provider, API key env var, model, default mode).

   ```sh
   aishe
   ```

3. Type real commands as usual, or type a request in plain English.

Not sure your setup is right? Run `aishe doctor` for a quick check of your shell,
config, resolved front-end, provider, and API key.

A full walkthrough is in [docs/getting-started.md](docs/getting-started.md).

## Modes

| Mode      | Glyph | Behavior                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | Default. The LLM proposes a command; you confirm with `[Enter] / [e]dit / [n]`. |
| `auto`    |  `»`  | Commands the safety gate deems safe run immediately; dangerous ones still require typing `yes`. |
| `yolo`    |  `⚡`  | Agentic loop: the model runs commands, reads output, and iterates until done. |

Switch at any time with `aishe mode auto`, or start with `aishe --mode yolo`. See
[docs/modes.md](docs/modes.md) for streaming, structured output, and input
prefixes.

### Input prefixes

- `?<text>` forces natural-language, for example `?how do I find large files`
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`

After a command fails, type `?` on the next line to ask the LLM to diagnose the
error.

## Front-ends

There are three ways to use aishe, in increasing order of real-zsh fidelity.
Details in [docs/front-ends.md](docs/front-ends.md).

1. **zsh-PTY (default when zsh is present).** aishe runs your real interactive
   zsh inside a pseudo-terminal, so your full `~/.zshrc` and every plugin you
   already use (zsh-autosuggestions, zsh-syntax-highlighting, fzf-tab,
   powerlevel10k, oh-my-zsh, completions) work unmodified. A
   `command_not_found_handler` routes natural language to the LLM.

   ```sh
   aishe              # "auto" front-end uses zsh-PTY when zsh is on $PATH
   aishe zsh          # force it (same as aishe --pty, or front_end = "zsh-pty")
   aishe --no-pty     # force the built-in reedline editor instead
   ```

2. **Built-in reedline editor.** A self-contained line editor with
   context-aware tab completion, history autosuggestions, `Ctrl-R` history
   search, history expansion, multi-line continuation, syntax highlighting,
   autocd, a directory stack, and emacs or vi keymaps.

3. **Native zsh/bash hook.** Add `eval "$(aishe init zsh)"` to your `~/.zshrc`
   (or `~/.bashrc` with `init bash`) to keep your own shell and editor while
   routing unknown input to aishe. See
   [docs/shell-integration.md](docs/shell-integration.md).

## Providers

aishe talks to two provider shapes: the **Anthropic Messages API** and any
**OpenAI-compatible Chat Completions API**. The OpenAI shape covers OpenAI,
Groq, Ollama, OpenRouter, Together, and similar services through `base_url`.

```toml
[providers.openai]
base_url = "https://api.groq.com/openai"   # or http://localhost:11434 for Ollama
api_key_env = "GROQ_API_KEY"
model = "openai/gpt-oss-120b"
```

API keys are read only from the named environment variable, never stored in the
config. See [docs/providers.md](docs/providers.md) for per-provider setup.

## Meta commands and slash-commands

```
aishe mode [suggest|auto|yolo]    show or set the interaction mode
aishe model [NAME]                show or set the model
aishe provider [anthropic|openai] show or set the provider
aishe editor [emacs|vi]           show or set the line-editor keymap
aishe frontend [auto|reedline|zsh-pty]  show or set the front-end
aishe stream [on|off]             show or toggle token streaming
aishe structured [schema|json|prompt]   output-format strategy (default: schema)
aishe theme [PRESET]              show or set the color preset
aishe usage                       session token and cost usage
aishe reset                       clear conversation memory
aishe commands                    list custom slash-commands
aishe skills                      list model-invoked skills
aishe config                      print the active config
aishe rehash                      rebuild the command cache
aishe help                        show help
```

Each meta command also works as a slash-command (Claude Code style), for example
`/mode auto`, `/config`, `/help`. They are tab-completable, and `/`-prefixed
paths like `/usr/bin/x` still run normally. Full reference in
[docs/commands.md](docs/commands.md).

## Conversation memory

In the interactive REPL, aishe remembers recent natural-language turns so
follow-ups have context: after "create alpha.txt containing apple", a follow-up
"now do the same for beta.txt" knows what "the same" means. Memory lives only for
the session (never written to disk), is capped in size, and is cleared with
`aishe reset`. Turn it off with `memory = false`. See [docs/modes.md](docs/modes.md).

## Custom commands and skills

Drop Markdown files into `~/.config/aishe/commands/` (user) or
`<project>/.aishe/commands/` (project, overrides user) to add your own
`/commands`. The file name is the command (`bigfiles.md` becomes `/bigfiles`).

```md
---
description: Suggest a command to find the biggest files
mode: suggest            # suggest | auto | yolo (NL commands); default = current mode
# shell: true            # run the body as a shell command instead of an NL request
---
Show the 10 largest files under $ARGUMENTS, human-readable, largest first.
```

**Skills** are the model-invoked counterpart. Put skill files in
`~/.config/aishe/skills/` as `<name>/SKILL.md`. In yolo mode aishe advertises
each skill's `name` and `description`; when your request matches, the model pulls
the skill's full instructions into context on demand.

The formats match Claude Code, so real Agent Skills from
[anthropics/skills](https://github.com/anthropics/skills) and slash commands from
collections like [wshobson/commands](https://github.com/wshobson/commands) drop
in unchanged. Ready-made examples live in [examples/commands/](examples/commands/)
and [examples/skills/](examples/skills/). Full guide:
[docs/custom-commands-and-skills.md](docs/custom-commands-and-skills.md).

## Token usage and cost

aishe meters every model call. After each interaction it prints a dim
`N in / N out / N req / ~$cost` line (disable with `show_usage = false`), and
`aishe usage` shows the session total. Costs come from a built-in price table
(USD per 1M tokens) that you can override per model in `[pricing]`. Set
`budget_usd` to stop calling the model once the estimated session cost reaches a
limit.

```toml
[aishe]
budget_usd = 0.50          # stop past ~$0.50 per session (0 = unlimited)

[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.60
```

More in [docs/usage-and-cost.md](docs/usage-and-cost.md).

## Safety gate

Before running an LLM-proposed command (in suggest and auto, and for yolo tool
calls when `yolo_confirm_dangerous = true`), aishe screens for irreversible
operations: `rm -rf`, `dd of=/dev/...`, `mkfs`, fork bombs, `curl ... | sh`,
`git push --force` to main, `shutdown` and `reboot`, and more. Dangerous commands
print a red panel and require you to type the full word `yes`.

The gate is path-aware for `rm -rf`: an in-tree relative path like
`rm -rf node_modules` runs without fuss, while absolute, home, variable, glob, or
escaping targets are flagged. The gate does not apply to commands you type
yourself or to `!`-forced lines. Details in [docs/safety.md](docs/safety.md).

## Prompt and theming

The reedline front-end supports a custom left prompt (`prompt_format` with
`{cwd}`, `{mode}`, `{model}`, `{exit}`), a right prompt with `model and mode` and
a git branch segment, and color themes. Pick a preset (`default`, `vivid`,
`mono`, `nord`, `gruvbox`) or override individual roles.

```toml
[theme]
preset = "nord"
cwd = "bright-cyan"
```

See [docs/prompt-and-theming.md](docs/prompt-and-theming.md).

## Startup file (.aishrc)

aishe sources `~/.aishrc` and `~/.config/aishe/aishrc` (in that order) into every
delegated command, so aliases, functions, and exports you define there are
available everywhere and recognized at the prompt.

```sh
# ~/.aishrc
alias gs='git status'
alias ll='ls -lah'
export EDITOR=nvim
gco() { git checkout "$@"; }
```

A ready-to-copy example is at [examples/aishrc](examples/aishrc).

## Configuration

The config file lives at `~/.config/aishe/config.toml`. A fully annotated example
is at [examples/config.toml](examples/config.toml), and every field is documented
in [docs/configuration.md](docs/configuration.md).

```toml
[aishe]
mode = "suggest"               # suggest | auto | yolo
provider = "anthropic"         # anthropic | openai
front_end = "auto"             # auto | reedline | zsh-pty
edit_mode = "emacs"            # emacs | vi
structured = "schema"          # schema | json | prompt
stream = false
show_usage = true
budget_usd = 0.0               # 0 = unlimited

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
```

## Documentation

The [docs/](docs/) directory has the full user guide:

- [Installation](docs/installation.md)
- [Getting started](docs/getting-started.md)
- [Modes](docs/modes.md)
- [Front-ends](docs/front-ends.md)
- [Providers](docs/providers.md)
- [Configuration reference](docs/configuration.md)
- [Commands and slash-commands](docs/commands.md)
- [Custom commands and skills](docs/custom-commands-and-skills.md)
- [Token usage and cost](docs/usage-and-cost.md)
- [Safety gate](docs/safety.md)
- [Prompt and theming](docs/prompt-and-theming.md)
- [Shell integration and .aishrc](docs/shell-integration.md)
- [Troubleshooting](docs/troubleshooting.md)
- [Roadmap](docs/ROADMAP.md)

## Development

```sh
cargo build
cargo test            # unit and integration (integration tests spawn a real shell)
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# end-to-end validation harness (no API key needed for the deterministic suites)
cargo build --release && python3 tests/admin_validation.py
```

See [docs/development.md](docs/development.md) for the test layout and how the
validation harness works.

## License

See [LICENSE](LICENSE).
