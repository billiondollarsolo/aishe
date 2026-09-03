#!/usr/bin/env python3
"""A user's prompt survives; a stock prompt gets the AIShe glyph."""

from pty_helper import Pty, environment


def run_case(label, zshrc, expect, absent, extra=None):
    _, env = environment("theme-" + label, zshrc=zshrc, extra=extra)
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("%s: shell never became ready" % label)
        shell.drain(0.5)
        plain = shell.plain()
        for text in expect:
            if text not in plain:
                raise AssertionError("%s: expected %r:\n%s" % (label, text, plain[-1200:]))
        for text in absent:
            if text in plain:
                raise AssertionError("%s: %r must not appear:\n%s" % (label, text, plain[-1200:]))
        shell.send("print -r -- STATUS=$_AISHE_STATUS_TEXT\r")
        if not shell.expect("STATUS=") or "menu-model" not in shell.plain():
            raise AssertionError("%s: status text was not composed" % label)
    finally:
        shell.close()


def main():
    run_case("theme", "unset HISTFILE\nPROMPT='THEME> '\n", ["THEME> "], ["»"])
    run_case("stock", "unset HISTFILE\n", ["»"], ["THEME> "])
    run_case(
        "forced",
        "unset HISTFILE\nPROMPT='THEME> '\n",
        ["»"],
        [],
        extra={"AISHE_PTY_PROMPT": "force"},
    )
    print("theme prompt: ok")


if __name__ == "__main__":
    main()
