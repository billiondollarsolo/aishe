# Token usage and cost

aishe meters every model call so you can see what a session costs and cap it.

## What you see

After each model interaction, aishe prints a dim line like:

```
  436 in · 119 out · 1 req · ~$0.0001
```

This is the running session total: input tokens, output tokens, number of
requests, and the estimated cost. Turn it off with:

```toml
[aishe]
show_usage = false
```

You can also ask for the total at any time:

```sh
aishe usage        # or /usage
```

Both work in the non-interactive `-c` form too:

```sh
aishe -c "/usage"
```

A fresh process starts at zero, so `aishe usage` reflects the current session
only.

## How cost is estimated

Cost is derived from token counts and a price table in USD per 1M tokens. aishe
ships a built-in table covering common Claude and GPT models plus a few others.
You can override or add any model in `[pricing]`:

```toml
[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.60
```

Lookup order for a model's price:

1. an exact key match in `[pricing]`,
2. a substring match in `[pricing]`,
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

- Checked before each model call. When the accrued cost reaches the budget, aishe
  prints a notice and stops rather than making another call.
- A single in-flight call is never interrupted mid-request. The check happens
  between calls (for example between yolo iterations).
- Only enforced when the model's price is known.

Example of a budget stopping a yolo run:

```
  ⚡ create files ...: echo a.txt > a.txt && ...
  budget reached (~$0.50 ≥ $0.50); raise budget_usd to continue
  369 in · 109 out · 1 req · ~$0.0001
```

## Notes on accuracy

- Token counts come straight from the provider's reported usage, including the
  streaming paths.
- Costs are estimates. Providers change prices, and some bill for extras (cached
  input, tool tokens) that the basic table does not model. Use `[pricing]`
  overrides if you need precise figures.
