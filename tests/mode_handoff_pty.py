#!/usr/bin/env python3
"""`aishe mode` and `/mode` agree; slash arguments reach clap word-split."""

from pty_helper import Pty, environment


def main():
    _, env = environment("mode")
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("aishe mode\r")
        if not shell.expect("mode: auto (this shell)"):
            raise AssertionError("aishe mode did not read this shell:\n" + shell.plain()[-1200:])
        shell.send("aishe mode suggest\r")
        shell.expect("mode: suggest (this shell)")
        shell.drain(0.8)
        shell.send("print -r -- MODE=$AISHE_MODE\r")
        if not shell.expect("MODE=suggest"):
            raise AssertionError("aishe mode did not hand off:\n" + shell.plain()[-1200:])
        shell.send("/reasoning high --default\r")
        shell.drain(1.5)
        tail = shell.plain()[-1200:]
        for broken in ("unexpected argument", "unexpected value", "invalid value"):
            if broken in tail:
                raise AssertionError("slash arguments were not word-split:\n" + tail)
        print("mode handoff: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
