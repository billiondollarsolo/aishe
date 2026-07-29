# Configuration reference

aishe reads `config.toml` from its platform config directory (see
[File locations](#file-locations) — it is **not** `~/.config` on macOS). A fully
annotated copy you can paste in is at
[examples/config.toml](../examples/config.toml). Every field has a default, so a
minimal config is valid.

Use `aishe setup` for guided configuration or `aishe settings` for the
interactive section editor. The settings hub shows the provenance of each
effective value and stages changes until you apply them. A few
[meta commands](commands.md) also persist individual choices.

## File locations

aishe follows each platform's own convention, so the directories differ between
Linux and macOS. This matters: a config file, custom command, or skill placed in
the wrong directory is not read and produces no error — it simply never appears.

| Directory | Linux | macOS |
|-----------|-------|-------|
| Config (`<config>` below) | `~/.config/aishe/` | `~/Library/Application Support/aishe/` |
| Data (`<data>` below) | `~/.local/share/aishe/` | `~/Library/Application Support/aishe/` |

Inside them:

- Config file: `<config>/config.toml`
- Custom commands: `<config>/commands/` and `<project>/.aishe/commands/`
- Skills: `<config>/skills/` and `<project>/.aishe/skills/`
- Startup file: `~/.aishrc` and `<config>/aishrc`
- Timestamped shell history: `<data>/history.ext`
- Audit log: `<data>/audit.jsonl` (override with `AISHE_LOG_FILE`)
- Undo journal: `<data>/undo.jsonl` (override with `AISHE_UNDO_JOURNAL`)
- Semantic-history index: `<data>/history.vec`
- Durable AI tasks: `<data>/tasks/*.json` (private and redacted; stateless
  reasoning resumes may retain opaque encrypted provider continuation data)
- Provider capability cache: `<data>/capabilities/*.json`
- Resumable setup draft: `<data>/setup-draft.json`

**Run `aishe doctor` to see the paths actually resolved on your machine** — it
prints the config and history locations rather than leaving you to guess.

On Linux `$XDG_CONFIG_HOME` and `$XDG_DATA_HOME` are honored. macOS ignores them
by design; use `AISHE_CONFIG_DIR` / `AISHE_DATA_DIR` to relocate the directories
on any platform (see [Environment variables](#environment-variables)).

The rest of the documentation writes these paths in their Linux form for
brevity. Read `~/.config/aishe/...` as `<config>/...` and
`~/.local/share/aishe/...` as `<data>/...`.

## `[aishe]` section

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `safety_profile` | string | `custom` | Named settings bundle: `conservative`, `balanced`, `autonomous`, or `custom`. |
| `mode` | string | `suggest` | Interaction mode: `suggest`, `auto`, or `yolo`. |
| `provider` | string | `anthropic` | Which provider block to use: `anthropic` or `openai`. |
| `yolo_confirm_dangerous` | bool | `true` | In yolo, confirm commands the safety gate flags. Honored only when `yolo_confirm` is unset. |
| `yolo_confirm` | string | `dangerous` | When the yolo loop confirms a command: `never`, `dangerous`, `writes`, or `all`. See [Safety gate](safety.md). |
| `yolo_sandbox` | bool | `false` | Policy sandbox: refuse yolo commands that reach the network or write outside the working tree. Toggle at the aishe prompt with `sandbox on`/`off`. |
| `max_yolo_iterations` | integer | `10` | Maximum tool-use steps for one yolo request. |
| `yolo_plan` | bool | `false` | Plan-first dry run: the model shows its intended steps and you approve before the loop runs (interactive only). Toggle at the aishe prompt with `plan on`/`off`. |
| `project_context` | bool | `true` | Include a per-project `.aishe/context.md` (at or above the cwd) in the model context. See [Per-project context](project-context.md). |
| `file_tools` | bool | `true` | Offer the built-in `read_file`/`write_file`/`edit_file`/`list_dir` tools to yolo. |
| `web_tool` | bool | `true` | Offer the built-in `fetch_url` tool to yolo (read web pages/docs; HTML stripped to text, size-capped). |
| `auto_pushd` | bool | `false` | zsh `AUTO_PUSHD`: every `cd` pushes the previous dir (`cd -N`/`cd +N`, `dirs -v`). |
| `cdpath` | array | `[]` | Extra base dirs searched by `cd <name>` (`CDPATH`); falls back to `$CDPATH`. |
| `share_history` | bool | `true` | Share Aishe's timestamped history across concurrent and future sessions (zsh `SHARE_HISTORY` when Aishe supplies the native-history fallback); off makes Aishe history per-session. |
| `structured` | string | `schema` | Suggest output format: `schema`, `json`, or `prompt`. |
| `stream` | bool | `false` | Stream answers token-by-token (suggest and auto). |
| `reasoning_effort` | string | `auto` | Provider reasoning effort; `auto` lets the model/endpoint choose. |
| `failure_hints` | bool | `true` | Show one concise recovery hint after an interactive command fails. |
| `context_exclude` | array | `[]` | Optional context section IDs to omit. Manage with `aishe context`. |
| `show_usage` | bool | `true` | Record and display model-call usage in the interactive session. |
| `status_line` | bool | `true` | Enable the branded prompt's live status display. |
| `status_line_position` | string | `right` | Status placement: `right`, `below`, or `off`. |
| `status_line_items` | array | `["model","mode","session_cost","requests"]` | Ordered fields; also supports `last_tokens`, `last_cost`, and `session_tokens`. |
| `budget_usd` | float | `0.0` | Stop calling the model past this session cost. `0` = unlimited. |
| `memory` | bool | `true` | Remember recent natural-language turns so follow-ups have context. Clear at the aishe prompt with `reset`. |
| `cache` | bool | `true` | Cache identical suggest-mode responses briefly so repeats are instant and free. Toggle at the aishe prompt with `cache on`/`off`. |
| `cache_ttl_secs` | integer | `300` | How long a cached response stays valid, in seconds. |
| `redact_secrets` | bool | `true` | Scrub likely secrets from the context block sent to the model. See [Logging and privacy](logging.md). |

The toggles named "at the aishe prompt" above (`sandbox`, `plan`, `cache`,
`reset`, and also `rehash`) are **prompt-only meta commands**, not `aishe`
subcommands: type them inside the interactive shell (bare, or with a leading `/`
as `/sandbox`). Running `aishe sandbox` from a terminal fails with
`error: unrecognized subcommand`. See
[Commands: prompt-only meta commands](commands.md#prompt-only-meta-commands).

## `[named_dirs]` section (optional)

Named directories for `~name` expansion in `cd` (zsh hashed dirs):

```toml
[named_dirs]
proj = "/home/me/projects"
dl = "/home/me/Downloads"
```

Then `cd ~proj` and `cd ~proj/app` work.

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
| `transport` | string | `auto`, `responses`, or `chat`. Auto uses Responses for official OpenAI and Chat Completions for compatible endpoints. |
| `auth_required` | bool | Optional explicit auth policy. When absent, loopback endpoints need no key; non-loopback endpoints do. |

Defaults:

```toml
[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"
transport = "auto"
auth_required = true

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
transport = "auto"
auth_required = true
```

See [Providers](providers.md) for Groq, Ollama, and others.

## `[pricing."<model>"]` (optional)

Per-model price overrides for cost estimates, in USD per 1M tokens. Exact keys
win; legacy substring overrides and then the built-in table are compatibility
fallbacks. Prefer `aishe price set MODEL --input PRICE --output PRICE`, which
writes an exact override. Setup prompts when it cannot price the selected model.

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
  default on every OS. `aishe` reads `$AISHE_CONFIG_DIR/aishe/config.toml`, and
  looks for `commands/`, `skills/`, and `aishrc` alongside it.
- `AISHE_DATA_DIR`: base directory for state — history, audit log, trust store,
  and the undo journal — overriding the platform default on every OS
  (`$AISHE_DATA_DIR/aishe/…`).

Both take a *base* directory; aishe appends `aishe/` itself. They are the
simplest way to keep a scratch or per-project setup away from your real one:

```sh
AISHE_CONFIG_DIR=/tmp/try AISHE_DATA_DIR=/tmp/try aishe doctor
```

## Command-line flags

```
aishe [--mode suggest|auto|yolo] [--model NAME] [--provider anthropic|openai] [-c "INPUT"]

aishe zsh                 launch your real zsh under aishe (zsh-PTY), explicitly
aishe setup               guided/resumable setup (`--non-interactive` for CI)
aishe settings            transactional interactive settings editor
aishe init <zsh|bash>     print a shell integration snippet
aishe doctor              diagnostics; add --probe/--live/--json/--fix/--bundle
```

Flags override config for that session only. `-c "INPUT"` runs a single input
non-interactively and exits.

## Recovery

If the config file is malformed, aishe reports the exact error and refuses to
silently replace it with defaults. Repair it with `aishe settings`, restore a
`.bak` file, or use `aishe setup --restart` to discard only a setup draft.

Schema upgrades are automatic and transactional: aishe writes a private backup,
then atomically writes the migrated v2 config. A pre-rename
`llmsh/config.toml` in the same config directory is migrated on first run.
Installers and package upgrades replace the binary only; they do not delete the
config, history, tasks, trust store, or other user data.
