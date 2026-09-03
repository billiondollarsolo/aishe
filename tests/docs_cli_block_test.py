#!/usr/bin/env python3
"""The CLI table in docs/commands.md is generated from the clap tree."""

import pathlib
import subprocess
import sys

BIN = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
ROOT = pathlib.Path(__file__).resolve().parents[1]
BEGIN = "<!-- BEGIN GENERATED CLI SURFACE -->"
END = "<!-- END GENERATED CLI SURFACE -->"


def main():
    generated = subprocess.run(
        [BIN, "commands", "--cli-markdown"], capture_output=True, text=True, check=True
    ).stdout.strip()
    docs = (ROOT / "docs" / "commands.md").read_text(encoding="utf-8")
    start = docs.index(BEGIN)
    end = docs.index(END) + len(END)
    checked_in = docs[start:end].strip()
    if checked_in != generated:
        raise AssertionError(
            "docs/commands.md CLI block is stale. Regenerate with:\n"
            "  aishe commands --cli-markdown\n"
        )
    print("docs CLI block: ok (%d rows)" % (generated.count("\n") - 3))


if __name__ == "__main__":
    main()
