#!/usr/bin/env python3
"""The palette repaints the prompt, clears the buffer on cancel, fills slash forms."""

from pty_helper import Pty, environment


def main():
    _, env = environment("palette")
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        start = len(shell.plain())
        shell.send("/\t")
        if not shell.expect("AIShe command palette"):
            raise AssertionError("palette did not open:\n" + shell.plain()[start:])
        shell.send("\x1b")
        if not shell.expect("palette cancelled"):
            raise AssertionError("palette did not report cancelling")
        shell.drain(0.6)
        # zle -I makes ZLE repaint the prompt below the picker frame.
        after_frame = shell.plain()[start:].rsplit("/help", 1)[-1]
        if "»" not in after_frame:
            raise AssertionError(
                "prompt was not repainted below the picker frame:\n" + repr(after_frame[-800:])
            )
        shell.send("print -r -- PAL_''OK\r")
        if not shell.expect("PAL_OK"):
            raise AssertionError("buffer still held '/' after cancel:\n" + shell.plain()[start:])
        shell.send("/\t")
        shell.drain(1.5)
        shell.send("\r")
        shell.drain(1.2)
        tail = shell.plain()[-400:]
        if "» /" not in tail:
            raise AssertionError("selection did not fill a slash form:\n" + repr(tail))
        shell.send("\x15")
        print("palette: ok")
    finally:
        shell.close()


if __name__ == "__main__":
    main()
