#!/usr/bin/env python3
"""/usage reports tokens, cache, cost basis, and plan without content logging.

The audit log carries prompts and answers and is opt-in; the usage ledger is
numbers only and always written, so the report must work with logging off.
"""

import json
import os
import subprocess
import sys

from pty_helper import binary, environment

LEDGER_ROWS = [
    {
        "schema_version": 1,
        "ts_ms": 1788400000000,
        "session": "s-1",
        "model": "gpt-test",
        "provider": "openai",
        "connection_id": "openai",
        "auth_type": "oauth",
        "mode": "auto",
        "tokens_in": 1000,
        "tokens_out": 200,
        "cache_read_tokens": 3000,
        "cache_write_tokens": 100,
        "reasoning_tokens": 50,
        "cost_usd": 0.0,
        "duration_ms": 2500,
    },
    {
        "schema_version": 1,
        "ts_ms": 1788400100000,
        "session": "s-2",
        "model": "gpt-test",
        "provider": "openai",
        "connection_id": "openai",
        "auth_type": "oauth",
        "mode": "auto",
        "tokens_in": 500,
        "tokens_out": 100,
        "cache_read_tokens": 500,
        "cache_write_tokens": 0,
        "reasoning_tokens": 10,
        "cost_usd": 0.0,
        "duration_ms": 1500,
    },
]


def run(env, *args):
    return subprocess.run(
        [binary(), *args], env=env, capture_output=True, text=True, timeout=60
    )


def main():
    home, env = environment("usagereport")
    # Audit logging stays off: the report must not depend on it.
    ledger = os.path.join(home, ".local", "share", "aishe", "usage.jsonl")
    os.makedirs(os.path.dirname(ledger), exist_ok=True)
    with open(ledger, "w", encoding="utf-8") as file:
        for row in LEDGER_ROWS:
            file.write(json.dumps(row) + "\n")

    text = run(env, "usage").stdout
    for needed in ("AIShe usage", "1,500 in", "300 out", "2 turns"):
        if needed not in text:
            raise AssertionError("usage report omitted %r:\n%s" % (needed, text))
    # 3500 cached of 5000 offered.
    if "70% cached" not in text:
        raise AssertionError("cache hit rate missing or wrong:\n%s" % text)
    if "60 thinking" not in text:
        raise AssertionError("reasoning tokens missing:\n%s" % text)
    # A subscription has no per-token price; that is not "unpriced".
    if "plan" not in text or "no price set" in text:
        raise AssertionError("subscription cost basis is wrong:\n%s" % text)

    document = json.loads(run(env, "usage", "--json").stdout)
    total = document["total"]
    if total["tokens_in"] != 1500 or total["cache_read_tokens"] != 3500:
        raise AssertionError("json totals are wrong: %s" % json.dumps(total))
    if total["cost_basis"] != "subscription":
        raise AssertionError("json cost basis is wrong: %s" % json.dumps(total))
    if round(total["cache_hit_percent"]) != 70:
        raise AssertionError("json cache hit rate is wrong: %s" % json.dumps(total))

    # Nothing in the ledger may carry conversation content.
    raw = open(ledger, encoding="utf-8").read().lower()
    for forbidden in ("prompt", "response", "summary", "command"):
        if forbidden in raw:
            raise AssertionError("the usage ledger carries %r" % forbidden)

    print("usage report: ok (%d turns, 70%% cached, subscription basis)" % len(LEDGER_ROWS))


if __name__ == "__main__":
    main()
