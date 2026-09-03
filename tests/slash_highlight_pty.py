#!/usr/bin/env python3
"""A registered /command reads as recognized, an unregistered one as an error.

A syntax-highlighting plugin sees `/model` as an absolute path that does not
exist and paints it in its error color, the same red it gives a real typo. AIShe
owns the `/` namespace, so it claims the span; `/modle` stays red, where red is
the truth.
"""

import re

from pty_helper import Pty, environment

# A stand-in for zsh-syntax-highlighting: defines the function AIShe defers to,
# and paints any unknown command red, exactly as the real plugin does for a
# missing path. Keeps the test free of an external plugin checkout.
ZSHRC = """
unset HISTFILE
_zsh_highlight() { }
_aishe_test_highlight() {
  emulate -L zsh
  local head="${BUFFER%%[[:space:]]*}"
  [[ -n "$head" ]] || return 0
  whence -w -- "$head" >/dev/null 2>&1 && return 0
  region_highlight+=("0 ${#head} fg=red")
}
autoload -Uz add-zle-hook-widget
add-zle-hook-widget line-pre-redraw _aishe_test_highlight
"""

# The line is repainted on every keystroke, so a prefix like `/mo` is legitimately
# red while it is still unregistered. The last color applied is what the user sees.
LAST_COLOR = re.compile(r"\x1b\[3([16])m")


def final_color(line):
    _, env = environment("highlight", zshrc=ZSHRC)
    shell = Pty(env)
    try:
        if not shell.ready():
            raise AssertionError("shell never became ready")
        shell.drain(0.3)
        start = len(shell.transcript)
        shell.send(line)
        shell.drain(1.0)
        colors = LAST_COLOR.findall(shell.transcript[start:])
        shell.send("\x15")
        shell.drain(0.2)
        if not colors:
            raise AssertionError("%s was not highlighted at all" % line)
        return {"1": "red", "6": "cyan"}[colors[-1]]
    finally:
        shell.close()


def main():
    recognized = final_color("/model")
    if recognized != "cyan":
        raise AssertionError("a registered /model reads as %s, not recognized" % recognized)
    unknown = final_color("/modle")
    if unknown != "red":
        raise AssertionError("an unregistered /modle reads as %s, not an error" % unknown)
    print("slash highlight: ok (/model cyan, /modle red)")


if __name__ == "__main__":
    main()
