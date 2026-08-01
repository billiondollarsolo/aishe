# Providers

AIShe configures three provider shapes:

- the **Anthropic Messages API**,
- the official OpenAI and xAI **Responses APIs**, and
- any custom **OpenAI-compatible Chat Completions API**.

For normal AI turns, AIShe generates a private provider definition for its
managed OpenCode engine. AIShe remains authoritative for the named connection,
endpoint, model, authentication binding, prices, budget, and organization restrictions. API keys
enter only the provider process environment; OAuth tokens remain in the
managed runtime's private store. Model-controlled tools inherit neither.

A connection's `transport = "auto"` selects the wire format
from `base_url`. `https://api.openai.com` and `https://api.x.ai` use Responses,
including reasoning-model tool calls and their continuation items. Groq, Ollama,
OpenRouter, Together, and other custom URLs use Chat Completions for broad
compatibility. Set `transport = "responses"` or `"chat"` only when a gateway
needs an explicit choice.

API-key connections resolve the profile named by `credential` in AIShe's private
`credentials.toml`. The variable named by `api_key_env` is an optional
higher-precedence override for CI, containers, and temporary testing. Keys are
never written to ordinary `config.toml`, generated backend config, session
mappings, tool journals, or support bundles.

OpenAI and xAI also support any number of labeled subscription OAuth profiles
through AIShe's pinned private OpenCode runtime:

```sh
aishe auth login openai --profile work
aishe auth login openai --profile personal --headless
aishe auth status openai --profile work
aishe auth logout openai --profile personal
```

OAuth is accepted only when the normalized endpoint is exactly
`https://api.openai.com` or `https://api.x.ai`; it cannot be redirected to a compatible gateway.
The local callback flow is the default on a workstation, while AIShe
automatically selects device authorization when SSH or a non-terminal output is
detected. `--browser` and `--headless` explicitly select either flow. Each
profile gets a complete private OpenCode HOME/XDG tree. An explicit `oauth`
connection ignores API-key variables and stores; an explicit `api_key`
connection ignores OAuth. Only schema-migrated `auto` connections retain the
old key-first behavior.

Create and inspect connections without editing TOML:

```sh
aishe connection add openai-work --provider openai --auth oauth --profile work
aishe connection add openai-api --provider openai --auth api-key --credential openai-team
aishe connection list
aishe connection show openai-work
aishe auth login --connection openai-work
```

With API-key authentication, interactive Setup calls `GET /v1/models`
immediately after the credential step. This is a token-free
credential/endpoint check and the source
of the model picker. Choosing a listed model or typing an ID present anywhere
in the full response validates it without a generation call. If a custom
endpoint has an incomplete or unavailable catalog, a manually typed ID must
pass one minimal generation request before Setup continues; the UI discloses
that request first.

Subscription OAuth is available only through the managed agent transport, so
Setup accepts a model ID directly and validates it through that runtime when a
live check is requested. Interactive `/model` for **Codex - OAuth** and
**Grok - OAuth** lists models from the managed OpenCode runtime
(`GET /config/providers` on the private loopback server) — the same
subscription-filtered catalog OpenCode uses — not from public
`GET /v1/models`. API-key connections still use the endpoint `/v1/models`
catalog.

## Anthropic

```toml
[aishe]
connection = "anthropic"

[connections.anthropic]
provider = "anthropic"
label = "Anthropic"
base_url = "https://api.anthropic.com"
model = "claude-sonnet-4-20250514"
[connections.anthropic.auth]
type = "api_key"
credential = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
```

```sh
aishe auth set anthropic
aishe
```

## OpenAI

```toml
[aishe]
connection = "openai-work"

[connections.openai-work]
provider = "openai"
label = "Codex - OAuth · work"
base_url = "https://api.openai.com"
model = "gpt-5.6-luna"
transport = "responses"
[connections.openai-work.auth]
type = "oauth"
profile = "work"
```

```sh
aishe auth login openai --profile work
# or: aishe auth login --connection openai-work
# API-key alternative: create an api_key connection and run aishe auth set PROFILE
aishe
```

Status and pickers brand this as **Codex - OAuth · work** (vs **Codex - API**
for an API-key connection).
The managed engine uses OpenAI Responses for reasoning and tools, avoiding the
Chat Completions incompatibility between reasoning effort and function tools on
models such as GPT-5.6. OpenCode owns provider-native reasoning continuation and
conversation compaction; AIShe owns the durable session mapping, tool effects,
usage authorization, and budget.

With an API key, the native compatibility backend also uses `/v1/responses` with
Responses-native `max_output_tokens`, reasoning items, and stateless
continuation. It remains available for legacy task resume and failures that
occur before the managed engine admits a prompt.

`base_url` is the service root. AIShe also accepts the commonly copied
`https://api.openai.com/v1` form and canonicalizes the trailing `/v1` before
appending `/v1/responses`; compatible and Anthropic endpoints receive the same
protection against accidental `/v1/v1/...` URLs.

## xAI / Grok

xAI uses the OpenAI-compatible provider block but receives a first-class setup
preset and the provider-native Responses transport:

```toml
[aishe]
connection = "xai-prod"

[connections.xai-prod]
provider = "xai"
label = "Grok - OAuth · prod"
base_url = "https://api.x.ai"
model = "grok-4.5"
transport = "responses"
[connections.xai-prod.auth]
type = "oauth"
profile = "prod"
```

```sh
aishe auth login xai --profile prod
# or: aishe auth login --connection xai-prod
# API-key alternative: a separate api_key connection with credential = "xai"
```

Branded as **Grok - OAuth · prod** (vs **Grok - API** for API-key auth).
The OAuth credential binds to OpenCode's exact `xai` provider identity so its
refresh and bearer-token hooks remain active. A custom `api.x.ai`-lookalike host
does not qualify.

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

## Switching accounts and models at runtime

Account and model are separate surfaces so changing a model never quietly
changes logins:

```sh
# Switch account (Enter is shell-local; the following prompt can save a default)
aishe connection pick               # or /connection in the shell
aishe connection use openai-work --default

# Change model on the *active* connection only
aishe model                         # or /model in the shell
aishe model gpt-5.6-terra
aishe model gpt-5.6-sol --connection openai-work
aishe model default                 # restore the durable default model
aishe models --connection openai-work
```

In an interactive terminal, Enter applies the highlighted choice only to that
shell; the following default-No prompt can save it as the durable default.
Navigation is ↑/↓ or Ctrl-P/Ctrl-N, with Home/End and Page Up/Page Down for
larger lists. Brands on status and `/status`:

| Brand | Meaning |
|-------|---------|
| **Codex - API** | OpenAI with API key |
| **Codex - OAuth · {profile}** | ChatGPT/Codex subscription OAuth |
| **Grok - API** | xAI with API key |
| **Grok - OAuth · {profile}** | SuperGrok subscription OAuth |

For **Codex - OAuth** and **Grok - OAuth**, `/model` lists models from the
managed OpenCode runtime (`GET /config/providers` on the private loopback
server) — the same subscription-filtered catalog OpenCode uses. When the
runtime cannot enumerate the account, AIShe falls back to configured, cached,
recently audited, and typed model IDs. API-key connections use endpoint
`GET /v1/models`. AIShe never queries `models.dev`. After `aishe auth login
openai|xai [--profile …]` without an existing connection, AIShe creates one
(e.g. `xai-work`) so `/connection` lists it immediately.
The native compatibility/provider-probe layer supports both `max_tokens` and
`max_completion_tokens` for custom Chat Completions endpoints and caches the
accepted spelling. Managed OpenCode turns use the endpoint's provider adapter.

## Fallback and offline behavior

`provider_fallback` is retained for the native compatibility backend. It lists
provider blocks to try when that native call fails before any effect. It is
useful for legacy tasks and repair-mode suggest/chat/auto:

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

Each authenticated native block must resolve a credential or it is skipped;
local blocks with `auth_required = false` need none. The setting is sensitive,
so a project overlay needs `aishe trust` to apply it.

Managed-turn safety takes precedence over transparent provider fallback. Once
OpenCode admits a prompt, emits partial output, or requests a tool, AIShe never
starts the turn again against another provider or backend. An outage marks the
durable conversation interrupted and offers resume. This prevents duplicated
commands and spend. To work fully offline, select the local Ollama endpoint as
the active provider before the managed turn rather than relying on a mid-turn
fallback.

To verify the chain is actually reachable — especially the "offline-capable"
claim for a local Ollama — run:

```sh
aishe doctor --probe
```

For native compatibility it sends one short, read-only `GET /v1/models` to each chain member (no
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
