#!/usr/bin/env python3
"""Deterministic tests for the local qualification driver."""

from __future__ import annotations

import json
import contextlib
import io
import pathlib
import tempfile
import unittest

import qualify


VERSION_OUTPUT = "aishe 0.6.5 (4a2c7e4, 2026-07-31)\n"


class FakeRunner:
    def __init__(self, failures=()):
        self.failures = {tuple(command) for command in failures}
        self.commands = []

    def run(self, command, *, cwd, env, timeout):
        command = tuple(command)
        self.commands.append(command)
        if command in self.failures:
            return qualify.CommandResult(9, "", "synthetic failure")
        stdout = VERSION_OUTPUT if command[-1:] == ("--version",) else ""
        return qualify.CommandResult(0, stdout, "")


class RepositoryFixture:
    def __init__(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "aishe"\nversion = "0.6.5"\n', encoding="utf-8"
        )
        (self.root / "Cargo.lock").write_text("# lock\n", encoding="utf-8")
        manifest = self.root / "assets/backend/opencode/runtime-manifest.json"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('{"runtime":"opencode","version":"1.18.9"}\n', encoding="utf-8")
        (manifest.parent / "aishe-plugin.mjs").write_text("export const fixture = true;\n", encoding="utf-8")
        (self.root / "SECURITY.md").write_text(
            "Threat-model version: 2026-07-31.1\n", encoding="utf-8"
        )
        fixtures = {
            "tests/safety_corpus.rs": "// safety\n",
            "tests/fixtures/routing/v1.json": '{"schema_version":1,"cases":[]}\n',
            "tests/fixtures/routing/typo-assistance-v1.json": '{"schema_version":1,"cases":[]}\n',
            "tests/boundary_fuzz.rs": "// deterministic boundary seeds\n",
            "tests/real_model.py": "CORPUS = []\n",
            "tests/real_fuzz.py": "# fuzz\n",
            "tests/fixtures/opencode/v1.18.9/events.jsonl": "{}\n",
            "tests/fixtures/opencode/v1.18.9/openapi-contract.json": "{}\n",
        }
        for relative, contents in fixtures.items():
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(contents, encoding="utf-8")
        binary = self.root / "target/release/aishe"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"synthetic release binary")

    def close(self):
        self.temporary.cleanup()


def accept_identity(binary, *, root, announce):
    return str(binary)


class QualificationTests(unittest.TestCase):
    def setUp(self):
        self.repository = RepositoryFixture()
        self.addCleanup(self.repository.close)
        self.output = self.repository.root / "reports/qualification.json"
        self.messages = []

    def run_profile(self, profile, runner, **kwargs):
        return qualify.run_qualification(
            profile,
            self.output,
            root=self.repository.root,
            runner=runner,
            env={"SHELL": "/bin/zsh", "PATH": "/usr/bin:/bin"},
            platform_name=kwargs.pop("platform_name", "Linux"),
            identity_verifier=kwargs.pop("identity_verifier", accept_identity),
            tool_finder=kwargs.pop("tool_finder", lambda tool: f"/fake/bin/{tool}"),
            announce=self.messages.append,
            **kwargs,
        )

    def test_quick_profile_builds_and_verifies_before_external_harnesses(self):
        runner = FakeRunner()
        report = self.run_profile(qualify.PROFILES["quick"], runner)

        build = ("cargo", "build", "--release", "--locked")
        identity = (
            str((self.repository.root / "target/release/aishe").resolve()),
            "--version",
        )
        first_harness = ("python3", "tests/live_contract_test.py")
        self.assertLess(runner.commands.index(build), runner.commands.index(identity))
        self.assertLess(runner.commands.index(identity), runner.commands.index(first_harness))
        self.assertTrue(all(isinstance(command, tuple) for command in runner.commands))
        self.assertEqual(report["binary"]["identity"]["commit"], "4a2c7e4")
        self.assertTrue(report["binary"]["verified_against_checkout"])
        self.assertEqual(report["runtime"]["pinned_version"], "1.18.9")
        self.assertEqual(report["security"]["threat_model_version"], "2026-07-31.1")
        self.assertEqual(report["security"]["safety_matcher_role"], "defense_in_depth")
        self.assertEqual(len(report["security"]["known_limitations"]), 4)
        self.assertTrue(report["runtime"]["trusted_plugin_sha256"])
        self.assertEqual(report["schema_version"], 1)
        self.assertEqual(json.loads(self.output.read_text())["kind"], "aishe_qualification")

    def test_failed_build_blocks_identity_and_harnesses_even_when_keep_going(self):
        build = ("cargo", "build", "--release", "--locked")
        runner = FakeRunner(failures=(build,))
        report = self.run_profile(qualify.PROFILES["quick"], runner, keep_going=True)
        records = {record["id"]: record for record in report["gates"]}

        self.assertEqual(records["release-build"]["status"], "fail")
        self.assertEqual(records["release-identity"]["status"], "skip")
        self.assertEqual(records["release-identity"]["skip_reason"], "release build did not pass")
        self.assertEqual(records["pty-smoke"]["status"], "skip")
        self.assertIn("not verified", records["pty-smoke"]["skip_reason"])
        self.assertFalse(any(command[0] == "python3" for command in runner.commands))
        self.assertEqual(report["summary"]["outcome"], "failed")

    def test_platform_and_credential_gates_are_explicit_skips(self):
        runner = FakeRunner()
        report = self.run_profile(
            qualify.PROFILES["local-full"], runner, platform_name="Darwin"
        )
        records = {record["id"]: record for record in report["gates"]}

        for gate_id in ("installer-upgrade-linux", "credentials-linux"):
            self.assertEqual(records[gate_id]["status"], "skip")
            self.assertIn("not applicable", records[gate_id]["skip_reason"])
        for gate_id in ("real-model", "real-model-fuzz"):
            self.assertEqual(records[gate_id]["status"], "skip")
            self.assertIn("AISHE_REALTEST_KEY", records[gate_id]["skip_reason"])
        self.assertEqual(report["summary"]["outcome"], "passed_with_skips")
        self.assertEqual(report["summary"]["counts"]["skip"], 4)
        self.assertTrue(all(records[gate_id]["status"] != "pass" for gate_id in (
            "installer-upgrade-linux", "credentials-linux", "real-model", "real-model-fuzz"
        )))

    def test_default_stop_records_every_remaining_gate_as_skipped(self):
        first = qualify.PROFILES["quick"].gates[0].command
        runner = FakeRunner(failures=(first,))
        report = self.run_profile(qualify.PROFILES["quick"], runner)

        self.assertEqual(len(runner.commands), 1)
        self.assertEqual(report["gates"][0]["status"], "fail")
        self.assertTrue(all(gate["status"] == "skip" for gate in report["gates"][1:]))
        self.assertTrue(all(gate["command"] for gate in report["gates"]))

    def test_identity_failure_blocks_harness_execution(self):
        runner = FakeRunner()

        def reject_identity(binary, *, root, announce):
            raise SystemExit("synthetic checkout mismatch")

        report = self.run_profile(
            qualify.PROFILES["quick"],
            runner,
            keep_going=True,
            identity_verifier=reject_identity,
        )
        records = {record["id"]: record for record in report["gates"]}
        self.assertEqual(records["release-identity"]["status"], "fail")
        self.assertFalse(any(command[0] == "python3" for command in runner.commands))
        self.assertFalse(report["binary"]["verified_against_checkout"])

    def test_missing_required_tool_is_incomplete_not_passed(self):
        runner = FakeRunner()
        report = self.run_profile(
            qualify.PROFILES["quick"],
            runner,
            tool_finder=lambda tool: None if tool == "zsh" else f"/fake/bin/{tool}",
        )
        records = {record["id"]: record for record in report["gates"]}
        self.assertEqual(records["pty-smoke"]["status"], "skip")
        self.assertTrue(records["pty-smoke"]["required"])
        self.assertEqual(report["summary"]["outcome"], "incomplete")
        self.assertEqual(report["summary"]["required_skips"], 3)

    def test_profile_registry_keeps_external_harnesses_after_identity(self):
        repository = pathlib.Path(__file__).resolve().parent.parent
        for profile in qualify.PROFILES.values():
            gate_ids = [gate.id for gate in profile.gates]
            self.assertEqual(len(gate_ids), len(set(gate_ids)))
            identity_index = gate_ids.index("release-identity")
            for index, gate in enumerate(profile.gates):
                if gate.external_harness:
                    self.assertGreater(index, identity_index, gate.id)
                if gate.command[:1] == ("python3",):
                    self.assertTrue((repository / gate.command[1]).is_file(), gate.id)

    def test_profiles_require_the_current_bash_declared_tier(self):
        for profile in qualify.PROFILES.values():
            gates = {gate.id: gate for gate in profile.gates}
            gate = gates["bash-hook-current"]
            self.assertIn("--require-current-family", gate.command)
            self.assertEqual(gate.required_tools, ("python3", "bash"))

    def test_profile_registry_contains_all_declared_release_profiles(self):
        self.assertEqual(
            set(qualify.PROFILES),
            {"quick", "local-full", "linux-full", "release", "paid-live"},
        )
        release = {gate.id: gate for gate in qualify.PROFILES["release"].gates}
        for gate_id in (
            "shell-contract",
            "lazy-loading",
            "performance-evidence",
            "terminal-local-latency",
            "terminal-linux-multiplexers",
            "advisory-policy-metadata",
        ):
            self.assertIn(gate_id, release)

    def test_paid_live_credentials_are_required_and_never_hidden_as_pass(self):
        runner = FakeRunner()
        report = self.run_profile(qualify.PROFILES["paid-live"], runner)
        records = {record["id"]: record for record in report["gates"]}
        for gate_id in ("real-model", "real-model-fuzz", "paid-live-release"):
            self.assertEqual(records[gate_id]["status"], "skip")
            self.assertTrue(records[gate_id]["required"])
            self.assertIn("AISHE_REALTEST_KEY", records[gate_id]["skip_reason"])
        self.assertEqual(report["summary"]["outcome"], "incomplete")
        self.assertEqual(report["summary"]["required_skips"], 3)

    def test_cross_platform_required_gate_is_explicit_but_not_a_hold(self):
        profile = qualify.Profile(
            "portable",
            "fixture",
            (
                qualify.RELEASE_BUILD,
                qualify.IDENTITY,
                qualify.TERMINAL_LINUX_MULTIPLEXERS,
            ),
        )
        report = self.run_profile(profile, FakeRunner(), platform_name="Darwin")
        record = report["gates"][-1]
        self.assertEqual(record["status"], "skip")
        self.assertFalse(record["required"])
        self.assertEqual(report["summary"]["outcome"], "passed_with_skips")


class ArgumentTests(unittest.TestCase):
    def test_list_needs_neither_profile_nor_output(self):
        args = qualify.parse_arguments(["--list"])
        self.assertTrue(args.list)
        self.assertIsNone(args.output)

    def test_running_requires_explicit_output(self):
        with contextlib.redirect_stderr(io.StringIO()):
            with self.assertRaises(SystemExit):
                qualify.parse_arguments(["quick"])

    def test_run_arguments_accept_keep_going(self):
        args = qualify.parse_arguments(
            ["local-full", "--output", "qualification.json", "--keep-going"]
        )
        self.assertEqual(args.profile, "local-full")
        self.assertEqual(args.output, pathlib.Path("qualification.json"))
        self.assertTrue(args.keep_going)


if __name__ == "__main__":
    unittest.main()
