#!/usr/bin/env python3
"""Shared PTY driver for the in-shell regression tests.

Each test used to carry its own copy of this; one module keeps the harness
honest when the prompt or hook changes.
"""

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

CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")


def binary():
    return require_current_binary(
        os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
    )


class Pty:
    def __init__(self, env, cols=100, rows=30, argv=None):
        self.master, slave = pty.openpty()
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
        self.proc = subprocess.Popen(
            argv or [binary(), "zsh"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
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
        self.send("print -r -- PTY_''READY\r")
        return self.expect("PTY_READY")

    def resize(self, cols, rows=30):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

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


def environment(label, zshrc="unset HISTFILE\n", mode="auto", extra=None, config_extra=""):
    """A private HOME with a usable config and `aishe` on PATH."""
    home = tempfile.mkdtemp(prefix="aishe-%s-" % label)
    config_dir = os.path.join(home, ".config", "aishe")
    os.makedirs(config_dir)
    with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
        file.write(
            "version = 2\n"
            "[aishe]\n"
            'mode = "%s"\n'
            'provider = "anthropic"\n'
            "pty_prompt = true\n"
            "%s\n"
            "[providers.anthropic]\n"
            'base_url = "https://api.anthropic.com"\n'
            'api_key_env = "UNUSED_FAKE_KEY"\n'
            'model = "menu-model"\n\n'
            "[backend]\n"
            'engine = "native"\n' % (mode, config_extra)
        )
    with open(os.path.join(home, ".zshrc"), "w", encoding="utf-8") as file:
        file.write(zshrc)
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir)
    os.symlink(binary(), os.path.join(bin_dir, "aishe"))
    env = dict(os.environ)
    env.pop("NO_COLOR", None)
    env.pop("AISHE_SHELL_ID", None)
    env.update(
        {
            "HOME": home,
            "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
            "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "ZDOTDIR": home,
            "ZSH_DISABLE_COMPFIX": "true",
            "TERM": "xterm-256color",
            "PATH": bin_dir + ":" + os.environ.get("PATH", ""),
        }
    )
    env.update(extra or {})
    return home, env
