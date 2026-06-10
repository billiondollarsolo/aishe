#!/usr/bin/env python3
"""PTY smoke test for the reedline front-end.

Drives the built-in reedline editor (forced via `front_end = "reedline"`) through
a pseudo-terminal and asserts the interactive editing path works end-to-end —
without needing an API key or network:

  1. a shell command typed at the prompt runs and prints its output,
  2. the multi-line validator holds an unterminated shell line on a continuation
     line (the `·` indicator) instead of submitting, then submits once complete,
  3. the editor exits cleanly on `exit`.

Unlike the zsh-PTY harness, reedline queries the terminal for the cursor
position (`ESC[6n` DSR) at startup, so this harness answers that query.

Usage:  python3 tests/reedline_smoke.py [path/to/aishe]
Default binary: target/release/aishe
"""

import os
import pty
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time

BINARY = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
TIMEOUT = 30.0


class Pty:
    def __init__(self, argv, env):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            argv,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        self.buf = ""

    def _drain(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        r, _, _ = select.select([self.master], [], [], min(remaining, 0.2))
        if not r:
            return True
        try:
            chunk = os.read(self.master, 4096)
        except OSError:
            return False
        if not chunk:
            return False
        text = chunk.decode("utf-8", "replace")
        self.buf += text
        # Answer the cursor-position (DSR) query so reedline can start.
        if "\x1b[6n" in text:
            try:
                os.write(self.master, b"\x1b[1;1R")
            except OSError:
                pass
        return True

    def expect(self, needle, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            idx = self.buf.find(needle)
            if idx != -1:
                self.buf = self.buf[idx + len(needle):]
                return True
            if not self._drain(deadline):
                return False

    def send(self, line):
        os.write(self.master, (line + "\r").encode("utf-8"))

    def settle(self, seconds=2.0):
        """Drain output for a fixed window so a running command finishes and the
        editor is idle at the prompt before the next (dependent) command — a
        human waits for the prompt; type-ahead during a running child can be
        consumed by that child rather than the editor."""
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self._drain(time.monotonic() + 0.2)

    def wait_exit(self, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                self._drain(time.monotonic() + 0.2)
                return self.proc.returncode
            self._drain(time.monotonic() + 0.2)
        return None

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
            return self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            return None


def make_env():
    home = tempfile.mkdtemp(prefix="aishe-reedline-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        # Force reedline, and turn off the right prompt so the only `·` in the
        # output is the multi-line continuation indicator.
        f.write(
            "[aishe]\n"
            'mode = "suggest"\n'
            'front_end = "reedline"\n'
            "show_right_prompt = false\n"
            "correct = true\n"  # exercise zsh CORRECT (REPL-only, no key needed)
            "auto_pushd = true\n"
            'prompt_format = "[{mode}] {cwd}"\n'
        )
    # An .aishrc whose alias should be sourced into delegated commands. The body
    # uses arithmetic so the result (AISHRC_42) appears only in command output,
    # never in the (un-evaluated) alias definition text.
    with open(os.path.join(cfgdir, "aishrc"), "w") as f:
        f.write("alias greet='echo AISHRC_$((6 * 7))'\n")
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "TERM": "xterm-256color",
            "ANTHROPIC_API_KEY": "",
            "OPENAI_API_KEY": "",
        }
    )
    return home, env


def fail(msg, sh):
    sys.stderr.write("\nFAIL: %s\n" % msg)
    sys.stderr.write(
        "---- recent PTY output ----\n%s\n---------------------------\n" % sh.buf[-2000:]
    )
    sh.close()
    sys.exit(1)


def main():
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home, env = make_env()
    sh = Pty([os.path.abspath(BINARY), "--no-pty"], env)
    try:
        # 0) the custom prompt_format ("[{mode}] {cwd}") is rendered.
        if not sh.expect("[suggest]"):
            fail("custom prompt_format was not rendered", sh)

        # 1) a shell command runs through the reedline editor.
        sh.send("echo REEDLINE_OK_$((6 * 7))")
        if not sh.expect("REEDLINE_OK_42"):
            fail("shell command did not run in the reedline editor", sh)

        # 2) the multi-line validator: a trailing backslash holds the line on a
        #    continuation (the `·` indicator) rather than submitting.
        sh.send("echo AAA \\")
        if not sh.expect("·"):
            fail("validator did not continue an unterminated line", sh)
        sh.send("BBB")  # complete the line; now it should submit and run.
        if not sh.expect("AAA BBB"):
            fail("continued line did not submit/run once complete", sh)

        # 3) .aishrc is sourced into delegated commands. A synchronous `rehash`
        #    first guarantees the alias is in the command cache (so `greet` is
        #    recognized as a command rather than routed to the LLM).
        sh.send("aishe rehash")
        if not sh.expect("rehashed"):
            fail("aishe rehash did not run", sh)
        sh.send("greet")
        if not sh.expect("AISHRC_42"):
            fail(".aishrc alias was not sourced into delegated commands", sh)

        # 4) an interactively-defined alias persists to the next command. The
        #    arithmetic result (REPLAY_42) appears only when `foo` actually runs
        #    the alias — not in the echoed definition text. A settle command in
        #    between ensures the alias command has finished and the editor is back
        #    at the prompt before `foo` is sent (otherwise type-ahead can be
        #    consumed by the still-running child shell).
        sh.send("alias foo='echo REPLAY_$((6 * 7))'")
        sh.settle(2.0)  # let the alias command finish; editor idle at the prompt
        sh.send("foo")
        if not sh.expect("REPLAY_42"):
            fail("interactively-defined alias did not persist across commands", sh)

        # 4b) the theme meta command accepts a known preset.
        sh.send("aishe theme nord")
        if not sh.expect("theme → nord"):
            fail("aishe theme command did not set the preset", sh)

        # 4c) history expansion: `!$` becomes the previous command's last word.
        #     "got NUMBER_42" only appears if !$ expanded to NUMBER_42.
        sh.send("echo NUMBER_42")
        sh.settle(1.5)
        sh.send("echo got !$")
        if not sh.expect("got NUMBER_42"):
            fail("history expansion (!$) did not work", sh)

        # 4d) a multi-line function definition persists and is callable. The
        #     body's arithmetic result (FUNC_42) only appears when greet2 runs.
        sh.send("greet2() {")
        sh.send("echo FUNC_$((6 * 7))")
        sh.send("}")
        sh.settle(2.0)
        sh.send("greet2")
        if not sh.expect("FUNC_42"):
            fail("interactively-defined function did not persist/run", sh)

        # 4e) a multi-line for-loop continues across lines and runs. LOOP_3 only
        #     appears when the loop body executes.
        sh.send("for n in 1 2 3; do")
        sh.send("echo LOOP_$n")
        sh.send("done")
        if not sh.expect("LOOP_3"):
            fail("multi-line control structure did not run", sh)

        # 4f) zsh AUTO_PUSHD: cd pushes the previous dir; `dirs -v` numbers it.
        sh.send("cd /tmp")
        sh.settle(1.0)
        sh.send("dirs -v")
        if not sh.expect("1\t"):
            fail("auto_pushd / dirs -v did not show a numbered stack", sh)

        # 4g) spelling correction (CORRECT): a mistyped command word is offered as
        #     a correction; accepting it runs the corrected command. "ehco" is a
        #     transposition of "echo"; CORRECT_42 only appears once echo runs.
        sh.send("ehco CORRECT_$((6 * 7))")
        # The prompt text is colorized, so match the plain `'ehco'` and `[Y/n]`
        # markers rather than the whole sentence.
        if not sh.expect("'ehco'") or not sh.expect("[Y/n]"):
            fail("spelling correction was not offered for a mistyped command", sh)
        sh.send("y")  # accept the correction
        if not sh.expect("CORRECT_42"):
            fail("accepting the correction did not run the corrected command", sh)

        # 5) clean exit.
        sh.send("exit")
        code = sh.wait_exit(timeout=10)
        if code is None:
            fail("reedline editor did not exit after `exit`", sh)
        if code != 0:
            sys.stderr.write("WARN: aishe exited with code %r\n" % code)
        print("PASS: aishe reedline PTY smoke test")
        sys.exit(0)
    finally:
        sh.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
