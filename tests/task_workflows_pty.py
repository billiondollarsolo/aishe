#!/usr/bin/env python3
"""User-facing review/timeline/workflow controls and bounded task history.

Actual isolated worker changes and recorded tool exits support the UI checks.
The large-history fixture uses private imported terminal records, alongside one
real active worker, to ensure observation remains bounded and editing stays
usable without invoking extra model work.
"""

from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import threading
import subprocess
import termios
import time
import traceback

from agentic_workflows import (isolated_start, project, review, save_workflow, stage,
                               run_workflow, workflow_record, finished_workflow)
from native_task_interactions import BINARY, input_text, response, tool, worker_alive
from task_interactions_pty import UiFixture, close_drawer
from background_tasks_pty import capture, probe, shown, wait_until


def timeline_view_preserves_draft_and_shows_observed_failure():
    def choose(index, payload):
        if index == 0:
            return response(tool("run_check", {"command": "printf 'TIMELINE_PTY_EXIT_PROOF\\n'; exit 11"}, "timeline-pty-check"))
        assert index == 1, index
        return response(text="I claim all checks passed. MODEL_TIMELINE_CLAIM_PROOF")

    fixture = UiFixture("timeline-drawer", choose)
    try:
        task_id = fixture.start("TIMELINE_PTY_OBJECTIVE_PROOF")
        fixture.finish(task_id)
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        shell.send("print -r -- TIMELINE_''DRAFT > \"$HOME/timeline-draft-effect\"\x01\x06\x06")
        before = probe(shell, fixture.home)
        shell.send("\x18b\r")
        shown(shell, "Task details")
        shell.send("t")
        shown(shell, "Task timeline")
        shell.send("\x06")
        shown(shell, "Timeline filter")
        shell.send("\x1b[H\x1b[B\x1b[B\r")
        shown(shell, "recorded check")
        shown(shell, "11")
        capture(shell, "Typed timeline of an observed failed check")
        shell.send("\x1b")
        shell.drain(.2)
        after = close_drawer(shell, fixture)
        assert after[:2] == before[:2], (before, after)
        assert not (fixture.home / "timeline-draft-effect").exists()
        assert len(fixture.loopback.calls) == 2, "timeline navigation started provider work"
        fixture.loopback.assert_ok()
        print("  ok   task timeline shows observed failure; returning restores the untouched editing draft", flush=True)
    finally:
        fixture.close()


def selective_review_defaults_to_no_and_applies_only_chosen_file():
    def choose(index, payload):
        if index == 0:
            return response(tool("run_command", {"command": "printf 'REVIEW_PTY_FIRST_EFFECT\\n' > first.txt; printf 'REVIEW_PTY_SECOND_EFFECT\\n' > second.txt"}, "review-pty-change"))
        assert index == 1, index
        return response(text="Review fixture is ready; checks were not run")

    fixture = UiFixture("selected-review", choose)
    try:
        project(fixture)
        task_id = isolated_start(fixture, "REVIEW_PTY_OBJECTIVE_PROOF")
        fixture.finish(task_id)
        manifest = review(fixture, task_id)
        assert len(manifest["files"]) == 2, manifest
        shell = fixture.shell(browser=task_id)
        shown(shell, "Task details")
        shell.send("a")
        shown(shell, "Review task changes")
        shown(shell, "first.txt")
        shown(shell, "second.txt")
        shown(shell, "No recorded checks")
        capture(shell, "Selective review with untested changes visible")
        shell.send("\x1b")
        shell.drain(.2)
        assert (fixture.work / "first.txt").read_text() == "first original\n"
        assert (fixture.work / "second.txt").read_text() == "second original\n"
        # The first file choice is independently reviewable. The confirmation
        # remains an explicit action after returning from the selection frame.
        shell.send("a")
        shown(shell, "Review task changes")
        shell.send("\x1b[H\r")
        shown(shell, "File changes")
        shell.send("\x1b[F\x1b[A\r")
        shown(shell, "[x] Hunk")
        shell.send("\x1b[F\r")
        shown(shell, "Review task changes")
        shell.send("\x1b[F\x1b[A\r")
        shown(shell, "selected hunks to the source repository?")
        shell.send("\r")
        shell.drain(.3)
        assert (fixture.work / "first.txt").read_text() == "first original\n", "empty confirmation applied a file"
        shell.send("\x1b[F\x1b[A\r")
        shown(shell, "selected hunks to the source repository?")
        capture(shell, "Selected change requires explicit confirmation")
        shell.send("y\r")
        wait_until(shell, lambda: (fixture.work / "first.txt").read_text() == "REVIEW_PTY_FIRST_EFFECT\n", "selected file is applied")
        assert (fixture.work / "second.txt").read_text() == "second original\n", "file selection applied another file"
        assert fixture.show(task_id)["state"] != "applied", "partial review hid the remaining change"
        assert len(fixture.loopback.calls) == 2, "review/apply initialized provider work"
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "review browser exits")
        fixture.loopback.assert_ok()
        print("  ok   untested selected changes are visible; default leaves source untouched; explicit approval applies one file", flush=True)
    finally:
        fixture.close()


def workflow_launcher_lists_saved_templates_and_exits_without_starting():
    def choose(_, payload):
        raise AssertionError("viewing/cancelling a workflow picker must not contact a provider: " + input_text(payload))

    fixture = UiFixture("workflow-launcher", choose)
    try:
        project(fixture)
        save_workflow(fixture, "pty-saved-template", [stage("discuss", "WORKFLOW_PTY_OBJECTIVE_PROOF")])
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        shell.send("/workflow\r")
        shown(shell, "Workflows")
        shown(shell, "pty-saved-template")
        capture(shell, "Saved workflow launcher")
        shell.send("\x1b")
        shell.drain(.5)
        assert not list((fixture.data / "aishe/workflows/runs").glob("*/record.json")), "cancelled picker launched a workflow"
        assert not fixture.loopback.calls, "saved workflow viewing contacted the provider"
        shell.send("print -r -- WORKFLOW_''PICKER_RETURNED > \"$HOME/workflow-picker-effect\"\r")
        wait_until(shell, lambda: (fixture.home / "workflow-picker-effect").exists(), "shell returns after workflow picker")
        assert (fixture.home / "workflow-picker-effect").read_text().strip() == "WORKFLOW_PICKER_RETURNED"
        fixture.loopback.assert_ok()
        print("  ok   /workflow shows saved templates without provider work; cancellation returns a usable shell", flush=True)
    finally:
        fixture.close()


def workflow_tree_exposes_queued_dependencies_without_disturbing_editing():
    first_received = threading.Event()
    first_release = threading.Event()

    def choose(_, payload):
        history = input_text(payload)
        if "TREE_STAGE_VERIFY" in history:
            if "tree-verify-check" not in history:
                return response(tool("run_check", {"command": "test -d ."}, "tree-verify-check"))
            return response(text="TREE_VERIFY_RESULT_PROOF")
        assert "TREE_STAGE_BUILD" in history, history
        if "tree-build-check" not in history:
            first_received.set()
            assert first_release.wait(25), "workflow tree provider not released"
            return response(tool("run_check", {"command": "test -d ."}, "tree-build-check"))
        return response(text="TREE_BUILD_RESULT_PROOF")

    fixture = UiFixture("workflow-tree", choose)
    try:
        project(fixture)
        save_workflow(fixture, "tree-fixture", [stage("build", "TREE_STAGE_BUILD", checks=["test -d ."]),
                                                stage("verify", "TREE_STAGE_VERIFY", ["build"], ["test -d ."])])
        run_id = run_workflow(fixture, "tree-fixture")
        rows = workflow_record(fixture, run_id)["stages"]
        fixture.task_ids.extend(row["task_id"] for row in rows)
        assert first_received.wait(8), "workflow's first stage never started"
        shell = fixture.shell(cols=84)
        assert shell.ready(), shell.plain()[-4000:]
        shell.send("print -r -- TREE_''DRAFT > \"$HOME/tree-draft-effect\"\x01\x06\x06")
        before = probe(shell, fixture.home)
        shell.send("\x18b")
        shown(shell, "Background tasks")
        shown(shell, "Workflow " + run_id[:8])
        shown(shell, "0/2 finished")
        shown(shell, "queued")
        shown(shell, "Build")
        shown(shell, "Verify")
        capture(shell, "Workflow task tree with running and queued stages")
        shell.send("\x1b[B\r")
        shown(shell, "Task details")
        shown(shell, "Depends on:")
        capture(shell, "Queued stage explains its dependency and required check")
        assert close_drawer(shell, fixture)[:2] == before[:2], "workflow tree navigation changed the editing draft"
        first_release.set()
        result = finished_workflow(fixture, run_id)
        assert result["state"] == "completed", result
        shell.send("\x18b")
        shown(shell, "2/2 finished", timeout=10)
        capture(shell, "Completed workflow tree with persistent results")
        shell.send("\x1b")
        shell.drain(.2)
        assert probe(shell, fixture.home)[:2] == before[:2]
        assert not (fixture.home / "tree-draft-effect").exists()
        assert len(fixture.loopback.calls) == 4, "viewing the tree duplicated stage/provider work"
        fixture.loopback.assert_ok()
        print("  ok   workflow tree shows queued dependencies and genuine completion; draft navigation adds no provider work", flush=True)
    finally:
        first_release.set()
        fixture.close()


def foreground_drawer_handoff_defaults_to_no_and_restores_terminal():
    first_received = threading.Event()
    first_release = threading.Event()

    def choose(index, payload):
        if index == 0:
            first_received.set()
            assert first_release.wait(20), "foreground drawer fixture not released"
            return response(tool("run_command", {"command": "touch DRAWER_STALE_BATCH_MUST_NOT_RUN"}, "drawer-stale-batch"))
        if index == 1:
            assert "drawer-stale-batch" in input_text(payload), input_text(payload)
            return response(tool("run_check", {"command": "printf 'once\\n' >> drawer-foreground-effect"}, "drawer-foreground-proof"))
        assert index == 2, index
        return response(text="DRAWER_FOREGROUND_RESULT_PROOF")

    fixture = UiFixture("foreground-drawer", choose)
    try:
        task_id = fixture.start("DRAWER_FOREGROUND_OBJECTIVE_PROOF")
        assert first_received.wait(8), "background provider request never started"
        original = fixture.show(task_id)
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        shell.send("print -r -- FOREGROUND_''DRAFT > \"$HOME/foreground-draft-effect\"\x01\x06\x06")
        before = probe(shell, fixture.home)
        attributes = termios.tcgetattr(shell.master)
        original_group = fixture.terminal_group()
        shell.send("\x18b\r")
        shown(shell, "Task details")
        shell.send("g")
        shown(shell, "Bring this task into the foreground?")
        shell.send("\r")
        shell.drain(.2)
        assert fixture.show(task_id)["state"] == "running" and len(fixture.loopback.calls) == 1, "default handoff confirmation changed execution"
        shell.send("g")
        shown(shell, "Bring this task into the foreground?")
        shell.send("y\r")
        # Observe the exact durable queued request before returning a provider
        # batch; no timer guesses are used to establish the safe boundary.
        path = fixture.data / "aishe/tasks/handoff" / (original["native_task_id"] + ".state.json")
        wait_until(shell, lambda: path.exists() and json.loads(path.read_text()).get("requested") == "foreground", "drawer's exact foreground request", 5)
        capture(shell, "Foreground continuation queued at a safe boundary")
        first_release.set()
        finished = fixture.wait(task_id, lambda row: row["state"] == "completed", "foreground continuation finishes", timeout=15)
        assert finished["state"] == "completed" and finished["native_task_id"] == original["native_task_id"], finished
        assert not worker_alive(finished)
        assert (fixture.work / "drawer-foreground-effect").read_text() == "once\n"
        assert not (fixture.work / "DRAWER_STALE_BATCH_MUST_NOT_RUN").exists()
        shown(shell, "DRAWER_FOREGROUND_RESULT_PROOF", timeout=10)
        capture(shell, "Foreground continuation completed with recorded checks")
        after = close_drawer(shell, fixture)
        assert after[:2] == before[:2], (before, after)
        assert termios.tcgetattr(shell.master) == attributes, "foreground continuation changed the restored shell's terminal settings"
        assert fixture.terminal_group() == original_group, "foreground continuation left the terminal owned by the wrong process group"
        assert not (fixture.home / "foreground-draft-effect").exists()
        fixture.loopback.assert_ok()
        print("  ok   foreground continuation defaults to no; explicit handoff has one owner and restores draft/TTY", flush=True)
    finally:
        first_release.set()
        fixture.close()


def many_completed_tasks_keep_observation_bounded_and_do_not_drive_provider_work():
    received = threading.Event()
    release = threading.Event()

    def choose(index, payload):
        assert index == 0, index
        received.set()
        assert release.wait(30), "history fixture provider not released"
        return response(text="HISTORY_ACTIVE_RESULT_PROOF")

    fixture = UiFixture("bounded-history", choose)
    try:
        active_id = fixture.start("HISTORY_ACTIVE_OBJECTIVE_PROOF")
        assert received.wait(8), "actual active task never reached the provider"
        active = fixture.show(active_id)
        root = fixture.data / "aishe/background-tasks"
        # Import more than the observer's bounded recovery scan. The already
        # indexed real active task must remain visible through reconciliation.
        for index in range(4200):
            row = dict(active)
            row.update(id=f"history-{index:05}", objective=f"OLD_HISTORY_OBJECTIVE_{index:05}",
                       state="completed", pid=None, process_start=None, native_task_id=None,
                       created_at_ms=1000 + index, updated_at_ms=2000 + index,
                       attempt_started_at_ms=None, elapsed_ms=100, exit_code=0,
                       result_revision=1, error=None)
            directory = root / row["id"]
            directory.mkdir(mode=0o700)
            path = directory / "record.json"
            path.write_text(json.dumps(row))
            path.chmod(0o600)
        shell = fixture.shell(cols=72)
        assert shell.ready(), shell.plain()[-4000:]
        wait_until(shell, lambda: "running" in probe(shell, fixture.home)[3].lower(), "real running task remains indexed", 12)
        shell.send("print -r -- HISTORY_''DRAFT > \"$HOME/history-draft-effect\"\x01\x06\x06")
        before = probe(shell, fixture.home)
        started = time.monotonic()
        shell.send("\x18b")
        shown(shell, "Background tasks", timeout=12)
        shown(shell, "HISTORY_ACTIVE_OBJECTIVE_PROOF", timeout=12)
        assert time.monotonic() - started < 12, "bounded recovery made the drawer unusable"
        cache_path = root / "status-v1.json"
        cache = json.loads(cache_path.read_text())
        assert 1000 < len(cache["entries"]) <= 4096, "large history was neither discovered nor bounded: " + str(len(cache["entries"]))
        assert active_id in {entry["id"] for entry in cache["entries"]}, "bounded cache dropped actual running work"
        assert cache_path.stat().st_size <= 8 * 1024 * 1024, "history cache exceeds its read bound"
        capture(shell, "Thousands of quiet results and one real active task")
        shell.send("\x1b")
        shell.drain(.2)
        after = probe(shell, fixture.home)
        assert after[:2] == before[:2], (before, after)
        assert len(fixture.loopback.calls) == 1, "history observation/editing caused additional provider turns"
        assert not (fixture.home / "history-draft-effect").exists(), "history refresh submitted the user's draft"
        shell.send("\x05\r")
        wait_until(shell, lambda: (fixture.home / "history-draft-effect").exists(), "editing remains usable with large history")
        assert (fixture.home / "history-draft-effect").read_text().strip() == "HISTORY_DRAFT"
        release.set()
        assert fixture.finish(active_id)["state"] == "completed"
        fixture.loopback.assert_ok()
        print("  ok   >4,096 imported results stay bounded; real active work remains visible; draft editing causes no model work", flush=True)
    finally:
        release.set()
        fixture.close()


def main():
    scenarios = [timeline_view_preserves_draft_and_shows_observed_failure,
                 selective_review_defaults_to_no_and_applies_only_chosen_file,
                 workflow_launcher_lists_saved_templates_and_exits_without_starting,
                 workflow_tree_exposes_queued_dependencies_without_disturbing_editing,
                 foreground_drawer_handoff_defaults_to_no_and_restores_terminal,
                 many_completed_tasks_keep_observation_bounded_and_do_not_drive_provider_work]
    failed = []
    for scenario in scenarios:
        print("  run  " + scenario.__name__, flush=True)
        try:
            scenario()
        except Exception:
            failed.append(scenario.__name__)
            traceback.print_exc()
    write_captures()
    if failed:
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} workflow UI scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} workflow UI scenarios", flush=True)


def write_captures():
    if output := os.environ.get("AISHE_TEST_CAPTURE_PATH"):
        from background_tasks_pty import CAPTURES
        with open(BINARY, "rb") as binary_file:
            digest = hashlib.file_digest(binary_file, "sha256").hexdigest()
        version = subprocess.run([BINARY, "--version"], capture_output=True, text=True,
                                 check=True, timeout=3).stdout.strip()
        head = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True,
                              check=True, timeout=3).stdout.strip()
        status = subprocess.run(["git", "status", "--porcelain"], capture_output=True, text=True,
                                check=True, timeout=3).stdout
        Path(output).write_text(json.dumps({"schema_version": 1,
                                          "binary": {"path": BINARY, "version": version, "sha256": digest},
                                          "source_observation": {"git_head": head, "uncommitted": bool(status),
                                                                 "status": status,
                                                                 "note": "Candidate binary; git version alone does not certify uncommitted source."},
                                          "captures": CAPTURES}, indent=2))


if __name__ == "__main__":
    main()
