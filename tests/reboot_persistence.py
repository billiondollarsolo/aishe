#!/usr/bin/env python3
"""Prepare/verify durable Aishe state across a real host reboot.

This is an explicit disposable-node release gate, not a normal CI test:

  AISHE_RUNTIME_DIR=... reboot_persistence.py prepare BIN ROOT
  # reboot the node
  AISHE_RUNTIME_DIR=... reboot_persistence.py verify BIN ROOT

The prepare phase creates one real managed OpenCode conversation plus Aishe
history, stops the backend, and records a hash manifest. The verify phase first
proves every persisted byte survived the reboot, then restarts the same
loopback provider and managed conversation and verifies its prior turn remains
in provider context. No external credential or paid request is used.
"""

import hashlib
import json
import os
import pathlib
import sys
import threading
import urllib.parse

import opencode_runtime_contract as contract


BEFORE = "AISHE_REBOOT_CONTEXT_BEFORE_42"
AFTER = "AISHE_REBOOT_CONTEXT_AFTER_42"
HISTORY = "AISHE_REBOOT_HISTORY_42"
SHELL_ID = "rebootshell0123456789abcdef"


def usage():
    raise SystemExit(
        "usage: reboot_persistence.py prepare|verify /path/to/aishe /persistent/root"
    )


def digest_tree(root):
    result = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            result[str(path.relative_to(root))] = {
                "kind": "symlink",
                "target": os.readlink(path),
            }
        elif path.is_file():
            result[str(path.relative_to(root))] = {
                "kind": "file",
                "size": path.stat().st_size,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            }
    return result


def environment(root, runtime):
    config = root / "config"
    data = root / "data"
    env = os.environ.copy()
    env.update(
        {
            "AISHE_CONFIG_DIR": str(config),
            "AISHE_DATA_DIR": str(data),
            "AISHE_RUNTIME_DIR": str(runtime),
            "XDG_CONFIG_HOME": str(config),
            "XDG_DATA_HOME": str(data),
            "CONTRACT_PROVIDER_KEY": contract.CANARY,
            "AISHE_SHELL_ID": SHELL_ID,
            "NO_COLOR": "1",
            "TERM": "dumb",
        }
    )
    return env


def start_provider(port=0):
    with contract.STATE.lock:
        contract.STATE.requests.clear()
        contract.STATE.authenticated_requests = 0
    server = contract.http.server.ThreadingHTTPServer(
        ("127.0.0.1", port), contract.ProviderHandler
    )
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread


def stop_provider(server, thread):
    server.shutdown()
    server.server_close()
    thread.join(timeout=5)


def run(binary, env, workspace, *args):
    return contract.run(binary, env, workspace, *args, timeout=120)


def stop_backend(binary, env, workspace):
    contract.subprocess.run(
        [binary, "backend", "stop"],
        cwd=workspace,
        env=env,
        stdout=contract.subprocess.DEVNULL,
        stderr=contract.subprocess.DEVNULL,
        timeout=20,
        check=False,
    )


def prepare(binary, root, runtime):
    if root.exists():
        raise SystemExit(f"refusing to overwrite existing reboot fixture: {root}")
    workspace = root / "workspace"
    workspace.mkdir(parents=True)
    (workspace / ".git").mkdir()
    server, thread = start_provider()
    endpoint = f"http://127.0.0.1:{server.server_port}"
    contract.write_config(root / "config" / "aishe" / "config.toml", endpoint)
    env = environment(root, runtime)
    try:
        result = run(binary, env, workspace, "suggest", "--json", BEFORE)
        if json.loads(result.stdout).get("kind") != "answer":
            raise AssertionError("pre-reboot managed turn did not return an answer")
        shell = run(binary, env, workspace, "-c", f"echo {HISTORY}")
        if HISTORY not in shell.stdout:
            raise AssertionError("pre-reboot shell history command did not run")
        sessions = json.loads(run(binary, env, workspace, "sessions", "--json").stdout)
        if len(sessions.get("managed", [])) != 1:
            raise AssertionError("pre-reboot managed session was not persisted")
    finally:
        stop_backend(binary, env, workspace)
        stop_provider(server, thread)

    expected = {
        "schema_version": 1,
        "endpoint": endpoint,
        "config": digest_tree(root / "config"),
        "data": digest_tree(root / "data"),
    }
    (root / "expected.json").write_text(
        json.dumps(expected, indent=2) + "\n", encoding="utf-8"
    )
    print(f"PREPARED: {root}")
    print("Reboot the disposable node, then run the verify phase.")


def verify(binary, root, runtime):
    expected_path = root / "expected.json"
    if not expected_path.is_file():
        raise SystemExit(f"reboot fixture is not prepared: {root}")
    expected = json.loads(expected_path.read_text(encoding="utf-8"))
    if digest_tree(root / "config") != expected["config"]:
        raise AssertionError("config/credentials bytes changed across reboot")
    if digest_tree(root / "data") != expected["data"]:
        raise AssertionError("history/session/backend bytes changed across reboot")

    endpoint = urllib.parse.urlparse(expected["endpoint"])
    server, thread = start_provider(endpoint.port)
    workspace = root / "workspace"
    env = environment(root, runtime)
    try:
        sessions = json.loads(run(binary, env, workspace, "sessions", "--json").stdout)
        if len(sessions.get("managed", [])) != 1:
            raise AssertionError("managed session mapping disappeared across reboot")
        result = run(binary, env, workspace, "suggest", "--json", AFTER)
        if json.loads(result.stdout).get("kind") != "answer":
            raise AssertionError("post-reboot managed turn did not return an answer")
        with contract.STATE.lock:
            requests = list(contract.STATE.requests)
            authenticated = contract.STATE.authenticated_requests
        if len(requests) != 1 or authenticated != 1:
            raise AssertionError("post-reboot provider request was not exact/authenticated")
        if not contract.contains_text(requests[0], BEFORE):
            raise AssertionError("post-reboot provider context lost the prior managed turn")
        history = run(binary, env, workspace, "-c", "history")
        if HISTORY not in history.stdout:
            raise AssertionError("Aishe history disappeared across reboot")
        sessions = json.loads(run(binary, env, workspace, "sessions", "--json").stdout)
        if len(sessions.get("managed", [])) != 1:
            raise AssertionError("post-reboot turn split the durable session")
    finally:
        stop_backend(binary, env, workspace)
        stop_provider(server, thread)
    print(
        "PASS: config, history, managed session, and conversation context "
        "survived a real host reboot"
    )


def main():
    if len(sys.argv) != 4 or sys.argv[1] not in {"prepare", "verify"}:
        usage()
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    mode = sys.argv[1]
    binary = str(pathlib.Path(sys.argv[2]).resolve())
    root = pathlib.Path(sys.argv[3]).resolve()
    runtime_path = pathlib.Path(runtime).resolve()
    if mode == "prepare":
        prepare(binary, root, runtime_path)
    else:
        verify(binary, root, runtime_path)


if __name__ == "__main__":
    main()
