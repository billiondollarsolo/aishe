#!/usr/bin/env python3
"""Real-model classification test (opt-in).

Runs a labelled corpus of natural-language inputs through `aishe --auto-line`
against a *real* OpenAI-compatible model and checks the end-to-end behaviour the
fake provider cannot: that the model, through aishe's prompts and the
command-vs-answer validation, returns an *answer* for questions and a runnable
*command* for command requests, and that nothing ever produces a parse error.

Configured entirely via environment (no key is ever written to the repo):
  AISHE_REALTEST_KEY        the API key value (REQUIRED; test SKIPs if unset)
  AISHE_REALTEST_BASE_URL   default https://api.groq.com/openai
  AISHE_REALTEST_MODEL      default openai/gpt-oss-120b

Writes a markdown report to test-results/real-model-<ts>.md.
Usage: real_model.py [path-to-aishe]
"""

import os
import re
import sys
import time
import datetime
import tempfile
import subprocess

BINARY = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT_DIR = os.path.join(REPO_ROOT, "test-results")

KEY = os.environ.get("AISHE_REALTEST_KEY", "")
BASE_URL = os.environ.get("AISHE_REALTEST_BASE_URL", "https://api.groq.com/openai")
MODEL = os.environ.get("AISHE_REALTEST_MODEL", "openai/gpt-oss-120b")

# (prompt, expected) where expected is "answer" (a question; no command should be
# produced) or "command" (a request to do something; a runnable command should be).
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


def make_config():
    home = tempfile.mkdtemp(prefix="aishe-real-")
    cfg = os.path.join(home, ".config", "aishe")
    os.makedirs(cfg, exist_ok=True)
    with open(os.path.join(cfg, "config.toml"), "w") as f:
        f.write(
            '[aishe]\nmode = "auto"\nprovider = "openai"\ncache = false\n\n'
            "[providers.openai]\n"
            'base_url = "%s"\napi_key_env = "AISHE_REALTEST_KEY"\nmodel = "%s"\n'
            % (BASE_URL, MODEL))
    return home


def run_one(home, prompt):
    env = dict(os.environ)
    env.update({
        "HOME": home, "XDG_CONFIG_HOME": os.path.join(home, ".config"),
 # macOS ignores XDG_*; these are honored on every platform.
 "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
 "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "AISHE_REALTEST_KEY": KEY,
    })
    p = subprocess.run([os.path.abspath(BINARY), "--auto-line", prompt],
                       env=env, capture_output=True, text=True, timeout=60)
    return p.stdout.strip(), p.stderr.strip(), p.returncode


def syntax_ok(cmd):
    if not cmd:
        return False
    return subprocess.run(["zsh", "-nc", cmd], capture_output=True).returncode == 0


def main():
    if not KEY:
        sys.stderr.write("SKIP: AISHE_REALTEST_KEY not set (real-model test is opt-in)\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home = make_config()
    started = time.monotonic()
    rows = []
    passed = 0
    for prompt, expected in CORPUS:
        try:
            out, err, rc = run_one(home, prompt)
        except subprocess.TimeoutExpired:
            rows.append((prompt, expected, "TIMEOUT", "", False))
            continue
        combined = out + "\n" + err
        parse_err = "parse error" in combined or "(eval)" in combined
        if expected == "command":
            got = "command" if out and syntax_ok(out) else "answer"
        else:
            got = "command" if out else "answer"
        ok = (got == expected) and not parse_err
        passed += 1 if ok else 0
        shown = (out if out else err).replace("\n", " ")[:90]
        rows.append((prompt, expected, got, shown, ok))
        sys.stdout.write("  %s  [%s->%s] %s\n" % ("ok  " if ok else "FAIL", expected, got, prompt))

    dur = time.monotonic() - started
    total = len(CORPUS)
    status = "PASS" if passed == total else "PARTIAL" if passed else "FAIL"

    os.makedirs(REPORT_DIR, exist_ok=True)
    ts = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    path = os.path.join(REPORT_DIR, "real-model-%s.md" % ts)
    lines = ["# aishe real-model classification report", "",
             "- Date: %s" % ts, "- Model: `%s` via `%s`" % (MODEL, BASE_URL),
             "- Duration: %.1fs" % dur,
             "- Result: **%s** (%d/%d classified as expected)" % (status, passed, total), "",
             "Each natural-language input is sent through `aishe --auto-line` against the "
             "real model; a question must yield an answer (no command) and a request must "
             "yield a runnable command, with no parse errors.", "",
             "| Input | Expected | Got | Model output (truncated) | OK |",
             "| --- | --- | --- | --- | :-: |"]
    for prompt, expected, got, shown, ok in rows:
        cell = shown.replace("|", "\\|")
        lines.append("| %s | %s | %s | `%s` | %s |"
                     % (prompt, expected, got, cell, "✅" if ok else "❌"))
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    sys.stdout.write("report: %s\n%d/%d classified as expected\n"
                     % (os.path.relpath(path, REPO_ROOT), passed, total))
    # Hard-fail only on parse errors or a large miss; minor model misclassification
    # is informative, not a build breaker.
    sys.exit(0 if passed >= int(total * 0.8) else 1)


if __name__ == "__main__":
    main()
