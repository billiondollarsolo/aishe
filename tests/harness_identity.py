#!/usr/bin/env python3
"""Shared source/binary identity checks for external AIShe test harnesses."""

from __future__ import annotations

import os
import pathlib
import re
import subprocess
import sys


_VERSION = re.compile(
    r"^aishe\s+(?P<version>\S+)\s+\((?P<commit>[^,\s]+),\s*(?P<date>[^)]+)\)$"
)
_CARGO_VERSION = re.compile(r'(?m)^version\s*=\s*"(?P<version>[^"]+)"\s*$')
_GIT_PREFIX = re.compile(r"^[0-9a-f]{7,40}$")


def parse_binary_identity(output: str) -> dict[str, str]:
    """Parse the human version line emitted by aishe --version."""

    match = _VERSION.match(output.strip())
    if not match:
        raise ValueError(f"unrecognized aishe --version output: {output!r}")
    return match.groupdict()


def cargo_version(cargo_toml: str) -> str:
    """Return the package version from the leading package table."""

    package = cargo_toml.split("[dependencies]", 1)[0]
    match = _CARGO_VERSION.search(package)
    if not match:
        raise ValueError("Cargo.toml package version is missing")
    return match.group("version")


def identity_problems(
    identity: dict[str, str], expected_version: str, expected_commit: str | None
) -> list[str]:
    problems = []
    if identity["version"] != expected_version:
        problems.append(
            f"binary version {identity['version']} != checkout version {expected_version}"
        )
    binary_commit = identity["commit"]
    commit_matches = (
        expected_commit
        and _GIT_PREFIX.fullmatch(binary_commit)
        and expected_commit.startswith(binary_commit)
    )
    if expected_commit and not commit_matches:
        problems.append(
            f"binary commit {binary_commit} != checkout commit {expected_commit}"
        )
    return problems


def _git_commit(root: pathlib.Path) -> str | None:
    if not (root / ".git").exists():
        return None
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    value = result.stdout.strip()
    return value or None


def require_current_binary(
    binary: os.PathLike[str] | str,
    *,
    root: os.PathLike[str] | str | None = None,
    announce: bool = True,
) -> str:
    """Resolve and verify a harness binary against this checkout.

    Set AISHE_ALLOW_MISMATCHED_BINARY=1 only when intentionally qualifying a
    packaged artifact that is not built from the current checkout.
    """

    path = pathlib.Path(binary).resolve()
    if not path.is_file():
        raise SystemExit(f"binary not found: {path}")
    repository = (
        pathlib.Path(root).resolve()
        if root is not None
        else pathlib.Path(__file__).resolve().parent.parent
    )
    try:
        result = subprocess.run(
            [str(path), "--version"],
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
        identity = parse_binary_identity(result.stdout)
        expected_version = cargo_version(
            (repository / "Cargo.toml").read_text(encoding="utf-8")
        )
    except (OSError, subprocess.SubprocessError, ValueError) as error:
        raise SystemExit(f"could not verify AIShe test binary {path}: {error}") from error

    problems = identity_problems(identity, expected_version, _git_commit(repository))
    if problems and os.environ.get("AISHE_ALLOW_MISMATCHED_BINARY") != "1":
        detail = "\n  ".join(problems)
        raise SystemExit(
            "refusing to test a binary that does not match this checkout:\n"
            f"  {detail}\n"
            "run cargo build --release --locked first, or set "
            "AISHE_ALLOW_MISMATCHED_BINARY=1 for an intentional artifact test"
        )
    if announce:
        suffix = " (mismatch explicitly allowed)" if problems else ""
        print(
            "test binary: "
            f"aishe {identity['version']} ({identity['commit']}, {identity['date']})"
            f" · {path}{suffix}",
            file=sys.stderr,
        )
    return str(path)
