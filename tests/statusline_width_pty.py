#!/usr/bin/env python3
"""The statusline shortens on a narrow terminal; the mode is the last to go."""

import os

from pty_helper import Pty, environment


def main():
    home, env = environment("width")
    deep = os.path.join(home, "a-directory-name-that-is-long", "another-long-segment", "and-one-more")
    os.makedirs(deep)
    shell = Pty(env, cols=80)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.send("cd %s\r" % deep)
        shell.drain(1.0)
        shell.send("print -r -- NARROW=$_AISHE_STATUS_TEXT\r")
        if not shell.expect("NARROW="):
            raise AssertionError("could not read the narrow status text")
        line = shell.plain().rsplit("NARROW=", 1)[1].splitlines()[0].strip()
        if "auto" not in line:
            raise AssertionError("narrow status dropped the mode: %r" % line)
        shell.send("print -r -- CELLS=${(m)#_AISHE_STATUS_TEXT}\r")
        shell.expect("CELLS=")
        cells = shell.plain().rsplit("CELLS=", 1)[1].splitlines()[0].strip()
        if not cells.isdigit() or int(cells) > 12:
            raise AssertionError(
                "narrow status was not shortened: %s cells (%r)" % (cells, line)
            )
        print("statusline width: ok (status %r, %s cells)" % (line, cells))
    finally:
        shell.close()


if __name__ == "__main__":
    main()
