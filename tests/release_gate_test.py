#!/usr/bin/env python3
"""Regression contracts for exact-source release and original startup gates."""

import copy
import datetime
import hashlib
import io
import json
import unittest
import urllib.request
import pathlib
import tempfile
import subprocess
import zipfile

from release_evidence import benchmark_problems, production_source_digest
from release_gate import REQUIRED_JOBS, SafeRedirect, ci_problems, disposition_problems, evidence_problems, pass_evidence_problems


COMMIT = "9" * 40
TODAY = datetime.date(2026, 10, 10)


def ready_record():
    return {
        "schema_version": 1, "version": "1.1.0", "decision": "ready",
        "release_owner": "fixture-owner", "production_source_sha256": "a" * 64, "dispositions": {
            "live_providers": {"state": "pass", "evidence": {"path": "recorded-provider-report.json", "sha256": "b" * 64, "production_source_sha256": "a" * 64}},
            "terminal_manual": {"state": "deferred", "owner": "fixture-owner", "reason": "terminal unavailable", "risk": "renderer differences remain", "expires": "2026-10-24"},
            "long_soak": {"state": "not_applicable", "reason": "bounded soak covers unchanged runtime pin"},
        },
    }


def benchmark():
    return {
        "kind": "aishe_direct_shell_performance", "commands": 100, "warmup": 10,
        "raw_zsh": {"p95_ms": 12.0}, "aishe": {"p95_ms": 20.0},
        "allowed_regression_ms": 10.0, "backend_started": False, "slo_pass": True,
        "source_commit": COMMIT, "source_dirty": False,
        "binary_identity": {"version": "1.1.0", "commit": COMMIT[:8]}, "binary_sha256": "a" * 64,
    }


def artifact(platform="Linux", *, evidence_change=None, benchmark_change=None):
    report = benchmark()
    if benchmark_change:
        benchmark_change(report)
    payload = json.dumps(report).encode()
    evidence = {
        "schema_version": 1, "kind": "aishe_release_evidence", "source_commit": COMMIT,
        "binary_identity": {"version": "1.1.0", "commit": COMMIT[:8]},
        "binary_sha256": "a" * 64, "platform": platform,
        "runtime_manifest_sha256": "b" * 64, "plugin_sha256": "c" * 64,
        "qualification_profile_revision": "fixture", "threat_model_version": "fixture", "runtime_version": "1.18.27",
        "benchmark": {"file": "direct-shell-benchmark-fixture.json", "sha256": hashlib.sha256(payload).hexdigest()},
    }
    if evidence_change:
        evidence_change(evidence)
    data = io.BytesIO()
    with zipfile.ZipFile(data, "w") as archive:
        archive.writestr(f"release-evidence-{platform}.json", json.dumps(evidence))
        archive.writestr("direct-shell-benchmark-fixture.json", payload)
    return data.getvalue()


class ReleaseGateTests(unittest.TestCase):
    def test_source_digest_changes_for_code_but_not_release_record(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            (root / "src").mkdir()
            (root / "src/main.rs").write_text("fn main() {}\n")
            before = production_source_digest(root)
            (root / "docs").mkdir()
            (root / "docs/release.json").write_text('{"decision":"hold"}')
            self.assertEqual(production_source_digest(root), before)
            (root / "src/main.rs").write_text("fn main() { todo!() }\n")
            self.assertNotEqual(production_source_digest(root), before)

    def test_source_digest_includes_new_and_changed_build_helpers(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            before = production_source_digest(root)
            (root / "build_support").mkdir()
            helper = root / "build_support/apple_link.rs"
            helper.write_text("fn configure() {}\n")
            added = production_source_digest(root)
            self.assertNotEqual(added, before)
            helper.write_text("fn configure() { fallback(); }\n")
            self.assertNotEqual(production_source_digest(root), added)

    def test_source_digest_includes_new_and_changed_codegen_configuration(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            before = production_source_digest(root)
            (root / ".cargo").mkdir()
            config = root / ".cargo/config.toml"
            config.write_text('[target.aarch64-apple-darwin]\nrustflags = []\n')
            added = production_source_digest(root)
            self.assertNotEqual(added, before)
            config.write_text('[target.aarch64-apple-darwin]\nrustflags = ["-C", "llvm-args=-enable-machine-outliner=never"]\n')
            self.assertNotEqual(production_source_digest(root), added)

    def test_record_accepts_owned_current_dispositions(self):
        self.assertEqual(disposition_problems(ready_record(), "1.1.0", TODAY, "a" * 64), [])

    def test_candidate_hold_cannot_publish(self):
        record = ready_record()
        record["decision"] = "hold"
        self.assertTrue(disposition_problems(record, "1.1.0", TODAY, "a" * 64))

    def test_unrun_expired_or_unowned_disposition_blocks_release(self):
        for change in ({"state": "not_run"}, {"expires": "2026-10-09"}, {"owner": ""}):
            record = ready_record()
            record["dispositions"]["terminal_manual"].update(change)
            with self.subTest(change=change):
                self.assertTrue(disposition_problems(record, "1.1.0", TODAY, "a" * 64))

    def test_same_version_stale_source_dispositions_are_rejected(self):
        self.assertTrue(disposition_problems(ready_record(), "1.1.0", TODAY, "c" * 64))
        record = ready_record()
        record["dispositions"]["live_providers"]["evidence"]["production_source_sha256"] = "c" * 64
        self.assertTrue(disposition_problems(record, "1.1.0", TODAY, "a" * 64))

    def test_pass_file_must_record_actual_pass_for_this_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            path = root / "provider.json"
            record = ready_record()
            for state in ("pass", "not_run", "fail"):
                path.write_text(json.dumps({"group": "live_providers", "state": state, "production_source_sha256": "a" * 64}))
                record["dispositions"]["live_providers"]["evidence"].update(path="provider.json", sha256=hashlib.sha256(path.read_bytes()).hexdigest())
                problems = pass_evidence_problems(record, root, "a" * 64)
                self.assertEqual(bool(problems), state != "pass")
            record["dispositions"]["live_providers"]["evidence"]["sha256"] = "c" * 64
            self.assertTrue(pass_evidence_problems(record, root, "a" * 64))

    def test_claimed_pass_requires_evidence(self):
        record = ready_record()
        del record["dispositions"]["live_providers"]["evidence"]
        self.assertTrue(disposition_problems(record, "1.1.0", TODAY, "a" * 64))

    def test_all_exact_source_ci_jobs_are_required(self):
        run = {"head_sha": COMMIT, "event": "push", "head_repository": {"full_name": "owner/aishe"}, "path": ".github/workflows/ci.yml", "workflow_id": 42, "status": "completed", "conclusion": "success"}
        jobs = [{"name": name, "status": "completed", "conclusion": "success"} for name in REQUIRED_JOBS]
        self.assertEqual(ci_problems(run, jobs, "owner/aishe", COMMIT, 42), [])
        for field, value in (("head_sha", "1" * 40), ("event", "pull_request"), ("conclusion", "failure"), ("workflow_id", 43)):
            changed = dict(run, **{field: value})
            with self.subTest(field=field):
                self.assertTrue(ci_problems(changed, jobs, "owner/aishe", COMMIT, 42))
        self.assertTrue(ci_problems(run, jobs[1:], "owner/aishe", COMMIT, 42))
        failed = copy.deepcopy(jobs)
        failed[0]["conclusion"] = "skipped"
        self.assertTrue(ci_problems(run, failed, "owner/aishe", COMMIT, 42))

    def test_strict_original_benchmark_accepts_measured_pass(self):
        self.assertEqual(benchmark_problems(benchmark()), [])
        for platform in ("Linux", "macOS"):
            self.assertEqual(evidence_problems(artifact(platform), platform, COMMIT, "1.1.0"), [])

    def test_shorter_or_relaxed_benchmark_cannot_pass(self):
        for field, value in (("commands", 40), ("warmup", 0), ("allowed_regression_ms", 20.0), ("allowed_regression_ms", float("nan")), ("backend_started", True), ("slo_pass", False)):
            report = dict(benchmark(), **{field: value})
            with self.subTest(field=field):
                self.assertTrue(benchmark_problems(report))

    def test_diagnostic_or_false_positive_slo_cannot_pass(self):
        for latency in (29.0, float("nan"), float("inf")):
            report = benchmark()
            report["aishe"]["p95_ms"] = latency
            self.assertTrue(benchmark_problems(report))

    def test_old_binary_or_wrong_platform_evidence_cannot_pass(self):
        changes = (
            lambda evidence: evidence.update(source_commit="1" * 40),
            lambda evidence: evidence["binary_identity"].update(version="1.0.0"),
            lambda evidence: evidence["binary_identity"].update(commit="1" * 8),
            lambda evidence: evidence.update(platform="macOS"),
            lambda evidence: evidence.update(binary_sha256="missing"),
        )
        for change in changes:
            self.assertTrue(evidence_problems(artifact(evidence_change=change), "Linux", COMMIT, "1.1.0"))

    def test_tampered_benchmark_or_invalid_zip_is_rejected(self):
        data = artifact(evidence_change=lambda evidence: evidence["benchmark"].update(sha256="d" * 64))
        self.assertTrue(evidence_problems(data, "Linux", COMMIT, "1.1.0"))
        self.assertTrue(evidence_problems(b"not an archive", "Linux", COMMIT, "1.1.0"))

    def test_strict_benchmark_must_identify_the_same_clean_candidate(self):
        for field, value in (("source_commit", "1" * 40), ("source_dirty", True), ("binary_sha256", "d" * 64)):
            data = artifact(benchmark_change=lambda report: report.update({field: value}))
            self.assertTrue(evidence_problems(data, "Linux", COMMIT, "1.1.0"))

    def test_artifact_redirect_strips_token_on_other_hosts(self):
        request = urllib.request.Request("https://api.github.com/artifact", headers={"Authorization": "Bearer fixture"})
        redirect = SafeRedirect().redirect_request(request, None, 302, "Found", {}, "https://blob.example/evidence.zip")
        self.assertIsNone(redirect.get_header("Authorization"))
        with self.assertRaises(ValueError):
            SafeRedirect().redirect_request(request, None, 302, "Found", {}, "http://api.github.com/evidence.zip")
        same = SafeRedirect().redirect_request(request, None, 302, "Found", {}, "https://api.github.com/other")
        self.assertEqual(same.get_header("Authorization"), "Bearer fixture")


if __name__ == "__main__":
    unittest.main()
