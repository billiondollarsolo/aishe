#!/usr/bin/env python3
"""Run a CI gate and reject a harness that silently exits zero after skipping."""

import subprocess
import sys

from qualify import reported_skip


def main(argv):
    if not argv:
        raise SystemExit("usage: require_gate.py COMMAND [ARG ...]")
    process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    skipped = None
    for line in process.stdout:
        print(line, end="", flush=True)
        skipped = skipped or reported_skip(line)
    code = process.wait()
    if code == 0 and skipped:
        print("FAIL: required CI gate reported " + skipped, file=sys.stderr)
        return 1
    return code


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
