#!/usr/bin/env python3
"""Unit tests for terminal compatibility report and SSH fixture semantics."""

from __future__ import annotations

import dataclasses
import importlib.util
import pathlib
import sys
import unittest
from unittest.mock import patch


MODULE_PATH = pathlib.Path(__file__).with_name("terminal_compat.py")
SPEC = importlib.util.spec_from_file_location("terminal_compat", MODULE_PATH)
assert SPEC and SPEC.loader
terminal_compat = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = terminal_compat
SPEC.loader.exec_module(terminal_compat)


class TerminalCompatibilityTests(unittest.TestCase):
    def test_resize_probe_observes_later_size_without_queued_commands(self) -> None:
        class DelayedResize:
            transcript = ""
            captures = 0
            sent = 0
            pending = None

            def sendline(self, line):
                self.assert_idle()
                self.pending = line.split()[2]
                self.sent += 1
                self.transcript += line + "\r\n"

            def assert_idle(self):
                if self.pending is not None:
                    raise AssertionError("queued a probe before the prior output")

            def capture(self):
                self.captures += 1
                if self.captures % 2 == 0 and self.pending is not None:
                    width = 80 if self.sent == 1 else 120
                    self.transcript += f"{self.pending}{width}\r\n"
                    self.pending = None
                return self.transcript

        transport = DelayedResize()
        with patch.object(terminal_compat.time, "sleep"):
            text = terminal_compat.wait_for_columns(transport, "DELAYED", 120)
        self.assertIn("DELAYED_COLS_0_80\r\n", text)
        self.assertIn("DELAYED_COLS_1_120\r\n", text)
        self.assertEqual(transport.sent, 2)

    def test_resize_probe_waits_for_fragmented_output_line_to_complete(self) -> None:
        class FragmentedResize:
            transcript = ""
            captures = 0
            sent = 0
            pending = None

            def sendline(self, line):
                if self.pending is not None:
                    raise AssertionError("queued a probe after only partial output")
                self.pending = line.split()[2]
                self.sent += 1
                self.transcript += line + "\r\n"

            def capture(self):
                self.captures += 1
                if self.captures == 1:
                    self.transcript += self.pending + "1"
                elif self.captures == 2:
                    self.transcript += "20"  # A partial 120 must not pass.
                elif self.captures == 3:
                    self.transcript += "0"  # The observed size is actually 1200.
                elif self.captures == 4:
                    self.transcript += "\r\n"
                    self.pending = None
                else:
                    self.transcript += self.pending + "120\r\n"
                    self.pending = None
                return self.transcript

        transport = FragmentedResize()
        with patch.object(terminal_compat.time, "sleep"):
            text = terminal_compat.wait_for_columns(transport, "FRAGMENTED", 120)
        self.assertIn("FRAGMENTED_COLS_0_1200\r\n", text)
        self.assertIn("FRAGMENTED_COLS_1_120\r\n", text)
        self.assertEqual(transport.sent, 2)
        self.assertEqual(transport.captures, 5)

    def test_resize_probe_fails_boundedly_without_actual_size_output(self) -> None:
        class NoResize:
            transcript = ""
            sent = 0

            def sendline(self, line):
                self.sent += 1
                marker = line.split()[2]
                # Even an echoed command mentioning 120 cannot prove resize.
                self.transcript += line + " # 120\r\n" + f"{marker}80\r\n"

            def capture(self):
                return self.transcript

        transport = NoResize()
        with patch.object(terminal_compat.time, "monotonic", side_effect=[0, .1, .2, .3, .4, .5]), \
                patch.object(terminal_compat.time, "sleep"):
            with self.assertRaisesRegex(terminal_compat.ContractFailure, "COLUMNS=120"):
                terminal_compat.wait_for_columns(transport, "NO_RESIZE", 120, timeout=.5)
        self.assertEqual(transport.sent, 4)
        self.assertIn("NO_RESIZE_COLS_3_80", transport.transcript)

    def test_ready_prompt_accepts_actual_colored_mode_and_glyph_segments(self) -> None:
        for glyph in (">", "❯", "»", ">>", "*"):
            prompt = f"\x1b[38;5;220mask\x1b[0m \x1b[1m{glyph}\x1b[0m "
            self.assertTrue(terminal_compat.has_ready_prompt(prompt))
        self.assertFalse(terminal_compat.has_ready_prompt("waiting for a mode"))

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
