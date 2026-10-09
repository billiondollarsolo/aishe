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
import shlex
import shutil
import statistics
import subprocess
import tempfile

from direct_shell_benchmark import percentile, timed, write_config
from harness_identity import parse_binary_identity, require_current_binary


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
        # Mirror init_session_rc and Executor::configure_shell_command exactly,
        # including the shell-specific alias commands and absent user rc files.
        rc = root / "session.zsh"
        rc.write_text(
            "# aishe session rc (generated)\n"
            "shopt -s expand_aliases 2>/dev/null\n"
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
        active_history = dict(env, AISHE_HISTFILE=str(root / "active-history.ext"))
        cases = [
            ("aishe_version", [binary, "--version"], env, version.stdout),
            ("raw_zsh", [zsh, "-c", command], env, expected),
            ("raw_zsh_private_rc", [zsh, "-c",
             f'source {shlex.quote(str(rc))} 2>/dev/null; eval "$AISHE_CMD"'],
             rc_env, expected),
            ("aishe_direct_default", [binary, "-c", command], env, expected),
            ("aishe_direct_active_history", [binary, "-c", command],
             active_history, expected),
        ]
        samples = {name: [] for name, *_ in cases}
        for number in range(args.warmup + args.commands):
            offset = number % len(cases)
            for name, argv, case_env, output in cases[offset:] + cases[:offset]:
                elapsed = timed(argv, case_env, output)
                if number >= args.warmup:
                    samples[name].append(elapsed)
        backend = root / "data/aishe/backend"
        assert not backend.exists(), "direct components must never materialize backend state"

    report = {
        "schema_version": 1, "kind": "aishe_direct_shell_profile",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": binary, "binary_identity": identity,
        "platform": platform.system(), "machine": platform.machine(),
        "commands_per_case": args.commands, "warmup": args.warmup,
        "backend_started": False,
        "cases": {name: {"p50_ms": round(statistics.median(values), 3),
                         "p95_ms": round(percentile(values, 95), 3),
                         "samples_ms": [round(value, 3) for value in values]}
                  for name, values in samples.items()},
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    summary = dict(report)
    summary["cases"] = {name: {key: value for key, value in case.items()
                               if key != "samples_ms"}
                        for name, case in report["cases"].items()}
    print(json.dumps(summary, indent=2))
    print(f"report: {args.output}")
    print("PASS: startup component output and backend isolation; SLO unchanged")


if __name__ == "__main__":
    main()
