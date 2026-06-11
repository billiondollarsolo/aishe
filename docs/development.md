# Development

## Build and check

```sh
cargo build
cargo test            # unit and integration tests (integration tests spawn a real shell)
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

## Test layout

- Rust unit tests live inline in each module under `src/`.
- Integration tests are in `tests/`:
  - `tests/cli.rs`, `tests/dispatcher.rs`, `tests/executor.rs`, `tests/modes.rs`,
    `tests/providers.rs`, `tests/safety.rs`: Rust integration tests.
  - `tests/pty_smoke.py`, `tests/reedline_smoke.py`: pseudo-terminal smoke tests
    for the two front-ends (need `python3`, no API key).
  - `tests/admin_validation.py`: the end-to-end validation harness.

## Validation harness

`tests/admin_validation.py` is a repeatable harness that exercises a large surface
area and writes a timestamped Markdown report under `test-results/`. It has five
suites:

1. Shell pass-through: runs many commands and shell constructs through `aishe -c`
   and compares output to the raw shell. Anything misrouted to the LLM shows up as
   a mismatch. Run with no API key.
2. Admin file operations: create, edit, move, permission, archive, and delete
   files, verifying on-disk state.
3. Plugins, slash-commands, and skills (deterministic): meta listings, custom
   command discovery, `shell` and `$ARGUMENTS` templating, no-frontmatter
   discovery, and project over user override precedence.
4. Dispatch classification: asserts each input routes to shell or natural language
   independent of output (including the `/usage`, `/reset`, `/ghost` meta slashes).
5. Config and meta robustness: a config exercising every newer field round-trips
   through `/config`, `aishe doctor` passes, the repo example config parses, and
   the new meta commands behave. No model needed.
6. Natural language (needs an API key): suggest, yolo, mode switching, custom NL
   commands, model-invoked skills, the token-usage line, the budget cap, and
   audit logging.

The two PTY smoke tests are part of the suite too: `tests/reedline_smoke.py`
exercises interactive, REPL-only features without a key (editing, multi-line
continuation, aliases/functions, history expansion, `AUTO_PUSHD`/`dirs -v`, and
spelling correction), and `tests/pty_smoke.py` drives the zsh-PTY front-end.

Run it:

```sh
cargo build --release
python3 tests/admin_validation.py            # deterministic suites need no key
```

To run the natural-language suite, provide an API key in the environment (the
harness reads `GROQ_API_KEY` or a local secrets file it documents at the top).

The deterministic suites are the pass gate; the natural-language suite is
informational because model output varies.

## CI

GitHub Actions runs the cross-platform Rust tests plus the pseudo-terminal smoke
tests on every push. Tagged releases build binaries.

## Adding to the harness

The harness is designed to grow. Add rows to the case lists at the top of
`tests/admin_validation.py` (`SHELL_CASES`, `FILE_OPS`, `DISPATCH_SHELL`,
`DISPATCH_NL`, `NL_SUGGEST`), or add command and skill fixtures in
`install_plugins`.

## Coding conventions

- Match the style of the surrounding code.
- Keep clippy and rustfmt clean.
- Prefer the deterministic test suites for anything that does not strictly need a
  live model.
