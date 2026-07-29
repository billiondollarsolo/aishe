# Providers

aishe talks to three provider shapes:

- the **Anthropic Messages API**, and
- the official OpenAI **Responses API**, and
- any custom **OpenAI-compatible Chat Completions API**.

The `[providers.openai]` block's `transport = "auto"` selects the wire format
from `base_url`. `https://api.openai.com` uses Responses, including
reasoning-model tool calls and their continuation items. Groq, Ollama,
OpenRouter, Together, and other custom URLs use Chat Completions for broad
compatibility. Set `transport = "responses"` or `"chat"` only when a gateway
needs an explicit choice.

API keys are resolved from the profile named by `credential` in Aishe's private
`credentials.toml`. The variable named by `api_key_env` is an optional
higher-precedence override for CI, containers, and temporary testing. Keys are
never written to ordinary `config.toml`.

During interactive Setup, Aishe calls `GET /v1/models` immediately after the
credential step. This is a token-free credential/endpoint check and the source
of the model picker. Choosing a listed model or typing an ID present anywhere
in the full response validates it without a generation call. If a custom
endpoint has an incomplete or unavailable catalog, a manually typed ID must
pass one minimal generation request before Setup continues; the UI discloses
that request first.

## Anthropic

```toml
[aishe]
provider = "anthropic"

[providers.anthropic]
base_url = "https://api.anthropic.com"
credential = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"
```

```sh
aishe auth set anthropic
aishe
```

## OpenAI

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "https://api.openai.com"
credential = "openai"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
transport = "auto"
auth_required = true
```

```sh
aishe auth set openai
aishe
```

Official OpenAI requests use `/v1/responses` for normal, structured, streaming,
and tool-use calls. In a multi-step yolo run, aishe sends the response's native
reasoning and function-call items back with each tool result, as required by
reasoning models. It uses Responses-native `max_output_tokens`,
`reasoning = { effort = ... }`, and `store = false`; this avoids the
Chat-Completions incompatibility between reasoning effort and function tools on
models such as GPT-5.6.

With `store = false`, current Responses reasoning items contain opaque
`encrypted_content` by default. Aishe replays every returned provider item in
memory and retains encrypted reasoning plus provider routing IDs exactly in
private durable checkpoints; plaintext checkpoint fields remain redacted. This
follows OpenAI's
[stateless reasoning guidance](https://developers.openai.com/api/docs/guides/reasoning#preserve-reasoning-without-stored-responses);
the legacy `include = ["reasoning.encrypted_content"]` request field is no
longer required.

`base_url` is the service root. Aishe also accepts the commonly copied
`https://api.openai.com/v1` form and canonicalizes the trailing `/v1` before
appending `/v1/responses`; compatible and Anthropic endpoints receive the same
protection against accidental `/v1/v1/...` URLs.

## Groq

Groq exposes an OpenAI-compatible endpoint and supports strict JSON schema, which
pairs well with aishe's default structured output.

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "https://api.groq.com/openai"
credential = "groq"
api_key_env = "GROQ_API_KEY"
model = "openai/gpt-oss-120b"
transport = "chat"
auth_required = true
```

```sh
aishe auth set groq
aishe
```

## Ollama (local models)

Ollama serves an OpenAI-compatible API locally. No key or dummy environment
variable is required:

```toml
[aishe]
provider = "openai"

[providers.openai]
base_url = "http://localhost:11434"
credential = "ollama"
api_key_env = "OLLAMA_API_KEY"
model = "llama3.1"
transport = "chat"
auth_required = false
```

```sh
aishe
```

Local models may not support strict JSON schema. aishe steps down automatically,
but you can also set `structured = "prompt"` for the loosest behavior. See
[Modes](modes.md).

### Embeddings (fully offline semantic history)

Ollama also serves embedding models on the same OpenAI-compatible route
(`/v1/embeddings`), which is exactly what [semantic history](../README.md#features)
calls — so pointing `embedding_provider` at Ollama keeps that feature entirely on
your machine. Pull an embedding model first:

```sh
ollama pull nomic-embed-text     # 274 MB, 768 dimensions
```

```toml
[aishe]
provider = "openai"
semantic_history = true
embedding_provider = "openai"        # the provider block below, not the company
embedding_model = "nomic-embed-text"

[providers.openai]
base_url = "http://localhost:11434"
api_key_env = "OLLAMA_API_KEY"
model = "llama3.1"
transport = "chat"
auth_required = false
```

```sh
aishe history index          # embeds your recorded history
aishe history search "the docker run with the prometheus volume"
```

`aishe doctor --probe` reports the embedding endpoint it can reach.

Two things worth knowing. The chat model and the embedding model are separate:
`model` answers questions, `embedding_model` builds the index, and an embedding
model cannot chat. And ranking quality is the embedding model's, not aishe's —
short shell commands are hard to embed well, so expect the right command near
the top rather than always first. Re-run `aishe history index` after switching
models, since vectors from different models are not comparable.

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
the config. In the zsh PTY front-end, the right-prompt model label refreshes on
the next prompt after this command.

For custom OpenAI-compatible Chat Completions endpoints, aishe supports both
`max_tokens` and `max_completion_tokens`. It records the spelling accepted by
each endpoint/model pair in the aishe state directory and reuses it in later
processes, so a compatibility retry happens at most once unless the endpoint's
behavior changes.

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
api_key_env = "OPENAI_API_KEY"        # ignored because auth_required is false
model = "llama3"
transport = "chat"
auth_required = false
```

Each authenticated block must have its API-key env set or it is skipped (so a
missing key in a fallback never breaks the primary); local blocks with
`auth_required = false` need none. `aishe doctor` shows the resolved chain.
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

For a focused provider check, run:

```sh
aishe provider test             # configuration + endpoint capability checks
aishe provider test --live      # minimal text/schema/tool/stream calls
aishe models --provider openai  # enumerate that configured endpoint
```

Capability results are cached per endpoint, model, and transport in aishe's
private data directory, so a compatible model does not relearn the same
behavior on every request.

## Cost and trust storage

aishe honors the system trust store, so it works behind corporate or
TLS-inspecting proxies whose CA is not in the bundled root set. Token usage is
metered per session; see [Token usage and cost](usage-and-cost.md).

## Behind a proxy or with a custom endpoint

Point `base_url` at your gateway. Standard `HTTPS_PROXY` and related environment
variables are respected by the HTTP layer. If your endpoint terminates TLS with a
private CA, aishe will use your operating system trust store automatically.
