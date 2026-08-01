#!/usr/bin/env python3
"""Fail when a cargo-deny advisory exception loses ownership or expiry data."""

from __future__ import annotations

import datetime as dt
import pathlib
import json
import re
import subprocess
import unittest


ROOT = pathlib.Path(__file__).resolve().parents[1]
POLICY = ROOT / "deny.toml"
IGNORE = re.compile(r'id\s*=\s*"(RUSTSEC-[0-9-]+)"')
DATE = re.compile(r"Review by:\s*(\d{4}-\d{2}-\d{2})")


def advisory_exceptions(
    text: str, *, today: dt.date | None = None
) -> list[tuple[str, str]]:
    today = today or dt.date.today()
    lines = text.splitlines()
    exceptions: list[tuple[str, str]] = []
    for index, line in enumerate(lines):
        match = IGNORE.search(line)
        if not match:
            continue
        context = "\n".join(lines[max(0, index - 8) : index + 1])
        for required in ("Owner:", "Added:", "Review by:", "Target removal:", "reason ="):
            if required not in context:
                raise AssertionError(f"{match.group(1)} is missing {required}")
        review = DATE.search(context)
        if not review:
            raise AssertionError(f"{match.group(1)} has no parseable review date")
        review_date = dt.date.fromisoformat(review.group(1))
        if review_date < today:
            raise AssertionError(
                f"{match.group(1)} review expired on {review_date.isoformat()}"
            )
        exceptions.append((match.group(1), review.group(1)))
    return exceptions


class AdvisoryPolicyTests(unittest.TestCase):
    def test_every_exception_is_owned_and_time_boxed(self) -> None:
        exceptions = advisory_exceptions(POLICY.read_text(encoding="utf-8"))
        self.assertGreater(len(exceptions), 0)
        self.assertEqual(len(exceptions), len({item[0] for item in exceptions}))

    def test_missing_metadata_fails(self) -> None:
        with self.assertRaisesRegex(AssertionError, "Owner"):
            advisory_exceptions(
                '[advisories]\nignore = [{ id = "RUSTSEC-2099-0001", reason = "x" }]\n'
            )

    def test_expired_review_fails(self) -> None:
        with self.assertRaisesRegex(AssertionError, "review expired"):
            advisory_exceptions(
                """[advisories]
# Owner: maintainers · Added: 2026-01-01 · Review by: 2026-01-31
# Target removal: upgrade dependency
ignore = [{ id = "RUSTSEC-2099-0001", reason = "temporary" }]
""",
                today=dt.date(2026, 2, 1),
            )

    def test_transport_and_terminal_stacks_are_single_version(self) -> None:
        completed = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked"],
            cwd=ROOT,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=120,
            check=True,
        )
        packages = json.loads(completed.stdout)["packages"]
        versions = {
            name: sorted(
                {package["version"] for package in packages if package["name"] == name}
            )
            for name in ("crossterm", "ureq", "rustls")
        }
        self.assertEqual(versions["crossterm"], ["0.29.0"])
        self.assertEqual(versions["ureq"], ["3.3.0"])
        self.assertEqual(len(versions["rustls"]), 1, versions["rustls"])


if __name__ == "__main__":
    unittest.main()
