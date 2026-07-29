#!/usr/bin/env python3
"""Deterministic tests for the real-model JSON/exit contract validator."""

import unittest

from live_contract import validate_suggest_result


class ContractTests(unittest.TestCase):
    def validate(self, payload, returncode, syntax_check=lambda _command: True):
        import json

        return validate_suggest_result(
            json.dumps(payload), "", returncode, syntax_check=syntax_check
        )[1]

    def test_answer_contract(self):
        payload = {
            "kind": "answer",
            "command": "",
            "explanation": "Paris.",
            "risk": "n/a",
            "reason": "",
        }
        self.assertEqual(self.validate(payload, 0), [])
        self.assertIn("answer risk is not n/a", self.validate({**payload, "risk": "safe"}, 0))
        self.assertIn("answer exit code is not 0", self.validate(payload, 20))

    def test_command_contract(self):
        payload = {
            "kind": "command",
            "command": "ls -la",
            "explanation": "List files.",
            "risk": "safe",
            "reason": "",
        }
        self.assertEqual(self.validate(payload, 0), [])
        held = {**payload, "risk": "dangerous", "reason": "test"}
        self.assertEqual(self.validate(held, 20), [])
        self.assertIn("risk/exit contract mismatch", self.validate(held, 0))
        self.assertIn(
            "invalid command syntax",
            self.validate(payload, 0, syntax_check=lambda _command: False),
        )

    def test_malformed_and_process_failures(self):
        _, problems = validate_suggest_result("", "thread panicked", 101)
        self.assertIn("panic", problems)
        self.assertIn("invalid JSON contract", problems)
        _, problems = validate_suggest_result("{", "parse error", 1)
        self.assertIn("parse/eval leak", problems)


if __name__ == "__main__":
    unittest.main()
