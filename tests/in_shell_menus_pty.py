#!/usr/bin/env python3
"""Menus launched from inside `aishe zsh` must read keys and exit cleanly."""

import time

from pty_helper import CSI, Pty, environment

FORBIDDEN = ("Failed to initialize input reader", "io.operation_failed", "internal.unexpected")


def wait_receipt(shell, receipt, start, timeout=5):
    deadline = time.monotonic() + timeout
    while receipt not in CSI.sub('', shell.transcript[start:]) and time.monotonic() < deadline:
        shell.drain(.1)
    assert receipt in CSI.sub('', shell.transcript[start:]), shell.plain()[-2000:]


def check_menu(shell, command, expected_row, marker, receipt):
    start = len(shell.transcript)
    shell.send(command + "\r")
    if not shell.expect(expected_row):
        raise AssertionError(
            "%s did not paint its menu:\n%s" % (command, shell.transcript[start:][-2000:])
        )
    shell.drain(0.3)
    cancel_start = len(shell.transcript)
    shell.send("\x1b")
    wait_receipt(shell, receipt, cancel_start)
    shell.send("print -r -- %s_''OK\r" % marker)
    if not shell.expect("%s_OK" % marker):
        raise AssertionError(
            "keystroke after %s was swallowed:\n%s" % (command, shell.transcript[start:][-2000:])
        )
    segment = CSI.sub('', shell.transcript[start:])
    for forbidden in FORBIDDEN:
        if forbidden in segment:
            raise AssertionError("%s printed %r:\n%s" % (command, forbidden, segment[-2000:]))


def main():
    _, env = environment("menus")
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        check_menu(shell, "/settings", "No unsaved changes", "SETTINGS", "Saved settings are unchanged.")
        check_menu(shell, "aishe tour", "Lesson 1", "TOUR", "Tour paused.")
        print("in-shell menus: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
