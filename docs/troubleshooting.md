# Troubleshooting

## Start with aishe doctor

```sh
aishe doctor
```

It reports your backing shell, config path, resolved front-end, provider, and
whether the API key is set. Most setup problems show up here.

## "API key not found"

aishe reads the key only from the environment variable named by `api_key_env` in
your config. Export it in the same shell you launch aishe from:

```sh
export ANTHROPIC_API_KEY=sk-ant-...      # or whatever api_key_env names
```

For Ollama and other local servers, the variable still has to be set to something
non-empty even if the server ignores it.

## A natural-language request ran as a command

If your request starts with a real command name (for example "find large files"),
aishe treats it as a command. Force the natural-language route with the `?`
prefix:

```
?find large files
```

## A command was treated as natural language

If a valid command was sent to the model, the command may not be on your `PATH`
or known yet. Rebuild the command cache:

```sh
aishe rehash
```

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
schema, aishe steps down automatically. You can loosen the constraint:

```sh
aishe structured json       # or: aishe structured prompt
```

## Job control (Ctrl-Z, bg, fg) does not work

In the reedline front-end, each command runs as a fresh shell invocation, so job
control of delegated processes is not supported. `Ctrl-C` reaches the foreground
child and aishe itself survives. For full job control, use the zsh-PTY front-end
(the default when zsh is present), which is your real zsh.

## Costs or budgets look wrong

Cost is estimated from a price table. If your model is missing or priced
differently, add a `[pricing]` override. See
[Token usage and cost](usage-and-cost.md). Budgets are only enforced when the
model's price is known.

## Streaming shows nothing until the end

The endpoint may not support SSE, in which case the whole answer arrives at once.
Streaming is also not used for the scriptable `-c` path. Confirm streaming is on
with `aishe stream on`.

## Resetting to defaults

Remove the config to re-run the first-run wizard:

```sh
rm ~/.config/aishe/config.toml
aishe
```

A malformed config does not stop aishe from starting; it reports the problem and
falls back to defaults.
