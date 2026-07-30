# Token usage and cost

aishe meters every model call so you can see what a session costs and cap it.

## What you see

The interactive shell keeps a live status display. By default it appears in the
right prompt and shows model, mode, backend, scope, session cost, and request
count. You can
place it under the prompt or turn it off, and choose its ordered fields during
setup or in `aishe settings`.

```
  436 in · 119 out · 1 req · ~$0.0001
```

Available fields are `model`, `mode`, `backend`, `scope`, `network`, `sandbox`,
`task`, `elapsed`, `context`, `last_tokens`, `last_cost`, `session_tokens`,
`session_cost`, `budget`, and `requests`. `context` is the latest provider-turn
input-token count, not a guessed percentage. A detailed status can render:

```
gpt-5.6-luna · auto · opencode · workspace · context 8.4K tok ·
last 1,697/374 tok · session cost ~$0.0112 · 2 reqs
```

The display refreshes after each call. `right` preserves the compact shell-like
layout; `below` is better for narrow terminals or detailed metrics; `off` hides
it. `show_usage = false` disables usage output, while
`status_line_position = "off"` hides only the live prompt line.

```toml
[aishe]
show_usage = false
```

### Whole-session summary

The interactive zsh front-end runs each natural-language line as its own process,
so when you exit the shell aishe prints a single dim line totalling the whole
session across every call:

```
aishe session: 18,204 in · 5,130 out · 9 reqs · ~$0.0731
```

Cost is summed per call using each command's own model price (so a session that
spans models stays accurate; any models without a known price are disclosed as
`(+N unpriced)`). It's gated on the same `show_usage` toggle and appears only when
at least one model call was made.

You can also ask for the total at any time:

```sh
aishe usage        # or /usage
aishe status       # session settings plus live spend
```

Both work in the non-interactive `-c` form too:

```sh
aishe -c "/usage"
```

When audit logging is enabled, `aishe usage --by model|day|session` reads
persisted totals. The prompt/statusline aggregation remains scoped to the live
Aishe shell.

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
cost as not available, and budget enforcement is skipped (it cannot price what it
does not know).

## Budgets

Set a session budget to stop calling the model once the estimated cost reaches a
limit. This is handy for keeping a runaway yolo loop in check.

```toml
[aishe]
budget_usd = 0.50      # 0 = unlimited
```

Behavior:

- The trusted plugin must obtain Aishe authorization before every managed
  provider turn. The bridge reserves the maximum estimated turn cost, caps
  output tokens to the remaining amount, and denies the request before it is
  sent when no safe allowance remains.
- Authoritative provider usage is accepted once per message, including child
  sessions, then replaces the reservation. An abandoned reservation expires
  after a bounded interval so a failed provider cannot lock the session forever.
- A single admitted provider request is not retried through another backend or
  provider after partial output or a tool effect.
- Only enforced when the model's price is known.

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
Toggle by typing `cache on` / `cache off` at the aishe prompt — a
[prompt-only meta command](commands.md#prompt-only-meta-commands), not an `aishe`
subcommand — or set `cache` in your config.

## Notes on accuracy

- Token counts come straight from the provider's reported usage, including the
  streaming paths.
- Costs are estimates. Providers change prices, and some bill for extras (cached
  input, tool tokens) that the basic table does not model. Use `[pricing]`
  overrides if you need precise figures.
