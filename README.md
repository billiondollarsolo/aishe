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
llmsh editor [emacs|vi]          show or set the line-editor keymap
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

There are three ways to use `llmsh`, in increasing order of real-zsh fidelity:

### 0. zsh-PTY front-end — *all* native zsh extensions (default when zsh is present)

```sh
llmsh              # "auto" front-end: uses zsh-PTY when zsh is on $PATH
llmsh zsh          # or force it: llmsh --pty   (or set front_end = "zsh-pty")
llmsh --no-pty     # force the built-in reedline editor instead
```

The default `front_end = "auto"` launches your **real interactive zsh inside a
pseudo-terminal** whenever `zsh` is on `$PATH` (falling back to the built-in
reedline editor otherwise). It loads your full `~/.zshrc` and **every plugin you
already use** — `zsh-autosuggestions`, `zsh-syntax-highlighting`,
`fast-syntax-highlighting`, `fzf-tab`, `powerlevel10k`, oh-my-zsh, completions —
completely unmodified. llmsh injects a `command_not_found_handler` so
natural-language input is still routed to the LLM (suggested commands pre-fill
your next prompt via `print -z`; set `LLMSH_MODE` to `suggest`/`auto`/`yolo`).

Nothing is forked or reimplemented: it's genuinely your zsh, so plugin behavior
is identical to your normal shell.

The hook ergonomics below (auto-run safe via `eval`, force-NL keybinding) apply
here too, since the PTY wrapper injects the same hook.

### 1. Standalone REPL (`llmsh`)

`llmsh` runs its own line editor ([`reedline`](https://github.com/nushell/reedline),
the editor behind nushell). It reimplements the most-loved zsh niceties
natively:

- **Tab completion** — press `Tab` for a completion menu (`Shift-Tab` to go
  back). Completes command names (from the `$PATH`/builtin/alias cache) at the
  command position, and file/directory paths (with `~/` expansion) in argument
  position.
- **History autosuggestions** (like `zsh-autosuggestions`) — fish-style inline
  hints from your history.
- **History search** — `Ctrl-R` opens a browsable, filterable menu of past
  commands (type to narrow, arrows to pick).
- **Multi-line continuation** — pressing Enter on an unterminated *shell* line
  (open quote, trailing `\`, or unbalanced `(`) drops to a continuation line
  instead of submitting, like zsh's `quote>`. Natural-language input is never
  trapped — apostrophes in `what's eating my disk` still submit normally.
- **Syntax highlighting** (like `zsh-syntax-highlighting`) — the command head is
  colored by whether it's a known command, with distinct colors for flags,
  quoted strings, operators (`| && ; > <`), paths, env assignments, and the
  `?`/`!` sigils. Fully [themeable](#theming).
- **oh-my-zsh aliases & functions** are picked up at startup (`zsh -ic 'alias +;
  …'`) so they're recognized as commands.
- **emacs or vi keymap** — set `edit_mode = "vi"` (or `llmsh editor vi`) for modal
  editing. In vi mode the prompt shows `[I]`/`[N]` for insert/normal; completion
  and `Ctrl-R` history work in both. Takes effect on the next session.

### 2. Native zsh/bash hook (`eval "$(llmsh init zsh)"`)

If you want your **real** zsh — with the actual `zsh-autosuggestions`,
`zsh-syntax-highlighting`, oh-my-zsh themes, completions, and ZLE widgets — add
this to your `~/.zshrc` (or `~/.bashrc` with `init bash`):

```sh
eval "$(llmsh init zsh)"
```

This installs a `command_not_found_handler` that routes anything that isn't a
command to `llmsh`. Your shell's line editor is **untouched**, so every native
plugin works exactly as before. When you type natural language, `llmsh` suggests
a command and pre-fills your next prompt (`print -z`) for you to confirm or edit
— and because it runs in your real shell, `cd`/`export` state persists normally.
Set `LLMSH_MODE=suggest|auto|yolo` to control behavior.

**auto-run safe via `eval` (zsh).** In `auto` mode the hook asks
`llmsh --auto-line`, which classifies the suggested command with the safety gate.
Safe commands are `eval`'d directly in your real shell — so `cd`/`export` stick
and the command is recorded in history — while dangerous ones are pre-filled
(`print -z`) for you to review. (bash runs `command_not_found_handle` in a
subshell, where eval'd state wouldn't persist, so bash keeps the pre-fill path in
auto mode.)

**force-NL keybinding.** Sometimes your input *is* a valid command but you mean
it as natural language. Press **Alt-Enter** (zsh) or **Ctrl-G** (bash) to send
the current line to the LLM as a request and replace it with the suggestion.
Override the zsh key with `LLMSH_NL_KEY` (a `bindkey` sequence), e.g.
`export LLMSH_NL_KEY='^o'`.

> This hook approach is intentionally chosen over wrapping zsh in a PTY: it
> gives the same "real ZLE + native plugins" result without a second terminal
> layer, SIGWINCH/IPC plumbing, or fighting the shell's own editor.

## Theming

Prompt and highlighter colors are configurable via a `[theme]` section. Pick a
preset (`default`, `vivid`, `mono`) and/or override individual roles. Colors may
be named (`red`, `bright-green`, `purple`), a palette index (`0`–`255`), or hex
(`#ff8800`):

```toml
[theme]
preset = "vivid"
cwd = "bright-cyan"
known_cmd = "#98c379"
unknown_cmd = "red"
flag = "yellow"
string = "green"
operator = "magenta"
path = "blue"
```

Roles: `cwd`, `glyph_ok`, `glyph_err`, `right_prompt`, `known_cmd`,
`unknown_cmd`, `flag`, `string`, `operator`, `path`, `assignment`, `sigil_nl`,
`sigil_shell`.

---

## Configuration reference

`~/.config/llmsh/config.toml`:

```toml
[llmsh]
mode = "suggest"               # suggest | auto | yolo
provider = "anthropic"         # anthropic | openai
front_end = "auto"             # auto (zsh-pty if zsh present, else reedline) | reedline | zsh-pty
edit_mode = "emacs"            # emacs | vi (reedline line-editor keymap)
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

# zsh-PTY front-end smoke test (drives a real zsh through a pseudo-terminal;
# needs `zsh` + python3, no API key). Runs in CI on every push.
cargo build --release && python3 tests/pty_smoke.py target/release/llmsh
```

### Manual checklist

- [ ] History persists across restarts (`~/.local/share/llmsh/history`).
- [ ] `Ctrl-C` at the prompt clears the line; `llmsh` survives `Ctrl-C` during a child.
- [ ] `vim` / `ssh` / `top` open and behave normally.
- [ ] First-run wizard creates a config; `llmsh config` reflects edits.
```
