#!/usr/bin/env python3
"""Exercise human interaction through real detached native workers.

An isolated loopback Responses provider chooses the next tool call and records
the actual requests it receives. Assertions inspect durable journals, process
identities, and command effects. No paid provider or managed runtime is used.
"""

from __future__ import annotations

import http.server
import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import threading
import time
import traceback

from native_background_lifecycle import (BINARY, Fixture as BackgroundFixture, process_identity,
                                         kill_fixture_group, read_json)


def tool(name, arguments, call_id):
    arguments = dict(arguments)
    if name in {"run_command", "run_check"}:
        arguments.setdefault("reason", "Private fixture operation")
    if name == "request_approval" and arguments.get("tool") in {"run_command", "run_check"}:
        arguments["arguments"] = dict(arguments["arguments"])
        arguments["arguments"].setdefault("reason", "Private fixture operation")
    return {"type": "function_call", "name": name, "arguments": json.dumps(arguments),
            "call_id": call_id, "id": "fc_" + call_id, "status": "completed"}


def response(*calls, text=None):
    output = list(calls)
    if text is not None:
        output.append({"type": "message", "role": "assistant", "content": [
            {"type": "output_text", "text": text}]})
    return {"id": "private-loopback-response", "output": output,
            "usage": {"input_tokens": 20, "output_tokens": 30}}


def input_text(payload):
    """The complete provider-visible canonical history, including tool results."""
    return json.dumps(payload.get("input", []), ensure_ascii=False)


def offered(payload):
    return {entry["name"] for entry in payload.get("tools", [])}


def worker_alive(row):
    pid = row.get("pid")
    identity = row.get("process_start")
    if not pid or not identity:
        return False
    if identity.startswith("proc:"):
        try:
            fields = Path(f"/proc/{pid}/stat").read_text().rsplit(") ", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError):
            return False
        return fields[0] != "Z" and identity == "proc:" + fields[19]
    result = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)], capture_output=True,
                            text=True, timeout=2, env=dict(os.environ, TZ="UTC", LC_ALL="C"))
    actual = result.stdout.strip()
    return bool(actual) and identity in {actual, "utc:" + actual}


def kill_worker(row):
    assert worker_alive(row), "fixture worker identity no longer matches: " + repr(row)
    pid = row["pid"]
    assert os.getpgid(pid) == pid, "fixture worker has no private process group"
    os.killpg(pid, signal.SIGKILL)


class Loopback:
    def __init__(self, choose):
        self.choose = choose
        self.calls = []
        self.errors = []
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def handle(self):
                # Hard-crash scenarios deliberately reset an in-flight or
                # pooled socket. Provider assertions are still recorded below.
                try:
                    super().handle()
                except (ConnectionResetError, BrokenPipeError):
                    pass

            def do_POST(self):
                payload = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                index = len(owner.calls)
                owner.calls.append(payload)
                try:
                    assert self.path == "/v1/responses", self.path
                    answer = owner.choose(index, payload)
                    if payload.get("stream"):
                        event = {"type": "response.completed", "response": answer}
                        body = ("data: " + json.dumps(event) + "\n\n").encode()
                        content_type = "text/event-stream"
                    else:
                        body = json.dumps(answer).encode()
                        content_type = "application/json"
                    self.send_response(200)
                except Exception:
                    owner.errors.append(traceback.format_exc())
                    body = json.dumps({"error": {"message": "private interaction fixture failed"}}).encode()
                    content_type = "application/json"
                    self.send_response(400)
                self.send_header("Content-Type", content_type)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                try:
                    self.wfile.write(body)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def log_message(self, *_):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=2)

    def assert_ok(self):
        assert not self.errors, "\n".join(self.errors)


class Fixture(BackgroundFixture):
    def __init__(self, label, choose):
        super().__init__("interactions-" + label)
        self.loopback = Loopback(choose)
        self.write_config(endpoint=f"http://127.0.0.1:{self.loopback.server.server_port}/v1")
        for key in ("AISHE_FAKE_LLM", "AISHE_FAKE_USAGE", "AISHE_FAKE_TOOL"):
            self.env.pop(key, None)

    def waiting(self, task_id):
        return self.wait(task_id, lambda row: row["state"] == "waiting" and not worker_alive(row),
                         "durably paused native worker")

    def request(self, row):
        pending = [request for request in row["mailbox"]["requests"] if request["status"] == "pending"]
        assert len(pending) == 1, row
        return pending[0]

    def answer(self, task_id, request_id, text):
        return self.cli("task", "answer", task_id, request_id, text)

    def close(self):
        # The server stays alive while cancelling active fixture workers.
        super().close()
        self.loopback.close()


def assert_refused(fixture, *args):
    before = len(fixture.loopback.calls)
    result = fixture.cli("task", *args, success=False)
    assert result.returncode != 0, (args, result.stdout, result.stderr)
    assert len(fixture.loopback.calls) == before, "rejected action started a provider request"
    return result


def questions_are_durable_and_do_not_spend_waiting_time():
    def choose(index, payload):
        if index == 0:
            assert "ask_user" in offered(payload), offered(payload)
            return response(tool("ask_user", {"question": "Which color should this fixture use?",
                                               "choices": ["blue", "green"]}, "color-question"),
                            tool("run_command", {"command": "touch skipped-after-question"}, "skipped-question"))
        assert index == 1, index
        transcript = input_text(payload)
        assert "blue" in transcript and "color-question" in transcript, transcript
        assert "skipped-question" in transcript, transcript
        assert "not" in transcript.lower(), "later batch tool did not receive a not-executed result"
        return response(text="QUESTION_RESULT_PROOF chosen blue")

    fixture = Fixture("question", choose)
    try:
        startup_marker = fixture.work / "native-startup-must-not-run"
        (fixture.home / ".aishrc").write_text(
            "printf 'unexpected startup\\n' > " + shlex.quote(str(startup_marker)) + "\n")
        objective = "Keep the original color objective"
        task_id = fixture.start(objective)
        waiting = fixture.waiting(task_id)
        request = fixture.request(waiting)
        checkpoint = fixture.checkpoint(waiting)
        assert request["kind"] == "question" and request["choices"] == ["blue", "green"], request
        assert request["binding"]["native_task_id"] == checkpoint["id"], request
        assert not (fixture.work / "skipped-after-question").exists(), "later batch action ran while waiting"
        assert not startup_marker.exists(), "native task admission sourced an interactive startup script"
        assert len(fixture.loopback.calls) == 1, "paused task continued calling its provider"
        before = waiting["elapsed_ms"]
        time.sleep(1.2)
        still_waiting = fixture.show(task_id)
        assert still_waiting["elapsed_ms"] <= before + 100, "human pause consumed the task's wallclock budget"
        assert_refused(fixture, "answer", task_id, "stale-request-id", "blue")
        fixture.answer(task_id, request["id"], "blue")
        finished = fixture.finish(task_id)
        after = fixture.checkpoint(finished)
        assert finished["state"] == "completed" and finished["native_task_id"] == waiting["native_task_id"], finished
        assert finished["objective"] == after["objective"] == objective, after
        assert after["execution"]["provider_turns"] == 2, after
        assert after["execution"]["elapsed_ms"] < before + 1100, "resume billed the human pause"
        assert not (fixture.work / "skipped-after-question").exists(), "question resume replayed a skipped batch action"
        assert not startup_marker.exists(), "native task answer/resume sourced an interactive startup script"
        assert finished["mailbox"]["requests"][0]["status"] == "consumed", finished
        assert_refused(fixture, "answer", task_id, request["id"], "green")
        fixture.loopback.assert_ok()
        print("  ok   question pause persists before effects; exact answer resumes once without billing human wait", flush=True)
    finally:
        fixture.close()


def approvals_are_exact_and_denials_do_not_authorize_actions():
    holder = {}

    def choose(index, payload):
        command_a = holder["command_a"]
        command_b = holder["command_b"]
        if index == 0:
            assert "request_approval" in offered(payload), offered(payload)
            return response(tool("request_approval", {"tool": "run_command", "arguments": {"command": command_a},
                                                       "reason": "Create the first private fixture marker"}, "approval-a"))
        if index == 1:
            assert "approv" in input_text(payload).lower(), input_text(payload)
            # A permission for A must not permit a different command B.
            return response(tool("run_command", {"command": command_b}, "different-action-b"))
        if index == 2:
            assert any(word in input_text(payload).lower() for word in ("denied", "declined")), input_text(payload)
            return response(tool("run_command", {"command": command_a}, "exact-action-a"))
        assert index == 3, index
        return response(text="APPROVAL_RESULT_PROOF only the exact approved action ran")

    fixture = Fixture("approval", choose)
    try:
        marker_a = fixture.work / "approved-action"
        marker_b = fixture.work / "unapproved-action"
        holder.update(command_a="printf 'once\\n' >> " + shlex.quote(str(marker_a)),
                      command_b="printf 'wrong\\n' >> " + shlex.quote(str(marker_b)))
        # Explicit approval grants must be consulted even when every ordinary
        # action normally requires confirmation.
        fixture.config.write_text(fixture.config.read_text().replace('yolo_confirm = "never"', 'yolo_confirm = "all"'))
        task_id = fixture.start("Bind permission to one exact action")
        first = fixture.waiting(task_id)
        request_a = fixture.request(first)
        assert request_a["kind"] == "approval", request_a
        assert not marker_a.exists() and not marker_b.exists(), "approval request executed its proposed command"
        assert_refused(fixture, "approve", task_id, "stale-request-id")
        fixture.cli("task", "approve", task_id, request_a["id"])
        second = fixture.wait(task_id, lambda row: row["state"] == "waiting" and not worker_alive(row)
                              and len(row["mailbox"]["requests"]) == 2, "changed action requires a new approval")
        request_b = fixture.request(second)
        assert request_b["binding"]["action_digest"] != request_a["binding"]["action_digest"], second
        assert not marker_a.exists() and not marker_b.exists(), "permission for A executed B"
        assert_refused(fixture, "approve", task_id, request_a["id"])
        fixture.cli("task", "deny", task_id, request_b["id"], "Do not create marker B")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        assert marker_a.read_text() == "once\n", "the exact approved command did not execute exactly once"
        assert not marker_b.exists(), "a denied command ran"
        assert_refused(fixture, "deny", task_id, request_b["id"], "duplicate denial")
        fixture.loopback.assert_ok()
        print("  ok   approval binds exact command/context; changed action pauses; denial has no effect; approval is one use", flush=True)
    finally:
        fixture.close()


def cancelled_and_exhausted_questions_reject_replies():
    for exhausted in (False, True):
        def choose(index, payload):
            assert index == 0, "rejected reply restarted provider"
            return response(tool("ask_user", {"question": "Reply only while this task has authority"}, "bounded-question"))

        fixture = Fixture("exhausted" if exhausted else "cancelled", choose)
        try:
            task_id = fixture.start(turns=1 if exhausted else 20)
            waiting = fixture.waiting(task_id)
            request = fixture.request(waiting)
            if not exhausted:
                fixture.cli("task", "cancel", task_id)
                cancelled = fixture.show(task_id)
                assert cancelled["state"] == "cancelled"
                assert fixture.checkpoint(cancelled)["native_state"] == "cancelled", cancelled
            refused = assert_refused(fixture, "answer", task_id, request["id"], "late reply")
            if exhausted:
                assert "budget" in (refused.stdout + refused.stderr).lower(), refused
            assert len(fixture.loopback.calls) == 1
            assert not (fixture.root / "runtime").exists(), "native reply initialized managed runtime"
            fixture.loopback.assert_ok()
        finally:
            fixture.close()
    print("  ok   cancelled and exhausted paused tasks reject replies without calls or budget replenishment", flush=True)


def file_approval_cannot_overwrite_a_changed_preimage():
    holder = {}

    def choose(index, payload):
        if index == 0:
            assert "write_file" in offered(payload), offered(payload)
            return response(tool("request_approval", {"tool": "write_file", "arguments": holder["arguments"],
                                                       "reason": "Review this exact file replacement"}, "file-approval"))
        if index == 1:
            return response(tool("write_file", holder["arguments"], "changed-file-action"))
        assert index == 2, index
        assert any(word in input_text(payload).lower() for word in ("denied", "declined")), input_text(payload)
        return response(text="FILE_PREIMAGE_RESULT_PROOF changed file was preserved")

    fixture = Fixture("preimage", choose)
    try:
        target = fixture.work / "approved-file.txt"
        target.write_text("ORIGINAL_PREIMAGE_PROOF\n")
        holder["arguments"] = {"path": str(target), "content": "PROPOSED_FILE_PROOF\n"}
        fixture.config.write_text(fixture.config.read_text().replace('yolo_confirm = "never"', 'yolo_confirm = "all"'))
        task_id = fixture.start("Never overwrite a file revision the user did not approve")
        first = fixture.waiting(task_id)
        request = fixture.request(first)
        assert request["binding"].get("file_preimage"), request
        target.write_text("EXTERNAL_CHANGED_PREIMAGE_PROOF\n")
        fixture.cli("task", "approve", task_id, request["id"])
        second = fixture.wait(task_id, lambda row: row["state"] == "waiting" and not worker_alive(row)
                              and len(row["mailbox"]["requests"]) == 2, "file revision change requires fresh approval")
        changed = fixture.request(second)
        assert changed["binding"]["action_digest"] != request["binding"]["action_digest"], second
        assert changed["binding"]["file_preimage"]["sha256"] != request["binding"]["file_preimage"]["sha256"], second
        assert target.read_text() == "EXTERNAL_CHANGED_PREIMAGE_PROOF\n", "stale file approval overwrote unseen content"
        fixture.cli("task", "deny", task_id, changed["id"], "Preserve the externally changed content")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed" and target.read_text() == "EXTERNAL_CHANGED_PREIMAGE_PROOF\n", finished
        fixture.loopback.assert_ok()
        print("  ok   file permission binds the exact preimage; changed content needs a fresh decision and survives denial", flush=True)
    finally:
        fixture.close()


def live_followups_are_revisioned_and_received_at_safe_boundaries():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("run_command", {"command": holder["command"]}, "slow-command"),
                            tool("run_command", {"command": "touch forbidden-old-batch-action"}, "old-batch-action"))
        assert index == 1, index
        transcript = input_text(payload)
        assert "FOLLOWUP_EDITED_PROOF" in transcript and "FOLLOWUP_THIRD_PROOF" in transcript, transcript
        assert "FOLLOWUP_ORIGINAL_PROOF" not in transcript and "FOLLOWUP_REMOVED_PROOF" not in transcript, transcript
        assert "old-batch-action" in transcript and "not" in transcript.lower(), transcript
        return response(text="STEERING_RESULT_PROOF received the latest follow-ups")

    fixture = Fixture("steering", choose)
    try:
        started = fixture.work / "slow-started"
        ended = fixture.work / "slow-ended"
        holder["command"] = ("touch " + shlex.quote(str(started)) + "; sleep 2; touch " + shlex.quote(str(ended)))
        task_id = fixture.start("Preserve the immutable steering objective")
        running = fixture.wait(task_id, lambda row: started.exists(), "running tool effect")
        request_file = fixture.data / "aishe/background-tasks" / task_id / "request.txt"
        original = request_file.read_bytes()
        for text in ("FOLLOWUP_ORIGINAL_PROOF", "FOLLOWUP_REMOVED_PROOF", "FOLLOWUP_THIRD_PROOF"):
            fixture.cli("task", "followup", task_id, text)
        queued = fixture.show(task_id)["mailbox"]["followups"]
        assert len(queued) == 3 and all(row["status"] == "queued" for row in queued), queued
        assert not ended.exists(), "steering fixture completed before queue assertions"
        revisions = [row["revision"] for row in queued]
        fixture.cli("task", "edit-followup", task_id, str(revisions[0]), "FOLLOWUP_EDITED_PROOF")
        fixture.cli("task", "remove-followup", task_id, str(revisions[1]))
        edited = fixture.show(task_id)["mailbox"]["followups"]
        assert edited[0]["text"] == "FOLLOWUP_EDITED_PROOF" and edited[1]["status"] == "removed", edited
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        after = fixture.checkpoint(finished)
        followups = finished["mailbox"]["followups"]
        assert [row["status"] for row in followups] == ["received", "removed", "received"], followups
        assert all(row.get("received_at_ms") for row in (followups[0], followups[2])), followups
        assert ended.exists() and not (fixture.work / "forbidden-old-batch-action").exists(), "steering replayed stale batch effects"
        assert finished["objective"] == after["objective"] == running["objective"], finished
        assert request_file.read_bytes() == original, "live follow-up replaced the original request"
        assert_refused(fixture, "edit-followup", task_id, str(revisions[0]), "too late")
        assert_refused(fixture, "remove-followup", task_id, str(revisions[2]))
        fixture.loopback.assert_ok()
        print("  ok   live follow-ups queue/edit/remove; exact latest revisions receive once after tool boundary; stale batch is skipped", flush=True)
    finally:
        fixture.close()


def recorded_checks_are_observed_not_model_claims():
    holder = {}

    def choose(index, payload):
        if index == 0:
            assert "run_check" in offered(payload), offered(payload)
            return response(tool("run_check", {"command": "printf 'REAL_PASS_PROOF\\n'; exit 0"}, "check-pass"),
                            tool("run_check", {"command": "printf 'REAL_FAIL_PROOF\\n'; exit 7"}, "check-fail"))
        if index == 1:
            return response(tool("run_command", {"command": holder["command"]}, "change-after-checks"))
        if index == 2:
            return response(tool("run_check", {"command": "test -f changed-after-checks && printf 'FRESH_PASS_PROOF\\n'"}, "fresh-check"))
        assert index == 3, index
        return response(text="MODEL_CLAIM_PROOF all 999 tests passed, nothing is unresolved")

    fixture = Fixture("checks", choose)
    try:
        holder["command"] = "touch changed-after-checks"
        task_id = fixture.start("Show only checks the executor actually observed")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        checkpoint = fixture.checkpoint(finished)
        checks = [row for row in checkpoint["evidence"] if row["kind"] == "check"]
        assert len(checks) == 3, checks
        assert [row["outcome"] for row in checks] == ["passed", "failed", "passed"], checks
        assert [row["exit_code"] for row in checks] == [0, 7, 0], checks
        revision = checkpoint["workspace_revision"]
        assert checks[0]["workspace_revision"] < revision and checks[1]["workspace_revision"] < revision, checks
        assert checks[2]["workspace_revision"] == revision, checks
        assert "REAL_PASS_PROOF" in json.dumps(checks[0]) and "REAL_FAIL_PROOF" in json.dumps(checks[1]), checks
        assert "FRESH_PASS_PROOF" in json.dumps(checks[2]), checks
        assert all(row["finished_at_ms"] >= row["started_at_ms"] for row in checks), checks
        assert all(row["duration_ms"] >= 0 for row in checks), checks
        fixture.loopback.assert_ok()
        print("  ok   actual exits determine recorded pass/fail; later mutation makes earlier checks stale; model claims add no evidence", flush=True)
    finally:
        fixture.close()


def received_followups_do_not_duplicate_after_a_worker_crash():
    holder = {}
    reached_provider = threading.Event()
    release_response = threading.Event()

    def choose(index, payload):
        if index == 0:
            return response(tool("run_command", {"command": holder["command"]}, "followup-before-crash"))
        transcript = input_text(payload)
        assert transcript.count("CRASH_RECEIVED_FOLLOWUP_PROOF") == 1, transcript
        if index == 1:
            reached_provider.set()
            assert release_response.wait(15), "fixture never released the interrupted provider response"
            return response(text="This response belongs to the deliberately crashed worker")
        assert index == 2, index
        return response(text="CRASH_STEERING_RESULT_PROOF received revision appears exactly once")

    fixture = Fixture("received-crash", choose)
    try:
        marker = fixture.work / "followup-crash-started"
        holder["command"] = "touch " + shlex.quote(str(marker)) + "; sleep 2"
        task_id = fixture.start("Retain delivered steering across process restarts")
        fixture.wait(task_id, lambda row: marker.exists(), "command boundary before steering delivery")
        fixture.cli("task", "followup", task_id, "CRASH_RECEIVED_FOLLOWUP_PROOF")
        assert reached_provider.wait(10), "provider never saw the delivered steering"
        active = fixture.show(task_id)
        followup = active["mailbox"]["followups"][0]
        assert followup["status"] == "received" and followup.get("received_at_ms"), active
        before = fixture.checkpoint(active)
        users = [message["data"] for message in before["messages"] if message["role"] == "user"]
        assert sum("CRASH_RECEIVED_FOLLOWUP_PROOF" in text for text in users) == 1, before
        kill_worker(active)
        release_response.set()
        fixture.wait(task_id, lambda row: row["state"] == "interrupted" and row.get("pid") is None,
                     "reconciled worker crash after received steering")
        fixture.cli("task", "resume", task_id)
        finished = fixture.finish(task_id)
        after = fixture.checkpoint(finished)
        users = [message["data"] for message in after["messages"] if message["role"] == "user"]
        assert sum("CRASH_RECEIVED_FOLLOWUP_PROOF" in text for text in users) == 1, after
        assert finished["mailbox"]["followups"][0]["received_at_ms"] == followup["received_at_ms"], finished
        assert after["followup_revision"] == before["followup_revision"], after
        fixture.loopback.assert_ok()
        print("  ok   received means durably journaled; restart during the next provider request does not duplicate steering", flush=True)
    finally:
        release_response.set()
        fixture.close()


def crashed_checks_resume_without_uncertain_replay():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("run_check", {"command": holder["command"]}, "uncertain-check"))
        assert index == 1, index
        transcript = input_text(payload)
        assert "uncertain-check" in transcript and "may have started" in transcript.lower(), transcript
        return response(text="CRASH_RESULT_PROOF the uncertain check was not replayed")

    fixture = Fixture("crash", choose)
    try:
        marker = fixture.work / "uncertain-check-effects"
        group_file = fixture.work / "uncertain-check-group"
        late = fixture.work / "uncertain-check-late-effect"
        holder["command"] = ("printf 'once\\n' >> " + shlex.quote(str(marker))
                             + "; printf '%s' \"$$\" > " + shlex.quote(str(group_file))
                             + "; sleep 30; touch " + shlex.quote(str(late)))
        task_id = fixture.start("Do not claim a crashed check passed")
        active = fixture.wait(task_id, lambda row: marker.exists() and group_file.exists(),
                              "check's actual effects before hard worker crash")
        checkpoint = fixture.checkpoint(active)
        checks = [row for row in checkpoint["evidence"] if row["kind"] == "check"]
        assert len(checks) == 1 and checks[0]["outcome"] == "running", checks
        group = int(group_file.read_text())
        identity = process_identity(group)
        fixture.tool_groups[group] = identity
        kill_worker(active)
        kill_fixture_group(group, identity)
        interrupted = fixture.wait(task_id, lambda row: row["state"] == "interrupted" and row.get("pid") is None,
                                   "reconciliation after hard worker crash")
        fixture.cli("task", "resume", task_id)
        finished = fixture.finish(task_id)
        after = fixture.checkpoint(finished)
        assert finished["state"] == "completed", finished
        checks = [row for row in after["evidence"] if row["kind"] == "check"]
        assert finished["native_task_id"] == interrupted["native_task_id"], finished
        assert len(checks) == 1 and checks[0]["outcome"] == "uncertain", checks
        assert checks[0].get("exit_code") is None, "crash fabricated an observed exit status"
        assert marker.read_text() == "once\n" and not late.exists(), "resume replayed an uncertain check"
        assert after.get("pending_tool") is None, after
        assert after["execution"]["tool_calls"] == checkpoint["execution"]["tool_calls"], after
        fixture.loopback.assert_ok()
        print("  ok   hard crash leaves check uncertain; resume keeps checkpoint/counters and never repeats effects", flush=True)
    finally:
        fixture.close()


def names_pin_archive_and_review_do_not_rewrite_the_objective():
    fixture = BackgroundFixture("interactions-metadata")
    try:
        objective = "METADATA_ORIGINAL_PROOF keep this immutable objective"
        task_id = fixture.start(objective)
        finished = fixture.finish(task_id)
        original_request = (fixture.data / "aishe/background-tasks" / task_id / "request.txt").read_bytes()
        metadata_path = fixture.data / "aishe/background-tasks" / task_id / "metadata.json"
        fixture.cli("task", "rename", task_id, "METADATA_NAME_PROOF calm title")
        fixture.cli("task", "pin", task_id)
        named = read_json(metadata_path)
        assert named["title"] == "METADATA_NAME_PROOF calm title" and named["pinned"], named
        assert fixture.show(task_id)["objective"] == objective
        fixture.cli("task", "reviewed", task_id)
        reviewed = read_json(metadata_path)
        stamp = reviewed.get("reviewed_revision")
        assert stamp, reviewed
        # Independent CLI processes observe one durable review, and purely
        # presentational changes must not resurrect the same result.
        fixture.cli("task", "rename", task_id, "METADATA_RENAMED_PROOF")
        fixture.cli("task", "unpin", task_id)
        assert read_json(metadata_path)["reviewed_revision"] == stamp
        fixture.cli("task", "archive", task_id)
        assert read_json(metadata_path)["archived"]
        assert "METADATA_RENAMED_PROOF" not in fixture.cli("task", "browse").stdout
        fixture.cli("task", "unarchive", task_id)
        assert not read_json(metadata_path)["archived"]
        fixture.env["AISHE_FAKE_LLM"] = "METADATA_NEW_RESULT_PROOF"
        fixture.cli("task", "rework", task_id, "Produce a new result revision")
        reworked = fixture.finish(task_id)
        assert reworked["state"] == "completed", reworked
        assert reworked["result_revision"] > finished.get("result_revision", 0), reworked
        assert read_json(metadata_path)["reviewed_revision"] == stamp, "rework silently marked its new result reviewed"
        assert fixture.show(task_id)["objective"] == objective
        assert (fixture.data / "aishe/background-tasks" / task_id / "request.txt").read_bytes() == original_request
        print("  ok   names/pin/archive are separate metadata; reviewed status persists exactly; new result needs review", flush=True)
    finally:
        fixture.close()


def main():
    scenarios = [questions_are_durable_and_do_not_spend_waiting_time,
                 approvals_are_exact_and_denials_do_not_authorize_actions,
                 cancelled_and_exhausted_questions_reject_replies,
                 file_approval_cannot_overwrite_a_changed_preimage,
                 live_followups_are_revisioned_and_received_at_safe_boundaries,
                 recorded_checks_are_observed_not_model_claims,
                 received_followups_do_not_duplicate_after_a_worker_crash,
                 crashed_checks_resume_without_uncertain_replay,
                 names_pin_archive_and_review_do_not_rewrite_the_objective]
    failed = []
    for scenario in scenarios:
        print("  run  " + scenario.__name__, flush=True)
        try:
            scenario()
        except Exception:
            failed.append(scenario.__name__)
            traceback.print_exc()
            print("  FAIL " + scenario.__name__, flush=True)
    if failed:
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} interaction scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} native interaction and recorded-check scenarios", flush=True)


if __name__ == "__main__":
    main()
