# Changelog

All notable changes to **aishe** are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com/); this project is pre-1.0, so
breaking changes can land in any release.

## [Unreleased]

### Fixed
- **Shell hook suggest-mode now actually prefills.** zsh/bash run the
  `command_not_found` handler in a *subshell*, so the old `print -z`/`READLINE_LINE`
  (and auto-mode `eval`) silently did nothing. The handler now hands off via a
  temp file to a `precmd` (zsh) / `PROMPT_COMMAND` (bash) hook that runs in the
  main shell — so suggestions prefill and safe auto commands run with `cd`/`export`
  persisting. (Found via live testing against a real model.)
- **Honor the system trust store** (`ureq` `native-certs`), so aishe works behind
  corporate / TLS-inspecting proxies whose CA isn't in the bundled root set.
- **No more spurious `LLM disabled` warning** on startup for purely-local use; the
  notice is shown once in the interactive REPL (or at the point an NL request needs
  the provider).

### Added
- **zsh-PTY front-end** — drive your real interactive zsh inside a pseudo-terminal
  with every native plugin (zsh-autosuggestions, zsh-syntax-highlighting, fzf-tab,
  powerlevel10k, oh-my-zsh) unmodified. Now the **default** (`front_end = "auto"`)
  when zsh is on `$PATH`, falling back to the built-in reedline editor.
- **reedline editor parity** — context-aware tab completion (commands, file
  paths, `$VAR` env vars, directories-only for `cd`/`pushd`, `aishe`
  subcommands/values, and per-command subcommands for git/cargo/docker/npm with
  live git-branch completion), multi-line continuation for **control structures**
  (`for`/`while`/`if`/`case`) and function definitions, `Ctrl-R`
  history-search menu, multi-line continuation validator, emacs/vi keymaps
  (`edit_mode`, with `[I]`/`[N]` prompt tags in vi mode), and zsh-style history
  expansion (`!!`, `!$`, `!-N`, `^old^new`, …), **autocd** (bare directory name →
  cd), and a **directory stack** (`pushd`/`popd`/`dirs`).
- **Native shell hook niceties** — `auto`-mode runs safe commands directly via
  `eval` (state persists), dangerous ones are pre-filled; force-NL keybinding
  (Alt-Enter / Ctrl-G).
- **Token streaming** of suggest/auto answers (`stream` / `aishe stream`) over
  Anthropic and OpenAI-compatible SSE.
- **`.aishrc` startup file** sourced into every command, plus persistence of
  interactively-defined aliases, shell options, and **functions** (multi-line
  `name() { … }`) across the reedline front-end.
- **Custom prompt** (`prompt_format`), a **git branch segment** in the right
  prompt (`git_prompt`, read from `.git/HEAD`), and **theme presets** (`default`,
  `vivid`, `mono`, `nord`, `gruvbox`) with an `aishe theme` command.
- **Fuzzy completion** — case-insensitive matching with a subsequence fallback
  (`gco`→`git-checkout`).
- **Structured output** for suggest mode (`structured` = `schema` | `json` |
  `prompt`, default strict `schema`): requests a strict JSON Schema on providers
  that support it, auto-stepping down (schema → json → prompt) when a provider
  rejects it, on top of the existing defensive parsing. Configurable live via
  `aishe structured`.
- **`aishe doctor`** — environment check for shell, config, front-end, provider,
  and API key.
- **Slash-commands** — every meta command also works as `/mode`, `/config`, … and
  **user-defined plugins/skills**: Markdown command files in
  `~/.config/aishe/commands/` and `<project>/.aishe/commands/` (Claude-Code
  style) define custom `/commands` that run as shell snippets or NL prompt
  templates (`$ARGUMENTS`/`$1`), with `/commands` listing and tab-completion.
- **Skills (model-invoked)** — Markdown skill files in `~/.config/aishe/skills/`
  / `<project>/.aishe/skills/` are advertised to the model in yolo mode; it pulls
  a skill's full instructions into context on demand via a `use_skill` tool
  (Claude-Code-style progressive disclosure).
- **CI** — cross-platform tests plus PTY smoke tests for both front-ends.

### Changed
- **Renamed the project from `llmsh` to `aishe`** — binary, command, config dir
  (`~/.config/aishe`), `AISHE_*` env vars, and shell hooks. A one-time migration
  imports a pre-rename `~/.config/llmsh/config.toml` automatically.
- **Path-aware `rm -rf`** — relative, in-tree targets (e.g. `rm -rf node_modules`)
  are no longer flagged; absolute/home/variable/glob/escaping targets still are.

## [0.1.0]

- Initial natural-language-aware shell: behaves like zsh for real commands,
  routes anything else to an LLM (suggest / auto / yolo), with a conservative
  safety gate. Anthropic + OpenAI-compatible providers, themable reedline editor,
  and a native `command_not_found` hook.
