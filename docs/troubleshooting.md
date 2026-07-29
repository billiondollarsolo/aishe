# Troubleshooting

## Start with aishe doctor

```sh
aishe doctor
```

It reports your backing shell, config path, resolved front-end, provider, and
whether the exact API-key environment variable is set. Most setup problems show
up here. Add `--probe` for reachability, `--live` for minimal feature calls,
`--json` for automation, `--fix` for safe local repairs, or `--bundle PATH` for
a redacted support bundle.

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

aishe reads the key only from the environment variable named by `api_key_env` in
your config. Export it in the same shell you launch aishe from:

```sh
export ANTHROPIC_API_KEY=sk-ant-...      # or whatever api_key_env names
```

For a local server such as Ollama, use `auth_required = false`; no dummy key is
needed.

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
`plan`, `cache`, and `reset`; see
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

A malformed config is never silently replaced with defaults. Aishe reports the
parse error so you can fix the file or restore one of its private `.bak` files.
Schema migrations also create a backup before an atomic rewrite.

Upgrades replace only the executable. They do not remove configuration,
`history.ext`, durable task records, audit data, or the trust store.
