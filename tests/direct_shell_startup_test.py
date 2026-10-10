#!/usr/bin/env python3
"""Qualify the optimized one-shot launcher without changing shell contracts.

In particular, history must be appended after a command completes even when it
replaces its shell or exits by signal. Fixtures use their own HOME/config/data
and prove that user rc aliases work without a writable temporary directory.
"""

import argparse
import datetime
import json
import os
import pathlib
import subprocess
import tempfile

from harness_identity import parse_binary_identity, require_current_binary
from direct_shell_benchmark import write_config


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    binary = require_current_binary(args.binary)
    identity = parse_binary_identity(
        subprocess.check_output([binary, "--version"], text=True)
    )
    checks = []
    with tempfile.TemporaryDirectory(prefix="aishe-one-shot-") as temporary:
        # Darwin's /var temporary paths resolve to /private/var in shell PWD.
        root = pathlib.Path(temporary).resolve()
        home = root / "home with ' quote"
        config = root / "configuration with ' quote"
        cwd = root / "working directory"
        history = root / "data" / "history.ext"
        for directory in (home, config / "aishe", cwd):
            directory.mkdir(parents=True)
        write_config(root)
        (config / "aishe" / "config.toml").write_text(
            (root / "config" / "aishe" / "config.toml").read_text().replace(
                'engine = "opencode"', 'engine = "native"'
            ), encoding="utf-8"
        )
        (home / ".aishrc").write_text(
            "RC_ORDER=home\nalias startup_alias='printf alias-ok'\n"
            "printf 'rc-error-hidden' >&2\n", encoding="utf-8"
        )
        (config / "aishe" / "aishrc").write_text(
            'RC_ORDER="$RC_ORDER/config"\n', encoding="utf-8"
        )
        environment = os.environ.copy()
        environment.update({
            "HOME": str(home), "ZDOTDIR": str(home),
            "AISHE_CONFIG_DIR": str(config),
            "AISHE_DATA_DIR": str(root / "data"),
            "AISHE_RUNTIME_DIR": str(root / "runtime"),
            "AISHE_HISTFILE": str(history),
            "TMPDIR": str(root / "missing-temp-directory"),
            "ONE_SHOT_ENV": "inherited value", "NO_COLOR": "1",
            "TERM": "dumb",
        })
        # Raw Unix environment values need not be UTF-8. A one-shot shell must
        # inherit them without collecting std::env::vars() and panicking.

        def run(name, command, expected_stdout=b"", expected_code=0):
            result = subprocess.run(
                [binary, "-c", command], env=environment, cwd=cwd,
                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                stderr=subprocess.PIPE, timeout=15, check=False,
            )
            assert result.returncode == expected_code, (name, result)
            assert result.stdout == expected_stdout, (name, result.stdout)
            assert not result.stderr, (name, result.stderr)
            entries = history.read_text(encoding="utf-8") if history.exists() else ""
            assert command.replace("\n", " ") in entries, (name, entries)
            checks.append({"name": name, "pass": True})

        run(
            "environment-cwd-rc-order-alias-without-tempfile",
            "printf '%s/%s/%s/' \"$PWD\" \"$ONE_SHOT_ENV\" \"$RC_ORDER\"; eval startup_alias",
            f"{cwd}/inherited value/home/config/alias-ok".encode(),
        )
        environment["ONE_SHOT_RAW"] = os.fsdecode(b"raw-\xff")
        run("non-utf8-environment", "printf '%s' \"$ONE_SHOT_RAW\"", b"raw-\xff")
        environment.pop("ONE_SHOT_RAW")
        history.unlink()
        run(
            "history-written-after-completion",
            'test ! -e "$AISHE_HISTFILE" && printf history-after',
            b"history-after",
        )
        run("explicit-shell-exit-recorded", "printf ''; exit 23", expected_code=23)
        run("exec-replacement-recorded", "printf ''; exec /bin/sh -c 'exit 24'", expected_code=24)
        run("signal-status-recorded", "kill -TERM $$", expected_code=143)
        before = history.read_bytes()
        result = subprocess.run(
            [binary, "-c", "history"], env=environment, cwd=cwd,
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, timeout=15, check=False,
        )
        assert result.returncode == 0 and not result.stderr, result
        assert history.read_bytes() == before, "history command recorded itself"
        checks.append({"name": "history-management-not-recorded", "pass": True})
        assert not (root / "data" / "aishe" / "backend").exists()
        assert not (root / "runtime").exists()
        checks.append({"name": "provider-and-backend-remain-lazy", "pass": True})

    report = {
        "schema_version": 1, "kind": "aishe_direct_shell_contracts",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": binary, "binary_identity": identity, "checks": checks,
        "pass": all(check["pass"] for check in checks),
    }
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
