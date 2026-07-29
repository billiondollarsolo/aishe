#!/usr/bin/env python3
"""Deterministic end-to-end contract test for the pinned OpenCode runtime.

This intentionally uses the real managed OpenCode binary with a local fake
OpenAI-compatible provider. It proves the boundary that mock Rust adapters
cannot: generated agent/tool policy, provider streaming, the trusted plugin
bridge, foreground execution, usage accounting, and durable session mapping.
"""

import http.server
import base64
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import threading
import urllib.parse
import urllib.error
import urllib.request


MODEL = "aishe-contract-model"
CANARY = "sk-proj-contract-canary-0123456789abcdefghijklmnopqrstuvwxyz"
TOOL_COMMAND = "env"
FORBIDDEN_HOST_TOOLS = {
    "bash",
    "read",
    "write",
    "edit",
    "patch",
    "apply_patch",
    "glob",
    "grep",
    "webfetch",
    "websearch",
    "skill",
    "lsp",
}


class ContractState:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = []
        self.authenticated_requests = 0

    def record(self, body, authorization):
        with self.lock:
            self.requests.append(body)
            if authorization == f"Bearer {CANARY}":
                self.authenticated_requests += 1


STATE = ContractState()


def chunk(delta=None, finish=None, usage=None):
    choice = {"index": 0, "delta": delta or {}}
    if finish is not None:
        choice["finish_reason"] = finish
    value = {
        "id": "chatcmpl-aishe-contract",
        "object": "chat.completion.chunk",
        "model": MODEL,
        "choices": [choice],
    }
    if usage is not None:
        value["usage"] = {
            "prompt_tokens": usage[0],
            "completion_tokens": usage[1],
            "total_tokens": usage[0] + usage[1],
        }
    return value


def tool_names(body):
    names = []
    for item in body.get("tools", []):
        if not isinstance(item, dict):
            continue
        function = item.get("function", {})
        if isinstance(function, dict) and isinstance(function.get("name"), str):
            names.append(function["name"])
    return names


def contains_text(value, needle):
    if isinstance(value, str):
        return needle in value
    if isinstance(value, list):
        return any(contains_text(item, needle) for item in value)
    if isinstance(value, dict):
        return any(contains_text(item, needle) for item in value.values())
    return False


def tool_result_messages(body):
    return [
        message
        for message in body.get("messages", [])
        if isinstance(message, dict) and message.get("role") == "tool"
    ]


class ProviderHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        return

    def do_GET(self):
        if self.path.rstrip("/") == "/v1/models":
            self.send_json({"data": [{"id": MODEL}]})
            return
        self.send_json({"error": "not found"}, status=404)

    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        if length <= 0 or length > 4 * 1024 * 1024:
            self.send_json({"error": "invalid body"}, status=400)
            return
        try:
            body = json.loads(self.rfile.read(length))
        except (ValueError, UnicodeDecodeError):
            self.send_json({"error": "invalid json"}, status=400)
            return
        if self.path.rstrip("/") != "/v1/chat/completions":
            self.send_json({"error": "not found"}, status=404)
            return

        STATE.record(body, self.headers.get("authorization", ""))
        names = tool_names(body)
        if not names:
            events = [
                chunk({"role": "assistant"}),
                chunk(
                    {
                        "content": json.dumps(
                            {
                                "type": "answer",
                                "command": "",
                                "explanation": "managed suggest contract passed",
                            },
                            separators=(",", ":"),
                        )
                    }
                ),
                chunk({}, "stop", (7, 2)),
            ]
        elif tool_result_messages(body):
            events = [
                chunk({"role": "assistant"}),
                chunk({"content": "managed auto contract passed"}),
                chunk({}, "stop", (17, 5)),
            ]
        else:
            events = [
                chunk({"role": "assistant"}),
                chunk(
                    {
                        "tool_calls": [
                            {
                                "index": 0,
                                "id": "call_aishe_contract",
                                "type": "function",
                                "function": {
                                    "name": "aishe_run_command",
                                    "arguments": "",
                                },
                            }
                        ]
                    }
                ),
                chunk(
                    {
                        "tool_calls": [
                            {
                                "index": 0,
                                "function": {
                                    "arguments": json.dumps(
                                        {"command": TOOL_COMMAND},
                                        separators=(",", ":"),
                                    )
                                },
                            }
                        ]
                    }
                ),
                chunk({}, "tool_calls", (11, 3)),
            ]
        self.send_sse(events)

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
        body += "data: [DONE]\n\n"
        encoded = body.encode()
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("content-length", str(len(encoded)))
        self.send_header("connection", "close")
        self.end_headers()
        self.wfile.write(encoded)
        self.wfile.flush()
        self.close_connection = True


def run(binary, env, cwd, *args, timeout=60):
    try:
        result = subprocess.run(
            [binary, *args],
            cwd=cwd,
            env=env,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as error:
        with STATE.lock:
            summaries = [
                {
                    "tools": tool_names(body),
                    "has_tool_result": bool(tool_result_messages(body)),
                }
                for body in STATE.requests
            ]
        raise AssertionError(
            f"{' '.join(args)} timed out after {timeout}s; requests={summaries}"
        ) from error
    if result.returncode != 0:
        raise AssertionError(
            f"{' '.join(args)} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def write_config(path, endpoint):
    path.parent.mkdir(parents=True)
    path.write_text(
        f"""version = 4

[aishe]
mode = "auto"
provider = "openai"
show_usage = false
status_line = true

[backend]
engine = "opencode"
fallback = "none"
default_scope = "workspace"
workspace_network = "deny"
output = "compact"
max_output_tokens = 512

[sandbox]
linux_backend = "policy"
require_functional = false

[providers.openai]
base_url = "{endpoint}"
credential = "contract"
api_key_env = "CONTRACT_PROVIDER_KEY"
model = "{MODEL}"
transport = "chat"
auth_required = true

[pricing.{MODEL}]
input = 1.0
output = 2.0
""",
        encoding="utf-8",
    )


def read_managed_snapshot(data_home, workspace):
    state = json.loads(
        (data_home / "aishe" / "backend" / "supervisor.json").read_text(
            encoding="utf-8"
        )
    )
    mappings = json.loads(
        (
            data_home
            / "aishe"
            / "backend"
            / "sessions"
            / "mappings.json"
        ).read_text(encoding="utf-8")
    )
    session_id = mappings["records"][0]["backend_session_id"]
    authorization = base64.b64encode(
        f"aishe:{state['opencode_password']}".encode()
    ).decode()

    def get(path):
        query = urllib.parse.urlencode({"directory": str(workspace)})
        request = urllib.request.Request(
            f"{state['opencode_url']}{path}?{query}",
            headers={"Authorization": f"Basic {authorization}"},
        )
        try:
            with urllib.request.urlopen(request, timeout=10) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", "replace")
            raise AssertionError(
                f"OpenCode snapshot GET {path} failed with {error.code}: {detail}"
            ) from error

    return get("/session/status"), get(f"/session/{session_id}/message")


def assert_runtime_contract(binary, runtime_dir):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="aishe-opencode-contract-") as root_text:
        root = pathlib.Path(root_text)
        config_home = root / "config"
        data_home = root / "data"
        workspace = root / "workspace"
        workspace.mkdir()
        (workspace / ".git").mkdir()
        config_path = config_home / "aishe" / "config.toml"
        usage_path = root / "usage.tsv"
        status_path = root / "status"
        write_config(config_path, endpoint)

        env = os.environ.copy()
        env.update(
            {
                "AISHE_CONFIG_DIR": str(config_home),
                "AISHE_DATA_DIR": str(data_home),
                "AISHE_RUNTIME_DIR": str(runtime_dir),
                "XDG_CONFIG_HOME": str(config_home),
                "XDG_DATA_HOME": str(data_home),
                "CONTRACT_PROVIDER_KEY": CANARY,
                "AISHE_SHELL_ID": "contractshell0123456789abcdef",
                "AISHE_USAGE_FILE": str(usage_path),
                "AISHE_STATUS_FILE": str(status_path),
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )

        try:
            suggested = run(
                binary,
                env,
                workspace,
                "suggest",
                "--json",
                "prove managed suggest",
                timeout=90,
            )
            suggestion = json.loads(suggested.stdout)
            if suggestion.get("kind") != "answer":
                raise AssertionError(f"unexpected managed suggestion: {suggestion}")
            if suggestion.get("explanation") != "managed suggest contract passed":
                raise AssertionError(f"managed answer was not preserved: {suggestion}")
            status, messages = read_managed_snapshot(data_home, workspace)
            if not isinstance(status, dict) or not isinstance(messages, list):
                raise AssertionError(
                    f"managed snapshot contract mismatch: {status!r}/{messages!r}"
                )

            automatic = run(
                binary,
                env,
                workspace,
                "--auto-line",
                "prove the managed tool bridge",
                timeout=30,
            )
            rendered = automatic.stdout + automatic.stderr
            if "managed auto contract passed" not in rendered:
                post_status, post_messages = read_managed_snapshot(
                    data_home, workspace
                )
                logs = run(binary, env, workspace, "backend", "logs").stdout
                with STATE.lock:
                    summaries = [
                        {
                            "tools": tool_names(body),
                            "has_tool_result": bool(tool_result_messages(body)),
                        }
                        for body in STATE.requests
                    ]
                raise AssertionError(
                    f"managed final text missing:\n{rendered}\nrequests={summaries}\n"
                    f"status={post_status}\nmessages={post_messages}\nlogs={logs}"
                )

            sessions = json.loads(
                run(binary, env, workspace, "sessions", "--json").stdout
            )
            managed = sessions.get("managed", [])
            if sessions.get("schema_version") != 1 or len(managed) != 1:
                raise AssertionError(f"unexpected unified sessions: {sessions}")
            if (
                managed[0].get("backend") != "opencode"
                or managed[0].get("mode") != "auto"
                or managed[0].get("scope") != "workspace"
            ):
                raise AssertionError(f"managed session identity mismatch: {managed[0]}")

            rows = usage_path.read_text(encoding="utf-8").splitlines()
            parsed = [row.split("\t") for row in rows]
            totals = tuple(sum(int(row[index]) for row in parsed) for index in range(3))
            if totals != (35, 10, 3):
                raise AssertionError(f"usage mapping mismatch: {totals}; rows={rows}")

            with STATE.lock:
                requests = list(STATE.requests)
                authenticated = STATE.authenticated_requests
            if len(requests) != 3 or authenticated != 3:
                raise AssertionError(
                    f"expected 3 authenticated provider turns, got "
                    f"{len(requests)} requests/{authenticated} authenticated"
                )
            if tool_names(requests[0]):
                raise AssertionError(
                    f"suggest exposed model tools: {tool_names(requests[0])}"
                )
            auto_tools = set(tool_names(requests[1]))
            if "aishe_run_command" not in auto_tools:
                raise AssertionError(f"Aishe proxy tools missing: {sorted(auto_tools)}")
            forbidden = auto_tools & FORBIDDEN_HOST_TOOLS
            if forbidden:
                raise AssertionError(
                    f"OpenCode host tools reached the model: {sorted(forbidden)}"
                )
            unexpected = {
                name
                for name in auto_tools
                if not name.startswith("aishe_")
                and name not in {"task", "todowrite", "todoread"}
            }
            if unexpected:
                raise AssertionError(
                    f"unexpected non-proxy model tools: {sorted(unexpected)}"
                )
            tool_results = tool_result_messages(requests[2])
            if not tool_results:
                raise AssertionError("tool result was not returned to the next provider turn")
            serialized_results = json.dumps(tool_results)
            if "PATH=" not in serialized_results:
                raise AssertionError(
                    f"environment probe did not execute through the tool bridge: {tool_results}"
                )
            leaked = [
                marker
                for marker in (
                    CANARY,
                    "CONTRACT_PROVIDER_KEY=",
                    "AISHE_PROVIDER_API_KEY=",
                )
                if marker in serialized_results
            ]
            if leaked:
                raise AssertionError(
                    f"provider credentials reached the tool subprocess: {leaked}"
                )

            journal_path = (
                data_home / "aishe" / "backend" / "journal" / "tool-calls.json"
            )
            journal = json.loads(journal_path.read_text(encoding="utf-8"))
            calls = journal.get("calls", [])
            if (
                len(calls) != 1
                or calls[0].get("tool") != "run_command"
                or calls[0].get("status") != "completed"
            ):
                raise AssertionError(
                    "durable tool journal omitted the completed proxy action:\n"
                    f"{json.dumps(journal, indent=2)}"
                )

            persisted = b""
            for base in (config_home, data_home):
                for path in base.rglob("*"):
                    if path.is_file() and not path.is_symlink():
                        persisted += path.read_bytes()
            if CANARY.encode() in persisted:
                raise AssertionError("provider credential leaked into persisted Aishe/OpenCode state")

            validation = run(
                binary,
                env,
                workspace,
                "setup",
                "--verify",
                "--live",
                "--json",
                timeout=120,
            )
            validation_report = json.loads(validation.stdout)
            provider_report = validation_report.get("provider", {})
            for check in ("text", "structured", "tools", "streaming"):
                state = provider_report.get(check, {}).get("state")
                if state != "pass":
                    raise AssertionError(
                        f"managed setup validation {check} did not pass: "
                        f"{json.dumps(validation_report, indent=2)}"
                    )
            if validation_report.get("backend") != "opencode":
                raise AssertionError(
                    f"setup certified the wrong backend: {validation_report}"
                )
            with STATE.lock:
                validation_requests = list(STATE.requests[3:])
            if len(validation_requests) != 3:
                raise AssertionError(
                    "managed setup validation did not use the expected "
                    f"structured + proxy-tool loop: {len(validation_requests)} request(s)"
                )
            if tool_names(validation_requests[0]):
                raise AssertionError("managed setup suggest validation exposed tools")
            if "aishe_run_command" not in tool_names(validation_requests[1]):
                raise AssertionError("managed setup tool validation omitted Aishe proxies")
            validation_tool_results = json.dumps(
                tool_result_messages(validation_requests[2])
            )
            if CANARY in validation_tool_results:
                raise AssertionError(
                    "managed setup validation leaked the provider credential"
                )
        finally:
            subprocess.run(
                [binary, "backend", "stop"],
                cwd=workspace,
                env=env,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=15,
                check=False,
            )
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: opencode_runtime_contract.py /path/to/aishe")
    binary = str(pathlib.Path(sys.argv[1]).resolve())
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    assert_runtime_contract(binary, pathlib.Path(runtime).resolve())
    print(
        "PASS: pinned OpenCode provider, tool bridge, credential isolation, "
        "usage, and session contract"
    )


if __name__ == "__main__":
    main()
