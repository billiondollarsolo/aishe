#!/usr/bin/env python3
"""Interactive signal/terminal behavior tests for the zsh-PTY front-end (G1).

Drives `aishe zsh` (the real wrapped zsh, over a pseudo-terminal) and exercises
the behaviors the smoke/scenario suites do not: Ctrl-C mid-command, Ctrl-Z job
suspension, window resize (SIGWINCH propagation through aishe's PTY), and
multi-line continuation. The model is never called, so no API key is needed.

A test fails if its expected marker does not appear within the timeout. zsh is
required; the suite skips cleanly when it is absent.

Usage: pty_signals.py [path-to-aishe] --profile clean|personal|legacy
"""

import fcntl
import argparse
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

from harness_identity import require_current_binary

ARGUMENTS = argparse.ArgumentParser(description=__doc__)
ARGUMENTS.add_argument("binary", nargs="?", default="target/release/aishe")
ARGUMENTS.add_argument("--profile", choices=("clean", "personal", "legacy"), default="clean")
ARGS = ARGUMENTS.parse_args()
BINARY = require_current_binary(ARGS.binary)
TIMEOUT = 8.0


class Pty:
    def __init__(self, argv, env, rows=24, cols=80):
        self.master, self.slave = pty.openpty()
        self.initial_termios = termios.tcgetattr(self.slave)
        self.set_size(rows, cols)
        self.proc = subprocess.Popen(
            argv, stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env=env,
            preexec_fn=lambda: (os.setsid(), fcntl.ioctl(0, termios.TIOCSCTTY, 0)),
            close_fds=True,
        )
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

    def wait_ready(self, timeout=20):
        """Block until zsh's line editor is really accepting input.

        The prompt appearing is not enough: ZLE enables bracketed paste after
        printing it, and input typed in that window arrives mangled (`echo` as
        `ccho`) on a slow runner, which then reads as a shell-wrapper bug. Send
        a marker through a full round trip first.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.send("print -r -- PTY_READY_''MARKER")
            if self.expect("PTY_READY_MARKER", timeout=2):
                return True
        return False

    def raw(self, data):
        os.write(self.master, data)

    def reset(self):
        self.transcript = ""

    def settle(self, seconds=0.6):
        self._drain(seconds)

    def close(self):
        try:
            os.close(self.slave)
        except OSError:
            pass
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
            '\n[backend]\n'
            'engine = "native"\n'
        )
    # Deterministic, minimal zsh: a fixed prompt and a fixed continuation prompt.
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write("PROMPT='ZP> '\nPS2='C> '\n"
                "export ZSH_PROFILE_CANARY=loaded\n")
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    os.symlink(os.path.abspath(binary), os.path.join(bindir, "aishe"))
    env = dict(os.environ)
    for name in list(env):
        if name.startswith("AISHE_"):
            env.pop(name)
    env.update({
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        # macOS ignores XDG_*; these are honored on every platform.
        "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
        "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "ZDOTDIR": home,
        # GitHub runners ship group-writable zsh completion dirs, so compinit
        # stops with an interactive "insecure directories" prompt that swallows
        # a keystroke and desynchronises every later expect().
        "ZSH_DISABLE_COMPFIX": "true",
        "TERM": "xterm-256color",
        "PATH": bindir + ":" + os.environ.get("PATH", ""),
        "ANTHROPIC_API_KEY": "",
        "OPENAI_API_KEY": "",
        "AISHE_LEAN": "0" if ARGS.profile == "legacy" else "1",
        "AISHE_LEGACY_OPENCODE": "1" if ARGS.profile == "legacy" else "0",
        "AISHE_ZSH_PROFILE": "personal" if ARGS.profile == "personal" else "clean",
        "AISHE_SPY_PROVIDER_MAKE": os.path.join(home, "provider-started"),
        "AISHE_SPY_OPENCODE": os.path.join(home, "opencode-started"),
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
        sys.stderr.write("FAIL: zsh not on PATH\n")
        sys.exit(1)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home, env = make_env(BINARY)
    sh = Pty([os.path.abspath(BINARY), "zsh"], env, rows=24, cols=80)
    try:
        check(sh, "zsh line editor became ready", sh.wait_ready())
        sh.send("print -r -- PROFILE_CANARY_''VALUE=${ZSH_PROFILE_CANARY:-absent}")
        canary = "absent" if ARGS.profile == "clean" else "loaded"
        check(sh, "startup profile is explicit", sh.expect("PROFILE_CANARY_VALUE=" + canary))

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

        # 5. Job control uses the real controlling terminal. Resume the stopped
        # job in the background, foreground it again, and interrupt it.
        sh.reset()
        sh.send("sleep 30")
        sh.settle(0.6)
        sh.raw(b"\x1a")         # Ctrl-Z
        check(sh, "Ctrl-Z suspends the foreground job", sh.expect("suspended"))
        sh.send('bg %+; jobs -r > "$HOME/jobs-running"; print -r -- BG_\'\'RUNNING')
        check(sh, "bg resumes the stopped job", sh.expect("BG_RUNNING"))
        with open(os.path.join(home, "jobs-running")) as file:
            running = file.read()
        check(sh, "jobs reports a running background process", "sleep 30" in running)
        sh.send("fg %+")
        sh.settle(0.4)
        sh.raw(b"\x03")
        sh.settle(0.4)
        sh.send("echo AFTERZ_$((3 + 4))")
        check(sh, "fg returns job ownership and Ctrl-C restores the prompt", sh.expect("AFTERZ_7"))

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

        # Bracketed multiline paste edits a buffer until Enter. Files verify
        # execution rather than accepting text that might just be terminal echo.
        paste_file = os.path.join(home, "paste-output")
        sh.reset()
        sh.send("bindkey -e")
        sh.settle(0.2)
        paste = ('print -r -- PASTE_ALPHA > "$HOME/paste-output"\n'
                 'print -r -- PASTE_BETA >> "$HOME/paste-output"')
        sh.raw(b"\x1b[200~" + paste.encode() + b"\x1b[201~")
        sh.settle(0.3)
        check(sh, "bracketed paste does not execute before Enter", not os.path.exists(paste_file))
        sh.raw(b"\r")
        sh.settle(0.4)
        with open(paste_file) as file:
            pasted = file.read()
        check(sh, "multiline paste executes exactly once after Enter",
              pasted == "PASTE_ALPHA\nPASTE_BETA\n")

        sh.reset()
        sh.raw(b"print -r -- EMACS_OX\x7fK > \"$HOME/emacs-output\"\r")
        sh.settle(0.3)
        with open(os.path.join(home, "emacs-output")) as file:
            edited = file.read()
        check(sh, "emacs editing executes the edited buffer", edited == "EMACS_OK\n")

        sh.send("bindkey -v")
        sh.settle(0.2)
        sh.raw(b"print -r -- VI_OX\x1b")
        sh.settle(0.8)
        sh.raw(b"xaK > \"$HOME/vi-output\"\r")
        sh.settle(0.3)
        with open(os.path.join(home, "vi-output")) as file:
            edited = file.read()
        check(sh, "vi command and insert modes execute the edited buffer", edited == "VI_OK\n")

        for name in ("provider-started", "opencode-started"):
            check(sh, "ordinary shell actions never start " + name,
                  not os.path.exists(os.path.join(home, name)))

        sh.send("exit")
        # Keep consuming the terminal while the relay flushes its final output.
        # Waiting without a reader can hold the child behind a full PTY buffer.
        deadline = time.monotonic() + 5
        while sh.proc.poll() is None and time.monotonic() < deadline:
            sh.settle(0.1)
        if sh.proc.poll() is None:
            raise AssertionError("shell did not exit within 5 seconds:\n" + sh.transcript[-4000:])
        check(sh, "shell exit succeeds", sh.proc.returncode == 0)
        check(sh, "shell restores every outer terminal attribute",
              termios.tcgetattr(sh.slave) == sh.initial_termios)
        sys.stdout.write("\nAll %d signal/terminal cases passed.\n" % len(PASSED))
    finally:
        sh.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
