# aishe

A natural-language-aware shell. `aishe` behaves like zsh for recognizable
commands, but treats anything that *isn't* a command as a natural-language
request handled by an LLM — which either **suggests** a command for you to
confirm, or **executes autonomously** in a tool-use loop.

```
~/projects/app ❯ git status            # runs exactly like zsh
~/projects/app ❯ whats eating my disk  # → LLM suggests: du -sh * | sort -rh | head
```

`aishe` does **not** reimplement a shell grammar. It owns the prompt/input loop
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
cargo build --release   # binary at target/release/aishe
```

On first run, an interactive wizard writes `~/.config/aishe/config.toml`
(provider → API key env var → model → default mode).

Set your API key in the environment (it is **never** stored in the config):

```sh
export ANTHROPIC_API_KEY=sk-ant-...      # or OPENAI_API_KEY=...
aishe
```

Not sure your setup is right? Run `aishe doctor` for a quick check of your shell,
config, resolved front-end, provider, and API key.

---

## Modes

| Mode      | Glyph | Behaviour                                                                    |
|-----------|:-----:|-----------------------------------------------------------------------------|
| `suggest` |  `❯`  | (default) LLM proposes a command; you confirm with `[Enter] / [e]dit / [n]`. |
| `auto`    |  `»`  | LLM commands the safety gate deems **safe** run immediately; **dangerous** ones still require typing `yes`. |
| `yolo`    |  `⚡`  | Agentic loop: the model runs commands, reads output, and iterates until done. |

Switch at any time: `aishe mode auto`, or start with `aishe --mode yolo`.

### Streaming

Enable token streaming with `aishe stream on` (or `stream = true` in config). In
`suggest`/`auto` mode, prose answers then render live as they arrive; once the
model commits to a command instead, aishe falls back to the usual confirm/run
flow (a command is never half-printed). Works with both providers (Anthropic and
OpenAI-compatible SSE); endpoints without SSE simply deliver the answer at once.

### Structured output (reliability)

To get dependable, actionable results from the model, suggest mode asks for a
**strict JSON schema** by default (`structured = "schema"`) on providers that
support it (OpenAI/Groq/…), which guarantees the `{type, command, explanation}`
shape. If a provider rejects it, aishe automatically steps down to a plain JSON
object, then to prompt-only — and **always parses defensively** (unrecognized
output becomes a plain answer, never a crash). yolo mode uses **tool calling**
for the same reason. And regardless of what the model returns, the deterministic
**safety gate** decides what actually runs — the model's output is never trusted
to be safe. Set `structured` to `json` or `prompt` to loosen the constraint.

### Input prefixes

- `?<text>` — force natural-language (e.g. `?how do I find large files`)
- `!<cmd>`  — force shell, bypassing the safety gate (e.g. `!rm -rf build`)

After a command fails, type `?` on the next line to ask the LLM to diagnose the
error.

---

## Meta commands

```
aishe mode [suggest|auto|yolo]   show or set the interaction mode
aishe model [NAME]               show or set the model
aishe provider [anthropic|openai] show or set the provider
aishe editor [emacs|vi]          show or set the line-editor keymap
aishe stream [on|off]            show or toggle token streaming
aishe structured [schema|json|prompt]  output-format strategy (default: schema)
aishe theme [PRESET]             show or set the color preset
aishe config                     print the active config
aishe rehash                     rebuild the command cache
aishe help                       show help
```

Each meta command also works as a **slash-command** (Claude-Code style), e.g.
`/mode auto`, `/config`, `/help` — tab-completable, and `/`-prefixed paths like
`/usr/bin/x` still run normally.

Exit with `exit`, `quit`, or `Ctrl-D`.

---

## Safety gate

Before running an LLM-proposed command (in `suggest`/`auto`, and for `yolo` tool
calls when `yolo_confirm_dangerous = true`), `aishe` screens for irreversible
operations — `rm -rf`, `dd of=/dev/…`, `mkfs`, fork bombs, `curl … | sh`,
`git push --force` to main, `shutdown`/`reboot`, and more. Dangerous commands
print a red panel and require you to type the full word `yes`.

**Path-aware `rm -rf`.** Recursive-force deletes are judged by their targets
(lexically, no filesystem access): a relative, in-tree path like `rm -rf
node_modules`, `rm -rf build dist`, or `rm -rf ./target` is treated as your own
project files and runs without fuss, while anything catastrophic or out-of-tree
is flagged — absolute (`/var`), home (`~`, `$HOME`), a variable, a bare glob
(`*`), or an escaping `..` path. The gate does **not** apply to commands you type
yourself or to `!`-forced lines — it's a shell, not a nanny.

---

## zsh ergonomics

There are three ways to use `aishe`, in increasing order of real-zsh fidelity:

### 0. zsh-PTY front-end — *all* native zsh extensions (default when zsh is present)

```sh
aishe              # "auto" front-end: uses zsh-PTY when zsh is on $PATH
aishe zsh          # or force it: aishe --pty   (or set front_end = "zsh-pty")
aishe --no-pty     # force the built-in reedline editor instead
```

The default `front_end = "auto"` launches your **real interactive zsh inside a
pseudo-terminal** whenever `zsh` is on `$PATH` (falling back to the built-in
reedline editor otherwise). It loads your full `~/.zshrc` and **every plugin you
already use** — `zsh-autosuggestions`, `zsh-syntax-highlighting`,
`fast-syntax-highlighting`, `fzf-tab`, `powerlevel10k`, oh-my-zsh, completions —
completely unmodified. aishe injects a `command_not_found_handler` so
natural-language input is still routed to the LLM (suggested commands pre-fill
your next prompt via `print -z`; set `AISHE_MODE` to `suggest`/`auto`/`yolo`).

Nothing is forked or reimplemented: it's genuinely your zsh, so plugin behavior
is identical to your normal shell.

The hook ergonomics below (auto-run safe via `eval`, force-NL keybinding) apply
here too, since the PTY wrapper injects the same hook.

### 1. Standalone REPL (`aishe`)

`aishe` runs its own line editor ([`reedline`](https://github.com/nushell/reedline),
the editor behind nushell). It reimplements the most-loved zsh niceties
natively:

- **Tab completion** — press `Tab` for a completion menu (`Shift-Tab` to go
  back). Context-aware: command names at the command position (incl. after a
  pipe); environment variables for `$VAR`/`${VAR` (with values shown);
  directories-only for `cd`/`pushd`/`rmdir`; subcommands for `git`/`cargo`/
  `docker`/`npm` (plus live branch names for `git checkout`/`switch`/`merge`/
  `rebase`); `aishe` meta subcommands (with descriptions) and their values; and
  file/directory paths (with `~/` expansion) elsewhere. Matching is
  case-insensitive and falls back to fuzzy subsequence (`gco`→`git-checkout`,
  `dwn`→`Downloads/`) when there's no prefix match.
- **History autosuggestions** (like `zsh-autosuggestions`) — fish-style inline
  hints from your history.
- **History search** — `Ctrl-R` opens a browsable, filterable menu of past
  commands (type to narrow, arrows to pick).
- **History expansion** — zsh-style `!!`, `!$`, `!^`, `!*`, `!-N` (and `!!:N`
  word selection) plus `^old^new` quick substitution. (`!cmd` stays aishe's
  force-shell prefix — `!`-prefix history matching is intentionally not used.)
- **Multi-line continuation** — pressing Enter on an unterminated *shell* line
  (open quote, trailing `\`, unbalanced `(`, an open function body, or an open
  control structure like `for … do` / `if … then`) drops to a continuation line
  instead of submitting, like zsh's `quote>`. Loops, conditionals, `case`, and
  function definitions can be typed across lines and run. Natural-language input
  is never trapped — apostrophes in `what's eating my disk` still submit.
- **Syntax highlighting** (like `zsh-syntax-highlighting`) — the command head is
  colored by whether it's a known command, with distinct colors for flags,
  quoted strings, operators (`| && ; > <`), paths, env assignments, and the
  `?`/`!` sigils. Fully [themeable](#theming).
- **oh-my-zsh aliases & functions** are picked up at startup (`zsh -ic 'alias +;
  …'`) so they're recognized as commands.
- **autocd** (like zsh's `AUTO_CD`) — type a bare directory name (e.g. `src`,
  `..`, `~/projects`) to `cd` into it.
- **Directory stack** — `pushd`/`popd`/`dirs` are intercepted in-process so the
  stack persists across commands (`cd -` returns to the previous directory).
- **emacs or vi keymap** — set `edit_mode = "vi"` (or `aishe editor vi`) for modal
  editing. In vi mode the prompt shows `[I]`/`[N]` for insert/normal; completion
  and `Ctrl-R` history work in both. Takes effect on the next session.
- **Custom prompt** — set `prompt_format` (e.g. `"[{mode}] {cwd}"`) to customize
  the left prompt with `{cwd}`/`{mode}`/`{model}`/`{exit}` placeholders. (For a
  full powerlevel10k/oh-my-zsh prompt, use the zsh-PTY front-end — it renders
  your real zsh prompt.)
- **Git prompt segment** — the right prompt shows the current branch (`⎇ main`),
  read straight from `.git/HEAD` (no `git` process). Disable with
  `git_prompt = false`.

### 2. Native zsh/bash hook (`eval "$(aishe init zsh)"`)

If you want your **real** zsh — with the actual `zsh-autosuggestions`,
`zsh-syntax-highlighting`, oh-my-zsh themes, completions, and ZLE widgets — add
this to your `~/.zshrc` (or `~/.bashrc` with `init bash`):

```sh
eval "$(aishe init zsh)"
```

This installs a `command_not_found_handler` (zsh) / `command_not_found_handle`
(bash) that routes anything that isn't a command to `aishe`. Your shell's line
editor is **untouched**, so every native plugin works exactly as before. Set
`AISHE_MODE=suggest|auto|yolo` to control behavior.

> **How it works:** shells run the not-found handler in a *subshell*, so it can't
> touch the line editor or shell state directly. aishe writes the suggested
> action to a temp file and a `precmd` (zsh) / `PROMPT_COMMAND` (bash) hook acts
> on it in the **main** shell — which is what makes prefill and state-changes work.

- **suggest**: zsh pre-fills your next prompt (`print -z`) to confirm/edit; bash
  prints the suggestion (recall it with `Ctrl-X Ctrl-R`).
- **auto**: a command the safety gate deems **safe** is run in your real shell —
  so `cd`/`export` persist and it's recorded in history — while a **dangerous**
  one is offered for review instead.
- **yolo**: the agentic loop runs directly.

**force-NL keybinding.** Sometimes your input *is* a valid command but you mean
it as natural language. Press **Alt-Enter** (zsh) or **Ctrl-G** (bash) to send
the current line to the LLM as a request and replace it with the suggestion.
Override the zsh key with `AISHE_NL_KEY` (a `bindkey` sequence), e.g.
`export AISHE_NL_KEY='^o'`.

> This hook approach is intentionally chosen over wrapping zsh in a PTY: it
> gives the same "real ZLE + native plugins" result without a second terminal
> layer, SIGWINCH/IPC plumbing, or fighting the shell's own editor.

## Theming

Prompt and highlighter colors are configurable via a `[theme]` section. Pick a
preset (`default`, `vivid`, `mono`, `nord`, `gruvbox`) — switch live-ish with
`aishe theme nord` (applies on restart) — and/or override individual roles.
Colors may
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

## Startup file (`.aishrc`)

aishe sources `~/.aishrc` and `~/.config/aishe/aishrc` (in that order) into
**every** delegated command, so aliases, functions, and exports you define there
are available to all commands and recognized at the prompt:

```sh
# ~/.aishrc
alias gs='git status'
alias ll='ls -lah'
export EDITOR=nvim
gco() { git checkout "$@"; }   # functions work here too
```

This is shell-agnostic setup that applies in both front-ends (the zsh-PTY
front-end of course also runs your real `~/.zshrc`).

Aliases, shell options (`setopt`/`unsetopt`), **and functions** you define
**interactively** also persist to later commands in the reedline front-end —
aishe replays the definition via the same mechanism (multi-line `name() { … }`
bodies continue until the braces close, then become callable).

---

## Configuration reference

`~/.config/aishe/config.toml`:

```toml
[aishe]
mode = "suggest"               # suggest | auto | yolo
provider = "anthropic"         # anthropic | openai
front_end = "auto"             # auto (zsh-pty if zsh present, else reedline) | reedline | zsh-pty
edit_mode = "emacs"            # emacs | vi (reedline line-editor keymap)
yolo_confirm_dangerous = true  # confirm dangerous commands even in yolo
max_yolo_iterations = 10
show_right_prompt = true        # show "model · mode" on the right
git_prompt = true               # show "⎇ branch" in the right prompt (reedline)
# prompt_format = "[{mode}] {cwd}"  # custom reedline left prompt: {cwd} {mode} {model} {exit}
stream = false                  # stream answers token-by-token (suggest/auto)
structured = "schema"           # suggest output: schema (strict) | json | prompt

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

- **Persistent state.** `cd`, `export`, `unset`, and `source` are intercepted so
  state persists across delegated commands; aliases, `setopt`, and functions —
  whether from `~/.aishrc` or defined interactively — persist too (see [Startup
  file](#startup-file-aishrc)). The remaining gap is functions/aliases created by
  a `source`d *file* (only its env diff is captured); put those in `.aishrc` or
  use the zsh-PTY front-end.
- **No job control.** `Ctrl-Z` / `bg` / `fg` for delegated processes are not
  supported. `Ctrl-C` reaches the foreground child; `aishe` itself survives.
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
cargo build --release && python3 tests/pty_smoke.py target/release/aishe
```

### Manual checklist

- [ ] History persists across restarts (`~/.local/share/aishe/history`).
- [ ] `Ctrl-C` at the prompt clears the line; `aishe` survives `Ctrl-C` during a child.
- [ ] `vim` / `ssh` / `top` open and behave normally.
- [ ] First-run wizard creates a config; `aishe config` reflects edits.
```
