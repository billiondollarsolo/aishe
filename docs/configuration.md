# Configuration reference

aishe reads `~/.config/aishe/config.toml` (or `$XDG_CONFIG_HOME/aishe/config.toml`).
A fully annotated copy you can paste in is at
[examples/config.toml](../examples/config.toml). Every field has a default, so a
minimal config is valid.

You can change most settings at runtime with the [meta commands](commands.md),
which persist your choice back to the file.

## File locations

- Config: `~/.config/aishe/config.toml`
- History (reedline): `~/.local/share/aishe/history`
- Custom commands: `~/.config/aishe/commands/` and `<project>/.aishe/commands/`
- Skills: `~/.config/aishe/skills/` and `<project>/.aishe/skills/`
- Startup file: `~/.aishrc` and `~/.config/aishe/aishrc`

## `[aishe]` section

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `mode` | string | `suggest` | Interaction mode: `suggest`, `auto`, or `yolo`. |
| `provider` | string | `anthropic` | Which provider block to use: `anthropic` or `openai`. |
| `front_end` | string | `auto` | Input loop: `auto`, `reedline`, or `zsh-pty`. |
| `edit_mode` | string | `emacs` | reedline keymap: `emacs` or `vi`. |
| `yolo_confirm_dangerous` | bool | `true` | In yolo, confirm commands the safety gate flags. |
| `max_yolo_iterations` | integer | `10` | Maximum tool-use steps for one yolo request. |
| `show_right_prompt` | bool | `true` | Show "model and mode" on the right (reedline). |
| `git_prompt` | bool | `true` | Show a git branch segment in the right prompt (reedline). |
| `prompt_format` | string | unset | Custom left prompt. Placeholders: `{cwd}`, `{mode}`, `{model}`, `{exit}`. |
| `structured` | string | `schema` | Suggest output format: `schema`, `json`, or `prompt`. |
| `stream` | bool | `false` | Stream answers token-by-token in the REPL (suggest and auto). |
| `show_usage` | bool | `true` | Print a per-session token and cost line after each interaction. |
| `budget_usd` | float | `0.0` | Stop calling the model past this session cost. `0` = unlimited. |
| `memory` | bool | `true` | Remember recent natural-language turns in the REPL so follow-ups have context. Clear with `aishe reset`. |

## `[providers.anthropic]` and `[providers.openai]`

| Field | Type | Meaning |
|-------|------|---------|
| `base_url` | string | API root. For OpenAI-compatible services, point this at the service. |
| `api_key_env` | string | Name of the environment variable that holds the API key. |
| `model` | string | Model identifier sent with each request. |

Defaults:

```toml
[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
```

See [Providers](providers.md) for Groq, Ollama, and others.

## `[pricing."<model>"]` (optional)

Per-model price overrides for cost estimates, in USD per 1M tokens. Keys are
matched by exact model name first, then by substring, then fall back to a
built-in table. Only needed when a model is missing from the table or priced
differently for you.

```toml
[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.60
```

See [Token usage and cost](usage-and-cost.md).

## `[theme]` (optional)

Colors for the reedline prompt and syntax highlighter. Pick a preset and override
any role. Colors may be names (`red`, `bright-green`), a palette index (`0`-`255`),
or hex (`#ff8800`).

```toml
[theme]
preset = "nord"     # default | vivid | mono | nord | gruvbox
cwd = "bright-cyan"
known_cmd = "#98c379"
```

Roles: `cwd`, `glyph_ok`, `glyph_err`, `right_prompt`, `known_cmd`,
`unknown_cmd`, `flag`, `string`, `operator`, `path`, `assignment`, `sigil_nl`,
`sigil_shell`. See [Prompt and theming](prompt-and-theming.md).

## Environment variables

- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or whatever `api_key_env` names: your
  API key.
- `AISHE_MODE`: mode used by the native shell hook (`suggest`, `auto`, `yolo`).
- `AISHE_NL_KEY`: override the force-NL keybinding for the zsh hook (a `bindkey`
  sequence, for example `^o`).
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME`: respected for config and history locations.

## Command-line flags

```
aishe [--mode suggest|auto|yolo] [--model NAME] [--provider anthropic|openai]
      [--pty | --no-pty] [-c "INPUT"]

aishe init <zsh|bash>     print a shell integration snippet
aishe zsh                 launch your real zsh under aishe (zsh-PTY)
aishe doctor              environment check
```

Flags override config for that session only. `-c "INPUT"` runs a single input
non-interactively and exits.

## Recovery

If the config file is malformed, aishe reports the problem and falls back to
defaults rather than refusing to start. A pre-rename `~/.config/llmsh/config.toml`
is migrated automatically on first run.
