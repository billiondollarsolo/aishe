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
- [ ] **Yolo streaming.** The agentic loop uses non-streaming
  `complete_with_tools`, so long runs look frozen. Stream assistant text and
  tool-call deltas (Anthropic `message_delta`, OpenAI tool-call streaming). Next.
- [ ] **Session memory.** NL and yolo invocations start cold each time. Keep a
  rolling conversation transcript in the interactive REPL so follow-ups work
  ("now do the same for the other file"); cap or summarize to control cost. Next.
- [ ] **Inline ghost-text AI autosuggestion.** Warp or Copilot style: as you
  type, asynchronously propose a completion of the current command, accept with
  the right arrow. Needs debounce, cancellation, and a cost guard. Later (biggest).
- [ ] **Richer yolo toolset and MCP.** Beyond `run_command`: `read_file`,
  `apply_patch`, web fetch; an optional MCP client so external tool servers plug
  in. Later.
- [ ] **Dry-run or plan preview for yolo.** Show the planned steps and let the
  user approve the batch before execution. Later.
- [ ] **Response caching.** Cache identical (prompt, context) suggestions briefly
  to cut latency and cost on repeats. Later.
- [ ] **Per-project context.** Optional `.aishe/context.md` fed to the model for
  repo-specific conventions. Later.

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
- [ ] **Sandbox or confirm tiers.** Optional restricted exec for yolo (no
  network, scratch dir) with graduated confirmation. Later.

## 3. zsh parity (mostly reedline)

- [ ] **Job control**: `cmd &`, `jobs`, `fg`, `bg`, `Ctrl-Z`/`SIGTSTP`,
  `disown`, `wait`. (reedline) The single biggest parity gap. Later (re-scope
  first).
- [ ] **Richer history**: dedup (`HIST_IGNORE_DUPS`), timestamps, cross-session
  sharing, `HISTIGNORE`. (reedline)
- [ ] **Prompt depth**: git dirty, staged, ahead-behind, and stash indicators, an
  exit-status glyph, command duration (`REPORTTIME`), async vcs_info. (reedline)
- [ ] **Spelling correction** (`CORRECT`), named dirs (`~proj`), `cdpath`,
  `AUTO_PUSHD`, global and suffix aliases. (reedline)
- [ ] **Completion depth**: closer to zsh compsys, with descriptions for
  arbitrary commands, file completion with glob qualifiers, and completion from
  man pages or `--help`. (reedline)

## 4. Test surface not yet exercised

- [ ] Adversarial safety corpus (see section 2).
- [ ] Provider failure modes: 429 rate-limit, timeouts, truncated or malformed
  SSE, non-JSON bodies, the schema to json to prompt step-down path, usage
  parsing.
- [ ] Interactive PTY behaviors: `Ctrl-C` mid-command, `Ctrl-Z`, window resize,
  completion-menu navigation, multi-line editing (only smoke-tested today).
- [ ] I/O edges: stdin piping and non-tty (`echo "prompt" | aishe`), large and
  binary captured output limits, Unicode and emoji line editing.
- [ ] Exit-code propagation: `aishe -c 'false'` returns 1, pipelines, `$?`
  chains.
- [ ] Config precedence: env vs flags vs file, legacy migration.

## 5. Distribution and polish

- [ ] Prebuilt packages (Homebrew, a Linux package or tarball, cargo-binstall),
  shell completion for `aishe` itself, a man page, build metadata in
  `aishe --version`, and a richer `aishe doctor`.
