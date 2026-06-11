# aishe roadmap

A living backlog of where aishe is headed. Grounded in a code audit (June 2026).
Ordered by theme, not strictly by priority. The Now / Next / Later tags at the
end of each item are the current intent.

## The architectural fork (read this first)

The reedline front-end runs each input line as a fresh `zsh -c "..."`
(`executor.rs`). Shell state is simulated by intercepting `cd`, `export`,
aliases, and functions and replaying them from a generated rc file. That model
cannot do real job control, because `cmd &`, `jobs`, `fg`, `bg`, and `Ctrl-Z`
need a persistent shell that owns the job table. The zsh-PTY front-end
(`pty.rs`, the `auto` default when zsh is present) is a real zsh and gets all of
this for free.

So most zsh-parity work below only matters if reedline stays a flagship. The
alternative is to make zsh-pty the flagship real shell and position reedline as
the lightweight AI command runner. Decision: deferred. Every reedline-only item
is tagged `(reedline)` so we can re-scope quickly.

## 1. AI-shell features (the differentiators), building now

- [x] **Token and cost accounting plus budget cap.** Surface usage from every
  provider call, accumulate per session, show `tokens · cost`, add a `budget_usd`
  guard, and an `aishe usage` command. Foundational; everything below benefits.
  Done.
- [x] **Yolo streaming.** The agentic loop streams assistant text live (and
  tool-call deltas) over both providers, re-rendering the final answer as
  markdown with syntax-highlighted code blocks. Done.
- [x] **Session memory.** The interactive REPL keeps a rolling, size-capped
  transcript so follow-ups ("now do the same for the other file") work; clear
  with `aishe reset`, toggle with `memory`. Done.
- [x] **Inline ghost-text AI autosuggestion.** Warp/Copilot style: a background
  worker (debounced, cached, budget-aware, shared provider) predicts the rest of
  the command; reedline shows it as dim ghost text, accept with the Right arrow.
  Off by default (`aishe ghost on`). See `src/ghost.rs` and docs/ghost-text.md.
  Done. (Live-during-pause repaint is a possible future polish.)
- [x] **Richer yolo toolset and MCP.** Built-in file tools
  (`read_file`/`write_file`/`edit_file`/`list_dir`, `file_tools`) and a web tool
  (`fetch_url`, `web_tool`) ship on by default (`src/tools.rs`). The **MCP
  client** (`src/mcp.rs`, `[mcp_servers]`, docs/mcp.md) connects stdio MCP servers
  over JSON-RPC, lists their tools, and proxies `tools/call`, namespaced
  `mcp__<server>__<tool>` so the MCP ecosystem plugs into the yolo loop. Both
  **stdio** and **Streamable HTTP** (`url`/`headers`) transports ship. Done.
  (Consuming MCP prompts/resources is a possible follow-up.)
- [x] **Dry-run or plan preview for yolo.** With `yolo_plan` (`aishe plan on`),
  the model lays out its intended steps and the user approves before the loop
  runs; the approved plan is threaded into the run. Interactive only. Done.
- [x] **Response caching.** Identical suggest-mode (prompt, context) responses are
  served from a short-lived in-memory cache (`cache`/`cache_ttl_secs`, on by
  default), so repeats are instant and free. See `src/cache.rs`. Done.
- [x] **Per-project context.** A `.aishe/context.md` found at or above the cwd is
  fed to the model for repo-specific conventions (`project_context`, on by
  default, capped at 4000 chars). See docs/project-context.md. Done.

## 2. Trust and safety, quick wins first

- [x] **Secret redaction in model context.** Recent commands are scrubbed of
  secret-named assignments, credential flags, URL credentials, `Authorization:`
  headers, known key shapes, and high-entropy tokens before being sent. On by
  default. See `src/redact.rs` and docs/logging.md. Done.
- [x] **Audit logging.** Optional JSONL log of AI requests, responses (with token
  usage), errors, and AI-initiated commands (with exit codes). Off by default;
  logged text is redacted. See `src/audit.rs`. Done.
- [x] **Adversarial safety corpus.** `tests/safety_corpus.rs` has ~90 dangerous
  (including wrapper, quote, env-prefix, and chained bypass attempts) and ~60
  benign look-alikes. The gate was hardened to strip leading
  wrappers/assignments and unquote `rm` targets, plus new `wipefs`/`shred`/
  `git clean -f` rules. Done.
- [~] **Sandbox or confirm tiers.** Graduated confirmation (`yolo_confirm`:
  never/dangerous/writes/all) and a policy sandbox (`yolo_sandbox`: refuse network
  access and out-of-tree writes) ship (`src/sandbox.rs`, docs/safety.md). A true
  kernel/scratch-dir sandbox is still open.

## 3. zsh parity (mostly reedline)

- [~] **Job control**: background jobs ship in the reedline front-end - a trailing
  `&` backgrounds a command, with `jobs`/`fg`/`bg`/`wait`/`disown` and a
  `[n]+ Done` notice before the prompt. `Ctrl-Z`/`SIGTSTP` foreground suspension
  and process groups remain the zsh-PTY front-end's native domain. (reedline)
- [x] **Richer history**: dedup (`HIST_IGNORE_DUPS`), ignore-space, and
  `HISTIGNORE` glob patterns, plus a timestamped `EXTENDED_HISTORY`-format log
  (zsh-readable), a `history` builtin (`history [-E] [N]`), and cross-session
  sharing (`share_history`, zsh `SHARE_HISTORY`). See `src/histlog.rs`. Done.
  (reedline)
- [x] **Prompt depth**: command duration (`report_time`) and git
  staged/unstaged/ahead-behind/stash (`git_status`) shipped; an exit-status glyph
  colors the prompt; and the git status is now computed **async** (off-thread,
  cached, so the prompt never blocks on a slow/huge repo - markers lag by one
  prompt). Done. (reedline)
- [~] **Spelling correction** (`CORRECT`, `correct`) and **named dirs**
  (`~proj`, `[named_dirs]`) shipped, along with `AUTO_PUSHD` and `cdpath`. Global
  and suffix aliases still open. (reedline)
- [~] **Completion depth**: flag/option completion from a command's `--help`
  (parsed, cached, time-limited, with descriptions in the menu) shipped
  (`complete_flags`). Glob qualifiers and richer subcommand discovery still open.
  (reedline)

## 4. Test surface not yet exercised

- [ ] Adversarial safety corpus (see section 2).
- [x] Provider failure modes: 429/5xx/connection retries with backoff + jitter +
  `Retry-After`, truncated-SSE tolerance, defensive non-JSON/malformed-data
  parsing, usage parsing, and the schema-to-json-to-text step-down (full chain
  plus terminal give-up) are all covered (`tests/providers.rs`, provider unit
  tests).
- [x] Interactive PTY behaviors: a deterministic PTY suite now runs in CI -
  `tests/pty_scenarios.py` (targeted flows), `tests/pty_fuzz.py` (thousands of
  generative cases, logged to `test-results/fuzz-*.md`), `tests/zsh_features.py`
  (44 zsh features: pipes, here-docs, process subst, arrays, control structures,
  functions/aliases, dir stack, job control, history expansion, quoting,
  arithmetic), and `tests/pty_signals.py` (Ctrl-C mid-command, Ctrl-Z job
  suspension, window resize / SIGWINCH propagation, multi-line continuation).
  Completion-menu navigation depends on the user's zsh setup and is left to the
  native ZLE.
- [~] I/O edges: stdin piping / non-tty runs each line as a command (pipe/script
  mode, `tests/cli.rs`); large captured-output truncation is covered
  (`tests/executor.rs::captured_output_truncates`). Binary captured output and
  Unicode/emoji line editing still open.
- [x] Exit-code propagation: `aishe -c 'false'` returns 1, pipelines, `$?` chains,
  and `exit N` are covered (`tests/cli.rs`). Done.
- [x] Config precedence: `--flags > file > defaults` (`Config::apply_overrides`),
  audit env-vs-file (`AISHE_LOG`/`AISHE_LOG_FILE` via `resolve_audit`), and the
  legacy `llmsh` -> `aishe` migration are covered by unit and CLI E2E tests, as is
  the per-project `.aishe/config.toml` overlay (A2) and its trust tiering.

## 5. Distribution and polish

- [~] Distribution and polish: release tarballs + `.sha256` checksums,
  `cargo binstall` metadata, a Homebrew formula template (`packaging/aishe.rb`),
  `aishe completions <shell>`, build metadata (git SHA + date) in
  `aishe --version`, a richer `aishe doctor`, and a generated `aishe(1)` man page
  (shipped from the release/package jobs) all ship.
- [x] Linux distribution: the release workflow now also builds static musl and
  aarch64 Linux tarballs, `.deb`/`.rpm` packages (via nfpm, with completions and
  a generated `aishe(1)` man page), and a `curl | sh` `install.sh`. Repo identity
  is reconciled on `billiondollarsolo/aishe` across `Cargo.toml`, binstall, the
  Homebrew formula, and docs.
