> **Lifecycle: Active.** Baseline: AIShe v0.7.0 (`d79dd6c`), branch
> `codex/daily-driver-elite`. This is the implementation plan for the
> 2026-09-03 daily-driver review. Phase 1 items were all reproduced against the
> release binary on macOS before this plan was written.

# AIShe daily-driver elite fix plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every user-facing surface of aishe (installer, setup, CLI help and errors, the live zsh shell with its prompt, statusline, palette and keys, the slash-command registry, the agent transcript, and the docs) work correctly and read as one product.

**Architecture:** Keep the existing shape: one declarative command registry (`src/command_surface.rs`) drives Rust routing, both shell hooks, help, palette, completions and the docs table; `src/promptui.rs` owns every interactive prompt; `src/ui.rs` owns terminal capability detection and styling; the zsh assets under `src/integration/assets/` are the only shell-side presentation code. The fixes remove second implementations rather than adding new layers: one key reader, one effect vocabulary, one controls hint, one color map exported from Rust into zsh.

**Tech Stack:** Rust 1.88 (clap 4, crossterm 0.29, anyhow, serde), zsh 5.9 hook and prompt assets, Bash hook, POSIX sh installer, Python 3 PTY tests (no third-party modules), `cargo test --all-targets --locked`, `python3 tests/qualify.py`.

**Spec:** The review report at https://claude.ai/code/artifact/a811f2eb-7a06-4c1d-a65c-d58d938a5bea (14 P0, 61 P1/P2 findings, all with `file:line`). Section 0 of this plan restates the findings each phase covers so the plan stands alone.

## Global constraints

- Rust MSRV is 1.88 (`Cargo.toml` `rust-version`). No new crates; `libc`, `crossterm`, `clap`, `anyhow` are already dependencies.
- Run `cargo fmt --all` and `cargo clippy --all-targets --locked -- -D warnings` before every commit; CI runs `cargo test --all-targets --verbose --locked`.
- Product name in prose is `AIShe`; the binary and every command spelling is `aishe`. Section headers printed by the CLI use `AIShe` (`AIShe setup`, `AIShe settings`), never `aishe setup`.
- Every user-fatal error goes through `crate::cli::error_contract::emit_classified` or a `UserError`; the exit code comes from `ErrorNamespace::exit_code()` (`cli` = 2, `config` = 3, `io` = 10, `internal` = 1). Never print `internal.unexpected` for user input.
- Every notice or message the user sees uses sentence case, no trailing period on a single-line label, `·` as the inline separator, and `Next:` as the only recovery label.
- Mode names are `suggest`, `auto`, `yolo` everywhere. The word `review` never denotes a mode.
- PTY tests live in `tests/*_pty.py`, take the binary path as `argv[1]`, and are self-contained (each file has its own `Pty` helper; nothing imports `pyte`).
- The generated block in `docs/commands.md` is checked by `commands_markdown_matches_the_generated_registry_block`; regenerate it by running that test and pasting the expected text from the assertion output, never by hand-editing rows.
- Every file under `docs/design/` must carry a `> **Lifecycle: …**` banner and a row in `docs/design/README.md` (`python3 tests/docs_contract_test.py` enforces both).
- Commit after each task with a conventional prefix (`fix:`, `feat:`, `refactor:`, `test:`, `docs:`) and end the message with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.

---

## 0. What each phase fixes, and why in this order

| Phase | Covers | Why now |
| --- | --- | --- |
| 1. Stop the bleeding | The 14 P0 findings: in-shell menus die, yolo decline is an internal error, palette corrupts the prompt and fills the wrong forms, SIGPIPE panics, usage errors wrapped as internal errors, `/mode` vs `aishe mode`, `/usage` on managed accounts, bare `reset`/`details`, first-run hand-off and PATH, branded prompt seizing themes, statusline width, setup verdict/apply/api-key, key bindings and failure spawn, orphaned runtime servers | Each one is hit in the first ten minutes of use and each is a contained change. Nothing in later phases depends on their absence, but shipping them first makes the product usable while the vocabulary work lands. |
| 2. One registry, one vocabulary | Effect labels, hidden aliases, duplicate and legacy commands, tombstones, help topics and controls hints, help overview length, error-path unification, empty and inconsistent clap help, status and doctor text | These are the changes that make the surface *look* like one product. They touch the registry, so they come after the registry-adjacent P0 fixes (palette, mode) and before the cosmetic pass. |
| 3. One look | Theme and colors exported from Rust to zsh, `review` → `suggest`, chevrons, separators, ellipsis, renderer widths, blank lines, Ctrl-O hint, markdown under NO_COLOR, resize | Purely presentational; depends on the glyph and style helpers that Phase 2 consolidates. |
| 4. Setup on a diet | Fewer steps and prompts, recommended defaults, no diff dump, terminology and casing, prompt footers, next actions, tour voice, installer polish | Setup is the largest single file (3118 lines) and the least frequently run surface; it benefits from the vocabulary decisions already being made. |
| 5. Docs and guards | Generated CLI docs block, getting-started fixes, stale doc paragraphs, PTY regression tests wired into `qualify.py`, test-suite process cleanup | Documentation and tests should describe the finished behavior, so they close the plan. |

Estimated effort: Phase 1 two days, Phase 2 three days, Phase 3 two days, Phase 4 two days, Phase 5 one day.

---

## File structure

Files created:

- `tests/in_shell_menus_pty.py` — PTY test: `/settings`, `aishe setup`, `aishe tour` typed inside `aishe zsh` read keys and exit cleanly.
- `tests/yolo_consent_pty.py` — PTY test: Shift-Tab then decline/Esc leaves the mode on `auto` with no error.
- `tests/palette_pty.py` — PTY test: `/`+Tab then Esc restores the prompt with an empty buffer; Enter fills the slash form.
- `tests/mode_handoff_pty.py` — PTY test: `aishe mode` inside the shell reads and sets this shell's mode.
- `tests/bare_words_pty.py` — PTY test: a user `reset` function is not hijacked.
- `tests/theme_prompt_pty.py` — PTY test: a user-set `PROMPT` survives; stock prompt gets the branded glyph.
- `tests/installer_bindir.sh` — shell test for `choose_bindir`.
- `tests/docs_cli_block_test.py` — asserts the CLI block in `docs/commands.md` equals `aishe --help` derived text.

Files modified (by responsibility):

- `src/promptui.rs` — every interactive prompt reads keys through `PickerInput`; `read_terminal_line` becomes public.
- `src/cli/runtime.rs` — yolo consent returns `YoloAcceptance`; one-shot slash handlers; `INTERCEPTED` handling.
- `src/main.rs` — SIGPIPE reset; exit codes; `--accept-yolo` mapping; hidden commands.
- `src/user_error.rs`, `src/cli/error_contract.rs` — `UserFacing` typed error; new `cli.*` codes.
- `src/command_surface.rs` — `Effect` enum, `hidden` flag, `Palette` hook action, `mode` availability, tombstone removal.
- `src/integration/registry.rs`, `src/integration/assets/zsh_hook.zsh`, `bash_hook.bash`, `pty_prompt.zsh`, `wrapper.zshrc` — hook rendering and shell-side presentation.
- `src/palette.rs`, `src/product_help.rs` — slash-form entries, help overview, `HELP_TOPICS`, `CONTROLS_HINT`.
- `src/cli/connection.rs`, `src/cli/status.rs`, `src/cli/backend.rs`, `src/diagnostics.rs` — mode handoff, usage text, orphan detection, doctor rows.
- `src/config.rs` — `load_or_init` pause handling; default status items; `pty_prompt` doc.
- `src/setup.rs`, `src/capabilities.rs`, `src/tour.rs` — verdict label, apply menu, credential menu, epilogue, step trimming.
- `src/agent/renderer.rs`, `src/ui.rs`, `src/ui/render.rs`, `src/modes/mod.rs` — widths, glyphs, blank lines, hint gating.
- `src/pty.rs` — style export, SIGWINCH.
- `install.sh` — `choose_bindir`, PATH note after setup, no-TTY exit.
- `src/cli/args.rs` — help strings, flags, groupings, hidden commands.
- `docs/commands.md`, `docs/getting-started.md`, `docs/installation.md`, `docs/front-ends.md`, `docs/shell-integration.md`, `docs/daily-driver.md`, `docs/troubleshooting.md`, `docs/accessibility.md`, `README.md`.

---

# Phase 1 — Stop the bleeding (P0)

### Task 1: One key reader for every interactive prompt

**Why:** `promptui::menu` and `promptui::secret` read keys with crossterm's `event::read()`, which opens `/dev/tty`. Under the zsh-PTY front-end, `/dev/tty` is the *outer* proxy terminal (see the comment at `src/promptui.rs:526-529`), so inside `aishe zsh` the reader cannot initialize: `/settings`, `aishe setup`, and `aishe tour` all print their first screen, then `Failed to initialize input reader`, and the next typed character is lost. The `/model` and `/connection` pickers already use `PickerInput` (an unbuffered read of the inherited stdin fd) and work everywhere. This task makes `menu` and `secret` use the same reader and deletes the crossterm event path.

**Files:**
- Modify: `src/promptui.rs:826-923` (`menu`), `src/promptui.rs:1036-1085` (`secret`), imports at the top of the file
- Test: `src/promptui.rs` (unit tests, `mod tests`), `tests/in_shell_menus_pty.py` (new)

**Interfaces:**
- Consumes: `PickerInput::open()`, `PickerInput::read_key() -> io::Result<PickerKey>`, `PickerInput::from_bytes(&[u8])` (test only), `RawGuard::enter()`, `MenuResult::{Selected(usize), Back, Cancel}`.
- Produces: `fn menu_select(keys: &mut PickerInput, options: &[String], selected: usize, allow_back: bool, help: &str, terminal_columns: usize) -> Result<MenuResult>` and `fn read_secret(keys: &mut PickerInput, max_bytes: usize) -> Result<Option<String>>` (both private, both unit-tested). Public signatures of `menu` and `secret` are unchanged.

- [ ] **Step 1: Write the failing unit tests**

Append inside `mod tests` in `src/promptui.rs`:

```rust
    #[test]
    fn menu_select_reads_arrow_digit_back_and_escape_through_picker_input() {
        let options: Vec<String> = ["one", "two", "three"].map(String::from).to_vec();
        let mut down_enter = PickerInput::from_bytes(b"\x1b[B\r");
        assert_eq!(
            menu_select(&mut down_enter, &options, 0, true, "", 80).unwrap(),
            MenuResult::Selected(1)
        );
        let mut digit = PickerInput::from_bytes(b"3\r");
        assert_eq!(
            menu_select(&mut digit, &options, 0, true, "", 80).unwrap(),
            MenuResult::Selected(2)
        );
        let mut back = PickerInput::from_bytes(b"b");
        assert_eq!(
            menu_select(&mut back, &options, 0, true, "", 80).unwrap(),
            MenuResult::Back
        );
        // A bare Esc with nothing after it (EOF on /dev/null) is a cancel, never a crash.
        let mut escape = PickerInput::from_bytes(b"\x1b");
        assert_eq!(
            menu_select(&mut escape, &options, 0, false, "", 80).unwrap(),
            MenuResult::Cancel
        );
    }

    #[test]
    fn read_secret_collects_characters_and_honours_backspace_and_cancel() {
        let mut typed = PickerInput::from_bytes(b"ab\x7fc\r");
        assert_eq!(read_secret(&mut typed, 64).unwrap().as_deref(), Some("ac"));
        let mut cancelled = PickerInput::from_bytes(b"secret\x03");
        assert_eq!(read_secret(&mut cancelled, 64).unwrap(), None);
        let mut bounded = PickerInput::from_bytes(b"abcdef\r");
        assert_eq!(read_secret(&mut bounded, 3).unwrap().as_deref(), Some("abc"));
    }
```

`MenuResult` needs `PartialEq, Debug` for `assert_eq!`; add `#[derive(Debug, PartialEq, Eq)]` to it if it does not already derive them.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --lib promptui::tests::menu_select -- --nocapture`
Expected: compile error, `menu_select` and `read_secret` not found.

- [ ] **Step 3: Implement `menu_select` and rewrite `menu`**

Replace the body of `menu` from `let guard = RawGuard::enter()?;` (line ~858) to the end of the function with:

```rust
    let mut keys = PickerInput::open().context("opening menu input")?;
    let guard = RawGuard::enter()?;
    print_selection(selected, &options[selected], terminal_columns);
    let result = menu_select(&mut keys, options, selected, allow_back, help, terminal_columns);
    // Leave raw mode before emitting the cooked newline; error unwinding also drops the guard.
    drop(guard);
    println!();
    result
}

fn menu_select(
    keys: &mut PickerInput,
    options: &[String],
    mut selected: usize,
    allow_back: bool,
    help: &str,
    terminal_columns: usize,
) -> Result<MenuResult> {
    let mut number_buffer = String::new();
    loop {
        let key = match keys.read_key() {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(MenuResult::Cancel)
            }
            Err(error) => return Err(error).context("reading menu input"),
        };
        match key {
            PickerKey::Up | PickerKey::Character('k') => {
                number_buffer.clear();
                selected = selected.checked_sub(1).unwrap_or(options.len() - 1);
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Down | PickerKey::Character('j') => {
                number_buffer.clear();
                selected = (selected + 1) % options.len();
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Enter => return Ok(MenuResult::Selected(selected)),
            PickerKey::Character(c) if c.is_ascii_digit() => {
                number_buffer.push(c);
                if !(1..=options.len()).any(|number| number.to_string().starts_with(&number_buffer))
                {
                    number_buffer.clear();
                    number_buffer.push(c);
                }
                let index = number_buffer.parse::<usize>().unwrap_or(0);
                if index >= 1 && index <= options.len() {
                    selected = index - 1;
                    print_selection(selected, &options[selected], terminal_columns);
                }
            }
            PickerKey::Character('b' | 'B') if allow_back => return Ok(MenuResult::Back),
            PickerKey::Character('?') => {
                number_buffer.clear();
                print_help(help, terminal_columns);
                print_selection(selected, &options[selected], terminal_columns);
            }
            PickerKey::Cancel | PickerKey::Character('q' | 'Q') => return Ok(MenuResult::Cancel),
            _ => {}
        }
    }
}
```

Keep the `selected` binding computed before this block (`let selected = default.min(options.len() - 1);`); it is no longer `mut` in `menu`.

- [ ] **Step 4: Rewrite `secret` on the same reader**

Replace `secret` from `let guard = RawGuard::enter()?;` to the end with:

```rust
    let mut keys = PickerInput::open().context("opening secret input")?;
    let guard = RawGuard::enter()?;
    let result = read_secret(&mut keys, max_bytes);
    drop(guard);
    println!();
    result
}

fn read_secret(keys: &mut PickerInput, max_bytes: usize) -> Result<Option<String>> {
    let mut value = String::new();
    loop {
        let key = match keys.read_key() {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(error) => return Err(error).context("reading hidden input"),
        };
        match key {
            PickerKey::Enter => return Ok(Some(value)),
            PickerKey::Backspace => {
                value.pop();
            }
            PickerKey::Cancel | PickerKey::Character('\u{4}') => return Ok(None),
            PickerKey::Character(character) if !character.is_control() => {
                if value.len() + character.len_utf8() <= max_bytes {
                    value.push(character);
                }
            }
            _ => {}
        }
    }
}
```

Pasted text arrives as ordinary characters because bracketed paste is never enabled, so the old `Event::Paste` arm has no replacement.

- [ ] **Step 5: Remove the crossterm event imports**

Delete `event`, `Event`, `KeyCode`, `KeyEvent`, `KeyModifiers` from the `use crossterm::…` line at the top of `src/promptui.rs`. `cargo build` must report no unused-import warnings; `grep -n "event::read" src/promptui.rs` must print nothing.

- [ ] **Step 6: Run the unit tests**

Run: `cargo test --lib promptui::tests -- --nocapture`
Expected: PASS, including the two new tests and the existing `picker_to_confirmation_keeps_following_tty_bytes`.

- [ ] **Step 7: Write the PTY regression test**

Create `tests/in_shell_menus_pty.py`:

```python
#!/usr/bin/env python3
"""Menus launched from inside `aishe zsh` must read keys and exit cleanly."""

import fcntl
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)
CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")


class Pty:
    def __init__(self, env, cols=100, rows=30):
        self.master, slave = pty.openpty()
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            [BINARY, "zsh"], stdin=slave, stdout=slave, stderr=slave, env=env,
            preexec_fn=lambda: (os.setsid(), fcntl.ioctl(0, termios.TIOCSCTTY, 0)),
            close_fds=True,
        )
        os.close(slave)
        self.transcript = ""

    def drain(self, seconds=0.2):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], 0.1)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                return
            if not chunk:
                return
            self.transcript += chunk.decode("utf-8", "replace")

    def expect(self, text, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if text in self.transcript:
                return True
            self.drain()
        return text in self.transcript

    def send(self, keys):
        os.write(self.master, keys.encode())

    def plain(self):
        return CSI.sub("", self.transcript)

    def ready(self):
        self.send("print -r -- MENU_PTY_''READY\r")
        return self.expect("MENU_PTY_READY")

    def close(self):
        try:
            os.close(self.master)
        except OSError:
            pass
        if self.proc.poll() is None:
            try:
                os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass


def environment():
    home = tempfile.mkdtemp(prefix="aishe-menus-")
    config_dir = os.path.join(home, ".config", "aishe")
    os.makedirs(config_dir)
    with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
        file.write(
            "version = 2\n[aishe]\nmode = \"auto\"\nprovider = \"anthropic\"\n"
            "pty_prompt = true\n\n[providers.anthropic]\n"
            "base_url = \"https://api.anthropic.com\"\napi_key_env = \"UNUSED_FAKE_KEY\"\n"
            "model = \"menu-model\"\n\n[backend]\nengine = \"native\"\n"
        )
    with open(os.path.join(home, ".zshrc"), "w", encoding="utf-8") as file:
        file.write("unset HISTFILE\n")
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir)
    os.symlink(BINARY, os.path.join(bin_dir, "aishe"))
    env = dict(os.environ)
    env.pop("NO_COLOR", None)
    env.update({
        "HOME": home,
        "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
        "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
        "ZDOTDIR": home,
        "ZSH_DISABLE_COMPFIX": "true",
        "TERM": "xterm-256color",
        "PATH": bin_dir + ":" + os.environ.get("PATH", ""),
    })
    return env


def check_menu(shell, command, expected_row, marker):
    start = len(shell.transcript)
    shell.send(command + "\r")
    if not shell.expect(expected_row):
        raise AssertionError("%s did not paint its menu:\n%s" % (command, shell.transcript[start:][-2000:]))
    shell.drain(0.3)
    shell.send("\x1b")  # Esc cancels the menu
    shell.drain(0.8)
    shell.send("print -r -- %s_''OK\r" % marker)
    if not shell.expect("%s_OK" % marker):
        raise AssertionError("keystroke after %s was swallowed:\n%s" % (command, shell.transcript[start:][-2000:]))
    segment = shell.plain()[start:]
    for forbidden in ("Failed to initialize input reader", "io.operation_failed", "internal.unexpected"):
        if forbidden in segment:
            raise AssertionError("%s printed %r:\n%s" % (command, forbidden, segment[-2000:]))


def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        check_menu(shell, "/settings", "Exit without changes", "SETTINGS")
        check_menu(shell, "aishe setup", "Continue setup", "SETUP")
        check_menu(shell, "aishe tour", "Lesson 1", "TOUR")
        print("in-shell menus: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
```

If `aishe tour` in this fake environment prints a different first row, read its first menu title from `src/tour.rs` (`Lesson 1 of 8` at the time of writing) and use that literal.

- [ ] **Step 8: Build and run the PTY test**

Run: `cargo build --release && python3 tests/in_shell_menus_pty.py target/release/aishe`
Expected: `in-shell menus: ok`. Before Step 3 this test fails with `Failed to initialize input reader`; keep that failing run in your notes as the reproduction.

- [ ] **Step 9: Commit**

```bash
git add src/promptui.rs tests/in_shell_menus_pty.py
git commit -m "fix: read menu and secret keys through the picker input so in-shell settings, setup, and tour work

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Declining the yolo consent prompt is a normal outcome

**Why:** `ensure_yolo_acceptance` (`src/cli/runtime.rs:313-407`) reads the answer with a cooked `read_line` and then `bail!("yolo scope was not accepted")`. `main.rs:499` turns that into `emit_from`, so a plain "n" prints `[internal.unexpected] … create a redacted support bundle`. Because the read is cooked, a second Shift-Tab prints `^[[Z` into the prompt and Esc does nothing. The shell side (`aishe-cycle-mode`, `zsh_hook.zsh:568-591`) already handles a non-zero exit by staying on `auto`; only the process output is wrong.

**Files:**
- Modify: `src/cli/runtime.rs:313-407`, `src/cli/runtime.rs:523`, `src/main.rs:497-505`, `src/promptui.rs` (make `read_confirmation_line` public as `read_terminal_line`)
- Test: `src/cli/runtime.rs` unit test, `tests/yolo_consent_pty.py` (new)

**Interfaces:**
- Produces: `pub enum YoloAcceptance { Accepted, Declined }`; `pub fn ensure_yolo_acceptance(config: &Config) -> Result<YoloAcceptance>`; `pub fn yolo_answer_accepts(answer: Option<&str>, expected: &str) -> bool`; `pub fn promptui::read_terminal_line(echo: bool) -> Result<Option<String>>`.

- [ ] **Step 1: Write the failing unit test**

In `src/cli/runtime.rs` tests module (create `#[cfg(test)] mod yolo_tests { use super::*; … }` if the file has none):

```rust
    #[test]
    fn yolo_answer_requires_the_exact_word() {
        assert!(yolo_answer_accepts(Some("yolo"), "yolo"));
        assert!(yolo_answer_accepts(Some("  yolo \n"), "yolo"));
        assert!(!yolo_answer_accepts(Some("n"), "yolo"));
        assert!(!yolo_answer_accepts(Some("yolo"), "yolo-host"));
        assert!(!yolo_answer_accepts(None, "yolo"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib yolo_answer_requires_the_exact_word`
Expected: compile error, `yolo_answer_accepts` not found.

- [ ] **Step 3: Expose the raw-mode line reader in promptui**

In `src/promptui.rs`, rename `fn read_confirmation_line()` to `pub fn read_terminal_line(echo: bool)` and pass `echo` through to `read_terminal_confirmation_line(&mut keys, echo)`. Update the one caller in `confirm` to `read_terminal_line(true)?`. The 16-byte cap inside `read_terminal_confirmation_line` stays; `yolo-host` is nine bytes.

- [ ] **Step 4: Return a typed outcome from the consent flow**

In `src/cli/runtime.rs` add above `ensure_yolo_acceptance`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum YoloAcceptance {
    Accepted,
    Declined,
}

pub fn yolo_answer_accepts(answer: Option<&str>, expected: &str) -> bool {
    answer.map(str::trim).is_some_and(|value| value == expected)
}
```

Change the signature to `pub fn ensure_yolo_acceptance(config: &Config) -> Result<YoloAcceptance>` and every early `return Ok(())` inside it to `return Ok(YoloAcceptance::Accepted)`. Replace the block from `std::io::stdout().flush().ok();` through `anyhow::bail!("yolo scope was not accepted");` with:

```rust
    std::io::stdout().flush().ok();
    let answer = crate::promptui::read_terminal_line(true).context("reading yolo acceptance")?;
    let expected = match scope {
        ExecutionScope::Workspace => "yolo",
        ExecutionScope::Host => "yolo-host",
    };
    if !yolo_answer_accepts(answer.as_deref(), expected) {
        let current = std::env::var("AISHE_MODE")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| config.aishe.mode.clone());
        println!("yolo not enabled · mode stays {current}");
        return Ok(YoloAcceptance::Declined);
    }
```

Delete the `StdinEchoGuard::visible()` block; the raw-mode reader echoes for itself. If `StdinEchoGuard` has no other users after this (`grep -n StdinEchoGuard src`), delete it.

- [ ] **Step 5: Map the outcome at both call sites**

`src/main.rs:497-505`:

```rust
    if args.accept_yolo {
        aishe::cli::history::init_audit(&config);
        return match aishe::cli::runtime::ensure_yolo_acceptance(&config) {
            Ok(aishe::cli::runtime::YoloAcceptance::Accepted) => Ok(0),
            Ok(aishe::cli::runtime::YoloAcceptance::Declined) => Ok(1),
            Err(error) => {
                aishe::cli::error_contract::emit_from(error.as_ref());
                Ok(1)
            }
        };
    }
```

`src/cli/runtime.rs:523` (`ensure_yolo_acceptance(config)?;` inside the yolo-line path): replace with

```rust
        if ensure_yolo_acceptance(config)? == YoloAcceptance::Declined {
            return Ok(0);
        }
```

(`return Ok(0)` is the same value that path returns for a cancelled turn; confirm by reading the surrounding function.)

- [ ] **Step 6: Run the unit test and the whole crate**

Run: `cargo test --lib yolo_answer_requires_the_exact_word && cargo build --release`
Expected: PASS, clean build.

- [ ] **Step 7: Write the PTY test**

Create `tests/yolo_consent_pty.py` by copying the `Pty`, `environment`, and imports from `tests/in_shell_menus_pty.py` (keep each PTY test self-contained), then:

```python
def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        start = len(shell.transcript)
        shell.send("\x1b[Z")  # Shift-Tab: auto -> yolo consent
        if not shell.expect("Type yolo to continue"):
            raise AssertionError("consent prompt did not appear:\n" + shell.transcript[start:])
        shell.send("n\r")
        if not shell.expect("mode stays auto"):
            raise AssertionError("decline was not acknowledged:\n" + shell.transcript[start:])
        shell.send("\x1b[Z")
        if not shell.expect("Type yolo to continue", timeout=10):
            raise AssertionError("second consent prompt did not appear")
        shell.send("\x1b")  # Esc cancels
        shell.drain(0.8)
        shell.send("print -r -- MODE=$AISHE_MODE\r")
        if not shell.expect("MODE=auto"):
            raise AssertionError("mode did not stay auto:\n" + shell.plain()[start:])
        segment = shell.plain()[start:]
        for forbidden in ("internal.unexpected", "support bundle", "^[[Z"):
            if forbidden in segment:
                raise AssertionError("consent flow printed %r:\n%s" % (forbidden, segment))
        print("yolo consent: ok")
    finally:
        shell.close()
```

- [ ] **Step 8: Run the PTY test**

Run: `python3 tests/yolo_consent_pty.py target/release/aishe`
Expected: `yolo consent: ok`.

- [ ] **Step 9: Commit**

```bash
git add src/cli/runtime.rs src/main.rs src/promptui.rs tests/yolo_consent_pty.py
git commit -m "fix: treat a declined yolo consent as a cancel and read it in raw mode

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: The palette repaints the prompt, fills slash forms, and `/palette` works

**Why:** Three defects in one feature. (1) `aishe-command-palette` (`zsh_hook.zsh:290-302`) prints a multi-row picker from inside a ZLE widget without `zle -I`, so ZLE redraws the buffer relative to a stale cursor: after Enter or Esc the user sees a blank line with no prompt. Slash dispatch at line 528 already calls `zle -I`. (2) `src/palette.rs:49-61` fills `aishe mode`, `aishe commands`, `aishe session fork`, whose effects differ from `/mode`, `/help`, `/fork` (`aishe mode auto` saves config and leaves the live glyph alone). (3) `/palette` typed as a command renders the generic CLI hook (`registry.rs:117-119`) with no `AISHE_PALETTE_FILE`, so a selection is printed as text. On cancel `/` stays in the buffer, so Enter reopens the palette.

**Files:**
- Modify: `src/integration/assets/zsh_hook.zsh:290-302, 508-512`, `src/integration/assets/bash_hook.bash` (dispatch case), `src/palette.rs:19-67`, `src/command_surface.rs:170-181` (`hook_action`), `src/integration/registry.rs:129-190` (`render_hook_action`), `src/product_help.rs:252` (`effect_label` becomes `pub`)
- Test: `src/palette.rs` tests, `src/integration/tests.rs`, `tests/palette_pty.py` (new)

**Interfaces:**
- Produces: `ShellHookAction::Palette`; palette `Entry.invocation` is `/alias` for every command with a slash alias; `pub fn product_help::effect_label(spec: &CommandSpec) -> &'static str`.

- [ ] **Step 1: Write the failing unit tests**

In `src/palette.rs` `mod tests`, add:

```rust
    #[test]
    fn entries_fill_slash_forms_and_name_their_effect() {
        let entries = entries(&Config::default());
        let mode = entries.iter().find(|entry| entry.id == "mode").expect("mode entry");
        assert_eq!(mode.invocation, "/mode");
        assert!(mode.label.starts_with("/mode — Inspect or select suggest, auto, or yolo mode · "));
        let help = entries.iter().find(|entry| entry.id == "help").expect("help entry");
        assert_eq!(help.invocation, "/help");
        assert!(entries.iter().all(|entry| !entry.label.starts_with("aishe ")));
    }
```

In `src/integration/tests.rs`, add:

```rust
#[test]
fn palette_widget_invalidates_zle_and_clears_the_buffer_on_cancel() {
    let hook = include_str!("assets/zsh_hook.zsh");
    let widget = hook
        .split("aishe-command-palette() {")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("palette widget body");
    assert!(widget.contains("zle -I\n"), "palette must invalidate the ZLE display before drawing");
    assert!(widget.contains("BUFFER=\"\""), "cancel must clear the buffer");
    let rendered = zsh_hook();
    assert!(rendered.contains("    palette)\n      aishe-command-palette\n"));
}
```

(`zsh_hook()` is the existing helper in that test module that renders the generated hook; if it is named differently, use the helper the neighbouring tests use.)

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib palette::tests::entries_fill_slash_forms && cargo test --lib integration::tests::palette_widget_invalidates`
Expected: FAIL (`aishe mode` invocation; missing `zle -I`).

- [ ] **Step 3: Fix the zsh widget and the accept-line intercept**

Replace `aishe-command-palette` in `src/integration/assets/zsh_hook.zsh` with:

```zsh
aishe-command-palette() {
  emulate -L zsh
  local handoff="${TMPDIR:-/tmp}/aishe-palette-${AISHE_SHELL_ID}"
  command rm -f "$handoff"
  # The picker paints below the prompt; tell ZLE its display is gone so the
  # prompt and buffer are repainted from scratch when the widget returns.
  zle -I
  AISHE_PALETTE_FILE="$handoff" command aishe palette <&$_AISHE_INPUT_FD
  if [[ -r "$handoff" ]]; then
    BUFFER="$(<"$handoff")"
    CURSOR=${#BUFFER}
    command rm -f "$handoff"
  else
    BUFFER=""
    CURSOR=0
    zle -M "aishe: palette closed"
  fi
}
```

In `aishe-accept-line`, change the first branch to:

```zsh
  if [[ "$BUFFER" == "/" || "$BUFFER" == "/palette" ]]; then
    aishe-command-palette
    return
```

- [ ] **Step 4: Register a dedicated hook action**

`src/command_surface.rs` `hook_action`: add `"palette" => ShellHookAction::Palette,` before the `_ => ShellHookAction::Cli` arm, and add `Palette,` to the `ShellHookAction` enum. In `src/integration/registry.rs` `render_hook_action`, add:

```rust
        ShellHookAction::Palette => match shell {
            HookShell::Zsh => "      aishe-command-palette\n".to_string(),
            HookShell::Bash => format!(
                "{}        _aishe_palette_handoff=\"${{TMPDIR:-/tmp}}/aishe-palette-${{AISHE_SHELL_ID:-$$}}\"\n        command rm -f \"$_aishe_palette_handoff\"\n        AISHE_PALETTE_FILE=\"$_aishe_palette_handoff\" command aishe palette < /dev/tty > /dev/tty 2>&1\n        if [ -r \"$_aishe_palette_handoff\" ]; then\n          printf 'fill\\n%s\\n' \"$(cat \"$_aishe_palette_handoff\")\" > \"$AISHE_PENDING_FILE\"\n          command rm -f \"$_aishe_palette_handoff\"\n        fi\n{}",
                no_argument_guard(),
                close_no_argument_guard()
            ),
        },
```

Check that the bash hook's pending-file consumer handles the `fill` action (grep `fill)` in `bash_hook.bash`); it stages the command for review the same way `_aishe_stage_command` does in zsh.

- [ ] **Step 5: Fill slash forms from the palette**

In `src/palette.rs` replace the `filter_map` closure in `entries` with:

```rust
        .filter_map(|spec| {
            let invocation = if let Some(alias) = spec.slash_aliases.first() {
                format!("/{alias}")
            } else {
                let cli = spec.cli?;
                let mut words = vec!["aishe", cli.command];
                words.extend(cli.prefix_args);
                words.join(" ")
            };
            Some(Entry {
                id: spec.id.into(),
                label: format!(
                    "{invocation} — {} · {}",
                    spec.summary,
                    crate::product_help::effect_label(spec)
                ),
                invocation,
                effect: effect(spec.side_effects).into(),
                available: true,
                note: None,
            })
        })
```

Make `effect_label` in `src/product_help.rs` `pub fn`. The sort key `a.invocation.cmp(&b.invocation)` still works.

- [ ] **Step 6: Run the unit tests**

Run: `cargo test --lib palette:: && cargo test --lib integration::tests`
Expected: PASS. Fix any existing palette test that asserted the `aishe …` prefix by updating its expectation to the slash form.

- [ ] **Step 7: Write the PTY test**

Create `tests/palette_pty.py` (copy the `Pty`/`environment` helpers from `tests/in_shell_menus_pty.py`; in `environment()` write `.zshrc` as `"unset HISTFILE\nPROMPT='USER_PROMPT> '\nexport AISHE_PTY_PROMPT=force\n"` so the prompt text is greppable) with:

```python
def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        start = len(shell.transcript)
        shell.send("/\t")
        if not shell.expect("AIShe command palette"):
            raise AssertionError("palette did not open:\n" + shell.transcript[start:])
        shell.send("\x1b")
        if not shell.expect("palette closed"):
            raise AssertionError("palette did not report closing")
        shell.drain(0.5)
        shell.send("print -r -- PAL_''OK\r")
        if not shell.expect("PAL_OK"):
            raise AssertionError("buffer still held '/' after cancel:\n" + shell.plain()[start:])
        after_close = shell.plain().rsplit("palette closed", 1)[1]
        if "USER_PROMPT>" not in after_close:
            raise AssertionError("prompt was not repainted after cancel:\n" + after_close)
        start = len(shell.transcript)
        shell.send("/\t")
        shell.expect("AIShe command palette")
        shell.send("\r")  # first entry
        shell.drain(0.8)
        shell.send("\x03")  # Ctrl-C shows the filled buffer then clears it
        shell.drain(0.5)
        filled = shell.plain()[start:]
        if "USER_PROMPT> /" not in filled:
            raise AssertionError("selection did not fill a slash form on the prompt line:\n" + filled)
        print("palette: ok")
    finally:
        shell.close()
```

- [ ] **Step 8: Run it**

Run: `cargo build --release && python3 tests/palette_pty.py target/release/aishe`
Expected: `palette: ok`.

- [ ] **Step 9: Commit**

```bash
git add src/integration/assets/zsh_hook.zsh src/integration/registry.rs src/command_surface.rs src/palette.rs src/product_help.rs src/integration/tests.rs tests/palette_pty.py
git commit -m "fix: repaint the prompt after the palette and fill slash forms

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Reset SIGPIPE so `aishe … | head` never panics

**Why:** Rust ignores SIGPIPE by default, so a closed pipe becomes an `EPIPE` write error and `println!` panics (`aishe log | head -1`); `clap_complete::generate` unwraps its write (`src/main.rs:244`). Restoring the default disposition makes the process exit quietly with status 141 like every other Unix CLI.

**Files:**
- Modify: `src/main.rs:26`
- Test: `tests/cli.rs`

- [ ] **Step 1: Write the failing integration test**

Append to `tests/cli.rs`:

```rust
#[test]
fn closing_stdout_early_does_not_panic() {
    let bin = assert_cmd::cargo::cargo_bin("aishe");
    for args in ["completions zsh", "--help"] {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("'{}' {args} | head -n 1", bin.display()))
            .output()
            .expect("run pipeline");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!stderr.contains("panicked"), "{args}: {stderr}");
        assert!(!stderr.contains("Broken pipe"), "{args}: {stderr}");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli closing_stdout_early_does_not_panic`
Expected: FAIL with `failed to write completion file: … BrokenPipe`.

- [ ] **Step 3: Restore the default SIGPIPE disposition**

At the top of `fn main()` in `src/main.rs`:

```rust
fn main() -> ExitCode {
    // Rust starts with SIGPIPE ignored; restore the Unix default so a closed
    // pipe (`aishe log | head`) ends the process quietly instead of panicking.
    #[cfg(unix)]
    // SAFETY: setting a signal disposition before any thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    match run() {
```

`libc` is already a dependency of the crate (`src/cli/runtime.rs:26` uses it).

- [ ] **Step 4: Run the test**

Run: `cargo test --test cli closing_stdout_early_does_not_panic`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs tests/cli.rs
git commit -m "fix: restore default SIGPIPE handling so piped output never panics

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: User-input errors surface as `cli.*`, never as `internal.unexpected`

**Why:** `UserError::from_error` (`src/user_error.rs:356-364`) classifies any unrecognised `anyhow::bail!` as `internal.unexpected` with "create a redacted support bundle". A typo'd connection id, `mcp add foo`, `task show nope`, or `aishe reset` outside a shell all hit it. The same typo through `connection use` prints a plain lowercase line (exit 1) and through `session show` leaks a filesystem path as `io.operation_failed` (exit 10). Two `cli.*` sites return exit 1 where the contract (`docs/automation.md:49-51`) says 2. The fix is one typed error that `from_error` recognises, used at every user-input site.

**Files:**
- Modify: `src/user_error.rs`, `src/config.rs:1292`, `src/tasks.rs:406`, `src/cli/session.rs:169, 271`, `src/cli/connection.rs:239-244, 809-815`, `src/main.rs:734-742, 901-908`, `src/cli/error_contract.rs:240-251` (code list), `docs/troubleshooting.md`
- Test: `src/user_error.rs` tests, `tests/cli.rs`

**Interfaces:**
- Produces: `pub struct UserFacing { pub namespace: ErrorNamespace, pub name: &'static str, pub message: String, pub next_action: &'static str }` with `UserFacing::cli(name, message, next_action) -> anyhow::Error` and `UserFacing::new(namespace, name, message, next_action) -> anyhow::Error`. `from_error` maps it to `namespace.name` with the namespace's exit code.

- [ ] **Step 1: Write the failing unit test**

In `src/user_error.rs` tests:

```rust
    #[test]
    fn user_facing_errors_keep_their_namespace_through_from_error() {
        let error = UserFacing::cli(
            "unknown_connection",
            "Unknown connection or provider 'nope'.",
            "Run `aishe connection list` to see the available ids.",
        );
        let public = UserError::from_error(error.as_ref());
        assert_eq!(public.code().as_str(), "cli.unknown_connection");
        assert_eq!(public.exit_code(), 2);
        assert!(!public.render_text().contains("support bundle"));
        // Wrapped with context, the typed error is still found in the chain.
        let wrapped = error.context("selecting a connection");
        assert_eq!(UserError::from_error(wrapped.as_ref()).code().as_str(), "cli.unknown_connection");
    }
```

(`UserErrorCode::as_str` may be named differently; use the accessor the existing tests use.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib user_facing_errors_keep_their_namespace`
Expected: compile error, `UserFacing` not found.

- [ ] **Step 3: Add the typed error and the chain walk**

In `src/user_error.rs`:

```rust
/// An error caused by user input or an unsupported invocation. `from_error`
/// preserves its namespace and code instead of falling back to
/// `internal.unexpected`.
#[derive(Debug)]
pub struct UserFacing {
    pub namespace: ErrorNamespace,
    pub name: &'static str,
    pub message: String,
    pub next_action: &'static str,
}

impl fmt::Display for UserFacing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for UserFacing {}

impl UserFacing {
    pub fn new(
        namespace: ErrorNamespace,
        name: &'static str,
        message: impl Into<String>,
        next_action: &'static str,
    ) -> anyhow::Error {
        anyhow::Error::new(Self { namespace, name, message: message.into(), next_action })
    }

    pub fn cli(name: &'static str, message: impl Into<String>, next_action: &'static str) -> anyhow::Error {
        Self::new(ErrorNamespace::Cli, name, message, next_action)
    }
}
```

At the top of `from_error`, before the keyword classification:

```rust
        let mut current: Option<&(dyn Error + 'static)> = Some(error);
        while let Some(candidate) = current {
            if let Some(facing) = candidate.downcast_ref::<UserFacing>() {
                return Self::classified(facing.namespace, facing.name, &facing.message, facing.next_action)
                    .expect("user-facing code is valid")
                    .with_detail(chain_of(error));
            }
            current = candidate.source();
        }
```

`from_error`'s parameter must be `&(dyn Error + 'static)` for `downcast_ref`; `main.rs` passes `error.as_ref()` from an `anyhow::Error`, which satisfies it. `chain_of` is whatever helper the function already uses to build `chain` (build the chain once and reuse it).

- [ ] **Step 4: Convert the user-input sites**

- `src/config.rs:1292`: `[] => return Err(crate::user_error::UserFacing::cli("unknown_connection", format!("Unknown connection or provider '{value}'."), "Run `aishe connection list` to see the available ids."))` (adjust to the function's return type; it already returns `anyhow::Result`).
- `src/cli/connection.rs:239-244` and `809-815`: replace the `eprintln!("aishe: {error}"); return 1;` pairs with

```rust
            crate::cli::error_contract::emit_classified(
                crate::user_error::ErrorNamespace::Cli,
                "unknown_connection",
                &format!("Unknown connection or provider '{value}'."),
                "Run `aishe connection list` to see the available ids.",
                None,
            );
            return crate::user_error::ErrorNamespace::Cli.exit_code();
```

- `src/tasks.rs:406`: before `std::fs::read`, `if !path.exists() { return Err(crate::user_error::UserFacing::cli("unknown_task", format!("No task or session '{id}'."), "Run `aishe sessions` to list what can be resumed.")); }` (use the id variable in scope).
- `src/cli/session.rs:169` and `271`: `return Err(UserFacing::cli("shell_required", "`aishe reset` only works inside an AIShe shell.", "Start `aishe`, then run `/reset`."))` (same shape for `session fork`).
- `src/main.rs:742` and `908`: `return Ok(aishe::user_error::ErrorNamespace::Cli.exit_code());`.

- [ ] **Step 5: Register the new codes**

Add `"cli.unknown_connection"`, `"cli.unknown_task"`, `"cli.shell_required"` to the `codes` list in `src/cli/error_contract.rs:240-251` and add one row each to the code table in `docs/troubleshooting.md`, in the same format as the existing rows, with the `Next:` text from Step 4. Fix the existing `cli.invalid_provider_flags` row (`docs/troubleshooting.md:31`) to say "`--live`/`--json` were passed to `aishe provider` without the `test` action" (Phase 2 removes this code entirely; the row stays accurate until then).

- [ ] **Step 6: Write the integration test**

Append to `tests/cli.rs` (use the `temp_home`/`aishe` helpers already in the file, or copy them from `tests/command_surface.rs:10-52`):

```rust
#[test]
fn unknown_connection_is_a_cli_error_with_exit_2_everywhere() {
    let home = temp_home("unknown-connection");
    for args in [
        vec!["--connection", "nope", "status"],
        vec!["connection", "show", "nope"],
        vec!["connection", "use", "nope"],
    ] {
        let output = aishe(&home).args(&args).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("[cli.unknown_connection]"), "{args:?}: {stderr}");
        assert!(!stderr.contains("support bundle"), "{args:?}: {stderr}");
    }
}
```

- [ ] **Step 7: Run everything**

Run: `cargo test --lib user_error && cargo test --test cli unknown_connection && cargo test --lib error_contract`
Expected: PASS (the error-contract test checks each code has exactly one troubleshooting row).

- [ ] **Step 8: Commit**

```bash
git add src/user_error.rs src/config.rs src/tasks.rs src/cli/session.rs src/cli/connection.rs src/main.rs src/cli/error_contract.rs docs/troubleshooting.md tests/cli.rs
git commit -m "fix: classify user-input errors as cli.* with exit 2 instead of internal.unexpected

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: `/mode` and `aishe mode` agree; slash arguments are word-split

**Why:** The hook's `/mode` reads and sets `AISHE_MODE` (this shell) while `aishe mode` reads config and saves config, never looks at `AISHE_MODE`, and has no handoff to the parent shell (`src/cli/connection.rs:782-806`; `scope` and `output` do hand off at 845-858). After Shift-Tab, `aishe mode` reports the wrong mode and `aishe mode auto` changes nothing visible. Separately, `render_cli_hook` (`registry.rs:117-119`) passes the whole remainder of an `OptionalValue` line as one argv word, so `/model gpt-5.5 --default` becomes `aishe model 'gpt-5.5 --default'`, and the zsh `PassThrough` path keeps quote characters because `(z)` splits without unquoting.

**Files:**
- Modify: `src/integration/registry.rs:108-127` (`render_cli_hook`), `registry.rs:166-188` (`SessionMode`), `registry.rs` (`_aishe_apply_session_mode` message), `src/integration/assets/zsh_hook.zsh:1` (export pending file), `src/cli/args.rs:333-337` (`Mode` gets `--default`), `src/main.rs:668-674` (dispatch), `src/cli/connection.rs` (new `mode` function), `src/command_surface.rs:314-326` (mode spec)
- Test: `src/integration/tests.rs`, `src/cli/connection.rs` tests, `tests/mode_handoff_pty.py` (new)

**Interfaces:**
- Produces: `pub struct ShellContext { pub in_shell: bool, pub pending_file: Option<PathBuf>, pub env_mode: Option<String> }` with `ShellContext::from_env()`; `pub fn mode(effective: &Config, value: Option<&str>, save_default: bool, shell: &ShellContext) -> u8`.

- [ ] **Step 1: Write the failing tests**

`src/integration/tests.rs`:

```rust
#[test]
fn optional_value_hooks_word_split_and_unquote_their_arguments() {
    let hook = zsh_hook();
    assert!(!hook.contains("command aishe model \"$_aishe_arg\""));
    assert!(hook.contains("_aishe_args=(\"${(Q@)${(z)_aishe_arg}}\")"));
    assert!(hook.contains("printf 'mode: %s (this shell)\\n'"));
    assert!(!hook.contains("mode = %s  (this shell)"));
}
```

`src/cli/connection.rs` tests:

```rust
    #[test]
    fn mode_inside_a_shell_hands_off_through_the_pending_file() {
        let dir = std::env::temp_dir().join(format!("aishe-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pending = dir.join("pending");
        let shell = ShellContext { in_shell: true, pending_file: Some(pending.clone()), env_mode: Some("auto".into()) };
        let config = Config::default();
        assert_eq!(mode(&config, Some("suggest"), false, &shell), 0);
        assert_eq!(std::fs::read_to_string(&pending).unwrap(), "mode\nsuggest\n");
    }

    #[test]
    fn mode_show_prefers_the_shell_environment() {
        let shell = ShellContext { in_shell: true, pending_file: None, env_mode: Some("yolo".into()) };
        assert_eq!(mode_show_line(&Config::default(), &shell), "mode: yolo (this shell)");
        let detached = ShellContext { in_shell: false, pending_file: None, env_mode: None };
        assert_eq!(mode_show_line(&Config::default(), &detached), "mode: suggest (default for new shells)");
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib optional_value_hooks_word_split && cargo test --lib mode_inside_a_shell`
Expected: FAIL / compile error.

- [ ] **Step 3: Word-split optional values in both hooks**

In `render_cli_hook` (`src/integration/registry.rs`), delete the `ArgumentPolicy::OptionalValue(_) => …` arm and change the `PassThrough` arm's pattern to `ArgumentPolicy::OptionalValue(_) | ArgumentPolicy::PassThrough(_)`. In the zsh branch replace `_aishe_args=(\"${{(z)_aishe_arg}}\")` with `_aishe_args=(\"${{(Q@)${{(z)_aishe_arg}}}}\")`. (In the Rust `format!` string that is `(\"${{(Q@)${{(z)_aishe_arg}}}}\")`, which renders as `("${(Q@)${(z)_aishe_arg}}")`.)

- [ ] **Step 4: Give `aishe mode` a shell context and a `--default` flag**

`src/cli/args.rs:333-337`:

```rust
    /// Show or set the interaction mode for this shell; `--default` also saves it for new shells.
    Mode {
        #[arg(value_parser = ["suggest", "auto", "yolo"])]
        value: Option<String>,
        /// Also save the mode as the default for new shells
        #[arg(long)]
        default: bool,
    },
```

`src/cli/connection.rs`, add:

```rust
pub struct ShellContext {
    pub in_shell: bool,
    pub pending_file: Option<std::path::PathBuf>,
    pub env_mode: Option<String>,
}

impl ShellContext {
    pub fn from_env() -> Self {
        let non_empty = |key: &str| std::env::var(key).ok().filter(|value| !value.is_empty());
        Self {
            in_shell: non_empty("AISHE_SHELL_ID").is_some(),
            pending_file: non_empty("AISHE_PENDING_FILE").map(std::path::PathBuf::from),
            env_mode: non_empty("AISHE_MODE"),
        }
    }
}

pub fn mode_show_line(effective: &Config, shell: &ShellContext) -> String {
    let current = shell.env_mode.clone().unwrap_or_else(|| effective.aishe.mode.clone());
    let scope = if shell.in_shell { "this shell" } else { "default for new shells" };
    format!("mode: {} ({scope})", crate::commands::display_safe(&current))
}

pub fn mode(effective: &Config, value: Option<&str>, save_default: bool, shell: &ShellContext) -> u8 {
    let Some(value) = value else {
        println!("{}", mode_show_line(effective, shell));
        return 0;
    };
    if shell.in_shell {
        if let Some(path) = &shell.pending_file {
            if let Err(error) = std::fs::write(path, format!("mode\n{value}\n")) {
                eprintln!("aishe: {error}");
                return 1;
            }
        }
        println!("mode: {value} (this shell)");
    }
    if save_default || !shell.in_shell {
        let mut cfg = match Config::load_or_init() {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("aishe: {error}");
                return 1;
            }
        };
        cfg.aishe.mode = value.to_string();
        cfg.aishe.safety_profile = "custom".to_string();
        if let Err(error) = cfg.save() {
            eprintln!("aishe: {error}");
            return 1;
        }
        println!("mode: {value} (default for new shells)");
    }
    0
}
```

Remove the `"mode"` arms from `set_or_show` (both the show match and the set match). In `src/main.rs` where `Cmd::Mode { value }` is dispatched (around line 668), call `aishe::cli::connection::mode(&config, value.as_deref(), *default, &aishe::cli::connection::ShellContext::from_env())`.

- [ ] **Step 5: Let the shell consume the handoff**

`src/integration/assets/zsh_hook.zsh:1` becomes:

```zsh
: ${AISHE_PENDING_FILE:=${TMPDIR:-/tmp}/aishe-pending-$$}
export AISHE_PENDING_FILE
```

(The precmd consumer at lines 141-151 already applies `mode\n<value>` through `_aishe_apply_session_mode`, including the yolo consent prompt.) In `registry.rs` change `_aishe_apply_session_mode`'s final line to `printf 'mode: %s (this shell)\n' "$AISHE_MODE"` and the `SessionMode` zsh render to:

```zsh
      if [[ -z "$_aishe_arg" ]]; then
        printf 'mode: %s (this shell)\n' "${AISHE_MODE:-suggest}"
      elif [[ "$_aishe_arg" == *--default* ]]; then
        command aishe mode ${=_aishe_arg} <&$_AISHE_INPUT_FD
      elif (( ${ZSH_SUBSHELL:-0} > 0 )); then
        printf 'mode\n%s\n' "$_aishe_arg" > "$AISHE_PENDING_FILE"
      else
        _aishe_apply_session_mode "$_aishe_arg"
      fi
```

Bash: add the same `--default` branch calling `command aishe mode $_aishe_arg < /dev/tty > /dev/tty 2>&1`.

- [ ] **Step 6: Fix the registry spec**

`src/command_surface.rs` `mode` spec: `side_effects: SideEffectClass::ShellState`, `shell_local: ShellLocalRequirement::RequiredHandoff`, `arguments: ArgumentPolicy::PassThrough("MODE [--default]")`. Run `cargo test --lib product_help` and `cargo test --test command_surface`; regenerate the docs block (see Global constraints) if the row text changed.

- [ ] **Step 7: Run the unit tests**

Run: `cargo test --lib optional_value_hooks_word_split mode_inside_a_shell mode_show_prefers`
Expected: PASS.

- [ ] **Step 8: Write the PTY test**

Create `tests/mode_handoff_pty.py` (copy helpers from `tests/in_shell_menus_pty.py`):

```python
def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("aishe mode\r")
        if not shell.expect("mode: auto (this shell)"):
            raise AssertionError("aishe mode did not read the shell mode:\n" + shell.plain()[-1500:])
        shell.send("aishe mode suggest\r")
        shell.expect("mode: suggest (this shell)")
        shell.send("print -r -- MODE=$AISHE_MODE\r")
        if not shell.expect("MODE=suggest"):
            raise AssertionError("aishe mode did not hand off to the shell:\n" + shell.plain()[-1500:])
        shell.send("/reasoning high --default\r")
        shell.drain(1.0)
        if "unexpected argument" in shell.plain() or "unexpected value" in shell.plain():
            raise AssertionError("slash arguments were not word-split:\n" + shell.plain()[-1500:])
        print("mode handoff: ok")
    finally:
        shell.close()
```

- [ ] **Step 9: Run it**

Run: `cargo build --release && python3 tests/mode_handoff_pty.py target/release/aishe`
Expected: `mode handoff: ok`.

- [ ] **Step 10: Commit**

```bash
git add src/integration src/cli/args.rs src/cli/connection.rs src/main.rs src/command_surface.rs docs/commands.md tests/mode_handoff_pty.py
git commit -m "fix: make aishe mode shell-aware and word-split optional slash arguments

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: `/usage` reports session spend on managed connections

**Why:** `print_usage_summary` (`src/cli/status.rs:300-319`) only knows the legacy in-process provider meter; with the managed OpenCode backend `provider` is `None`, so a working OAuth account is told `usage: provider not configured` one line after `/status` printed `session spend:` correctly.

**Files:**
- Modify: `src/cli/status.rs` (`command` and `print_usage_summary`)
- Test: `src/cli/status.rs` tests

**Interfaces:**
- Produces: `pub fn session_spend_line(config: &Config) -> Option<String>` (the text `status` prints after `session spend:`); `pub fn usage_summary_text(provider: Option<&dyn Provider>, config: &Config) -> String`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn usage_without_a_legacy_provider_reports_the_session_not_a_config_error() {
        let text = usage_summary_text(None, &Config::default());
        assert!(!text.contains("provider not configured"), "{text}");
        assert!(text.starts_with("usage: "), "{text}");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib usage_without_a_legacy_provider`
Expected: compile error.

- [ ] **Step 3: Share the session-spend source**

In `src/cli/status.rs` `command`, the block that computes the `session` variable (the value printed after `session spend:` at lines 152-161; it reads the managed usage journal and `AISHE_STATUS_FILE` metrics) moves into `pub fn session_spend_line(config: &Config) -> Option<String>` returning the same string with the `aishe session: ` prefix already stripped. `command` calls it. Then:

```rust
pub fn usage_summary_text(provider: Option<&dyn Provider>, config: &Config) -> String {
    let mut lines = Vec::new();
    match provider {
        Some(p) => {
            let snap = p.meter().snapshot();
            lines.push(if snap.is_empty() {
                "usage: no model calls yet this session".to_string()
            } else {
                format!("usage: {}", crate::usage::summary(snap, config.active_model(), &config.pricing))
            });
        }
        None => lines.push(format!(
            "usage: {}",
            session_spend_line(config).unwrap_or_else(|| "no model calls yet this session".into())
        )),
    }
    if config.aishe.budget_usd > 0.0 {
        lines.push(format!("budget: ${:.2} (set budget_usd=0 for unlimited)", config.aishe.budget_usd));
    }
    lines.join("\n")
}

pub fn print_usage_summary(provider: Option<&dyn Provider>, config: &Config) {
    println!("{}", usage_summary_text(provider, config));
}
```

Keep whatever early-return branch `print_usage_summary` had before the `match` (the `if … { …; return; }` at lines 290-299) by moving it into `usage_summary_text` as the first branch.

- [ ] **Step 4: Run the test and the status tests**

Run: `cargo test --lib cli::status`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli/status.rs
git commit -m "fix: report session spend from /usage on managed connections

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: Stop hijacking the bare words `reset` and `details`

**Why:** `aishe-accept-line` (`zsh_hook.zsh:513-524`) and `command_not_found_handle` (`bash_hook.bash:64-69`) intercept the bare words `reset` and `details`. `reset` is the ncurses command people type to recover a garbled terminal. Neither word is in `/help`, completions, or the registry.

**Files:**
- Modify: `src/integration/assets/zsh_hook.zsh:513-524`, `src/integration/assets/bash_hook.bash:64-69`, `docs/commands.md:276-278`
- Test: `tests/bare_words_pty.py` (new); update any existing test that sends bare `reset`/`details` (`grep -rn '"reset\\r"\|send("reset")\|send("details")\|"details\\r"' tests/`)

- [ ] **Step 1: Write the failing PTY test**

Create `tests/bare_words_pty.py` (helpers copied from `tests/in_shell_menus_pty.py`; in `environment()` write `.zshrc` as `"unset HISTFILE\nreset() { print -r -- USER_RESET_RAN }\ndetails() { print -r -- USER_DETAILS_RAN }\n"`):

```python
def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("reset\r")
        if not shell.expect("USER_RESET_RAN"):
            raise AssertionError("bare reset was hijacked:\n" + shell.plain()[-1500:])
        shell.send("details\r")
        if not shell.expect("USER_DETAILS_RAN"):
            raise AssertionError("bare details was hijacked:\n" + shell.plain()[-1500:])
        shell.send("/details\r")
        if not shell.expect("details:"):
            raise AssertionError("/details stopped working:\n" + shell.plain()[-1500:])
        print("bare words: ok")
    finally:
        shell.close()
```

(`/details` prints `aishe agent details: …` today; Phase 3 renames it to `details: …`. Use whichever literal the binary prints at the time you run this and update it in Phase 3.)

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 tests/bare_words_pty.py target/release/aishe`
Expected: FAIL, `bare reset was hijacked`.

- [ ] **Step 3: Delete the intercepts**

In `zsh_hook.zsh` delete the two `elif [[ "$BUFFER" == "reset" ]]` and `elif [[ "$BUFFER" == "details" ]]` branches (lines 513-524). In `bash_hook.bash` delete the `case "$line" in details) … esac` block (lines 64-69). In `docs/commands.md:276-278` remove the sentence that documents the bare forms and leave `/reset`, `/details`, and Ctrl-O.

- [ ] **Step 4: Rebuild, run the new test and any test you updated**

Run: `cargo build --release && python3 tests/bare_words_pty.py target/release/aishe && cargo test --lib integration::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/integration/assets docs/commands.md tests/bare_words_pty.py
git commit -m "fix: stop intercepting the bare words reset and details

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: First launch hands off cleanly; the installer's PATH story is true

**Why:** Bare `aishe` with no config runs the wizard and then `load_quiet()?.context("setup did not create a configuration")` (`src/config.rs:1013-1016`), so choosing "Pause and resume later" prints `[config.invalid]`. After Apply the epilogue says "Run: aishe" while the shell launches anyway (`src/setup.rs:1548-1554`). On an arm64 Mac without `/usr/local/bin` the installer falls back to `~/.local/bin`, warns about PATH before the wizard (it scrolls away), and `aishe` then warns it is not on PATH (`install.sh:198-206, 269-275`; `src/main.rs:154-160`).

**Files:**
- Modify: `src/config.rs:1005-1022`, `src/setup.rs:198-225` (`Options`), `src/setup.rs:1543-1554` (epilogue), `install.sh:198-206, 269-275, 307-323`
- Test: `src/setup.rs` tests, `tests/installer_bindir.sh` (new)

**Interfaces:**
- Produces: `Options.launch_follows: bool`; `pub fn completion_next_steps(launch_follows: bool) -> String`; `choose_bindir` shell function; `AISHE_INSTALL_LIB_ONLY=1` sourcing guard in `install.sh`.

- [ ] **Step 1: Write the failing tests**

`src/setup.rs` tests:

```rust
    #[test]
    fn epilogue_matches_what_happens_next() {
        let launching = completion_next_steps(true);
        assert!(launching.contains("Starting your shell"));
        assert!(!launching.contains("Run: aishe"));
        assert!(launching.contains("? install kubectl please"));
        let standalone = completion_next_steps(false);
        assert!(standalone.contains("Run: aishe"));
    }
```

`tests/installer_bindir.sh`:

```sh
#!/bin/sh
# choose_bindir picks a PATH-visible, writable directory, in this order.
set -eu
cd "$(dirname "$0")/.."
AISHE_INSTALL_LIB_ONLY=1 . ./install.sh
got="$(AISHE_BIN_DIR=/tmp/explicit choose_bindir)"
[ "$got" = /tmp/explicit ] || { echo "AISHE_BIN_DIR ignored: $got"; exit 1; }
got="$(PATH=/nonexistent HOME=/tmp/home choose_bindir)"
[ "$got" = /tmp/home/.local/bin ] || { echo "fallback was $got, expected /tmp/home/.local/bin"; exit 1; }
brew="$(mktemp -d)"
got="$(PATH="$brew:/usr/bin" AISHE_HOMEBREW_BIN="$brew" choose_bindir)"
[ "$got" = "$brew" ] || { echo "writable Homebrew bin on PATH was not preferred: $got"; exit 1; }
echo "installer bindir: ok"
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib epilogue_matches_what_happens_next; sh tests/installer_bindir.sh`
Expected: compile error; `choose_bindir: not found`.

- [ ] **Step 3: Pause is not an error; the epilogue knows what comes next**

`src/setup.rs`: add `pub launch_follows: bool` to `Options` (keep `#[derive(Default)]`). Add:

```rust
pub fn completion_next_steps(launch_follows: bool) -> String {
    let mut out = String::new();
    if launch_follows {
        out.push_str("\n  Starting your shell…\n");
    } else {
        out.push_str("\n  Run: aishe\n");
    }
    out.push_str("  Inside AIShe:\n");
    out.push_str("    git status                 runs in zsh\n");
    out.push_str("    explain this repository    asks the agent\n");
    out.push_str("    ? install kubectl please   asks the agent even though `install` is a command\n");
    out.push_str("\n  Run `aishe tour` when you are ready.\n");
    out
}
```

Replace the `else` block at `setup.rs:1548-1554` with `print!("{}", completion_next_steps(options.launch_follows));` (thread `options` to that scope if it is not already in reach; `run(options)` owns it).

`src/config.rs:1013-1016`:

```rust
            if std::io::stdin().is_terminal() {
                let outcome = crate::setup::run(crate::setup::Options {
                    launch_follows: true,
                    ..crate::setup::Options::default()
                })?;
                if !outcome.applied {
                    return Err(crate::user_error::UserFacing::new(
                        crate::user_error::ErrorNamespace::Config,
                        "setup_incomplete",
                        "Setup was paused before a configuration was saved.",
                        "Run `aishe setup --resume` to continue, or `aishe setup --restart` to start over.",
                    ));
                }
                return Self::load_quiet()?.context("setup did not create a configuration");
            }
```

Add `"config.setup_incomplete"` to the error-contract code list and a troubleshooting row (same pattern as Task 5).

- [ ] **Step 4: Installer: choose the bindir once, warn about PATH last, never fail after a successful install**

At the top of `install.sh`, immediately after `set -eu` and the `note`/`err` helpers, add:

```sh
# Pick the install directory: explicit override, then a writable Homebrew bin
# that is already on PATH (arm64 macOS), then /usr/local/bin, then ~/.local/bin.
choose_bindir() {
  if [ -n "${AISHE_BIN_DIR:-}" ]; then printf '%s\n' "$AISHE_BIN_DIR"; return; fi
  brew_bin="${AISHE_HOMEBREW_BIN:-/opt/homebrew/bin}"
  case ":$PATH:" in
    *":$brew_bin:"*) if [ -d "$brew_bin" ] && [ -w "$brew_bin" ]; then printf '%s\n' "$brew_bin"; return; fi ;;
  esac
  if [ -w /usr/local/bin ] 2>/dev/null || [ "$(id -u)" = "0" ]; then printf '%s\n' /usr/local/bin; return; fi
  printf '%s\n' "$HOME/.local/bin"
}
if [ "${AISHE_INSTALL_LIB_ONLY:-0}" = 1 ]; then return 0 2>/dev/null || exit 0; fi
```

Replace the `if … elif … else … fi` block at lines 198-206 with `bindir="$(choose_bindir)"`. Delete the PATH `case` at lines 269-275 and append at the very end of the script:

```sh
case ":$PATH:" in
  *":$bindir:"*) : ;;
  *)
    note "$bindir is not on your PATH. Add it to your shell rc:"
    note "  export PATH=\"$bindir:\$PATH\""
    note "until then, run: $bindir/aishe"
    ;;
esac
```

Change lines 307-313 so the `Run \`aishe doctor\`` note is skipped when `RUN_SETUP=1`, and change the no-TTY branch to `note "--setup skipped: no terminal attached; run '$bindir/aishe setup' from a terminal"` (no `err`, so the script exits 0 after a successful install).

- [ ] **Step 5: Run the tests and the linters**

Run: `cargo test --lib epilogue_matches_what_happens_next && sh -n install.sh && shellcheck install.sh && sh tests/installer_bindir.sh`
Expected: PASS, no shellcheck findings, `installer bindir: ok`.

- [ ] **Step 6: Commit**

```bash
git add src/config.rs src/setup.rs src/cli/error_contract.rs docs/troubleshooting.md install.sh tests/installer_bindir.sh
git commit -m "fix: hand off cleanly from first-run setup and the installer

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: The branded prompt never replaces a theme; the status survives `pty_prompt = false`

**Why:** `pty_prompt.zsh:216-223` assigns `PROMPT="${base_prompt}"` on every normal precmd, so any theme (powerlevel10k, starship, pure, or a plain `PROMPT='%~ %# '`) is replaced, contradicting the README's "prompt untouched". A global `zstyle ':vcs_info:git:*' formats '%b'` (line 11-12) overrides theme patterns, and `vcs_info` runs git on every prompt although `branch` is not a default status item. The whole file is gated on `AISHE_PTY_PROMPT` (line 2), so `pty_prompt = false` also removes the RPROMPT status that `status_line*` settings are supposed to control.

**Files:**
- Modify: `src/integration/assets/pty_prompt.zsh:1-12, 63-64, 214-223`, `src/config.rs:328-332` (doc comment), `docs/front-ends.md:35-47`, `README.md` (feature bullet "Your real zsh, untouched")
- Test: `tests/statusline_pty.py` (environment), `tests/theme_prompt_pty.py` (new)

**Interfaces:**
- Produces: `_AISHE_BRAND_PROMPT` (1 when the user's prompt is zsh's stock prompt or `AISHE_PTY_PROMPT=force`); branch lookup via `git symbolic-ref` only when needed.

- [ ] **Step 1: Write the failing PTY test**

Create `tests/theme_prompt_pty.py` (helpers from `tests/in_shell_menus_pty.py`; make `environment(prompt_line)` take the `.zshrc` prompt line):

```python
def run_case(prompt_line, expect_visible, expect_absent):
    shell = Pty(environment(prompt_line))
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready for %r" % prompt_line)
        shell.drain(0.5)
        plain = shell.plain()
        for text in expect_visible:
            if text not in plain:
                raise AssertionError("%r: expected %r on screen:\n%s" % (prompt_line, text, plain[-1500:]))
        for text in expect_absent:
            if text in plain:
                raise AssertionError("%r: %r must not be on screen:\n%s" % (prompt_line, text, plain[-1500:]))
        shell.send("print -r -- STATUS=$_AISHE_STATUS_TEXT\r")
        if not shell.expect("STATUS=menu-model"):
            raise AssertionError("%r: status text was not composed" % prompt_line)
    finally:
        shell.close()


def main():
    run_case("PROMPT='THEME> '", ["THEME> "], ["»", "❯"])
    run_case("", ["»"], ["THEME> "])
    run_case("PROMPT='THEME> '\nexport AISHE_PTY_PROMPT=force", ["»"], ["THEME> "])
    print("theme prompt: ok")
```

(The config in `environment()` sets `mode = "auto"`, so the branded glyph is `»`; keep `status_line_items` at its default so `model` renders.)

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 tests/theme_prompt_pty.py target/release/aishe`
Expected: FAIL on the first case (`THEME> ` was replaced).

- [ ] **Step 3: Gate the left prompt on a stock prompt, keep the status independent**

`src/integration/assets/pty_prompt.zsh`:

Line 2: `if [[ -o interactive ]]; then` (drop the `AISHE_PTY_PROMPT` test from the file gate).

After the `typeset` block (line 10) add:

```zsh
  # Brand the left prompt only when the user has not set one. zsh's stock
  # prompt is '%m%# '; macOS /etc/zshrc sets '%n@%m %1~ %# '. Anything else is
  # a theme or a personal prompt and stays untouched; the mode glyph then lives
  # in the RPROMPT status. AISHE_PTY_PROMPT=force overrides the detection.
  typeset -gi _AISHE_BRAND_PROMPT=0
  case "${PROMPT-}" in
    ''|'%m%# '|'%n@%m %1~ %# ') _AISHE_BRAND_PROMPT=1 ;;
  esac
  [[ "${AISHE_PTY_PROMPT:-1}" == 0 ]] && _AISHE_BRAND_PROMPT=0
  [[ "${AISHE_PTY_PROMPT:-1}" == force ]] && _AISHE_BRAND_PROMPT=1
```

Delete lines 11-12 (`autoload -Uz vcs_info` and the `zstyle`). Replace lines 63-64 (`vcs_info` / `branch="${vcs_info_msg_0_:-}"`) with:

```zsh
    branch=""
    if [[ -n "${AISHE_PROTECTED_PATTERNS:-}" || ",${AISHE_STATUS_ITEMS:-}," == *,branch,* ]]; then
      branch="$(command git symbolic-ref --short -q HEAD 2>/dev/null)"
    fi
```

Change the prompt assignment condition (line 216-218) to:

```zsh
    if (( _AISHE_BRAND_PROMPT )) && [[ "$_AISHE_PROMPT_HOST" != spaceship &&
          ( "${1:-}" != status-only || "$PROMPT" == "$_AISHE_PROMPT_VALUE" ) ]]; then
```

- [ ] **Step 4: Update the config comment, docs, and the existing statusline test**

`src/config.rs:328-332` doc comment: "In the zsh-PTY front-end, brand the left prompt (`<cwd> <glyph>`, glyph per mode) when your zshrc leaves zsh's stock prompt in place. A theme or a custom `PROMPT` is never replaced; set `AISHE_PTY_PROMPT=force` to brand anyway, or `pty_prompt = false` to never brand. The right-prompt status is controlled by `status_line*`, not by this flag."

`docs/front-ends.md:40-47`: replace the paragraph "The branded prompt overrides your own" with the same rule. `README.md` feature bullet "Your real zsh, untouched" stays true; add "(the mode glyph moves into the status when you run a prompt theme)".

`tests/statusline_pty.py:137-138`: change the `.zshrc` content to `"unset HISTFILE\n"` (stock prompt, so the branded glyph assertions in the `off` case still hold), and grep the file for `USER_PROMPT` to remove any remaining dependency on that literal.

- [ ] **Step 5: Run both PTY tests**

Run: `cargo build --release && python3 tests/theme_prompt_pty.py target/release/aishe && python3 tests/statusline_pty.py target/release/aishe`
Expected: `theme prompt: ok` and the statusline suite's existing success output.

- [ ] **Step 6: Commit**

```bash
git add src/integration/assets/pty_prompt.zsh src/config.rs docs/front-ends.md README.md tests/statusline_pty.py tests/theme_prompt_pty.py
git commit -m "fix: never replace a prompt theme and keep the status independent of pty_prompt

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 11: The statusline shortens instead of vanishing, and measures cells

**Why:** zsh drops the whole RPROMPT when it does not fit beside the left prompt. `pty_prompt.zsh:85-88` budgets against `COLUMNS - 20`, not against the left prompt's real width, so at 80 columns with a 58-cell path the mode disappears. `${#value}` counts code points (CJK model names overflow), an over-budget field hits `continue` so later shorter fields still render, and a user RPROMPT is never subtracted.

**Files:**
- Modify: `src/integration/assets/pty_prompt.zsh:85-90, 150-159, 180-183, 214-223`, `src/config.rs:713-722` (default item order)
- Test: `tests/statusline_pty.py` (new `narrow` case)

- [ ] **Step 1: Write the failing test**

In `tests/statusline_pty.py`, add a case function and call it from `main`:

```python
def run_narrow_case():
    home, env, model = environment("right")
    deep = os.path.join(home, "a-directory-name-that-is-long", "another-long-segment", "and-one-more-segment")
    os.makedirs(deep)
    shell = Pty(env, 80)
    try:
        if not shell.ready():
            raise AssertionError("narrow session never became ready")
        shell.send("cd %s\r" % deep)
        shell.drain(0.8)
        shell.send("print -r -- NARROW=$_AISHE_STATUS_TEXT\r")
        if not shell.expect("NARROW="):
            raise AssertionError("could not read narrow status text")
        line = shell.plain().rsplit("NARROW=", 1)[1].splitlines()[0]
        if "auto" not in line:
            raise AssertionError("narrow status dropped the mode: %r" % line)
        shell.send("print -r -- WIDTH=${(m)#${(%%)PROMPT}}:${(m)#${(%%)RPROMPT}}\r")
        shell.expect("WIDTH=")
        widths = shell.plain().rsplit("WIDTH=", 1)[1].splitlines()[0]
        left, right = (int(part) for part in widths.split(":"))
        if left + right + 1 > 80:
            raise AssertionError("prompt and status do not fit in 80 columns: %s" % widths)
    finally:
        shell.close()
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 tests/statusline_pty.py target/release/aishe`
Expected: FAIL, `narrow status dropped the mode` or the width assertion.

- [ ] **Step 3: Budget against the real left prompt and count cells**

In `aishe_set_prompt`, move the `base_prompt=…` line and the `PROMPT=` assignment block (currently lines 214-223) to just before `status_items=(…)` (line 90) so `PROMPT` is final before the budget is computed. Then replace lines 85-88 with:

```zsh
    local left_plain="${(S)${(%%)PROMPT}//$'\e'\[[0-9;]#m/}"
    local -i left_cells=${(m)#left_plain}
    local -i user_rprompt_cells=0
    if [[ -n "$_AISHE_USER_RPROMPT" ]]; then
      local user_plain="${(S)${(%%)_AISHE_USER_RPROMPT}//$'\e'\[[0-9;]#m/}"
      user_rprompt_cells=$(( ${(m)#user_plain} + 3 ))
    fi
    max_width=$(( ${COLUMNS:-80} - left_cells - user_rprompt_cells - 2 ))
    (( max_width > 72 )) && max_width=72
    (( max_width < 8 )) && max_width=8
```

Replace `${#value}` at line 153 with `${(m)#value}` and the two truncation slices keep `value[1,N]` (character slices; acceptable because the width check now counts cells and the slice can only shorten). Replace line 180-182 with:

```zsh
        if [[ -n "$status_row" ]] && (( ${(m)#status_row} + ${(m)#value} + 3 > max_width )); then
          break
        fi
```

Change the fallback item list at line 90 and `default_status_line_items` in `src/config.rs:713-722` to the same order, mode first: `mode,model,scope,session_tokens,session_cost,requests` (the mode is the safety-relevant item and must be the last to go).

- [ ] **Step 4: Run the suite**

Run: `cargo build --release && python3 tests/statusline_pty.py target/release/aishe && cargo test --lib config`
Expected: PASS including the narrow case.

- [ ] **Step 5: Commit**

```bash
git add src/integration/assets/pty_prompt.zsh src/config.rs tests/statusline_pty.py
git commit -m "fix: budget the statusline against the real prompt width in cells

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 12: Setup reports the truth, "No" goes back one section, and the API-key path is reachable

**Why:** `Report::verified()` (`src/capabilities.rs:99-104`) requires the paid live checks, so declining them yields "warnings remain" (`setup.rs:2573-2580`), "saved with warnings" (`setup.rs:1533-1542`), and "Live provider: not verified" in the tour (`tour.rs:128-140`). Answering `n` at Apply jumps to `Step::Service` (`setup.rs:1482-1489`) and re-asks eleven prompts. With an existing OAuth login, picking the API-key provider row auto-advances past the credential menu (`setup.rs:942-955`) because `apply_service` never resets `connection.auth`.

**Files:**
- Modify: `src/capabilities.rs:98-110`, `src/setup.rs:942-955, 965-1000, 1482-1489, 1533-1542, 2573-2580`, `src/tour.rs:128-140`
- Test: `src/capabilities.rs` tests, `tests/setup_pty.py` (existing decline path expectations)

**Interfaces:**
- Produces: `Report::locally_verified() -> bool`, `Report::live_skipped() -> bool`, `Report::verdict_label() -> &'static str`.

- [ ] **Step 1: Write the failing unit test**

In `src/capabilities.rs` tests (use the `Check { state, detail, error_kind: None }` literal; `Report` has one `Check` per field):

```rust
    fn check(state: State) -> Check {
        Check { state, detail: String::new(), error_kind: None }
    }

    #[test]
    fn verdict_label_distinguishes_skipped_live_checks_from_warnings() {
        let mut report = Report {
            credential: check(State::Pass),
            reachability: check(State::Pass),
            model_list: check(State::Pass),
            model_available: check(State::Pass),
            text: check(State::Skipped),
            structured: check(State::Skipped),
            tools: check(State::Skipped),
            streaming: check(State::Skipped),
        };
        assert_eq!(report.verdict_label(), "local checks passed · live checks not run (aishe setup --verify --live)");
        report.text = check(State::Pass);
        report.structured = check(State::Pass);
        report.tools = check(State::Pass);
        report.streaming = check(State::Pass);
        assert_eq!(report.verdict_label(), "verified");
        report.credential = check(State::Fail);
        assert_eq!(report.verdict_label(), "warnings remain; run `aishe setup --verify --live`");
    }
```

If `Report` has fields beyond the eight above, fill them the same way; the test must construct a literal so the verdict logic is exercised without a network.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib verdict_label_distinguishes`
Expected: compile error.

- [ ] **Step 3: Implement the verdict**

In `impl Report`:

```rust
    pub fn locally_verified(&self) -> bool {
        self.credential.state == State::Pass
            && self.reachability.state != State::Fail
            && self.model_available.state == State::Pass
    }

    pub fn live_skipped(&self) -> bool {
        [&self.text, &self.structured, &self.tools, &self.streaming]
            .iter()
            .all(|check| check.state == State::Skipped)
    }

    pub fn verdict_label(&self) -> &'static str {
        if self.verified() {
            "verified"
        } else if self.locally_verified() && self.live_skipped() {
            "local checks passed · live checks not run (aishe setup --verify --live)"
        } else {
            "warnings remain; run `aishe setup --verify --live`"
        }
    }
```

Use it: `setup.rs:1533-1542` → `println!("  provider: {}", report.verdict_label());`; `setup.rs:2573-2580` → `println!("    validation: {}", report.verdict_label());`; `tour.rs:128-140` → compute `let state = report.map(|r| if r.verified() { "previously verified and ready" } else if r.locally_verified() && r.live_skipped() { "ready; live checks not run yet (aishe setup --verify --live)" } else { "not verified; lesson stays offline (run `aishe doctor --live`)" })`.

- [ ] **Step 4: Replace the Apply confirm with a section menu**

`setup.rs:1482-1489`:

```rust
                let choice = promptui::menu(
                    "Review and apply",
                    &[
                        "Apply this configuration".to_string(),
                        "Change account or model".to_string(),
                        "Change behavior and scope".to_string(),
                        "Change interface".to_string(),
                        "Pause and resume later".to_string(),
                    ],
                    0,
                    false,
                    "Apply saves config and credentials. The change rows jump back to that section; setup then continues forward to this review.",
                )?;
                match choice {
                    promptui::MenuResult::Selected(0) => {}
                    promptui::MenuResult::Selected(1) => {
                        draft.step = Step::Service;
                        save_draft(&draft)?;
                        continue;
                    }
                    promptui::MenuResult::Selected(2) => {
                        draft.step = Step::Profile;
                        save_draft(&draft)?;
                        continue;
                    }
                    promptui::MenuResult::Selected(3) => {
                        draft.step = Step::Status;
                        save_draft(&draft)?;
                        continue;
                    }
                    _ => return cancel(draft),
                }
```

Update `tests/setup_pty.py` where it answers `y` to "Apply this configuration" so it presses Enter on the menu instead (the first row is pre-selected, so `\r` still works; check the expectation string).

- [ ] **Step 5: Make the API-key path reachable next to an OAuth login**

`setup.rs:942-955`: wrap the auto-advance in `if draft.prefer_oauth { … }`. Then, in the credential menu construction (lines 965-1000), when `existing` has no secret and `crate::oauth::active_provider(&draft.config)?` is `Some(oauth)`, push `format!("Keep the existing {oauth} OAuth login")` as the first choice, remember its index as `keep_oauth_index`, and when it is selected do exactly what the removed branch did: `pending_credential = None; advance(&mut draft)?; continue;`. Change the row `"Enter and save an API key locally (recommended)"` to `"Enter and save an API key locally"` and append ` (recommended)` only to whichever row is `default_credential`.

- [ ] **Step 6: Run the tests**

Run: `cargo test --lib capabilities && cargo test --lib setup && cargo build --release && python3 tests/setup_pty.py target/release/aishe`
Expected: PASS; the PTY suite's decline path now prints `local checks passed · live checks not run`.

- [ ] **Step 7: Commit**

```bash
git add src/capabilities.rs src/setup.rs src/tour.rs tests/setup_pty.py
git commit -m "fix: report skipped live checks honestly and keep setup navigation local

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 13: Shift-Tab delegates to completion on a non-empty line; failures are recorded lazily

**Why:** The hook binds `^[[Z` unconditionally (`zsh_hook.zsh:610-632`); oh-my-zsh binds Shift-Tab to `reverse-menu-complete`, and `docs/shell-integration.md:123-124` claims a guard that does not exist. `_aishe_capture_exit` (`zsh_hook.zsh:203-219`) spawns `aishe --record-failure` synchronously after every non-zero exit, before every prompt, even though the capsule is only read when the user presses Ctrl-X Ctrl-F or asks `?`.

**Files:**
- Modify: `src/integration/assets/zsh_hook.zsh:21-40 (_aishe_handle_nl), 203-219, 247-270 (aishe-fix-command), 568-591, 610-632`, `docs/shell-integration.md:123-124`
- Test: `tests/keys_pty.py` (new)

- [ ] **Step 1: Write the failing PTY test**

Create `tests/keys_pty.py` (helpers from `tests/in_shell_menus_pty.py`; `.zshrc` = `"unset HISTFILE\nreverse-menu-complete-marker() { BUFFER+=\"<RMC>\" }\nzle -N reverse-menu-complete-marker\nbindkey '^[[Z' reverse-menu-complete-marker\n"`, and set `env["AISHE_FAKE_LLM"]` to `'{"type":"answer","text":"FAKE_DIAGNOSIS"}'`):

```python
def main():
    shell = Pty(environment())
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("echo partial")
        shell.send("\x1b[Z")
        shell.drain(0.5)
        if "<RMC>" not in shell.plain():
            raise AssertionError("Shift-Tab with text on the line did not delegate:\n" + shell.plain()[-800:])
        shell.send("\x15")  # kill line
        shell.send("\x1b[Z")  # empty line: cycle mode (auto -> yolo consent)
        if not shell.expect("Type yolo to continue"):
            raise AssertionError("Shift-Tab on an empty line did not cycle the mode")
        shell.send("\x1b")
        shell.drain(0.5)
        shell.send("false\r")
        shell.drain(0.5)
        shell.send("print -r -- PENDING=$_AISHE_FAILURE_PENDING\r")
        if not shell.expect("PENDING=1"):
            raise AssertionError("failure was not marked pending:\n" + shell.plain()[-800:])
        shell.send("? why did that fail\r")
        if not shell.expect("FAKE_DIAGNOSIS", timeout=30):
            raise AssertionError("diagnosis did not run")
        shell.send("aishe last show\r")
        if not shell.expect("false"):
            raise AssertionError("failure was not recorded before the diagnosis")
        print("keys: ok")
    finally:
        shell.close()
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 tests/keys_pty.py target/release/aishe`
Expected: FAIL at the delegation check.

- [ ] **Step 3: Capture the prior Shift-Tab widget and delegate**

In the binding block (`zsh_hook.zsh:610-632`), replace `zle -N aishe-cycle-mode` / `bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode` with:

```zsh
  typeset -ga _aishe_mode_binding
  _aishe_mode_binding=("${(@z)$(bindkey "${AISHE_MODE_KEY:-^[[Z}")}")
  if [[ "${_aishe_mode_binding[-1]:-}" != aishe-cycle-mode && "${_aishe_mode_binding[-1]:-}" != undefined-key ]]; then
    typeset -g _AISHE_ORIG_MODE_WIDGET="${_aishe_mode_binding[-1]}"
  fi
  unset _aishe_mode_binding
  zle -N aishe-cycle-mode
  bindkey "${AISHE_MODE_KEY:-^[[Z}" aishe-cycle-mode
```

At the top of `aishe-cycle-mode` (after `emulate -L zsh`):

```zsh
  # With text on the line, Shift-Tab keeps whatever the user's plugins bound
  # (oh-my-zsh: reverse-menu-complete). Mode cycling is an empty-line action.
  if [[ -n "$BUFFER" && -n "${_AISHE_ORIG_MODE_WIDGET:-}" ]]; then
    zle "$_AISHE_ORIG_MODE_WIDGET"
    return
  fi
```

- [ ] **Step 4: Record failures only when they are about to be used**

Replace the `command aishe --record-failure …` line and its `typeset -g _AISHE_FAILURE_ACTIVE=1` in `_aishe_capture_exit` with:

```zsh
    typeset -g _AISHE_FAILURE_PENDING=1
    typeset -g _AISHE_FAILURE_DURATION_MS="$elapsed"
```

and keep the `elif` branch that clears (`command aishe last clear`) only when `_AISHE_FAILURE_ACTIVE` is 1. Add:

```zsh
# Persist the last failure capsule on demand. Called before anything reads it.
_aishe_record_failure_now() {
  [[ "${_AISHE_FAILURE_PENDING:-0}" == 1 ]] || return 0
  AISHE_LAST_DURATION_MS="${_AISHE_FAILURE_DURATION_MS:-}" command aishe --record-failure "$AISHE_LAST_CMD" >/dev/null 2>&1
  typeset -g _AISHE_FAILURE_PENDING=""
  typeset -g _AISHE_FAILURE_ACTIVE=1
}
```

Call `_aishe_record_failure_now` as the first statement of `aishe-fix-command` (line 247) and at the start of `_aishe_handle_nl` (line 21) when the line begins with `?` or is the bare `?`. Apply the same lazy pattern to `bash_hook.bash` (`grep -n record-failure src/integration/assets/bash_hook.bash`).

- [ ] **Step 5: Fix the docs claim**

`docs/shell-integration.md:123-124`: "Shift-Tab cycles the mode on an empty line. With text on the line it runs whatever your plugins bound to Shift-Tab (oh-my-zsh: reverse menu completion)."

- [ ] **Step 6: Run the tests**

Run: `cargo build --release && python3 tests/keys_pty.py target/release/aishe && python3 tests/bash_hook.py target/release/aishe --bash bash`
Expected: `keys: ok`; bash suite green.

- [ ] **Step 7: Commit**

```bash
git add src/integration/assets docs/shell-integration.md tests/keys_pty.py
git commit -m "fix: delegate Shift-Tab on a non-empty line and record failures lazily

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 14: Orphaned runtime servers are visible and reapable

**Why:** The review machine had 21 `opencode serve --hostname=127.0.0.1 --port=…` processes with parent 1, 30-33 days old, plus ten stale `aishe` processes. `aishe backend status` shows only the current supervisor and `backend gc --dry-run` says "runtime cache is clean". The old servers are bare `opencode` from PATH (soak tests from early August, not the pinned supervisor), but nothing detects them and each holds a loopback port and memory forever.

**Files:**
- Modify: `src/cli/backend.rs:154-168` (gc), `src/cli/args.rs` (`backend gc --kill-orphans`), `src/diagnostics.rs` (one `backend.orphans` check), `tests/opencode_soak.py`, `tests/opencode_concurrency.py` (cleanup)
- Test: `src/cli/backend.rs` tests

**Interfaces:**
- Produces: `pub struct OrphanServer { pub pid: u32, pub elapsed: String, pub command: String }`; `pub fn orphaned_runtime_servers() -> Vec<OrphanServer>`; `pub fn parse_orphans(ps_output: &str) -> Vec<OrphanServer>`.

- [ ] **Step 1: Write the failing unit test**

```rust
    #[test]
    fn parse_orphans_keeps_only_parentless_loopback_servers() {
        let ps = "\
  101     1 31-09:44:59 opencode serve --hostname=127.0.0.1 --port=60833
  202   150 00:04:32 /Users/x/runtime/opencode/1.18.27/opencode serve --hostname=127.0.0.1 --port=61112
  303     1 00:00:10 vim notes.md
  404     1 02:11:00 opencode serve --hostname=0.0.0.0 --port=9000
";
        let orphans = parse_orphans(ps);
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].pid, 101);
        assert_eq!(orphans[0].elapsed, "31-09:44:59");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib parse_orphans_keeps_only`
Expected: compile error.

- [ ] **Step 3: Implement detection**

In `src/cli/backend.rs`:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanServer {
    pub pid: u32,
    pub elapsed: String,
    pub command: String,
}

/// Parse `ps -axo pid=,ppid=,etime=,command=` and keep loopback OpenCode
/// servers whose parent is init: a supervisor never leaves one behind, so
/// these come from crashed shells or test harnesses.
pub fn parse_orphans(ps_output: &str) -> Vec<OrphanServer> {
    ps_output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse::<u32>().ok()?;
            let ppid = parts.next()?.parse::<u32>().ok()?;
            let elapsed = parts.next()?.to_string();
            let command = parts.collect::<Vec<_>>().join(" ");
            (ppid == 1
                && command.contains("opencode serve")
                && command.contains("--hostname=127.0.0.1"))
            .then_some(OrphanServer { pid, elapsed, command })
        })
        .collect()
}

pub fn orphaned_runtime_servers() -> Vec<OrphanServer> {
    std::process::Command::new("ps")
        .args(["-axo", "pid=,ppid=,etime=,command="])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| parse_orphans(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}
```

In the `Action::Gc { dry_run, kill_orphans }` arm, after the cache pass:

```rust
            let orphans = orphaned_runtime_servers();
            for orphan in &orphans {
                if *kill_orphans && !*dry_run {
                    // SAFETY: sending SIGTERM to a pid we just enumerated.
                    unsafe { libc::kill(orphan.pid as i32, libc::SIGTERM) };
                    println!("stopped orphaned runtime server pid {} (up {})", orphan.pid, orphan.elapsed);
                } else {
                    println!("orphaned runtime server pid {} (up {}): {}", orphan.pid, orphan.elapsed, orphan.command);
                }
            }
            if !orphans.is_empty() && !*kill_orphans {
                println!("Next: aishe backend gc --kill-orphans");
            }
            if removed.is_empty() && orphans.is_empty() {
                println!("runtime cache is clean");
            }
```

Add `/// Stop orphaned loopback runtime servers that no supervisor owns` `#[arg(long)] kill_orphans: bool` to `BackendCmd::Gc` in `src/cli/args.rs`. In `src/diagnostics.rs`, next to the `backend.supervisor` check, push a `Check::new("backend.orphans", Status::Warn|Pass, …)` that reports the count and points at `aishe backend gc --kill-orphans`.

- [ ] **Step 4: Make the soak and concurrency tests clean up after themselves**

In `tests/opencode_soak.py` and `tests/opencode_concurrency.py`, find every `subprocess.Popen` that starts `opencode` or `aishe` (grep `Popen(`) and wrap the run in `try: … finally:` that calls `os.killpg(os.getpgid(proc.pid), signal.SIGTERM)` for each still-running process, using `preexec_fn=os.setsid` so the whole group dies. Run each once (`python3 tests/opencode_soak.py target/release/aishe --help` for usage) and confirm `pgrep -f "opencode serve"` shows no new survivors afterward.

- [ ] **Step 5: Run the tests**

Run: `cargo test --lib parse_orphans_keeps_only && cargo test --lib diagnostics && target/release/aishe backend gc --dry-run`
Expected: PASS; the dry run lists the stale servers on this machine with `Next: aishe backend gc --kill-orphans`.

- [ ] **Step 6: Commit**

```bash
git add src/cli/backend.rs src/cli/args.rs src/diagnostics.rs tests/opencode_soak.py tests/opencode_concurrency.py
git commit -m "feat: detect and reap orphaned runtime servers from backend gc and doctor

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Phase 1 exit gate

- [ ] `cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --all-targets --locked`
- [ ] `for t in in_shell_menus yolo_consent palette mode_handoff bare_words theme_prompt keys statusline model_picker setup; do python3 tests/${t}_pty.py target/release/aishe || exit 1; done`
- [ ] `sh tests/installer_bindir.sh && sh -n install.sh && shellcheck install.sh`
- [ ] Manual: open `aishe`, type `/settings`, Esc; Shift-Tab, `n`; `/`, Tab, Esc; `reset`; resize the window to 80 columns. Every one of those matches the "What I saw" section of the review, fixed.

---

# Phase 2 — One registry, one vocabulary

Phase 2 changes the *shape* of the command surface. Every task keeps the conformance tests green (`cargo test --test command_surface`, `cargo test --lib product_help`, `cargo test --lib integration::tests`) and regenerates `docs/commands.md` when a row changes.

### Task 15: Declare `effect` on `CommandSpec` with six labels

**Why:** `effect_label` (`src/product_help.rs:252-266`) derives nine labels from two orthogonal enums and gets several wrong: `/ask [no effect]` (paid model call), `/test [read-only]` (`--live` is paid), `/palette [read-only]` (a launcher), `/undo [may change state]` (edits files), `/output [durable setting]` vs `/scope [durable; refreshes this shell]` although both hand off, `/demo [session]` and `/sessions [session]` meaning different things. The palette prints a third vocabulary in raw enum names (`mixed`, `read_only`). One declared field replaces the derivation.

**Files:**
- Modify: `src/command_surface.rs` (enum + every `CommandSpec` literal), `src/product_help.rs:252-266`, `src/palette.rs:143-152`, `docs/commands.md` (regenerated), `docs/commands.md` prose above the table ("State/effect" legend)
- Test: `src/product_help.rs` tests, `tests/command_surface.rs`

**Interfaces:**
- Produces:

```rust
/// What running the command changes. Rendered verbatim in help, palette, and docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// Prints and exits.
    ReadOnly,
    /// Changes only this shell session.
    ThisShell,
    /// Changes this shell; `--default` (or the follow-up prompt) also saves config.
    ThisShellOrDefault,
    /// Writes config.toml.
    SavesConfig,
    /// Changes the active conversation or its sessions.
    Conversation,
    /// Runs an agent, executes commands, or edits files.
    RunsAgent,
}

impl Effect {
    pub const fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::ThisShell => "this shell",
            Self::ThisShellOrDefault => "this shell · --default saves",
            Self::SavesConfig => "saves config",
            Self::Conversation => "conversation",
            Self::RunsAgent => "runs agent / edits files",
        }
    }
}
```

- [ ] **Step 1: Write the failing test**

In `src/product_help.rs` tests:

```rust
    #[test]
    fn effect_labels_come_from_the_declared_effect() {
        let labels: std::collections::BTreeSet<&str> = COMMANDS
            .iter()
            .filter(|spec| spec.is_active())
            .map(|spec| spec.effect.label())
            .collect();
        for label in labels {
            assert!(
                ["read-only", "this shell", "this shell · --default saves", "saves config", "conversation", "runs agent / edits files"].contains(&label),
                "unexpected effect label {label}"
            );
        }
        assert_eq!(by_id("ask").unwrap().effect, Effect::RunsAgent);
        assert_eq!(by_id("undo").unwrap().effect, Effect::RunsAgent);
        assert_eq!(by_id("scope").unwrap().effect, Effect::SavesConfig);
        assert_eq!(by_id("model").unwrap().effect, Effect::ThisShellOrDefault);
        assert_eq!(by_id("status").unwrap().effect, Effect::ReadOnly);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib effect_labels_come_from_the_declared_effect`
Expected: compile error, no field `effect`.

- [ ] **Step 3: Add the field and assign it to every spec**

Add `pub effect: Effect,` to `CommandSpec` and set it on every literal using this table (ids not listed are `ReadOnly`):

| Effect | ids |
| --- | --- |
| `ThisShell` | `details`, `mode` |
| `ThisShellOrDefault` | `connection`, `model`, `reasoning` |
| `SavesConfig` | `scope`, `network`, `output`, `settings`, `role`, `trust`, `untrust`, `context` (its `--include/--exclude` save) |
| `Conversation` | `reset`, `sessions`, `resume`, `fork`, `plan`, `replan`, `demo` |
| `RunsAgent` | `agent`, `inbox`, `task`, `ask`, `last`, `undo`, `index`, `test`, `palette` |

Replace the body of `effect_label` with `spec.effect.label()`. In `src/palette.rs`, make `Entry.effect` the same label (`spec.effect.label().into()`) and delete the private `effect()` mapping. Leave `side_effects` and `shell_local` in place for the other consumers (approval, automation) but stop using them for presentation.

- [ ] **Step 4: Regenerate the docs table and update the legend**

Run `cargo test --lib commands_markdown_matches_the_generated_registry_block`; paste the expected block from the assertion output into `docs/commands.md` between the `BEGIN/END GENERATED COMMAND SURFACE` markers. Replace the paragraph above the table that explains "This shell by default" with the six-label legend (one line each, from the enum docs).

- [ ] **Step 5: Run the conformance tests**

Run: `cargo test --lib product_help && cargo test --lib palette && cargo test --test command_surface`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/command_surface.rs src/product_help.rs src/palette.rs docs/commands.md
git commit -m "refactor: declare each command's effect with one six-label vocabulary

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 16: Hide aliases and remove tombstones so the visible surface is twenty commands

**Why:** 39 visible slash names plus 9 tombstones contain seven duplicate pairs (`/provider`/`/connection`, `/commands`/`/help`, `/usage`/`/status`, `/resume`/`/fork`/`/sessions`, `/plan`/`/replan`, `/task`/`/agent`, `/demo`/`aishe tour`). The tombstones (`/editor /frontend /stream /structured /theme /rehash /ghost /sandbox /cache`, `src/command_surface.rs:775-918`) date from 0.6.5 and now cost a help topic, a docs table, hook case arms, and reserved names. `aishe hints` is a QA affordance that `product_help.rs:268-276` admits into every slash list.

**Files:**
- Modify: `src/command_surface.rs` (`hidden: bool` on `CommandSpec`; delete tombstone specs; `resume`/`fork` merge into `sessions`; `replan` into `plan`), `src/product_help.rs` (`append_terminal_commands` filters `hidden`; delete `render_migration` and the `migration` topic; `Topics:` line), `src/palette.rs` (filter `hidden`), `src/integration/registry.rs` (aliases still dispatch), `src/cli/runtime.rs:1466-1475` (tombstone arm removed), `docs/commands.md:150-158, 215-227`, `tests/command_surface.rs:95-115` (`every_tombstone_reports_its_local_migration_guidance` deleted), `src/product_help.rs` tests (`migration_help_contains_every_tombstone_and_exact_guidance` deleted)
- Test: `src/product_help.rs` tests

**Interfaces:**
- Produces: `CommandSpec.hidden: bool`. Hidden commands dispatch and tab-complete but never appear in help rows, the palette, or the docs table. The `Lifecycle::Tombstone` variant is deleted along with `ShellHookAction::CompatibilityDiagnostic`.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn visible_slash_surface_is_exactly_the_target_list() {
        let visible: Vec<&str> = COMMANDS
            .iter()
            .filter(|spec| spec.is_active() && !spec.hidden && !spec.slash_aliases.is_empty())
            .map(|spec| spec.slash_aliases[0])
            .collect();
        assert_eq!(
            visible,
            [
                "help", "status", "log", "context",
                "connection", "model", "reasoning", "mode", "details",
                "scope", "network", "settings",
                "reset", "sessions", "plan",
                "agent", "inbox", "ask", "last", "undo",
            ]
        );
        assert!(by_slash_alias("provider").is_some_and(|spec| spec.hidden));
        assert!(by_slash_alias("usage").is_some_and(|spec| spec.id == "status"));
        assert!(by_slash_alias("theme").is_none());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib visible_slash_surface_is_exactly_the_target_list`
Expected: compile error / FAIL.

- [ ] **Step 3: Restructure the registry**

- Add `pub hidden: bool` to `CommandSpec`; set `hidden: true` on `commands`, `auth`, `config`, `output`, `palette`, `skills`, `mcp`, `capabilities`, `test`, `demo`, `trust`, `untrust`, `index`, `role`, `task`, `hints`, `resume`, `fork`, `replan`; `false` everywhere else.
- `status` gets `slash_aliases: &["status", "usage"]` (delete the separate `usage` spec; `aishe usage` stays a CLI command outside the registry). `hook_action` no longer needs `"usage" => OneShot`; delete that arm and the `OneShot` variant if unused.
- `connection` gets `slash_aliases: &["connection", "provider"]` (it already does; keep it).
- `help` keeps `&["help", "commands"]`.
- `sessions` gets `slash_aliases: &["sessions", "resume", "fork"]`; delete the `resume` and `fork` specs. `/resume ID` and `/fork ID` keep working because the hook passes arguments through to `aishe sessions` — add `resume` and `fork` handling to `aishe sessions [ACTION] [ID]` if `aishe sessions` does not already accept them (`grep -n "resume\|fork" src/cli/session.rs`); the CLI commands `aishe resume` and `aishe session fork` stay.
- `plan` gets `slash_aliases: &["plan", "replan"]` and `arguments: PassThrough("[ID] [--replace]")`; delete the `replan` spec; `aishe plan --replace` becomes the implementation of `aishe replan` (`grep -n "replan" src/main.rs src/cli/args.rs`), and `Cmd::Replan` becomes `#[command(hide = true)]`.
- Delete every `Lifecycle::Tombstone` spec and the `Tombstone` variant; delete `ShellHookAction::CompatibilityDiagnostic` and its render arm; delete the tombstone arm in `reject_unavailable_one_shot_slash` (`runtime.rs:1466-1475`); delete `render_migration` and the `"migration" | "removed" | "legacy"` topic in `product_help.rs`; delete `docs/commands.md:215-227` (removed-commands table) and the `/help migration` line at 150-158; delete the two tombstone tests. `is_reserved_slash` keeps reserving the visible and hidden aliases only.
- `append_terminal_commands` and `palette::entries` filter `!spec.hidden`. The overview `Topics:` line becomes `Topics: /help accounts · models · agent · session · config · routing`.

- [ ] **Step 4: Regenerate the docs table and run every registry test**

Run: `cargo test --lib product_help && cargo test --lib palette && cargo test --lib integration::tests && cargo test --test command_surface && cargo test --test dispatcher`
Expected: PASS after pasting the regenerated block into `docs/commands.md`.

- [ ] **Step 5: Commit**

```bash
git add src/command_surface.rs src/product_help.rs src/palette.rs src/integration/registry.rs src/cli/runtime.rs src/cli/session.rs src/cli/args.rs src/main.rs docs/commands.md tests/command_surface.rs
git commit -m "refactor: hide duplicate slash aliases and drop the 0.6.5 tombstones

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 17: Hide duplicate top-level CLI commands and group `aishe --help`

**Why:** `aishe --help` lists 60 commands flat. `tour` and `demo` route to the same function (`src/main.rs:180-196`) with different help text; `provider` is documented as legacy (`docs/commands.md:37`) and hosts `provider test` which duplicates `test`; `models` duplicates `model`; top-level `plan`/`replan` duplicate `task plan`/`task replan`; `__backend-supervisor` leaks into typo tips (`aishe backnd`).

**Files:**
- Modify: `src/cli/args.rs` (`Cmd` enum attributes), `src/main.rs:718-743` (`provider test` removal), `src/cli/args.rs:203-205` (supervisor as hidden flag)
- Test: `tests/cli.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn root_help_is_grouped_and_hides_duplicates() {
    let home = temp_home("root-help");
    let output = aishe(&home).arg("--help").output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for heading in ["Shell:", "Accounts and models:", "Session:", "Agent work:", "Maintenance:"] {
        assert!(stdout.contains(heading), "missing {heading}\n{stdout}");
    }
    for hidden in ["\n  demo ", "\n  provider ", "\n  models ", "\n  replan ", "__backend-supervisor"] {
        assert!(!stdout.contains(hidden), "{hidden} still visible\n{stdout}");
    }
    let tip = aishe(&home).arg("backnd").output().unwrap();
    assert!(!String::from_utf8_lossy(&tip.stderr).contains("__backend-supervisor"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli root_help_is_grouped_and_hides_duplicates`
Expected: FAIL.

- [ ] **Step 3: Apply the clap attributes**

- `Demo`: `#[command(hide = true)]` and about "Alias of `tour`". `Provider`: `#[command(hide = true)]`; delete the `test` action, its `--live/--json` flags, and the `invalid_provider_flags` branch in `main.rs:718-743` (remove the code from the error-contract list and `docs/troubleshooting.md`). `Models`: `#[command(hide = true)]`, about "Alias of `model --list`" (add `--list` to `Model` if it does not exist: prints the same list). `Plan`/`Replan` top-level: `#[command(hide = true)]`.
- Group with `#[command(help_heading = "…")]` on each variant: Shell (`zsh`, `init`, `completions`, `man`, `route`, `suggest`, `ask`, `last`, `undo`, `dry-run`, `history`, `index`), Accounts and models (`setup`, `settings`, `auth`, `connection`, `model`, `reasoning`, `role`, `price`, `capabilities`), Session (`status`, `mode`, `scope`, `network`, `output`, `reset`, `sessions`, `session`, `resume`, `context`, `log`, `usage`), Agent work (`agent`, `inbox`, `task`, `plan`, `runbook`, `mcp`, `skills`), Maintenance (`doctor`, `backend`, `update`, `uninstall`, `trust`, `untrust`, `profile`, `readiness`, `config`, `commands`, `palette`, `hints`, `test`, `tour`).
- `__backend-supervisor`: convert to a hidden long flag `--backend-supervisor` on `Args` (same pattern as `accept_yolo` at `args.rs:41-75`) so it can never fuzzy-match a subcommand; update the one spawn site (`grep -rn "__backend-supervisor" src`).

- [ ] **Step 4: Run the tests**

Run: `cargo test --test cli && cargo test --test command_surface canonical_cli_commands_are_in_root_help_and_both_completion_scripts`
Expected: PASS (adjust that conformance test if it asserted hidden commands appear in root help; hidden aliases are still in the completion scripts).

- [ ] **Step 5: Commit**

```bash
git add src/cli/args.rs src/main.rs src/cli/error_contract.rs docs/troubleshooting.md docs/commands.md tests/cli.rs tests/command_surface.rs
git commit -m "refactor: group root help and hide duplicate top-level commands

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 18: One `HELP_TOPICS`, one `CONTROLS_HINT`, one help overview that fits on a screen

**Why:** Help topics are listed five ways (`product_help.rs:55`, `args.rs:22` after_help, `args.rs:444`, `docs/commands.md:150-158`, `README.md:270`), and after_help points at `aishe help`, which is the same clap screen, with literal `*active*` asterisks. The launch hint (`wrapper.zshrc:34`), `aishe status` controls (`status.rs:176`), and `/help` Keys line (`product_help.rs:54`) list the same keys three ways, and the Keys line is printed for bash where force-NL is Ctrl-G. `/help` prints 57 lines so the task header scrolls off; the table pads to 30 columns and two rows overflow; `aishe commands bogus` exits 0; `/help | less` runs `aishe commands "| less"`.

**Files:**
- Modify: `src/product_help.rs` (constants, overview, `all` topic, column width, exit code), `src/cli/args.rs:19-22, 444`, `src/integration/assets/wrapper.zshrc:34`, `src/cli/status.rs:176`, `src/integration/assets/zsh_hook.zsh:525-529`, `src/integration/tests.rs:281`, `docs/commands.md:150-158`, `README.md:270`
- Test: `src/product_help.rs` tests, `tests/cli.rs`

**Interfaces:**
- Produces: `pub const HELP_TOPICS: &[&str] = &["accounts", "models", "agent", "session", "config", "routing"];` `pub const CONTROLS_HINT: &str = "/help · /connection · /model · Shift-Tab mode · Ctrl-O details · ? asks the agent";` `pub fn help_topics_line() -> String` (`"accounts · models · agent · session · config · routing"`).

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn overview_fits_one_screen_and_points_at_all() {
        let overview = render_help(None);
        assert!(overview.lines().count() <= 24, "{}", overview.lines().count());
        assert!(overview.contains("/help all"));
        assert!(overview.contains(CONTROLS_HINT));
        assert!(!overview.contains("aishe hints"));
        let all = render_help(Some("all"));
        assert!(all.contains("/agent"));
        assert!(all.lines().all(|line| !line.starts_with("  aishe ")), "CLI-only rows leaked");
    }

    #[test]
    fn every_surface_lists_the_same_topics() {
        let expected = help_topics_line();
        assert!(render_help(None).contains(&expected));
        let root = crate::cli::args::Args::command().render_long_help().to_string();
        assert!(root.contains(&expected), "{root}");
        assert!(!root.contains("*active*"));
        assert!(std::fs::read_to_string("docs/commands.md").unwrap().contains(&expected));
        assert!(std::fs::read_to_string("README.md").unwrap().contains(&expected));
    }
```

`tests/cli.rs`: `aishe commands bogus` exits 2 with `unknown help topic`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib overview_fits_one_screen && cargo test --lib every_surface_lists_the_same_topics`
Expected: FAIL.

- [ ] **Step 3: Implement**

- Define `HELP_TOPICS`, `CONTROLS_HINT`, `help_topics_line()` in `src/product_help.rs`. `render_help`: unknown topic prints `unknown help topic '{t}'` plus `Next: /help {topics}` and returns exit 2 (thread a `u8` back to `print_help_command` in `runtime.rs` and to `main.rs` for `aishe commands`). Add `Some("all")` which prints the full table; `render_overview` drops the table and ends with `Every command: /help all · CLI: aishe --help`. Replace the `Keys:` line with `Keys: {CONTROLS_HINT}`.
- `append_terminal_commands`: filter `!spec.hidden`, print the primary alias only, pad to 26, group rows under their `help_topic` in the `all` topic.
- `args.rs:19-22` after_help: `"AIShe is AI Shell; the CLI package is aishe.\nIn the shell: /help · /help {topics}. Add an account: aishe setup."` rendered from the constant (`after_help` accepts a `String` via `#[command(after_help = …)]` with a `const`-built string, or set it at runtime with `Args::command().after_help(...)` where `main` builds the command). `args.rs:444` `commands` about: "List slash commands or show help for one topic: {topics}".
- `wrapper.zshrc:34` and `status.rs:176` print `CONTROLS_HINT` (the wrapper is generated from Rust: `grep -n "aishe: /help" src/integration/templates.rs` and substitute the constant). Update `src/integration/tests.rs:281`.
- `zsh_hook.zsh:525-529`: skip the slash intercept when `$BUFFER` contains an unquoted `|`, `>`, or `;` (`[[ "$BUFFER" == *[\|\>\;]* ]]` is enough; quoted pipes inside slash arguments are not a supported case).
- `docs/commands.md:150-158` and `README.md:270`: the same topic list, generated text pasted verbatim.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib product_help && cargo test --lib integration::tests && cargo test --test cli`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/product_help.rs src/cli/args.rs src/cli/runtime.rs src/main.rs src/integration src/cli/status.rs docs/commands.md README.md tests/cli.rs
git commit -m "refactor: one help topic list, one controls hint, one-screen help overview

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 19: Every fatal error goes through the contract; one `Next:` label; one listing style

**Why:** Four stderr styles coexist: clap `error:`, contract `aishe: Message. [code]` + `Next:`, ad hoc lowercase `aishe: unknown profile 'bogus'` (`settings.rs:402`), `aishe: no integration for 'fish'` (`main.rs:307`), `aishe: route needs a non-empty line` (`backend.rs:381`), `AIShe · shell override —` (`runtime.rs:1661,1665`), `aishe undo:` in red (`settings.rs:706`), `aishe: models: {:?}` printing a Debug enum (`settings.rs:51-55`). Next-action labels are `Next:`, `next:` (`backend.rs:66,74`; `runtime.rs:1484`), and `reset:` (`hints.rs:38`). Listings use ` — ` in `skills` and ` - ` in `mcp`; `auth list` prints a tab-separated uppercase header while `connection list` prints aligned rows.

**Files:**
- Modify: the sites above plus `src/cli/backend.rs:66,74`, `src/cli/hints.rs:38`, `src/main.rs:588`, `src/cli/runtime.rs:1856`, `src/auth.rs` (`auth list` header)
- Test: `tests/cli.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn ad_hoc_errors_use_the_contract_shape() {
    let home = temp_home("error-shape");
    for (args, code) in [
        (vec!["profile", "bogus"], "cli.unknown_profile"),
        (vec!["init", "fish"], "cli.unsupported_shell"),
        (vec!["route"], "cli.missing_argument"),
    ] {
        let output = aishe(&home).args(&args).output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains(&format!("[{code}]")), "{args:?}: {stderr}");
        assert!(stderr.contains("Next:"), "{args:?}: {stderr}");
        assert!(!stderr.contains("next:"), "{args:?}: {stderr}");
    }
}
```

(`route` with no line: make `LINE` optional in clap so the contract error, not clap's usage error, is emitted, or keep clap's error and drop that row; pick one and keep the test honest.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli ad_hoc_errors_use_the_contract_shape`
Expected: FAIL.

- [ ] **Step 3: Route each site through `emit_classified` or `UserFacing`**

For every site listed in **Why**, replace the `eprintln!` + `return 1` with `emit_classified(ErrorNamespace::Cli, "<name>", "<Sentence-case message.>", "<Next action>", None)` and `return ErrorNamespace::Cli.exit_code()`, or with `return Err(UserFacing::cli(...))` where the function returns `Result`. Names: `unknown_profile`, `unsupported_shell` (`init`), `missing_argument` (`route`), `models_failed` (`settings.rs:51-55`, message built from `error.to_string()`, never `{:?}`). Register each name in the error-contract list and `docs/troubleshooting.md`. Change every `next:` and `reset:` label to `Next:`. Keep `AIShe · shell override` and `AIShe · # agent prefix is deprecated` as in-shell cues but move them to stdout through one `crate::ui::cue(text)` helper that paints with `StyleToken::Muted`. `skills` and `mcp` listings both use `  {name}  —  {desc}` through one `crate::ui::render::listing_row(name, desc, width)`; `auth list` prints `profiles:` then aligned rows like `connection list` (no uppercase header). `aishe undo:` uses `emit_classified(Io, …)` for failures and plain stdout for success.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test cli && cargo test --lib error_contract`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src tests/cli.rs docs/troubleshooting.md
git commit -m "refactor: send every fatal CLI error through the error contract

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 20: Fill the empty help strings and standardize flag vocabulary

**Why:** About 45 clap arguments have no help text (`aishe connection add --help` shows `--provider <PROVIDER>` followed by nothing). `--json` is described seventeen ways; count flags are `-n --limit`, `-n --lines`, and `--tail`; `log --action` filters `kind`; `-c <COMMAND>` collides with the `[COMMAND]` subcommand slot; the same archive is `--runtime-file` in setup and `--from` in backend; the override flags are not global (`aishe status --mode auto` errors); `init <SHELL>` is a free string; `mcp add` validates after parse; `--since bogus` is silently ignored; bare `mcp` runs while every other group prints help.

**Files:**
- Modify: `src/cli/args.rs` throughout, `src/cli/history.rs:341, 493`, `src/main.rs:299-313`
- Test: `tests/cli.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn every_argument_has_help_and_json_is_phrased_once() {
    use clap::CommandFactory;
    fn walk(cmd: &clap::Command, path: &str, missing: &mut Vec<String>, json: &mut std::collections::BTreeSet<String>) {
        for arg in cmd.get_arguments() {
            if arg.is_hide_set() { continue; }
            match arg.get_help() {
                None => missing.push(format!("{path} {}", arg.get_id())),
                Some(help) if help.to_string().trim().is_empty() => missing.push(format!("{path} {}", arg.get_id())),
                Some(help) => {
                    if arg.get_id() == "json" { json.insert(help.to_string()); }
                }
            }
        }
        for sub in cmd.get_subcommands() {
            walk(sub, &format!("{path} {}", sub.get_name()), missing, json);
        }
    }
    let mut missing = Vec::new();
    let mut json = std::collections::BTreeSet::new();
    walk(&aishe::cli::args::Args::command(), "aishe", &mut missing, &mut json);
    assert!(missing.is_empty(), "arguments without help:\n{}", missing.join("\n"));
    assert!(json.len() <= 2, "--json phrased {} ways: {json:?}", json.len());
}
```

(Two phrasings are allowed: "Print JSON instead of text" and "Print JSONL instead of a table" for `log`.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli every_argument_has_help`
Expected: FAIL listing ~45 arguments.

- [ ] **Step 3: Edit `src/cli/args.rs`**

- Write a one-line sentence-case help for every listed argument (`connection add`: `--provider` "Provider preset: anthropic, openai, xai, groq, openrouter, together, ollama, custom"; `--label` "Display label shown in /connection and the statusline"; `--base-url` "Provider base URL (host root, without /v1)"; `--model` "Default model for this connection"; `--transport` "API transport"; `--auth` "How this connection authenticates"; `--profile` "OAuth profile name"; `--credential` "Saved credential profile"; `--key-env` "Environment variable that holds the API key"; `--reasoning` "Default reasoning effort"; `<ID>` "Connection id (letters, digits, dashes)"). Same care for `session`, `task`, `role`, `mcp`, `update`, `last`, `price`, `backend logs/gc`, and every `[VALUE]`/`[ID]`.
- `--json`: "Print JSON instead of text" everywhere; `log --json`: "Print JSONL instead of a table".
- Count flags: `-n, --lines` on `task tail` and `backend logs` (keep `--tail` as a hidden alias); `-n, --limit` on `log`, `history search`, `index`.
- `log --action` → `--kind` with `#[arg(alias = "action", hide_alias = true)]`… clap has `visible_alias`/`alias` for args: use `alias = "action"`.
- `-c`: `value_name = "LINE"`. `--hunk <HUNKS>` → `value_name = "N"`; `--arg <ARGS>` → `value_name = "VALUE"`; `palette --query` and `index --query` both `value_name = "TEXT"`. `backend install/repair --from` gets `visible_alias = "runtime-file"` so both spellings work; setup keeps `--runtime-file`.
- `--mode/--model/--provider/--connection` on `Args`: `#[arg(global = true)]`.
- `init`: `#[arg(value_parser = ["zsh", "bash"])]`; delete the ad hoc message at `main.rs:299-313`.
- `mcp add`: `#[group(required = true, multiple = false)]` over `command`/`url`.
- `log`/`usage --since`: `value_parser = parse_since` so `bogus` is a clap error listing `30m, 2h, 3d, 1w`.
- `mcp`: make the subcommand required and add `list` as the documented way (`aishe mcp` prints help and exits 2 like every other group); unify the empty message to `no MCP servers configured (see docs/mcp.md)`.
- Split the four paragraph abouts (`suggest`, `trust`, `dry-run`, `reset`) with a blank line after the first sentence. Move the second sentences of `auth set`, `auth list`, `connection remove` into `long_about`. Rewrite jargon abouts: `hints` "Show or reset first-run hints", `capabilities` "Show what the active model has been verified to do", `backend` "Manage the bundled OpenCode runtime", `agent` "Run an agent in the foreground or in an isolated background worktree", `last show` "Show the recorded failure without calling a model", `zsh` "Launch your real interactive zsh under AIShe".

- [ ] **Step 4: Run the tests**

Run: `cargo test --test cli && cargo test --test command_surface && python3 tests/shell_contract.py target/debug/aishe`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli/args.rs src/cli/history.rs src/main.rs tests/cli.rs
git commit -m "docs: fill every clap help string and standardize flag names

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 21: `aishe status` and `aishe doctor` say each thing once

**Why:** `status` prints `connection: Codex - OAuth · codex1 (openai) · provider: openai · endpoint: api.openai.com · default` (provider twice, wraps at 100 columns). `doctor` prints `✓ version: version: aishe 0.7.0`, eight `· backend.*` rows that all say "run `aishe doctor --live`", a bubblewrap warning on macOS, and `terminal ui: none theme · none color`. Titles are lowercase `aishe status` while `AIShe self-test` is not.

**Files:**
- Modify: `src/cli/status.rs:103-110, 176`, `src/diagnostics.rs:118-126, 1162-1190`, the `ui.terminal_policy` check, the eight `backend.server.*`/`backend.config.*`/`backend.plugin.*`/`backend.tools.*`/`backend.tool_bridge`/`backend.events` checks
- Test: `src/diagnostics.rs` tests, `tests/cli.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn status_and_doctor_do_not_repeat_themselves() {
    let home = temp_home("status-doctor");
    let status = String::from_utf8_lossy(&aishe(&home).arg("status").output().unwrap().stdout).to_string();
    assert!(status.starts_with("AIShe status\n"), "{status}");
    assert!(!status.contains("· provider: "), "{status}");
    let doctor = String::from_utf8_lossy(&aishe(&home).arg("doctor").output().unwrap().stdout).to_string();
    assert!(!doctor.contains("version: version:"), "{doctor}");
    assert!(doctor.matches("aishe doctor --live").count() <= 1, "{doctor}");
    assert!(!doctor.contains("none theme"), "{doctor}");
    if cfg!(target_os = "macos") {
        assert!(doctor.contains("bubblewrap: not applicable on macOS"), "{doctor}");
        assert!(!doctor.contains("! sandbox.bubblewrap"), "{doctor}");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli status_and_doctor_do_not_repeat_themselves`
Expected: FAIL.

- [ ] **Step 3: Implement**

- `status.rs:103-110`: title `AIShe status`; connection line `connection: {label} · {endpoint_host} · {selection_scope}` (the label already carries the provider brand; the id goes on the `auth:` line as `auth: {auth} ({connection_id}) · {auth_state}`).
- `diagnostics.rs:118-126`: summary `format!("aishe {version}")` (the id `version` is already the row label).
- Collapse the not-running backend rows into one check `backend.live` with summary `backend live checks: not run` and detail `run \`aishe doctor --live\` for listener, auth, health, isolation, plugin, tool-bridge, and event-stream checks`; keep the eight individual checks only when `--live` ran (they then have real results).
- `sandbox.bubblewrap.*` on macOS: one `Info` row `bubblewrap: not applicable on macOS · workspace actions are policy-checked` instead of `Unsupported` + `Warn`.
- `ui.terminal_policy`: `terminal ui: theme {auto|dark|light|mono} · color {auto|always|never} · {unicode|ascii} glyphs · {live|static} motion` from the resolved policy with Display impls, never `none`.
- Titles: `AIShe status`, `AIShe doctor`, `AIShe self-test`, `AIShe uninstall plan`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib diagnostics && cargo test --test cli && cargo test --lib cli::status`
Expected: PASS (update any test that pinned `aishe status` lowercase).

- [ ] **Step 5: Commit**

```bash
git add src/cli/status.rs src/diagnostics.rs tests/cli.rs
git commit -m "fix: de-duplicate status and doctor output and use one title casing

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 22: One-shot slash handling matches the registry

**Why:** `INTERCEPTED` (`src/dispatcher.rs:372-377`) contains `"aishe"`, so `aishe -c 'aishe doctor'` prints `command registry has no one-shot handler for 'doctor'` and `aishe -c 'aishe status --json'` drops `--json`. The registry marks `/reset` and `/auth` shell-only although one-shot implements `reset` (`runtime.rs:1588`, unreachable) and `auth status` needs no TTY. `/status --json` and `/log --since 1h` are rejected by `ArgumentPolicy::None`. Custom commands with reserved names load silently and can never run (`commands.rs:227-261`).

**Files:**
- Modify: `src/dispatcher.rs:372-377`, `src/cli/runtime.rs:1542-1597`, `src/command_surface.rs` (`reset`, `auth`, `status`, `log`, `config` availability and argument policy), `src/commands.rs:227-261`, `src/cli/runtime.rs:1881-1884`
- Test: `tests/dispatcher.rs`, `tests/command_surface.rs`

- [ ] **Step 1: Write the failing tests**

`tests/dispatcher.rs`:

```rust
#[test]
fn aishe_prefixed_lines_run_as_shell_unless_a_one_shot_builtin_exists() {
    let home = temp_home("aishe-prefix");
    let doctor = aishe(&home).args(["-c", "aishe doctor"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&doctor.stderr).contains("no one-shot handler"));
    let status = aishe(&home).args(["-c", "aishe status --json"]).output().unwrap();
    assert!(String::from_utf8_lossy(&status.stdout).trim_start().starts_with('{'));
    let reset = aishe(&home).args(["-c", "/reset"]).output().unwrap();
    assert!(!String::from_utf8_lossy(&reset.stderr).contains("needs an interactive shell"));
}
```

`tests/command_surface.rs`:

```rust
#[test]
fn reserved_custom_command_names_are_skipped_with_a_warning() {
    let home = temp_home("reserved-custom");
    let commands = home.join("aishe").join("commands");
    std::fs::create_dir_all(&commands).unwrap();
    std::fs::write(commands.join("status.md"), "# shadowed\necho hi\n").unwrap();
    let output = aishe(&home).arg("commands").output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("custom command /status is shadowed by a built-in"), "{stderr}");
    assert!(!String::from_utf8_lossy(&output.stdout).contains("custom slash-commands: /status"));
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --test dispatcher aishe_prefixed_lines && cargo test --test command_surface reserved_custom_command_names`
Expected: FAIL.

- [ ] **Step 3: Implement**

- Remove `"aishe"` from `INTERCEPTED`. In `one_shot` (`runtime.rs:1540-1597`), before the builtin match, treat `aishe <id> …` as a builtin only when `by_id(id)` exists and `spec.support(Surface::OneShot).is_supported()`; otherwise `return Ok(executor.run(trimmed) as u8)`. Pass `tokens.contains("--json")` into `status::command` and `history::log` instead of the hard-coded `false`/`Some(20)`.
- Registry: `reset` and `auth` availability `surfaces(SUPPORTED, SUPPORTED, SUPPORTED)`; `status`, `log`, `config` `arguments: PassThrough("OPTIONS")` (the hook's word-split path from Task 6 passes them through); keep `ArgumentPolicy::None` only on `details`.
- `commands.rs` `load_dir`: skip a file whose stem satisfies `command_surface::is_reserved_slash` and push its name onto a `shadowed: Vec<String>` on the registry; `runtime.rs:1881-1884` prints `aishe: custom command /{name} is shadowed by a built-in (rename the file)` on stderr for each.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test dispatcher && cargo test --test command_surface && cargo test --lib product_help`
Expected: PASS (regenerate the docs block for the changed argument columns).

- [ ] **Step 5: Commit**

```bash
git add src/dispatcher.rs src/cli/runtime.rs src/command_surface.rs src/commands.rs docs/commands.md tests/dispatcher.rs tests/command_surface.rs
git commit -m "fix: align one-shot slash handling and custom-command loading with the registry

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 23: One cancel string, one mode-change message, one prompt footer

**Why:** Cancelling prints `aishe: palette closed`, `model selection cancelled`, `connection selection cancelled`, `OAuth logout cancelled`, `credential removal cancelled`, `resume cancelled`. Mode changes print `mode: %s (this shell)`, `mode = %s  (this shell)`, `aishe mode: %s`, `\naishe mode: %s`, `mode = auto  (saved to …)`, and Ctrl-O prints `aishe agent details: detailed`. Menus print `↑/↓ or number select · Enter accept · ? help · Esc cancel · b back`; text prompts `(or :back/:cancel):`; secrets `(hidden; Esc cancels):`; confirms accept a hidden `q`; static-motion menus use `|` separators; `menu()` prints `↑/↓` and `·` even under the ASCII policy.

**Files:**
- Modify: `src/promptui.rs` (`PICKER_HELP`, menu footer, text prompt suffix, secret suffix, confirm suffix via `crate::ui::render::approval_suffix`), `src/cli/connection.rs:210, 418, 723`, `src/auth.rs:280, 561`, `src/cli/session.rs:366-369`, `src/integration/registry.rs` (mode messages), `src/integration/assets/zsh_hook.zsh:561, 589`, `bash_hook.bash:313`
- Test: `src/promptui.rs` tests, `src/integration/tests.rs`

**Interfaces:**
- Produces: `pub const CANCELLED: &str = "cancelled";` `pub fn cancelled(what: &str) -> String` (`"{what} cancelled"` — e.g. `model selection cancelled` becomes `cancelled`, printed muted, with no `aishe:` prefix); `pub fn prompt_footer(caps: &TerminalCapabilities, back: bool) -> String` returning `↑/↓ or number · Enter accept · b back · ? help · Esc cancel` or the ASCII form `Up/Down or number | Enter accept | b back | ? help | Esc cancel`; `pub fn mode_line(mode: &str, scope: &str) -> String` (`"mode: {mode} ({scope})"`) shared by Rust and rendered into both hooks.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn prompt_footer_respects_the_glyph_policy() {
        let unicode = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, locale: Some("en_US.UTF-8".into()), ..CapabilityInputs::default() });
        assert_eq!(prompt_footer(&unicode, true), "↑/↓ or number · Enter accept · b back · ? help · Esc cancel");
        let ascii = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, locale: Some("C".into()), ..CapabilityInputs::default() });
        assert_eq!(prompt_footer(&ascii, false), "Up/Down or number | Enter accept | ? help | Esc cancel");
    }
```

`src/integration/tests.rs`: the rendered zsh and bash hooks contain `mode: %s (this shell)` exactly twice each (show and apply) and never `mode = ` or `aishe mode:`; the zsh hook's details toggle prints `details: %s`.

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib prompt_footer_respects && cargo test --lib integration::tests`
Expected: FAIL.

- [ ] **Step 3: Implement**

- Add `prompt_footer`, `cancelled`, `mode_line` to `src/promptui.rs`; `menu`, `filter_picker`, `static_menu`, `text` (`(or b back · Esc cancel):`), `secret` (`(hidden · Esc cancel):`), and `confirm` all use it. Every `println!("… cancelled")` site prints `println!("  {}", paint(CANCELLED, MUTED))`. The palette widget's `zle -M "aishe: palette closed"` becomes `zle -M "cancelled"`.
- `registry.rs`: render `mode_line("%s", "this shell")` into both hooks; `zsh_hook.zsh:589` fallback `zle -M "$(printf 'mode: %s (this shell)' "$AISHE_MODE")"`; `bash_hook.bash:313` likewise; `zsh_hook.zsh:561` `zle -M "details: ${AISHE_AGENT_OUTPUT}"`; `connection.rs` `set_or_show` prints `{field}: {value} (saved as default)`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib promptui && cargo test --lib integration::tests && python3 tests/model_picker_pty.py target/release/aishe`
Expected: PASS (update `model_picker_pty.py` expectations for the new footer and cancel text).

- [ ] **Step 5: Commit**

```bash
git add src/promptui.rs src/cli src/auth.rs src/integration tests/model_picker_pty.py tests/bare_words_pty.py
git commit -m "refactor: one cancel string, one mode message, one prompt footer

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Phase 2 exit gate

- [ ] `cargo fmt --all -- --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --all-targets --locked`
- [ ] `aishe --help` shows five groups and no duplicates; `aishe commands` fits in 24 lines; `aishe commands all` lists exactly the twenty visible commands; `aishe commands bogus` exits 2.
- [ ] `python3 tests/docs_contract_test.py` passes (the generated block was regenerated, the removed-commands table is gone, the troubleshooting rows match the code list).

---

# Phase 3 — One look

### Task 24: Export the resolved style policy from Rust into the zsh assets

**Why:** zsh paints with literal indices (`pty_prompt.zsh:161-186`: 242, 244, 215, 209, 204, yellow model, green scope) while Rust paints through `palette_entry` (`src/ui.rs:391-451`) with light and dark variants where scope is magenta. Scope is green in the RPROMPT and magenta in approval panels; light-theme users get yellow on white; 256-color indices are emitted regardless of depth. `pty.rs:99-105` exports `AISHE_UNICODE` but not theme or depth, so `ui.theme = "none"` never reaches the prompt (contradicting `docs/accessibility.md:8`). `wrapper.zshrc:34`, `zsh_hook.zsh:132`, and `bash_hook.bash:189` hard-code `%F{244}` / `\033[2m`.

**Files:**
- Modify: `src/pty.rs:99-115`, `src/ui.rs` (new `zsh_color_map`), `src/integration/assets/pty_prompt.zsh:161-200, 186, 216, 231`, `wrapper.zshrc:34`, `zsh_hook.zsh:132`, `bash_hook.bash:189`, `docs/accessibility.md`
- Test: `src/ui.rs` tests, `tests/statusline_pty.py` (NO_COLOR case)

**Interfaces:**
- Produces: `pub fn ui::zsh_color_map(caps: &TerminalCapabilities) -> Vec<(&'static str, String)>` returning pairs like `("AISHE_COLOR_MODEL", "%F{yellow}")`, `("AISHE_COLOR_SCOPE", "%F{magenta}")`, `("AISHE_COLOR_MUTED", "%F{242}")`, `("AISHE_COLOR_MODE_SUGGEST", "%F{yellow}%B")`, `("AISHE_COLOR_MODE_AUTO", "%F{cyan}%B")`, `("AISHE_COLOR_MODE_YOLO", "%F{red}%B")`, `("AISHE_COLOR_DANGER", "%F{red}%B")`, `("AISHE_COLOR_METRIC", "%F{209}")`, all empty strings when `!caps.styled()`; plus `AISHE_STYLE=none|mono|dark|light`. The zsh side reads only these variables.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn zsh_color_map_is_empty_when_styling_is_off_and_themed_otherwise() {
        let off = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, no_color: true, ..CapabilityInputs::default() });
        assert!(zsh_color_map(&off).iter().all(|(_, value)| value.is_empty()));
        let dark = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, term: Some("xterm-256color".into()), ..CapabilityInputs::default() });
        let map = zsh_color_map(&dark);
        let scope = map.iter().find(|(key, _)| *key == "AISHE_COLOR_SCOPE").unwrap();
        assert!(scope.1.contains("magenta"), "scope must match the Rust palette: {}", scope.1);
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib zsh_color_map_is_empty`
Expected: compile error.

- [ ] **Step 3: Implement**

- `src/ui.rs`: `zsh_color_map` maps each `StyleToken` used by the statusline through `palette_entry(token, theme)` to a zsh `%F{…}` (and `%B` for bold) string; when `depth` is 16-color, use named colors only; when styled is false, empty strings.
- `src/pty.rs:99-115`: after `AISHE_UNICODE`, `for (key, value) in crate::ui::zsh_color_map(&caps) { cmd.env(key, value); }` and `cmd.env("AISHE_STYLE", caps.style_name())` (`none` when unstyled).
- `pty_prompt.zsh:161-200`: replace the `style=` `case` and the `prompt_open` `case` with a lookup `prompt_open="${(P)${:-AISHE_COLOR_${field:u}}:-}"` for identity fields, `AISHE_COLOR_MODE_${mode:u}` for mode, `AISHE_COLOR_METRIC` for the token/cost fields, and `prompt_close='%f%b'` when `prompt_open` is non-empty. Line 186 separator `' %F{242}·%f '` becomes `" ${AISHE_COLOR_MUTED}·%f "` (empty color when unstyled). Line 216 `base_prompt` uses `${AISHE_COLOR_PATH}` / `${AISHE_COLOR_MODE_${mode:u}}`. `wrapper.zshrc:34` and `zsh_hook.zsh:132` use `${AISHE_COLOR_MUTED}`; `bash_hook.bash:189` reads `AISHE_STYLE` and prints no escapes when it is `none`.
- `docs/accessibility.md`: state that `ui.theme`, `NO_COLOR`, and `TERM=dumb` now apply to the prompt and statusline too.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib ui && cargo build --release && NO_COLOR=1 python3 tests/statusline_pty.py target/release/aishe && python3 tests/statusline_pty.py target/release/aishe`
Expected: PASS; add a `NO_COLOR` case to `statusline_pty.py` asserting no `\x1b[3` sequences in the prompt line.

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs src/pty.rs src/integration/assets docs/accessibility.md tests/statusline_pty.py
git commit -m "feat: drive the prompt and statusline colors from the Rust palette

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 25: One mode label, one focus glyph, one separator, one ellipsis

**Why:** `pty_prompt.zsh:139` maps `suggest` to `review` in the statusline while every other surface says `suggest` (`tests/model_picker_pty.py:216` pins the wrong one). Three focus chevrons: prompt `❯`/`»`, menu `›` (`ui.rs:590-596`), picker `>` (`render.rs:231`). Separators mix `·`, ` — `, `  |  ` (`renderer.rs:594,651`), `;` (`tool_worker.rs:704-707`). `src/ui.rs:473` always emits U+2026 even under the ASCII policy (also `render.rs:348`, `renderer.rs:859,869`, `commands.rs`). Hardcoded escapes live at `promptui.rs:467,474,477,947,955`, `renderer.rs:553,562`, `tool_worker.rs:1464`, `modes/mod.rs:361`.

**Files:**
- Modify: `src/integration/assets/pty_prompt.zsh:136-142`, `tests/model_picker_pty.py:216`, `src/ui.rs` (`Glyphs::ellipsis`, `Glyphs::separator`, `truncate_cells_with`, `pub mod cursor`), `src/ui/render.rs:231, 348`, `src/agent/renderer.rs:594, 651, 859, 869`, `src/agent/tool_worker.rs:704-707, 1464`, `src/promptui.rs:467-477, 947, 955`, `src/modes/mod.rs:361`, `src/commands.rs` (`command_status_summary`)
- Test: `src/ui.rs` tests

**Interfaces:**
- Produces: `Glyphs::ellipsis(self) -> &'static str` (`…` / `...`), `Glyphs::separator(self) -> &'static str` (`·` / `|`), `pub fn truncate_cells_with(value: &str, width: usize, glyphs: Glyphs) -> String`, `pub mod ui::cursor { pub fn clear_line(caps) -> &'static str; pub fn up(caps, n) -> String; pub fn clear_below(caps) -> &'static str }` returning empty strings when `caps.motion == Motion::Static` or not a TTY.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn ascii_policy_truncates_with_three_dots_and_pipes() {
        let ascii = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, locale: Some("C".into()), ..CapabilityInputs::default() });
        assert_eq!(truncate_cells_with("abcdefgh", 6, ascii.glyphs()), "abc...");
        assert_eq!(ascii.glyphs().separator(), "|");
        let unicode = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, locale: Some("en_US.UTF-8".into()), ..CapabilityInputs::default() });
        assert_eq!(truncate_cells_with("abcdefgh", 6, unicode.glyphs()), "abcde…");
        assert_eq!(unicode.glyphs().focus(), "›");
        assert_eq!(cursor::clear_line(&ascii), "\r\x1b[2K");
        let static_caps = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: false, ..CapabilityInputs::default() });
        assert_eq!(cursor::clear_line(&static_caps), "");
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib ascii_policy_truncates`
Expected: compile error.

- [ ] **Step 3: Implement**

- `pty_prompt.zsh:139`: `suggest) value='suggest' ;;`. `tests/model_picker_pty.py:216`: expect `suggest`.
- `ui.rs`: add the glyph methods and `truncate_cells_with`; `truncate_cells` keeps its signature and calls `truncate_cells_with(value, width, TerminalCapabilities::detect_stdout().glyphs())`. Add `pub mod cursor`. `render.rs:231` picker rows use `glyphs.focus()` (the ASCII variant of `focus()` is already `>`; the Unicode variant `›` becomes the one chevron for menus and pickers; the prompt keeps its mode glyphs because they encode mode, not focus).
- Replace `"  |  "` joins in `renderer.rs:594,651` and `"; "` in `tool_worker.rs:704-707` with `format!(" {} ", glyphs.separator())`. Replace every literal `"\r\x1b[2K"`, `"\x1b[A"`, `"\x1b[J"`, `"\x1b[0m"` at the listed sites with the `cursor` helpers.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib ui && cargo test --lib agent && cargo build --release && python3 tests/model_picker_pty.py target/release/aishe && python3 tests/agent_ui_pty.py target/release/aishe`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/ui.rs src/ui/render.rs src/agent src/promptui.rs src/modes/mod.rs src/commands.rs src/integration/assets/pty_prompt.zsh tests/model_picker_pty.py
git commit -m "refactor: one mode label, one focus glyph, one separator, one ellipsis policy

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 26: Renderer widths, blank lines, hint gating, and unstyled markdown

**Why:** Question/approval panels draw the top rule at `content_width + 5` and the bottom at `content_width + 6` (`renderer.rs:748-751` vs `777-780`), so `┐` sits one column left of `┘`; the test at `:1005-1007` only checks `<= 42`. The live status line uses `safe(value, columns - 2)` (chars) plus two leading spaces (`renderer.rs:552-553`), so CJK wraps and the next clear leaves a fragment. `render.rs:185-194` and `modes/mod.rs:456-465, 500-509` draw fixed 52/40-column boxes. `renderer.rs:453` and `:526` print two blank lines before the answer. `renderer.rs:629-631` appends `Ctrl-O details next turn` to every Focus summary, ungated and also outside the hook. `modes/mod.rs:165-172` prints raw `**bold**` when unstyled, and Detailed streams raw markdown while Focus renders through termimad.

**Files:**
- Modify: `src/agent/renderer.rs:453, 526, 552-553, 629-631, 748-780, 1005-1007`, `src/ui/render.rs:185-194`, `src/modes/mod.rs:165-172, 383-387, 456-465, 500-509`, `src/hints.rs`
- Test: `src/agent/renderer.rs` tests

**Interfaces:**
- Produces: `hints::details_hint_pending(config) -> bool` / `hints::mark_details_hint_seen(config)` (same pattern as `launch_hint_*`); `render::panel_width(caps) -> usize`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn panel_corners_align() {
        let caps = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, columns: 100, locale: Some("en_US.UTF-8".into()), ..CapabilityInputs::default() });
        let panel = question_panel("Title", &[("label", "value")], "body line", "footer", &caps);
        let lines: Vec<&str> = panel.lines().collect();
        assert_eq!(crate::ui::cell_width(lines[0]), crate::ui::cell_width(lines[lines.len() - 1]), "{panel}");
    }

    #[test]
    fn live_status_never_exceeds_the_terminal_width() {
        let caps = TerminalCapabilities::resolve(&CapabilityInputs { is_tty: true, columns: 40, locale: Some("en_US.UTF-8".into()), ..CapabilityInputs::default() });
        let line = status_line("running 日本語のとても長いディレクトリ名/もう一つ/更に長い", &caps);
        assert!(crate::ui::cell_width(&line) <= 40, "{line}");
    }
```

(`question_panel` and `status_line` are the pure builders; if the renderer builds these inline, extract them first so they take `&TerminalCapabilities` and return `String`.)

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib renderer::tests::panel_corners_align live_status_never_exceeds`
Expected: FAIL / compile error.

- [ ] **Step 3: Implement**

- Bottom rule: `horizontal.repeat(content_width + 1)`. Replace the `<= 42` assertion with the corner test above.
- Status line: `let width = usize::from(self.capabilities.columns).saturating_sub(3); print!("{}  {}", cursor::clear_line(&self.capabilities), crate::ui::truncate_cells_with(&safe(value, 4096), width, glyphs));`.
- `render.rs:185-194` and `modes/mod.rs` boxes: width from `panel_width(caps)` = `columns.saturating_sub(4).clamp(24, 100)` like `waiting_panel`.
- Delete the `println!()` at `renderer.rs:453` (keep the one in `begin_assistant_answer`).
- Ctrl-O hint: push it only when `std::env::var_os("AISHE_SHELL_ID").is_some() && hints::details_hint_pending(config)`, then `mark_details_hint_seen`; add the two functions to `hints.rs` following `launch_hint_pending`/`mark_launch_hint_seen` and a `details` field to `DiscoveryStatus` (`aishe hints status` prints it).
- Unstyled markdown: `modes/mod.rs:165-172` renders through `termimad::MadSkin::no_style()` so headers and lists keep structure without color; Detailed mode (`renderer.rs:145-150`, `mod.rs:383-387`) renders the final answer through the same function as Focus after streaming completes.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib agent && cargo test --lib modes && cargo test --lib hints && python3 tests/agent_ui_pty.py target/release/aishe`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/agent/renderer.rs src/ui/render.rs src/modes/mod.rs src/hints.rs
git commit -m "fix: align panel corners, measure the live status in cells, show the Ctrl-O hint once

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 27: Connection picker layout, `/details` vocabulary, SIGWINCH, default items

**Why:** The connection picker prints `Add a new account: aishe setup · /help accounts` above two blank lines and the title, repeats the label in two columns, and shows `Auto (legacy)` for a migrated row (`cli/connection.rs:360-420`). Ctrl-O prints `details: detailed` while `/output` speaks of focus/compact/detailed and `/details` is a toggle: two commands, two vocabularies. Resize is a 200 ms poll (`pty.rs:329-347`). `pty_prompt.zsh:90` and `config.rs:713-722` carry two default item lists (Task 11 aligned the order; this makes it one constant).

**Files:**
- Modify: `src/cli/connection.rs:360-420`, `src/command_surface.rs` (`details` accepts `[focus|compact|detailed] [--default]`; `output` hidden alias), `src/integration/assets/zsh_hook.zsh:555-562` (Ctrl-O cycles focus → detailed → compact), `src/pty.rs:329-347`, `src/pty.rs` (export `AISHE_STATUS_ITEMS` from `default_status_line_items` always; `pty_prompt.zsh:90` fallback removed)
- Test: `src/cli/connection.rs` tests, `tests/statusline_pty.py`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn connection_rows_show_label_auth_and_model_once() {
        let row = connection_row("Codex - OAuth · codex1", "OAuth · codex1", "gpt-5.6-luna", 80);
        assert_eq!(row.matches("codex1").count(), 2);
        assert!(!row.contains("Auto (legacy)"));
        let legacy = connection_row("Anthropic", "legacy · run aishe connection edit", "claude-x", 80);
        assert!(legacy.contains("legacy"));
    }
```

`tests/statusline_pty.py`: a case that sends a `TIOCSWINSZ` resize from 100 to 60 columns and expects the next prompt's `RPROMPT` width to fit within 60 within one second (the poll is 200 ms today; SIGWINCH makes it immediate; assert `<= 0.3 s`).

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test --lib connection_rows_show`
Expected: compile error.

- [ ] **Step 3: Implement**

- Picker: title first (`Select an account`), then rows `label · auth · model` through `connection_row(label, auth, model, width)`; the `Add a new account` hint becomes the picker footer line; a connection with `auth = auto` renders `legacy · run aishe connection edit`.
- `/details`: registry `arguments: PassThrough("[focus|compact|detailed] [--default]")`, `effect: ThisShellOrDefault`; the hook's `ToggleDetails` action accepts a value (`AISHE_AGENT_OUTPUT=$value`) and with `--default` calls `command aishe output $value`; Ctrl-O cycles `focus → detailed → compact → focus` and prints `details: {value} (this shell)`; `output` spec becomes `hidden: true` with the same handler. `/help session` documents `/details` only.
- SIGWINCH: in `pty.rs` register `libc::signal(libc::SIGWINCH, handler)` where the handler sets an `AtomicBool`; the existing thread checks the flag every 20 ms and resizes immediately, keeping the 200 ms size poll as a fallback.
- `pty.rs` always exports `AISHE_STATUS_ITEMS` (from config or `default_status_line_items()`); delete the fallback literal at `pty_prompt.zsh:90` and read `${AISHE_STATUS_ITEMS}` only.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib cli::connection && cargo test --lib product_help && python3 tests/statusline_pty.py target/release/aishe && python3 tests/model_picker_pty.py target/release/aishe`
Expected: PASS (regenerate the docs block for the `details` row).

- [ ] **Step 5: Commit**

```bash
git add src/cli/connection.rs src/command_surface.rs src/integration src/pty.rs docs/commands.md tests/statusline_pty.py
git commit -m "refactor: tidy the account picker, unify /details, resize on SIGWINCH

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Phase 3 exit gate

- [ ] `cargo test --all-targets --locked` and every `tests/*_pty.py` green.
- [ ] Manual, in a light-theme terminal with `ui.theme = "light"`: the statusline colors match the approval panel colors; with `NO_COLOR=1` nothing in the prompt line contains an escape sequence; resize the window and the status re-fits before the next keystroke.

---

# Phase 4 — Setup on a diet

### Task 28: Skip the empty first step; collapse environment checks to one line each

**Why:** On a fresh install Step 1 prints seven "fresh / not created / none" lines and a menu whose only real choice is "Continue setup" (`setup.rs:576-608`); Step 2 prints Debug-formatted enums (`{:?}` of `Theme`, `ColorDepth`, `UnicodePolicy`, `Motion`, `setup.rs:1683-1696`) and `config:` again; Step 3 prints a 64-hex SHA-256 and a license line (`setup.rs:1742-1751`); Step 4 is a warning. Roughly 25 lines before "Provider service". `active preference: anthropic · claude-sonnet-4-20250514 · suggest` is a preference nobody set with a 2025 model id (`config.rs:774`).

**Files:**
- Modify: `src/setup.rs:576-608, 1610-1651, 1683-1696, 1729-1753, 766-774`, `src/ui.rs` (Display impls for the four enums), `src/config.rs:774`
- Test: `src/setup.rs` tests, `tests/setup_pty.py`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn fresh_install_environment_summary_is_three_lines() {
        let summary = environment_summary(&EnvironmentFacts {
            zsh: Some("zsh 5.9".into()),
            runtime: Some("OpenCode 1.18.27 verified".into()),
            sandbox: SandboxFact::PolicyOnly("macOS"),
            fresh: true,
        });
        assert_eq!(summary.lines().count(), 3, "{summary}");
        assert!(summary.contains("✓ zsh 5.9"));
        assert!(!summary.contains("SHA-256"));
        assert!(!summary.to_lowercase().contains("truecolor"));
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib fresh_install_environment_summary`
Expected: compile error.

- [ ] **Step 3: Implement**

- Add `struct EnvironmentFacts`, `enum SandboxFact { Bubblewrap, PolicyOnly(&'static str) }`, and `fn environment_summary(facts) -> String` producing `  ✓ zsh 5.9 · /opt/homebrew/bin/zsh`, `  ✓ agent runtime OpenCode 1.18.27 · verified`, `  ✓ sandbox: policy checks (macOS has no kernel sandbox)` through `promptui::success`/`promptui::warning` so the ASCII fallback applies.
- In `run`'s interactive loop: when `Step::Discovery` finds no config file and no draft, print `AIShe setup` + one line `Fresh install · config will be written to {path} on Apply` and advance without a menu. `Step::Platform`, `Step::Runtime` (when the runtime is already verified), and `Step::Sandbox` print their `environment_summary` line and advance; the detailed inventory moves behind `aishe doctor`. When the runtime needs installing, the Step 3 menu stays but its preamble is `engine: OpenCode 1.18.27 · 44 MiB from github.com · checksum-verified` (one line).
- `impl Display` for `Theme`, `ColorDepth`, `UnicodePolicy`, `Motion` in `src/ui.rs` (`dark`, `truecolor`, `unicode`, `live`), and use them anywhere `{:?}` was printed to a user.
- `config.rs:774`: the fallback default model becomes the current Anthropic default id used by `provider_catalog.rs` for Anthropic (one constant, `grep -n "claude-" src/provider_catalog.rs`), and Step 1 never prints `active preference` on a fresh install.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib setup && cargo build --release && python3 tests/setup_pty.py target/release/aishe`
Expected: PASS after updating `setup_pty.py` expectations that waited for `Continue setup` on a fresh install.

- [ ] **Step 5: Commit**

```bash
git add src/setup.rs src/ui.rs src/config.rs tests/setup_pty.py
git commit -m "refactor: start setup at the first real question

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 29: Recommended defaults; endpoint only when it varies; the backend starts once

**Why:** Seventeen prompts on the happy path (`setup.rs:909-928, 1779-1842, 1335-1415`). `API endpoint [https://api.openai.com]` is asked for every catalog row although `docs/getting-started.md:71-73` promises it is filled in; scope, network, density, placement, contents, and audit are choices a first-time user cannot evaluate and Settings can change later. The backend is spawned three or four times (`setup.rs:668, 745, 1844-1864, 2650` plus `install.sh:243`) with multi-second silent pauses. Review dumps a full TOML diff on a fresh install (`setup.rs:2582-2587`).

**Files:**
- Modify: `src/setup.rs:909-928` (Endpoint), `1335-1415` (interface), `1779-1842` (scope/network), `668, 745, 1844-1864, 2650` (backend), `2582-2587` (diff)
- Test: `tests/setup_pty.py`

- [ ] **Step 1: Write the failing PTY expectation**

In `tests/setup_pty.py`'s happy-path scenario, count the prompts answered between "Provider service" and "Review and apply"; assert it is at most 5 for the recommended-defaults path (`service`, `credential`, `model`, `defaults`, `live checks`). Run it to see the current count (about 12) fail the assertion.

- [ ] **Step 2: Implement**

- `Step::Endpoint`: `if provider.preset != Preset::Custom && provider.preset != Preset::Ollama { advance(&mut draft)?; continue; }` (use whichever field names the catalog exposes; `grep -n "custom\|ollama" src/provider_catalog.rs`).
- After `Step::Model`, insert one menu `Behavior and interface`: rows `Use recommended defaults (workspace scope · network deny · focus output · status on the right · audit off)` and `Customize each setting`. The first row sets those values on `draft.config` and jumps to `Step::Pricing`; the second continues into `Profile`, `Status` as today.
- Backend: `Step::Runtime` skips its smoke test when `Step::Validation` will run (the draft is interactive and not `--verify`); `validate_managed_backend` keeps the supervisor running (drop `request_stop`) and `save_transactional` reuses it (`ensure_running` returns fast when alive). Print `  starting the agent runtime…` before each wait that can exceed one second.
- Diff: `if baseline_existed { println!("\n{diff}") }` else print `  New configuration; nothing to compare.`

- [ ] **Step 3: Run the tests**

Run: `cargo test --lib setup && cargo build --release && python3 tests/setup_pty.py target/release/aishe`
Expected: PASS with the prompt count at or below 5 on the defaults path and the customize path still exercised by a second scenario.

- [ ] **Step 4: Commit**

```bash
git add src/setup.rs tests/setup_pty.py
git commit -m "feat: recommended defaults in setup, endpoint only when it varies, one backend start

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 30: Completion summary, terminology, casing, footers, labels, headers

**Why:** The completion block prints `✓ sandbox  policy · workspace` on macOS after Step 4 warned there is none, `✓ provider  openai · gpt-5` in raw keys, `✓ history  preserved` on a fresh install, with a hard-coded `✓` (`setup.rs:1505-1542`). The runtime has six names (`setup.rs:653, 683, 1432, 1514, 1597, 1742, 2504`, `tour.rs:125, 209`, `product_help.rs:159`); setup says "Provider service"/"Credential profile" where the shell says "account"/"connection"; "status-line"/"statusline"/"status line"; headers are lowercase `aishe setup`/`aishe guided tour`/`aishe settings`. Notice capitalization is random (`setup.rs:614, 670, 782, 1457` lowercase vs `933, 949, 1134, 1190, 1505` capitalized); "Review and Apply" and "OpenCode Agent Runtime" are the only Title Case titles. Confirm labels mix "?" (`1406, 2102`) and none (`1463, 1482, 1543, 637, 827`). Misleading labels at `769, 985, 1108, 1220, 1254-1257, 1274, 1379, 2322`. Endpoint, Credential, Pricing print no step header (`909, 929, 1283`); Step 1 allows back with nothing behind it (`595`); Service forbids back (`871`). Every setup error gets "Run `aishe setup --verify`" (`main.rs:143-149`).

**Files:**
- Modify: `src/setup.rs` (all sites above), `src/tour.rs:60, 65, 105, 112, 125, 209`, `src/settings.rs:257`, `src/product_help.rs:159`, `src/main.rs:143-149`
- Test: `src/setup.rs` tests (string-level), `tests/setup_pty.py`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn user_facing_setup_strings_follow_the_style_rules() {
        let source = include_str!("setup.rs");
        for forbidden in ["\"aishe setup\"", "status-line", "status line", "Review and Apply", "OpenCode Agent Runtime", "Managed backend", "agent engine", "(recommended)\"", "Provider service"] {
            assert!(!source.contains(forbidden), "setup.rs still contains {forbidden:?}");
        }
        for question in ["Apply this configuration", "Run the guided first-session tour now", "Run the disclosed live capability checks now"] {
            assert!(!source.contains(&format!("\"{question}\"")), "yes/no label {question:?} must end with a question mark");
        }
    }
```

(A source-level test is deliberate: these strings are scattered through a 3000-line file and a grep-style guard is the cheapest way to keep them from drifting back.)

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib user_facing_setup_strings_follow_the_style_rules`
Expected: FAIL.

- [ ] **Step 3: Apply the vocabulary**

| Was | Becomes |
| --- | --- |
| Agent runtime / OpenCode Agent Runtime / agent engine / backend: opencode / Managed backend / pinned agent engine / managed OpenCode runtime | `agent runtime (OpenCode 1.18.27)` on first mention, `agent runtime` after |
| Provider service / Credential profile 'x' | `Account` / `Sign-in for {label}` |
| status-line / status line | `statusline` |
| aishe setup / aishe guided tour / aishe settings (headers) | `AIShe setup` / `AIShe tour` / `AIShe settings` |
| Review and Apply | `Review and apply` |
| every notice | sentence case, no trailing period on one-liners |
| yes/no labels | end with `?` |
| Enter and save an API key locally (recommended) | `(recommended)` only on the pre-selected row (Task 12) |
| Back to credential or endpoint | `Back` |
| OAuth is ready; model availability is validated through the managed runtime | `OAuth login found; the model list comes from your subscription` |
| Conservative — suggest, confirm all tool commands (etc.) | `Suggest (conservative) — propose, you confirm` / `Auto (balanced) — run safe commands, confirm the rest` / `Yolo (autonomous) — run without asking in this shell` / `Custom — keep each setting as it is` |
| 3 safety setting(s) changed | removed |
| preview: (status line off) / preview (right): … | `preview: {text}` / `preview: off` |
| OS sandbox: unavailable in this macOS release | `macOS: no kernel sandbox; policy checks only` |
| OAuth profile label [work] | `Profile name for this login (for example work or personal) [work]` |

Completion summary: rows through `promptui::success` / `promptui::skipped` (add `skipped` next to `success` in `promptui.rs` printing the `·` glyph); `sandbox` row shows `policy checks (macOS)` with the skipped glyph; `provider` row prints the connection label and model; `history` row only when a history file exists. Step headers before Endpoint, Credential, Pricing (`step_header`); `allow_back = false` at Discovery, `true` at Service. `main.rs:143-149`: next action by exit code (`EXIT_INPUT` → "Run `aishe setup`", `EXIT_RUNTIME` → "Run `aishe backend install`", `EXIT_SANDBOX` → the package command from `dependencies.rs`, `EXIT_PROVIDER` → "Run `aishe setup --verify --live`", `EXIT_PAUSED` → "Run `aishe setup --resume`").

Tour: `tour.rs:60`/`105` one completion sentence `Tour complete · run aishe and type a command or a question`; lesson number printed once (drop the numeral from `112`, keep `Lesson 1 of 8`); the setup epilogue and the tour both use `CONTROLS_HINT`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib setup && cargo test --lib tour && cargo build --release && python3 tests/setup_pty.py target/release/aishe`
Expected: PASS after updating the PTY expectations for the renamed strings.

- [ ] **Step 5: Commit**

```bash
git add src/setup.rs src/tour.rs src/settings.rs src/product_help.rs src/promptui.rs src/main.rs tests/setup_pty.py
git commit -m "style: one vocabulary and one casing across setup, tour, and settings

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 31: Setup and installer leftovers

**Why:** `--verify --json` always reports `runtime: null` (`setup.rs:247-253`); `--verify --live` and `--non-interactive --live` use different verdict functions (`243-245` vs `316-319`); a draft resumed at Review has no report so the validation line disappears; `install.sh` exits 1 after a successful install without a TTY (`319-323`, fixed in Task 9), honors `AISHE_CONFIG_DIR`/`AISHE_DATA_DIR` (`217-221`) but does not document them (`8-16`), and interleaves stderr notes with the backend subcommands' stdout.

**Files:**
- Modify: `src/setup.rs:243-253, 316-319`, the Review step (persist `report` in the draft or re-run validation on resume), `install.sh:8-16, 243`
- Test: `tests/cli.rs` (`setup --verify --json` has a `runtime` object), `sh tests/installer_bindir.sh`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn verify_json_reports_the_runtime() {
    let home = temp_home("verify-json");
    let output = aishe(&home).args(["setup", "--verify", "--json"]).output().unwrap();
    let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(document["runtime"].is_object(), "{document}");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --test cli verify_json_reports_the_runtime`
Expected: FAIL (`runtime` is null).

- [ ] **Step 3: Implement**

- `setup.rs:247-253`: pass the runtime status the non-interactive path computes (`grep -n "runtime:" src/setup.rs` near 316) into the verify JSON.
- Both `--live` paths use `report.verified()`.
- Persist `report` in the draft (`serde` on `Report` already exists for `--json`) so a resumed Review prints its validation line.
- `install.sh:8-16`: add `AISHE_CONFIG_DIR` and `AISHE_DATA_DIR` rows to the header table; `install.sh:243` and the backend install call: append `>&2` so notes and runtime output share one stream, or pass `--quiet` if `backend verify` supports it.

- [ ] **Step 4: Run the tests**

Run: `cargo test --test cli verify_json && sh -n install.sh && shellcheck install.sh && sh tests/installer_bindir.sh && sh tests/installer_upgrade.sh`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/setup.rs install.sh tests/cli.rs
git commit -m "fix: consistent setup verdicts and installer output

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Phase 4 exit gate

- [ ] `python3 tests/setup_pty.py target/release/aishe` and `python3 tests/qualify.py quick --output test-results/qualification-quick.json` pass.
- [ ] Manual fresh install in a temp `AISHE_CONFIG_DIR`/`AISHE_DATA_DIR`: from `aishe` to the shell prompt takes at most eight prompts on the recommended path and the scrollback before "Account" is three lines.

---

# Phase 5 — Docs and guards

### Task 32: Generate the CLI block in `docs/commands.md` from clap

**Why:** The hand-written "Subcommands" block (`docs/commands.md:12-60`) omits 18 existing commands (palette, agent, inbox, task, plan, replan, last, ask, index, capabilities, test, demo, role, dry-run, history, update, man, suggest) and still lists `aishe provider … (legacy form)`. The slash table is generated and guarded; the CLI block is not.

**Files:**
- Create: `tests/docs_cli_block_test.py`
- Modify: `src/product_help.rs` (`pub fn markdown_cli_reference() -> String`), `src/cli/args.rs` (`Commands { markdown_cli: bool }` hidden flag on `commands` or a hidden `aishe commands --cli-markdown`), `docs/commands.md:12-60` (between new `<!-- BEGIN GENERATED CLI SURFACE -->` / `END` markers)
- Test: `src/product_help.rs` tests (`commands_markdown_matches_the_generated_cli_block`), the new Python test wired into CI next to `docs_contract_test.py`

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn commands_markdown_matches_the_generated_cli_block() {
        let document = std::fs::read_to_string("docs/commands.md").unwrap();
        let start = document.find("<!-- BEGIN GENERATED CLI SURFACE -->").expect("begin marker");
        let end = document.find("<!-- END GENERATED CLI SURFACE -->").expect("end marker");
        let actual = document[start..end].lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(actual.trim(), markdown_cli_reference().trim());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --lib commands_markdown_matches_the_generated_cli_block`
Expected: FAIL (no markers / no function).

- [ ] **Step 3: Implement**

`markdown_cli_reference` walks `Args::command()` (skipping hidden subcommands), grouping by `help_heading`, and emits a fenced block of `aishe <name> <usage>   <about first sentence>` rows, 22-column padded, matching the existing hand-written style. Add a hidden `--cli-markdown` flag to `aishe commands` that prints it (so a maintainer can regenerate with `aishe commands --cli-markdown`). Replace lines 12-60 of `docs/commands.md` with the markers and the generated text. `tests/docs_cli_block_test.py` runs `target/release/aishe commands --cli-markdown` (or the debug binary in CI) and diffs it against the marked block, mirroring the Rust test for environments that only run the Python suite.

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib product_help && python3 tests/docs_cli_block_test.py target/release/aishe && python3 tests/docs_contract_test.py`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/product_help.rs src/cli/args.rs src/main.rs docs/commands.md tests/docs_cli_block_test.py .github/workflows/ci.yml
git commit -m "docs: generate the CLI reference block from clap and guard it

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 33: Docs say what the binary does

**Why:** `docs/getting-started.md:8-31` makes the user run `aishe auth set` and then says setup does that for you; line 41 refers to "That directory" never introduced; line 101 "Setup writes nothing until you apply the review" is false (draft, runtime, packages, and OAuth tokens are written earlier); lines 139-143 show a suggest-mode key prompt that does not match `suggest.rs:519` and does not exist in the zsh front-end. `docs/installation.md:43-48` says `--setup` opens setup; on an existing install the script prints "Run `aishe doctor`" first. `docs/daily-driver.md:16, 21-23` describes palette features removed in d20e627/d79dd6c. `docs/troubleshooting.md:31` was corrected in Task 5; the `cli.invalid_provider_flags` row is deleted in Task 17.

**Files:**
- Modify: `docs/getting-started.md`, `docs/installation.md`, `docs/daily-driver.md`, `docs/front-ends.md` (Task 10 rule), `docs/shell-integration.md` (Task 13 rule), `docs/accessibility.md` (Task 24), `README.md` (feature bullets: prompt, `/details`, palette)
- Test: `python3 tests/docs_contract_test.py`; `cargo test --lib product_help` for any doc paragraph the Rust tests quote

- [ ] **Step 1: Edit**

- getting-started: section 1 becomes "Run `aishe setup`" (setup signs you in); `aishe auth` moves to an "Alternatives" note; introduce the config directory before "That directory"; replace line 101 with "Setup does not change `config.toml` or `credentials.toml` until Apply. The runtime, system packages, and an OAuth login are written when you choose them, and the resumable draft is saved after every step."; replace the key-prompt figure with the real text from `suggest.rs:519` and add one sentence for the zsh front-end ("the proposal is staged on your line; Enter runs it, editing it first is normal").
- installation: "`--setup` runs guided setup after the install; on an upgrade the installer skips the doctor reminder and starts setup directly."
- daily-driver: the palette section lists what `aishe palette --json` returns today (the twenty visible commands with their effect) and the three ways to open it.
- README: keep "Your real zsh, untouched" with the Task 10 parenthetical; describe `/details [focus|compact|detailed]`; describe the palette as filling slash forms.

- [ ] **Step 2: Run the docs tests**

Run: `python3 tests/docs_contract_test.py && cargo test --lib product_help`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add docs README.md
git commit -m "docs: match getting-started, installation, and daily-driver to the binary

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 34: Wire the new PTY tests into `qualify.py` and CI

**Why:** Every Phase 1 defect had no test because the in-shell paths were exercised only by hand. The new PTY tests must run where `statusline_pty.py` and `model_picker_pty.py` run.

**Files:**
- Modify: `tests/qualify.py` (the `local-full`/`release` suites), `.github/workflows/ci.yml` (macOS and Linux jobs that already run `model_picker_pty.py`), `docs/development.md:150-175`
- Test: `python3 tests/qualify.py --list`

- [ ] **Step 1: Register the tests**

Add `in_shell_menus_pty.py`, `yolo_consent_pty.py`, `palette_pty.py`, `mode_handoff_pty.py`, `bare_words_pty.py`, `theme_prompt_pty.py`, `keys_pty.py` next to `model_picker_pty.py` in `qualify.py`'s suite definitions and in the CI steps that run PTY tests on macOS and Linux. List them in `docs/development.md` under the same heading.

- [ ] **Step 2: Run the quick suite**

Run: `python3 tests/qualify.py --list && python3 tests/qualify.py quick --output test-results/qualification-quick.json`
Expected: the seven new tests appear in the listing and the quick suite passes.

- [ ] **Step 3: Commit**

```bash
git add tests/qualify.py .github/workflows/ci.yml docs/development.md
git commit -m "test: run the in-shell PTY regressions in qualify and CI

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Phase 5 exit gate

- [ ] `python3 tests/qualify.py local-full --output test-results/qualification-local.json` passes on macOS.
- [ ] `python3 tests/qualify.py linux-full …` passes on the Linux qualification host (bubblewrap present).
- [ ] `CHANGELOG.md` gains an "Unreleased" section listing the Phase 1 fixes as user-visible bullets and the Phase 2 surface changes (hidden aliases, removed tombstones, `aishe provider`/`demo`/`replan` hidden, `/mode --default`, `/details` values) under "Changed".

---

## Self-review against the report

**Coverage.** Every P0 in the report maps to Phase 1 Task 1-14 in the report's order. P1/P2 items: help/topics/controls (Tasks 18, 21), duplicates and tombstones (16, 17), effect vocabulary (15), error shapes (5, 19), clap help (20), one-shot and custom commands (22), cancel/mode/footer strings (23), colors and NO_COLOR (24), labels/glyphs/separators/ellipsis/escapes (25), renderer and hints and markdown (26), picker layout, `/details`, resize, default items (27), setup steps and prompts and backend spawns and diff (28, 29), setup terminology/casing/labels/headers/next-actions and tour voice (30), setup JSON and installer leftovers (31), docs (32, 33), tests in CI (34). Items deliberately not planned: the `PickerResult::SaveDefault` dead variant and the per-fragment `detect_stdout` calls (report "Minor leftovers") — delete the variant in Task 23 if it is still unused after the footer change; the detection calls are cheap and stay.

**Decisions recorded here so executors do not re-litigate them.**
- The yolo consent keeps its typed word for both scopes (Task 2). Only the decline path and the raw-mode reader change; the safety contract in `docs/safety.md` is untouched.
- The branded prompt stays on by default for stock zsh prompts (Task 10). Detection, not a config flip, is what protects theme users; `AISHE_PTY_PROMPT=force` exists for people who want the glyph regardless.
- Shift-Tab cycles the mode only on an empty line (Task 13). This is the smallest rule that keeps oh-my-zsh's binding working and makes the documented behaviour true.
- Tombstones are deleted rather than aged out (Task 16). 0.7.0 is pre-1.0 and the migration topic has served its purpose; `CHANGELOG.md` keeps the record.
- `aishe usage` remains the audit-log report; `/usage` becomes an alias of `/status` (Tasks 7, 16). Two different reports under one name was the confusion.
- `mode`, `details`, `connection`, `model`, `reasoning` are the five shell-local commands with a `--default` promotion (Task 15). Everything else either saves config or touches nothing.

**Type and name consistency.** `UserFacing` (Task 5) is used by Tasks 9 and 19. `ShellContext` (Task 6) is the only reader of `AISHE_SHELL_ID`/`AISHE_PENDING_FILE` on the Rust side. `Effect` (Task 15) feeds `effect_label`, the palette (Task 3 label), and the docs table. `CONTROLS_HINT`/`HELP_TOPICS` (Task 18) are consumed by the wrapper, status, help, setup epilogue (Task 30), and the tour. `Glyphs::separator`/`ellipsis`/`focus` and `ui::cursor` (Task 25) are consumed by the renderer (Task 26) and prompt footer (Task 23). `read_terminal_line` (Task 2) is the single raw-mode line reader; `PickerInput` (Task 1) is the single key reader.

## Execution

Work phase by phase on `codex/daily-driver-elite`. Each task is one commit; each phase ends at its exit gate. Run `cargo build --release` before any PTY test because the tests execute the release binary and `harness_identity.require_current_binary` refuses a stale one.
