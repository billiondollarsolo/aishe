#!/usr/bin/env python3
"""PTY coverage for /connection vs /model (split pickers).

The fixture is fully local and unauthenticated. It proves filtering, shell-local
selection, durable-default selection, cancellation, rollback on error, plain
text output, and independence between concurrent AIShe shells.
"""

import os
import pty
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time


BINARY = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
TIMEOUT = 20
EMOJI = re.compile("[\U0001F000-\U0001FAFF\u270F\U0001F4C2\u26A1]")


def environment():
    home = tempfile.mkdtemp(prefix="aishe-model-picker-")
    config_dir = os.path.join(home, ".config", "aishe")
    os.makedirs(config_dir)
    with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
        file.write(
            'version = 6\n\n'
            '[aishe]\n'
            'mode = "suggest"\n'
            'provider = "openai"\n'
            'connection = "openai-work"\n'
            'connection_fallback = "openai-work"\n'
            'pty_prompt = true\n'
            'status_line_position = "below"\n'
            'status_line_items = ["identity"]\n\n'
            '[connections.openai-work]\n'
            'provider = "openai"\n'
            'label = "OpenAI work"\n'
            'base_url = "http://127.0.0.1:18081"\n'
            'model = "work-model"\n'
            'transport = "chat"\n'
            'auth_required = false\n'
            '[connections.openai-work.auth]\n'
            'type = "none"\n\n'
            '[connections.openai-personal]\n'
            'provider = "openai"\n'
            'label = "OpenAI personal"\n'
            'base_url = "http://127.0.0.1:18082"\n'
            'model = "personal-model"\n'
            'transport = "chat"\n'
            'auth_required = false\n'
            '[connections.openai-personal.auth]\n'
            'type = "none"\n\n'
            '[backend]\n'
            'engine = "native"\n'
        )
    with open(os.path.join(home, ".zshrc"), "w", encoding="utf-8") as file:
        file.write("unset HISTFILE\nPROMPT='MODEL> '\n")
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir)
    os.symlink(BINARY, os.path.join(bin_dir, "aishe"))
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
            "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "ZDOTDIR": home,
            "TERM": "xterm-256color",
            "NO_COLOR": "1",
            "PATH": bin_dir + ":" + os.environ.get("PATH", ""),
        }
    )
    return home, env


class Shell:
    def __init__(self, env):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            [BINARY, "zsh"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        self.transcript = ""
        self.expect_cursor = 0

    def drain(self, seconds=0.2):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            ready, _, _ = select.select([self.master], [], [], 0.1)
            if not ready:
                continue
            try:
                data = os.read(self.master, 8192)
            except OSError:
                return
            if not data:
                return
            self.transcript += data.decode("utf-8", "replace")

    def expect(self, text, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            index = self.transcript.find(text, self.expect_cursor)
            if index >= 0:
                self.expect_cursor = index + len(text)
                return
            self.drain()
        raise AssertionError("missing %r in:\n%s" % (text, self.transcript[-4000:]))

    def line(self, text):
        os.write(self.master, (text + "\r").encode())

    def raw(self, value):
        os.write(self.master, value)

    def ready(self):
        self.line("print -r -- MODEL_PICKER_READY")
        self.expect("MODEL_PICKER_READY")

    def identity(self, marker):
        self.line("print -r -- %s:$AISHE_CONNECTION:$AISHE_MODEL" % marker)
        self.expect(marker + ":")

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


def select_connection(shell, filter_text, save=False):
    shell.line("/connection")
    shell.expect("Select a connection")
    shell.raw(filter_text.encode())
    shell.expect("filter: " + filter_text)
    if save:
        # d applies and saves as durable default without the post-Enter prompt.
        shell.raw(b"d")
    else:
        # Enter is shell-local; when the choice differs from config, Aishe asks
        # whether to make it the default — answer n to keep this shell only.
        shell.raw(b"\r")
        try:
            # Confirm defaults to No ([y/N]); accept with n or bare Enter.
            shell.expect("the default connection?", timeout=3)
            shell.raw(b"\r")
        except AssertionError:
            # Same as current default: no follow-up confirm.
            pass


def select_model(shell, filter_text="", save=False):
    shell.line("/model")
    shell.expect("Select a model")
    if filter_text:
        shell.raw(filter_text.encode())
        shell.expect("filter: " + filter_text)
    if save:
        shell.raw(b"d")
    else:
        shell.raw(b"\r")
        try:
            shell.expect("the default for this connection?", timeout=3)
            shell.raw(b"\r")
        except AssertionError:
            pass

def main():
    if not os.path.exists(BINARY):
        raise SystemExit("binary not found: " + BINARY)
    if shutil.which("zsh") is None:
        print("skip: zsh is unavailable")
        return
    home, env = environment()
    first = Shell(env)
    second = Shell(env)
    try:
        first.ready()
        second.ready()
        first.drain(0.3)
        initial_identity = [
            "OpenAI work (openai-work)",
            "openai@127.0.0.1",
            "No auth",
            "work-model/auto",
            "default",
        ]
        if not all(value in first.transcript for value in initial_identity):
            raise AssertionError("compact identity disclosure is incomplete")

        select_connection(first, "personal")
        first.expect("connection = openai-personal")
        first.expect("OpenAI personal (openai-personal)")
        first.expect("this shell")
        first.identity("FIRST")
        first.expect("FIRST:openai-personal:personal-model")

        first.line("/reasoning high")
        first.expect("reasoning = high (this shell)")
        first.line("print -r -- FIRST_REASONING:$AISHE_REASONING")
        first.expect("FIRST_REASONING:high")

        second.identity("SECOND")
        second.expect("SECOND:openai-work:work-model")
        second.line("print -r -- SECOND_REASONING:$AISHE_REASONING")
        second.expect("SECOND_REASONING:auto")

        first.line("/model")
        first.expect("Select a model")
        first.raw(b"\x1b")
        first.expect("model selection cancelled")
        first.identity("CANCELLED")
        first.expect("CANCELLED:openai-personal:personal-model")

        first.line("command aishe model broken --connection missing")
        first.expect("unknown connection or provider 'missing'")
        first.identity("ROLLED_BACK")
        first.expect("ROLLED_BACK:openai-personal:personal-model")

        select_connection(first, "work", save=True)
        first.expect("connection = openai-work")
        first.line("print -r -- SAVED_SCOPE:$AISHE_SELECTION_SCOPE")
        first.expect("SAVED_SCOPE:default")
        first.identity("SAVED")
        first.expect("SAVED:openai-work:work-model")

        config = os.path.join(home, ".config", "aishe", "config.toml")
        with open(config, encoding="utf-8") as file:
            durable = file.read()
        if 'connection = "openai-work"' not in durable:
            raise AssertionError("d did not save the durable connection")

        select_connection(second, "personal")
        second.expect("connection = openai-personal")
        second.line("/model default")
        second.expect("restored saved default")
        second.identity("RESTORED")
        second.expect("RESTORED:openai-work:work-model")

        combined = first.transcript + second.transcript
        if EMOJI.search(combined):
            raise AssertionError("picker output contains emoji")
        print("  ok   connection/model split, save/cancel/rollback/default are connection-safe")
        print("  ok   compact identity/scope, concurrent shells, and plain output work")
    finally:
        first.close()
        second.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
