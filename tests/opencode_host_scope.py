#!/usr/bin/env python3
"""Real pinned-runtime contract for accepted yolo host scope.

The deterministic loopback provider requests one command that writes outside
the workspace but inside a disposable test root. This proves that the managed
OpenCode loop, authenticated tool bridge, per-shell acceptance marker, and
Aishe foreground worker preserve the explicit host-scope authority boundary
without displaying a second per-action approval.
"""

import http.server
import json
import os
import pathlib
import shlex
import subprocess
import sys
import tempfile
import threading

import opencode_runtime_contract as contract


def run(binary, env, cwd, *args, timeout=90):
    result = subprocess.run(
        [binary, *args],
        cwd=cwd,
        env=env,
        text=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    if result.returncode != 0:
        raise AssertionError(
            f"{' '.join(args)} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result


def main():
    if len(sys.argv) != 2:
        raise SystemExit("usage: opencode_host_scope.py /path/to/aishe")
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    binary = str(pathlib.Path(sys.argv[1]).resolve())
    with contract.STATE.lock:
        contract.STATE.requests.clear()
        contract.STATE.authenticated_requests = 0

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), contract.ProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    shell_id = f"hostscope{os.getpid():016d}"
    acceptance = pathlib.Path(tempfile.gettempdir()) / f"aishe-yolo-accept-{shell_id}"
    acceptance.write_text("host\n", encoding="utf-8")
    acceptance.chmod(0o600)

    with tempfile.TemporaryDirectory(prefix="aishe-opencode-host-") as root_text:
        root = pathlib.Path(root_text)
        config_home = root / "config"
        data_home = root / "data"
        workspace = root / "workspace"
        workspace.mkdir()
        (workspace / ".git").mkdir()
        target = root / "outside-workspace" / "host-scope.txt"
        target.parent.mkdir()
        marker = "managed yolo host scope passed"
        contract.TOOL_COMMAND = (
            f"printf '%s\\n' {shlex.quote(marker)} > {shlex.quote(str(target))}"
        )
        config_path = config_home / "aishe" / "config.toml"
        contract.write_config(config_path, endpoint)
        config = config_path.read_text(encoding="utf-8")
        config = config.replace('mode = "auto"', 'mode = "yolo"')
        config = config.replace(
            'default_scope = "workspace"', 'default_scope = "host"'
        )
        config = config.replace(
            "require_functional = false",
            "require_functional = false\nallow_host_yolo = true",
        )
        config_path.write_text(config, encoding="utf-8")

        env = os.environ.copy()
        env.update(
            {
                "AISHE_CONFIG_DIR": str(config_home),
                "AISHE_DATA_DIR": str(data_home),
                "AISHE_RUNTIME_DIR": str(pathlib.Path(runtime).resolve()),
                "XDG_CONFIG_HOME": str(config_home),
                "XDG_DATA_HOME": str(data_home),
                "CONTRACT_PROVIDER_KEY": contract.CANARY,
                "AISHE_SHELL_ID": shell_id,
                "AISHE_ACCEPTANCE_FILE": str(acceptance),
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )
        try:
            result = run(
                binary,
                env,
                workspace,
                "--yolo-line",
                "write the host-scope contract marker",
            )
            rendered = result.stdout + result.stderr
            if "Type yolo" in rendered or "Approve this action" in rendered:
                raise AssertionError(
                    f"accepted yolo emitted a per-action approval:\n{rendered}"
                )
            if target.read_text(encoding="utf-8") != f"{marker}\n":
                raise AssertionError("host-scope tool did not write outside the workspace")

            sessions = json.loads(
                run(binary, env, workspace, "sessions", "--json").stdout
            )
            managed = sessions.get("managed", [])
            if len(managed) != 1 or managed[0].get("scope") != "host":
                raise AssertionError(f"durable session omitted host scope: {sessions}")
            journal = json.loads(
                (
                    data_home
                    / "aishe"
                    / "backend"
                    / "journal"
                    / "tool-calls.json"
                ).read_text(encoding="utf-8")
            )
            calls = journal.get("calls", [])
            if (
                len(calls) != 1
                or calls[0].get("tool") != "run_command"
                or calls[0].get("status") != "completed"
            ):
                raise AssertionError(f"host tool journal mismatch: {journal}")
            with contract.STATE.lock:
                request_count = len(contract.STATE.requests)
                authenticated = contract.STATE.authenticated_requests
            if request_count != 2 or authenticated != 2:
                raise AssertionError(
                    f"expected two authenticated provider turns, got "
                    f"{request_count}/{authenticated}"
                )
        finally:
            subprocess.run(
                [binary, "backend", "stop"],
                cwd=workspace,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=15,
                check=False,
            )
            acceptance.unlink(missing_ok=True)
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)

    print(
        "PASS: accepted yolo host scope crossed the workspace boundary through "
        "one authenticated Aishe tool lease without a per-action approval"
    )


if __name__ == "__main__":
    main()
