#!/usr/bin/env python3
"""PTY smoke test for the aishe zsh front-end.

Drives `aishe zsh` through a real pseudo-terminal and asserts that the wrapper
launches the user's genuine interactive zsh with the aishe integration injected.
It verifies, *without needing an API key or network*:

  1. native commands run through the wrapped zsh (the PTY proxy works),
  2. the `command_not_found_handler` AI hook is installed,
  3. the `auto` eval path is wired into that hook,
  4. the force-NL ZLE widget is defined and bound to a key,
  5. zsh exits cleanly.

Usage:  python3 tests/pty_smoke.py [path/to/aishe]
Default binary: target/release/aishe
Exit code 0 on success, non-zero on the first failed assertion.
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
TIMEOUT = 30.0  # generous: CI shells can be slow to start


class Pty:
    """A spawned process attached to a PTY with simple expect()/send()."""

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
        self.buf = ""  # decoded output not yet consumed by an expect()

    def _drain(self, deadline):
        """Pull whatever is available from the PTY into self.buf (non-blocking)."""
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
        self.buf += chunk.decode("utf-8", "replace")
        return True

    def expect(self, needle, timeout=TIMEOUT):
        """Read until `needle` appears, then consume the buffer through it."""
        deadline = time.monotonic() + timeout
        while True:
            idx = self.buf.find(needle)
            if idx != -1:
                self.buf = self.buf[idx + len(needle):]
                return True
            if not self._drain(deadline):
                return False

    def send(self, line):
        os.write(self.master, (line + "\n").encode("utf-8"))

    def wait_ready(self, timeout=TIMEOUT):
        """Block until zsh's line editor is actually accepting input.

        Typing immediately after spawn races ZLE startup: on a slow runner the
        first characters are swallowed or doubled (`echo` arriving as `ccho`),
        zsh then reports `command not found`, and the failure looks like a bug
        in the shell wrapper rather than the harness. Send a marker through a
        round trip and wait for its *output*, so the editor has demonstrably
        processed a full line before the real assertions start.
        """
        marker = "PTY_READY_OUTPUT"
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            # Keep the expected output distinct from the command text. ZLE can
            # interleave prompt-redraw escape sequences with the echoed input
            # (notably when SHARE_HISTORY imports a concurrent shell's entry),
            # so a literal match against the terminal echo is inherently racy.
            self.send("print -r -- %s # PTY_READY_COMMAND" % marker)
            if self.expect(marker, timeout=2):
                return True
        return False

    def wait_exit(self, timeout=10):
        """Wait for the child to exit on its own, draining output. Returns the
        exit code, or None on timeout."""
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                self._drain(time.monotonic() + 0.2)  # flush trailing output
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
    """An isolated HOME/XDG so no real config or wizard interferes."""
    home = tempfile.mkdtemp(prefix="aishe-smoke-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    # Pre-write a config so the first-run wizard never blocks the PTY.
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        f.write("[aishe]\n" 'mode = "suggest"\n' 'provider = "anthropic"\n')
    # Explicitly model a minimal Linux root account. macOS sets HISTFILE from
    # its global /etc/zshrc even with an empty HOME, which would correctly make
    # aishe preserve that system history instead of exercising the fallback.
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write("unset HISTFILE\nHISTSIZE=30\nSAVEHIST=0\n")
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            # macOS ignores XDG_*; these are honored on every platform.
            "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
            "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "ZDOTDIR": home,  # no real .zshrc here -> clean wrapper
            # GitHub runners ship group-writable zsh completion dirs, so compinit
            # stops with an interactive "insecure directories" prompt that swallows
            # a keystroke and desynchronises every later expect().
            "ZSH_DISABLE_COMPFIX": "true",
            "TERM": "xterm-256color",
            # Used indirectly by the history assertions so the probe command
            # itself does not contain (and therefore match) the marker.
            "AISHE_TEST_HISTORY_NEEDLE": "PTY_OK_42",
            "AISHE_TEST_PEER_NEEDLE": "PTY_PEER_42",
            # Make sure no stray key is picked up; the hook must not need one.
            "ANTHROPIC_API_KEY": "",
            "OPENAI_API_KEY": "",
        }
    )
    return home, env


def fail(msg, sh):
    sys.stderr.write("\nFAIL: %s\n" % msg)
    sys.stderr.write("---- recent PTY output ----\n%s\n---------------------------\n" % sh.buf[-2000:])
    sh.close()
    sys.exit(1)


def main():
    if shutil.which("zsh") is None:
        sys.stderr.write("SKIP: zsh not on PATH\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home, env = make_env()
    sh = Pty([os.path.abspath(BINARY), "zsh"], env)
    second = None
    peer = None
    try:
        if not sh.wait_ready():
            fail("wrapped zsh never became ready for input", sh)
        # 1) native command runs through the wrapped zsh. Keep the history
        #    marker in a comment so the independently asserted output cannot
        #    accidentally match a terminal echo of the command.
        sh.send("print -r -- PTY_OUTPUT_42 # PTY_OK_42")
        if not sh.expect("PTY_OUTPUT_42"):
            fail("native command did not run through wrapped zsh", sh)

        # 2) the AI hook is installed.
        sh.send("print -r -- HOOK_IS=${+functions[command_not_found_handler]}")
        if not sh.expect("HOOK_IS=1"):
            fail("command_not_found_handler not installed", sh)

        # 3) the auto eval path is present in the hook. command_not_found_handler
        #    delegates to _aishe_handle_nl, which is where the mode dispatch (and
        #    the `aishe --auto-line` call) lives.
        sh.send("print -r -- AUTO=$(functions _aishe_handle_nl | grep -c -- --auto-line)")
        if not sh.expect("AUTO=1"):
            fail("auto-line eval path missing from hook", sh)

        # 4) the force-NL widget is defined and bound. The widget name appears in
        #    the typed echo for the first probe, so match the value (user:...).
        sh.send("print -r -- WIDGET=${widgets[aishe-nl-widget]}")
        if not sh.expect("user:aishe-nl-widget"):
            fail("force-NL ZLE widget not defined", sh)
        # The keybinding probe's typed text does not contain the widget name, so
        # matching it confirms the binding line in the output.
        sh.send("bindkey '^[^M'")
        if not sh.expect("aishe-nl-widget"):
            fail("force-NL key not bound", sh)

        # 4b) the semantic-recall widget is defined and bound to Ctrl-X Ctrl-R.
        sh.send("print -r -- RECALL=${widgets[aishe-recall]}")
        if not sh.expect("user:aishe-recall"):
            fail("semantic-recall ZLE widget not defined", sh)
        sh.send("bindkey '^X^R'")
        if not sh.expect("aishe-recall"):
            fail("semantic-recall key not bound", sh)

        # 4c) With no user .zshrc/HISTFILE, aishe's persistent log becomes the
        #     native zsh history too. That preserves Up-arrow/Ctrl-R across
        #     sessions and enables concurrent-session sharing.
        sh.send(
            "print -r -- HISTMANAGED=$([[ -n \"$AISHE_MANAGED_HISTFILE\" "
            "&& \"$HISTFILE\" == \"$AISHE_HISTFILE\" ]] && echo 1 || echo 0)"
        )
        if not sh.expect("HISTMANAGED=1"):
            fail("minimal zsh did not adopt AISHE_HISTFILE", sh)
        sh.send(
            "print -r -- HISTOPTS=$([[ -o extendedhistory && -o appendhistory "
            "&& -o sharehistory ]] && echo 1 || echo 0)"
        )
        if not sh.expect("HISTOPTS=1"):
            fail("managed zsh history options are not enabled", sh)
        # Use an environment-held needle: putting PTY_OK_42 literally in this
        # command would make the current history entry match itself.
        sh.send(
            "print -r -- HISTLOGGED=$(grep -c "
            "\"$AISHE_TEST_HISTORY_NEEDLE\" \"$AISHE_HISTFILE\")"
        )
        if not sh.expect("HISTLOGGED=1"):
            fail("interactive command not recorded to AISHE_HISTFILE", sh)

        # 5) Clean exit, then launch a new aishe process against the same HOME.
        #    The prior marker must appear in native `fc` history exactly once:
        #    persisted across processes, with no duplicate from the old manual
        #    preexec writer plus zsh's own history writer.
        sh.send("exit")
        code = sh.wait_exit(timeout=10)
        if code is None:
            fail("zsh did not exit after `exit`", sh)
        if code != 0:
            sys.stderr.write("WARN: zsh exited with code %r\n" % code)

        histfile = os.path.join(home, ".local", "share", "aishe", "history.ext")
        with open(histfile, encoding="utf-8") as f:
            occurrences = f.read().count(env["AISHE_TEST_HISTORY_NEEDLE"])
        if occurrences != 1:
            fail(
                "history marker written %d times instead of once" % occurrences,
                sh,
            )

        second = Pty([os.path.abspath(BINARY), "zsh"], env)
        if not second.wait_ready():
            fail("second wrapped zsh never became ready for input", second)
        second.send(
            "print -r -- HISTRESTORED=$(fc -l 1 | grep -c "
            "\"$AISHE_TEST_HISTORY_NEEDLE\")"
        )
        if not second.expect("HISTRESTORED=1"):
            fail("native history did not survive a new aishe session", second)

        # 6) Start another shell before writing a new marker. SHARE_HISTORY must
        #    make the entry visible to this already-running peer, not merely to
        #    shells launched later.
        peer = Pty([os.path.abspath(BINARY), "zsh"], env)
        if not peer.wait_ready():
            fail("concurrent wrapped zsh never became ready for input", peer)
        second.send("print -r -- PTY_PEER_OUTPUT # PTY_PEER_42")
        if not second.expect("PTY_PEER_OUTPUT"):
            fail("concurrent history marker command did not run", second)
        peer.send(
            "print -r -- PEERSEEN=$(fc -l 1 | grep -c "
            "\"$AISHE_TEST_PEER_NEEDLE\")"
        )
        if not peer.expect("PEERSEEN=1"):
            fail("concurrent aishe session did not import shared history", peer)

        peer.send("exit")
        peer_code = peer.wait_exit(timeout=10)
        if peer_code is None:
            fail("concurrent zsh did not exit after `exit`", peer)
        second.send("exit")
        second_code = second.wait_exit(timeout=10)
        if second_code is None:
            fail("second zsh did not exit after `exit`", second)
        print("PASS: aishe zsh PTY smoke test")
        sys.exit(0)
    finally:
        sh.close()
        if second is not None:
            second.close()
        if peer is not None:
            peer.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
