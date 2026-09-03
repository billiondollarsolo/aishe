#!/usr/bin/env python3
"""Menus launched from inside `aishe zsh` must read keys and exit cleanly."""

from pty_helper import Pty, environment

FORBIDDEN = ("Failed to initialize input reader", "io.operation_failed", "internal.unexpected")


def check_menu(shell, command, expected_row, marker):
    start = len(shell.transcript)
    shell.send(command + "\r")
    if not shell.expect(expected_row):
        raise AssertionError(
            "%s did not paint its menu:\n%s" % (command, shell.transcript[start:][-2000:])
        )
    shell.drain(0.3)
    shell.send("\x1b")
    shell.drain(0.8)
    shell.send("print -r -- %s_''OK\r" % marker)
    if not shell.expect("%s_OK" % marker):
        raise AssertionError(
            "keystroke after %s was swallowed:\n%s" % (command, shell.transcript[start:][-2000:])
        )
    segment = shell.plain()[start:]
    for forbidden in FORBIDDEN:
        if forbidden in segment:
            raise AssertionError("%s printed %r:\n%s" % (command, forbidden, segment[-2000:]))


def main():
    _, env = environment("menus")
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        check_menu(shell, "/settings", "Exit without changes", "SETTINGS")
        check_menu(shell, "aishe tour", "Lesson 1", "TOUR")
        print("in-shell menus: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
