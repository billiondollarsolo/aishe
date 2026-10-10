#!/usr/bin/env python3
"""Record direct-command startup components without changing the strict SLO.

Every command checks its exact output against an isolated local fixture. This
diagnostic never invokes a model or starts the managed backend. Case order
rotates to keep concurrent runner load from favoring one component.
"""

import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import resource
import shlex
import shutil
import statistics
import subprocess
import tempfile
import time

from direct_shell_benchmark import percentile, write_config
from harness_identity import parse_binary_identity, require_current_binary


RAW_ROW_LIMIT = 710
PAIR_ROUNDS = 40


def measured(argv, env, expected):
    """CPU accounting brackets the wall interval; neither measures pure wait."""
    started_at = datetime.datetime.now(datetime.timezone.utc).isoformat()
    child_before = resource.getrusage(resource.RUSAGE_CHILDREN)
    parent_before = resource.getrusage(resource.RUSAGE_SELF)
    started = time.perf_counter_ns()
    result = subprocess.run(argv, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=15, check=False)
    elapsed = (time.perf_counter_ns() - started) / 1_000_000
    parent_after = resource.getrusage(resource.RUSAGE_SELF)
    child_after = resource.getrusage(resource.RUSAGE_CHILDREN)
    if result.returncode != 0 or result.stdout != expected or result.stderr:
        raise AssertionError(
            "direct command contract failed\n"
            f"argv={argv!r}\nrc={result.returncode}\n"
            f"stdout={result.stdout!r}\nstderr={result.stderr!r}"
        )
    row = {"started_at": started_at, "monotonic_start_ns": started,
           "wall_ms": elapsed}
    for name, before, after in (("child", child_before, child_after),
                                ("parent", parent_before, parent_after)):
        row[f"{name}_user_ms"] = (after.ru_utime - before.ru_utime) * 1000
        row[f"{name}_system_ms"] = (after.ru_stime - before.ru_stime) * 1000
        row[f"{name}_cpu_ms"] = (row[f"{name}_user_ms"] +
                                row[f"{name}_system_ms"])
    row["unaccounted_wall_ms"] = (elapsed - row["child_cpu_ms"] -
                                  row["parent_cpu_ms"])
    return row


def summarize(rows):
    walls = [row["wall_ms"] for row in rows]
    result = {"p50_ms": round(statistics.median(walls), 3),
              "p95_ms": round(percentile(walls, 95), 3)}
    for field in ("child_cpu_ms", "parent_cpu_ms", "unaccounted_wall_ms"):
        values = [row[field] for row in rows]
        result[f"mean_{field}"] = round(statistics.mean(values), 3)
        result[f"p95_{field}"] = round(percentile(values, 95), 3)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--commands", type=int, default=100)
    parser.add_argument("--warmup", type=int, default=10)
    parser.add_argument("--output", type=Path,
                        default=Path("test-results/direct-shell-profile.json"))
    args = parser.parse_args()
    if args.commands < 20 or args.warmup < 0:
        parser.error("--commands must be >= 20 and --warmup must be >= 0")
    binary = require_current_binary(args.binary)
    zsh = shutil.which("zsh")
    if not zsh:
        raise SystemExit("zsh is required")

    with tempfile.TemporaryDirectory(prefix="aishe-direct-shell-profile-") as text:
        root = Path(text).resolve()
        write_config(root)
        env = {name: value for name, value in os.environ.items()
               if not name.startswith("AISHE_")}
        env.update({
            "HOME": str(root),
            "AISHE_CONFIG_DIR": str(root / "config"),
            "AISHE_DATA_DIR": str(root / "data"),
            "AISHE_RUNTIME_DIR": str(root / "runtime"),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "AISHE_SHELL_ID": "directshellprofile0123456789",
            "NO_COLOR": "1", "TERM": "dumb",
        })
        version = subprocess.run([binary, "--version"], env=env,
                                 capture_output=True, check=True, timeout=15)
        assert not version.stderr
        identity = parse_binary_identity(version.stdout.decode())
        # Retain the persistent Executor bootstrap as a comparison with the
        # standalone in-memory bootstrap. Neither diagnostic replaces the
        # unchanged strict end-to-end direct-shell benchmark.
        rc = root / "session.zsh"
        rc.write_text(
            "# aishe session rc (generated)\n"
            "setopt aliases 2>/dev/null\n"
            f"[ -f {shlex.quote(str(root / '.aishrc'))} ] && "
            f"source {shlex.quote(str(root / '.aishrc'))}\n"
            f"[ -f {shlex.quote(str(root / 'config/aishe/aishrc'))} ] && "
            f"source {shlex.quote(str(root / 'config/aishe/aishrc'))}\n"
        )
        rc.chmod(0o600)
        command = "printf 'direct-shell-component\\n'"
        expected = b"direct-shell-component\n"
        rc_env = dict(env, AISHE_CMD=command)
        # A native parent exports its history destination and selected shell
        # experience together, so the child can retain both without a config read.
        active_history = dict(env, AISHE_HISTFILE=str(root / "active-history.ext"),
                              AISHE_ZSH_PROFILE="clean")
        cases = [
            ("aishe_version", [binary, "--version"], env, version.stdout),
            ("raw_zsh", [zsh, "-c", command], env, expected),
            ("raw_zsh_private_rc", [zsh, "-c",
             f'source {shlex.quote(str(rc))} 2>/dev/null; eval "$AISHE_CMD"'],
             rc_env, expected),
            ("aishe_direct_default", [binary, "-c", command], env, expected),
            ("aishe_direct_active_history", [binary, "-c", command],
             active_history, expected),
            ("raw_zsh_inline_rc", [zsh, "-c",
             '{\n' + rc.read_text() + '} 2>/dev/null; eval "$AISHE_CMD"'],
             rc_env, expected),
        ]
        samples = {name: [] for name, *_ in cases}
        rows = {name: [] for name, *_ in cases}
        raw_rows = []
        sequence = 0
        for number in range(args.warmup + args.commands):
            offset = number % len(cases)
            for order, (name, argv, case_env, output) in enumerate(
                    cases[offset:] + cases[:offset]):
                row = measured(argv, case_env, output)
                if number >= args.warmup:
                    row.update(sequence=sequence, phase="rotating", case=name,
                               round=number - args.warmup, order=order)
                    sequence += 1
                    rows[name].append(row)
                    samples[name].append(row["wall_ms"])
                    if len(raw_rows) < RAW_ROW_LIMIT - PAIR_ROUNDS * 2:
                        raw_rows.append(row)

        # A separate diagnostic reverses within-pair order. The strict SLO
        # benchmark and its raw-first alternating series remain unchanged.
        paired = {order: {name: [] for name in ("raw_zsh", "aishe_direct_default")}
                  for order in ("raw_first", "aishe_first")}
        pair_deltas = {order: [] for order in paired}
        for number in range(PAIR_ROUNDS):
            order_name = "raw_first" if number % 2 == 0 else "aishe_first"
            pair = [cases[1], cases[3]]
            if order_name == "aishe_first":
                pair.reverse()
            walls = {}
            for order, (name, argv, case_env, output) in enumerate(pair):
                row = measured(argv, case_env, output)
                row.update(sequence=sequence, phase="balanced_pairs", case=name,
                           round=number, order=order, pair_order=order_name)
                sequence += 1
                paired[order_name][name].append(row)
                walls[name] = row["wall_ms"]
                raw_rows.append(row)
            pair_deltas[order_name].append(walls["aishe_direct_default"] -
                                          walls["raw_zsh"])
        backend = root / "data/aishe/backend"
        assert not backend.exists(), "direct components must never materialize backend state"

    report = {
        "schema_version": 1, "kind": "aishe_direct_shell_profile",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": binary, "binary_identity": identity,
        "platform": platform.system(), "machine": platform.machine(),
        "commands_per_case": args.commands, "warmup": args.warmup,
        "backend_started": False,
        "accounting_note": (
            "Child CPU includes reaped descendants; parent CPU includes Python "
            "subprocess bookkeeping. CPU deltas bracket the wall interval. "
            "Unaccounted wall is wall minus child and parent CPU; it includes "
            "I/O, scheduling, and accounting effects, may be negative, and is "
            "not a measurement of scheduler delay. Warmup rows are omitted."
        ),
        "cases": {name: {**summarize(rows[name]),
                         "samples_ms": [round(value, 3) for value in values]}
                  for name, values in samples.items()},
        "balanced_pairs": {
            "rounds": PAIR_ROUNDS, "slo_enforced": False,
            "orders": {order: {
                "pairs": len(pair_deltas[order]),
                "cases": {name: summarize(values) for name, values in cases.items()},
                "mean_paired_wall_difference_ms": round(statistics.mean(pair_deltas[order]), 3),
                "p95_paired_wall_difference_ms": round(percentile(pair_deltas[order], 95), 3),
            } for order, cases in paired.items()},
        },
        "raw_rows_total": sequence, "raw_rows_recorded": len(raw_rows),
        "raw_row_limit": RAW_ROW_LIMIT,
        "raw_rows": [{key: round(value, 6) if isinstance(value, float) else value
                      for key, value in row.items()} for row in raw_rows],
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    summary = {key: value for key, value in report.items() if key != "raw_rows"}
    summary["cases"] = {name: {key: value for key, value in case.items()
                               if key != "samples_ms"}
                        for name, case in report["cases"].items()}
    print(json.dumps(summary, indent=2))
    # Artifacts can be inaccessible through their signed download URLs. Keep
    # bounded raw evidence in the job log as well as the JSON report.
    for row in report["raw_rows"]:
        print("sample: " + json.dumps(row, separators=(",", ":")))
    print(f"report: {args.output}")
    print("PASS: startup component output and backend isolation; SLO unchanged")


if __name__ == "__main__":
    main()
