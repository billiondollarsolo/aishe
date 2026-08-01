# Release readiness and rollback

An AIShe release candidate is publishable only when its qualification evidence
has an unambiguous disposition. A passing build is necessary but not sufficient:
the checked binary, persistent schemas, managed runtime, terminal front ends,
dependency policy, installers, safety boundary, and rollback path are one
release unit.

## Required evidence

Run the qualification driver from the exact release commit and retain its JSON
artifact:

```sh
python3 tests/qualify.py release \
  --output test-results/qualification-release.json \
  --keep-going
```

The release profile must build `target/release/aishe` itself and verify the
binary's Cargo version and commit before any external harness runs. Evidence
from a different or unverified binary is invalid.

The release owner records one of `pass`, `fail`, `not_run`, or
`not_applicable` for every group below. A missing credential, tool, OS, or paid
budget is never represented as pass.

| Group | Release requirement |
|---|---|
| Source/build identity | clean intended commit; locked release build; exact embedded version/commit; artifact digest |
| Rust correctness | format; strict all-target/all-feature Clippy; all-target locked tests; no-default-feature tests where maintained |
| Dependency policy | `cargo deny` advisories/bans/licenses/sources; every exception still owned and before its review deadline; no unexpected duplicate transport/terminal stack |
| JSON/persistence compatibility | v0.5/v0.6/v1 API fixtures; error schema; route corpus; config migrations with backup/rollback; task/session records remain readable |
| Routing/shell UX | route corpus; zsh highlight/submit parity; direct shell latency/lazy-loading; Bash declared tier; forced-route non-stickiness |
| Terminal UX | Linux and macOS bounded PTY suites; picker/layout/static/ASCII/NO_COLOR; signals, resize, setup, statusline, staging, and answer boundaries |
| Safety/security | versioned threat model; Linux functional bubblewrap evidence; workspace/host authority tests; policy and secret-isolation tests; deterministic parser/boundary fuzz seeds |
| Runtime/backend | pinned manifest/plugin digests; install, authenticated verify, repair, rollback; event/tool contract; concurrency, interruption/resume, reconnect, and bounded soak |
| Install/upgrade | transactional install fault tests; supported package/install paths; upgrade preserves every user-state category by default; uninstall category preservation tests |
| Performance | versioned direct-shell, route, picker, rendering, RSS, binary-size, cold/warm backend report; enforced stable-host thresholds and explicit informational metrics |
| Live providers | paid live classification/fuzz/soak report, or a named owner, reason, risk assessment, and expiry for deferral |

`passed_with_skips` can be useful local evidence, but it is not by itself a
release decision. Platform-specific required gates must pass on their maintained
platform. A paid-live or long-soak skip requires an explicit written disposition
in the release record; a deterministic supported-platform skip is a hold.

## Decision states

- **Ready:** every deterministic supported-platform requirement passed; any
  paid/live deferral has an owner, rationale, expiry, and accepted risk; artifact
  digests match the candidate.
- **Hold:** a required gate failed, was not run, used an unverified binary, has
  stale/expired policy metadata, or lacks a clear skip disposition.
- **Rejected:** evidence found a security boundary violation, persistent-state
  loss/incompatibility, unsafe installer behavior, or nondeterministic duplicate
  side effects. Do not waive these into Ready.

The release record must name the qualification profile revision, threat-model
version, route-corpus digest, runtime/plugin pins, OS/shell families, binary
digest, all skips, and the final owner decision.

## Binary rollback

Rollback replaces only program/runtime artifacts unless the user separately
requests state deletion:

1. Preserve `<config>` and `<data>` in place. Never restore an old directory
   snapshot over newer state as an automatic binary rollback.
2. Stop the managed supervisor: `aishe backend stop`.
3. Use `aishe backend rollback` for the immediately previous verified compatible
   OpenCode runtime, or install the runtime pinned by the target AIShe binary.
4. Reinstall the prior verified AIShe package/binary through its original
   package manager.
5. Run `aishe doctor --json` and `aishe backend verify --live` with the rolled
   back binary before resuming agent work.

Config migrations are transactional and create timestamped backups, but a
newer binary may have written fields an older binary does not understand.
Rollback therefore follows a forward-compatibility rule: the older binary must
either ignore documented additive fields or fail with `config.invalid`; it must
not destructively rewrite the file. Restore a migration backup only after
reviewing the diff and only when abandoning newer settings intentionally.

Task, session, audit, undo, history, and OAuth records are persistent user state.
They are not runtime rollback material. A prior binary must keep promised legacy
records readable or fail closed with a recovery command. If it cannot, hold the
rollback and use the newer binary to export or migrate the state first.

## Failed rollout response

For a newly published artifact with a regression:

1. Stop promotion/download references while preserving published checksums and
   provenance for incident analysis.
2. Classify whether the fault is binary-only, runtime-pin-specific, provider
   compatibility, terminal/platform-specific, or persistent-state-affecting.
3. If any prompt was admitted or tool effect may be outcome-unknown, preserve
   the task/session and reconcile it; do not transparently replay through a
   fallback engine.
4. Roll back binary/runtime artifacts using the sequence above. Do not invoke
   `aishe uninstall --all`.
5. Capture a redacted support bundle and the failed qualification gate artifact;
   inspect both before sharing.
6. Add the finding as a deterministic regression fixture before re-promotion.

See [Data retention and deletion](data-retention.md),
[Automation contracts](automation.md), [Development](development.md), and the
[threat model](../SECURITY.md).
