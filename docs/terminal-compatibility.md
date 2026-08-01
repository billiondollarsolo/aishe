# Terminal and transport compatibility

AIShe's flagship interface is a real zsh running through AIShe's PTY. Terminal
compatibility is a tested contract, not a claim that every emulator behaves the
same. The automated evidence covers the PTY byte stream, agent proposal staging,
history, delayed escape sequences, and resize propagation. Visual behavior in a
named terminal application still needs a named manual result.

## Status vocabulary

The machine-readable report from `tests/terminal_compat.py` uses four outcomes:

| Status | Meaning |
| --- | --- |
| `pass` | The complete automated contract ran and every assertion passed. |
| `fail` | The transport started, but an asserted AIShe behavior failed. This is a release-blocking regression when the capability is required. |
| `limitation` | The tool exists, but this machine lacks necessary external state, such as an authorized SSH target. It is not a pass. |
| `unsupported` | The transport executable is unavailable on the host. It is not a pass. |

The harness exits non-zero for any `fail`. A capability named with
`--require-capability` must be `pass`; `limitation` and `unsupported` then also
exit non-zero. This prevents a missing tool from silently satisfying CI.

## Automated contract

Every selected transport runs the same deterministic, credential-free checks:

1. Launch a real interactive zsh with isolated config, data, runtime, and
   history directories.
2. Submit a fake-provider agent request and prove suggest mode stages a command
   without executing it; a second Enter executes it.
3. Run a direct shell command and recall it with an Up-arrow sequence split
   after the Escape byte by exactly 300 ms.
4. Resize from 80x24 to 120x40 and prove the nested zsh observes
   `COLUMNS=120`.
5. Record the effective `TERM` value and all tool versions in JSON.

Run all locally available transports while recording unavailable state honestly:

```sh
cargo build --release --locked
python3 tests/terminal_compat_test.py
python3 tests/terminal_compat.py target/release/aishe \
  --json test-results/terminal-compat.json
```

The Linux release gate requires local PTY, tmux, and GNU screen:

```sh
python3 tests/terminal_compat.py target/release/aishe \
  --capability local-latency --require-capability local-latency \
  --capability tmux --require-capability tmux \
  --capability screen --require-capability screen
```

SSH is intentionally opt-in. Use only a host you are authorized to access and
on which the candidate AIShe binary is installed:

```sh
python3 tests/terminal_compat.py target/release/aishe \
  --capability ssh --require-capability ssh \
  --ssh-target user@qualification-host \
  --ssh-identity ~/.ssh/qualification-key \
  --ssh-binary /path/to/candidate/aishe \
  --json test-results/terminal-compat-ssh.json
```

The harness uses batch authentication, a bounded connection timeout, a private
known-hosts file, and an isolated remote fixture. The optional identity is
passed as one `ssh -i` argv value and is never written to the JSON report. The
harness first records the remote binary identity. Without `--ssh-target`, SSH
is `limitation`, never `pass`.

## macOS flagship CI gate

The `macOS flagship PTY gate (bounded)` job runs on `macos-latest` and blocks a
push or pull request when it regresses. It builds the release binary and runs
these deterministic suites:

- `terminal_compat.py` for local 300 ms escape latency and resize propagation;
- `pty_scenarios.py` for routing, sigils, suggest/auto behavior, and history;
- `model_picker_pty.py` for connection/model selection and shell-local state;
- `statusline_pty.py` for placement and live metrics;
- `setup_pty.py` for the setup state machine; and
- `pty_signals.py` for Ctrl-C, Ctrl-Z, resize, and multiline input.

The job has a 35-minute bound; the release build has 10 minutes and each PTY
step has four or five minutes. These tests create ordinary CLI pseudo-terminals.
They do not drive macOS app UI and require none of Accessibility, Screen
Recording, Automation, or Full Disk Access. A runner that cannot allocate a PTY
fails the required contract rather than skipping it.

This bounded job deliberately omits paid providers, long-running backend soak,
installer mutation, and fuzz scale. Those remain separate gates.

## Point-in-time local evidence

On 2026-08-01, candidate `aishe 0.6.5 (4a2c7e4, 2026-08-01)` passed the full
contract on macOS 14.6.1 arm64 with zsh 5.9:

| Transport | Tool | Result | Observed `TERM` |
| --- | --- | --- | --- |
| Local PTY | zsh 5.9 | `pass` | `xterm-256color` |
| tmux | tmux 3.6a | `pass` | `tmux-256color` |
| GNU screen | Screen 4.00.03 | `pass` | `screen.xterm-256color` |
| SSH | OpenSSH 9.7p1 to an authorized Ubuntu 26.04 target | `pass` | `xterm-256color` |

The SSH pass used the exact candidate identity on the remote host, an isolated
temporary HOME/config/data/runtime, the fake provider, 300 ms split escape input,
and 80x24 → 120x40 resize. No provider credential was read or used. Running the
harness without an explicit authorized target still records `limitation`.

The same candidate also passed a native Linux run on Ubuntu 26.04 LTS x86_64
(kernel 7.0.0-28) with zsh 5.9:

| Transport | Tool | Result | Observed `TERM` |
| --- | --- | --- | --- |
| Local PTY | zsh 5.9 | `pass` | `xterm-256color` |
| tmux | tmux 3.6 | `pass` | `tmux-256color` |
| GNU screen | Screen 4.09.01 | `pass` | `screen.xterm-256color` |

These native and SSH matrices were rerun after the shell assets/templates
extraction. The SSH harness placed a symlink to the explicitly supplied
isolated candidate at the front of the remote fixture's `PATH`; the machine's
different host-installed AIShe binary was not selected. The persisted report
contains neither the target address nor the local identity-file path.

GNU screen must run attached to a real controlling PTY for resize evidence. A
detached screen has no display PTY to resize; changing its logical `width`
alone left the kernel size at 80x24 and produced `COLUMNS=-1` on this host. The
harness tests the attached path users actually operate.

This evidence applies only to that candidate and the named OS/tool versions. CI
supplies fresh local, tmux, and screen evidence for later checkouts. A single
authorized SSH target does not imply every SSH server, network, authentication
method, or terminal emulator is supported.

## Terminal-emulator manual matrix

The PTY contract cannot prove emulator-specific key configuration, width
measurement, glyph rendering, contrast, or clipboard behavior. Until a dated
manual transcript/screenshot and candidate identity are recorded, the state is
`not_run`:

| Terminal | Current manual state | Required observations |
| --- | --- | --- |
| Apple Terminal | `not_run` | Option/Meta configuration, routing cue, picker keys, resize, colors/mono |
| iTerm2 | `not_run` | Option Esc+ configuration, routing cue, picker keys, resize, colors/mono |
| WezTerm | `not_run` | key protocol, routing cue, picker keys, resize, colors/mono |
| kitty | `not_run` | keyboard protocol, routing cue, picker keys, resize, colors/mono |
| GNOME Terminal | `not_run` | Alt/Meta, routing cue, picker keys, resize, colors/mono |
| VS Code integrated terminal | `not_run` | `macOptionIsMeta` where relevant, routing cue, picker, resize |
| Cursor integrated terminal | `not_run` | `macOptionIsMeta` where relevant, routing cue, picker, resize |

Do not turn `not_run` into `pass` based only on the generic PTY suite. Record the
terminal/version, OS, AIShe binary identity, width, `TERM`, and exact failed or
passed observations.

Fish and WSL are separate product decisions, not implied by this matrix. See the
[Fish integration decision](design/FISH_INTEGRATION_DECISION.md) and
[WSL compatibility decision](design/WSL_COMPATIBILITY_DECISION.md).
