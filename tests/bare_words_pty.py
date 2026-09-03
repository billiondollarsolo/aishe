#!/usr/bin/env python3
"""The bare words `reset` and `details` belong to the user, not to AIShe."""

from pty_helper import Pty, environment

ZSHRC = (
    "unset HISTFILE\n"
    "reset() { print -r -- USER_RESET_RAN }\n"
    "details() { print -r -- USER_DETAILS_RAN }\n"
)


def main():
    _, env = environment("barewords", zshrc=ZSHRC)
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("reset\r")
        if not shell.expect("USER_RESET_RAN"):
            raise AssertionError("bare reset was hijacked:\n" + shell.plain()[-1200:])
        shell.send("details\r")
        if not shell.expect("USER_DETAILS_RAN"):
            raise AssertionError("bare details was hijacked:\n" + shell.plain()[-1200:])
        shell.send("/details\r")
        if not shell.expect("details:"):
            raise AssertionError("/details stopped working:\n" + shell.plain()[-1200:])
        print("bare words: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
