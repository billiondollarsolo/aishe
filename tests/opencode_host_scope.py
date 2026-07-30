#!/usr/bin/env python3
"""Real pinned-runtime contract for workspace-to-host authority rotation.

The deterministic loopback provider first runs a workspace-scoped turn, then
switches the same Aishe shell to host scope and writes outside the workspace.
This proves the managed conversation rotates with authority, the authenticated
tool bridge preserves host scope, reset is non-destructive, and focus output
does not leave routine agent scaffolding in terminal scrollback.

Set AISHE_TEST_REQUIRE_BWRAP=1 on a capable Linux qualification node to require
the functional bubblewrap path. Restricted hosted runners retain policy-only
workspace checks while still exercising the complete authority transition.
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
    contract.FINAL_TEXT = (
        "# Current capacity\n\n"
        "- **managed auto contract passed**\n\n"
        "```bash\n"
        "printf 'ok\\n'\n"
        "```\n"
    )

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), contract.ProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    shell_id = f"hostscope{os.getpid():016d}"
    acceptance = pathlib.Path(tempfile.gettempdir()) / f"aishe-yolo-accept-{shell_id}"
    acceptance.write_text("workspace\n", encoding="utf-8")
    acceptance.chmod(0o600)

    with tempfile.TemporaryDirectory(prefix="aishe-opencode-host-") as root_text:
        root = pathlib.Path(root_text)
        config_home = root / "config"
        data_home = root / "data"
        workspace = root / "workspace"
        workspace.mkdir()
        (workspace / ".git").mkdir()
        workspace_target = workspace / "workspace-scope.txt"
        host_target = root / "outside-workspace" / "host-scope.txt"
        host_target.parent.mkdir()
        marker = "managed yolo host scope passed"
        config_path = config_home / "aishe" / "config.toml"
        contract.write_config(config_path, endpoint)
        config = config_path.read_text(encoding="utf-8")
        config = config.replace('mode = "auto"', 'mode = "yolo"')
        if os.environ.get("AISHE_TEST_REQUIRE_BWRAP") == "1":
            config = config.replace(
                'linux_backend = "policy"', 'linux_backend = "bwrap"'
            )
            config = config.replace(
                "require_functional = false",
                "require_functional = true\nallow_host_yolo = true",
            )
        else:
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
            contract.TOOL_COMMAND = (
                f"printf '%s\\n' workspace-first > {shlex.quote(str(workspace_target))}"
            )
            workspace_result = run(
                binary,
                env,
                workspace,
                "--yolo-line",
                "write the workspace-scope contract marker",
            )
            if workspace_target.read_text(encoding="utf-8") != "workspace-first\n":
                raise AssertionError("workspace-scoped tool did not write its project marker")
            first_sessions = json.loads(
                run(binary, env, workspace, "sessions", "--json").stdout
            ).get("managed", [])
            if len(first_sessions) != 1 or first_sessions[0].get("scope") != "workspace":
                raise AssertionError(
                    f"durable session omitted workspace scope: {first_sessions}"
                )
            first_session_id = first_sessions[0]["backend_session_id"]

            config = config_path.read_text(encoding="utf-8").replace(
                'default_scope = "workspace"', 'default_scope = "host"'
            )
            config_path.write_text(config, encoding="utf-8")
            acceptance.write_text("host\n", encoding="utf-8")
            contract.TOOL_COMMAND = (
                f"printf '%s\\n' {shlex.quote(marker)} > {shlex.quote(str(host_target))}"
            )
            host_result = run(
                binary,
                env,
                workspace,
                "--yolo-line",
                "write the host-scope contract marker",
            )
            rendered = (
                workspace_result.stdout
                + workspace_result.stderr
                + host_result.stdout
                + host_result.stderr
            )
            if "Type yolo" in rendered or "Approve this action" in rendered:
                raise AssertionError(
                    f"accepted yolo emitted a per-action approval:\n{rendered}"
                )
            for noise in ("○ queued", "● running", "✓ ran", '{"type":"answer"'):
                if noise in rendered:
                    raise AssertionError(
                        f"focus output leaked routine agent scaffolding {noise!r}:\n{rendered}"
                    )
            if "\\x0a" in rendered:
                raise AssertionError(
                    f"focus output escaped Markdown line breaks:\n{rendered}"
                )
            markdown = (
                "# Current capacity\n\n"
                "- **managed auto contract passed**\n\n"
                "```bash\n"
                "printf 'ok\\n'\n"
                "```\n"
            )
            if rendered.count(markdown) != 2:
                raise AssertionError(
                    f"focus output did not preserve Markdown structure:\n{rendered}"
                )
            if rendered.count("managed auto contract passed") != 2:
                raise AssertionError(f"focus output omitted a final response:\n{rendered}")
            if host_target.read_text(encoding="utf-8") != f"{marker}\n":
                raise AssertionError("host-scope tool did not write outside the workspace")

            sessions = json.loads(
                run(binary, env, workspace, "sessions", "--json").stdout
            )
            managed = sessions.get("managed", [])
            if len(managed) != 1 or managed[0].get("scope") != "host":
                raise AssertionError(f"durable session omitted host scope: {sessions}")
            host_session_id = managed[0]["backend_session_id"]
            if host_session_id == first_session_id:
                raise AssertionError(
                    "workspace-to-host switch reused a stale authority conversation"
                )
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
                len(calls) != 2
                or any(call.get("tool") != "run_command" for call in calls)
                or any(call.get("status") != "completed" for call in calls)
            ):
                raise AssertionError(f"host tool journal mismatch: {journal}")

            reset = run(binary, env, workspace, "reset")
            if "previous session retained" not in reset.stdout:
                raise AssertionError(f"reset did not report retained session:\n{reset.stdout}")
            after_reset = json.loads(
                run(binary, env, workspace, "sessions", "--json").stdout
            )
            if after_reset.get("managed"):
                raise AssertionError(f"reset left an active mapping: {after_reset}")
            resumed = run(binary, env, workspace, "resume", host_session_id)
            if f"resumed managed session {host_session_id}" not in resumed.stdout:
                raise AssertionError(f"retained session could not resume:\n{resumed.stdout}")

            with contract.STATE.lock:
                request_count = len(contract.STATE.requests)
                authenticated = contract.STATE.authenticated_requests
            if request_count != 4 or authenticated != 4:
                raise AssertionError(
                    f"expected four authenticated provider turns, got "
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
        "PASS: workspace-to-host authority rotated conversations, focus output "
        "stayed clean, host execution succeeded, and reset remained resumable"
    )


if __name__ == "__main__":
    main()
