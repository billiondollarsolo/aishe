#!/usr/bin/env python3
"""Every visible flag and positional in `aishe --help` explains itself."""

import re
import subprocess
import sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
# "      --flag <VALUE>  " or "  <ID>  " with nothing after it.
EMPTY = re.compile(r"^\s+(-{1,2}[A-Za-z0-9][^\s]*|<[A-Z_]+>|\[[A-Z_]+\.{0,3}\])"
                   r"(,\s*--[^\s]+)?(\s+<[^>]+>|\s+\[[^\]]+\])?\s*$")
SECTION = re.compile(r"^(Options|Arguments):\s*$")


def help_for(path):
    out = subprocess.run([BIN] + path + ["--help"], capture_output=True, text=True)
    return out.stdout


def subcommands(text):
    names, in_commands = [], False
    for line in text.splitlines():
        if line.startswith("Commands:"):
            in_commands = True
            continue
        if in_commands:
            if not line.strip() or not line.startswith("  "):
                break
            name = line.strip().split()[0]
            if name != "help":
                names.append(name)
    return names


def walk(path, missing, json_help, seen):
    key = " ".join(path)
    if key in seen:
        return
    seen.add(key)
    text = help_for(path)
    lines = text.splitlines()
    section = None
    for index, line in enumerate(lines):
        if SECTION.match(line.strip()):
            section = line.strip()
            continue
        if not line.startswith(" "):
            section = None
            continue
        if not section:
            continue
        if line.strip().startswith("--json"):
            rest = line.strip()[len("--json"):].strip()
            if rest:
                json_help.add(rest)
        if not EMPTY.match(line):
            continue
        # clap's long form puts the description on the following, deeper line.
        indent = len(line) - len(line.lstrip())
        following = lines[index + 1] if index + 1 < len(lines) else ""
        described = (
            following.strip()
            and len(following) - len(following.lstrip()) > indent
            and re.search(r"[A-Za-z]", following)
        )
        if not described:
            missing.append("aishe %s: %s" % (key, line.strip()))
    for name in subcommands(text):
        walk(path + [name], missing, json_help, seen)


def main():
    missing, json_help = [], set()
    walk([], missing, json_help, set())
    if missing:
        raise AssertionError("arguments without help:\n" + "\n".join(missing))
    if len(json_help) > 2:
        raise AssertionError("--json is phrased %d ways:\n%s" % (len(json_help), "\n".join(sorted(json_help))))
    print("help strings: ok (%d --json phrasings)" % len(json_help))


if __name__ == "__main__":
    main()
