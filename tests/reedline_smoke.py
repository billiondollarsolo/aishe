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
        )
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

        # 3) clean exit.
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
