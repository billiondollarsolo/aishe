#!/usr/bin/env python3
"""Deterministic contract tests for PERF-001/002 harnesses."""

from __future__ import annotations

import json
import pathlib
import platform
import shutil
import stat
import tempfile
import unittest

import lazy_loading_test
import performance_benchmark


class PerformanceReportTests(unittest.TestCase):
    def test_report_reader_requires_version_and_kind(self):
        with tempfile.TemporaryDirectory() as text:
            path = pathlib.Path(text) / "report.json"
            path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "kind": "aishe_pure_performance",
                    }
                ),
                encoding="utf-8",
            )
            report = performance_benchmark.read_report(
                path, "aishe_pure_performance"
            )
            self.assertEqual(report["schema_version"], 1)
            with self.assertRaises(AssertionError):
                performance_benchmark.read_report(path, "wrong_kind")

    def test_backend_evidence_never_disguises_an_unrun_fixture_as_pass(self):
        evidence = performance_benchmark.backend_evidence(None)
        self.assertEqual(evidence["status"], "not_run")
        self.assertIn("opencode_soak.py", evidence["fixture_command"])

    def test_backend_evidence_accepts_only_the_versioned_soak_contract(self):
        fields = {
            "runtime_version": "fixture",
            "managed_start_ms": 1,
            "cold_ready_p95_ms": 2,
            "cold_turn_p95_ms": 3,
            "warm_health_p95_ms": 4,
            "supervisor_rss_max_kib": 5,
            "opencode_rss_max_kib": 6,
            "opencode_rss_growth_kib": 7,
        }
        with tempfile.TemporaryDirectory() as text:
            path = pathlib.Path(text) / "backend.json"
            path.write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "kind": "aishe_backend_performance",
                        "thresholds": {"fixture": {"pass": True}},
                        **fields,
                    }
                ),
                encoding="utf-8",
            )
            evidence = performance_benchmark.backend_evidence(path)
            self.assertEqual(evidence["status"], "measured")
            self.assertEqual(evidence["metrics"], fields)
            self.assertTrue(evidence["source_thresholds"]["fixture"]["pass"])

    def test_argument_defaults_are_bounded_but_statistically_useful(self):
        args = performance_benchmark.parse_arguments(["target/release/aishe"])
        self.assertGreaterEqual(args.samples, 20)
        self.assertGreaterEqual(args.commands, 20)
        self.assertGreaterEqual(args.warmup, 0)

    def test_initial_prompt_probe_records_spawn_to_visible_prompt(self):
        with tempfile.TemporaryDirectory() as text:
            root = pathlib.Path(text)
            binary = root / "fake-aishe"
            binary.write_text(
                "#!/bin/sh\nprintf 'AISHE_PERF_PROMPT> '\nexec sleep 30\n",
                encoding="utf-8",
            )
            binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
            report = performance_benchmark.initial_pty_prompt_probe(
                str(binary), root / "fixture", samples=2
            )
            self.assertEqual(report["classification"], "informational")
            self.assertEqual(report["samples"], 2)
            self.assertGreater(report["p50_ms"], 0)
            self.assertGreaterEqual(report["p95_ms"], report["p50_ms"])

    def test_rss_probe_executes_every_declared_surface(self):
        with tempfile.TemporaryDirectory() as text:
            root = pathlib.Path(text)
            binary = root / "fake-aishe"
            binary.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
            binary.chmod(binary.stat().st_mode | stat.S_IXUSR)
            report = performance_benchmark.measure_rss(str(binary), {})
            self.assertEqual(report["classification"], "informational")
            self.assertGreater(report["max_rss_kib"], 0)
            self.assertEqual(
                set(report["surfaces"]), {"shell", "help", "route", "status"}
            )
            for record in report["surfaces"].values():
                self.assertEqual(record["returncode"], 0)
                self.assertGreater(record["rss_kib"], 0)


class LazyLoadingInstrumentationTests(unittest.TestCase):
    def test_audit_parser_rejects_unknown_operations(self):
        with tempfile.TemporaryDirectory() as text:
            path = pathlib.Path(text) / "audit.jsonl"
            path.write_text(
                '{"pid":1,"operation":"connect","fd":3,"family":2}\n',
                encoding="utf-8",
            )
            self.assertEqual(
                lazy_loading_test.parse_audit(path)[0]["operation"], "connect"
            )
            path.write_text(
                '{"pid":1,"operation":"open","fd":3,"family":2}\n',
                encoding="utf-8",
            )
            with self.assertRaises(AssertionError):
                lazy_loading_test.parse_audit(path)

    @unittest.skipUnless(
        platform.system() in {"Darwin", "Linux"} and shutil.which("cc"),
        "dynamic-library audit requires macOS/Linux and cc",
    )
    def test_network_audit_self_test_proves_connect_interposition(self):
        with tempfile.TemporaryDirectory() as text:
            audit = lazy_loading_test.NetworkAudit(pathlib.Path(text))
            self.assertTrue(audit.library.is_file())

    def test_lazy_fixture_makes_provider_construction_observable(self):
        with tempfile.TemporaryDirectory() as text:
            root = pathlib.Path(text)
            lazy_loading_test.write_config(root, 12345)
            config = (root / "config/aishe/config.toml").read_text(encoding="utf-8")
            self.assertIn('api_key_env = "AISHE_LAZY_PROVIDER_KEY"', config)
            self.assertIn("auth_required = true", config)
            self.assertIn("http://127.0.0.1:12345", config)


if __name__ == "__main__":
    unittest.main()
