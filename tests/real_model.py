#!/usr/bin/env python3
"""Opt-in real-model answer-vs-command classification test.

Every input goes through the stable `aishe suggest --json` scripting contract.
The harness checks JSON shape, command syntax, risk/exit agreement, and labelled
answer-vs-command quality without trying to infer type from rendered prose.

Environment:
  AISHE_REALTEST_KEY        API key value (required; skips when absent)
  AISHE_REALTEST_BASE_URL   default https://api.groq.com/openai
  AISHE_REALTEST_MODEL      default openai/gpt-oss-120b
  AISHE_REALTEST_TIMEOUT    outer process deadline in seconds (default 300)

Usage: real_model.py [path-to-aishe]
"""

import datetime
import os
import shutil
import subprocess
import sys
import tempfile
import time

from live_contract import validate_suggest_result
from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT_DIR = os.path.join(REPO_ROOT, "test-results")
KEY = os.environ.get("AISHE_REALTEST_KEY", "")
BASE_URL = os.environ.get("AISHE_REALTEST_BASE_URL", "https://api.groq.com/openai")
MODEL = os.environ.get("AISHE_REALTEST_MODEL", "openai/gpt-oss-120b")
PROCESS_TIMEOUT = int(os.environ.get("AISHE_REALTEST_TIMEOUT", "300"))

CORPUS = [
    ("what is the capital of France", "answer"),
    ("explain how DNS resolution works", "answer"),
    ("why is the sky blue", "answer"),
    ("who was the first person to walk on the moon", "answer"),
    ("what is the difference between TCP and UDP", "answer"),
    ("when was the Linux kernel first released", "answer"),
    ("is the moon bigger than mars", "answer"),
    ("what does the chmod command do", "answer"),
    ("how many bytes are in a kilobyte", "answer"),
    ("define idempotent", "answer"),
    ("list the files in the current directory", "command"),
    ("show me disk usage of this folder sorted by size", "command"),
    ("find all files larger than 100 megabytes under /var", "command"),
    ("count how many lines are in all python files here", "command"),
    ("show the running processes using the most memory", "command"),
    ("create a directory called backups", "command"),
    ("show the last 20 lines of /etc/hosts", "command"),
    ("what is my current working directory", "command"),
    ("print the current date in ISO format", "command"),
    ("search for the word error in all log files here", "command"),
]


def make_home():
    home = tempfile.mkdtemp(prefix="aishe-real-")
    config_dir = os.path.join(home, ".config", "aishe")
    os.makedirs(config_dir, exist_ok=True)
    with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
        file.write(
            "version = 2\n"
            '[aishe]\nmode = "auto"\nprovider = "openai"\ncache = false\n\n'
            "[providers.openai]\n"
            'base_url = "%s"\n'
            'api_key_env = "AISHE_REALTEST_KEY"\n'
            'model = "%s"\n'
            'transport = "auto"\n'
            "auth_required = true\n" % (BASE_URL, MODEL)
        )
    return home


def environment(home):
    env = dict(os.environ)
    env.update(
        {
            "HOME": home,
            "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
            "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "AISHE_REALTEST_KEY": KEY,
        }
    )
    return env


def run_one(home, prompt):
    env = environment(home)
    for attempt in range(2):
        try:
            result = subprocess.run(
                [BINARY, "suggest", "--json", prompt],
                env=env,
                capture_output=True,
                text=True,
                # Aishe can make four provider attempts with a 60-second
                # per-attempt timeout. Stay outside that retry envelope.
                timeout=PROCESS_TIMEOUT,
            )
        except subprocess.TimeoutExpired:
            if attempt == 0:
                time.sleep(2)
                continue
            return "", "TIMEOUT", -1
        combined = (result.stdout + "\n" + result.stderr).lower()
        transient = result.returncode == 1 and any(
            marker in combined
            for marker in (
                "status 429",
                "status 500",
                "status 502",
                "status 503",
                "status 504",
                "rate limit",
                "timed out",
                "temporarily unavailable",
            )
        )
        if transient and attempt == 0:
            time.sleep(2)
            continue
        return result.stdout.strip(), result.stderr.strip(), result.returncode
    return "", "RETRY_FAILED", -1


def main():
    if not KEY:
        print("SKIP: AISHE_REALTEST_KEY not set (real-model test is opt-in)", file=sys.stderr)
        return 0
    if not os.path.exists(BINARY):
        print("FAIL: binary not found: %s" % BINARY, file=sys.stderr)
        return 1
    if PROCESS_TIMEOUT < 60:
        print(
            "FAIL: AISHE_REALTEST_TIMEOUT must be at least 60 seconds",
            file=sys.stderr,
        )
        return 1

    home = make_home()
    rows = []
    contract_breaches = 0
    classified = 0
    started = time.monotonic()
    try:
        for prompt, expected in CORPUS:
            out, err, returncode = run_one(home, prompt)
            payload, problems = validate_suggest_result(out, err, returncode)
            got = payload.get("kind", "invalid")
            quality_ok = got == expected
            contract_ok = not problems
            classified += int(quality_ok and contract_ok)
            contract_breaches += int(not contract_ok)
            shown = payload.get("command") or payload.get("explanation") or err
            shown = str(shown).replace("\n", " ")[:100]
            rows.append(
                (prompt, expected, got, payload.get("risk", "invalid"), returncode,
                 shown, problems, quality_ok and contract_ok)
            )
            print(
                "  %s [%s->%s %s/%s] %s"
                % (
                    "ok  " if quality_ok and contract_ok else "FAIL",
                    expected,
                    got,
                    payload.get("risk", "invalid"),
                    returncode,
                    prompt,
                )
            )
            time.sleep(0.4)
    finally:
        shutil.rmtree(home, ignore_errors=True)

    duration = time.monotonic() - started
    total = len(CORPUS)
    threshold = int(total * 0.8)
    passed = contract_breaches == 0 and classified >= threshold
    os.makedirs(REPORT_DIR, exist_ok=True)
    timestamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    report = os.path.join(REPORT_DIR, "real-model-%s.md" % timestamp)
    with open(report, "w", encoding="utf-8") as file:
        file.write("# Aishe real-model classification report\n\n")
        file.write("- Model: `%s` via `%s`\n" % (MODEL, BASE_URL))
        file.write("- Contract breaches: %d\n" % contract_breaches)
        file.write("- Classification: %d/%d (minimum %d)\n" % (classified, total, threshold))
        file.write("- Duration: %.1fs\n\n" % duration)
        file.write("| input | expected | got | risk / rc | output | result |\n")
        file.write("|---|---|---|---|---|---|\n")
        for prompt, expected, got, risk, rc, shown, problems, ok in rows:
            detail = shown if not problems else "%s [%s]" % (shown, ", ".join(problems))
            file.write(
                "| `%s` | %s | %s | %s / %s | `%s` | %s |\n"
                % (
                    prompt.replace("|", "\\|"),
                    expected,
                    got,
                    risk,
                    rc,
                    detail.replace("|", "\\|"),
                    "PASS" if ok else "FAIL",
                )
            )
    print(
        "real-model: %d/%d classified, %d contract breaches (%.0fs) -> %s"
        % (classified, total, contract_breaches, duration, report)
    )
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
