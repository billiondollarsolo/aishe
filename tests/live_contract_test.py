#!/usr/bin/env python3
"""Deterministic tests for the real-model JSON/exit contract validator."""

import os
import tempfile
import unittest

from live_contract import create_workspace_acceptance, validate_suggest_result


class ContractTests(unittest.TestCase):
    def validate(self, payload, returncode, syntax_check=lambda _command: True):
        import json

        return validate_suggest_result(
            json.dumps(payload), "", returncode, syntax_check=syntax_check
        )[1]

    def test_answer_contract(self):
        payload = {
            "schema_version": 1,
            "kind": "answer",
            "command": "",
            "explanation": "Paris.",
            "risk": "n/a",
            "reason": "",
        }
        self.assertEqual(self.validate(payload, 0), [])
        self.assertIn(
            "answer explanation is empty",
            self.validate({**payload, "explanation": ""}, 0),
        )
        self.assertIn("answer risk is not n/a", self.validate({**payload, "risk": "safe"}, 0))
        self.assertIn("answer exit code is not 0", self.validate(payload, 20))

    def test_command_contract(self):
        payload = {
            "schema_version": 1,
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

    def test_workspace_acceptance_is_exact_private_and_unique(self):
        first_id, first_path = create_workspace_acceptance("livecontract")
        second_id, second_path = create_workspace_acceptance("livecontract")
        try:
            self.assertNotEqual(first_id, second_id)
            self.assertNotEqual(first_path, second_path)
            for shell_id, path in ((first_id, first_path), (second_id, second_path)):
                self.assertEqual(
                    os.path.basename(path), "aishe-yolo-accept-" + shell_id
                )
                self.assertEqual(
                    os.path.realpath(os.path.dirname(path)),
                    os.path.realpath(tempfile.gettempdir()),
                )
                self.assertEqual(os.stat(path).st_mode & 0o777, 0o600)
                with open(path, encoding="utf-8") as file:
                    self.assertEqual(file.read(), "workspace\n")
        finally:
            for path in (first_path, second_path):
                try:
                    os.unlink(path)
                except FileNotFoundError:
                    pass


if __name__ == "__main__":
    unittest.main()
