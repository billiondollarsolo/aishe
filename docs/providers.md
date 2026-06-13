# Providers

aishe talks to two provider shapes:

- the **Anthropic Messages API**, and
- any **OpenAI-compatible Chat Completions API**.

The OpenAI shape covers OpenAI itself plus Groq, Ollama, OpenRouter, Together,
and similar services through `base_url`. You configure both blocks and select one
with `provider`.

API keys are read only from the environment variable named by `api_key_env`. They
are never written to the config file.

## Anthropic

```toml
[aishe]
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"
```

```sh
export ANTHROPIC_API_KEY=sk-ant-...
aishe
```

## OpenAI

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "https://api.openai.com"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
```

```sh
export OPENAI_API_KEY=sk-...
aishe
```

## Groq

Groq exposes an OpenAI-compatible endpoint and supports strict JSON schema, which
pairs well with aishe's default structured output.

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "https://api.groq.com/openai"
api_key_env = "GROQ_API_KEY"
model = "openai/gpt-oss-120b"
```

```sh
export GROQ_API_KEY=gsk_...
aishe
```

## Ollama (local models)

Ollama serves an OpenAI-compatible API locally. No real key is needed, but
`api_key_env` must name a variable that is set to something non-empty.

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "http://localhost:11434"
api_key_env = "OLLAMA_API_KEY"
model = "llama3.1"
```

```sh
export OLLAMA_API_KEY=ollama   # any non-empty value
aishe
```

Local models may not support strict JSON schema. aishe steps down automatically,
but you can also set `structured = "prompt"` for the loosest behavior. See
[Modes](modes.md).

## OpenRouter, Together, and others

Any service that speaks the OpenAI Chat Completions format works. Set `base_url`
to the service root, `api_key_env` to your key variable, and `model` to a model
the service exposes.

## Switching providers and models at runtime

```sh
aishe provider openai
aishe model gpt-4o-mini
```

`aishe model` sets the model for the currently selected provider. Both persist to
the config.

## Fallback chain (resilience and offline)

`provider_fallback` lists providers (by block name) to try, in order, when the
primary `provider` fails *after its own retries* — a dead endpoint, a hard auth
error, or a blown budget degrades to the next provider instead of failing the
call. It's how you keep working when your main API is down, or fall back to a
**local** model for offline use:

```toml
[aishe]
provider = "anthropic"
provider_fallback = ["openai"]   # then a local Ollama configured below

[providers.openai]
base_url = "http://localhost:11434"   # Ollama
api_key_env = "OPENAI_API_KEY"        # set it to any non-empty value
model = "llama3"
```

Each named block must have its API-key env set or it is skipped (so a missing key
in a fallback never breaks the primary). `aishe doctor` shows the resolved chain.
When a fallback is used, a one-line notice is printed once. Note: configuring a
chain serves answers non-streamed (resilience over live token streaming); a
single provider streams as usual. The setting is treated as sensitive, so a
project overlay needs `aishe trust` to apply it.

To verify the chain is actually reachable — especially the "offline-capable"
claim for a local Ollama — run:

```sh
aishe doctor --probe
```

It sends one short, read-only `GET /v1/models` to each chain member (no
completion, so it costs no tokens) and reports each as **reachable**, **reachable
but key rejected** (a 401/403 — the endpoint is up but the key is wrong), or
**unreachable** (connection refused / timeout). An unreachable member is a
warning, not a failure, so `doctor` still passes offline.

## Cost and trust storage

aishe honors the system trust store, so it works behind corporate or
TLS-inspecting proxies whose CA is not in the bundled root set. Token usage is
metered per session; see [Token usage and cost](usage-and-cost.md).

## Behind a proxy or with a custom endpoint

Point `base_url` at your gateway. Standard `HTTPS_PROXY` and related environment
variables are respected by the HTTP layer. If your endpoint terminates TLS with a
private CA, aishe will use your operating system trust store automatically.
