#!/usr/bin/env python3
"""Default lean UI regressions against a real controlling terminal.

Run: python3 tests/lean_ui_pty.py target/release/aishe
Fake replies deliberately resemble shell commands. Verify effects and shell
responsiveness, rather than matching text that could merely be input echo.
"""

import json
from pathlib import Path
import shutil
import tempfile

from pty_helper import Pty, environment


def run():
    root = Path(tempfile.mkdtemp(prefix="aishe-ui-regression-"))
    marker = root / "answer-must-not-execute"
    reply = json.dumps({"type": "answer", "command": None,
                        "explanation": f"touch {marker}"})
    home, env = environment("lean-ui", mode="ask", extra={
        "AISHE_LEAN": "1", "AISHE_FAKE_LLM": reply,
        "AISHE_FAKE_USAGE": "12,34", "NO_COLOR": "1",
        "AISHE_UNICODE": "ascii", "AISHE_COMMAND_HINT_SHOWN": "1",
    })
    env.pop("AISHE_LEGACY_OPENCODE", None)
    env.pop("AISHE_FAKE_LLM_FILE", None)
    shell = Pty(env, cols=80, rows=24)
    try:
        assert shell.ready(), "lean shell did not start"
        shell.drain(.2)
        start = len(shell.transcript)
        shell.send("? give an answer only\r")
        shell.drain(1)
        assert not marker.exists(), "ask answer executed as shell input"
        shown = shell.transcript[start:]
        assert f"touch {marker}" in shell.plain(), "answer never reached display"
        assert len(shown) < 20000, "answer recursively resubmitted itself"
        shell.send("print -r -- DISPLAY_''STILL_RESPONSIVE\r")
        assert shell.expect("DISPLAY_STILL_RESPONSIVE", 3), "output left shell busy"

        shell.send("/details\r")
        shell.drain(.4)
        assert not marker.exists(), "slash output executed as shell input"
        shell.send("print -r -- SLASH_''STILL_RESPONSIVE\r")
        assert shell.expect("SLASH_STILL_RESPONSIVE", 3), "slash output reentered NL"

        shell.send("/model audit-next-model\r")
        shell.drain(.4)
        shell.send("print -r -- SELECTION_''SYNC=$AISHE_MODEL\r")
        assert shell.expect("SELECTION_SYNC=audit-next-model", 3), "model handoff stale"
        shell.send("/usage\r")
        shell.drain(.4)
        assert "12 in" in shell.plain() and "34 out" in shell.plain(), "switch lost usage"
        shell.send("aishe -c '? standalone answer'\r")
        shell.drain(.5)
        shell.send("/usage\r")
        shell.drain(.4)
        assert "24 in" in shell.plain() and "68 out" in shell.plain(), "standalone call missing from shell usage"
        assert not marker.exists(), "standalone ask answer executed as input"
        shell.send("aishe output detailed\r")
        shell.drain(.4)
        shell.send("/status\r")
        shell.drain(.4)
        assert "details detailed" in shell.plain(), "CLI output setting disagrees with parent"
        shell.send("print -r -- BUFFER_''SURVIVED")
        shell.send("\x0f")
        shell.drain(.4)
        shell.send("\r")
        assert shell.expect("BUFFER_SURVIVED", 3), "Ctrl-O lost or corrupted editable input"

        start = len(shell.transcript)
        shell.send("print COLOR_AUDIT")
        shell.drain(.2)
        typing = shell.transcript[start:]
        assert "\x1b[3" not in typing and "\x1b[9" not in typing, "NO_COLOR typing is colored"
        shell.send("\x15")
        shell.send("/mode ask\r")
        shell.drain(.3)
        assert "unknown mode" not in shell.plain(), "slash mode argument not trimmed"
        shell.send("print -r -- MODE_''SYNC=$AISHE_MODE\r")
        assert shell.expect("MODE_SYNC=ask", 3), "slash mode disagrees with shell"
        shell.resize(32, 24)
        shell.drain(.3)
        shell.send("\x0c")
        shell.drain(.3)
        assert "ask >" in shell.plain(), "narrow prompt lost mode"
        print("PASS: lean display direction, responsiveness, selection, usage, plain styling, mode, narrow prompt")
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    run()
