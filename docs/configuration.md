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
- Shared credentials: `<config>/credentials.toml` (private mode `0600`)
- Custom commands: `<config>/commands/` and `<project>/.aishe/commands/`
- Skills: `<config>/skills/` and `<project>/.aishe/skills/`
- Startup file: `~/.aishrc` and `<config>/aishrc`
- Timestamped shell history: `<data>/history.ext`
- Audit log: `<data>/audit.jsonl` (override with `AISHE_LOG_FILE`)
- Undo journal: `<data>/undo.jsonl` (override with `AISHE_UNDO_JOURNAL`)
- Semantic-history index: `<data>/history.vec`
- Durable AI tasks: `<data>/tasks/*.json` (private and redacted; stateless
  reasoning resumes may retain opaque encrypted provider continuation data)
- Managed runtime: `<data>/runtime/opencode/<version>/`
- Isolated OAuth roots: `<data>/backend/opencode/profiles/<provider>/<profile>/`
- Connection runtime roots: `<data>/backend/opencode/profiles/connections/<safe-id>/`
- Supervisor pool state: `<data>/backend/instances/<safe-id>/`
- Managed session map: `<data>/backend/sessions/mappings.json`
- Tool idempotency/usage journal: `<data>/backend/journal/tool-calls.json`
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
| `connection` | string | `anthropic` | Durable default named connection ID. `/connection` switches account for this shell unless `d` or `--default` is used; `/model` changes only the model on the active connection. |
| `connection_fallback` | string | active connection | Named compatibility fallback connection. |
| `provider` | string | `anthropic` | Which provider block to use: `anthropic` or `openai`. |
| `provider_fallback` | array | `[]` | Native compatibility-provider fallback chain. Managed turns do not start a second provider request after prompt admission or any effect. |
| `yolo_confirm_dangerous` | bool | `true` | Deprecated native compatibility behavior; managed yolo uses one per-shell scope acceptance. |
| `yolo_confirm` | string | `dangerous` | Native compatibility confirmation tier: `never`, `dangerous`, `writes`, or `all`. Managed yolo has no per-action prompts after scope acceptance. |
| `yolo_sandbox` | bool | `false` | Policy sandbox: refuse yolo commands that reach the network or write outside the working tree. Toggle at the aishe prompt with `sandbox on`/`off`. |
| `max_yolo_iterations` | integer | `10` | Maximum tool-use steps for one yolo request. |
| `yolo_plan` | bool | `false` | Plan-first dry run: the model shows its intended steps and you approve before the loop runs (interactive only). Toggle at the aishe prompt with `plan on`/`off`. |
| `project_context` | bool | `true` | Include a per-project `.aishe/context.md` (at or above the cwd) in the model context. See [Per-project context](project-context.md). |
| `file_tools` | bool | `true` | Offer the built-in `read_file`/`write_file`/`edit_file`/`list_dir` tools to yolo. |
| `web_tool` | bool | `true` | Offer the built-in `fetch_url` tool to yolo (read web pages/docs; HTML stripped to text, size-capped). |
| `auto_pushd` | bool | `false` | zsh `AUTO_PUSHD`: every `cd` pushes the previous dir (`cd -N`/`cd +N`, `dirs -v`). |
| `cdpath` | array | `[]` | Extra base dirs searched by `cd <name>` (`CDPATH`); falls back to `$CDPATH`. |
| `share_history` | bool | `true` | Share AIShe's timestamped history across concurrent and future sessions (zsh `SHARE_HISTORY` when AIShe supplies the native-history fallback); off makes AIShe history per-session. |
| `structured` | string | `schema` | Suggest output format: `schema`, `json`, or `prompt`. |
| `stream` | bool | `false` | Stream answers token-by-token (suggest and auto). |
| `hook_timeout_secs` | integer | `60` | Maximum wait (1–600 seconds) for a prompt-blocking native shell hook. Explicit `aishe suggest` calls wait through the provider's retry policy and are not signal-truncated; exhausted provider failures return exit 1. |
| `reasoning_effort` | string | `auto` | Provider reasoning effort: `auto`, `none`, `low`, `medium`, `high`, `xhigh`, or `max`. Managed OpenCode turns receive an explicit model option unless this is `auto`; support remains model-dependent. |
| `failure_hints` | bool | `true` | Show one concise recovery hint after an interactive command fails. |
| `context_exclude` | array | `[]` | Optional context section IDs to omit. Manage with `aishe context`. |
| `show_usage` | bool | `true` | Record and display model-call usage in the interactive session. |
| `status_line` | bool | `true` | Enable the branded prompt's live status display. |
| `status_line_position` | string | `right` | Status placement: `right`, `below`, or `off`. |
| `status_line_items` | array | `["identity","mode","scope","session_cost","requests"]` | Ordered fields. `identity` is the compact safe connection/provider/endpoint/auth/model/reasoning/default disclosure. Individual identity fields and `backend`, `task`, `elapsed`, `context`, `last_tokens`, `last_cost`, and `session_tokens` are also available. |
| `budget_usd` | float | `0.0` | Stop calling the model past this session cost. `0` = unlimited. |
| `memory` | bool | `true` | Remember recent natural-language turns so follow-ups have context. Clear at the aishe prompt with `reset`. |
| `cache` | bool | `true` | Cache identical suggest-mode responses briefly so repeats are instant and free. Toggle at the aishe prompt with `cache on`/`off`. |
| `cache_ttl_secs` | integer | `300` | How long a cached response stays valid, in seconds. |
| `redact_secrets` | bool | `true` | Scrub likely secrets from the context block sent to the model. See [Logging and privacy](logging.md). |

The toggles named "at the aishe prompt" above (`sandbox`, `plan`, `cache`,
`details`, and `rehash`) are **prompt-only meta commands**: type them inside the
interactive shell (bare, or with a leading `/` as `/sandbox`). `reset` works
there too and is also a real `aishe reset` subcommand. Running `aishe sandbox`
from a terminal fails with `error: unrecognized subcommand`. See
[Commands: prompt-only meta commands](commands.md#prompt-only-meta-commands).

## `[backend]` section

The agent orchestrator is separately configured from shell behavior. Runtime
version/hash are deliberately absent: the AIShe build's embedded compatibility
manifest owns them.

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `engine` | string | `opencode` | Managed agent engine. `native` is a temporary repair/legacy compatibility override. |
| `fallback` | string | `native` | Compatibility engine allowed only when OpenCode fails before prompt admission. |
| `managed` | bool | `true` | Install and launch AIShe's private compatibility-pinned runtime. |
| `idle_timeout_secs` | integer | `1800` | Stop the private per-user supervisor after this idle period (30–86400). |
| `default_scope` | string | `workspace` | Default `workspace` or `host` selection. Yolo acceptance itself is never persisted. |
| `workspace_network` | string | `deny` | `allow` or `deny` network capability for workspace agent tools. |
| `output` | string | `focus` | `focus` (transient current command plus a bounded three-command digest, one activity summary, and final response), `compact` (one persistent completion row per action), or `detailed` (raw command output, diffs, usage, and agent events). |
| `max_output_tokens` | integer | `0` | Hard provider output cap; `0` delegates to backend/model unless organization policy caps it. |
| `max_instances` | integer | `8` | Maximum isolated connection supervisors that may coexist (1–32). Starting another deterministically stops the oldest idle candidate. |

```toml
[backend]
engine = "opencode"
fallback = "native"
managed = true
idle_timeout_secs = 1800
default_scope = "workspace"
workspace_network = "deny"
output = "focus"
max_output_tokens = 0
max_instances = 8
```

Fallback is one-way and pre-admission only. AIShe never duplicates a prompt
after OpenCode has accepted it, emitted partial output, or requested a tool.

## `[sandbox]` section

```toml
[sandbox]
linux_backend = "bwrap"
require_functional = false
workspace_roots = []
allow_host_yolo = true
```

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `linux_backend` | string | `bwrap` | Linux agent isolation implementation; `policy` explicitly selects best-effort policy-only behavior. |
| `require_functional` | bool | `false` | Fail setup/use instead of degrading when bubblewrap cannot pass its namespace self-test. |
| `workspace_roots` | array | `[]` | Additional canonical roots permitted in workspace scope. |
| `allow_host_yolo` | bool | `true` | Whether a user may explicitly accept host-wide yolo scope. Organization policy can only narrow this. |

On macOS, `linux_backend` cannot create an OS sandbox; Setup and Doctor report
policy-only behavior.

## `[named_dirs]` section (optional)

Named directories for `~name` expansion in `cd` (zsh hashed dirs):

```toml
[named_dirs]
proj = "/home/me/projects"
dl = "/home/me/Downloads"
```

Then `cd ~proj` and `cd ~proj/app` work.

## `[logging]` section (optional)

Audit logging of AI calls, complete bounded visible responses, managed tool
lifecycle records, approvals, file diffs, usage/cost, and AI-initiated actions.
Off by default because it persists conversation and command history.

| Field | Type | Default | Meaning |
|-------|------|---------|---------|
| `enabled` | bool | `false` | Write a JSONL audit log. Also enableable with `AISHE_LOG=1`. |
| `file` | string | unset | Log path. Default `$XDG_DATA_HOME/aishe/audit.jsonl`. Override with `AISHE_LOG_FILE`. |
| `redact` | bool | `true` | Scrub secrets from logged text. |

See [Logging and privacy](logging.md) for the event shapes and examples.

## `[connections.<id>]`

Schema 6 makes a named connection the unit of provider selection. A connection
binds a safe stable ID and label to a provider family, endpoint, transport,
model, reasoning choice, and one explicit authentication method. Any number of
connections may use the same provider.

```toml
[aishe]
connection = "openai-work"
connection_fallback = "openai-work"

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
reasoning_effort = "high"
[connections.openai-work.auth]
type = "oauth"
profile = "work"

[connections.openai-api]
provider = "openai"
label = "OpenAI API project"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-api.auth]
type = "api_key"
credential = "openai-team"
api_key_env = "OPENAI_API_KEY"

[connections.ollama-local]
provider = "openai"
label = "Ollama local"
base_url = "http://127.0.0.1:11434"
model = "qwen3:14b"
transport = "chat"
auth_required = false
[connections.ollama-local.auth]
type = "none"
```

Authentication types are deliberately non-overlapping:

- `api_key` resolves only its named credential and environment override.
- `oauth` resolves only the exact provider/profile OAuth root and ignores API
  key variables, even when they are set.
- `none` performs no credential lookup.
- `auto` is migration compatibility: key first, then the legacy OAuth store.

OAuth endpoints must normalize exactly to `https://api.openai.com` or
`https://api.x.ai`. Profile labels are normalized for their private directory;
config validation rejects two connections that would collide on one path.

Use `aishe connection list|show|add|edit|remove|use|pick` (or `/connection` in
the shell) to switch accounts; use `/model` only for models on the active
connection. `aishe models --connection ID` scopes discovery and its cache to the
connection's safe launch identity. See
[Commands — primary slash commands](commands.md#primary-slash-commands).

### Legacy `[providers.anthropic]` and `[providers.openai]`

These blocks remain for schema-5 migration and unambiguous v0.5 workflows.
Schema 5 is upgraded automatically to deterministic `anthropic` and `openai`
`auto` connections. Before atomic replacement, AIShe writes the existing
byte-for-byte versioned backup. New configuration should use `[connections]`.

| Field | Type | Meaning |
|-------|------|---------|
| `base_url` | string | API root. For OpenAI-compatible services, point this at the service. |
| `credential` | string | Named profile in `credentials.toml`; service presets keep this aligned with the endpoint owner. |
| `api_key_env` | string | Optional environment override. A non-empty value wins over the saved profile without changing it. |
| `model` | string | Model identifier sent with each request. |
| `transport` | string | `auto`, `responses`, or `chat`. Auto uses Responses for official OpenAI and Chat Completions for compatible endpoints. |
| `auth_required` | bool | Optional explicit auth policy. When absent, loopback endpoints need no key; non-loopback endpoints do. |

Defaults:

```toml
[providers.anthropic]
base_url = "https://api.anthropic.com"
credential = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"
transport = "auto"
auth_required = true

[providers.openai]
base_url = "https://api.openai.com"
credential = "openai"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
transport = "auto"
auth_required = true
```

See [Providers](providers.md) for Groq, Ollama, and others.

## Shared credentials

AIShe follows the AWS CLI pattern: ordinary settings and secrets are separate,
and the matching profile names are combined at runtime. Save a key without
putting it in shell history:

```sh
aishe auth set openai                 # hidden terminal prompt
aishe auth status openai
aishe auth list
```

For automation, use `aishe auth set openai --stdin` or
`--from-env VARIABLE`. A key is intentionally never accepted as a command-line
argument. `credentials.toml` is versioned TOML:

```toml
version = 1

[profiles.openai]
api_key = "..."
```

Resolution order is the configured non-empty environment variable, an
in-memory setup value, then the saved profile. Environment overrides never
overwrite the file. New files and atomic temporary files are mode `0600`, and
the containing config directory is mode `0700`. AIShe rejects symlinked,
non-regular, oversized, malformed, or group/world-readable credential files;
`aishe doctor --fix` can repair permissions without printing or changing a key.

OpenAI and xAI subscription OAuth is profile-isolated:

```sh
aishe auth login openai --profile work
aishe auth login openai --profile personal --headless
aishe auth status openai --profile work --json
aishe auth logout openai --profile personal
```

Tokens are written by the pinned runtime to a complete profile-specific
OpenCode HOME/XDG root and `auth.json`, which is required to be a current-user-owned regular file
with mode `0600`. Status and diagnostics deserialize only the provider, type,
and expiration metadata; they never serialize access or refresh tokens. An
explicit OAuth connection never consults an API key, and an explicit API-key
connection never consults OAuth. Only a migrated `auto` connection uses the old
precedence behavior.

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

- `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, or whatever `api_key_env` names:
  optional per-process override for the matching saved profile.
- `AISHE_CREDENTIALS_FILE`: override only the shared credentials path (useful
  for a mounted private volume or isolated test).
- `AISHE_RUNTIME_DIR`: relocate the managed runtime root (deployment/testing).
- `AISHE_RUNTIME_BASE_URL`: approved pinned-runtime mirror base URL. The
  embedded checksum and exact asset size remain mandatory.
- `AISHE_POLICY_FILE`: alternate organization-policy path for managed
  deployment/testing.
- `AISHE_MODE`: mode used by the native shell hook (`suggest`, `auto`, `yolo`).
- `AISHE_NL_KEY`: override the force-NL keybinding for the zsh hook (a `bindkey`
  sequence, for example `^o`).
- `AISHE_MODE_KEY`: override the mode-cycle keybinding for the zsh hook (a
  `bindkey` sequence; default `^[[Z`, Shift-Tab).
- `AISHE_AGENT_OUTPUT`: session override for `focus`, `compact`, or `detailed`
  agent transcripts. Ctrl-O toggles `focus`/`detailed` in the interactive shell.
- `AISHE_DETAILS_KEY`: override that zsh detail-toggle key (default `^O`,
  Ctrl-O).
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
aishe auth <command>      set/status/list/remove/path for saved API keys
aishe backend <command>   managed runtime status/install/verify/repair/recovery
aishe uninstall           state-preserving category-based removal
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
then atomically writes the migrated schema-v4 config. Backend/sandbox defaults
preserve or reduce authority; credentials and user state are never migrated
into backend data. A pre-rename
`llmsh/config.toml` in the same config directory is migrated on first run.
Installers and package upgrades replace the binary only; they do not delete the
config, history, tasks, trust store, or other user data.

## Organization policy

Administrators can install a read-only constraint file at
`/etc/aishe/policy.toml` on Linux or `/Library/Application
Support/Aishe/policy.toml` on macOS (the on-disk directory name is the
historical install path and is **not** renamed with the AIShe product
wordmark). `AISHE_POLICY_FILE` is the explicit deployment/test override. Policy
schema 1 may require/disable OpenCode, specify
a runtime mirror and approved hashes, require functional bubblewrap, disable
host yolo or network, restrict provider hosts/models, require audit/redaction,
disable user MCP/skills, cap budgets/output, and exclude support-bundle fields.

Policy never contains credentials and never grants more authority. Effective
precedence is organization constraints, then CLI request, trusted project
overlay, user config, and defaults. Setup and Settings label constrained values
as managed and fail with setup exit code 7 when a requested configuration
violates policy.
