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
        "requests": 1,
        "unreported_requests": 0,
        "reported_tokens_in": 1000,
        "reported_tokens_out": 200,
        "attributed_requests": 1,
        "attributed_tokens_in": 1000,
        "attributed_tokens_out": 200,
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
        "requests": 1,
        "unreported_requests": 0,
        "reported_tokens_in": 500,
        "reported_tokens_out": 100,
        "attributed_requests": 1,
        "attributed_tokens_in": 500,
        "attributed_tokens_out": 100,
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
    if total["cost_usd"] is not None or total["cost_coverage"] != "subscription":
        raise AssertionError("subscription tokens fabricated a dollar estimate: %s" % total)

    # Nothing in the ledger may carry conversation content.
    raw = open(ledger, encoding="utf-8").read().lower()
    for forbidden in ("prompt", "response", "summary", "command"):
        if forbidden in raw:
            raise AssertionError("the usage ledger carries %r" % forbidden)

    coverage_reports(env, ledger)
    print("usage report: ok (subscription, unknown usage, known zero, partial subtotal, legacy coverage)")


def coverage_reports(env, ledger):
    """Exercise persisted CLI aggregation without inferring coverage from zeros."""
    base = {
        "schema_version": 1,
        "ts_ms": 1788400000000,
        "session": "priced-coverage",
        "model": "gpt-4o",
        "provider": "openai",
        "connection_id": "priced-work",
        "auth_type": "api_key",
        "mode": "ask",
        "tokens_in": 0,
        "tokens_out": 0,
    }

    def report(rows):
        with open(ledger, "w", encoding="utf-8") as file:
            for row in rows:
                file.write(json.dumps(row) + "\n")
        result = run(env, "usage", "--json")
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)["total"], run(env, "usage").stdout

    missing = dict(base, requests=1, unreported_requests=1,
                   reported_tokens_in=0, reported_tokens_out=0,
                   attributed_requests=0, attributed_tokens_in=0, attributed_tokens_out=0)
    old = dict(base, tokens_in=600, tokens_out=100)
    for row in (missing, old):
        total, text = report([row])
        assert total["cost_usd"] is None and total["cost_coverage"] == "unknown", total
        assert total["unreported_requests"] == 1, total
        assert "cost n/a" in text and "tokens n/a" in text and "~$0.0000" not in text, text

    zero = dict(base, requests=1, unreported_requests=0,
                reported_tokens_in=0, reported_tokens_out=0,
                attributed_requests=1, attributed_tokens_in=0, attributed_tokens_out=0)
    total, text = report([zero])
    assert total["cost_usd"] == 0.0 and total["cost_coverage"] == "complete", total
    assert total["unreported_requests"] == 0 and "~$0.0000" in text, text

    known = dict(zero, tokens_in=1000, tokens_out=200,
                 reported_tokens_in=1000, reported_tokens_out=200,
                 attributed_tokens_in=1000, attributed_tokens_out=200)
    total, text = report([known, dict(missing, tokens_in=9000), old])
    assert total["cost_usd"] is None and total["cost_coverage"] == "partial", total
    assert abs(total["known_cost_subtotal_usd"] - 0.0045) < 1e-10, total
    assert total["reported_tokens_in"] == 1000 and total["reported_tokens_out"] == 200, total
    assert total["requests"] == 3 and total["unknown_cost_requests"] == 2, total
    assert "~$0.0045 (partial; 2 unknown)" in text, text
    assert "1,000 in · 200 out (partial)" in text, text

    total, text = report([zero, missing])
    assert total["cost_usd"] is None and total["known_cost_subtotal_usd"] == 0, total
    assert "~$0.0000 (partial; 1 unknown)" in text, text

    # Provider usage can be complete while the billed model is ambiguous.
    # Preserve those token counts independently from the cost provenance.
    unattributed = dict(known, attributed_requests=0,
                        attributed_tokens_in=0, attributed_tokens_out=0)
    for row in (unattributed, {key: value for key, value in known.items()
                              if not key.startswith("attributed_")}):
        total, text = report([row])
        assert total["unreported_requests"] == 0 and total["reported_tokens_in"] == 1000, total
        assert total["attributed_requests"] == 0 and total["cost_usd"] is None, total
        assert "1,000 in · 200 out" in text and "(partial)" not in text and "cost n/a" in text, text


if __name__ == "__main__":
    main()
