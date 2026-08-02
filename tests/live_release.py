#!/usr/bin/env python3
"""Run the paid, isolated GPT-5.6 release-candidate matrix.

The API key is accepted only through ``AISHE_REALTEST_KEY``. The harness writes
it nowhere, never includes it in output, and removes all isolated config/state
afterward. It combines direct capability and behavior checks with the labelled
real-model corpus and robustness fuzz.

Usage: live_release.py [path-to-aishe] [fuzz-scale]
"""

import datetime
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

from live_contract import create_workspace_acceptance, validate_suggest_result
from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)
FUZZ_SCALE = int(sys.argv[2]) if len(sys.argv) > 2 else 10
TEST_DIR = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(TEST_DIR)
REPORT_DIR = os.path.join(REPO_ROOT, "test-results")
KEY = os.environ.get("AISHE_REALTEST_KEY", "")
BASE_URL = os.environ.get("AISHE_REALTEST_BASE_URL", "https://api.openai.com")
MODEL = os.environ.get("AISHE_REALTEST_MODEL", "gpt-5.6-luna")


def run(command, env, cwd=None, timeout=300):
    result = subprocess.run(
        command,
        env=env,
        cwd=cwd,
        capture_output=True,
        text=True,
        timeout=timeout,
    )
    return result


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def write_config(root):
    config_root = os.path.join(root, "config")
    data_root = os.path.join(root, "data")
    config_dir = os.path.join(config_root, "aishe")
    os.makedirs(config_dir)
    os.makedirs(data_root)
    with open(
        os.path.join(config_dir, "config.toml"), "w", encoding="utf-8"
    ) as file:
        file.write(
            "version = 2\n"
            "[aishe]\n"
            'mode = "yolo"\n'
            'provider = "openai"\n'
            "cache = false\n"
            "stream = true\n"
            'structured = "native"\n'
            'reasoning_effort = "medium"\n'
            'yolo_confirm = "never"\n'
            "yolo_plan = false\n"
            "yolo_sandbox = false\n"
            "yolo_dry_run = false\n"
            "file_tools = false\n"
            "web_tool = false\n"
            "max_yolo_iterations = 4\n\n"
            "[providers.openai]\n"
            'base_url = "%s"\n'
            'api_key_env = "AISHE_REALTEST_KEY"\n'
            'model = "%s"\n'
            'transport = "responses"\n'
            "auth_required = true\n" % (BASE_URL, MODEL)
        )
    return config_root, data_root


def isolated_env(root, config_root, data_root, shell_id, acceptance_path):
    env = dict(os.environ)
    env.update(
        {
            "HOME": root,
            "AISHE_CONFIG_DIR": config_root,
            "AISHE_DATA_DIR": data_root,
            "XDG_CONFIG_HOME": config_root,
            "XDG_DATA_HOME": data_root,
            "AISHE_REALTEST_KEY": KEY,
            "OPENAI_API_KEY": KEY,
            "AISHE_SHELL_ID": shell_id,
            "AISHE_ACCEPTANCE_FILE": acceptance_path,
        }
    )
    return env


def check_capabilities(command, env, label):
    result = run(command, env, timeout=600)
    require(result.returncode == 0, "%s failed: %s" % (label, result.stderr))
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise AssertionError("%s returned invalid JSON: %s" % (label, error)) from error
    report = payload.get("capability_report") if label == "doctor --live" else payload
    require(isinstance(report, dict), "%s omitted capability report" % label)
    for field in (
        "credential",
        "reachability",
        "model_available",
        "text",
        "structured",
        "tools",
        "streaming",
    ):
        check = report.get(field, {})
        require(
            check.get("state") == "pass",
            "%s %s was not pass: %r" % (label, field, check),
        )
    require(report.get("model") == MODEL, "%s tested the wrong model" % label)
    require(
        report.get("transport") == "responses",
        "%s did not use Responses" % label,
    )
    return result.stdout + result.stderr


def check_suggest(env, prompt, expected_kind, contains=None):
    result = run([BINARY, "suggest", "--json", prompt], env)
    payload, problems = validate_suggest_result(
        result.stdout.strip(), result.stderr.strip(), result.returncode
    )
    require(not problems, "suggest contract breach: %s" % ", ".join(problems))
    require(
        payload.get("kind") == expected_kind,
        "suggest classified %r as %r, expected %r"
        % (prompt, payload.get("kind"), expected_kind),
    )
    if contains:
        explanation = payload.get("explanation", "").lower()
        require(contains.lower() in explanation, "answer omitted %r" % contains)
    return result.stdout + result.stderr


def check_yolo_tool(env, work):
    marker = os.path.join(work, "live-tool-marker.txt")
    prompt = (
        "Use the run_command tool exactly once. Its command must be exactly "
        "`printf LIVE_TOOL_OK > live-tool-marker.txt`. After the tool result, "
        "reply exactly `done`. Do not use another tool."
    )
    result = run([BINARY, "--yolo-line", prompt], env, cwd=work, timeout=600)
    combined = result.stdout + result.stderr
    require(result.returncode == 0, "yolo function-tool request failed: " + combined)
    require(os.path.isfile(marker), "yolo did not create its isolated marker")
    with open(marker, encoding="utf-8") as file:
        require(file.read() == "LIVE_TOOL_OK", "yolo marker content was wrong")
    require(
        "unsupported parameter" not in combined.lower()
        and "function tools with reasoning_effort" not in combined.lower(),
        "yolo used an incompatible GPT-5.6 transport",
    )
    return combined


def check_cache(data_root):
    capability_dir = os.path.join(data_root, "aishe", "capabilities")
    files = []
    if os.path.isdir(capability_dir):
        files = [
            os.path.join(capability_dir, name)
            for name in os.listdir(capability_dir)
            if name.endswith(".json")
        ]
    require(files, "live capability report was not persisted")
    for path in files:
        with open(path, encoding="utf-8") as file:
            payload = json.load(file)
        if payload.get("model") == MODEL:
            require(payload.get("transport") == "responses", "cached wrong transport")
            require(payload.get("tools", {}).get("state") == "pass", "tools not cached")
            return
    raise AssertionError("no cached capability report for %s" % MODEL)


def run_child(script, env, *args):
    result = run(
        [sys.executable, os.path.join(TEST_DIR, script), BINARY, *args],
        env,
        cwd=REPO_ROOT,
        timeout=7200,
    )
    require(
        result.returncode == 0,
        "%s failed\nstdout:\n%s\nstderr:\n%s"
        % (script, result.stdout[-4000:], result.stderr[-4000:]),
    )
    return result.stdout + result.stderr


def main():
    if not KEY:
        print("SKIP: AISHE_REALTEST_KEY not set (paid release matrix is opt-in)")
        return 0
    if not os.path.isfile(BINARY):
        print("FAIL: binary not found: %s" % BINARY, file=sys.stderr)
        return 1
    require(FUZZ_SCALE > 0, "fuzz scale must be positive")

    root = tempfile.mkdtemp(prefix="aishe-live-release-")
    acceptance_path = None
    started = time.monotonic()
    checks = []
    transcript = []
    try:
        config_root, data_root = write_config(root)
        work = os.path.join(root, "work")
        os.makedirs(work)
        shell_id, acceptance_path = create_workspace_acceptance()
        env = isolated_env(
            root, config_root, data_root, shell_id, acceptance_path
        )

        transcript.append(
            check_capabilities(
                [BINARY, "provider", "test", "--live", "--json"],
                env,
                "provider test --live",
            )
        )
        checks.append("provider text/structured/tools/streaming")
        check_cache(data_root)
        checks.append("capability cache persistence")

        transcript.append(
            check_capabilities(
                [BINARY, "doctor", "--live", "--json"], env, "doctor --live"
            )
        )
        checks.append("doctor live report")

        transcript.append(
            check_suggest(
                env, "what is the capital of France", "answer", contains="Paris"
            )
        )
        transcript.append(
            check_suggest(env, "print the current directory", "command")
        )
        checks.append("answer/command JSON contracts")

        transcript.append(check_yolo_tool(env, work))
        checks.append("real yolo function-tool round trip")

        transcript.append(run_child("real_model.py", env))
        checks.append("20-case labelled classification")
        transcript.append(run_child("real_fuzz.py", env, str(FUZZ_SCALE)))
        checks.append("%d-case adversarial fuzz" % (28 * FUZZ_SCALE))

        incompatible = (
            "unsupported parameter",
            "function tools with reasoning_effort",
            "/v1/chat/completions",
        )
        joined = "\n".join(transcript).lower()
        for marker in incompatible:
            require(marker not in joined, "compatibility regression: " + marker)

        duration = time.monotonic() - started
        os.makedirs(REPORT_DIR, exist_ok=True)
        timestamp = datetime.datetime.now(datetime.timezone.utc).strftime(
            "%Y%m%dT%H%M%SZ"
        )
        report = os.path.join(REPORT_DIR, "live-release-%s.md" % timestamp)
        with open(report, "w", encoding="utf-8") as file:
            file.write("# Aishe paid release-candidate matrix\n\n")
            file.write("- Model: `%s`\n" % MODEL)
            file.write("- Endpoint: `%s`\n" % BASE_URL)
            file.write("- Transport: `responses`\n")
            file.write("- Fuzz scale: `%d`\n" % FUZZ_SCALE)
            file.write("- Duration: `%.1fs`\n" % duration)
            file.write("- Result: **PASS**\n\n")
            for check in checks:
                file.write("- PASS %s\n" % check)
        print(
            "live-release: PASS (%d checks, %.0fs) -> %s"
            % (len(checks), duration, report)
        )
        return 0
    except (AssertionError, subprocess.TimeoutExpired) as error:
        print("live-release: FAIL: %s" % error, file=sys.stderr)
        return 1
    finally:
        shutil.rmtree(root, ignore_errors=True)
        if acceptance_path:
            try:
                os.unlink(acceptance_path)
            except FileNotFoundError:
                pass


if __name__ == "__main__":
    sys.exit(main())
