# llmsh

A natural-language-aware shell. `llmsh` behaves like zsh for recognizable
commands, but treats anything that *isn't* a command as a natural-language
request handled by an LLM — which either **suggests** a command for you to
confirm, or **executes autonomously** in a tool-use loop.

```
~/projects/app ❯ git status            # runs exactly like zsh
~/projects/app ❯ whats eating my disk  # → LLM suggests: du -sh * | sort -rh | head
```

`llmsh` does **not** reimplement a shell grammar. It owns the prompt/input loop
and delegates execution of shell lines to `zsh -c` (falling back to `bash -c`),
so pipes, globs, redirection, subshells, and interactive child programs (vim,
ssh, top) all work unmodified.

---

## Install

Requires Rust ≥ 1.80 and `zsh` or `bash` on your `PATH`. Targets macOS
(arm64/x86_64) and Linux (x86_64/arm64).

```sh
cargo install --path .
# or, from a checkout:
cargo build --release   # binary at target/release/llmsh
```

On first run, an interactive wizard writes `~/.config/llmsh/config.toml`
(provider → API key env var → model → default mode).

Set your API key in the environment (it is **never** stored in the config):

```sh
export ANTHROPIC_API_KEY=sk-ant-...      # or OPENAI_API_KEY=...
llmsh
```

---

## Modes

| Mode      | Glyph | Behaviour                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | (default) LLM proposes a command; you confirm with `[Enter] / [e]dit / [n]`. |
| `auto`    |  `»`  | LLM commands the safety gate deems **safe** run immediately; **dangerous** ones still require typing `yes`. |
| `yolo`    |  `⚡`  | Agentic loop: the model runs commands, reads output, and iterates until done. |

Switch at any time: `llmsh mode auto`, or start with `llmsh --mode yolo`.

### Input prefixes

- `?<text>` — force natural-language (e.g. `?how do I find large files`)
- `!<cmd>`  — force shell, bypassing the safety gate (e.g. `!rm -rf build`)

After a command fails, type `?` on the next line to ask the LLM to diagnose the
error.

---

## Meta commands

```
llmsh mode [suggest|auto|yolo]   show or set the interaction mode
llmsh model [NAME]               show or set the model
llmsh provider [anthropic|openai] show or set the provider
llmsh config                     print the active config
llmsh rehash                     rebuild the command cache
llmsh help                       show help
```

Exit with `exit`, `quit`, or `Ctrl-D`.

---

## Safety gate

Before running an LLM-proposed command (in `suggest`/`auto`, and for `yolo` tool
calls when `yolo_confirm_dangerous = true`), `llmsh` screens for irreversible
operations — `rm -rf`, `dd of=/dev/…`, `mkfs`, fork bombs, `curl … | sh`,
`git push --force` to main, `shutdown`/`reboot`, and more. Dangerous commands
print a red panel and require you to type the full word `yes`.

The gate is intentionally conservative: in v0.1 `rm -rf` is **always** flagged,
even `rm -rf node_modules`. The gate does **not** apply to commands you type
yourself or to `!`-forced lines — it's a shell, not a nanny.

---

## zsh ergonomics

`llmsh` uses [`reedline`](https://github.com/nushell/reedline) (the line editor
behind nushell), not zsh's ZLE, so native zsh plugins cannot be loaded
directly. Instead it provides equivalents to the most popular ones:

- **History autosuggestions** (like `zsh-autosuggestions`) — fish-style inline
  hints from your history.
- **Command syntax highlighting** (like `zsh-syntax-highlighting`) — the command
  head is green when it's a known command, red when unknown.
- **oh-my-zsh aliases & functions** are picked up: at startup `llmsh` queries
  your interactive shell (`zsh -ic 'alias +; …'`) so your aliases and functions
  are recognized as commands and dispatch to the shell. (ZLE widgets, prompt
  themes, and completion definitions do **not** carry over.)

---

## Configuration reference

`~/.config/llmsh/config.toml`:

```toml
[llmsh]
mode = "suggest"               # suggest | auto | yolo
provider = "anthropic"         # anthropic | openai
yolo_confirm_dangerous = true  # confirm dangerous commands even in yolo
max_yolo_iterations = 10
show_right_prompt = true        # show "model · mode" on the right
stream = false                  # reserved (SSE streaming, deferred)

[providers.anthropic]
base_url = "https://api.anthropic.com"
api_key_env = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514"

[providers.openai]
base_url = "https://api.openai.com"   # point at Ollama/OpenRouter/Together here
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
```

API keys are read **only** from the named environment variables.

---

## Limitations (v0.1)

- **Persistent state caveats.** `cd`, `export`, `unset`, and `source` are
  intercepted so state persists across delegated commands. But aliases and
  functions defined by a `source`d file (or your `.zshrc`) do **not** persist
  into later commands, since each line runs in a fresh `zsh -c`.
- **No job control.** `Ctrl-Z` / `bg` / `fg` for delegated processes are not
  supported. `Ctrl-C` reaches the foreground child; `llmsh` itself survives.
- **yolo runs with stdin closed.** Autonomous commands are non-interactive
  (stdin is `/dev/null`); the model is instructed to always use non-interactive
  flags. Captured output is tee'd to your terminal and truncated to the last
  8,000 characters for the model. Commands time out after 120s.
- **Naive pipeline parsing.** Operator splitting (`|`, `&&`, `;`) for routing
  decisions ignores quoting.
- **Unix only.** No Windows support.

---

## Development

```sh
cargo build
cargo test            # unit + integration (integration tests spawn a real shell)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

### Manual checklist

- [ ] History persists across restarts (`~/.local/share/llmsh/history`).
- [ ] `Ctrl-C` at the prompt clears the line; `llmsh` survives `Ctrl-C` during a child.
- [ ] `vim` / `ssh` / `top` open and behave normally.
- [ ] First-run wizard creates a config; `llmsh config` reflects edits.
```
