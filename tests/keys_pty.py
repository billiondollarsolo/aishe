#!/usr/bin/env python3
"""Shift-Tab cycles the mode only on an empty line."""

from pty_helper import Pty, environment

ZSHRC = (
    "unset HISTFILE\n"
    "aishe-test-shift-tab() { BUFFER+='<RMC>'; }\n"
    "zle -N aishe-test-shift-tab\n"
    "bindkey '^[[Z' aishe-test-shift-tab\n"
)


def main():
    _, env = environment("keys", zshrc=ZSHRC)
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("echo partial")
        shell.send("\x1b[Z")
        shell.drain(0.6)
        if "<RMC>" not in shell.plain():
            raise AssertionError(
                "Shift-Tab with text on the line did not delegate:\n" + shell.plain()[-800:]
            )
        shell.send("\x15")  # kill the line
        shell.drain(0.3)
        shell.send("\x1b[Z")  # empty line: auto -> yolo consent
        if not shell.expect("Type yolo to continue"):
            raise AssertionError(
                "Shift-Tab on an empty line did not cycle the mode:\n" + shell.plain()[-800:]
            )
        shell.send("\x1b")
        shell.drain(0.5)
        print("keys: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
