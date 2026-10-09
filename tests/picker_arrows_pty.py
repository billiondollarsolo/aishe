#!/usr/bin/env python3
"""Arrow keys move the selection in every in-shell picker.

/dev/tty inside the zsh-PTY front end is the *outer* proxy terminal, where
AIShe's forwarding loop also reads: it won the race for the bytes after an ESC,
so an arrow arrived as a bare Esc (cancel) and the rest leaked to the prompt.
"""

from pty_helper import Pty, environment

ARROWS = [("\x1b[B", "CSI"), ("\x1bOB", "SS3")]


def check(command, opened, label):
    for sequence, encoding in ARROWS:
        _, env = environment("arrows")
        shell = Pty(env)
        try:
            if not shell.ready():
                raise AssertionError("shell never became ready")
            shell.send(command + "\r")
            if not shell.expect(opened):
                raise AssertionError("%s did not open:\n%s" % (label, shell.plain()[-800:]))
            shell.drain(0.5)
            start = len(shell.plain())
            shell.send(sequence)
            shell.drain(1.0)
            tail = shell.plain()[start:]
            if "cancelled" in tail:
                raise AssertionError(
                    "%s: a %s arrow cancelled the picker:\n%s" % (label, encoding, tail[-600:])
                )
            focused = [line for line in tail.splitlines() if line.strip().startswith(">")]
            if not focused:
                raise AssertionError(
                    "%s: a %s arrow moved nothing:\n%s" % (label, encoding, tail[-600:])
                )
        finally:
            shell.close()


def main():
    check("/model", "type to search", "/model")
    check("/connection", "type to search", "/connection")
    print("picker arrows: ok")


if __name__ == "__main__":
    main()
