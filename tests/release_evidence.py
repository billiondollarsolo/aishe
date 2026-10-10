#!/usr/bin/env python3
"""Record the tested native binary and its unchanged direct-shell SLO evidence."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import math
import pathlib
import platform
import subprocess

from harness_identity import parse_binary_identity, require_current_binary
from qualify import PROFILE_REVISION, THREAT_MODEL_VERSION


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def production_source_digest(root: pathlib.Path) -> str:
    """Hash tracked/new production and qualification files; exclude release prose.

    Paths and byte lengths delimit each file, making the hash independent of
    timestamps, checkout location, permissions, and release-record self-reference.
    """
    paths = subprocess.check_output([
        "git", "ls-files", "--cached", "--others", "--exclude-standard", "-z", "--",
        "src", "assets", "tests", "Cargo.toml", "Cargo.lock", "build.rs", "install.sh",
        ".github/workflows/ci.yml",
    ], cwd=root).decode().split("\0")
    result = hashlib.sha256()
    for name in sorted(set(filter(None, paths))):
        path = root / name
        if not path.is_file() or "__pycache__" in path.parts:
            continue
        payload = path.read_bytes()
        result.update(name.encode() + b"\0" + str(len(payload)).encode() + b"\0" + payload)
    return result.hexdigest()


def benchmark_problems(report: dict) -> list[str]:
    problems = []
    if report.get("kind") != "aishe_direct_shell_performance":
        problems.append("wrong benchmark kind")
    if report.get("commands") != 100 or report.get("warmup") != 10:
        problems.append("required benchmark needs 100 commands and 10 warmups")
    if report.get("backend_started") is not False:
        problems.append("direct commands started the managed backend")
    try:
        raw = float(report["raw_zsh"]["p95_ms"])
        actual = float(report["aishe"]["p95_ms"])
        allowed = max(10.0, raw * 0.10)
        recorded_allowed = float(report["allowed_regression_ms"])
        if not math.isfinite(recorded_allowed) or not math.isfinite(raw) or not math.isfinite(actual) or raw < 0 or actual < 0 or abs(recorded_allowed - allowed) > 0.01:
            problems.append("benchmark threshold changed")
        if max(0.0, actual - raw) > allowed + 0.002 or report.get("slo_pass") is not True:
            problems.append("direct-shell startup SLO failed")
    except (KeyError, TypeError, ValueError):
        problems.append("benchmark latency evidence is missing or invalid")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    root = pathlib.Path(__file__).resolve().parents[1]
    binary = pathlib.Path(require_current_binary(args.binary, root=root))
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip():
        raise SystemExit("release evidence requires a clean committed candidate")
    identity = parse_binary_identity(subprocess.check_output([str(binary), "--version"], text=True))
    binary_digest = digest(binary)
    candidates = sorted((root / "test-results").glob("direct-shell-benchmark-*.json"), key=lambda p: p.stat().st_mtime_ns)
    if not candidates:
        raise SystemExit("release evidence missing the strict direct-shell benchmark")
    benchmark = candidates[-1]
    report = json.loads(benchmark.read_text())
    problems = benchmark_problems(report)
    if pathlib.Path(report.get("binary", "")).resolve() != binary:
        problems.append("benchmark did not run the identified release binary")
    if (report.get("source_commit") != commit or report.get("source_dirty") is not False
            or report.get("binary_identity") != identity or report.get("binary_sha256") != binary_digest):
        problems.append("benchmark binary/source identity does not match the clean candidate")
    if problems:
        raise SystemExit("release evidence rejected: " + "; ".join(problems))
    manifest = root / "assets/backend/opencode/runtime-manifest.json"
    record = {
        "schema_version": 1,
        "kind": "aishe_release_evidence",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "source_commit": commit,
        "production_source_sha256": production_source_digest(root),
        "route_corpus_sha256": digest(root / "tests/fixtures/routing/v1.json"),
        "binary_identity": identity,
        "binary_sha256": binary_digest,
        "platform": "macOS" if platform.system() == "Darwin" else platform.system(),
        "qualification_profile_revision": PROFILE_REVISION,
        "threat_model_version": THREAT_MODEL_VERSION,
        "runtime_version": json.loads(manifest.read_text())["version"],
        "runtime_manifest_sha256": digest(manifest),
        "plugin_sha256": digest(root / "assets/backend/opencode/aishe-plugin.mjs"),
        "benchmark": {"file": benchmark.name, "sha256": digest(benchmark)},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(f"PASS: candidate {commit}, {identity['version']}, {record['platform']}, strict startup evidence recorded")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
