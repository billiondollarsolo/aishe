#!/usr/bin/env python3
"""Deterministic tests for the local qualification driver."""

from __future__ import annotations

import json
import contextlib
import io
import pathlib
import tempfile
import subprocess
import sys
import unittest

import qualify


VERSION_OUTPUT = "aishe 0.6.5 (4a2c7e4, 2026-07-31)\n"


class FakeRunner:
    def __init__(self, failures=(), outputs=None):
        self.failures = {tuple(command) for command in failures}
        self.outputs = outputs or {}
        self.commands = []
        self.environments = []

    def run(self, command, *, cwd, env, timeout):
        command = tuple(command)
        self.commands.append(command)
        self.environments.append(dict(env))
        if command in self.failures:
            return qualify.CommandResult(9, "", "synthetic failure")
        if command in self.outputs:
            stdout, stderr = self.outputs[command]
            return qualify.CommandResult(0, stdout, stderr)
        stdout = VERSION_OUTPUT if command[-1:] == ("--version",) else ""
        return qualify.CommandResult(0, stdout, "")


class RepositoryFixture:
    def __init__(self):
        self.temporary = tempfile.TemporaryDirectory()
        # macOS maps /var tempfile paths through /private/var. Keep the fake
        # runner's command keys identical to the driver's canonical paths.
        self.root = pathlib.Path(self.temporary.name).resolve()
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "aishe"\nversion = "0.6.5"\n', encoding="utf-8"
        )
        (self.root / "Cargo.lock").write_text("# lock\n", encoding="utf-8")
        manifest = self.root / "assets/backend/opencode/runtime-manifest.json"
        manifest.parent.mkdir(parents=True)
        manifest.write_text('{"runtime":"opencode","version":"1.18.27"}\n', encoding="utf-8")
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
            "tests/fixtures/opencode/v1.18.27/events.jsonl": "{}\n",
            "tests/fixtures/opencode/v1.18.27/openapi-contract.json": "{}\n",
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
            env=kwargs.pop("env", {"SHELL": "/bin/zsh", "PATH": "/usr/bin:/bin"}),
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
        self.assertEqual(report["runtime"]["pinned_version"], "1.18.27")
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
        expected = sum("zsh" in gate.required_tools for gate in qualify.PROFILES["quick"].gates)
        self.assertEqual(report["summary"]["required_skips"], expected)

    def test_zero_exit_legacy_skip_is_incomplete_instead_of_a_pass(self):
        for stdout, stderr in (
            ("SKIP (LEGACY-gated): run with AISHE_LEGACY_OPENCODE=1\n", ""),
            ("", "\x1b[33mSKIP: no controlling terminal\x1b[0m\n"),
        ):
            runner = FakeRunner(outputs={qualify.PTY_SMOKE.command: (stdout, stderr)})
            # Resolve the binary placeholder just as the driver does.
            command = tuple(qualify._resolved_command(
                qualify.PTY_SMOKE, self.repository.root / "target/release/aishe"
            ))
            runner.outputs = {command: (stdout, stderr)}
            report = self.run_profile(qualify.PROFILES["quick"], runner)
            record = next(row for row in report["gates"] if row["id"] == "pty-smoke")
            self.assertEqual(record["returncode"], 0)
            self.assertEqual(record["status"], "skip")
            self.assertTrue(record["required"])
            self.assertIn("harness did not qualify", record["skip_reason"])
            self.assertEqual(report["summary"]["outcome"], "incomplete")

    def test_optional_reported_skip_is_visible_without_blocking_qualification(self):
        gate = qualify.Gate("optional", "fixture", ("fixture",), required=False)
        profile = qualify.Profile("fixture", "fixture", (gate,))
        report = self.run_profile(profile, FakeRunner(outputs={gate.command: ("SKIP: absent fixture\n", "")}))
        self.assertEqual(report["summary"]["outcome"], "passed_with_skips")
        self.assertEqual(report["gates"][0]["status"], "skip")

    def test_execution_environment_is_explicit_and_does_not_leak_between_gates(self):
        gates = tuple(
            qualify.Gate(name, name, (name,), execution_env=execution_env)
            for name, execution_env in (
                ("native", qualify.NATIVE_CLEAN_ENV),
                ("personal", qualify.NATIVE_PERSONAL_ENV),
                ("legacy", qualify.LEGACY_ENV),
                ("native-again", qualify.NATIVE_CLEAN_ENV),
            )
        )
        runner = FakeRunner()
        report = self.run_profile(
            qualify.Profile("fixture", "fixture", gates), runner,
            env={"PATH": "/usr/bin:/bin", "AISHE_LEAN": "0", "AISHE_LEGACY_OPENCODE": "1",
                 "AISHE_ZSH_PROFILE": "invalid", "PRIVATE_FIXTURE_SECRET": "do-not-record"},
        )
        for gate, environment, record in zip(gates, runner.environments, report["gates"]):
            self.assertTrue(all(environment[key] == value for key, value in gate.execution_env))
            self.assertEqual(record["execution_env"], dict(gate.execution_env))
        self.assertNotIn("do-not-record", self.output.read_text())

    def test_profiles_cover_native_modes_and_explicit_legacy_gates(self):
        quick = qualify.PROFILES["quick"].gates
        self.assertTrue(all(gate.execution_env == qualify.NATIVE_CLEAN_ENV for gate in quick))
        self.assertFalse(any("backend" in gate.command and "install" in gate.command for gate in quick))
        for profile in qualify.PROFILES.values():
            if profile.name == "quick":
                continue
            gates = {gate.id: gate for gate in profile.gates}
            for gate_id in ("native-picker-pty", "native-mode-grants-pty", "native-cancel-pty",
                            "native-settings-pty", "native-prompts-pty"):
                self.assertEqual(gates[gate_id].execution_env, qualify.NATIVE_CLEAN_ENV)
            self.assertEqual(gates["pty-signals-personal"].execution_env, qualify.NATIVE_PERSONAL_ENV)
            self.assertEqual(gates["zsh-features-personal"].execution_env, qualify.NATIVE_PERSONAL_ENV)
            for gate_id in ("legacy-pty-smoke", "legacy-pty-scenarios", "theme-prompt-pty", "keys-pty", "pty-fuzz"):
                self.assertEqual(gates[gate_id].execution_env, qualify.LEGACY_ENV)

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

    def test_linux_release_profiles_require_functional_bubblewrap(self):
        for profile_name in ("linux-full", "release", "paid-live"):
            runner = FakeRunner()
            self.run_profile(qualify.PROFILES[profile_name], runner)
            self.assertTrue(runner.environments)
            self.assertTrue(
                all(
                    environment.get("AISHE_TEST_REQUIRE_BWRAP") == "1"
                    for environment in runner.environments
                ),
                profile_name,
            )

        runner = FakeRunner()
        self.run_profile(
            qualify.PROFILES["release"], runner, platform_name="Darwin"
        )
        self.assertTrue(
            all("AISHE_TEST_REQUIRE_BWRAP" not in env for env in runner.environments)
        )


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


class RequiredCiGateTests(unittest.TestCase):
    def test_guard_rejects_zero_exit_skips_and_preserves_actual_failures(self):
        guard = pathlib.Path(__file__).with_name("require_gate.py")
        cases = (
            ("print('SKIP (LEGACY-gated): missing explicit route')", 1),
            ("import sys; print('SKIP: no PTY', file=sys.stderr)", 1),
            ("print('PASS: the word SKIP in an explanation is harmless')", 0),
            ("raise SystemExit(7)", 7),
        )
        for script, code in cases:
            completed = subprocess.run(
                [sys.executable, str(guard), sys.executable, "-c", script],
                capture_output=True, text=True, timeout=10,
            )
            self.assertEqual(completed.returncode, code, completed.stdout + completed.stderr)


if __name__ == "__main__":
    unittest.main()
