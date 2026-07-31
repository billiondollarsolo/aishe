#!/usr/bin/env python3
"""Deterministic end-to-end contract test for the pinned OpenCode runtime.

This intentionally uses the real managed OpenCode binary with a local fake
OpenAI-compatible provider. It proves the boundary that mock Rust adapters
cannot: generated agent/tool policy, provider streaming, the trusted plugin
bridge, foreground execution, usage accounting, and durable session mapping.
The harmless environment probe uses host scope so the bridge contract remains
portable to hosted runners that prohibit bubblewrap namespaces; the separate
workspace-to-host test requires functional bubblewrap on a qualification node.
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
FINAL_TEXT = "managed auto contract passed"
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


class EgressProbeState:
    def __init__(self):
        self.lock = threading.Lock()
        self.requests = []

    def record(self, method, path):
        with self.lock:
            self.requests.append((method, path))


EGRESS = EgressProbeState()


class EgressProbeHandler(http.server.BaseHTTPRequestHandler):
    """Reject and record any request that escapes localhost through a proxy."""

    protocol_version = "HTTP/1.1"

    def log_message(self, _format, *_args):
        return

    def reject(self):
        EGRESS.record(self.command, self.path)
        self.send_response(502)
        self.send_header("content-length", "0")
        self.end_headers()

    do_CONNECT = reject
    do_DELETE = reject
    do_GET = reject
    do_HEAD = reject
    do_OPTIONS = reject
    do_PATCH = reject
    do_POST = reject
    do_PUT = reject


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


def find_persisted_secret(roots, secret):
    """Return the first persisted file containing secret without retaining trees.

    The complete isolated backend tree is intentionally included in the
    credential-leak assertion. Stream every regular file with enough overlap to
    catch a secret split across read boundaries without retaining the tree.
    """
    needle = secret.encode()
    overlap = max(len(needle) - 1, 0)
    for base in roots:
        for path in base.rglob("*"):
            if not path.is_file() or path.is_symlink():
                continue
            tail = b""
            with path.open("rb") as source:
                while chunk_bytes := source.read(1024 * 1024):
                    candidate = tail + chunk_bytes
                    if needle in candidate:
                        return path
                    tail = candidate[-overlap:] if overlap else b""
    return None


def assert_dependency_free_layout(data_home):
    backend = data_home / "aishe" / "backend" / "opencode"
    locks = sorted(backend.rglob("package-lock.json"))
    if len(locks) < 6:
        raise AssertionError(f"isolated runtime loader layouts are missing: {locks}")
    for lock_path in locks:
        root = lock_path.parent
        lock = json.loads(lock_path.read_text(encoding="utf-8"))
        pinned = (
            lock.get("packages", {})
            .get("", {})
            .get("dependencies", {})
            .get("@opencode-ai/plugin")
        )
        if pinned != "1.18.9":
            raise AssertionError(f"offline loader pin mismatch in {lock_path}: {pinned}")
        installed = root / "node_modules" / "@opencode-ai" / "plugin"
        if installed.exists():
            raise AssertionError(
                f"trusted bridge unexpectedly installed a runtime SDK: {installed}"
            )
    npm_caches = list(backend.rglob(".npm"))
    if npm_caches:
        raise AssertionError(f"managed npm caches were not retired: {npm_caches}")

    total = sum(
        path.stat().st_size
        for path in backend.rglob("*")
        if path.is_file() and not path.is_symlink()
    )
    if total > 32 * 1024 * 1024:
        raise AssertionError(
            f"private backend tree unexpectedly exceeds 32 MiB ({total} bytes)"
        )


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
                chunk({"content": FINAL_TEXT}),
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
                                        {"input": {"command": TOOL_COMMAND}},
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


def write_config(path, endpoint, default_scope="host"):
    path.parent.mkdir(parents=True)
    path.write_text(
        f"""version = 5

[aishe]
mode = "auto"
provider = "openai"
show_usage = false
status_line = true

[backend]
engine = "opencode"
fallback = "none"
default_scope = "{default_scope}"
workspace_network = "deny"
output = "focus"
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
    state_paths = sorted(
        (data_home / "aishe" / "backend" / "instances").glob(
            "*/supervisor.json"
        )
    )
    if len(state_paths) != 1:
        raise AssertionError(f"expected one managed runtime state, found {state_paths}")
    state = json.loads(state_paths[0].read_text(encoding="utf-8"))
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
    with STATE.lock:
        STATE.requests.clear()
        STATE.authenticated_requests = 0
    with EGRESS.lock:
        EGRESS.requests.clear()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"
    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), EgressProbeHandler)
    proxy_thread = threading.Thread(target=proxy.serve_forever, daemon=True)
    proxy_thread.start()
    proxy_endpoint = f"http://127.0.0.1:{proxy.server_port}"

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
        backend_root = data_home / "aishe" / "backend" / "opencode"
        for legacy_root in (
            backend_root / "config",
            backend_root / "xdg" / "config" / "opencode",
        ):
            legacy_sdk = legacy_root / "node_modules" / "@opencode-ai" / "plugin"
            legacy_sdk.mkdir(parents=True)
            (legacy_sdk / "package.json").write_text(
                "legacy disposable SDK", encoding="utf-8"
            )
        legacy_npm = backend_root / "home" / ".npm"
        legacy_npm.mkdir(parents=True)
        (legacy_npm / "cache").write_text("legacy disposable cache", encoding="utf-8")
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
                "HTTP_PROXY": proxy_endpoint,
                "HTTPS_PROXY": proxy_endpoint,
                "http_proxy": proxy_endpoint,
                "https_proxy": proxy_endpoint,
                "NO_PROXY": "127.0.0.1,localhost",
                "no_proxy": "127.0.0.1,localhost",
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
                or managed[0].get("scope") != "host"
            ):
                raise AssertionError(f"managed session identity mismatch: {managed[0]}")

            rows = usage_path.read_text(encoding="utf-8").splitlines()
            parsed = [row.split("\t") for row in rows]
            usage_rows = []
            for row in parsed:
                if len(row) == 6 and row[0] == "v2":
                    usage_rows.append(
                        (int(row[1]), int(row[2]), int(row[3]), row[4], row[5])
                    )
                elif len(row) >= 4:
                    # Keep the contract useful against a pre-attribution binary
                    # while the Rust reader separately proves old-file support.
                    usage_rows.append(
                        (int(row[0]), int(row[1]), int(row[2]), "\t".join(row[3:]), None)
                    )
                else:
                    raise AssertionError(f"malformed usage row: {row}")
            totals = tuple(sum(row[index] for row in usage_rows) for index in range(3))
            if totals != (35, 10, 3):
                raise AssertionError(f"usage mapping mismatch: {totals}; rows={rows}")
            if any(row[4] != "openai" for row in usage_rows):
                raise AssertionError(
                    f"usage omitted migrated connection attribution: {usage_rows}"
                )

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
            run_definition = next(
                (
                    item.get("function", {})
                    for item in requests[1].get("tools", [])
                    if item.get("function", {}).get("name") == "aishe_run_command"
                ),
                {},
            )
            parameters = run_definition.get("parameters", {})
            input_schema = parameters.get("properties", {}).get("input", {})
            if (
                parameters.get("required") != ["input"]
                or input_schema.get("type") != "object"
                or input_schema.get("required") != ["command"]
                or input_schema.get("additionalProperties") is not False
            ):
                raise AssertionError(
                    "dependency-free proxy schema contract changed: "
                    f"{json.dumps(parameters, sort_keys=True)}"
                )
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

            leak_path = find_persisted_secret((config_home, data_home), CANARY)
            if leak_path is not None:
                raise AssertionError(
                    "provider credential leaked into persisted Aishe/OpenCode "
                    f"state: {leak_path}"
                )

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
            assert_dependency_free_layout(data_home)
            with EGRESS.lock:
                egress = list(EGRESS.requests)
            if egress:
                raise AssertionError(
                    "managed OpenCode attempted unexpected install-time egress "
                    f"through the denied proxy: {egress}"
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
            proxy.shutdown()
            proxy.server_close()
            proxy_thread.join(timeout=5)


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
