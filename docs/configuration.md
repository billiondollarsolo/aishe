# Configuration reference

aishe reads `~/.config/aishe/config.toml` (or `$XDG_CONFIG_HOME/aishe/config.toml`).
A fully annotated copy you can paste in is at
[examples/config.toml](../examples/config.toml). Every field has a default, so a
minimal config is valid.

You can change most settings at runtime with the [meta commands](commands.md),
which persist your choice back to the file.

## File locations

- Config: `~/.config/aishe/config.toml`
- History (the `history` builtin): `~/.local/share/aishe/history`
- Custom commands: `~/.config/aishe/commands/` and `<project>/.aishe/commands/`
- Skills: `~/.config/aishe/skills/` and `<project>/.aishe/skills/`
- Startup file: `~/.aishrc` and `~/.config/aishe/aishrc`

## `[aishe]` section

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `mode` | string | `suggest` | Interaction mode: `suggest`, `auto`, or `yolo`. |
| `provider` | string | `anthropic` | Which provider block to use: `anthropic` or `openai`. |
| `yolo_confirm_dangerous` | bool | `true` | In yolo, confirm commands the safety gate flags. Honored only when `yolo_confirm` is unset. |
| `yolo_confirm` | string | `dangerous` | When the yolo loop confirms a command: `never`, `dangerous`, `writes`, or `all`. See [Safety gate](safety.md). |
| `yolo_sandbox` | bool | `false` | Policy sandbox: refuse yolo commands that reach the network or write outside the working tree. Toggle with `aishe sandbox`. |
| `max_yolo_iterations` | integer | `10` | Maximum tool-use steps for one yolo request. |
| `yolo_plan` | bool | `false` | Plan-first dry run: the model shows its intended steps and you approve before the loop runs (interactive only). Toggle with `aishe plan`. |
| `project_context` | bool | `true` | Include a per-project `.aishe/context.md` (at or above the cwd) in the model context. See [Per-project context](project-context.md). |
| `file_tools` | bool | `true` | Offer the built-in `read_file`/`write_file`/`edit_file`/`list_dir` tools to yolo. |
| `web_tool` | bool | `true` | Offer the built-in `fetch_url` tool to yolo (read web pages/docs; HTML stripped to text, size-capped). |
| `auto_pushd` | bool | `false` | zsh `AUTO_PUSHD`: every `cd` pushes the previous dir (`cd -N`/`cd +N`, `dirs -v`). |
| `cdpath` | array | `[]` | Extra base dirs searched by `cd <name>` (`CDPATH`); falls back to `$CDPATH`. |
| `share_history` | bool | `true` | Share one timestamped history across sessions (zsh `SHARE_HISTORY`); off makes history per-session. Backs the `history` builtin. |

## `[named_dirs]` section (optional)

Named directories for `~name` expansion in `cd` (zsh hashed dirs):

```toml
[named_dirs]
proj = "/home/me/projects"
dl = "/home/me/Downloads"
```

Then `cd ~proj` and `cd ~proj/app` work.
| `structured` | string | `schema` | Suggest output format: `schema`, `json`, or `prompt`. |
| `stream` | bool | `false` | Stream answers token-by-token (suggest and auto). |
| `show_usage` | bool | `true` | Print a per-session token and cost line after each interaction. |
| `budget_usd` | float | `0.0` | Stop calling the model past this session cost. `0` = unlimited. |
| `memory` | bool | `true` | Remember recent natural-language turns so follow-ups have context. Clear with `aishe reset`. |
| `cache` | bool | `true` | Cache identical suggest-mode responses briefly so repeats are instant and free. Toggle with `aishe cache`. |
| `cache_ttl_secs` | integer | `300` | How long a cached response stays valid, in seconds. |
| `redact_secrets` | bool | `true` | Scrub likely secrets from the context block sent to the model. See [Logging and privacy](logging.md). |

## `[logging]` section (optional)

Audit logging of AI calls, responses, and AI-initiated actions. Off by default.

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `enabled` | bool | `false` | Write a JSONL audit log. Also enableable with `AISHE_LOG=1`. |
| `file` | string | unset | Log path. Default `$XDG_DATA_HOME/aishe/audit.jsonl`. Override with `AISHE_LOG_FILE`. |
| `redact` | bool | `true` | Scrub secrets from logged text. |

See [Logging and privacy](logging.md) for the event shapes and examples.

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

## `[mcp_servers]` section (optional)

Model Context Protocol servers whose tools are offered to the yolo loop, keyed by
a short name used to namespace them (`mcp__<name>__<tool>`):

```toml
[mcp_servers.filesystem]            # stdio server
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/me/projects"]
# env = { KEY = "value" }   # extra environment for the server process
# enabled = false           # keep configured but turned off (default true)

[mcp_servers.remote]                # HTTP server (Streamable HTTP)
url = "https://mcp.example.com/mcp"
# headers = { Authorization = "Bearer ..." }
```

Per-server keys: `command`/`args`/`env` (stdio), `url`/`headers` (HTTP), and
`enabled`. A server with a `url` connects over HTTP; otherwise it is a stdio
server launched from `command`. List connected tools with `aishe mcp`. See
[MCP servers](mcp.md).

## Environment variables

- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or whatever `api_key_env` names: your
  API key.
- `AISHE_MODE`: mode used by the native shell hook (`suggest`, `auto`, `yolo`).
- `AISHE_NL_KEY`: override the force-NL keybinding for the zsh hook (a `bindkey`
  sequence, for example `^o`).
- `AISHE_MODE_KEY`: override the mode-cycle keybinding for the zsh hook (a
  `bindkey` sequence; default `^[[Z`, Shift-Tab).
- `XDG_CONFIG_HOME`, `XDG_DATA_HOME`: respected for config and history locations
  **on Linux**. macOS follows the platform convention
  (`~/Library/Application Support`) and ignores them — use the two variables
  below if you need to relocate those directories there.
- `AISHE_CONFIG_DIR`: base directory for the config, overriding the platform
  default on every OS. `aishe` reads `$AISHE_CONFIG_DIR/aishe/config.toml`.
- `AISHE_DATA_DIR`: base directory for state — history, audit log, trust store,
  and the undo journal — overriding the platform default on every OS.

## Command-line flags

```
aishe [--mode suggest|auto|yolo] [--model NAME] [--provider anthropic|openai] [-c "INPUT"]

aishe zsh                 launch the interactive zsh-PTY shell explicitly
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
