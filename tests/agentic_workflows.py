#!/usr/bin/env python3
"""Observe real native workflow, patch-review, and timeline effects.

Only private Git projects and a deterministic loopback Responses provider are
used. Provider statements are deliberately dishonest in one scenario so that
the required-check gate must rely on recorded execution.
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import shlex
import subprocess
import time
import traceback

from native_task_interactions import BINARY, Fixture, input_text, response, tool, worker_alive, kill_worker


def git(fixture, *args):
    result = subprocess.run(["git", "-C", str(fixture.work), *args], env=fixture.env,
                            capture_output=True, text=True, timeout=12)
    assert result.returncode == 0, result.stdout + result.stderr
    return result.stdout.strip()


def project(fixture):
    git(fixture, "init", "-q")
    git(fixture, "config", "user.name", "Private Workflow Fixture")
    git(fixture, "config", "user.email", "workflow@localhost")
    (fixture.work / "first.txt").write_text("first original\n")
    (fixture.work / "second.txt").write_text("second original\n")
    (fixture.work / "hunks.txt").write_text("\n".join(f"line {index:03}" for index in range(40)) + "\n")
    git(fixture, "add", ".")
    git(fixture, "commit", "-qm", "Private fixture base")


def isolated_start(fixture, objective):
    result = fixture.cli("task", "start", "--max-minutes", "1", "--max-turns", "20", objective)
    match = re.search(r"started task (\S+)", result.stdout)
    assert match, result.stdout + result.stderr
    fixture.task_ids.append(match.group(1))
    return match.group(1)


def refused(fixture, *args):
    calls = len(fixture.loopback.calls)
    result = fixture.cli(*args, success=False)
    assert result.returncode != 0, (args, result.stdout, result.stderr)
    assert len(fixture.loopback.calls) == calls, "rejected read/control action initialized a provider"
    return result


def review(fixture, task_id):
    value = json.loads(fixture.cli("task", "review", task_id, "--json").stdout)
    assert re.fullmatch(r"[0-9a-f]{64}", value["revision"]), value
    return value


def file_by_path(value, name):
    matches = [row for row in value["files"] if row.get("path", row.get("new_path")) == name]
    assert len(matches) == 1, (name, value)
    return matches[0]


def selected_changes_are_revision_bound_and_apply_only_once():
    def choose(index, payload):
        if index == 0:
            script = "\n".join([
                "import pathlib",
                "pathlib.Path('first.txt').write_text('FIRST_REVIEW_EFFECT\\n')",
                "pathlib.Path('second.txt').write_text('SECOND_REVIEW_EFFECT\\n')",
                "p=pathlib.Path('hunks.txt'); lines=p.read_text().splitlines()",
                "lines[1]='FIRST_HUNK_EFFECT'; lines[36]='LAST_HUNK_EFFECT'",
                "p.write_text('\\n'.join(lines)+'\\n')",
            ])
            return response(tool("run_command", {"command": "python3 -c " + shlex.quote(script)}, "make-review-changes"))
        if index == 1:
            return response(tool("run_check", {"command": "test -s first.txt && test -s second.txt"}, "check-review-changes"))
        assert index == 2, index
        return response(text="Selection fixture has two files and two distant hunks")

    fixture = Fixture("selective-review", choose)
    try:
        project(fixture)
        task_id = isolated_start(fixture, "SELECTED_CHANGES_PROOF edit the private fixture")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        assert (fixture.work / "first.txt").read_text() == "first original\n", "isolated task wrote directly to the source"
        initial = review(fixture, task_id)
        first = file_by_path(initial, "first.txt")
        hunks = file_by_path(initial, "hunks.txt")["hunks"]
        assert len(hunks) == 2, hunks
        refused(fixture, "task", "apply", task_id, "--revision", "0" * 64, "--file", str(first["id"]))
        fixture.cli("task", "apply", task_id, "--revision", initial["revision"], "--file", str(first["id"]))
        assert (fixture.work / "first.txt").read_text() == "FIRST_REVIEW_EFFECT\n"
        assert (fixture.work / "second.txt").read_text() == "second original\n"
        assert "HUNK_EFFECT" not in (fixture.work / "hunks.txt").read_text()
        assert fixture.show(task_id)["state"] != "applied", "partial apply hid remaining changes"
        refused(fixture, "task", "apply", task_id, "--revision", initial["revision"], "--hunk", str(hunks[0]["id"]))
        current = review(fixture, task_id)
        assert current["revision"] != initial["revision"], "partial apply did not invalidate the viewed revision"
        refused(fixture, "task", "apply", task_id, "--revision", current["revision"], "--file", str(first["id"]))
        fixture.cli("task", "apply", task_id, "--revision", current["revision"], "--hunk", str(hunks[0]["id"]))
        lines = (fixture.work / "hunks.txt").read_text().splitlines()
        assert lines[1] == "FIRST_HUNK_EFFECT" and lines[36] == "line 036", lines
        current = review(fixture, task_id)
        (fixture.work / "second.txt").write_text("USER_EDIT_AFTER_REVIEW\n")
        second = file_by_path(current, "second.txt")
        refused(fixture, "task", "apply", task_id, "--revision", current["revision"], "--file", str(second["id"]))
        assert (fixture.work / "second.txt").read_text() == "USER_EDIT_AFTER_REVIEW\n", "stale review overwrote a user change"
        (fixture.work / "second.txt").write_text("second original\n")
        current = review(fixture, task_id)
        fixture.cli("task", "apply", task_id, "--revision", current["revision"])
        assert (fixture.work / "second.txt").read_text() == "SECOND_REVIEW_EFFECT\n"
        assert (fixture.work / "hunks.txt").read_text().splitlines()[36] == "LAST_HUNK_EFFECT"
        assert fixture.show(task_id)["state"] == "applied"
        refused(fixture, "task", "apply", task_id, "--revision", review(fixture, task_id)["revision"])
        assert len(fixture.loopback.calls) == 3, "reviewing/applying initialized another provider turn"
        fixture.loopback.assert_ok()
        print("  ok   file/hunk selections affect only selected source lines; stale/repeated reviews reject safely", flush=True)
    finally:
        fixture.close()


def actual_timeline_is_private_and_does_not_promote_model_claims():
    secret = "sk-proj-PRIVATE_TIMELINE_CREDENTIAL_12345678901234567890"

    def choose(index, payload):
        if index == 0:
            return response(tool("run_check", {"command": "printf 'TIMELINE_CHECK_EFFECT\\n'; exit 7"}, "timeline-observed-check"))
        assert index == 1, index
        return response(text="I ran a hundred checks successfully. MODEL_CLAIM_SHOULD_NOT_BE_AN_EVENT " + secret)

    fixture = Fixture("timeline", choose)
    try:
        fixture.env["PRIVATE_TIMELINE_API_TOKEN"] = secret
        task_id = fixture.start("Timeline of observed work")
        fixture.finish(task_id)
        value = json.loads(fixture.cli("task", "timeline", task_id, "--json").stdout)
        events = value.get("events", value.get("timeline", [])) if isinstance(value, dict) else value
        assert events, value
        kinds = {row["kind"] for row in events}
        assert {"task_started", "provider_turn", "tool_started", "tool_result", "check_result", "finished"} <= kinds, kinds
        observed = [row for row in events if row["kind"] == "check_result"]
        assert len(observed) == 1, observed
        assert "7" in json.dumps(observed), "failed observed exit is missing from the timeline"
        serialized = json.dumps(value)
        assert secret not in serialized, "timeline exposed a credential"
        assert "MODEL_CLAIM_SHOULD_NOT_BE_AN_EVENT" not in serialized, "model prose became factual activity evidence"
        assert len(fixture.loopback.calls) == 2, "reading the timeline started model work"
        fixture.loopback.assert_ok()
        print("  ok   timeline records observed tools/check exits and keeps model claims and credentials out", flush=True)
    finally:
        fixture.close()


def budget():
    return {"max_minutes": 1, "max_provider_turns": 10, "max_cost_usd": 0,
            "max_tool_calls": 10, "max_changed_files": 10, "max_changed_bytes": 100000,
            "max_network_calls": 10}


def stage(key, objective, dependencies=(), checks=()):
    return {"key": key, "name": key.title(), "objective": objective, "depends_on": list(dependencies),
            "required_checks": list(checks), "budget": budget(), "scope": "host", "network": "allow"}


def save_workflow(fixture, name, stages, *, parameters=()):
    value = {"schema_version": 1, "name": name, "description": "Private qualification workflow",
             "max_parallel": 2, "parameters": list(parameters), "stages": stages}
    path = fixture.root / (name + ".json")
    path.write_text(json.dumps(value))
    return fixture.cli("workflow", "save", name, "--file", str(path))


def run_workflow(fixture, name, *args):
    result = fixture.cli("workflow", "run", name, *args)
    ids = re.findall(r"\b(?:wf|workflow)-[A-Za-z0-9_-]+\b", result.stdout)
    if ids:
        return ids[-1]
    # The public run record is also the authoritative observation when the
    # human launch text changes. Each fixture starts only one workflow run.
    root = fixture.data / "aishe/workflows/runs"
    records = list(root.glob("*/record.json"))
    assert len(records) == 1, result.stdout + result.stderr
    return records[0].parent.name


def workflow_record(fixture, run_id):
    return json.loads(fixture.cli("workflow", "show", run_id, "--json").stdout)


def finished_workflow(fixture, run_id, timeout=20):
    deadline = time.monotonic() + timeout
    latest = None
    while time.monotonic() < deadline:
        latest = workflow_record(fixture, run_id)
        rows = latest.get("stages", [])
        if rows and latest.get("state") in {"completed", "blocked", "cancelled"}:
            for row in rows:
                task_id = row.get("task_id")
                if task_id and task_id not in fixture.task_ids:
                    fixture.task_ids.append(task_id)
            return latest
        time.sleep(.08)
    raise AssertionError("workflow did not settle: " + repr(latest))


def workflow_dependencies_use_real_snapshots_and_recorded_checks():
    def choose(_, payload):
        history = input_text(payload)
        if "VERIFY_STAGE_OBJECTIVE" not in history and "BUILD_STAGE_OBJECTIVE" in history:
            if "build-write" not in history:
                return response(tool("run_command", {"command": "printf 'DEPENDENCY_ARTIFACT_EFFECT\\n' > artifact.txt"}, "build-write"))
            if "build-real-check" not in history:
                return response(tool("run_check", {"command": "test -s artifact.txt"}, "build-real-check"))
            return response(text="Build stage completed")
        assert "VERIFY_STAGE_OBJECTIVE" in history, history
        if "verify-real-check" not in history:
            return response(tool("run_check", {"command": "grep -q DEPENDENCY_ARTIFACT_EFFECT artifact.txt"}, "verify-real-check"))
        return response(text="Dependency snapshot was checked")

    fixture = Fixture("workflow-dependency", choose)
    try:
        project(fixture)
        save_workflow(fixture, "fixture-pipeline", [stage("build", "BUILD_STAGE_OBJECTIVE", checks=["test -s artifact.txt"]),
                                                    stage("verify", "VERIFY_STAGE_OBJECTIVE", ["build"],
                                                          ["grep -q DEPENDENCY_ARTIFACT_EFFECT artifact.txt"])])
        run_id = run_workflow(fixture, "fixture-pipeline")
        result = finished_workflow(fixture, run_id)
        rows = {row["key"]: row for row in result["stages"]}
        assert all(row.get("state", row.get("status")) == "completed" for row in rows.values()), result
        assert not (fixture.work / "artifact.txt").exists(), "workflow stage wrote to the source rather than an isolated worktree"
        build_record = fixture.show(rows["build"]["task_id"])
        verify_record = fixture.show(rows["verify"]["task_id"])
        assert build_record["run_cwd"] != verify_record["run_cwd"], "dependent stages reused a writer's workspace"
        assert (Path(verify_record["run_cwd"]) / "artifact.txt").read_text() == "DEPENDENCY_ARTIFACT_EFFECT\n"
        for record in (build_record, verify_record):
            evidence = [row for row in fixture.checkpoint(record)["evidence"] if row["kind"] == "check"]
            assert len(evidence) == 1 and evidence[0]["exit_code"] == 0, evidence
        # Reviewing the final stage must include the implementation inherited
        # from its dependency, not merely its own (empty) verification diff.
        leaf = review(fixture, rows["verify"]["task_id"])
        artifact = file_by_path(leaf, "artifact.txt")
        fixture.cli("task", "apply", rows["verify"]["task_id"], "--revision", leaf["revision"], "--file", str(artifact["id"]))
        assert (fixture.work / "artifact.txt").read_text() == "DEPENDENCY_ARTIFACT_EFFECT\n", "reviewing the checked leaf omitted its upstream implementation"
        assert len(fixture.loopback.calls) == 5, "workflow duplicated provider work or omitted a stage"
        fixture.loopback.assert_ok()
        print("  ok   dependency stages receive isolated committed snapshots and require actual passing checks", flush=True)
    finally:
        fixture.close()


def workflow_validation_and_required_checks_fail_closed():
    def choose(_, payload):
        assert "CLAIM_ONLY_STAGE" in input_text(payload), input_text(payload)
        return response(text="I ran test -f required-proof and it passed. This claim has no tool evidence.")

    fixture = Fixture("workflow-gates", choose)
    try:
        project(fixture)
        value = {"schema_version": 1, "name": "cyclic-fixture", "description": "Invalid fixture", "max_parallel": 2,
                 "parameters": [], "stages": [stage("one", "cycle one", ["two"]), stage("two", "cycle two", ["one"])]}
        path = fixture.root / "cycle.json"
        path.write_text(json.dumps(value))
        refused(fixture, "workflow", "save", "cyclic-fixture", "--file", str(path))
        value["stages"] = [stage("one", "Unknown prerequisite", ["missing"])]
        path.write_text(json.dumps(value))
        refused(fixture, "workflow", "save", "cyclic-fixture", "--file", str(path))
        save_workflow(fixture, "claim-only-fixture", [stage("claim", "CLAIM_ONLY_STAGE", checks=["test -f required-proof"]),
                                                      stage("downstream", "DOWNSTREAM_MUST_NOT_START", ["claim"])])
        result = finished_workflow(fixture, run_workflow(fixture, "claim-only-fixture"))
        rows = {row["key"]: row for row in result["stages"]}
        assert rows["claim"].get("state", rows["claim"].get("status")) == "failed", result
        assert rows["downstream"].get("state", rows["downstream"].get("status")) == "blocked", result
        downstream = fixture.show(rows["downstream"]["task_id"])
        assert downstream["state"] == "blocked" and downstream.get("native_task_id") is None, \
            "scheduler launched a stage behind missing evidence: " + repr(downstream)
        assert len(fixture.loopback.calls) == 1, "missing required check caused a downstream provider request"
        fixture.loopback.assert_ok()
        print("  ok   cycles/unknown dependencies reject; assistant check claims cannot open a downstream gate", flush=True)
    finally:
        fixture.close()


def workflow_parameters_are_literal_and_do_not_execute():
    literal = "$(touch PARAMETER_MUST_NOT_EXECUTE); `touch SECOND_PARAMETER_MUST_NOT_EXECUTE`"

    def choose(_, payload):
        assert literal in input_text(payload), input_text(payload)
        return response(text="Literal parameter received as objective text")

    fixture = Fixture("workflow-parameters", choose)
    try:
        project(fixture)
        save_workflow(fixture, "literal-fixture", [stage("literal", "Discuss {{topic}} literally")],
                      parameters=[{"name": "topic", "description": "Text to discuss", "default": None}])
        refused(fixture, "workflow", "run", "literal-fixture")
        refused(fixture, "workflow", "run", "literal-fixture", "--param", "unknown=value")
        result = finished_workflow(fixture, run_workflow(fixture, "literal-fixture", "--param", "topic=" + literal))
        assert result["stages"][0].get("state", result["stages"][0].get("status")) == "completed", result
        assert not list(fixture.root.rglob("*PARAMETER_MUST_NOT_EXECUTE*")), "template parameter was evaluated by a shell"
        fixture.loopback.assert_ok()
        print("  ok   workflow parameters are literal objective text; missing/unknown bindings reject before launch", flush=True)
    finally:
        fixture.close()


def workflow_parallelism_and_scheduler_restart_do_not_duplicate_workers():
    holder = {}

    def choose(_, payload):
        history = input_text(payload)
        key = next((key for key in ("one", "two", "three") if "PARALLEL_STAGE_" + key in history), None)
        assert key, history
        if "parallel-effect-" + key not in history:
            script = "\n".join([
                "import pathlib,time",
                f"pathlib.Path({str(holder['root'] / ('entered-' + key))!r}).write_text(str(pathlib.Path.cwd()))",
                f"pathlib.Path('same-writer-path.txt').write_text({key!r})",
                "deadline=time.monotonic()+20",
                f"while not pathlib.Path({str(holder['release'])!r}).exists():",
                " assert time.monotonic()<deadline,'parallel fixture not released'",
                " time.sleep(.03)",
            ])
            return response(tool("run_command", {"command": "python3 -c " + shlex.quote(script)}, "parallel-effect-" + key))
        return response(text="Isolated parallel stage " + key + " completed")

    fixture = Fixture("workflow-parallel-restart", choose)
    try:
        project(fixture)
        holder.update(root=fixture.root, release=fixture.root / "release-parallel-fixture")
        save_workflow(fixture, "parallel-fixture", [stage(key, "PARALLEL_STAGE_" + key) for key in ("one", "two", "three")])
        run_id = run_workflow(fixture, "parallel-fixture")
        initial = workflow_record(fixture, run_id)
        fixture.task_ids.extend(row["task_id"] for row in initial["stages"])
        deadline = time.monotonic() + 12
        while len(list(fixture.root.glob("entered-*"))) < 2 and time.monotonic() < deadline:
            time.sleep(.05)
        entered = list(fixture.root.glob("entered-*"))
        assert len(entered) == 2, "max_parallel=2 did not bound actual concurrent effects: " + repr(entered)
        workspaces = [path.read_text() for path in entered]
        assert len(set(workspaces)) == 2 and all(path != str(fixture.work) for path in workspaces), workspaces
        before = workflow_record(fixture, run_id)
        active = [fixture.show(row["task_id"]) for row in before["stages"] if row["state"] == "running"]
        assert len(active) == 2 and len(fixture.loopback.calls) == 2, before
        scheduler = {"pid": before["scheduler_pid"], "process_start": before["scheduler_process_start"]}
        assert worker_alive(scheduler), before
        kill_worker(scheduler)
        fixture.cli("workflow", "resume", run_id)
        deadline = time.monotonic() + 8
        fresh = None
        while time.monotonic() < deadline:
            fresh = workflow_record(fixture, run_id)
            if fresh.get("scheduler_pid") and fresh["scheduler_pid"] != scheduler["pid"]:
                break
            time.sleep(.05)
        assert fresh and fresh.get("scheduler_pid") != scheduler["pid"], fresh
        assert len(fixture.loopback.calls) == 2, "scheduler restart duplicated active worker/provider work"
        for original in active:
            current = fixture.show(original["id"])
            assert current["pid"] == original["pid"] and current["native_task_id"] == original["native_task_id"], current
        holder["release"].touch()
        result = finished_workflow(fixture, run_id)
        assert result["state"] == "completed" and len(list(fixture.root.glob("entered-*"))) == 3, result
        for row in result["stages"]:
            record = fixture.show(row["task_id"])
            checkpoint = fixture.checkpoint(record)
            assert checkpoint["execution"]["provider_turns"] == 2 and checkpoint["execution"]["tool_calls"] == 1, checkpoint
            assert (Path(record["run_cwd"]) / "same-writer-path.txt").read_text() == row["key"]
        assert not (fixture.work / "same-writer-path.txt").exists(), "parallel writers collided in the user's source workspace"
        assert len(fixture.loopback.calls) == 6, "restart replayed a provider or tool turn"
        fixture.loopback.assert_ok()
        print("  ok   parallel work is bounded and isolated; scheduler restart retains exact active workers and effects", flush=True)
    finally:
        if "release" in holder:
            holder["release"].touch()
        fixture.close()


def workflow_cancellation_prevents_late_effects_and_dependent_release():
    holder = {}

    def choose(_, payload):
        history = input_text(payload)
        assert "CANCEL_FIRST_STAGE" in history and "CANCEL_DOWNSTREAM_STAGE" not in history, history
        script = "\n".join([
            "import pathlib,time",
            f"pathlib.Path({str(holder['started'])!r}).write_text('started')",
            "deadline=time.monotonic()+15",
            f"while not pathlib.Path({str(holder['release'])!r}).exists():",
            " assert time.monotonic()<deadline,'cancel fixture not released'",
            " time.sleep(.03)",
            f"pathlib.Path({str(holder['late'])!r}).write_text('late effect')",
        ])
        return response(tool("run_command", {"command": "python3 -c " + shlex.quote(script)}, "workflow-cancellable-effect"))

    fixture = Fixture("workflow-cancel", choose)
    try:
        project(fixture)
        holder.update(started=fixture.root / "cancel-started", release=fixture.root / "cancel-release", late=fixture.root / "cancel-late")
        save_workflow(fixture, "cancel-fixture", [stage("first", "CANCEL_FIRST_STAGE"), stage("second", "CANCEL_DOWNSTREAM_STAGE", ["first"])])
        run_id = run_workflow(fixture, "cancel-fixture")
        rows = workflow_record(fixture, run_id)["stages"]
        fixture.task_ids.extend(row["task_id"] for row in rows)
        deadline = time.monotonic() + 12
        while not holder["started"].exists() and time.monotonic() < deadline:
            time.sleep(.05)
        assert holder["started"].exists(), "workflow's cancellable command never started"
        fixture.cli("workflow", "cancel", run_id)
        result = finished_workflow(fixture, run_id)
        assert result["state"] == "cancelled", result
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            records = [fixture.show(row["task_id"]) for row in rows]
            if all(record["state"] == "cancelled" and not worker_alive(record) for record in records):
                break
            time.sleep(.05)
        assert all(record["state"] == "cancelled" and not worker_alive(record) for record in records), records
        holder["release"].touch()
        time.sleep(.2)
        assert not holder["late"].exists(), "workflow cancellation orphaned an in-flight tool"
        assert len(fixture.loopback.calls) == 1, "cancellation released a dependent stage"
        fixture.loopback.assert_ok()
        print("  ok   workflow cancellation kills actual in-flight tools and keeps dependent stages unstarted", flush=True)
    finally:
        if "release" in holder:
            holder["release"].touch()
        fixture.close()


def main():
    scenarios = [selected_changes_are_revision_bound_and_apply_only_once,
                 actual_timeline_is_private_and_does_not_promote_model_claims,
                 workflow_dependencies_use_real_snapshots_and_recorded_checks,
                 workflow_validation_and_required_checks_fail_closed,
                 workflow_parameters_are_literal_and_do_not_execute,
                 workflow_parallelism_and_scheduler_restart_do_not_duplicate_workers,
                 workflow_cancellation_prevents_late_effects_and_dependent_release]
    failed = []
    for scenario in scenarios:
        print("  run  " + scenario.__name__, flush=True)
        try:
            scenario()
        except Exception:
            failed.append(scenario.__name__)
            traceback.print_exc()
    if failed:
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} agentic scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} agentic workflow/review/timeline scenarios", flush=True)


if __name__ == "__main__":
    main()
