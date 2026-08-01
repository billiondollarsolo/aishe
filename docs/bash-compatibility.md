# Native Bash hook compatibility

The native Bash hook is a **Tier B** front-end on Bash 5.x and a reduced
**Tier B-** front-end on Bash 3.2. It keeps the user's real Bash session and line
editor, but its supported surface is intentionally smaller than AIShe's Tier A
zsh PTY. Compatibility claims come from the deterministic interactive harness,
not from generated-script string assertions.

## Qualification authority

Run every locally discoverable Bash 3.2 and 5.x binary:

```sh
python3 tests/bash_hook.py target/release/aishe --strict-matrix
```

Run the current Linux Bash gate:

```sh
python3 tests/bash_hook.py target/release/aishe \
  --bash /bin/bash --require-family 5.x
```

The harness launches a real interactive Bash in a pseudo-terminal. Each run has
an isolated `HOME`, rc file, config, data, runtime, temporary directory,
selection handoff, history file, and pre-existing exit trap. AIShe's in-process
fake provider supplies deterministic responses with no credentials; proxy
variables point at a closed loopback port as an additional network tripwire.
The shared binary-identity check rejects a stale AIShe build.

Reports distinguish `pass`, `fail`, `not_run`, and `unavailable`. An unavailable
Bash family is never counted as a pass. Bash 3.2 may also report
`expected_difference` for exactly five declared Readline cases; that status is
neither a pass nor a skip for the individual affordance, but it satisfies the
reduced B- matrix only when the documented alternative was exercised. The same
status on Bash 5.x or any undeclared case fails qualification. Use `--json PATH`
to retain the versioned machine-readable evidence (`schema_version: 1`).

## Declared Tier-B matrix

The harness covers these observable behaviors on each requested Bash:

| Area | Contract |
| --- | --- |
| Hook startup | generated hook sources in an isolated interactive shell |
| Routing | unknown natural language reaches the fake provider |
| Forced agent | `?` and the Ctrl-G Readline binding reach the fake provider |
| Collision | a real command remains native and never calls the provider |
| Slash surface | `/help` and `/status` reach their AIShe CLI handlers |
| Trap coexistence | a pre-existing ERR trap observes routed 127 and ordinary status 1 |
| Suggest review | printed suggestion can be recalled with Ctrl-X Ctrl-R |
| Session controls | Shift-Tab changes mode; Ctrl-O changes output detail |
| Shell-local state | an auto-mode safe `cd` is applied in the parent Bash |
| Failure UX | optional failure hint and Ctrl-X Ctrl-F correction path |
| History | Up arrow and the isolated persistent `HISTFILE` agree |
| Signals | Ctrl-C interrupts; Ctrl-Z stops a foreground job that can be cleaned up |
| Cleanup | all per-shell files are removed and an existing EXIT trap still runs |

The declared matrix passes only when every row passes for every required
version family or matches an exact, family-scoped B- difference below.
Component evidence obtained by calling a generated hook function directly does
not turn a failed interactive route into a pass.

### Bash 3.2 Tier B- differences

macOS Bash 3.2 predates the editable-buffer behavior the generated `bind -x`
widgets use. The core shell contract remains required, including unknown-NL and
`?` routing, real-command collision, slash dispatch, auto-mode state handoff,
history, signals, and cleanup. These five affordances have explicit
alternatives:

| Unavailable Readline affordance | Required tested alternative |
| --- | --- |
| Ctrl-G forced-agent buffer | leading `?`, which must pass the routing case |
| Ctrl-X Ctrl-R suggestion recall | printed suggestion remains available for manual copy/edit |
| Shift-Tab mode cycle | set `AISHE_MODE` explicitly |
| Ctrl-O details toggle | use `/details` |
| Ctrl-X Ctrl-F correction prefill | use the failure hint and CLI suggestion for manual review |

The report uses `expected_difference` for those rows. It never calls them
passes, and a missing alternative or a difference on another row fails B-.

## Expected differences from Tier A

- `#` stays Bash comment syntax. Use `?` on every declared tier, or Ctrl-G on
  Bash 5.x, for a forced-agent request.
- Suggest mode prints a command for review. Bash cannot reliably prefill the
  next prompt from `PROMPT_COMMAND` the way zsh's ZLE hook can; Bash 3.2 also
  uses manual copy/edit instead of the recall binding.
- Command-not-found routing is narrower than zsh's full-buffer classifier. If
  the first token is a real command, Bash runs it; use a supported explicit
  override when agent routing is intended.
- The hook does not supply zsh's route coloring, AIShe-owned PTY, branded
  prompt, or live status-line layout.
- The guarded status-127 fallback may print Bash's native
  `command not found`/`No such file or directory` diagnostic before AIShe
  handles the request.

These are declared limitations, not skipped tests. Slash dispatch, documented
keys, state handoff, signals, and cleanup remain testable Tier-B requirements.

## Qualified evidence: AIShe 0.6.5 development checkout

The first real audit exposed unreachable slash cases, missing Bash 3.2 routing,
and a literal `$AISHE_PENDING` recall macro. After adding the guarded status-127
fallback and a real recall widget, the strict matrix passed on macOS arm64:

| Runtime | Result | Evidence |
| --- | --- | --- |
| Bash 5.3.9 | **Tier B pass: 18/18** | all routing, trap-chain, key, state, history, signal, job-control, and cleanup cases passed |
| Bash 3.2.57 | **Tier B- pass: 13 core passes + 5 expected differences** | all core rows passed; each Readline difference matched the declared case and its alternative passed |

`python3 tests/bash_hook.py target/release/aishe --strict-matrix` exited zero and
wrote a schema-v1 JSON report. The Linux CI gate requires its `/bin/bash` to be
in the 5.x family and pass all Tier-B rows. The reusable qualification driver
uses `--require-current-family`, so an undeclared Bash family or an unavailable
requested binary cannot silently qualify.
