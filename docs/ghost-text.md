# Inline AI ghost text

In the reedline front-end, aishe can show an inline AI suggestion (ghost text) as
you type: a dim continuation of your current command line that you accept with the
Right arrow, like a history hint but predicted by the model.

It is **off by default** because it spends tokens as you type. Turn it on with:

```sh
aishe ghost on      # or ghost_text = true in config
aishe ghost off
aishe ghost         # show current state
```

## How it works

- As you type, a background worker asks the model for the most likely full
  command line for your current input and caches it.
- The reedline hinter shows the remainder of that prediction as dim ghost text.
- Accept the whole suggestion with the Right arrow (at the end of the line), or a
  single word with Ctrl-Right, exactly like history hints.
- When there is no AI prediction yet, aishe falls back to ordinary history hints.

The model call runs on a background thread (debounced and cached), so typing
never blocks on the network. The worker shares the main provider, so ghost tokens
count in the same `aishe usage` total and respect the same `budget_usd`. Ghost
requests are also written to the audit log with `mode: ghost` (so you can filter
or audit them).

## Responsiveness note

reedline only repaints the line on input events, so a prediction that finishes
while you have paused appears on your next keystroke. In practice the ghost tracks
your prefix as you type: pause briefly after a few characters and the suggestion
shows as you continue. This is a deliberate, robust trade-off (no flicker, no
background redraw hacks).

## Cost

Ghost text issues a model request each time you pause typing on a non-trivial
line. To keep this bounded it is:

- off by default and opt-in,
- debounced (it waits for a short typing pause),
- skipped for very short inputs,
- cached per prefix (no repeat requests for the same text),
- subject to `budget_usd` (it stops predicting once the session budget is hit).

Even so, leaving it on during heavy editing costs tokens. Watch `aishe usage`, set
a `budget_usd`, and consider a cheap, fast model for the provider. See
[Token usage and cost](usage-and-cost.md).

## Limitations

- reedline front-end only (the zsh-PTY front-end uses your real zsh, which has its
  own autosuggestion plugins).
- The ghost worker is started at session start with the current provider; switch
  the model or provider and restart aishe for ghost to use it.
- Prediction quality depends on the model. A small, fast model gives snappier,
  cheaper suggestions.
