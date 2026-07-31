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

## A command was treated as natural language

If a valid command was sent to the model, the command may not be on your `PATH`
or known yet. Rebuild the command cache with the `rehash` meta command, typed
**at the aishe prompt** (bare or as `/rehash`):

```
~/projects/app ❯ rehash
```

`rehash` is not an `aishe` subcommand — running `aishe rehash` from a terminal
fails with `error: unrecognized subcommand`. The same is true of `sandbox`,
`plan`, `cache`, and `details`; `reset` also has the real `aishe reset` form. See
[Prompt-only meta commands](commands.md#prompt-only-meta-commands).

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
with `stream on` at the aishe prompt, or `stream = true` in your config
(`stream` is a [prompt-only meta command](commands.md#prompt-only-meta-commands),
not an `aishe` subcommand).

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
