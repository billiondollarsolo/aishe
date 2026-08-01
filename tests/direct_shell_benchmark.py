#!/usr/bin/env python3
"""Direct-shell performance and backend-isolation qualification.

Runs the same bounded command through raw zsh and `aishe -c`, verifies exact
output for every sample, and proves direct commands never materialize or start
the managed backend. The full release gate uses 1,000 commands; CI may use a
smaller deterministic smoke count.
"""

import argparse
import datetime
import json
import os
import pathlib
import shutil
import statistics
import subprocess
import tempfile
import time

from harness_identity import require_current_binary


def percentile(values, rank):
    ordered = sorted(values)
    if not ordered:
        return 0.0
    position = (len(ordered) - 1) * rank / 100
    lower = int(position)
    upper = min(lower + 1, len(ordered) - 1)
    fraction = position - lower
    return ordered[lower] * (1 - fraction) + ordered[upper] * fraction


def timed(argv, env, expected):
    started = time.perf_counter_ns()
    result = subprocess.run(
        argv,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=15,
        check=False,
    )
    elapsed_ms = (time.perf_counter_ns() - started) / 1_000_000
    if result.returncode != 0 or result.stdout != expected or result.stderr:
        raise AssertionError(
            "direct command contract failed\n"
            f"argv={argv!r}\nrc={result.returncode}\n"
            f"stdout={result.stdout!r}\nstderr={result.stderr!r}"
        )
    return elapsed_ms


def write_config(root):
    config = root / "config" / "aishe" / "config.toml"
    config.parent.mkdir(parents=True)
    config.write_text(
        """version = 7

[aishe]
mode = "suggest"
provider = "openai"
connection = "direct-shell"
connection_fallback = "direct-shell"

[backend]
engine = "opencode"
fallback = "none"

[providers.openai]
base_url = "http://127.0.0.1:9"
credential = "direct-shell"
api_key_env = "DIRECT_SHELL_UNUSED_KEY"
model = "direct-shell-model"
transport = "chat"
auth_required = false

[connections.direct-shell]
provider = "openai"
label = "Direct shell benchmark"
base_url = "http://127.0.0.1:9"
credential = "direct-shell"
api_key_env = "DIRECT_SHELL_UNUSED_KEY"
model = "direct-shell-model"
transport = "chat"
auth_required = false

[connections.direct-shell.auth]
type = "none"
""",
        encoding="utf-8",
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--commands", type=int, default=1000)
    parser.add_argument("--warmup", type=int, default=20)
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        help="write the versioned JSON report to this exact path",
    )
    parser.add_argument(
        "--no-enforce-slo",
        action="store_true",
        help="record timing without failing the source-of-truth p95 regression SLO",
    )
    args = parser.parse_args()
    if args.commands < 20 or args.warmup < 0:
        raise SystemExit("--commands must be >= 20 and --warmup must be >= 0")

    binary = pathlib.Path(require_current_binary(args.binary))
    zsh = shutil.which("zsh")
    if not zsh:
        raise SystemExit("zsh is required")

    with tempfile.TemporaryDirectory(prefix="aishe-direct-shell-bench-") as text:
        root = pathlib.Path(text)
        write_config(root)
        data = root / "data"
        runtime = root / "runtime"
        env = os.environ.copy()
        env.update(
            {
                "HOME": str(root),
                "AISHE_CONFIG_DIR": str(root / "config"),
                "AISHE_DATA_DIR": str(data),
                "AISHE_RUNTIME_DIR": str(runtime),
                "XDG_CONFIG_HOME": str(root / "config"),
                "XDG_DATA_HOME": str(data),
                "AISHE_SHELL_ID": "directshellbenchmark0123456789",
                "NO_COLOR": "1",
                "TERM": "dumb",
            }
        )
        env.pop("DIRECT_SHELL_UNUSED_KEY", None)
        subprocess.run(
            [str(binary), "backend", "stop"],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        )

        for number in range(args.warmup):
            payload = f"warm-{number:04d}\n".encode()
            command = f"printf 'warm-{number:04d}\\n'"
            timed([zsh, "-c", command], env, payload)
            timed([str(binary), "-c", command], env, payload)

        raw_samples = []
        aishe_samples = []
        for number in range(args.commands):
            payload = f"aishe-direct-{number:06d}\n".encode()
            command = f"printf 'aishe-direct-{number:06d}\\n'"
            raw_samples.append(timed([zsh, "-c", command], env, payload))
            aishe_samples.append(timed([str(binary), "-c", command], env, payload))

        backend = data / "aishe" / "backend"
        if backend.exists():
            paths = [str(path.relative_to(root)) for path in backend.rglob("*")]
            raise AssertionError(
                "direct shell commands materialized managed backend state: "
                f"{paths[:20]}"
            )

    raw_p95 = percentile(raw_samples, 95)
    aishe_p95 = percentile(aishe_samples, 95)
    regression = max(0.0, aishe_p95 - raw_p95)
    allowed = max(10.0, raw_p95 * 0.10)
    report = {
        "schema_version": 1,
        "kind": "aishe_direct_shell_performance",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": str(binary),
        "commands": args.commands,
        "warmup": args.warmup,
        "raw_zsh": {
            "p50_ms": round(statistics.median(raw_samples), 3),
            "p95_ms": round(raw_p95, 3),
        },
        "aishe": {
            "p50_ms": round(statistics.median(aishe_samples), 3),
            "p95_ms": round(aishe_p95, 3),
        },
        "p95_regression_ms": round(regression, 3),
        "allowed_regression_ms": round(allowed, 3),
        "backend_started": False,
        "slo_pass": regression <= allowed,
    }
    if args.output:
        output = args.output.resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
    else:
        results = pathlib.Path("test-results")
        results.mkdir(exist_ok=True)
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        output = results / f"direct-shell-benchmark-{stamp}.json"
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    print(f"report: {output}")
    if not args.no_enforce_slo and not report["slo_pass"]:
        raise SystemExit(
            "FAIL: direct-shell p95 regression exceeded "
            f"{allowed:.3f} ms ({regression:.3f} ms)"
        )
    print("PASS: direct shell output, isolation, and startup p95")


if __name__ == "__main__":
    main()
