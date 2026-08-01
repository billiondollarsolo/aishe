#!/usr/bin/env python3
"""Kill a live fake-tool task, then prove resume never repeats the pending tool."""

import json
import os
import shutil
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
            )
        marker = os.path.join(work, "tool-ran.txt")
        env = dict(os.environ)
        env.update(
            {
                "AISHE_CONFIG_DIR": config_root,
                "AISHE_DATA_DIR": data_root,
                "AISHE_FAKE_LLM": "initial fake response",
                "AISHE_FAKE_TOOL": "printf 'once\\n' >> %s; sleep 30" % marker,
            }
        )
        process = subprocess.Popen(
            [BINARY, "--yolo-line", "run the resumable test"],
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
                    if pending.get("may_have_started") and os.path.exists(marker):
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
        task_id = record["id"]
        if open(marker, encoding="utf-8").read().splitlines() != ["once"]:
            raise AssertionError("tool did not run exactly once before interruption")

        # Exercise the provider-neutral fallback at the same time. The canonical
        # task messages remain usable after a provider/model change.
        record["provider"] = "anthropic"
        record["model"] = "old-provider-model"
        with open(record_path, "w", encoding="utf-8") as file:
            json.dump(record, file)

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
            "using provider-neutral canonical history",
            "resume complete",
        ]:
            if expected not in combined:
                raise AssertionError("resume output missing %r\n%s" % (expected, combined))
        if open(marker, encoding="utf-8").read().splitlines() != ["once"]:
            raise AssertionError("resume repeated a possibly-started tool")
        final = json.load(open(record_path, encoding="utf-8"))
        if final["status"] != "completed" or final.get("pending_tool") is not None:
            raise AssertionError("resumed task did not complete cleanly: %r" % final)
        print("PASS: interrupted durable task resumed without repeating its tool")
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
