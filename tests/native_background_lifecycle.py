#!/usr/bin/env python3
"""Qualify detached native agents through their public CLI and durable effects.

Only the deterministic fake provider and an isolated loopback failure server
are used. Every task has an explicit host scope, private working directory,
short budget, and private config/data; no managed runtime is needed.
"""

import http.server
import json
import os
from pathlib import Path
import re
import shlex
import signal
import subprocess
import sys
import tempfile
import threading
import time

from harness_identity import require_current_binary

BINARY = require_current_binary(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
TERMINAL_STATES = {"completed", "failed", "interrupted", "cancelled", "applied", "discarded"}


def read_json(path):
    return json.loads(path.read_text(encoding="utf-8"))


def process_identity(pid):
    result = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)],
                            capture_output=True, text=True, timeout=2)
    return result.stdout.strip() if result.returncode == 0 else None


def kill_fixture_group(pid, identity):
    if not identity or process_identity(pid) != identity:
        return
    try:
        if os.getpgid(pid) == pid:
            os.killpg(pid, signal.SIGKILL)
    except ProcessLookupError:
        pass


class Fixture:
    def __init__(self, label):
        self.temporary = tempfile.TemporaryDirectory(prefix="aishe-native-background-" + label + "-")
        # Match the real cwd visible to detached workers. Darwin's /var temp
        # alias and symlink-spelled TMPDIR must not turn ordinary fixture file
        # approvals into correctly refused symlink targets.
        self.root = Path(self.temporary.name).resolve()
        self.home = self.root / "home"
        self.work = self.root / "work"
        self.config_root = self.root / "config"
        self.data = self.root / "data"
        self.config = self.config_root / "aishe/config.toml"
        for directory in (self.home, self.work, self.config.parent, self.data):
            directory.mkdir(parents=True)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith("AISHE_") and key not in {"OPENAI_API_KEY", "ANTHROPIC_API_KEY", "ZDOTDIR"}}
        self.env.update({
            "HOME": str(self.home), "AISHE_CONFIG_DIR": str(self.config_root),
            "AISHE_DATA_DIR": str(self.data), "XDG_CONFIG_HOME": str(self.config_root),
            "XDG_DATA_HOME": str(self.data), "AISHE_RUNTIME_DIR": str(self.root / "runtime"),
            "AISHE_LEAN": "1", "AISHE_LEGACY_OPENCODE": "0", "AISHE_ZSH_PROFILE": "clean",
            "AISHE_POLICY_FILE": str(self.root / "policy.toml"),
            "AISHE_FAKE_LLM": "background fixture complete", "AISHE_FAKE_USAGE": "20,30",
            "NATIVE_BACKGROUND_FIXTURE_KEY": "loopback-fixture", "NO_COLOR": "1",
        })
        self.task_ids = []
        self.tool_groups = {}
        self.write_config()

    def write_config(self, provider="openai", model="original-background-model", endpoint="http://127.0.0.1:9/v1", engine="native"):
        self.config.write_text(
            'version = 2\n[aishe]\nmode = "yolo"\n'
            f'provider = "{provider}"\n'
            'yolo_confirm = "never"\nyolo_plan = false\nyolo_sandbox = false\n'
            'max_yolo_iterations = 20\nbudget_usd = 0.0\n'
            f'\n[providers.{provider}]\nbase_url = "{endpoint}"\n'
            'api_key_env = "NATIVE_BACKGROUND_FIXTURE_KEY"\n'
            f'model = "{model}"\n'
            + ('transport = "responses"\n' if provider == "openai" else '')
            + f'\n[backend]\nengine = "{engine}"\ndefault_scope = "host"\nworkspace_network = "allow"\n'
            + '\n[sandbox]\nallow_host_yolo = true\n',
            encoding="utf-8",
        )

    def cli(self, *args, env=None, success=True):
        completed = subprocess.run([BINARY, *args], cwd=self.work, env=env or self.env,
                                   capture_output=True, text=True, timeout=15)
        if success and completed.returncode != 0:
            raise AssertionError(f"CLI {args} failed ({completed.returncode})\n{completed.stdout}\n{completed.stderr}")
        return completed

    def start(self, objective="Keep this original objective", turns=20):
        result = self.cli("task", "start", "--no-isolation", "--max-minutes", "1",
                          "--max-turns", str(turns), objective)
        match = re.search(r"started task (\S+)", result.stdout)
        assert match, result.stdout + result.stderr
        task_id = match.group(1)
        self.task_ids.append(task_id)
        return task_id

    def show(self, task_id):
        return json.loads(self.cli("task", "show", task_id, "--json").stdout)

    def checkpoint(self, record):
        native_id = record.get("native_task_id")
        assert native_id, "background record lost its native checkpoint: " + repr(record)
        return read_json(self.data / "aishe/tasks" / (native_id + ".json"))

    def wait(self, task_id, predicate, description, timeout=12):
        deadline = time.monotonic() + timeout
        latest = None
        while time.monotonic() < deadline:
            latest = self.show(task_id)
            if predicate(latest):
                return latest
            time.sleep(.05)
        log = self.data / "aishe/background-tasks" / task_id / "activity.log"
        raise AssertionError(f"timed out waiting for {description}: {latest}\n{log.read_text() if log.exists() else ''}")

    def finish(self, task_id):
        return self.wait(task_id, lambda row: row["state"] in TERMINAL_STATES,
                         "terminal task state")

    def close(self):
        for task_id in self.task_ids:
            try:
                record = self.show(task_id)
                if record["state"] in {"starting", "running"}:
                    self.cli("task", "cancel", task_id, success=False)
                pid = record.get("pid")
                if pid:
                    kill_fixture_group(pid, record.get("process_start"))
            except (AssertionError, OSError, subprocess.SubprocessError):
                pass
        for pid, identity in self.tool_groups.items():
            kill_fixture_group(pid, identity)
        self.temporary.cleanup()


def cancellation_and_resume():
    fixture = Fixture("cancel")
    try:
        marker = fixture.work / "ran-once"
        late = fixture.work / "must-not-survive-cancel"
        group_file = fixture.work / "tool-process-group"
        # The tool itself checks the durable linkage BEFORE writing its marker.
        # Parent-side polling alone could miss a write-before-checkpoint race.
        proof = (
            "import glob,json,pathlib,os; "
            f"records=glob.glob({str(fixture.data / 'aishe/background-tasks/*/record.json')!r}); "
            "assert len(records)==1; record=json.load(open(records[0])); "
            "native_id=record['native_task_id']; "
            f"checkpoint=json.load(open(str(pathlib.Path({str(fixture.data / 'aishe/tasks')!r})/(native_id+'.json')))); "
            "assert checkpoint['pending_tool']['may_have_started']; "
            f"pathlib.Path({str(group_file)!r}).write_text(str(os.getpgrp())); "
            f"pathlib.Path({str(marker)!r}).write_text({('once' + chr(10))!r})"
        )
        fixture.env["AISHE_FAKE_TOOL"] = (
            "python3 -c " + shlex.quote(proof) + "; sleep 4; touch " + shlex.quote(str(late))
        )
        task_id = fixture.start()
        running = fixture.wait(task_id, lambda row: marker.exists(), "tool's persisted-checkpoint proof")
        group = int(group_file.read_text())
        fixture.tool_groups[group] = process_identity(group)
        before = fixture.checkpoint(running)
        assert running["engine"] == "native", running
        assert before["execution"]["tool_calls"] == 1, before
        assert before["execution"]["provider_turns"] >= 1, before
        assert before["usage"]["input"] >= 20, before
        fixture.cli("task", "cancel", task_id)
        cancelled = fixture.wait(task_id, lambda row: row["state"] == "cancelled" and row.get("pid") is None,
                                 "cancelled worker exit")
        assert cancelled["exit_code"] == 130, cancelled
        checkpoint = fixture.checkpoint(cancelled)
        assert checkpoint["native_state"] == "cancelled", checkpoint
        time.sleep(4.2)
        assert not late.exists(), "cancelled tool process group survived the worker"
        assert marker.read_text() == "once\n", "initial tool unexpectedly repeated"

        # A different current default and legacy launch environment must not
        # move an admitted native continuation to a managed runtime.
        fixture.write_config(provider="anthropic", model="new-default-model", engine="opencode")
        fixture.env.update({"AISHE_LEAN": "0", "AISHE_LEGACY_OPENCODE": "1"})
        fixture.env.pop("AISHE_FAKE_TOOL")
        fixture.env["AISHE_FAKE_LLM"] = "resume fixture complete"
        fixture.cli("task", "resume", task_id)
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed" and finished["exit_code"] == 0, finished
        assert finished["engine"] == "native", finished
        assert not (fixture.root / "runtime").exists(), "native continuation created a managed runtime"
        after = fixture.checkpoint(finished)
        assert finished["native_task_id"] == running["native_task_id"], "resume replaced the native checkpoint"
        assert after["model"] == before["model"] == "original-background-model", after
        assert after["connection_id"] == before["connection_id"], after
        for field in ("execution_scope", "network_policy", "workspace_root", "connection", "execution_limits"):
            assert after.get(field) == before.get(field), (field, before, after)
        assert after["messages"][:len(before["messages"])] == before["messages"], "resume replaced canonical history"
        assert after.get("pending_tool") is None, after
        assert after["execution"]["tool_calls"] == before["execution"]["tool_calls"], after
        assert after["execution"]["provider_turns"] > before["execution"]["provider_turns"], after
        for field in ("input", "output", "requests"):
            assert after["usage"][field] > before["usage"][field], (field, before, after)
        assert marker.read_text() == "once\n", "resume replayed a possibly-started tool"
        assert not late.exists(), "resume restarted the original objective's tool"
        print("  ok   cancellation kills child groups and resume keeps native engine, identity, profile, transcript, and counters")
    finally:
        fixture.close()


def exhausted_task_cannot_reset_its_budget():
    fixture = Fixture("bounded")
    try:
        marker = fixture.work / "budget-tool"
        fixture.env["AISHE_FAKE_TOOL"] = "printf 'once\\n' >> " + shlex.quote(str(marker))
        task_id = fixture.start(turns=1)
        finished = fixture.finish(task_id)
        assert finished["state"] == "failed", finished
        assert finished["exit_code"] in {75, 124}, finished
        before = fixture.checkpoint(finished)
        expected = "iteration_limit" if finished["exit_code"] == 75 else "budget_exhausted"
        assert before["native_state"] == expected, before
        assert before["status"] != "completed", before
        assert before["execution"]["provider_turns"] == 1, before
        assert before["execution"]["tool_calls"] == 1, before
        assert marker.read_text() == "once\n"
        fixture.write_config(model="changed-budget-default")
        fixture.env.pop("AISHE_FAKE_TOOL")
        refused = fixture.cli("task", "resume", task_id, success=False)
        assert refused.returncode != 0, "resume replenished an exhausted task budget"
        after_record = fixture.show(task_id)
        after = fixture.checkpoint(after_record)
        assert after_record["state"] == "failed" and after_record["native_task_id"] == finished["native_task_id"], after_record
        assert after["execution"] == before["execution"], "failed resume reset cumulative reservations"
        assert marker.read_text() == "once\n"
        print("  ok   iteration or budget exhaustion is failed and resume cannot replenish counters")
    finally:
        fixture.close()


def foreground_sigint_kills_tool_group():
    fixture = Fixture("foreground")
    process = None
    try:
        marker = fixture.work / "foreground-started"
        late = fixture.work / "foreground-must-not-survive"
        group_file = fixture.work / "foreground-group"
        fixture.env["AISHE_FAKE_LLM"] = "CANCELLED_FOREGROUND_ANSWER_MUST_NOT_APPEAR"
        fixture.env["AISHE_FAKE_TOOL"] = (
            "printf '%s' \"$$\" > " + shlex.quote(str(group_file))
            + "; touch " + shlex.quote(str(marker)) + "; sleep 4; touch " + shlex.quote(str(late))
        )
        process = subprocess.Popen([BINARY, "agent", "--scope", "host", "Run the cancellable foreground fixture"],
                                   cwd=fixture.work, env=fixture.env, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True, start_new_session=True)
        deadline = time.monotonic() + 12
        while not marker.exists() and time.monotonic() < deadline:
            if process.poll() is not None:
                out, err = process.communicate()
                raise AssertionError("foreground task exited before the tool started\n" + out + err)
            time.sleep(.05)
        assert marker.exists(), "foreground tool never started"
        group = int(group_file.read_text())
        fixture.tool_groups[group] = process_identity(group)
        os.killpg(process.pid, signal.SIGINT)
        out, err = process.communicate(timeout=3)
        assert process.returncode == 130, (process.returncode, out, err)
        assert "CANCELLED_FOREGROUND_ANSWER_MUST_NOT_APPEAR" not in out + err
        checkpoints = [read_json(path) for path in (fixture.data / "aishe/tasks").glob("*.json")]
        assert len(checkpoints) == 1 and checkpoints[0]["native_state"] == "cancelled", checkpoints
        assert checkpoints[0]["execution"]["tool_calls"] == 1, checkpoints
        time.sleep(4.2)
        assert not late.exists(), "foreground SIGINT orphaned its separate tool process group"
        print("  ok   public foreground SIGINT exits 130 and kills its separate tool process group")
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=3)
        fixture.close()


def explicit_cost_cap_refuses_unpriced_model_before_provider_work():
    fixture = Fixture("cost")
    try:
        marker = fixture.work / "unpriced-tool-must-not-run"
        fixture.env["AISHE_FAKE_TOOL"] = "touch " + shlex.quote(str(marker))
        refused = fixture.cli("agent", "--scope", "host", "--max-cost", "0.01",
                              "Refuse an unpriced provider call", success=False)
        assert refused.returncode != 0, refused
        checkpoints = [read_json(path) for path in (fixture.data / "aishe/tasks").glob("*.json")]
        assert len(checkpoints) == 1, checkpoints
        checkpoint = checkpoints[0]
        assert checkpoint["status"] == "failed" and checkpoint["native_state"] == "failed", checkpoint
        assert checkpoint["execution"]["provider_turns"] == 0 and checkpoint["execution"]["tool_calls"] == 0, checkpoint
        assert checkpoint["usage"]["requests"] == 0, checkpoint
        assert not marker.exists(), "unpriced --max-cost admitted an effect"
        assert "price" in (refused.stdout + refused.stderr).lower(), "cost refusal omitted its public explanation: " + repr(refused)
        print("  ok   public --max-cost refuses an unpriced model before provider or tool reservations")
    finally:
        fixture.close()


def provider_failure_is_not_completion():
    fixture = Fixture("failure")
    calls = []

    class Failure(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", "0")))
            calls.append(self.path)
            body = json.dumps({"error": {"message": "isolated background provider failure"}}).encode()
            self.send_response(500)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Failure)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        fixture.write_config(endpoint=f"http://127.0.0.1:{server.server_port}/v1")
        fixture.env.pop("AISHE_FAKE_LLM")
        fixture.env.pop("AISHE_FAKE_USAGE")
        task_id = fixture.start("Report the local provider failure accurately")
        finished = fixture.finish(task_id)
        assert calls, "worker never reached the loopback failure provider"
        assert finished["state"] == "failed" and finished["exit_code"] == 1, finished
        checkpoint = fixture.checkpoint(finished)
        assert checkpoint["status"] == "failed" and checkpoint["native_state"] == "failed", checkpoint
        assert checkpoint["execution"]["provider_turns"] >= 1, checkpoint
        assert "provider failure" in json.dumps(checkpoint), checkpoint
        print("  ok   real local provider errors publish a failed background task")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
        fixture.close()


def rework_keeps_original_request_and_appends_steering_once():
    fixture = Fixture("rework")
    try:
        objective = "Preserve this original rework objective"
        task_id = fixture.start(objective)
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        before = fixture.checkpoint(finished)
        request = fixture.data / "aishe/background-tasks" / task_id / "request.txt"
        original = request.read_bytes()
        instruction = "Add one final verification explanation"
        fixture.env["AISHE_FAKE_LLM"] = "reworked fixture complete"
        fixture.cli("task", "rework", task_id, instruction)
        reworked = fixture.finish(task_id)
        assert reworked["state"] == "completed" and reworked["exit_code"] == 0, reworked
        after = fixture.checkpoint(reworked)
        assert reworked["native_task_id"] == finished["native_task_id"], reworked
        assert reworked["objective"] == objective and after["objective"] == objective, after
        assert request.read_bytes() == original, "rework rewrote the original request"
        assert reworked["steering"] == [instruction] and reworked["steering_revision"] == 1, reworked
        steering = [message["data"] for message in after["messages"]
                    if message["role"] == "user" and message["data"].startswith("Rework request:\n")]
        assert steering == ["Rework request:\n" + instruction], after
        assert after["steering_revision"] == 1, after
        assert after["execution"]["provider_turns"] > before["execution"]["provider_turns"], after
        assert after["usage"]["input"] > before["usage"]["input"], after
        print("  ok   rework retains the original request and consumes one new steering revision")
    finally:
        fixture.close()


def mcp_discovery_follows_checkpoint_and_network_admission():
    fixture = Fixture("mcp")
    calls = []
    errors = []

    # This HTTP/1.0 fixture closes after each response. Announce that closure
    # so the client pool cannot reuse a socket while the peer's FIN is in flight.
    class Mcp(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            message = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            method = message.get("method")
            calls.append(method)
            if method == "initialize":
                try:
                    records = [read_json(path) for path in (fixture.data / "aishe/background-tasks").glob("*/record.json")]
                    active = [row for row in records if row["state"] in {"starting", "running"}]
                    assert len(active) == 1, records
                    checkpoint = fixture.checkpoint(active[0])
                    assert checkpoint["id"] == active[0]["native_task_id"], checkpoint
                except Exception as error:
                    errors.append(str(error))
                result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                          "serverInfo": {"name": "private-lifecycle-fixture", "version": "1"}}
            elif method == "tools/list":
                result = {"tools": []}
            elif method == "notifications/initialized":
                self.send_response(202)
                self.send_header("Content-Length", "0")
                self.send_header("Connection", "close")
                self.end_headers()
                return
            else:
                errors.append("unexpected MCP method " + str(method))
                result = {}
            body = json.dumps({"jsonrpc": "2.0", "id": message["id"], "result": result}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Mcp)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        mcp_config = f'\n[mcp_servers.lifecycle]\nurl = "http://127.0.0.1:{server.server_port}/mcp"\n'
        with fixture.config.open("a") as file:
            file.write(mcp_config)
        task_id = fixture.start("Exercise bounded local MCP discovery")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        assert "initialize" in calls and "tools/list" in calls, calls
        assert not errors, errors
        calls.clear()
        (fixture.root / "policy.toml").write_text("version = 1\nallow_network = false\n")
        denied = fixture.cli("agent", "--scope", "host", "Refuse host network prohibited by policy", success=False)
        assert denied.returncode != 0 and "network" in (denied.stdout + denied.stderr).lower(), denied
        assert not calls, "policy-refused host admission initialized an MCP server: " + repr(calls)
        print("  ok   MCP initializes after durable linkage and policy-refused host admission performs no handshake")
    except AssertionError as error:
        # Preserve worker diagnostics before the private temporary tree is
        # removed, while keeping failure output bounded to each log's tail.
        diagnostics = []
        for task_id in fixture.task_ids:
            log = fixture.data / "aishe/background-tasks" / task_id / "activity.log"
            try:
                with log.open("rb") as file:
                    file.seek(0, os.SEEK_END)
                    file.seek(max(0, file.tell() - 16_384))
                    tail = file.read(16_384).decode("utf-8", errors="replace")
            except OSError as log_error:
                tail = "activity log unavailable: " + str(log_error)
            diagnostics.append(f"worker {task_id} activity.log tail:\n{tail}")
        raise AssertionError(str(error) + "\n" + "\n".join(diagnostics)) from error
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
        fixture.close()


def main():
    cancellation_and_resume()
    foreground_sigint_kills_tool_group()
    explicit_cost_cap_refuses_unpriced_model_before_provider_work()
    exhausted_task_cannot_reset_its_budget()
    provider_failure_is_not_completion()
    rework_keeps_original_request_and_appends_steering_once()
    mcp_discovery_follows_checkpoint_and_network_admission()
    print("PASS: detached native lifecycle, truthful outcomes, at-most-once resume, and cumulative budgets")


if __name__ == "__main__":
    main()
