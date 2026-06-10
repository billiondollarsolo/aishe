# Changelog

All notable changes to **aishe** are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com/); this project is pre-1.0, so
breaking changes can land in any release.

## [Unreleased]

### Fixed
- **More dispatcher edge cases no longer misroute to the LLM** (found by the
  expanded validation harness): scalar assignments with quoted or
  command-substituted values that contain spaces (`v='a b'`, `x=$(cmd args)`),
  array-element assignments (`m[k]=v`), `|` inside arithmetic/command
  substitution (`echo $((7 | 8))`), the `>|` clobber redirect, and the
  `unsetopt`/`integer`/`float` builtins. `split_top_level` is now paren-depth
  aware, and assignment-head detection handles any value.
- **Builtins no longer misroute to the LLM in one-shot/`-c` (and the first
  interactive prompt).** The shell-builtin list was fetched on a background
  thread, so `aishe -c 'print …'` / `let` / `typeset` / `jobs` / `:` could race
  the thread and be sent to the model. The fallback builtin set is now seeded
  **synchronously** at startup. (Caught by the expanded validation harness.)
- **zsh array assignments route to shell.** `arr=(a b c)` (spaces inside the
  parens) and `path+=(/x)` were tokenized into a bare value head and misrouted
  to the LLM; they're now recognized as shell. Added `repeat`/`:`/`noglob` to
  the builtin/keyword sets too.
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
- **Built-in file tools for yolo.** Beyond `run_command`, the agentic loop can
  now call `read_file`, `write_file`, `edit_file`, and `list_dir` to work with
  files precisely instead of round-tripping through `cat`/`sed`/heredocs. Writes
  outside the working tree are confirmed (when `yolo_confirm_dangerous`), and each
  call is audit-logged. On by default (`file_tools`).
- **Spelling correction and named directories (reedline).** With `correct = true`
  (zsh `CORRECT`), a mistyped first word that is a near-miss of a known command
  prompts `correct 'gti' to 'git'? [Y/n]` instead of going to the LLM (uses
  Damerau-Levenshtein so transpositions count as one typo). `[named_dirs]` adds
  `~name` expansion in `cd` (`cd ~proj`, `cd ~proj/app`).
- **Deeper zsh parity (reedline prompt, navigation, and history).** The right
  prompt now shows the **last command's duration** (`report_time`, like zsh
  REPORTTIME) and a richer **git segment**: `+` staged, `*` unstaged, `⇡N`/`⇣N`
  ahead/behind, and `⚑N` stashes (`git_status`). Navigation: **`AUTO_PUSHD`**
  (`auto_pushd`, with `cd -N`/`cd +N` and `dirs -v`) and **`cdpath`** (`cd <name>`
  searches extra base dirs / `$CDPATH`). History: **`HIST_IGNORE_DUPS`**
  (`hist_ignore_dups`, default on), **`HIST_IGNORE_SPACE`** (`hist_ignore_space`),
  and **`HISTIGNORE`** glob patterns (`hist_ignore`). (Full job control remains
  the zsh-PTY front-end's domain, where it works natively.)
- **Inline AI ghost text.** In the reedline front-end, aishe can predict the rest
  of your command as you type and show it as dim ghost text (accept with the Right
  arrow), Copilot/Warp style. A background worker (debounced, cached) keeps typing
  non-blocking; it shares the main provider so ghost tokens count in `aishe usage`
  and respect `budget_usd`, and the calls are audit-logged as `mode: ghost`. Off
  by default; toggle with `aishe ghost on` / `ghost_text`.
- **Hardened the safety gate against bypasses, with an adversarial corpus.** The
  gate now strips leading wrappers and env assignments before judging a command
  (`sudo -i rm -rf /`, `FOO=bar rm -rf /`, `env`/`time`/`nohup`/`nice`/`timeout`
  prefixes) and unquotes `rm` targets (`rm -rf "$HOME"`, `rm -rf '/'`), closing
  real under-flagging holes. Added `wipefs`, `shred /dev/...`, `git clean -f`,
  more device names, and cwd-wiping `rm -rf ./`/`./*`. New `tests/safety_corpus.rs`
  with ~90 dangerous (including bypass attempts) and ~60 benign look-alikes.
- **Secret redaction in the model context.** Recent commands sent to the model
  are scrubbed of likely credentials (secret-named assignments, `--password`/
  `--token` flags, URL credentials, `Authorization:` headers, known key shapes
  like `sk-`/`ghp_`/`gsk_`/`AKIA`, and long high-entropy tokens). On by default
  (`redact_secrets`). Heuristic, not a guarantee.
- **Audit logging.** Optional JSONL log of every AI call (`ai_request`), response
  with token usage (`ai_response`), error (`ai_error`), and AI-initiated command
  with exit code (`action`). Off by default; enable with `[logging] enabled` or
  `AISHE_LOG=1`, path via `AISHE_LOG_FILE`. Logged text is redacted unless
  disabled. `aishe doctor` shows redaction and logging status.
- **Conversation memory.** The interactive REPL now remembers recent
  natural-language turns (across suggest, auto, and yolo) so follow-ups like "now
  do the same for the other file" have context. It stores requests and replies
  (not the full tool transcript), is size-capped, and is never written to disk.
  Clear it with `aishe reset` (`/reset`); disable with `memory = false`.
- **Syntax-highlighted code blocks.** Rendered model answers now highlight fenced
  code blocks by language (via syntect, pure-Rust fancy-regex), for both streamed
  and non-streamed output. On by default; build `--no-default-features` for a
  smaller binary that renders code blocks plain.
- **Markdown re-render for streamed suggest/auto answers.** A streamed prose
  answer is re-rendered as markdown in place when it finishes, matching yolo.
- **Yolo streaming.** The agentic loop now streams the model's text live (over
  Anthropic and OpenAI-compatible SSE, including streamed tool calls), so long
  runs no longer look frozen. A streamed final answer is re-rendered as markdown
  in place when it fit on screen; piped or non-tty output stays plain. Providers
  without streaming tool support fall back to a single non-streaming call.
- **Token & cost accounting** — every model call's `usage` is metered; a dim
  `N in · N out · N req · ~$cost` line prints after each interaction (toggle with
  `show_usage`), and `aishe usage` (`/usage`) shows the session total. Cost uses a
  built-in price table (USD/Mtok) overridable per model in `[pricing]`.
- **Budget cap** — set `budget_usd` to stop calling the model once the estimated
  session cost reaches it (e.g. a runaway yolo loop halts cleanly). `0` =
  unlimited; only enforced when the model's price is known.
- **`docs/ROADMAP.md`** — the tracked backlog (AI-shell features, zsh parity,
  trust/safety, test surface).
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
  (Claude-Code-style progressive disclosure). `aishe skills` (`/skills`) lists
  what's loaded; both `/skills` and `/commands` also work non-interactively
  (`aishe -c`).
- **Claude Code compatibility** — real Agent Skills from `anthropics/skills`
  (e.g. `internal-comms`, `brand-guidelines`) and slash commands from community
  collections (e.g. `wshobson/commands`) drop into `~/.config/aishe/skills/` and
  `~/.config/aishe/commands/` unchanged; aishe reads the `name`/`description`
  frontmatter and ignores keys it doesn't use (`allowed-tools`, `model`,
  `license`, …). Verified end-to-end against a live model.
- **CI** — cross-platform tests plus PTY smoke tests for both front-ends. The
  validation harness (`tests/admin_validation.py`) gained a deterministic
  plugins suite (meta `/commands`·`/skills`·`/config`·`/help`, custom-command
  discovery + `shell:`/`$ARGUMENTS` execution) and model-gated checks for custom
  NL commands and model-invoked skills (progressive disclosure verified via a
  unique skill-body token).

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
