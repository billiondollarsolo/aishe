#!/usr/bin/env python3
"""Qualify exclusive execution handoff and the live-shell detach gesture.

Requests are timed while an actual provider call or command is in flight.
Proof comes from durable lease/checkpoint identities and marker-file effects,
with the PTY additionally verifying that ZLE editing resumes intact.
"""

from __future__ import annotations

import json
import os
import shlex
import signal
import subprocess
import threading
import time
import traceback

from native_task_interactions import BINARY, Fixture, input_text, response, tool, worker_alive
from task_interactions_pty import UiFixture, close_drawer
from background_tasks_pty import capture, probe, shown, wait_until
from task_workflows_pty import write_captures


def eventually(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.04)
    raise AssertionError("timed out waiting for " + description)


def native_records(fixture):
    return [json.loads(path.read_text()) for path in (fixture.data / "aishe/tasks").glob("*.json")]


def lease(fixture, native_id):
    path = fixture.data / "aishe/tasks/handoff" / (native_id + ".state.json")
    return json.loads(path.read_text()) if path.exists() else None


def background_records(fixture):
    result = [json.loads(path.read_text()) for path in (fixture.data / "aishe/background-tasks").glob("*/record.json")]
    for row in result:
        if row["id"] not in fixture.task_ids:
            fixture.task_ids.append(row["id"])
    return result


def stop_process(process):
    if process is not None and process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=3)


def provider_boundary_detach_preserves_context_and_skips_old_batch():
    first_received = threading.Event()
    first_release = threading.Event()
    background_received = threading.Event()
    background_release = threading.Event()
    holder = {}

    def choose(index, payload):
        assert payload.get("model") == "original-background-model", payload
        if index == 0:
            first_received.set()
            assert first_release.wait(15), "fixture did not release first provider turn"
            return response(tool("run_command", {"command": "touch STALE_PROVIDER_BATCH_MUST_NOT_RUN"}, "old-foreground-batch"))
        if index == 1:
            history = input_text(payload)
            assert "old-foreground-batch" in history and "not" in history.lower(), history
            background_received.set()
            assert background_release.wait(15), "fixture did not release background provider turn"
            return response(tool("run_check", {"command": holder["command"]}, "continued-background-proof"))
        assert index == 2, index
        return response(text="DETACHED_RESULT_PROOF original context completed")

    fixture = Fixture("detach-provider-boundary", choose)
    process = None
    try:
        fixture.env["HANDOFF_EXPORTED_VALUE"] = "HANDOFF_ENVIRONMENT_PROOF"
        holder["command"] = "test \"$HANDOFF_EXPORTED_VALUE\" = HANDOFF_ENVIRONMENT_PROOF && printf 'once\\n' >> continuation-effect"
        process = subprocess.Popen([BINARY, "agent", "--scope", "host", "DETACH_ORIGINAL_OBJECTIVE_PROOF"],
                                   cwd=fixture.work, env=fixture.env, stdin=subprocess.DEVNULL,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
        assert first_received.wait(10), "foreground agent never called its loopback provider"
        original = eventually(lambda: native_records(fixture), "original native checkpoint")[0]
        native_id = original["id"]
        fixture.cli("task", "bg", native_id)
        queued = lease(fixture, native_id)
        assert queued["status"] == "queued" and queued["requested"] == "background", queued
        assert process.poll() is None, "handoff interrupted an in-flight provider request"
        # A new default applies to future work, not to an already admitted
        # conversation. Its handoff must remain native with its original model.
        fixture.write_config(model="new-default-must-not-replace-active-model", engine="opencode")
        first_release.set()
        assert background_received.wait(12), "background continuation never acquired the native task"
        rows = background_records(fixture)
        assert len(rows) == 1 and rows[0]["native_task_id"] == native_id, rows
        row = rows[0]
        assert row["engine"] == "native" and row["model"] == "original-background-model", row
        saved = fixture.checkpoint(row)
        assert saved["id"] == native_id and saved["objective"] == original["objective"], saved
        assert saved["execution"]["provider_turns"] == 2 and saved["execution"]["tool_calls"] == 0, saved
        for field in ("execution_scope", "network_policy", "workspace_root", "connection", "execution_limits"):
            assert saved.get(field) == original.get(field), (field, original.get(field), saved.get(field))
        current_lease = lease(fixture, native_id)
        assert current_lease["mode"] == "background" and current_lease["status"] == "active", current_lease
        assert not (fixture.work / "STALE_PROVIDER_BATCH_MUST_NOT_RUN").exists(), "detach executed a batch proposed before the handoff"
        background_release.set()
        finished = fixture.finish(row["id"])
        assert finished["state"] == "completed", finished
        after = fixture.checkpoint(finished)
        assert after["execution"]["provider_turns"] == 3 and after["execution"]["tool_calls"] == 1, after
        assert (fixture.work / "continuation-effect").read_text() == "once\n"
        out, err = process.communicate(timeout=5)
        assert process.returncode in {0, 75}, (process.returncode, out, err)
        assert len(native_records(fixture)) == 1, "handoff created a new objective/checkpoint"
        fixture.loopback.assert_ok()
        print("  ok   provider-boundary detach skips stale tool batch, keeps task/env/authority/counters, continues once", flush=True)
    finally:
        first_release.set()
        background_release.set()
        stop_process(process)
        fixture.close()


def foreground_attach_waits_for_actual_tool_and_has_one_owner():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("run_command", {"command": holder["command"]}, "background-command-before-attach"))
        if index == 1:
            assert "background-command-before-attach" in input_text(payload), input_text(payload)
            return response(tool("run_check", {"command": "printf 'attached\\n' >> attached-effect"}, "foreground-command-after-attach"))
        assert index == 2, index
        return response(text="ATTACHED_RESULT_PROOF exclusive owner completed")

    fixture = Fixture("attach-tool-boundary", choose)
    process = None
    duplicate = None
    try:
        marker = fixture.work / "inflight-command-effect"
        release = fixture.work / "release-inflight-command"
        script = "\n".join([
            "import pathlib,time",
            "p=pathlib.Path('inflight-command-effect'); p.write_text(p.read_text()+'once\\n' if p.exists() else 'once\\n')",
            "deadline=time.monotonic()+15",
            "while not pathlib.Path('release-inflight-command').exists():",
            " assert time.monotonic()<deadline,'fixture command was never released'",
            " time.sleep(.03)",
            "pathlib.Path('command-completed-proof').write_text('completed')",
        ])
        holder["command"] = "python3 -c " + shlex.quote(script)
        task_id = fixture.start("ATTACH_ORIGINAL_OBJECTIVE_PROOF")
        running = fixture.wait(task_id, lambda row: marker.exists(), "actual background tool effect")
        original = fixture.checkpoint(running)
        process = subprocess.Popen([BINARY, "task", "fg", task_id], cwd=fixture.work, env=fixture.env,
                                   stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                   text=True, start_new_session=True)
        queued = eventually(lambda: (row if (row := lease(fixture, original["id"])) and row.get("requested") == "foreground" else None),
                            "queued foreground ownership request")
        assert queued["status"] == "queued" and queued["mode"] == "background", queued
        assert worker_alive(fixture.show(task_id)), "attach killed a tool that had already started"
        assert process.poll() is None and len(fixture.loopback.calls) == 1, "foreground replay began before command completion"
        duplicate = subprocess.Popen([BINARY, "task", "fg", task_id], cwd=fixture.work, env=fixture.env,
                                     stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                     text=True, start_new_session=True)
        time.sleep(.1)
        assert process.poll() is None and duplicate.poll() is None, "foreground claim proceeded before the tool boundary"
        assert marker.read_text() == "once\n", "attach replayed the in-flight command"
        release.touch()
        out, err = process.communicate(timeout=15)
        duplicate_out, duplicate_err = duplicate.communicate(timeout=15)
        assert sorted([process.returncode == 0, duplicate.returncode == 0]) == [False, True], \
            ("exactly one foreground owner must win", process.returncode, out, err,
             duplicate.returncode, duplicate_out, duplicate_err)
        finished = fixture.finish(task_id)
        after = fixture.checkpoint(finished)
        assert finished["state"] == "completed" and after["id"] == original["id"], finished
        assert marker.read_text() == "once\n" and (fixture.work / "command-completed-proof").exists()
        assert (fixture.work / "attached-effect").read_text() == "attached\n"
        assert after["execution"]["tool_calls"] == 2 and after["execution"]["provider_turns"] == 3, after
        assert not worker_alive(finished), "old detached worker remains active after foreground ownership finished"
        fixture.loopback.assert_ok()
        print("  ok   attach waits for and records the in-flight command; duplicate attach refuses; effects execute once", flush=True)
    finally:
        (fixture.work / "release-inflight-command").touch()
        stop_process(process)
        stop_process(duplicate)
        fixture.close()


def live_shell_detach_returns_zle_and_preserves_next_draft():
    foreground_received = threading.Event()
    foreground_release = threading.Event()
    background_received = threading.Event()
    background_release = threading.Event()

    def choose(index, payload):
        if index == 0:
            foreground_received.set()
            assert foreground_release.wait(15), "live foreground provider not released"
            return response(text="PARK_THIS_OLD_FOREGROUND_REPLY")
        assert index == 1, index
        background_received.set()
        assert background_release.wait(15), "live background provider not released"
        return response(text="LIVE_HANDOFF_BACKGROUND_RESULT_PROOF")

    fixture = UiFixture("live-handoff", choose)
    try:
        fixture.config.write_text(fixture.config.read_text().replace('mode = "ask"', 'mode = "yolo"'))
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        shell.send("/mode agent-host\r")
        shown(shell, "Type agent-host to continue")
        shell.send("agent-host\r")
        shell.drain(.3)
        shell.send("print -r -- HOST_''GRANT_READY > \"$HOME/handoff-host-grant-ready\"\r")
        wait_until(shell, lambda: (fixture.home / "handoff-host-grant-ready").exists(), "explicit session host grant")
        shell.send("capture LIVE_HANDOFF_ORIGINAL_OBJECTIVE_PROOF\r")
        wait_until(shell, foreground_received.is_set, "live foreground provider call", 8)
        shell.send("\x18d")
        foreground_release.set()
        assert background_received.wait(12), shell.plain()[-5000:]
        wait_until(shell, lambda: "running" in probe(shell, fixture.home)[3].lower(), "handed-off task's running badge", 10)
        rows = background_records(fixture)
        assert len(rows) == 1, rows
        shell.send("print -r -- HANDOFF_''DRAFT > \"$HOME/handoff-draft-effect\"\x01\x06\x06")
        before = probe(shell, fixture.home)
        shell.send("\x18b")
        shown(shell, "Background tasks")
        shown(shell, "LIVE_HANDOFF_ORIGINAL_OBJECTIVE_PROOF")
        capture(shell, "Handed-off conversation visible while the next draft is preserved")
        detail_start = len(shell.plain())
        shell.send("\r")
        shown(shell, "Task details", start=detail_start)
        shown(shell, "no task cap", start=detail_start)
        assert "4294967295" not in shell.plain()[detail_start:], "task drawer exposes the handoff's internal unlimited sentinel"
        capture(shell, "Handed-off task shows its original uncapped limits clearly")
        after = close_drawer(shell, fixture)
        assert after[:2] == before[:2], (before, after)
        assert not (fixture.home / "handoff-draft-effect").exists(), "detach/drawer executed the user's unfinished draft"
        background_release.set()
        assert fixture.finish(rows[0]["id"])["state"] == "completed"
        shell.send("\x05\r")
        wait_until(shell, lambda: (fixture.home / "handoff-draft-effect").exists(), "restored post-detach draft executes")
        assert (fixture.home / "handoff-draft-effect").read_text().strip() == "HANDOFF_DRAFT"
        fixture.loopback.assert_ok()
        print("  ok   Ctrl-X d hands off a live conversation; prompt/badge/drawer and subsequent editing remain usable", flush=True)
    finally:
        foreground_release.set()
        background_release.set()
        fixture.close()


def main():
    scenarios = [provider_boundary_detach_preserves_context_and_skips_old_batch,
                 foreground_attach_waits_for_actual_tool_and_has_one_owner,
                 live_shell_detach_returns_zle_and_preserves_next_draft]
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
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} handoff scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} handoff scenarios", flush=True)


if __name__ == "__main__":
    main()
