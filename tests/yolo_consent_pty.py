#!/usr/bin/env python3
"""Declining the yolo consent prompt is a cancel, not an internal error."""

from pty_helper import Pty, environment


def main():
    _, env = environment("yolo")
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        start = len(shell.transcript)
        shell.send("\x1b[Z")  # Shift-Tab: auto -> yolo consent
        if not shell.expect("Type yolo to continue"):
            raise AssertionError("consent prompt did not appear:\n" + shell.transcript[start:])
        shell.send("n\r")
        if not shell.expect("mode stays auto"):
            raise AssertionError("decline was not acknowledged:\n" + shell.plain()[start:])
        shell.send("\x1b[Z")
        if not shell.expect("Type yolo to continue", timeout=10):
            raise AssertionError("second consent prompt did not appear")
        shell.send("\x1b")  # Esc cancels the raw-mode read
        shell.drain(1.0)
        shell.send("print -r -- MODE=$AISHE_MODE\r")
        if not shell.expect("MODE=auto"):
            raise AssertionError("mode did not stay auto:\n" + shell.plain()[start:])
        segment = shell.plain()[start:]
        for forbidden in ("internal.unexpected", "support bundle", "^[[Z"):
            if forbidden in segment:
                raise AssertionError("consent flow printed %r:\n%s" % (forbidden, segment[-1500:]))
        print("yolo consent: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
