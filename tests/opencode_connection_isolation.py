#!/usr/bin/env python3
"""Same-provider, same-model credential and runtime isolation qualification."""

import concurrent.futures
import http.server
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import time

from harness_identity import require_current_binary
from opencode_runtime_contract import chunk, find_persisted_secret


MODEL = "aishe-isolation-model"
WORK_KEY = "aishe-work-key-never-persist-000000000000"
PERSONAL_KEY = "aishe-personal-key-never-persist-111111111111"
KEYS = {
    f"Bearer {WORK_KEY}": "work",
    f"Bearer {PERSONAL_KEY}": "personal",
}


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = []

    def record(self, authorization, body):
        with self.lock:
            self.requests.append((authorization, body))


STATE = State()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        self.send_json({"data": [{"id": MODEL}]})

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = json.loads(self.rfile.read(length))
        authorization = self.headers.get("authorization", "")
        identity = KEYS.get(authorization)
        STATE.record(authorization, body)
        if identity is None:
            self.send_json({"error": {"message": "wrong credential"}}, status=401)
            return
        answer = json.dumps(
            {
                "type": "answer",
                "command": "",
                "explanation": f"credential:{identity}",
            },
            separators=(",", ":"),
        )
        self.send_sse(
            [
                chunk({"role": "assistant"}),
                chunk({"content": answer}),
                chunk({}, "stop", (5, 2)),
            ]
        )

    def send_json(self, value, status=200):
        body = json.dumps(value, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.close_connection = True

    def send_sse(self, events):
        body = "".join(
            f"data: {json.dumps(event, separators=(',', ':'))}\n\n"
            for event in events
        )
        encoded = (body + "data: [DONE]\n\n").encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(encoded)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(encoded)
        self.close_connection = True


def write_config(path, endpoint):
    path.parent.mkdir(parents=True)
    path.write_text(
        f'''version = 7

[aishe]
mode = "suggest"
provider = "openai"
connection = "openai-work"
connection_fallback = "openai-work"
show_usage = false

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "{endpoint}"
model = "{MODEL}"
transport = "chat"
auth_required = true
[connections.openai-work.auth]
type = "api_key"
credential = "work"
api_key_env = "AISHE_ISOLATION_WORK_KEY"

[connections.openai-personal]
provider = "openai"
label = "OpenAI personal"
base_url = "{endpoint}"
model = "{MODEL}"
transport = "chat"
auth_required = true
[connections.openai-personal.auth]
type = "api_key"
credential = "personal"
api_key_env = "AISHE_ISOLATION_PERSONAL_KEY"

[backend]
engine = "opencode"
fallback = "none"
default_scope = "host"
workspace_network = "deny"
max_instances = 8

[logging]
enabled = true
redact = true
''',
        encoding="utf-8",
    )


def invoke(binary, env, workspace, connection, identity):
    call_env = env.copy()
    call_env["AISHE_SHELL_ID"] = f"isolation-{identity}-0123456789abcdef"
    result = subprocess.run(
        [binary, "--connection", connection, "suggest", "--json", identity],
        cwd=workspace,
        env=call_env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=90,
    )
    if result.returncode != 0:
        raise AssertionError(
            f"{connection} failed ({result.returncode})\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    payload = json.loads(result.stdout)
    if payload.get("explanation") != f"credential:{identity}":
        raise AssertionError(f"{connection} crossed credential identity: {payload}")
    return payload


def process_exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: opencode_connection_isolation.py /path/to/aishe")
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    binary = require_current_binary(sys.argv[1])
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="aishe-connection-isolation-") as root_text:
        root = pathlib.Path(root_text)
        config_home = root / "config"
        data_home = root / "data"
        workspace = root / "workspace"
        workspace.mkdir()
        (workspace / ".git").mkdir()
        write_config(config_home / "aishe" / "config.toml", endpoint)
        env = os.environ.copy()
        env.update(
            {
                "AISHE_CONFIG_DIR": str(config_home),
                "AISHE_DATA_DIR": str(data_home),
                "AISHE_RUNTIME_DIR": str(pathlib.Path(runtime).resolve()),
                "XDG_CONFIG_HOME": str(config_home),
                "XDG_DATA_HOME": str(data_home),
                "AISHE_ISOLATION_WORK_KEY": WORK_KEY,
                "AISHE_ISOLATION_PERSONAL_KEY": PERSONAL_KEY,
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )
        states = []
        try:
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
                futures = [
                    executor.submit(
                        invoke,
                        binary,
                        env,
                        workspace,
                        "openai-work",
                        "work",
                    ),
                    executor.submit(
                        invoke,
                        binary,
                        env,
                        workspace,
                        "openai-personal",
                        "personal",
                    ),
                ]
                [future.result() for future in futures]

            state_paths = sorted(
                (data_home / "aishe" / "backend" / "instances").glob(
                    "*/supervisor.json"
                )
            )
            states = [json.loads(path.read_text(encoding="utf-8")) for path in state_paths]
            if {state.get("connection_id") for state in states} != {
                "openai-work",
                "openai-personal",
            }:
                raise AssertionError(f"runtime pool did not isolate connections: {states}")
            if len({state["opencode_pid"] for state in states}) != 2:
                raise AssertionError("connections reused one OpenCode process")

            with STATE.lock:
                authorizations = [authorization for authorization, _ in STATE.requests]
            if sorted(authorizations) != sorted(KEYS):
                raise AssertionError(f"provider observed crossed credentials: {authorizations}")

            for secret in (env["AISHE_ISOLATION_WORK_KEY"], env["AISHE_ISOLATION_PERSONAL_KEY"]):
                leaked = find_persisted_secret((config_home, data_home), secret)
                if leaked is not None:
                    raise AssertionError(f"credential persisted at {leaked}")
            audit = (data_home / "aishe" / "audit.jsonl").read_text(encoding="utf-8")
            if "openai-work" not in audit or "openai-personal" not in audit:
                raise AssertionError("audit omitted safe connection attribution")
            if WORK_KEY in audit or PERSONAL_KEY in audit:
                raise AssertionError("audit contained provider credentials")
            print("PASS: duplicate-provider credentials used two isolated runtimes and sessions")
        finally:
            subprocess.run(
                [binary, "backend", "stop"],
                cwd=workspace,
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=20,
                check=False,
            )
            pids = [
                pid
                for state in states
                for pid in (state["supervisor_pid"], state["opencode_pid"])
            ]
            deadline = time.monotonic() + 8
            while any(process_exists(pid) for pid in pids) and time.monotonic() < deadline:
                time.sleep(0.05)
            survivors = [pid for pid in pids if process_exists(pid)]
            if survivors:
                raise AssertionError(f"isolated runtimes survived stop: {survivors}")
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


if __name__ == "__main__":
    main()
