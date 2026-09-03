# Troubleshooting

## Start with aishe doctor

```sh
aishe doctor
```

It reports your backing shell, config and credentials paths, resolved front-end,
provider, active credential source, managed runtime/version/hash, authenticated
loopback server, trusted plugin/tool restrictions, credential isolation,
session/journal state, and sandbox self-test. Most setup problems show up here.
Add `--probe` for reachability, `--live` for minimal feature calls,
`--json` for automation, `--fix` for safe local repairs, or `--bundle PATH` for
a redacted support bundle.

Support bundles exclude API keys, OAuth tokens, provider request bodies,
private backend control URLs/tokens, environment values, and unredacted
credential data. They still contain paths, configuration metadata, endpoint
hosts, versions, and diagnostic summaries; inspect a bundle before sharing it.

## Stable error-code index

Structured errors use the same codes in text and JSON. Start with the exact
primary action in the error; use this table when automating recovery or when
the action did not resolve the problem.

| Code | Meaning | Maintained recovery |
| --- | --- | --- |
| `cli.missing_request` | `suggest` received no request | `aishe suggest "list files by size"` |
| `cli.interactive_shell_missing` | the requested interactive zsh front end is unavailable | install zsh, use `aishe -c …`, or generate the tested Bash hook with `aishe init bash` |
| `config.setup_failed` | setup validation or its transactional apply step failed | rerun `aishe setup --resume`; use `aishe doctor` for the reported dependency/config category |
| `config.provider_unavailable` | no usable connection/provider | `aishe setup`, then `aishe doctor --live` |
| `auth.missing_credential` | configured credential is absent | `aishe auth status`; then `aishe auth set PROFILE` or set the configured environment variable |
| `auth.invalid_credential` | provider rejected authentication | `aishe auth status`; replace the credential; `aishe doctor --live` |
| `auth.permission_denied` | credential lacks endpoint/model access | `aishe connection show --json`; verify project/model access; `aishe doctor --live` |
| `provider.model_not_found` | model ID is unavailable | `aishe models --refresh`; then `aishe model MODEL` |
| `provider.unsupported_tools` | transport cannot execute tools | `aishe settings`; choose a tool-capable Responses transport; `aishe provider test --live` |
| `provider.unsupported_parameter` | model rejected a request option | `aishe doctor --live`; review the connection in `aishe settings` |
| `provider.unsupported_format` | structured-output mode is unsupported | `aishe doctor --live`; review transport/model settings |
| `provider.rate_limited` | short-term provider rate limit | wait, then retry; inspect `aishe status` for repeated requests |
| `provider.quota_exhausted` | provider billing/quota exhausted | check provider billing/quota before retrying |
| `network.timeout` | provider request timed out | `aishe doctor --probe`; verify proxy/endpoint; retry |
| `network.provider_unreachable` | DNS/TLS/connectivity failure | `aishe doctor --probe`; verify proxy, OS trust store, and endpoint |
| `provider.server_unavailable` | provider returned a retryable server failure | retry later; use `aishe doctor --probe` if persistent |
| `provider.malformed_response` | endpoint returned an incompatible document/stream | `aishe doctor --live`; confirm provider transport compatibility |
| `provider.unknown` | provider failure could not be classified safely | `aishe doctor --live`; create a redacted support bundle if persistent |
| `provider.connection_unavailable` | the selected line needs an agent but no usable provider/connection is active | `aishe connection list`; select one or run `aishe setup`; then `aishe doctor --live` |
| `backend.suggest_failed` | managed backend failed an admitted suggest turn | `aishe backend status`; `aishe backend verify --live`; then retry or resume |
| `backend.suggest_line_managed` | the interactive suggest hook failed in the managed backend | `aishe backend status`; inspect `aishe sessions`; verify/repair before retrying |
| `backend.fix_line_managed` | the fix-last-command hook failed in the managed backend | keep the failed command unchanged; run `aishe backend status`, then retry the explicit fix action |
| `backend.yolo_line_managed` | an autonomous hook turn failed after managed dispatch | inspect `aishe sessions` for partial effects; verify the backend; resume rather than blindly replaying |
| `backend.auto_line_managed` | an automatic hook turn failed after managed dispatch | inspect the shell and session state; run `aishe backend status`; retry only after reconciling effects |
| `backend.one_shot_managed` | a managed `-c`/pipe turn failed | inspect `aishe sessions`; run `aishe backend verify --live`; preserve the nonzero status in automation |
| `network.operation_failed` | an untyped outer network path failed before a more specific provider classification | `aishe doctor --probe`; verify endpoint/proxy/TLS; retry |
| `auth.unavailable` | an outer command could not load required authentication | `aishe auth status`; repair the named credential; retry |
| `policy.denied` | effective organization/user policy rejected the operation | `aishe doctor --json`; review effective policy and requested authority |
| `sandbox.unavailable` | required kernel/policy sandbox could not be established | `aishe doctor --json`; repair the reported sandbox requirement |
| `backend.operation_failed` | runtime/supervisor/backend operation failed | `aishe backend status`; `aishe backend verify --live` |
| `config.invalid` | effective configuration could not be loaded | `aishe doctor`; repair the reported config file |
| `io.operation_failed` | local path, permissions, or operating-system operation failed | `aishe doctor`; inspect the bounded details and correct the path/permissions |
| `internal.unexpected` | no safer public domain classification was available | `aishe doctor`; if persistent, create and inspect a redacted support bundle |
| `cli.unknown_connection` | a connection or provider id was not found in the active configuration | `aishe connection list`; use an exact id or label |
| `cli.unknown_task` | the named task or session id does not exist | `aishe sessions`; resume an id from that list |
| `cli.shell_required` | the command changes this shell's conversation and needs an AIShe shell | start `aishe`, then run the slash form |
| `config.setup_incomplete` | setup was paused before it wrote a configuration | `aishe setup --resume`, or `aishe setup --restart` to begin again |

Error namespaces also define stable domain statuses for automation; see
[Automation and machine-readable contracts](automation.md#error-document-v1).
`aishe suggest --json` deliberately keeps its legacy aggregate process status 1
on failure while reporting the precise domain status inside the stderr object.

## "Managed agent engine unavailable"

Inspect the runtime separately from the provider:

```sh
aishe backend status
aishe backend verify --live
aishe backend logs --tail 200
```

If the runtime is missing, use `aishe backend install`; if its metadata,
checksum, executable version, or plugin health is invalid, use `aishe backend
repair`. `aishe backend rollback` selects the immediately previous compatible
verified install. AIShe may use native suggest/chat/auto compatibility only when
the failure occurs before OpenCode admits the prompt. It never retries a turn
through another engine after partial output or a tool effect.

An interrupted admitted turn remains visible:

```sh
aishe sessions
aishe session show ses_...
aishe resume ses_...
```

This avoids blindly repeating a command whose outcome may be unknown.

## Bubblewrap is installed but Doctor says unusable

AIShe tests namespace creation, a read-only host, writable workspace/private
`/tmp`, and the network profile. Finding `bwrap` on `PATH` is not enough. A
container, kernel, or security profile can disable the required user
namespaces. Run `aishe doctor --json` for the exact state, then either enable
the host capability, choose explicit policy-only behavior where allowed, or ask
the administrator to change organization policy. AIShe never labels an
unusable bubblewrap install as sandboxed.

Setup can install bubblewrap on supported Linux package managers, but only after
showing the exact command and receiving consent. Doctor `--fix` deliberately
does not install system packages.

## A config file, custom command, or skill is ignored

Usually the file is in the wrong directory. aishe follows each platform's own
convention, so its config directory is `~/.config/aishe/` on Linux but
`~/Library/Application Support/aishe/` on macOS. Nothing under `~/.config/aishe`
is read on macOS, and nothing warns you — a command dropped there simply never
shows up in `aishe commands`.

`aishe doctor` prints the config path it actually resolved; put `commands/`,
`skills/`, and `aishrc` next to that `config.toml`. On macOS:

```sh
mkdir -p ~/"Library/Application Support/aishe/commands"
```

If you would rather not think about the difference, point both directories
somewhere explicit — these work identically on Linux and macOS, and each takes a
*base* directory that aishe appends `aishe/` to:

```sh
export AISHE_CONFIG_DIR="$HOME/.config"        # config.toml, commands/, skills/, aishrc
export AISHE_DATA_DIR="$HOME/.local/share"     # history, audit log, undo journal, trust store
aishe doctor                                   # confirm the new paths
```

The full table is in [File locations](configuration.md#file-locations).

## "API key not found"

AIShe could not find the saved credential profile named in your provider config.
Enter it with the hidden prompt:

```sh
aishe auth set anthropic
aishe auth status anthropic
```

An `api_key_env` value remains a higher-precedence override when set. If Doctor
reports insecure permissions, run `aishe doctor --fix` or `chmod 600` on the
exact credentials path it prints. For a local server such as Ollama, use
`auth_required = false`; no dummy key is needed.

The managed-backend error names the missing AIShe credential profile and
configured environment override. The secret is passed only to the provider
server process; it is removed from model-controlled command/tool environments
and never written to backend config, session mappings, or tool journals.

## A natural-language request ran as a command

The full-buffer router recognizes common question grammar even when the first
word is an installed command: `what is ...`, `where is ...`, and `who am ...`
route to the LLM and use the natural-language highlight, while `what --version`
remains a command. For an ambiguous line such as `find large files`, force the
natural-language route with the `?` prefix:

```
?find large files
```

## Natural language ran as a shell command (`install`, `find`, …)

If English starts with a real binary name, AIShe runs **shell**, not the agent:

```text
install kubectl please
# → /usr/bin/install …  often: install: No such file or directory
```

**Fix:** force natural language with a leading `?` (most reliable):

```text
? install kubectl please
```

The semantic route highlight distinguishes shell and agent input when color is
available; `aishe route -- '<line>'` names the route and reason in plain text,
and `?` remains the unambiguous non-color agent cue. There is no separate NL
mode on the status line. Optional Meta/Alt+Return needs Option-as-Meta in the
terminal (iTerm/Terminal/VS Code); see
[Getting started §6](getting-started.md#6-force-a-route-when-needed).

## A command was treated as natural language

If a valid command was sent to the model, first confirm that it is available to
the same shell and `PATH` AIShe is using:

```sh
command -v your-command
```

The flagship front end uses your live zsh command resolution. If you just
installed a command and zsh cached an earlier miss, run zsh's own `rehash` (or
`hash -r`) as an ordinary shell builtin. `/rehash` is not an AIShe command.

If it is a shell builtin or a function from a sourced file, define it in
`.aishrc` so aishe knows about it, or use the zsh-PTY front-end where your real
shell resolves everything.

## TLS or certificate errors behind a proxy

aishe uses your operating system trust store, so corporate or TLS-inspecting
proxies with a private CA generally work. Make sure your CA is installed in the
system trust store. Set `HTTPS_PROXY` if your environment requires it.

## The model returns malformed output

suggest mode asks for strict JSON by default and parses defensively, so malformed
output becomes a plain answer rather than a crash. If a provider rejects the
schema, aishe steps down automatically. You can loosen the constraint in your
config:

```toml
[aishe]
structured = "json"     # or "prompt" for unconstrained text
```

## Job control (Ctrl-Z, bg, fg)

The interactive shell is your real zsh in a PTY, so job control works exactly as
it does in zsh. The only exception is the non-interactive paths (`aishe -c …`,
piped stdin), which run each line in a fresh shell and so do not manage
long-lived background jobs; `Ctrl-C` reaches the foreground child and aishe
survives.

## Costs or budgets look wrong

Cost is estimated from a price table. If your model is missing or priced
differently, run `aishe price set MODEL --input PRICE --output PRICE` or use
`aishe settings`; setup asks automatically for unknown models. See
[Token usage and cost](usage-and-cost.md). Budgets are only enforced when the
model's price is known.

## Streaming shows nothing until the end

The endpoint may not support SSE, in which case the whole answer arrives at once.
Streaming is also not used for the scriptable `-c` path. Confirm streaming is on
through `aishe settings`, or set `stream = true` in your config. `stream` is not
a slash or CLI command.

## Repairing or reconfiguring

Use `aishe settings` to edit the active configuration or `aishe setup` to rerun
the guided flow. An interrupted setup resumes with `aishe setup --resume`;
`aishe setup --restart` discards only its draft and leaves the active config
untouched.

A malformed config is never silently replaced with defaults. AIShe reports the
parse error so you can fix the file or restore one of its private `.bak` files.
Schema migrations also create a backup before an atomic rewrite.

Upgrades replace the executable and add/activate only a verified compatible
runtime when needed. They do not remove configuration, credentials,
`history.ext`, managed sessions, durable legacy tasks, tool journals, audit
data, undo data, or the trust store.

For removal, preview `aishe uninstall --dry-run`. Plain uninstall selects only
replaceable binary/runtime components; state deletion is separated by category
and requires explicit confirmation.
