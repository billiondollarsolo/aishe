#!/usr/bin/env python3
"""Unit tests for terminal compatibility report and SSH fixture semantics."""

from __future__ import annotations

import dataclasses
import importlib.util
import pathlib
import sys
import unittest


MODULE_PATH = pathlib.Path(__file__).with_name("terminal_compat.py")
SPEC = importlib.util.spec_from_file_location("terminal_compat", MODULE_PATH)
assert SPEC and SPEC.loader
terminal_compat = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = terminal_compat
SPEC.loader.exec_module(terminal_compat)


class TerminalCompatibilityTests(unittest.TestCase):
    def test_status_vocabulary_is_machine_stable(self) -> None:
        for status in ("pass", "fail", "limitation", "unsupported"):
            result = terminal_compat.CapabilityResult("sample", status, "detail")
            self.assertEqual(dataclasses.asdict(result)["status"], status)

    def test_remote_fixture_uses_isolated_state_and_cleanup(self) -> None:
        command = terminal_compat.remote_fixture_command("/opt/aishe", "SSH")
        self.assertIn("mktemp -d", command)
        self.assertIn('rm -rf -- "$root"', command)
        self.assertIn('AISHE_CONFIG_DIR="$root/config"', command)
        self.assertIn("AISHE_FAKE_LLM=", command)
        self.assertIn('ln -s /opt/aishe "$root/bin/aishe"', command)
        self.assertIn('PATH="$root/bin:$PATH"', command)
        self.assertIn("/opt/aishe zsh", command)

    def test_required_capability_must_also_be_selected(self) -> None:
        with self.assertRaises(SystemExit) as caught:
            terminal_compat.main(
                [
                    "missing-binary",
                    "--capability",
                    "local-latency",
                    "--require-capability",
                    "tmux",
                ]
            )
        self.assertIn("also requires", str(caught.exception))

    def test_capability_choices_are_complete(self) -> None:
        self.assertEqual(
            terminal_compat.CAPABILITIES,
            ("local-latency", "tmux", "screen", "ssh"),
        )

    def test_ssh_identity_is_a_path_argument_not_report_metadata(self) -> None:
        parsed = terminal_compat.parser().parse_args(
            ["candidate", "--ssh-identity", "/private/key", "--ssh-target", "host"]
        )
        self.assertEqual(parsed.ssh_identity, pathlib.Path("/private/key"))
        fields = {field.name for field in dataclasses.fields(terminal_compat.CapabilityResult)}
        self.assertNotIn("ssh_identity", fields)

    def test_ssh_detail_redacts_target_and_identity(self) -> None:
        detail = "host root@example.test used /private/key; example.test refused"
        sanitized = terminal_compat.sanitize_ssh_detail(
            detail, "root@example.test", pathlib.Path("/private/key")
        )
        self.assertNotIn("root@example.test", sanitized)
        self.assertNotIn("example.test", sanitized)
        self.assertNotIn("/private/key", sanitized)
        self.assertIn("<ssh-target>", sanitized)

    def test_screen_uses_attached_controlling_pty_for_real_resize(self) -> None:
        argv = terminal_compat.attached_screen_argv(
            "/usr/bin/screen", "qualification", "/candidate/aishe"
        )
        self.assertEqual(argv[-2:], ["/candidate/aishe", "zsh"])
        self.assertNotIn("-d", argv)
        self.assertNotIn("-dmS", argv)
        self.assertEqual(argv[1:3], ["-c", "/dev/null"])


if __name__ == "__main__":
    unittest.main()
