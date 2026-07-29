#!/usr/bin/env python3
"""Interactive setup/settings/tour PTY coverage."""

import json
import os
import pty
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time

BINARY = os.path.abspath(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
)


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
        self.buffer = ""
        self.transcript = ""

    def drain(self, seconds=0.2):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], 0.1)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 8192)
            except OSError:
                return
            if not chunk:
                return
            text = chunk.decode("utf-8", "replace")
            self.buffer += text
            self.transcript += text

    def expect(self, text, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            position = self.buffer.find(text)
            if position >= 0:
                self.buffer = self.buffer[position + len(text) :]
                return
            self.drain()
        raise AssertionError(
            "did not see %r\n---- transcript ----\n%s"
            % (text, self.transcript[-4000:])
        )

    def send(self, text):
        os.write(self.master, text.encode())

    def line(self, text=""):
        self.send(text + "\r")

    def menu(self, number):
        self.send(str(number) + "\r")

    def finish(self, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                self.drain()
                return self.proc.returncode
            self.drain()
        raise AssertionError("process did not exit\n" + self.transcript[-4000:])

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


def isolated_env(root):
    env = dict(os.environ)
    env.update(
        {
            "HOME": root,
            "AISHE_CONFIG_DIR": os.path.join(root, "config"),
            "AISHE_DATA_DIR": os.path.join(root, "data"),
            "XDG_CONFIG_HOME": os.path.join(root, "config"),
            "XDG_DATA_HOME": os.path.join(root, "data"),
            "TERM": "xterm-256color",
        }
    )
    return env


def complete_setup(root, env):
    shell = Pty([BINARY, "setup"], env)
    try:
        shell.expect("Provider service")
        shell.menu(6)  # Ollama

        shell.expect("API endpoint")
        shell.line("ftp://bad")
        shell.expect("endpoint must use http:// or https://")
        shell.line()  # accept local preset

        shell.expect("Model")
        shell.line(":back")
        # Credential is skipped for local endpoints; back returns to it and then
        # immediately advances to Model again.
        shell.expect("Credential: not required")
        shell.expect("Model")
        shell.line("setup-local-model")

        shell.expect("Safety profile")
        shell.menu(2)  # balanced
        shell.expect("No price is known")
        shell.menu(1)  # enter exact rates
        shell.expect("Input price")
        shell.line("-1")
        shell.expect("price must be a finite non-negative number")
        shell.line("0.125")
        shell.expect("Output price")
        shell.line("0.5")

        shell.expect("Live status-line placement")
        shell.menu(2)  # below
        shell.expect("Status-line contents")
        shell.menu(2)  # detailed
        shell.expect("preview (below)")

        shell.expect("Run live text/structured/tool/streaming checks?")
        shell.line("n")
        shell.expect("Provider check:")
        shell.expect("Review")
        shell.expect("Apply this configuration")
        shell.line("y")
        shell.expect("Setup complete")
        shell.expect("Run the guided first-session tour now")
        shell.line("n")
        shell.expect("Next: run `aishe tour`")
        if shell.finish() != 0:
            raise AssertionError("setup returned nonzero\n" + shell.transcript[-4000:])
    finally:
        shell.close()

    config = os.path.join(root, "config", "aishe", "config.toml")
    text = open(config, encoding="utf-8").read()
    required = [
        'model = "setup-local-model"',
        'safety_profile = "balanced"',
        'status_line_position = "below"',
        "input = 0.125",
        "output = 0.5",
    ]
    for value in required:
        if value not in text:
            raise AssertionError("saved config missing %r\n%s" % (value, text))
    draft = os.path.join(root, "data", "aishe", "setup-draft.json")
    if os.path.exists(draft):
        raise AssertionError("successful setup left a resumable draft")
    print("  ok   full setup flow, invalid input, back, pricing, status, apply")
    return config


def cancel_preserves_active_config(root, env, config):
    before = open(config, "rb").read()
    shell = Pty([BINARY, "setup"], env)
    try:
        shell.expect("Provider service")
        shell.send("\x1b")
        shell.expect("Setup paused")
        if shell.finish() != 0:
            raise AssertionError("cancel returned nonzero")
    finally:
        shell.close()
    after = open(config, "rb").read()
    if before != after:
        raise AssertionError("cancel changed the active config")
    draft = os.path.join(root, "data", "aishe", "setup-draft.json")
    if not os.path.isfile(draft):
        raise AssertionError("cancel did not save a resumable draft")

    resumed = Pty([BINARY, "setup", "--resume"], env)
    try:
        resumed.expect("Provider service")
        resumed.send("\x03")
        resumed.expect("Setup paused")
        if resumed.finish() != 0:
            raise AssertionError("Ctrl-C cancel returned nonzero")
    finally:
        resumed.close()
    if before != open(config, "rb").read():
        raise AssertionError("resumed Ctrl-C changed the active config")

    # Force the saved draft to a later valid state, then prove --restart
    # discards only that draft and begins at the first step.
    with open(draft, encoding="utf-8") as file:
        draft_state = json.load(file)
    draft_state["step"] = "review"
    with open(draft, "w", encoding="utf-8") as file:
        json.dump(draft_state, file)
    restarted = Pty([BINARY, "setup", "--restart"], env)
    try:
        restarted.expect("Provider service")
        restarted.send("\x1b")
        restarted.expect("Setup paused")
        if restarted.finish() != 0:
            raise AssertionError("restarted setup cancel returned nonzero")
    finally:
        restarted.close()
    if before != open(config, "rb").read():
        raise AssertionError("setup --restart changed the active config")
    print("  ok   cancel/Ctrl-C/resume/restart preserve active config")


def settings_are_transactional(root, env, config):
    before = open(config, "rb").read()
    shell = Pty([BINARY, "settings"], env)
    try:
        shell.expect("Choose a section")
        shell.menu(1)  # provider transaction
        shell.expect("Provider service")
        shell.menu(2)  # OpenAI
        shell.expect("Endpoint")
        shell.line(":cancel")
        shell.expect("Choose a section")
        shell.menu(9)  # exit without changes
        if shell.finish() != 0:
            raise AssertionError("settings cancel returned nonzero")
    finally:
        shell.close()
    if before != open(config, "rb").read():
        raise AssertionError("cancelled provider transaction changed config")

    apply = Pty([BINARY, "settings"], env)
    try:
        apply.expect("Choose a section")
        apply.menu(2)  # shell/history/status
        apply.expect("Shell, history & statusline")
        apply.menu(4)  # hook timeout
        apply.expect("AI hook timeout seconds")
        apply.line("75")
        apply.expect("Shell, history & statusline")
        apply.menu(5)  # placement
        apply.expect("Statusline placement")
        apply.menu(1)  # right
        apply.expect("preview (right)")
        apply.expect("Shell, history & statusline")
        apply.menu(8)  # back
        apply.expect("Choose a section")
        apply.menu(8)  # review/apply
        apply.expect("Apply these settings")
        apply.line("y")
        apply.expect("saved:")
        if apply.finish() != 0:
            raise AssertionError("settings apply returned nonzero")
    finally:
        apply.close()
    text = open(config, encoding="utf-8").read()
    if 'status_line_position = "right"' not in text:
        raise AssertionError("settings did not apply selected placement")
    if "hook_timeout_secs = 75" not in text:
        raise AssertionError("settings did not apply selected hook timeout")
    print("  ok   settings provider cancel is transactional; reviewed apply works")


def tour_pause_resume_skip_restart_and_complete(root, env):
    paused = Pty([BINARY, "tour"], env)
    try:
        paused.expect("1. Normal shell commands")
        paused.expect("Lesson 1 of 7")
        paused.menu(3)  # exit and resume later
        paused.expect("Tour paused")
        if paused.finish() != 0:
            raise AssertionError("tour pause returned nonzero")
    finally:
        paused.close()

    resumed = Pty([BINARY, "tour"], env)
    try:
        resumed.expect("1. Normal shell commands")
        resumed.expect("Lesson 1 of 7")
        resumed.menu(2)  # skip lesson one
        resumed.expect("2. Natural-language routing")
        resumed.expect("not verified; lesson stays offline")
        resumed.expect("Lesson 2 of 7")
        resumed.menu(3)  # pause on lesson two
        resumed.expect("Tour paused")
        if resumed.finish() != 0:
            raise AssertionError("resumed tour pause returned nonzero")
    finally:
        resumed.close()

    restarted = Pty([BINARY, "tour", "--restart"], env)
    lessons = [
        "1. Normal shell commands",
        "2. Natural-language routing",
        "3. Suggest mode",
        "4. Recover from failures",
        "5. File change and undo",
        "6. Modes and safety",
        "7. Your persistent state",
    ]
    try:
        for index, lesson in enumerate(lessons, start=1):
            restarted.expect(lesson)
            restarted.expect("Lesson %d of 7" % index)
            restarted.menu(1)
        restarted.expect("Tour complete")
        if restarted.finish() != 0:
            raise AssertionError("restarted tour returned nonzero")
    finally:
        restarted.close()

    state_path = os.path.join(root, "data", "aishe", "tour", "state.json")
    with open(state_path, encoding="utf-8") as file:
        state = json.load(file)
    if not state.get("completed") or state.get("next_lesson") != 7:
        raise AssertionError("tour did not persist completed state: %r" % state)
    undo_demo = os.path.join(root, "data", "aishe", "tour", "workspace", "undo-demo.txt")
    if os.path.exists(undo_demo):
        raise AssertionError("tour undo lesson left its demo file behind")
    print("  ok   tour pause/resume/skip/restart/offline/undo flow")


def main():
    if not os.path.exists(BINARY):
        raise SystemExit("FAIL: binary not found: " + BINARY)
    root = tempfile.mkdtemp(prefix="aishe-setup-pty-")
    try:
        env = isolated_env(root)
        config = complete_setup(root, env)
        cancel_preserves_active_config(root, env, config)
        settings_are_transactional(root, env, config)
        tour_pause_resume_skip_restart_and_complete(root, env)
        print("PASS: interactive setup/settings/tour PTY")
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
