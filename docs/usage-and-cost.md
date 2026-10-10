# Token usage and cost

AIShe records provider usage when it is reported, so you can inspect session
spend and configure a cap. A missing usage object is unknown, even for a priced
model; numeric input and output counts of zero are a known zero.

Background task details distinguish recorded cost from missing pricing. A
fully priced task can show a known zero; unpriced work shows n/a, and partially
priced work labels its recorded subtotal and missing coverage. Older task
records without pricing provenance remain unknown. A money-limited task cannot
resume by treating unknown historical spend as zero; token and request limits
continue to apply independently. Provider responses without token usage remain
unmetered even when the model has an exact price. Automatic provider fallbacks
also leave task pricing coverage unknown; a positive task money cap requires a
fixed provider/model with automatic fallbacks disabled.

## What you see

The interactive shell keeps a live status chip in zsh's native right prompt.
It shows the safe connection identity, mode, scope, active connection cost, and
request count. You can turn it off and choose its ordered fields during setup or
in `aishe settings`.

```
  436 in · 119 out · 1 req · ~$0.0001
```

The compact `identity` field combines connection label/ID, provider/endpoint,
authentication label, model/reasoning, and shell-local/default state. Individual
fields are `connection`, `provider`, `endpoint`, `auth`, `selection`, `model`,
`reasoning`, `mode`, `backend`, `scope`, `task`, `elapsed`, `context`,
`last_tokens`, `last_cost`, `session_tokens`, `session_cost`, and `requests`.
`context` is the latest provider-turn
input-token count, not a guessed percentage. A detailed status can render:

```
OpenAI work (openai-work) · openai@api.openai.com · OAuth work · gpt-5.6-luna/high · this shell · workspace · context 8.4K tok ·
last 1,697/374 tok · session cost ~$0.0112 · 2 reqs
```

The display refreshes after each call. `off` hides it. `show_usage = false`
disables usage output, while
`status_line_position = "off"` hides only the live prompt line.

```toml
[aishe]
show_usage = false
```

### Whole-session summary

When you exit the interactive shell, AIShe prints a single dim line totalling
the session across model calls:

```
aishe session: 18,204 in · 5,130 out · 9 reqs · ~$0.0731
```

Cost is summed per call using each command's own model price (so a session that
spans models keeps its recorded attribution). Missing usage, unknown model
prices, and ambiguous automatic fallbacks never become free calls. Fully
covered work can show `~$0.0000`; mixed work shows the known subtotal as
`~$0.0731 (partial; 2 unknown)`, including a zero subtotal when appropriate.
Entirely unknown work shows `cost n/a`. Token fields likewise show `tokens n/a`
or the verified token subtotal with `(partial)` rather than filling missing
reports with zero. Automatic fallback ambiguity preserves the provider's
reported tokens while leaving price attribution unknown. It's gated on the same `show_usage` toggle and appears only when
at least one model call was made.

### The full report

`/usage` (or `aishe usage`) is the detailed view:

```
AIShe usage
  this shell  4,812 in · 331 out · 6 turns · plan · 88 thinking · 64% cached · 21.4s model time
  today       26,782 in · 1,398 out · 20 turns · plan · 293 thinking · 62% cached · 107.3s model time
  all time    27,549 in · 1,429 out · 22 turns · plan · 381 thinking · 61% cached · 114.3s model time
  plan        5h 39% left · week 62% left (from your provider subscription)

by model:
  gpt-5.6-luna    24,743 in   1,332 out   21 req   plan   62% cached
  TOTAL           27,549 in   1,429 out   22 req   plan   61% cached
```

Each line carries the recorded coverage alongside input and output tokens,
prompt-cache reads and writes as a hit rate, thinking tokens, turn count, model
time, and cost. Group with `--by model|connection|day|session`, narrow with
`--since 2h` or `--connection ID`, and script it with `--json`.
JSON uses `cost_usd: null` for an incomplete cost estimate and reports the
verified amount separately as `known_cost_subtotal_usd`; coverage and unknown
request counts explain why a subtotal is partial. Subscription quota remains
separate from dollar estimates.

`aishe status` still prints the one-line session spend. Both work in the
non-interactive `-c` form:

```sh
aishe -c "/usage"
```

### Where the numbers come from

Two local files, for two different questions.

`<data>/usage.jsonl` is the **usage ledger**: one line per model turn holding
timestamp, session, model, connection, authentication kind, tokens, usage coverage, cache,
thinking tokens, reported cost, and duration. It carries no prompt, answer,
command, or path, so it is always written and needs no opt-in. `aishe usage`
reads it, which is why the report works with audit logging off.

`<data>/audit.jsonl` is the **audit log**, which stores prompts, answers, and
executed commands, and is off by default. `aishe usage` falls back to it for
history recorded before the ledger existed.

The interactive shell also keeps a per-shell TSV tally. Version 3 persists
request counts, missing-usage counts, complete-usage token subtotals, and pricing
attribution with model and connection identity. Version 2 and older rows still load, but their
missing provenance is treated as unknown; an old zero does not establish a
free call. Older JSON ledger/audit rows receive the same treatment.

The prompt and statusline totals follow the active connection inside the live
AIShe shell; the exit summary remains the whole shell session.

### Plan usage

Where the provider exposes remaining subscription quota, `/usage` and the
statusline show it. Today that is OpenAI subscriptions only, read through the
locally installed `codex` CLI, and reported as best effort: a machine without
`codex` simply shows no plan line rather than a wrong one. API-key connections
have no plan quota; their spend is the cost column.

## How cost is estimated

Cost is derived from token counts and a price table in USD per 1M tokens. aishe
ships a built-in table covering common Claude and GPT models plus a few others.
Setup asks for input and output prices whenever the selected exact model has no
known price. You can inspect and manage overrides later:

```sh
aishe price list
aishe price set gpt-5.6-luna --input 1.25 --output 10.00
aishe price remove gpt-5.6-luna
```

Or override/add a model directly in `[pricing]`:

```toml
[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.60
```

Lookup order for a model's price:

1. an exact key match in `[pricing]` (what `aishe price set` writes),
2. a legacy substring match in `[pricing]`,
3. the built-in table,
4. otherwise unknown.

When a model's price is unknown, aishe still shows token counts but reports the
cost as not available. Native budgeted work requires exact valid pricing; the
legacy substring resolver is used only for labelled display estimates.

## Budgets

Set a session budget to stop calling the model once the estimated cost reaches a
limit. This is handy for keeping a runaway yolo loop in check.

```toml
[aishe]
budget_usd = 0.50      # 0 = unlimited
```

Behavior:

- Native session admission rejects a positive dollar cap when the exact model
  price is unknown or invalid, automatic provider fallbacks obscure billing
  attribution, or any prior session request lacks complete usage. Configure
  exact pricing, use a fixed provider/model, or start a fresh session after
  missing usage. Explicitly setting `budget_usd = 0` removes the money cap.
- Unreadable, malformed, or incomplete session tally records hold positive
  native dollar-budget admission; they cannot silently become zero spend.
- Local inspection such as `/usage` and `/status` remains available after a
  budget rejection. Request, token, and time limits apply independently.
- Native task money caps also require complete recorded coverage on resume.
  Dollar amounts are token-price estimates, not provider billing receipts.

The separately enabled historical managed runtime has its own admission rules:

- The trusted plugin must obtain AIShe authorization before every managed
  provider turn. The bridge reserves the maximum estimated turn cost, caps
  output tokens to the remaining amount, and denies the request before it is
  sent when no safe allowance remains.
- Authoritative provider usage is accepted once per message, including child
  sessions, then replaces the reservation. An abandoned reservation expires
  after a bounded interval so a failed provider cannot lock the session forever.
- A single admitted provider request is not retried through another backend or
  provider after partial output or a tool effect.
- Historical managed usage without coverage remains unknown in reports; native
  session admission does not invent enforcement over that older runtime.

Example of a budget stopping a yolo run:

```
  * create files ...: echo a.txt > a.txt && ...
  budget reached (~$0.50 ≥ $0.50); raise budget_usd to continue
  369 in · 109 out · 1 req · ~$0.0001
```

## Response caching

The native compatibility suggest path can cache responses in memory
for a short window (`cache`, on by default; `cache_ttl_secs`, default 300). Ask
the same thing twice in a row and the second answer is instant and adds no tokens
(a cache hit never calls the model, so the usage line and budget are unchanged).

The cache key includes the freshly-built environment context (cwd, recent
commands, git state), so running anything between two otherwise-identical
requests changes the key and misses the cache — you never get a stale suggestion
after the situation has moved on. Managed conversations rely on their durable
session rather than this native response cache; tool loops are never cached.
Configure the native cache through `aishe settings` or the `cache` field in
`config.toml`. `cache` is not a slash or CLI command.

## Notes on accuracy

- Token counts come straight from the provider's reported usage, including the
  streaming paths.
- Costs are estimates. Providers change prices, and some bill for extras (cached
  input, tool tokens) that the basic table does not model. Use `[pricing]`
  overrides if you need precise figures.
