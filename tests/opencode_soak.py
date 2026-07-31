#!/usr/bin/env python3
"""Managed OpenCode lifecycle, reconnect, memory, and prompt soak.

The test uses the exact installed runtime and the deterministic local provider
from opencode_runtime_contract.py. It never needs an external credential.

Qualification example:
  AISHE_RUNTIME_DIR=... python3 tests/opencode_soak.py target/release/aishe \
    --turns 100 --cold-cycles 10 --warm-probes 50 --reconnect-every 20

Release soak:
  ... --turns 1000 --cold-cycles 40 --warm-probes 200 --reconnect-every 25
"""

import argparse
import datetime
import http.server
import json
import math
import os
import pathlib
import statistics
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request

from opencode_runtime_contract import (
    CANARY,
    MODEL,
    ProviderHandler,
    STATE,
    write_config,
)


def percentile(values, percent):
    if not values:
        return 0.0
    ordered = sorted(values)
    index = max(0, math.ceil((percent / 100.0) * len(ordered)) - 1)
    return ordered[index]


def run(binary, env, cwd, *args, timeout=60):
    started = time.monotonic()
    result = subprocess.run(
        [binary, *args],
        cwd=cwd,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=timeout,
    )
    elapsed = time.monotonic() - started
    if result.returncode != 0:
        raise AssertionError(
            f"{' '.join(args)} exited {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result, elapsed


def process_rss_kib(pid):
    proc_status = pathlib.Path(f"/proc/{pid}/status")
    if proc_status.is_file():
        for line in proc_status.read_text(encoding="utf-8").splitlines():
            if line.startswith("VmRSS:"):
                return int(line.split()[1])
    result = subprocess.run(
        ["ps", "-o", "rss=", "-p", str(pid)],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    value = result.stdout.strip()
    return int(value) if value.isdigit() else 0


def process_exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def state_paths(data_home):
    instances = data_home / "aishe" / "backend" / "instances"
    return sorted(instances.glob("*/supervisor.json"))


def read_state(data_home):
    paths = state_paths(data_home)
    if len(paths) != 1:
        raise AssertionError(f"expected one managed runtime state, found {paths}")
    path = paths[0]
    return json.loads(path.read_text(encoding="utf-8"))


def control_health_seconds(state):
    request = urllib.request.Request(
        f"{state['control_url'].rstrip('/')}/v1/health",
        headers={"Authorization": f"Bearer {state['control_token']}"},
    )
    started = time.monotonic()
    with urllib.request.urlopen(request, timeout=3) as response:
        payload = json.load(response)
    elapsed = time.monotonic() - started
    if (
        not payload.get("healthy")
        or payload.get("supervisor_pid") != state["supervisor_pid"]
        or payload.get("opencode_pid") != state["opencode_pid"]
    ):
        raise AssertionError(f"authenticated health identity mismatch: {payload!r}")
    return elapsed


def observed_cold_prompt(binary, env, workspace, data_home, label, timeout=45):
    """Run one prompt while observing when authenticated supervisor health lands."""
    if state_paths(data_home):
        raise AssertionError("cold-start sample began with stale supervisor state")
    started = time.monotonic()
    process = subprocess.Popen(
        [binary, "suggest", "--json", label],
        cwd=workspace,
        env=env,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    ready_seconds = None
    state = None
    deadline = started + timeout
    while process.poll() is None and time.monotonic() < deadline:
        paths = state_paths(data_home)
        if len(paths) == 1:
            try:
                candidate = json.loads(paths[0].read_text(encoding="utf-8"))
                control_health_seconds(candidate)
                state = candidate
                ready_seconds = time.monotonic() - started
                break
            except (OSError, ValueError, KeyError, urllib.error.URLError):
                pass
        time.sleep(0.02)
    try:
        stdout, stderr = process.communicate(
            timeout=max(0.1, deadline - time.monotonic())
        )
    except subprocess.TimeoutExpired:
        process.kill()
        stdout, stderr = process.communicate()
        raise AssertionError(
            f"cold prompt timed out\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    total_seconds = time.monotonic() - started
    if process.returncode != 0:
        raise AssertionError(
            f"cold prompt exited {process.returncode}\n"
            f"stdout:\n{stdout}\nstderr:\n{stderr}"
        )
    payload = json.loads(stdout)
    if payload.get("kind") != "answer":
        raise AssertionError(f"cold prompt returned {payload!r}")
    if ready_seconds is None or state is None:
        raise AssertionError("cold prompt completed without observed authenticated health")
    return ready_seconds, total_seconds, state


def stop_and_assert_reaped(binary, env, workspace, state=None, data_home=None):
    subprocess.run(
        [binary, "backend", "stop"],
        cwd=workspace,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=15,
        check=False,
    )
    pids = []
    if state:
        pids = [state["supervisor_pid"], state["opencode_pid"]]
    deadline = time.monotonic() + 8
    while any(process_exists(pid) for pid in pids) and time.monotonic() < deadline:
        time.sleep(0.05)
    survivors = [pid for pid in pids if process_exists(pid)]
    if survivors:
        raise AssertionError(f"managed backend did not reap process(es): {survivors}")
    if data_home is not None:
        while state_paths(data_home) and time.monotonic() < deadline:
            time.sleep(0.05)
        if state_paths(data_home):
            raise AssertionError("managed backend left stale supervisor state")


def write_report(report):
    output_dir = pathlib.Path(__file__).resolve().parent.parent / "test-results"
    output_dir.mkdir(exist_ok=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    json_path = output_dir / f"opencode-soak-{stamp}.json"
    markdown_path = output_dir / f"opencode-soak-{stamp}.md"
    json_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    markdown_path.write_text(
        "\n".join(
            [
                "# Managed OpenCode qualification",
                "",
                f"- Date: {report['date']}",
                f"- Result: **{report['result']}**",
                f"- Runtime: OpenCode {report['runtime_version']}",
                f"- Turns: {report['turns']}",
                f"- Supervisor reconnects: {report['reconnects']}",
                f"- Provider requests: {report['provider_requests']}",
                f"- Lifecycle cycles: {report['lifecycle_cycles']}",
                f"- Lifecycle duration: {report['lifecycle_elapsed_seconds']:.1f}s",
                f"- Bootstrap: {report['bootstrap_seconds']:.3f}s",
                f"- Initial managed start: {report['managed_start_ms']:.1f}ms",
                f"- Cold ready p95: {report['cold_ready_p95_ms']:.1f}ms",
                f"- Cold full-turn p95: {report['cold_turn_p95_ms']:.1f}ms",
                f"- Full live-verify p95: {report['live_verify_p95_ms']:.1f}ms",
                f"- Warm authenticated health p95: {report['warm_health_p95_ms']:.1f}ms",
                f"- Managed turn p95: {report['turn_p95_ms']:.1f}ms",
                f"- Supervisor RSS max: {report['supervisor_rss_max_kib']} KiB",
                f"- OpenCode RSS max: {report['opencode_rss_max_kib']} KiB",
                f"- OpenCode RSS warm-to-final growth: "
                f"{report['opencode_rss_growth_kib']} KiB",
                "",
                "The provider was a loopback deterministic fixture. No external "
                "credential or paid request was used.",
                "",
            ]
        ),
        encoding="utf-8",
    )
    print(f"report: {markdown_path.relative_to(output_dir.parent)}")


def qualify(binary, runtime_dir, args):
    with STATE.lock:
        STATE.requests.clear()
        STATE.authenticated_requests = 0

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProviderHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    endpoint = f"http://127.0.0.1:{server.server_port}"

    with tempfile.TemporaryDirectory(prefix="aishe-opencode-soak-") as root_text:
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
                "AISHE_RUNTIME_DIR": str(runtime_dir),
                "XDG_CONFIG_HOME": str(config_home),
                "XDG_DATA_HOME": str(data_home),
                "CONTRACT_PROVIDER_KEY": CANARY,
                "AISHE_SHELL_ID": "soakshell0123456789abcdef",
                "AISHE_USAGE_FILE": str(root / "usage.tsv"),
                "AISHE_STATUS_FILE": str(root / "status"),
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )

        last_state = None
        try:
            # First launch includes one-time isolated plugin dependency setup.
            _, bootstrap_seconds = run(
                binary, env, workspace, "backend", "verify", "--live", "--json",
                timeout=180,
            )

            live_verify = []
            for _ in range(args.cold_cycles):
                _, elapsed = run(
                    binary, env, workspace, "backend", "verify", "--live", "--json",
                    timeout=30,
                )
                live_verify.append(elapsed)

            # Measure the real lazy-start path separately from the deliberately
            # heavier runtime-hash/tool-policy smoke test above.
            cold_ready = []
            cold_turns = []
            for index in range(args.cold_cycles):
                ready, elapsed, last_state = observed_cold_prompt(
                    binary,
                    env,
                    workspace,
                    data_home,
                    f"managed cold-start sample {index}",
                    timeout=45,
                )
                cold_ready.append(ready)
                cold_turns.append(elapsed)
                stop_and_assert_reaped(
                    binary, env, workspace, last_state, data_home
                )
                last_state = None

            # Start the persistent managed supervisor through a real prompt, then
            # measure its authenticated health path without provider latency.
            _, managed_start = run(
                binary,
                env,
                workspace,
                "suggest",
                "--json",
                "managed soak warmup",
                timeout=45,
            )
            last_state = read_state(data_home)
            runtime_version = last_state["runtime_version"]
            with STATE.lock:
                STATE.requests.clear()
                STATE.authenticated_requests = 0
            warm_health = []
            for _ in range(args.warm_probes):
                warm_health.append(control_health_seconds(last_state))

            turn_times = []
            supervisor_rss = []
            opencode_rss = []
            reconnects = 0
            for index in range(args.turns):
                if index and args.reconnect_every and index % args.reconnect_every == 0:
                    last_state = read_state(data_home)
                    stop_and_assert_reaped(
                        binary, env, workspace, last_state, data_home
                    )
                    last_state = None
                    reconnects += 1

                result, elapsed = run(
                    binary,
                    env,
                    workspace,
                    "suggest",
                    "--json",
                    f"managed soak turn {index}",
                    timeout=45,
                )
                payload = json.loads(result.stdout)
                if payload.get("kind") != "answer":
                    raise AssertionError(f"turn {index} returned {payload!r}")
                turn_times.append(elapsed)
                last_state = read_state(data_home)
                supervisor_rss.append(process_rss_kib(last_state["supervisor_pid"]))
                opencode_rss.append(process_rss_kib(last_state["opencode_pid"]))

            lifecycle_cycles = 0
            lifecycle_started = time.monotonic()
            lifecycle_deadline = lifecycle_started + args.lifecycle_hours * 3600
            while time.monotonic() < lifecycle_deadline:
                time.sleep(
                    min(
                        args.lifecycle_interval,
                        max(0.0, lifecycle_deadline - time.monotonic()),
                    )
                )
                if time.monotonic() >= lifecycle_deadline:
                    break
                last_state = read_state(data_home)
                stop_and_assert_reaped(
                    binary, env, workspace, last_state, data_home
                )
                last_state = None
                time.sleep(
                    min(
                        args.lifecycle_interval,
                        max(0.0, lifecycle_deadline - time.monotonic()),
                    )
                )
                if time.monotonic() >= lifecycle_deadline:
                    break
                result, elapsed = run(
                    binary,
                    env,
                    workspace,
                    "suggest",
                    "--json",
                    f"managed lifecycle turn {lifecycle_cycles}",
                    timeout=45,
                )
                if json.loads(result.stdout).get("kind") != "answer":
                    raise AssertionError("lifecycle restart returned a non-answer")
                turn_times.append(elapsed)
                last_state = read_state(data_home)
                supervisor_rss.append(process_rss_kib(last_state["supervisor_pid"]))
                opencode_rss.append(process_rss_kib(last_state["opencode_pid"]))
                lifecycle_cycles += 1

            sessions, _ = run(binary, env, workspace, "sessions", "--json")
            managed = json.loads(sessions.stdout).get("managed", [])
            if len(managed) != 1:
                raise AssertionError(f"soak split durable context: {managed!r}")
            with STATE.lock:
                provider_requests = len(STATE.requests)
                authenticated = STATE.authenticated_requests
            expected_requests = args.turns + lifecycle_cycles
            if provider_requests != expected_requests or authenticated != expected_requests:
                raise AssertionError(
                    "provider request count mismatch: "
                    f"{provider_requests}/{authenticated} for "
                    f"{expected_requests} turns"
                )

            warm_index = min(len(opencode_rss) - 1, max(0, args.reconnect_every - 1))
            rss_growth = opencode_rss[-1] - opencode_rss[warm_index]
            report = {
                "date": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                "result": "PASS",
                "runtime_version": runtime_version,
                "turns": args.turns,
                "reconnects": reconnects,
                "provider_requests": provider_requests,
                "lifecycle_cycles": lifecycle_cycles,
                "lifecycle_elapsed_seconds": time.monotonic() - lifecycle_started,
                "bootstrap_seconds": bootstrap_seconds,
                "managed_start_ms": managed_start * 1000,
                "cold_ready_samples_ms": [round(value * 1000, 3) for value in cold_ready],
                "cold_ready_p95_ms": percentile(cold_ready, 95) * 1000,
                "cold_turn_p95_ms": percentile(cold_turns, 95) * 1000,
                "live_verify_p95_ms": percentile(live_verify, 95) * 1000,
                "warm_health_p95_ms": percentile(warm_health, 95) * 1000,
                "turn_p50_ms": statistics.median(turn_times) * 1000,
                "turn_p95_ms": percentile(turn_times, 95) * 1000,
                "supervisor_rss_max_kib": max(supervisor_rss),
                "opencode_rss_max_kib": max(opencode_rss),
                "opencode_rss_growth_kib": rss_growth,
            }
            # Operational cold readiness excludes first-time runtime/plugin
            # installation and must satisfy the documented local target.
            if report["cold_ready_p95_ms"] > 2500:
                raise AssertionError(
                    f"cold backend p95 exceeded 2500ms: "
                    f"{report['cold_ready_p95_ms']:.1f}ms"
                )
            write_report(report)
            print(
                f"PASS: {args.turns} managed turns, {reconnects} reconnects; "
                f"cold p95 {report['cold_ready_p95_ms']:.1f}ms; "
                f"turn p95 {report['turn_p95_ms']:.1f}ms"
            )
        finally:
            if last_state is None:
                paths = state_paths(data_home)
                if len(paths) == 1:
                    try:
                        last_state = json.loads(paths[0].read_text(encoding="utf-8"))
                    except (OSError, ValueError):
                        last_state = None
            stop_and_assert_reaped(binary, env, workspace, last_state, data_home)
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--turns", type=int, default=100)
    parser.add_argument("--cold-cycles", type=int, default=10)
    parser.add_argument("--warm-probes", type=int, default=50)
    parser.add_argument("--reconnect-every", type=int, default=20)
    parser.add_argument("--lifecycle-hours", type=float, default=0)
    parser.add_argument("--lifecycle-interval", type=float, default=60)
    args = parser.parse_args()
    for name in ("turns", "cold_cycles", "warm_probes"):
        if getattr(args, name) < 1:
            parser.error(f"--{name.replace('_', '-')} must be positive")
    if args.reconnect_every < 0:
        parser.error("--reconnect-every cannot be negative")
    if args.lifecycle_hours < 0:
        parser.error("--lifecycle-hours cannot be negative")
    if args.lifecycle_interval <= 0:
        parser.error("--lifecycle-interval must be positive")
    runtime = os.environ.get("AISHE_RUNTIME_DIR")
    if not runtime:
        raise SystemExit("AISHE_RUNTIME_DIR must point to the installed pinned runtime")
    qualify(
        str(pathlib.Path(args.binary).resolve()),
        pathlib.Path(runtime).resolve(),
        args,
    )


if __name__ == "__main__":
    main()
