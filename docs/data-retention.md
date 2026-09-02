# Data retention, export, and deletion

AIShe preserves user state by default. Setup, updates, runtime repair, and a
plain `aishe uninstall` do not delete configuration, credentials, history,
sessions, tasks, audit records, or undo records. Deletion is category-specific,
has a dry-run preview, and requires confirmation.

AIShe has no cloud-side retention control for a provider or for OpenCode itself.
This page describes local AIShe-managed files. Provider-side conversations,
logs, and billing records remain subject to that provider's policy.

## Local state inventory

Paths below use `<config>` and `<data>`. Run `aishe doctor` to see the resolved
paths for the current environment. `AISHE_CONFIG_DIR`, `AISHE_DATA_DIR`,
`AISHE_LOG_FILE`, `AISHE_UNDO_JOURNAL`, and `AISHE_RUNTIME_DIR` can override
individual locations.

| State | Typical location | Default retention and bound | Export or inspect | Exact deletion preview |
|---|---|---|---|---|
| Configuration | `<config>/config.toml`, `aishrc`, `commands/`, `skills/` | Indefinite; Doctor warns when config plus managed credential state reaches 128 MiB | `aishe config --effective`; copy private custom files directly | `aishe uninstall --config --dry-run` |
| API credentials | `<config>/credentials.toml` | Indefinite; private file, mode `0600` on Unix | `aishe auth status`; secret values are intentionally not exportable through JSON | `aishe uninstall --config --dry-run` |
| Managed OAuth state | `<data>/backend/opencode/xdg/` | Indefinite; profile-isolated private token state | `aishe auth status`; use the provider's own account controls for provider-side revocation | `aishe uninstall --config --dry-run` |
| Shell and semantic history | `<data>/history.ext`, `history.vec` | Indefinite; no automatic rotation; Doctor warns at 64 MiB | Copy the private files before deletion | `aishe uninstall --history --dry-run` |
| AIShe/OpenCode session mappings | `<data>/backend/sessions/` | Indefinite; the mapping index is bounded to 10,000 records and 8 MiB | `aishe sessions --json`; `aishe session show ID --json` | All: `aishe uninstall --sessions --dry-run`; one task session: `aishe session delete ID` |
| Durable tasks/checkpoints | `<data>/tasks/` | Indefinite; Doctor warns when sessions, tasks, and usage journals total 512 MiB | `aishe sessions --json`; `aishe session show ID --json` | `aishe uninstall --sessions --dry-run` |
| Background agent tasks | `<data>/background-tasks/` | Indefinite; each log is capped at 8 MiB and each task owns at most one isolated worktree | `aishe task list`; `aishe task show ID --json`; `aishe task tail ID` | Review then `aishe task discard ID`; removal through uninstall is not yet categorized |
| Repository indexes | `<data>/repo-index/` | Indefinite; tracked text only, capped at 10,000 files and 64 MiB per repository | `aishe index --status`; `aishe index --query TEXT --json` | Manual removal of the exact private per-repository directory |
| Failure capsules | `<data>/failures/` | One bounded capsule per live shell; cleared after success and normal shell exit | `aishe last show --json` | `aishe last clear` |
| Binary rollback slot | `<data>/updates/previous-aishe` | Exactly one previous executable after an in-app update | `aishe update check` | Replaced on the next update/rollback; manual exact-file removal |
| Tool/idempotency and durable usage journals | `<data>/backend/journal/`, `journal.json` | Indefinite; included in the shared 512 MiB Doctor threshold | Relevant task/session JSON and the audit export | `aishe uninstall --sessions --dry-run` |
| Interactive usage tally/status | system temporary directory | One interactive shell lifetime; guards remove it on every normal return path | `aishe status --json`, `aishe usage`, or the final session summary | Exit the owning AIShe shell; stale OS temporary files may be removed by normal OS cleanup |
| Capability cache | `<data>/capabilities/` | Seven-day freshness TTL; Doctor warns at 32 MiB and `doctor --fix` removes stale or unreadable records | `aishe models --json`; `aishe provider test --json` | Safe stale cleanup: `aishe doctor --fix`; refresh one source: `aishe models --refresh` |
| Audit log | `<data>/audit.jsonl` or configured path | Off by default; when enabled, the active file rotates at 256 MiB and exactly one `.1` generation is retained | `aishe log --json`; copy both JSONL generations if a complete retained export is required | `aishe uninstall --audit-undo --dry-run` |
| Undo journal | `<data>/undo.jsonl` | Active file rotates before append at 128 MiB; exactly one complete `.1` generation is retained | `aishe undo --list`; copy both private JSONL generations | `aishe uninstall --audit-undo --dry-run` |
| Managed runtime/cache | `<data>/runtime/` or `AISHE_RUNTIME_DIR` | Retained across upgrades; Doctor warns at 2 GiB | `aishe backend status --json`; this contains replaceable artifacts, not user conversations | `aishe uninstall --runtime --dry-run` |
| Setup/tour drafts | `<data>/setup-draft.json`, `tour-state.json` | Until setup/tour completes, restarts, or config state is removed | Resume setup or tour; inspect the private JSON locally if needed | `aishe uninstall --config --dry-run` |
| Trust records | Within AIShe configuration state | Indefinite until explicitly untrusted or config state is removed | `aishe trust --list` | Prefer `aishe untrust`; full removal is under `--config` |
| Support bundles | The exact path passed to `aishe doctor --bundle PATH` | Caller-owned and indefinite; no background discovery or cleanup | Inspect the JSON before sharing it | Delete that exact caller-chosen file with normal filesystem tools |

Audit and undo rotation happens before a writer appends to an active file that
has reached its limit. Rotation replaces the older `.1` generation. AIShe does
not truncate an undo pre-image: partial file contents would make restoration
unsafe. A configured audit or undo path outside `<data>` remains private user
state; the uninstall preview resolves and displays that exact path.

## Preview, confirm, and preserve

Always preview first:

```sh
aishe uninstall --dry-run
aishe uninstall --sessions --dry-run
aishe uninstall --config --dry-run
aishe uninstall --history --dry-run
aishe uninstall --audit-undo --dry-run
aishe uninstall --all --dry-run
```

The default preview selects only the binary/completions/man pages and the
replaceable managed runtime/cache. It preserves every user-state category.
Applying a user-state plan requires an interactive confirmation, or an explicit
`--yes` in non-interactive automation:

```sh
aishe uninstall --history --yes
aishe uninstall --sessions --yes
```

These operations are permanently unrecoverable by AIShe. `--sessions` removes
task, session-mapping, tool-journal, and durable usage state, but deliberately
preserves API credentials and OpenCode OAuth login state. `--config` removes
configuration, API credentials, managed OAuth state, custom commands/skills,
and setup/tour drafts. It does not imply `--sessions`, `--history`, or
`--audit-undo`.

`--all` is the only shortcut that selects every category. Package-manager-owned
binaries should still be removed through the package manager after any desired
state cleanup.

## Export before deletion

Machine-readable commands write only their documented schema to stdout:

```sh
aishe config --effective --json > aishe-config-export.json
aishe sessions --json > aishe-sessions-index.json
aishe session show SESSION_ID --json > aishe-session.json
aishe log --json > aishe-audit-export.jsonl
aishe doctor --json > aishe-diagnostics.json
```

The config export is intentionally not a credential backup. If credentials or
OAuth state must be migrated, protect a direct filesystem backup with the same
care as the original private files. Audit JSONL can contain prompts, responses,
commands, paths, and tool output even when secret redaction is enabled.

## Doctor size checks

`aishe doctor` and `aishe doctor --json` expose stable `state.size.*` checks for
history, audit, undo, sessions/tasks, config/credentials, capability cache, and
managed runtime. Directory scans do not follow symbolic links and stop after
10,000 filesystem entries. A stopped or unreadable scan is a warning rather
than an unsafe recursive traversal. Every warning names a non-destructive
preview or safe cache cleanup command.

The thresholds are operational warnings, not automatic deletion quotas. They
make growth visible while leaving retention choices with the user.

See also [Automation contracts](automation.md), [Logging and privacy](logging.md),
[Installation](installation.md#uninstall), and [Troubleshooting](troubleshooting.md).
