#!/usr/bin/env python3
"""Produce one versioned PERF-001 report from deterministic local probes.

Stable, host-resistant budgets are enforced for direct-shell overhead and the
pure route/picker workloads. Rendering, RSS, binary size, and backend fixture
measurements are informational because terminal, linker, kernel, and runtime
host differences dominate those values.
"""

from __future__ import annotations

import argparse
import datetime
import fcntl
import json
import os
import pathlib
import platform
import pty
import select
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

from harness_identity import parse_binary_identity, require_current_binary
from lazy_loading_test import write_config


SCHEMA_VERSION = 1
ROOT = pathlib.Path(__file__).resolve().parent.parent


RSS_HELPER = r"""
import json
import resource
import subprocess
import sys

command = json.loads(sys.argv[1])
completed = subprocess.run(
    command,
    stdin=subprocess.DEVNULL,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    check=False,
)
usage = resource.getrusage(resource.RUSAGE_CHILDREN)
rss = float(usage.ru_maxrss)
if sys.platform == "darwin":
    rss /= 1024.0
print(json.dumps({
    "returncode": completed.returncode,
    "rss_kib": round(rss, 3),
    "stdout_bytes": len(completed.stdout),
    "stderr_bytes": len(completed.stderr),
}))
"""


def run_checked(command: list[str], *, env: dict[str, str] | None = None,
                timeout: int = 1800, stdout=None) -> subprocess.CompletedProcess:
    result = subprocess.run(
        command,
        cwd=ROOT,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE if stdout is None else stdout,
        stderr=subprocess.PIPE,
        text=True,
        timeout=timeout,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(
            f"performance command failed ({result.returncode}): {command!r}\n"
            f"stdout:\n{result.stdout or ''}\nstderr:\n{result.stderr or ''}"
        )
    return result


def read_report(path: pathlib.Path, kind: str) -> dict:
    report = json.loads(path.read_text(encoding="utf-8"))
    if report.get("schema_version") != SCHEMA_VERSION or report.get("kind") != kind:
        raise AssertionError(
            f"expected {kind} schema {SCHEMA_VERSION}, received "
            f"{report.get('kind')} schema {report.get('schema_version')}"
        )
    return report


def pure_probe(output: pathlib.Path, samples: int, *, no_highlight: bool,
               target_dir: pathlib.Path | None = None) -> dict:
    command = ["cargo", "run", "--quiet", "--release", "--locked"]
    if no_highlight:
        command.append("--no-default-features")
    if target_dir is not None:
        command.extend(["--target-dir", str(target_dir)])
    command.extend(
        [
            "--example",
            "performance_probe",
            "--",
            "--output",
            str(output),
            "--samples",
            str(samples),
        ]
    )
    run_checked(command, timeout=1800, stdout=subprocess.DEVNULL)
    report = read_report(output, "aishe_pure_performance")
    expected = "no_highlight" if no_highlight else "default"
    if report.get("feature_set") != expected:
        raise AssertionError(f"pure probe feature mismatch: {report.get('feature_set')}")
    return report


def direct_shell_probe(binary: str, output: pathlib.Path, commands: int,
                       warmup: int) -> dict:
    run_checked(
        [
            sys.executable,
            "tests/direct_shell_benchmark.py",
            binary,
            "--commands",
            str(commands),
            "--warmup",
            str(warmup),
            "--output",
            str(output),
        ],
        timeout=max(900, commands * 2),
    )
    return read_report(output, "aishe_direct_shell_performance")


def measure_rss(binary: str, environment: dict[str, str]) -> dict:
    surfaces = {
        "shell": [binary, "-c", "printf 'rss-ok\\n'"],
        "help": [binary, "--help"],
        "route": [binary, "route", "--json", "--", "printf rss-route"],
        "status": [binary, "status", "--json"],
    }
    records = {}
    for name, command in surfaces.items():
        result = run_checked(
            [sys.executable, "-c", RSS_HELPER, json.dumps(command)],
            env=environment,
            timeout=60,
        )
        record = json.loads(result.stdout)
        if record["returncode"] != 0 or record["rss_kib"] <= 0:
            raise AssertionError(f"could not measure RSS for {name}: {record!r}")
        record["classification"] = "informational"
        records[name] = record
    return {
        "classification": "informational",
        "unit": "KiB",
        "method": "isolated resource.getrusage(RUSAGE_CHILDREN).ru_maxrss",
        "surfaces": records,
        "max_rss_kib": max(record["rss_kib"] for record in records.values()),
    }


def percentile(values: list[float], rank: float) -> float:
    ordered = sorted(values)
    position = (len(ordered) - 1) * rank / 100.0
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    return ordered[lower] * (1.0 - position % 1.0) + ordered[upper] * (position % 1.0)


def initial_pty_prompt_probe(binary: str, root: pathlib.Path, samples: int = 5) -> dict:
    home = root / "pty-home"
    config = root / "pty-config" / "aishe"
    data = root / "pty-data"
    runtime = root / "pty-runtime"
    for directory in (home, config, data, runtime):
        directory.mkdir(parents=True, exist_ok=True)
    (home / ".zshrc").write_text("PROMPT='AISHE_PERF_PROMPT> '\n", encoding="utf-8")
    (config / "config.toml").write_text(
        "[aishe]\nmode = \"suggest\"\nprovider = \"anthropic\"\n"
        "pty_prompt = false\n\n[backend]\nengine = \"native\"\n",
        encoding="utf-8",
    )
    environment = os.environ.copy()
    environment.update(
        {
            "HOME": str(home),
            "ZDOTDIR": str(home),
            "AISHE_CONFIG_DIR": str(root / "pty-config"),
            "AISHE_DATA_DIR": str(data),
            "AISHE_RUNTIME_DIR": str(runtime),
            "XDG_CONFIG_HOME": str(root / "pty-config"),
            "XDG_DATA_HOME": str(data),
            "ZSH_DISABLE_COMPFIX": "true",
            "TERM": "xterm-256color",
            "NO_COLOR": "1",
        }
    )
    values = []
    for _ in range(samples):
        master, slave = pty.openpty()
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        started = time.monotonic_ns()
        process = subprocess.Popen(
            [binary, "zsh"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=environment,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        transcript = b""
        try:
            deadline = time.monotonic() + 15.0
            while b"AISHE_PERF_PROMPT> " not in transcript:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AssertionError(
                        f"initial PTY prompt did not appear; transcript={transcript[-1000:]!r}"
                    )
                ready, _, _ = select.select([master], [], [], min(remaining, 0.2))
                if ready:
                    transcript += os.read(master, 65536)
                elif process.poll() is not None:
                    raise AssertionError(
                        f"PTY exited before prompt ({process.returncode}); transcript={transcript!r}"
                    )
            values.append((time.monotonic_ns() - started) / 1_000_000.0)
        finally:
            try:
                os.killpg(os.getpgid(process.pid), signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)
            os.close(master)
    return {
        "classification": "informational",
        "samples": samples,
        "unit": "ms",
        "p50_ms": percentile(values, 50.0),
        "p95_ms": percentile(values, 95.0),
        "min_ms": min(values),
        "max_ms": max(values),
        "ready_marker": "isolated user zsh prompt",
    }


def backend_evidence(path: pathlib.Path | None) -> dict:
    if path is None:
        return {
            "classification": "informational",
            "status": "not_run",
            "reason": "requires installed pinned runtime; run tests/opencode_soak.py and pass --backend-report",
            "fixture_command": (
                "AISHE_RUNTIME_DIR=... python3 tests/opencode_soak.py "
                "target/release/aishe --turns 20 --cold-cycles 5 --warm-probes 20"
            ),
        }
    report = read_report(path.resolve(), "aishe_backend_performance")
    fields = (
        "runtime_version",
        "managed_start_ms",
        "cold_ready_p95_ms",
        "cold_turn_p95_ms",
        "warm_health_p95_ms",
        "supervisor_rss_max_kib",
        "opencode_rss_max_kib",
        "opencode_rss_growth_kib",
    )
    return {
        "classification": "informational",
        "status": "measured",
        "source": str(path.resolve()),
        "metrics": {field: report[field] for field in fields},
        "source_thresholds": report.get("thresholds", {}),
    }


def build_no_highlight(target_dir: pathlib.Path) -> pathlib.Path:
    run_checked(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "--no-default-features",
            "--target-dir",
            str(target_dir),
            "--bin",
            "aishe",
        ],
        timeout=1800,
    )
    suffix = ".exe" if os.name == "nt" else ""
    binary = target_dir / "release" / f"aishe{suffix}"
    if not binary.is_file():
        raise AssertionError(f"no-highlight binary was not produced: {binary}")
    return binary


def parse_arguments(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--output", type=pathlib.Path)
    parser.add_argument("--samples", type=int, default=60)
    parser.add_argument("--commands", type=int, default=100)
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--backend-report", type=pathlib.Path)
    return parser.parse_args(argv)


def main() -> None:
    args = parse_arguments()
    if args.samples < 20 or args.commands < 20 or args.warmup < 0:
        raise SystemExit("--samples/--commands must be >= 20 and --warmup must be >= 0")
    binary = require_current_binary(args.binary)
    version = run_checked([binary, "--version"], timeout=30)
    identity = parse_binary_identity(version.stdout)

    with tempfile.TemporaryDirectory(prefix="aishe-performance-") as text:
        temporary = pathlib.Path(text)
        direct = direct_shell_probe(
            binary, temporary / "direct-shell.json", args.commands, args.warmup
        )
        default_probe = pure_probe(
            temporary / "pure-default.json", args.samples, no_highlight=False
        )

        no_highlight_target = ROOT / "target" / "performance" / "no-highlight"
        no_highlight_binary = build_no_highlight(no_highlight_target)
        no_highlight_probe = pure_probe(
            temporary / "pure-no-highlight.json",
            args.samples,
            no_highlight=True,
            target_dir=no_highlight_target,
        )

        rss_root = temporary / "rss"
        write_config(rss_root, 9)
        rss_env = os.environ.copy()
        rss_env.update(
            {
                "HOME": str(rss_root),
                "AISHE_CONFIG_DIR": str(rss_root / "config"),
                "AISHE_DATA_DIR": str(rss_root / "data"),
                "AISHE_RUNTIME_DIR": str(rss_root / "runtime"),
                "XDG_CONFIG_HOME": str(rss_root / "config"),
                "XDG_DATA_HOME": str(rss_root / "data"),
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )
        rss_env.pop("AISHE_LAZY_PROVIDER_KEY", None)
        rss = measure_rss(binary, rss_env)
        initial_prompt = initial_pty_prompt_probe(binary, temporary)

    default_size = pathlib.Path(binary).stat().st_size
    no_highlight_size = no_highlight_binary.stat().st_size
    thresholds_pass = bool(
        direct["slo_pass"]
        and default_probe["thresholds_pass"]
        and no_highlight_probe["thresholds_pass"]
    )
    report = {
        "schema_version": SCHEMA_VERSION,
        "kind": "aishe_performance",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "profile_revision": "2026-08-01.1",
        "host": {
            "platform": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "binary": {
            "path": binary,
            "identity": identity,
        },
        "threshold_policy": {
            "enforced": [
                "direct shell p95 overhead versus raw zsh",
                "pure route decision p95",
                "1,000-row picker ranking p95",
                "picker pure frame construction p95",
            ],
            "informational": [
                "long-answer render",
                "initial interactive PTY prompt",
                "resident set size",
                "default/no-highlight binary size",
                "backend fixture timing and RSS",
            ],
        },
        "direct_shell": direct,
        "initial_pty_prompt": initial_prompt,
        "pure_default": default_probe,
        "pure_no_highlight": no_highlight_probe,
        "resident_set": rss,
        "binary_size": {
            "classification": "informational",
            "unit": "bytes",
            "default": default_size,
            "no_highlight": no_highlight_size,
            "highlight_delta": default_size - no_highlight_size,
        },
        "backend": backend_evidence(args.backend_report),
        "thresholds_pass": thresholds_pass,
        "result": "pass" if thresholds_pass else "fail",
    }
    if args.output:
        output = args.output.resolve()
    else:
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        output = ROOT / "test-results" / f"performance-{stamp}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    print(f"report: {output}")
    if not thresholds_pass:
        raise SystemExit("FAIL: one or more stable performance thresholds were exceeded")
    print("PASS: performance thresholds and informational evidence recorded")


if __name__ == "__main__":
    main()
