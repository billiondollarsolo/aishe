#!/usr/bin/env python3
"""Unit tests for the shell-contract harness."""

from __future__ import annotations

import pathlib
import subprocess
import tempfile
import unittest
from unittest import mock

import shell_contract


class ShellContractTests(unittest.TestCase):
    def test_lints_static_sources_and_parses_both_generated_hooks(self):
        calls: list[tuple[tuple[str, ...], str | None]] = []

        def fake_run(command, **kwargs):
            calls.append((tuple(command), kwargs.get("input")))
            if command[-2:] == ["init", "zsh"]:
                return subprocess.CompletedProcess(command, 0, "zsh hook\n", "")
            if command[-2:] == ["init", "bash"]:
                return subprocess.CompletedProcess(command, 0, "bash hook\n", "")
            return subprocess.CompletedProcess(command, 0, "", "")

        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "aishe"
            binary.touch()
            with mock.patch.object(shell_contract, "require_current_binary", return_value=str(binary)):
                shell_contract.validate(binary, root=root, run=fake_run)

        self.assertEqual(calls[0][0], ("shellcheck", *shell_contract.STATIC_SHELL_SOURCES))
        self.assertIn((("zsh", "-n")), [command for command, _ in calls])
        self.assertIn((("bash", "-n")), [command for command, _ in calls])
        syntax_inputs = [input_text for _, input_text in calls if input_text is not None]
        self.assertEqual(syntax_inputs, ["zsh hook\n", "bash hook\n"])

    def test_reports_the_failing_command(self):
        def fake_run(command, **kwargs):
            return subprocess.CompletedProcess(command, 2, "", "syntax problem")

        with self.assertRaisesRegex(RuntimeError, "shellcheck.*syntax problem"):
            shell_contract._checked(("shellcheck", "bad.sh"), root=pathlib.Path("."), run=fake_run)


if __name__ == "__main__":
    unittest.main()
