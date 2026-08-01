#!/usr/bin/env python3
"""PTY acceptance for long agent questions, reconnect, and resize stability.

Build the fixture first with:
  cargo test --test agent_ui_acceptance --no-run

The test intentionally drives the backend-neutral renderer fixture rather than
requiring a downloaded managed runtime. Ctrl-C/cancel command staging remains
covered by pty_scenarios.py; fail-closed question input is covered in Rust.
"""

import fcntl
import glob
import os
import pty
import select
import signal
import stat
import struct
import subprocess
import sys
import termios
import time


def fixture_path():
    if len(sys.argv) == 2:
        return os.path.abspath(sys.argv[1])
    candidates = []
    for path in glob.glob("target/debug/deps/agent_ui_acceptance-*"):
        if path.endswith((".d", ".rlib", ".rmeta")):
            continue
        try:
            if stat.S_ISREG(os.stat(path).st_mode) and os.access(path, os.X_OK):
                candidates.append(path)
        except OSError:
            pass
    if not candidates:
        raise SystemExit(
            "fixture missing; run cargo test --test agent_ui_acceptance --no-run"
        )
    return os.path.abspath(max(candidates, key=os.path.getmtime))


def set_size(fd, rows, columns):
    fcntl.ioctl(
        fd,
        termios.TIOCSWINSZ,
        struct.pack("HHHH", rows, columns, 0, 0),
    )


def read_until(fd, transcript, needle, timeout):
    deadline = time.monotonic() + timeout
    while needle not in transcript[0] and time.monotonic() < deadline:
        ready, _, _ = select.select([fd], [], [], 0.1)
        if not ready:
            continue
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            break
        if not chunk:
            break
        transcript[0] += chunk.decode("utf-8", "replace")
    return needle in transcript[0]


def main():
    fixture = fixture_path()
    master, slave = pty.openpty()
    set_size(slave, 24, 40)
    env = os.environ.copy()
    env.update(
        {
            "AISHE_AGENT_RENDERER_FIXTURE": "questions",
            "AISHE_AGENT_UI_PAUSE_MS": "600",
            "NO_COLOR": "1",
            "TERM": "dumb",
            "LC_ALL": "C",
        }
    )
    process = subprocess.Popen(
        [
            fixture,
            "--exact",
            "renderer_fixture_child",
            "--nocapture",
            "--test-threads=1",
        ],
        stdin=slave,
        stdout=slave,
        stderr=slave,
        env=env,
        preexec_fn=os.setsid,
        close_fds=True,
    )
    os.close(slave)
    transcript = [""]
    try:
        if not read_until(master, transcript, "question-0", 8):
            raise AssertionError("first long question did not render")
        set_size(master, 40, 120)
        os.killpg(os.getpgid(process.pid), signal.SIGWINCH)
        if not read_until(master, transcript, "question-1", 8):
            raise AssertionError("second question did not render after resize/reconnect")
        read_until(master, transcript, "test result: ok", 8)
        code = process.wait(timeout=8)
        rendered = transcript[0]
        if code != 0:
            raise AssertionError(f"renderer fixture exited {code}:\n{rendered}")
        if "\x1b" in rendered:
            raise AssertionError("NO_COLOR/TERM=dumb PTY emitted an ESC byte")
        if rendered.count("waiting for you: agent") != 2:
            raise AssertionError(f"question panels duplicated or disappeared:\n{rendered}")
        for marker in ("question-0", "question-1"):
            if rendered.count(marker) != 1:
                raise AssertionError(f"{marker} duplicated after resize:\n{rendered}")
        for marker in ("planner", "task-acceptance", "phase: recovering"):
            if marker not in rendered:
                raise AssertionError(f"missing {marker!r}:\n{rendered}")
        if len(rendered) >= 18_000:
            raise AssertionError("long question transcript exceeded its bounded budget")
        print("PASS: long/multiple questions, no-color, reconnect, resize, no duplicates")
    finally:
        try:
            os.close(master)
        except OSError:
            pass
        if process.poll() is None:
            try:
                os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)


if __name__ == "__main__":
    main()
