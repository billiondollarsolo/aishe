#!/usr/bin/env python3
"""Deterministic tests for external-harness binary identity validation."""

import unittest

from harness_identity import cargo_version, identity_problems, parse_binary_identity


class IdentityTests(unittest.TestCase):
    def test_parses_current_version_shape(self):
        self.assertEqual(
            parse_binary_identity("aishe 0.6.5 (4a2c7e4, 2026-08-01)\n"),
            {"version": "0.6.5", "commit": "4a2c7e4", "date": "2026-08-01"},
        )

    def test_rejects_unrecognized_shape(self):
        with self.assertRaises(ValueError):
            parse_binary_identity("aishe 0.6.5")

    def test_reads_package_version_before_dependencies(self):
        text = """[package]
name = "aishe"
version = "0.7.0"

[dependencies]
version = "not-the-package"
"""
        self.assertEqual(cargo_version(text), "0.7.0")

    def test_reports_version_and_commit_mismatch(self):
        identity = {
            "version": "0.6.3",
            "commit": "old1234",
            "date": "2026-07-01",
        }
        self.assertEqual(
            identity_problems(identity, "0.6.5", "new5678"),
            [
                "binary version 0.6.3 != checkout version 0.6.5",
                "binary commit old1234 != checkout commit new5678",
            ],
        )
        self.assertEqual(identity_problems(identity, "0.6.3", None), [])

    def test_accepts_any_unambiguous_binary_prefix_of_full_checkout_commit(self):
        expected = "a6173470eac292ae1241d4d156ba53d82a82fae2"
        for prefix in ("a617347", "a6173470", expected):
            with self.subTest(prefix=prefix):
                identity = {
                    "version": "0.6.5",
                    "commit": prefix,
                    "date": "2026-08-02",
                }
                self.assertEqual(identity_problems(identity, "0.6.5", expected), [])

    def test_rejects_invalid_or_incorrect_binary_commit_prefix(self):
        expected = "a6173470eac292ae1241d4d156ba53d82a82fae2"
        for prefix in ("a61734", "a6173471", "unknown", "A6173470"):
            with self.subTest(prefix=prefix):
                identity = {
                    "version": "0.6.5",
                    "commit": prefix,
                    "date": "2026-08-02",
                }
                self.assertEqual(len(identity_problems(identity, "0.6.5", expected)), 1)


if __name__ == "__main__":
    unittest.main()
