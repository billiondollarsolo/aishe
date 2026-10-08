#!/usr/bin/env python3
"""Lean Ctrl-C must cancel work and preserve FIFO request/reply alignment."""

from pathlib import Path
import shutil
import tempfile
import time

from pty_helper import Pty, environment


def wait_new(shell, text, start, timeout=5):
    deadline = time.monotonic() + timeout
    while text not in shell.plain()[start:] and time.monotonic() < deadline:
        shell.drain(.1)
    assert text in shell.plain()[start:], shell.plain()[-3000:]


def check_reply_alignment(shell):
    start = len(shell.plain())
    shell.send("print -r -- CANCEL_PROBE_''SYNC=$(_aishe_lean_send "
               "$'SLASH\\tagent\\t'\"$PWD\"$'\\t/status')\r")
    wait_new(shell, "CANCEL_PROBE_SYNC=OK", start)
    assert "CANCEL_PROBE_SYNC=RAN" not in shell.plain()[start:], "stale cancelled reply"


def fixture(label, extra):
    home, env = environment(label, mode="ask", extra={
        "AISHE_LEAN": "1", "AISHE_LEGACY_OPENCODE": "0",
        "AISHE_FAKE_LLM": "CANCELLED_ANSWER_MUST_NOT_APPEAR",
        "NO_COLOR": "1", "AISHE_UNICODE": "ascii", **extra,
    })
    env.pop("AISHE_FAKE_LLM_FILE", None)
    config = Path(home) / ".config/aishe/config.toml"
    config.write_text(config.read_text() + '\ndefault_scope = "host"\n')
    shell = Pty(env)
    assert shell.ready()
    start = len(shell.plain())
    shell.send("/mode agent-host\r")
    wait_new(shell, "Type agent-host", start)
    shell.send("agent-host\r")
    shell.drain(.3)
    return home, shell


def main():
    root = Path(tempfile.mkdtemp(prefix="aishe-lean-cancel-"))
    try:
        marker = root / "late-provider-tool"
        home, shell = fixture("lean-cancel-provider", {
            "AISHE_FAKE_DELAY_MS": "1000", "AISHE_FAKE_TOOL": f"touch {marker}",
        })
        try:
            start = len(shell.plain())
            shell.send("?make the harmless delayed marker\r")
            wait_new(shell, "task ", start)
            shell.send("\x03")
            wait_new(shell, "cancelled", start)
            assert not marker.exists(), "provider result admitted a tool after Ctrl-C"
            assert "CANCELLED_ANSWER_MUST_NOT_APPEAR" not in shell.plain()[start:]
            check_reply_alignment(shell)
        finally:
            shell.close()
            shutil.rmtree(home, ignore_errors=True)

        home, shell = fixture("lean-cancel-answer", {
            "AISHE_FAKE_DELAY_MS": "1000", "AISHE_FAKE_TOOL": "",
        })
        try:
            shell.send("/mode ask\r")
            shell.drain(.3)
            start = len(shell.plain())
            shell.send("?give the delayed answer\r")
            shell.drain(.2)
            shell.send("\x03")
            wait_new(shell, "cancelled", start)
            assert "CANCELLED_ANSWER_MUST_NOT_APPEAR" not in shell.plain()[start:]
            assert "AIShe · answer" not in shell.plain()[start:]
            check_reply_alignment(shell)
        finally:
            shell.close()
            shutil.rmtree(home, ignore_errors=True)

        marker = root / "late-child-write"
        started = root / "child-started"
        home, shell = fixture("lean-cancel-child", {
            "AISHE_FAKE_DELAY_MS": "0",
            "AISHE_FAKE_TOOL": f"touch {started}; sleep 2; touch {marker}",
        })
        try:
            start = len(shell.plain())
            shell.send("?run the harmless cancellable child\r")
            deadline = time.monotonic() + 4
            while not started.exists() and time.monotonic() < deadline:
                shell.drain(.1)
            assert started.exists(), shell.plain()[-3000:]
            shell.send("\x03")
            wait_new(shell, "cancelled", start, timeout=2)
            shell.drain(2.2)
            assert not marker.exists(), "cancelled command's process group survived"
            assert "CANCELLED_ANSWER_MUST_NOT_APPEAR" not in shell.plain()[start:]
            check_reply_alignment(shell)
        finally:
            shell.close()
            shutil.rmtree(home, ignore_errors=True)
    finally:
        shutil.rmtree(root, ignore_errors=True)
    print("PASS: lean Ctrl-C cancels delayed providers and child processes without stale FIFO replies")


if __name__ == "__main__":
    main()
