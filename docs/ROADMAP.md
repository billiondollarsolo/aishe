# aishe roadmap

A living backlog of where aishe is headed. Grounded in a code audit (June 2026).
Ordered by theme, not strictly by priority; the **Now / Next / Later** tags at the
end of each item are the current intent.

## The architectural fork (read this first)

The **reedline front-end runs each input line as a fresh `zsh -c "…"`**
(`executor.rs`). Shell state is *simulated* by intercepting `cd`/`export`/aliases/
functions and replaying them from a generated rc file. That model **cannot do real
job control** — `cmd &`, `jobs`, `fg`, `bg`, `Ctrl-Z` need a persistent shell that
owns the job table. The **zsh-PTY front-end** (`pty.rs`, the `auto` default when zsh
is present) *is* a real zsh and gets all of this for free.

So most zsh-parity work below only matters **if reedline stays a flagship**. The
alternative is to make zsh-pty the flagship "real shell" and position reedline as
the lightweight AI command runner. **Decision: deferred** — but every reedline-only
item is tagged `(reedline)` so we can re-scope quickly.

---

## 1. AI-shell features (the differentiators) — **building now**

- [ ] **Token & cost accounting + budget cap.** Surface `usage` from every
  provider call, accumulate per session, show `tokens in/out · ~$cost`, add a
  `budget_usd` guard that stops before overspending, and an `aishe usage`
  command. *Foundational — everything below benefits.* **Now**
- [ ] **Yolo streaming.** The agentic loop uses non-streaming
  `complete_with_tools`, so long runs look frozen. Stream assistant text +
  tool-call deltas (Anthropic `message_delta`, OpenAI tool-call streaming). **Next**
- [ ] **Session memory.** NL/yolo invocations start cold each time. Keep a rolling
  conversation transcript in the interactive REPL so follow-ups work ("now do the
  same for the other file"); cap/summarize to control cost. **Next**
- [ ] **Inline ghost-text AI autosuggestion.** Warp/Copilot-style: as you type,
  asynchronously propose a completion of the current command, accept with `→`.
  Needs debounce + cancellation + cost guard. **Later (biggest)**
- [ ] **Richer yolo toolset / MCP.** Beyond `run_command`: `read_file`,
  `apply_patch`, web fetch; optional MCP client so external tool servers plug in. **Later**
- [ ] **Dry-run / plan preview for yolo.** Show the planned step(s) and let the
  user approve the batch before execution. **Later**
- [ ] **Response caching.** Cache identical (prompt, context) suggestions briefly
  to cut latency/cost on repeats. **Later**
- [ ] **Per-project context.** Optional `.aishe/context.md` fed to the model for
  repo-specific conventions. **Later**

## 2. Trust & safety — **quick wins first**

- [ ] **Secret redaction in model context.** `context.rs` ships the last 10
  commands verbatim to the provider; a prior `export TOKEN=…`, `mysql -p…`, or a
  URL with credentials leaks. Redact `KEY=value`, `-p<secret>`, `Authorization:`,
  and high-entropy tokens before sending. **Now**
- [ ] **Adversarial safety corpus.** `safety.rs` has the rules but only ~2 inline
  tests. Build a large dangerous-vs-benign-lookalike battery: `rm -rf "$EMPTY"/`,
  obfuscated `dd`, `sudo` wrappers, base64/`eval` payloads, `find … -delete`,
  `git clean -xfd`, `> /dev/sda`, chained/quoted variants. **Now**
- [ ] **Sandbox / confirm tiers.** Optional restricted exec for yolo (no network,
  scratch dir), graduated confirmation. **Later**

## 3. zsh parity (mostly `reedline`)

- [ ] **Job control** — `cmd &`, `jobs`, `fg`, `bg`, `Ctrl-Z`/`SIGTSTP`,
  `disown`, `wait`. *(reedline)* The single biggest parity gap. **Later (re-scope first)**
- [ ] **Richer history** — dedup (`HIST_IGNORE_DUPS`), timestamps, cross-session
  sharing, `HISTIGNORE`. *(reedline)*
- [ ] **Prompt depth** — git **dirty/staged/ahead-behind/stash**, **exit-status**
  glyph, **command duration** (`REPORTTIME`), async vcs_info. *(reedline)*
- [ ] **Spelling correction** (`CORRECT`), **named dirs** (`~proj`), **`cdpath`**,
  **`AUTO_PUSHD`**, **global/suffix aliases**. *(reedline)*
- [ ] **Completion depth** — closer to zsh compsys: descriptions for arbitrary
  commands, file completion with glob qualifiers, completion from man/`--help`. *(reedline)*

## 4. Test surface not yet exercised

- [ ] **Adversarial safety corpus** (see §2).
- [ ] **Provider failure modes** — 429 rate-limit, timeouts, truncated/malformed
  SSE, non-JSON bodies, the schema→json→prompt step-down path, usage parsing.
- [ ] **Interactive PTY behaviors** — `Ctrl-C` mid-command, `Ctrl-Z`, window
  resize, completion-menu navigation, multi-line editing (only smoke-tested today).
- [ ] **I/O edges** — stdin piping / non-tty (`echo "prompt" | aishe`), large &
  binary captured output limits, Unicode/emoji/wide-char line editing.
- [ ] **Exit-code propagation** — `aishe -c 'false'` → 1, pipelines, `$?` chains.
- [ ] **Config precedence** — env vs flags vs file, legacy migration.

## 5. Distribution & polish

- [ ] Packaging (Homebrew/AUR/cargo-binstall), shell-completion for `aishe`
  itself, man page, `aishe --version` build metadata, richer `aishe doctor`.
