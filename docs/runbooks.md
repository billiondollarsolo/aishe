# Runbooks

A successful yolo session — "set up nginx with TLS", "provision this box" — is
usually throwaway. `aishe runbook` turns one into a committable, auditable
**script + markdown runbook**, generated from the [audit log](logging.md) after
the fact. It's how an ad-hoc AI session becomes a reproducible ops artifact your
team can review and re-run.

Requires [logging](logging.md) to be on (so the session was recorded):

```toml
[aishe]
[logging]
enabled = true
```

## Generating a runbook

```sh
aishe runbook                         # export the most recent session
aishe runbook --session <id>          # a specific session (see `aishe log`)
aishe runbook -o ./ops                # write the files to a directory
```

It writes two files named `runbook-<session>.{sh,md}`:

- **`runbook-<session>.sh`** — a runnable script: a `#!/usr/bin/env bash` header
  with the original request and `set -uo pipefail`, then the exact commands the
  AI ran, in order. Commands that failed when recorded are annotated. Review and
  edit before running.
- **`runbook-<session>.md`** — a human narrative: the request as the title, each
  step as a numbered command with its exit code and source, the model's plan/notes
  as block quotes, and a "Reproduce" section.

Secrets are already redacted in the audit log the runbook is built from, so the
files are safe to commit and share (don't disable `[logging] redact`).

## Replaying

```sh
aishe runbook --replay                # re-run the recorded commands
```

`--replay` runs each recorded command through the **safety gate** (not the model),
in order: safe commands run, dangerous ones are skipped with a warning. Because
the model is never involved, reproduction is deterministic — the same recorded
commands run the same way every time.

To find a session id, use [`aishe log`](logging.md):

```sh
aishe log --action action     # the commands, grouped by session in the timeline
```
