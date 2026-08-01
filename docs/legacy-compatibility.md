# Legacy front-end and configuration lifecycle

This inventory prevents old implementation details from becoming accidental
product promises. The supported interactive product is the real zsh PTY; the
native zsh and qualified Bash hooks are additional front ends. Reedline and its
simulated shell were removed before config schema 7 and are not dormant modes.

| Surface | Current state | Compatibility behavior | Earliest removal review |
| --- | --- | --- | --- |
| `front_end = "reedline"`, `--pty`, `--no-pty` | removed | Old schema files are backed up transactionally; unknown reedline fields are omitted when migrated. Launch `aishe` or `aishe zsh`. | already removed |
| reedline ghost-text/editor/completion fields | removed | Migration backup retains the original bytes; active config drops the tombstones. `/editor` and related removed slash names fail locally with guidance and never reach a model. | already removed |
| `#` force-agent prefix | deprecated zsh/Rust alias; Bash comment | `?` is canonical. A bounded local migration cue remains through 0.8; removal is planned for 0.9. | 0.9 |
| `[providers]` direct blocks and `auth = auto` | read/write migration compatibility | Named connections are authoritative. A migrated auto connection preserves the v0.5 key-first/OAuth fallback until the operator chooses an explicit auth type. | after two minor releases with migration telemetry/evidence |
| `yolo_confirm_dangerous`, `yolo_confirm`, `yolo_sandbox`, native `memory`/`cache` | active only for native pre-admission fallback and legacy task resume | They do not control managed OpenCode authority. Config/help labels them compatibility settings; legacy tasks retain canonical resume. | not before native fallback and legacy-task support are separately retired |
| legacy native task records | readable retained data | `aishe sessions` labels them legacy; `aishe resume` continues without replaying completed effects. Deletion remains explicit and category-specific. | at least two minor releases after an export/migration tool ships |
| legacy `llmsh` config path | one-time import | If AIShe config is absent, a valid old file is ported; malformed source is left untouched. | retain through 1.0 unless migration evidence says otherwise |

## Deletion rules

A field is not removed merely because the managed path no longer consumes it.
Removal requires: an executable usage inventory; no active help advertising it;
an old-config fixture; an idempotent migration; a byte-for-byte backup before
rewrite; a tombstone or precise unknown-field policy; and a documented support
window for persisted records. Provider fallback and legacy task resume are
effectful compatibility paths, not dead code, so this milestone does not delete
their fields.

The schema-2 CLI migration fixture proves both halves of the policy: removed
reedline fields disappear from schema 7, while the timestamped migration backup
is byte-identical to the source. Command-registry conformance proves removed
slash names remain local tombstones.
