#!/usr/bin/env python3
"""Validate maintained shell sources and the generated zsh/Bash hooks."""

from __future__ import annotations

import argparse
import pathlib
import subprocess
from collections.abc import Callable, Sequence

from harness_identity import require_current_binary


Run = Callable[..., subprocess.CompletedProcess[str]]
STATIC_SHELL_SOURCES = (
    "install.sh",
    "tests/installer_runtime_transaction.sh",
    "tests/installer_upgrade.sh",
)


def _checked(
    command: Sequence[str],
    *,
    root: pathlib.Path,
    run: Run,
    input_text: str | None = None,
) -> str:
    completed = run(
        list(command),
        cwd=root,
        input=input_text,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = completed.stderr.strip() or completed.stdout.strip()
        raise RuntimeError(f"{' '.join(command)} failed: {detail}")
    return completed.stdout


def validate(binary: pathlib.Path, *, root: pathlib.Path, run: Run = subprocess.run) -> None:
    verified = pathlib.Path(require_current_binary(binary, root=root, announce=False))
    _checked(("shellcheck", *STATIC_SHELL_SOURCES), root=root, run=run)
    for shell in ("zsh", "bash"):
        generated = _checked((str(verified), "init", shell), root=root, run=run)
        _checked((shell, "-n"), root=root, run=run, input_text=generated)


def parse_arguments(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=pathlib.Path)
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_arguments(argv)
    root = pathlib.Path(__file__).resolve().parent.parent
    validate(args.binary.resolve(), root=root)
    print("shell-contract: PASS (shellcheck + generated zsh/Bash syntax)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
