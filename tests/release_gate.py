#!/usr/bin/env python3
"""Fail closed unless the exact release source has CI and disposition evidence."""

from __future__ import annotations

import argparse
import datetime
import hashlib
import io
import json
import os
import pathlib
import re
import subprocess
import urllib.parse
import urllib.request
import zipfile

from release_evidence import benchmark_problems, digest, production_source_digest
from qualify import PROFILE_REVISION, THREAT_MODEL_VERSION


REQUIRED_JOBS = frozenset({
    "documentation lifecycle", "dependency advisories, licenses, and sources",
    "test (ubuntu-latest)", "test (macos-latest)", "MSRV (1.88) build",
    "Linux namespace diagnostics", "Native shell (ubuntu-latest)",
    "Native shell (macos-latest)", "Explicit legacy shell (ubuntu-latest)",
    "Explicit legacy shell (macos-latest)", "Managed OpenCode runtime contracts",
})
DISPOSITIONS = frozenset({"live_providers", "terminal_manual", "long_soak"})
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def disposition_problems(record: dict, version: str, today: datetime.date, source_digest: str) -> list[str]:
    problems = []
    if record.get("schema_version") != 1 or record.get("version") != version:
        problems.append("release record schema/version does not match")
    if record.get("decision") != "ready" or not isinstance(record.get("release_owner"), str) or not record["release_owner"].strip():
        problems.append("release owner has not recorded a ready decision")
    if record.get("production_source_sha256") != source_digest:
        problems.append("release dispositions describe stale production/qualification sources")
    groups = record.get("dispositions", {})
    if not isinstance(groups, dict):
        return problems + ["release dispositions are missing"]
    for group in sorted(DISPOSITIONS):
        item = groups.get(group, {})
        state = item.get("state") if isinstance(item, dict) else None
        if state == "pass":
            evidence = item.get("evidence", {})
            if (not isinstance(evidence, dict) or not evidence.get("path")
                    or not SHA256.fullmatch(str(evidence.get("sha256", "")))
                    or evidence.get("production_source_sha256") != source_digest):
                problems.append(f"{group}: pass lacks source-bound evidence")
        elif state == "deferred":
            if not all(isinstance(item.get(key), str) and item[key].strip() for key in ("owner", "reason", "risk", "expires")):
                problems.append(f"{group}: deferral needs owner, reason, risk, expiry")
                continue
            try:
                if datetime.date.fromisoformat(item["expires"]) < today:
                    problems.append(f"{group}: deferral expired")
            except (TypeError, ValueError):
                problems.append(f"{group}: invalid deferral expiry")
        elif state == "not_applicable" and item.get("reason"):
            pass
        else:
            problems.append(f"{group}: incomplete or failed disposition")
    return problems


def pass_evidence_problems(record: dict, root: pathlib.Path, source_digest: str) -> list[str]:
    problems = []
    for group, item in record.get("dispositions", {}).items():
        if not isinstance(item, dict) or item.get("state") != "pass":
            continue
        evidence = item.get("evidence", {})
        try:
            path = (root / evidence["path"]).resolve()
            path.relative_to(root.resolve())
            if digest(path) != evidence["sha256"]:
                raise ValueError("evidence file digest differs")
            report = json.loads(path.read_text())
            if (report.get("state") != "pass" or report.get("group") != group
                    or report.get("production_source_sha256") != source_digest):
                raise ValueError("evidence is unrun/failed or describes another source")
        except (OSError, KeyError, TypeError, ValueError) as error:
            problems.append(f"{group}: invalid passing evidence: {error}")
    return problems


def ci_problems(run: dict, jobs: list[dict], repository: str, commit: str, workflow_id: int) -> list[str]:
    problems = []
    if (run.get("head_sha") != commit or run.get("event") != "push"
            or run.get("head_repository", {}).get("full_name") != repository
            or run.get("path") != ".github/workflows/ci.yml" or run.get("workflow_id") != workflow_id):
        problems.append("CI run does not identify the exact repository/source/workflow")
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        problems.append("exact-source CI did not complete successfully")
    by_name = {job.get("name"): job for job in jobs}
    for name in sorted(REQUIRED_JOBS):
        job = by_name.get(name, {})
        if job.get("status") != "completed" or job.get("conclusion") != "success":
            problems.append(f"required CI job missing or unsuccessful: {name}")
    return problems


def evidence_problems(data: bytes, platform_name: str, commit: str, version: str, expected_metadata: dict | None = None) -> list[str]:
    problems = []
    try:
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            manifests = [name for name in archive.namelist() if pathlib.PurePosixPath(name).name == f"release-evidence-{platform_name}.json"]
            if len(manifests) != 1:
                return [f"{platform_name}: one candidate identity manifest is required"]
            evidence = json.loads(archive.read(manifests[0]))
            if evidence.get("schema_version") != 1 or evidence.get("kind") != "aishe_release_evidence":
                problems.append(f"{platform_name}: invalid evidence schema")
            if expected_metadata:
                for key, value in expected_metadata.items():
                    if evidence.get(key) != value:
                        problems.append(f"{platform_name}: stale or mismatched {key}")
            identity = evidence.get("binary_identity", {})
            binary_commit = identity.get("commit", "")
            if (evidence.get("source_commit") != commit or identity.get("version") != version
                    or not re.fullmatch(r"[0-9a-f]{7,40}", binary_commit)
                    or not commit.startswith(binary_commit) or evidence.get("platform") != platform_name):
                problems.append(f"{platform_name}: tested binary identity differs from release source")
            for key in ("binary_sha256", "runtime_manifest_sha256", "plugin_sha256"):
                if not SHA256.fullmatch(str(evidence.get(key, ""))):
                    problems.append(f"{platform_name}: missing {key}")
            for key in ("qualification_profile_revision", "threat_model_version", "runtime_version"):
                if not evidence.get(key):
                    problems.append(f"{platform_name}: missing {key}")
            benchmark = evidence.get("benchmark", {})
            filenames = [name for name in archive.namelist() if pathlib.PurePosixPath(name).name == benchmark.get("file")]
            if len(filenames) != 1:
                return problems + [f"{platform_name}: strict benchmark artifact is missing"]
            payload = archive.read(filenames[0])
            if hashlib.sha256(payload).hexdigest() != benchmark.get("sha256"):
                problems.append(f"{platform_name}: benchmark digest does not match")
            report = json.loads(payload)
            if (report.get("source_commit") != commit or report.get("source_dirty") is not False
                    or report.get("binary_identity") != identity or report.get("binary_sha256") != evidence.get("binary_sha256")):
                problems.append(f"{platform_name}: strict benchmark describes a different/dirty binary or source")
            problems.extend(f"{platform_name}: {problem}" for problem in benchmark_problems(report))
    except (zipfile.BadZipFile, KeyError, TypeError, ValueError, RuntimeError) as error:
        problems.append(f"{platform_name}: invalid artifact: {error}")
    return problems


class SafeRedirect(urllib.request.HTTPRedirectHandler):
    """GitHub artifact redirects must never send the token to blob storage."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if urllib.parse.urlsplit(newurl).scheme != "https":
            raise ValueError("release evidence redirect must use HTTPS")
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is not None and urllib.parse.urlsplit(req.full_url).netloc != urllib.parse.urlsplit(newurl).netloc:
            redirected.remove_header("Authorization")
        return redirected


class GitHub:
    def __init__(self, repository: str, token: str):
        self.base = f"https://api.github.com/repos/{repository}"
        self.opener = urllib.request.build_opener(SafeRedirect())
        self.token = token

    def get(self, path: str, *, raw: bool = False):
        request = urllib.request.Request(self.base + path, headers={
            "Accept": "application/vnd.github+json", "User-Agent": "aishe-release-gate",
            "Authorization": f"Bearer {self.token}", "X-GitHub-Api-Version": "2022-11-28",
        })
        with self.opener.open(request, timeout=45) as response:
            payload = response.read(32 * 1024 * 1024 + 1)
        if len(payload) > 32 * 1024 * 1024:
            raise ValueError("release evidence response exceeded 32 MiB")
        return payload if raw else json.loads(payload)

    def pages(self, path: str, key: str) -> list[dict]:
        result = []
        separator = "&" if "?" in path else "?"
        for page in range(1, 11):
            rows = self.get(f"{path}{separator}per_page=100&page={page}")[key]
            result.extend(rows)
            if len(rows) < 100:
                return result
        raise ValueError("release evidence pagination exceeded 1000 records")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--record", required=True, type=pathlib.Path)
    parser.add_argument("--output", required=True, type=pathlib.Path)
    args = parser.parse_args()
    result = {
        "schema_version": 1, "source_commit": args.commit, "version": args.version,
        "ci_run_id": None, "qualified": False, "problems": [],
    }

    def hold(message: str):
        result["problems"] = [message]
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        raise SystemExit(message)

    root = pathlib.Path(__file__).resolve().parents[1]
    actual_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if actual_commit != args.commit or not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        hold("release checkout differs from intended commit")
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True).strip():
        hold("release gate requires a clean committed candidate")
    try:
        record = json.loads(args.record.read_text())
    except (OSError, ValueError) as error:
        hold(f"release hold: missing/invalid candidate record: {error}")
    result["release_record_sha256"] = hashlib.sha256(args.record.read_bytes()).hexdigest()
    source_digest = production_source_digest(root)
    problems = disposition_problems(record, args.version, datetime.datetime.now(datetime.timezone.utc).date(), source_digest)
    problems.extend(pass_evidence_problems(record, root, source_digest))
    manifest = root / "assets/backend/opencode/runtime-manifest.json"
    expected_metadata = {
        "production_source_sha256": source_digest,
        "route_corpus_sha256": digest(root / "tests/fixtures/routing/v1.json"),
        "runtime_manifest_sha256": digest(manifest),
        "plugin_sha256": digest(root / "assets/backend/opencode/aishe-plugin.mjs"),
        "runtime_version": json.loads(manifest.read_text())["version"],
        "qualification_profile_revision": PROFILE_REVISION,
        "threat_model_version": THREAT_MODEL_VERSION,
    }
    if problems:
        hold("release hold: " + "; ".join(problems))
    token = os.environ.get("GH_TOKEN", "")
    if not token:
        hold("GH_TOKEN is required to read exact-source CI evidence")
    result["problems"] = ["qualification incomplete; remote CI evidence has not been accepted"]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    github = GitHub(args.repository, token)
    workflow_id = github.get("/actions/workflows/ci.yml")["id"]
    runs = github.pages(f"/actions/workflows/ci.yml/runs?head_sha={args.commit}&event=push", "workflow_runs")
    matching = [run for run in runs if run.get("head_sha") == args.commit]
    if not matching:
        hold("release hold: no exact-source CI run; qualify the release commit before tagging")
    run = max(matching, key=lambda item: (item["run_number"], item.get("run_attempt", 1)))
    result.update(ci_run_id=run["id"], ci_run_attempt=run.get("run_attempt", 1), ci_url=run["html_url"])
    jobs = github.pages(f"/actions/runs/{run['id']}/jobs?filter=latest", "jobs")
    problems.extend(ci_problems(run, jobs, args.repository, args.commit, workflow_id))
    artifacts = github.pages(f"/actions/runs/{run['id']}/artifacts", "artifacts")
    artifact_ids = {}
    for platform_name in ("Linux", "macOS"):
        named = [artifact for artifact in artifacts if artifact.get("name") == f"native-terminal-evidence-{platform_name}"]
        if len(named) != 1 or named[0].get("expired") is not False:
            problems.append(f"{platform_name}: exact-source native evidence is missing/expired")
            continue
        artifact = named[0]
        artifact_ids[platform_name] = artifact["id"]
        problems.extend(evidence_problems(github.get(f"/actions/artifacts/{artifact['id']}/zip", raw=True), platform_name, args.commit, args.version, expected_metadata))
    result.update({
        "schema_version": 1, "source_commit": args.commit, "version": args.version,
        "ci_run_id": run["id"], "ci_run_attempt": run.get("run_attempt", 1),
        "ci_url": run["html_url"], "native_artifact_ids": artifact_ids,
        "release_record_sha256": hashlib.sha256(args.record.read_bytes()).hexdigest(),
        "qualified": not problems, "problems": problems,
    })
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    if problems:
        raise SystemExit("release hold: " + "; ".join(problems))
    print(f"PASS: {args.commit}, CI {run['id']}, strict Linux/macOS startup gates and owner dispositions")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
