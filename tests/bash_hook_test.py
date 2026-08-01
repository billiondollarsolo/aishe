#!/usr/bin/env python3
"""Unit tests for Bash version discovery and honest qualification reporting."""

from __future__ import annotations

import contextlib
import io
import pathlib
import tempfile
import unittest
from unittest import mock

import bash_hook


class BashHookReportingTests(unittest.TestCase):
    def identity(self, version: tuple[int, int, int], path: str) -> bash_hook.BashIdentity:
        major, minor, patch = version
        return bash_hook.BashIdentity(
            path=path,
            version=f"{major}.{minor}.{patch}",
            major=major,
            minor=minor,
            patch=patch,
            platform="test-platform",
        )

    def result(self, identity: bash_hook.BashIdentity, passed: bool) -> bash_hook.BashResult:
        cases = [bash_hook.CaseResult(case_id, "pass") for case_id in bash_hook.CASE_IDS]
        if not passed:
            cases[0].status = "fail"
        return bash_hook.BashResult(identity, cases)

    def test_parse_bash_32_identity(self):
        parsed = bash_hook.parse_bash_identity(
            "/bin/bash",
            "GNU bash, version 3.2.57(1)-release (x86_64-apple-darwin23)\n",
        )
        self.assertEqual(parsed.version, "3.2.57")
        self.assertEqual(parsed.family, "3.2")
        self.assertEqual(parsed.platform, "x86_64-apple-darwin23")

    def test_parse_bash_5_identity(self):
        parsed = bash_hook.parse_bash_identity(
            "/usr/bin/bash",
            "GNU bash, version 5.2.21(1)-release (x86_64-pc-linux-gnu)\n",
        )
        self.assertEqual(parsed.family, "5.x")

    def test_unavailable_family_is_not_a_pass(self):
        five = self.result(self.identity((5, 2, 21), "/test/bash5"), True)
        self.assertEqual(
            bash_hook.family_coverage([five]), {"3.2": "unavailable", "5.x": "pass"}
        )
        self.assertEqual(bash_hook.qualification_exit_code([five], ["3.2"]), 1)
        self.assertEqual(bash_hook.qualification_exit_code([five], ["5.x"]), 0)

    def test_failed_member_fails_family_and_run(self):
        old = self.result(self.identity((3, 2, 57), "/test/bash32"), False)
        new = self.result(self.identity((5, 2, 21), "/test/bash5"), True)
        self.assertEqual(bash_hook.family_coverage([old, new])["3.2"], "fail")
        self.assertEqual(bash_hook.qualification_exit_code([old, new], []), 1)

    def test_no_discovered_bash_fails(self):
        self.assertEqual(bash_hook.qualification_exit_code([], []), 1)

    def test_discovery_deduplicates_real_paths_and_records_missing(self):
        first = self.identity((5, 2, 21), "/real/bash")

        def inspect(candidate: str) -> bash_hook.BashIdentity:
            if candidate == "missing":
                raise FileNotFoundError(candidate)
            return first

        with mock.patch.object(bash_hook, "inspect_bash", side_effect=inspect), mock.patch(
            "bash_hook.os.path.realpath", return_value="/real/bash"
        ):
            found, unavailable = bash_hook.discover_bashes(["bash", "alias", "missing"])
        self.assertEqual(found, [first])
        self.assertEqual(len(unavailable), 1)
        self.assertIn("missing", unavailable[0])

    def test_json_payload_names_tier_b_differences(self):
        result = self.result(self.identity((5, 2, 21), "/test/bash5"), True)
        payload = bash_hook.report_payload(
            "/test/aishe",
            [result],
            [],
            ["5.x"],
            {"version": "0.6.5", "commit": "abc1234", "date": "2026-07-31"},
        )
        self.assertEqual(payload["schema_version"], 1)
        self.assertEqual(payload["binary"]["identity"]["commit"], "abc1234")
        self.assertEqual(payload["declared_tiers"], {"3.2": "B-", "5.x": "B"})
        self.assertEqual(payload["results"][0]["effective_tier"], "B")
        self.assertTrue(payload["expected_tier_b_differences"])
        self.assertEqual(payload["family_coverage"]["3.2"], "unavailable")

    def test_bash_32_accepts_only_declared_expected_differences(self):
        identity = self.identity((3, 2, 57), "/test/bash32")
        result = self.result(identity, True)
        for case in result.cases:
            if case.id in bash_hook.EXPECTED_DIFFERENCE_CASES["3.2"]:
                case.status = "expected_difference"
                case.detail = (
                    "unsupported; alternative: "
                    + bash_hook.EXPECTED_DIFFERENCE_CASES["3.2"][case.id]
                )
        self.assertTrue(result.passed)
        self.assertEqual(bash_hook.qualification_exit_code([result], ["3.2"]), 0)

    def test_expected_difference_is_rejected_for_bash_5(self):
        identity = self.identity((5, 2, 21), "/test/bash5")
        result = self.result(identity, True)
        result.cases[0].status = "expected_difference"
        self.assertFalse(result.passed)
        self.assertIn("not allowed", bash_hook.case_matrix_problems(identity, result.cases)[0])

    def test_expected_difference_without_declared_alternative_is_rejected(self):
        identity = self.identity((3, 2, 57), "/test/bash32")
        result = self.result(identity, True)
        case = next(case for case in result.cases if case.id == "force-agent-key")
        case.status = "expected_difference"
        case.detail = "unsupported"
        self.assertFalse(result.passed)
        self.assertIn("tested alternative", bash_hook.case_matrix_problems(identity, result.cases)[0])

    def test_missing_case_prevents_matrix_pass(self):
        identity = self.identity((5, 2, 21), "/test/bash5")
        result = self.result(identity, True)
        result.cases.pop()
        self.assertFalse(result.passed)
        self.assertIn("missing cases", bash_hook.case_matrix_problems(identity, result.cases)[0])

    def test_json_destination_is_not_needed_for_text_report(self):
        result = self.result(self.identity((5, 2, 21), "/test/bash5"), True)
        payload = bash_hook.report_payload("/test/aishe", [result], [], [])
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            bash_hook.print_report(payload)
        self.assertIn("Bash 3.2 Tier B- unavailable", output.getvalue())
        self.assertIn("Expected Tier-B differences", output.getvalue())

    def test_required_families_parser_supports_strict_matrix(self):
        args = bash_hook.parse_args(["--strict-matrix"])
        self.assertTrue(args.strict_matrix)
        self.assertEqual(args.binary, "target/release/aishe")

    def test_parser_supports_requiring_the_requested_binary_family(self):
        args = bash_hook.parse_args(["--bash", "bash", "--require-current-family"])
        self.assertEqual(args.bashes, ["bash"])
        self.assertTrue(args.require_current_family)

    def test_current_family_resolution_requires_known_family(self):
        known = self.identity((3, 2, 57), "/test/bash32")
        required, problems = bash_hook.resolve_required_families(
            [known], [], strict_matrix=False, require_current=True
        )
        self.assertEqual(required, ["3.2"])
        self.assertEqual(problems, [])

        unknown = self.identity((4, 4, 23), "/test/bash4")
        required, problems = bash_hook.resolve_required_families(
            [unknown], [], strict_matrix=False, require_current=True
        )
        self.assertEqual(required, ["other"])
        self.assertIn("no declared tier", problems[0])

    def test_report_can_be_serialized_to_a_caller_owned_path(self):
        # The test deliberately writes only to its own temporary directory; the
        # harness itself performs the equivalent write for --json.
        payload = bash_hook.report_payload("/test/aishe", [], ["missing"], [])
        with tempfile.TemporaryDirectory() as directory:
            destination = pathlib.Path(directory) / "result.json"
            import json

            destination.write_text(json.dumps(payload), encoding="utf-8")
            self.assertIn('"schema_version": 1', destination.read_text(encoding="utf-8"))

    def test_appended_call_marker_ignores_old_log_content(self):
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / "calls.log"
            old = "--suggest-line old request\n"
            path.write_text(old + "--suggest-line new request\n", encoding="utf-8")
            bash_hook.wait_for_appended_text(
                path, len(old), "--suggest-line new request", timeout=0.1
            )
            with self.assertRaisesRegex(AssertionError, "call log marker"):
                bash_hook.wait_for_appended_text(
                    path, len(old), "--suggest-line old request", timeout=0.01
                )


if __name__ == "__main__":
    unittest.main()
