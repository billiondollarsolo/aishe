#!/usr/bin/env python3
"""Validation helpers shared by opt-in real-model test harnesses."""

import json
import subprocess


def shell_syntax_ok(command):
    if not command:
        return False
    try:
        return (
            subprocess.run(
                ["zsh", "-nc", command],
                capture_output=True,
                timeout=10,
            ).returncode
            == 0
        )
    except (OSError, subprocess.SubprocessError):
        return False


def validate_suggest_result(stdout, stderr, returncode, syntax_check=shell_syntax_ok):
    """Validate `aishe suggest --json` and return `(payload, problems)`.

    Contract:
      * answer: empty command, risk n/a, exit 0;
      * safe command: non-empty valid shell, risk safe, exit 0;
      * held command: non-empty valid shell, dangerous/unknown, exit 20.
    """

    combined = (stdout + "\n" + stderr).lower()
    problems = []
    if returncode == 101 or "panicked" in combined:
        problems.append("panic")
    if returncode < 0:
        problems.append("crash/timeout")
    if "parse error" in combined or "(eval)" in combined:
        problems.append("parse/eval leak")

    try:
        payload = json.loads(stdout)
    except (TypeError, json.JSONDecodeError):
        payload = {}
        problems.append("invalid JSON contract")

    if not isinstance(payload, dict):
        payload = {}
        problems.append("JSON response is not an object")

    response_kind = payload.get("kind")
    if payload.get("schema_version") != 1:
        problems.append("unsupported suggest schema_version")
    command = payload.get("command")
    explanation = payload.get("explanation")
    risk = payload.get("risk")
    reason = payload.get("reason")
    if not isinstance(command, str):
        problems.append("command is not a string")
        command = ""
    if not isinstance(explanation, str):
        problems.append("explanation is not a string")
    if not isinstance(reason, str):
        problems.append("reason is not a string")

    if response_kind == "answer":
        if command:
            problems.append("answer unexpectedly contains command")
        if not isinstance(explanation, str) or not explanation.strip():
            problems.append("answer explanation is empty")
        if risk != "n/a":
            problems.append("answer risk is not n/a")
        if returncode != 0:
            problems.append("answer exit code is not 0")
    elif response_kind == "command":
        if not command:
            problems.append("command response is empty")
        elif not syntax_check(command):
            problems.append("invalid command syntax")
        if risk not in ("safe", "dangerous", "unknown"):
            problems.append("invalid command risk")
        else:
            expected = 0 if risk == "safe" else 20
            if returncode != expected:
                problems.append("risk/exit contract mismatch")
    else:
        problems.append("invalid response kind")

    return payload, problems
