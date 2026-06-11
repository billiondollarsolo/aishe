#!/usr/bin/env python3
"""Interactive signal/terminal behavior tests for the zsh-PTY front-end (G1).

Drives `aishe zsh` (the real wrapped zsh, over a pseudo-terminal) and exercises
the behaviors the smoke/scenario suites do not: Ctrl-C mid-command, Ctrl-Z job
suspension, window resize (SIGWINCH propagation through aishe's PTY), and
multi-line continuation. The model is never called, so no API key is needed.

A test fails if its expected marker does not appear within the timeout. zsh is
required; the suite skips cleanly when it is absent.

Usage: pty_signals.py [path-to-aishe]   (defaults to target/release/aishe)
"""

import fcntl
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

BINARY = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
TIMEOUT = 8.0


class Pty:
    def __init__(self, argv, env, rows=24, cols=80):
        self.master, slave = pty.openpty()
        self.set_size(rows, cols)
        self.proc = subprocess.Popen(
            argv, stdin=slave, stdout=slave, stderr=slave,
            env=env, preexec_fn=os.setsid, close_fds=True,
        )
        os.close(slave)
        self.transcript = ""

    def set_size(self, rows, cols):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, cols, 0, 0))

    def _drain(self, seconds):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            r, _, _ = select.select([self.master], [], [], 0.2)
            if not r:
                continue
            try:
                chunk = os.read(self.master, 4096)
            except OSError:
                return
            if not chunk:
                return
            self.transcript += chunk.decode("utf-8", "replace")

    def expect(self, needle, timeout=TIMEOUT):
        """Wait until `needle` appears anywhere in output since the last reset."""
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if needle in self.transcript:
                return True
            self._drain(0.2)
        return needle in self.transcript

    def send(self, line):
        os.write(self.master, (line + "\r").encode("utf-8"))

    def raw(self, data):
        os.write(self.master, data)

    def reset(self):
        self.transcript = ""

    def settle(self, seconds=0.6):
        self._drain(seconds)

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


def make_env(binary):
    home = tempfile.mkdtemp(prefix="aishe-sig-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        f.write(
            "[aishe]\n"
            'mode = "suggest"\n'
            'provider = "anthropic"\n'
            'front_end = "zsh-pty"\n'
            "pty_prompt = false\n"   # plain prompt for stable matching
        )
    # Deterministic, minimal zsh: a fixed prompt and a fixed continuation prompt.
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write("PROMPT='ZP> '\nPS2='C> '\n")
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    os.symlink(os.path.abspath(binary), os.path.join(bindir, "aishe"))
    env = dict(os.environ)
    env.update({
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "ZDOTDIR": home,
        "TERM": "xterm-256color",
        "PATH": bindir + ":" + os.environ.get("PATH", ""),
        "ANTHROPIC_API_KEY": "",
        "OPENAI_API_KEY": "",
    })
    return home, env


PASSED = []


def check(sh, name, ok):
    if ok:
        PASSED.append(name)
        sys.stdout.write("  ok   %s\n" % name)
    else:
        sys.stderr.write(
            "\nFAIL: %s\n---- recent output ----\n%s\n-----------------------\n"
            % (name, sh.transcript[-2500:]))
        sh.close()
        sys.exit(1)


def main():
    if shutil.which("zsh") is None:
        sys.stderr.write("SKIP: zsh not on PATH\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home, env = make_env(BINARY)
    sh = Pty([os.path.abspath(BINARY), "zsh"], env, rows=24, cols=80)
    try:
        sh.expect("ZP> ")  # first prompt

        # 1. The wrapped zsh sees the initial terminal width.
        sh.reset()
        sh.send("echo COLS_$COLUMNS")
        check(sh, "initial $COLUMNS is forwarded", sh.expect("COLS_80"))

        # 2. Resizing the terminal propagates through aishe (SIGWINCH) to zsh.
        #    aishe polls the size ~every 200ms, so give it a moment.
        sh.set_size(40, 132)
        time.sleep(0.6)
        sh.reset()
        sh.send("echo COLS_$COLUMNS")
        check(sh, "window resize reaches zsh ($COLUMNS updates)",
              sh.expect("COLS_132"))

        # 3. Ctrl-C interrupts a running command; the shell survives and prompts.
        sh.reset()
        sh.send("sleep 30")
        sh.settle(0.6)          # let sleep start
        sh.raw(b"\x03")         # Ctrl-C
        sh.settle(0.5)
        sh.send("echo ALIVE_$((1 + 1))")
        check(sh, "Ctrl-C interrupts a command, shell survives",
              sh.expect("ALIVE_2"))

        # 4. Ctrl-C on an empty line does not kill the shell.
        sh.reset()
        sh.raw(b"\x03")
        sh.settle(0.3)
        sh.send("echo EMPTYC_$((2 + 3))")
        check(sh, "Ctrl-C on empty prompt is harmless", sh.expect("EMPTYC_5"))

        # 5. Ctrl-Z suspends the foreground job (real job control via the PTY).
        sh.reset()
        sh.send("sleep 30")
        sh.settle(0.6)
        sh.raw(b"\x1a")         # Ctrl-Z
        check(sh, "Ctrl-Z suspends the foreground job", sh.expect("suspended"))
        sh.send("kill %1")      # clean up the stopped job
        sh.settle(0.4)
        sh.send("echo AFTERZ_$((3 + 4))")
        check(sh, "shell continues after suspend+kill", sh.expect("AFTERZ_7"))

        # 6. Multi-line continuation: a control structure typed across lines runs
        #    (the accept-line wrapper must not break zsh's continuation).
        sh.reset()
        sh.send("for i in A B C; do")
        sh.settle(0.4)
        sh.send("echo LINE_$i")
        sh.settle(0.3)
        sh.send("done")
        check(sh, "multi-line for-loop continues and runs",
              sh.expect("LINE_A") and sh.expect("LINE_B") and sh.expect("LINE_C"))

        sh.send("exit")
        sh.settle(0.5)
        sys.stdout.write("\nAll %d signal/terminal cases passed.\n" % len(PASSED))
    finally:
        sh.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
