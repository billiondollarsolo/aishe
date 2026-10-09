#!/usr/bin/env python3
"""Kill a live fake-tool task, then prove resume never repeats the pending tool."""

import json
import os
import shutil
import shlex
import signal
import subprocess
import sys
import tempfile
import time

from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)


def main():
    root = tempfile.mkdtemp(prefix="aishe-task-resume-")
    process = None
    tool_pid = None
    tool_reaped = False
    try:
        config_root = os.path.join(root, "config")
        data_root = os.path.join(root, "data")
        work = os.path.join(root, "work")
        os.makedirs(os.path.join(config_root, "aishe"))
        os.makedirs(work)
        with open(
            os.path.join(config_root, "aishe", "config.toml"),
            "w",
            encoding="utf-8",
        ) as file:
            file.write(
                "version = 2\n"
                "[aishe]\n"
                'mode = "yolo"\n'
                'provider = "openai"\n'
                'yolo_confirm = "never"\n'
                "yolo_plan = false\n"
                "yolo_sandbox = false\n"
                "max_yolo_iterations = 5\n\n"
                "[providers.openai]\n"
                'base_url = "https://api.openai.com"\n'
                'api_key_env = "UNUSED_FAKE_KEY"\n'
                'model = "fake-resume-model"\n'
                'transport = "responses"\n'
                "\n[backend]\n"
                'engine = "native"\n'
                'default_scope = "host"\n'
            )
        marker = os.path.join(work, "tool-ran.txt")
        tool_pid = os.path.join(work, "tool-pid.txt")
        env = dict(os.environ)
        env.update(
            {
                "AISHE_CONFIG_DIR": config_root,
                "AISHE_DATA_DIR": data_root,
                "AISHE_LEAN": "1",
                "AISHE_LEGACY_OPENCODE": "0",
                "AISHE_FAKE_LLM": "initial fake response",
                "AISHE_FAKE_TOOL": "printf 'once\\n' >> %s; printf '%%s' \"$$\" > %s; sleep 30" % (
                    shlex.quote(marker), shlex.quote(tool_pid)),
            }
        )
        process = subprocess.Popen(
            [BINARY, "agent", "--scope", "host", "run the resumable test"],
            cwd=work,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            preexec_fn=os.setsid,
        )
        tasks = os.path.join(data_root, "aishe", "tasks")
        record_path = None
        record = None
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            if os.path.isdir(tasks):
                paths = [
                    os.path.join(tasks, name)
                    for name in os.listdir(tasks)
                    if name.endswith(".json")
                ]
                if paths:
                    try:
                        candidate = json.load(open(paths[0], encoding="utf-8"))
                    except (OSError, json.JSONDecodeError):
                        time.sleep(0.05)
                        continue
                    pending = candidate.get("pending_tool") or {}
                    if pending.get("may_have_started") and os.path.exists(marker) and os.path.exists(tool_pid):
                        record_path = paths[0]
                        record = candidate
                        break
            if process.poll() is not None:
                out, err = process.communicate()
                raise AssertionError(
                    "task exited before checkpoint\nstdout=%s\nstderr=%s" % (out, err)
                )
            time.sleep(0.05)
        if record_path is None:
            raise AssertionError("pending started checkpoint was not persisted")

        os.killpg(os.getpgid(process.pid), signal.SIGKILL)
        process.wait(timeout=5)
        # This deliberately simulates a hard worker crash. Reap the fixture's
        # separate tool process group before checking the continuation; normal
        # cancellation is qualified by native_background_lifecycle.py.
        try:
            with open(tool_pid, encoding="utf-8") as file:
                os.killpg(int(file.read()), signal.SIGKILL)
        except ProcessLookupError:
            pass
        tool_reaped = True
        task_id = record["id"]
        if open(marker, encoding="utf-8").read().splitlines() != ["once"]:
            raise AssertionError("tool did not run exactly once before interruption")

        # Change the current default while preserving the saved task identity.
        # Continuation uses its saved connection snapshot and canonical history.
        with open(os.path.join(config_root, "aishe", "config.toml"), "w", encoding="utf-8") as file:
            file.write('version = 2\n[aishe]\nmode = "yolo"\nprovider = "anthropic"\n'
                       'yolo_plan = false\nyolo_sandbox = false\nyolo_confirm = "never"\n'
                       '[providers.anthropic]\nbase_url = "https://api.anthropic.com"\n'
                       'api_key_env = "UNUSED_FAKE_KEY"\nmodel = "new-default-model"\n'
                       '[backend]\nengine = "native"\ndefault_scope = "host"\n')

        resume_env = dict(env)
        resume_env.pop("AISHE_FAKE_TOOL", None)
        resume_env["AISHE_FAKE_LLM"] = "resume complete"
        resumed = subprocess.run(
            [BINARY, "resume", task_id],
            cwd=work,
            env=resume_env,
            capture_output=True,
            text=True,
            timeout=20,
        )
        combined = resumed.stdout + resumed.stderr
        if resumed.returncode != 0:
            raise AssertionError("resume failed\n" + combined)
        for expected in [
            "pending tool",
            "resume complete",
        ]:
            if expected not in combined:
                raise AssertionError("resume output missing %r\n%s" % (expected, combined))
        if open(marker, encoding="utf-8").read().splitlines() != ["once"]:
            raise AssertionError("resume repeated a possibly-started tool")
        final = json.load(open(record_path, encoding="utf-8"))
        if final["status"] != "completed" or final.get("pending_tool") is not None:
            raise AssertionError("resumed task did not complete cleanly: %r" % final)
        if final["id"] != task_id or final["model"] != "fake-resume-model" or final["provider"] != "openai":
            raise AssertionError("resume changed the saved task identity: %r" % final)
        if final["execution"]["tool_calls"] != record["execution"]["tool_calls"]:
            raise AssertionError("resume reset or repeated the pending tool reservation")
        print("PASS: interrupted durable task resumed without repeating its tool")
    finally:
        if tool_pid and not tool_reaped and os.path.exists(tool_pid):
            # Only this fixture writes this PID, and the tool sleeps for 30 s
            # while the checkpoint wait is bounded to 15 s.
            try:
                with open(tool_pid, encoding="utf-8") as file:
                    pid = int(file.read())
                if os.getpgid(pid) == pid:
                    os.killpg(pid, signal.SIGKILL)
            except (ProcessLookupError, ValueError, OSError):
                pass
        if process is not None and process.poll() is None:
            try:
                os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
