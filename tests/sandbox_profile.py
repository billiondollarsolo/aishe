#!/usr/bin/env python3
"""Read-only Linux runner evidence for strict bubblewrap namespace failures."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess


def command(argv):
    try:
        result = subprocess.run(argv, capture_output=True, text=True, timeout=15)
        return {"argv": argv, "exit_code": result.returncode,
                "stdout": result.stdout[-4000:], "stderr": result.stderr[-4000:]}
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"argv": argv, "error": str(error)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path,
                        default=Path("test-results/sandbox-profile.json"))
    parser.add_argument("--require-namespaces", action="store_true")
    args = parser.parse_args()
    binary = shutil.which("bwrap")
    policy = Path("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
    report = {"schema_version": 1, "kind": "aishe_sandbox_runner_profile",
              "uid": os.getuid(), "bwrap": binary,
              "apparmor_restrict_unprivileged_userns":
                  policy.read_text().strip() if policy.exists() else None,
              "host_user_namespace": os.readlink("/proc/self/ns/user"),
              "host_network_namespace": os.readlink("/proc/self/ns/net")}
    try:
        report["apparmor_profile"] = Path("/proc/self/attr/current").read_text().strip()
    except OSError as error:
        report["apparmor_profile_error"] = str(error)
    report["host_capabilities"] = [line for line in Path("/proc/self/status").read_text().splitlines()
                                   if line.startswith(("Cap", "NoNewPrivs:", "Seccomp:"))]
    if binary:
        metadata = os.stat(binary)
        report["bwrap_mode"] = oct(metadata.st_mode & 0o7777)
        report["bwrap_owner"] = {"uid": metadata.st_uid, "gid": metadata.st_gid}
        report["bwrap_version"] = command([binary, "--version"])
        getcap = shutil.which("getcap")
        report["bwrap_capabilities"] = command([getcap, binary]) if getcap else None
        report["probes"] = {
            "user_namespace": command([binary, "--unshare-user", "--ro-bind", "/", "/",
                                        "--die-with-parent", "--", "/bin/true"]),
            "user_and_network_namespace": command(
                [binary, "--unshare-user", "--unshare-net", "--ro-bind", "/", "/",
                 "--die-with-parent", "--", "/bin/true"]),
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    print(f"diagnostic report: {args.output}; functional isolation gate remains separate")
    if args.require_namespaces and (
        not binary or any(probe.get("exit_code") != 0 for probe in report["probes"].values())
    ):
        raise SystemExit("Required strict user/network namespace probes did not pass")


if __name__ == "__main__":
    main()
