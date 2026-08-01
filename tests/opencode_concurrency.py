#!/usr/bin/env python3
"""Concurrent managed-session isolation against the exact OpenCode runtime."""

import argparse
import concurrent.futures
import datetime
import http.server
import json
import os
import pathlib
import subprocess
import tempfile
import threading
import time

from harness_identity import require_current_binary
from opencode_runtime_contract import (
    CANARY,
    ProviderHandler,
    STATE,
    contains_text,
    write_config,
)
from opencode_soak import read_state, stop_and_assert_reaped


def invoke(binary, base_env, workspace, index):
    sentinel = f"AISHE_CONCURRENT_PROMPT_{index:03d}"
    env = base_env.copy()
    env.update(
        {
            "AISHE_SHELL_ID": f"concurrentshell{index:03d}0123456789abcdef",
            "AISHE_USAGE_FILE": str(workspace / "usage.tsv"),
            "AISHE_STATUS_FILE": str(workspace / "status"),
        }
    )
    started = time.monotonic()
    result = subprocess.run(
        [binary, "suggest", "--json", sentinel],
        cwd=workspace,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=90,
    )
    if result.returncode != 0:
        raise AssertionError(
            f"session {index} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    payload = json.loads(result.stdout)
    if payload.get("kind") != "answer":
        raise AssertionError(f"session {index} returned {payload!r}")
    return sentinel, time.monotonic() - started


def write_report(sessions, elapsed, pids):
    output_dir = pathlib.Path(__file__).resolve().parent.parent / "test-results"
    output_dir.mkdir(exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    path = output_dir / f"opencode-concurrency-{stamp}.md"
    path.write_text(
        "\n".join(
            [
                "# Managed OpenCode concurrency qualification",
                "",
                f"- Date: {stamp}",
                "- Result: **PASS**",
                f"- Isolated concurrent sessions: {sessions}",
                f"- Wall time: {elapsed:.3f}s",
                f"- Supervisor PID: {pids[0]}",
                f"- OpenCode PID: {pids[1]}",
                "",
                "Every workspace/shell identity produced exactly one provider "
                "request and one durable managed session. No external credential "
                "or paid request was used.",
                "",
            ]
        ),
        encoding="utf-8",
    )
    print(f"report: {path.relative_to(output_dir.parent)}")


def qualify(binary, runtime_dir, session_count):
    with STATE.lock:
        STATE.requests.clear()
        STATE.authenticated_requests = 0
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProviderHandler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="aishe-opencode-concurrency-") as root_text:
        root = pathlib.Path(root_text)
        config_home = root / "config"
        data_home = root / "data"
        write_config(config_home / "aishe" / "config.toml", endpoint)
        base_env = os.environ.copy()
        base_env.update(
            {
                "AISHE_CONFIG_DIR": str(config_home),
                "AISHE_DATA_DIR": str(data_home),
                "AISHE_RUNTIME_DIR": str(runtime_dir),
                "XDG_CONFIG_HOME": str(config_home),
                "XDG_DATA_HOME": str(data_home),
                "CONTRACT_PROVIDER_KEY": CANARY,
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )
        workspaces = []
        for index in range(session_count):
            workspace = root / f"workspace-{index:03d}"
            workspace.mkdir()
            (workspace / ".git").mkdir()
            workspaces.append(workspace)

        state = None
        try:
            started = time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(
                max_workers=session_count
            ) as executor:
                futures = [
                    executor.submit(invoke, binary, base_env, workspace, index)
                    for index, workspace in enumerate(workspaces)
                ]
                results = [future.result() for future in futures]
            elapsed = time.monotonic() - started

            sentinels = [sentinel for sentinel, _ in results]
            with STATE.lock:
                requests = list(STATE.requests)
                authenticated = STATE.authenticated_requests
            if len(requests) != session_count or authenticated != session_count:
                raise AssertionError(
                    f"expected {session_count} authenticated requests, got "
                    f"{len(requests)}/{authenticated}"
                )
            for sentinel in sentinels:
                matches = sum(contains_text(request, sentinel) for request in requests)
                if matches != 1:
                    raise AssertionError(
                        f"prompt isolation failed for {sentinel}: {matches} request(s)"
                    )
            for request in requests:
                present = sum(contains_text(request, value) for value in sentinels)
                if present != 1:
                    raise AssertionError(
                        f"provider request mixed concurrent prompts: {present}"
                    )

            env = base_env.copy()
            env["AISHE_SHELL_ID"] = "concurrencycontrol0123456789abcdef"
            listing = subprocess.run(
                [binary, "sessions", "--json"],
                cwd=root,
                env=env,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=30,
                check=True,
            )
            managed = json.loads(listing.stdout).get("managed", [])
            if len(managed) != session_count:
                raise AssertionError(
                    f"expected {session_count} durable sessions, got {len(managed)}"
                )
            workspace_ids = {item.get("workspace") for item in managed}
            expected_ids = {str(path.resolve()) for path in workspaces}
            if workspace_ids != expected_ids:
                raise AssertionError("durable session workspaces were cross-contaminated")

            state = read_state(data_home)
            write_report(
                session_count,
                elapsed,
                (state["supervisor_pid"], state["opencode_pid"]),
            )
            print(
                f"PASS: {session_count} concurrent managed sessions remained isolated "
                f"in {elapsed:.3f}s"
            )
        finally:
            stop_and_assert_reaped(binary, base_env, root, state, data_home)
            server.shutdown()
            server.server_close()
            server_thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--sessions", type=int, default=10)
    args = parser.parse_args()
    if not 1 <= args.sessions <= 100:
        parser.error("--sessions must be between 1 and 100")
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    qualify(
        require_current_binary(args.binary),
        pathlib.Path(runtime).resolve(),
        args.sessions,
    )


if __name__ == "__main__":
    main()
