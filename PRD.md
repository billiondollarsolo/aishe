# PRD: `llmsh` — A Natural-Language-Aware Shell (Rust)

**Version:** 1.1
**Status:** Implemented (v0.1)
**Target:** Working v0.1, single repo, single static binary, `cargo install llmsh`

> This document is the design spec that `llmsh` v0.1 was built against. For
> usage, see [README.md](README.md).

## 1. Overview

`llmsh` is an interactive shell REPL that behaves like zsh for valid shell
commands, but treats anything that is *not* a recognizable command as a
natural-language request, interpreted by an LLM (Anthropic Messages API or any
OpenAI-compatible Chat Completions API). The LLM either **suggests** a command
for confirmation, or **executes autonomously** ("yolo mode") via a tool-use
loop.

`llmsh` does NOT reimplement a shell grammar. It owns the prompt/input loop and
**delegates execution of shell lines to `zsh -c`** (fallback `bash -c`), so
pipes, globs, redirection, subshells, and interactive child programs (vim, ssh,
top) work unmodified.

### Modes

- `suggest` (default) — confirm before running.
- `auto` — auto-run commands the safety gate classifies as safe; confirm
  dangerous ones. (Added beyond the original spec.)
- `yolo` — agentic tool loop.

## 2. Tech Stack

reedline (line editor), ureq (sync HTTP, rustls), serde/serde_json, toml,
crossterm, termimad, clap, anyhow + thiserror, regex, dirs, libc (non-fatal
SIGINT handler), nu-ansi-term (prompt/highlight colors), portable-pty (zsh-PTY
front-end). Dev: assert_cmd, mockito, predicates. No async runtime, no vendor
SDK crates.

### Front-ends

- `reedline` (default) — built-in editor with native autosuggestions, tab
  completion (command names + file paths), `Ctrl-R` history-search menu,
  multi-line continuation for unterminated shell lines, themeable syntax
  highlighting, and a custom prompt.
- `zsh-pty` (`llmsh zsh` / `--pty` / `front_end = "zsh-pty"`) — drives the user's
  real interactive zsh inside a pseudo-terminal with their full config and all
  plugins loaded; injects a `command_not_found_handler` for NL routing. No
  plugin is forked or reimplemented.
- Native hook (`eval "$(llmsh init zsh|bash)"`) — same `command_not_found`
  routing inside the user's own shell session.

Hook ergonomics (shared by `zsh-pty` and the native zsh hook):
- **auto-run safe via `eval`.** In `auto` mode the hook calls `llmsh --auto-line`,
  which prints the suggested command and exits `0` if the safety gate deems it
  safe (hook `eval`s it in the real shell; `cd`/`export` persist, recorded in
  history) or a non-zero code if dangerous (hook pre-fills it for review). bash
  runs `command_not_found_handle` in a subshell, so it keeps the pre-fill path.
- **force-NL keybinding.** A ZLE widget (zsh, default Alt-Enter, `LLMSH_NL_KEY`
  override) / `bind -x` on Ctrl-G (bash) routes the current line to the LLM as
  natural language even when it is also a valid command.

The `zsh-pty` front-end is exercised in CI by a PTY smoke test
(`tests/pty_smoke.py`) that drives a real zsh and asserts the wrapper proxies
native commands, installs the hook (incl. the auto path), and binds the force-NL
widget — all without an API key.

## 3. Repository Layout

```
src/
  main.rs           clap args, init, REPL loop
  lib.rs            library facade (lets tests reach internals)
  dispatcher.rs     command-vs-NL detection + command cache
  executor.rs       zsh -c delegation, intercepted builtins, state
  context.rs        LLM context block builder
  safety.rs         destructive-command gate
  config.rs         load/save TOML, first-run wizard
  prompt.rs         reedline Prompt impl
  highlight.rs      reedline Highlighter (syntax highlighting)
  modes/            mod.rs, suggest.rs, yolo.rs
  providers/        mod.rs, anthropic.rs, openai_compat.rs
tests/
  dispatcher.rs safety.rs executor.rs providers.rs modes.rs cli.rs
```

## 4. Functional Specification

See the inline module documentation for the authoritative behavior. Highlights:

- **Dispatcher** routes by: forced `?`/`!` prefixes, intercepted builtins,
  shell-syntax signals, env assignments, pipeline head checks, command cache,
  else natural language. The cache is built from a synchronous `$PATH` scan plus
  a background fetch of shell builtins, aliases, and functions.
- **Executor** delegates lines to the backing shell with inherited stdio;
  intercepts `cd`/`export`/`unset`/`source` to persist state; provides captured
  execution (tee + 8k truncation + 120s timeout, stdin closed) for yolo.
- **Safety gate** screens each operator-split segment against a table of
  conservative regexes; dangerous commands require typing `yes`.
- **Providers** expose a small `Provider` trait with `complete` and
  `complete_with_tools`, implemented for the Anthropic Messages API and any
  OpenAI-compatible Chat Completions API. 60s timeout, one retry on 429/5xx,
  clear 401 messages.

## 5. Non-Goals (v0.1)

Full in-editor shell scripting, job control for delegated processes, Windows,
plugins/completions for arbitrary CLIs, PTY-based yolo capture, SSE streaming
(config flag reserved), path-aware `rm -rf` risk analysis.
