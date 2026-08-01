# Automation and machine-readable contracts

AIShe is pre-1.0, but its public JSON, JSONL, persisted task records, and exit
statuses are compatibility surfaces. This document is the inventory and change
policy for those surfaces. A `--json` command writes data only to stdout and
diagnostics only to stderr. JSON output never contains terminal styling.

## Compatibility rules

- A top-level `schema_version` identifies a document contract. It is independent
  of AIShe's package version and the config file's `version`.
- Within one schema version, fields may be added and enum values may be added.
  Consumers must ignore unknown fields and fail closed on an unknown enum that
  controls execution or authority.
- Removing a field, changing its type or meaning, changing nullability, or
  weakening a safety enum requires a schema bump. Keep readers and fixtures for
  at least the previous two AIShe minor releases before 1.0.
- One-document commands emit exactly one JSON document followed by a newline.
  JSONL commands emit one complete object per line. Human notices, warnings,
  recovery actions, and structured failures belong on stderr.
- JSON mode implies plain output: no ANSI escapes, cursor controls, progress
  animation, terminal glyph assumptions, or prompts.
- Unless a row below says otherwise, success is exit 0. Clap usage errors are
  exit 2. A command-specific nonzero status is part of that command's contract.
- Paths are platform-native strings. Timestamps ending in `_ms` are integer
  milliseconds since the Unix epoch. Money is USD. Optional fields are either
  omitted or explicitly nullable exactly as documented; consumers must not
  treat an omitted field and an empty string as equivalent.
- Secrets and local backend control credentials are never public fields. Values
  sourced from providers, repositories, or models must be redacted,
  control-safe, and bounded before they are placed in an error envelope.

The compatibility owner named below is the source module plus its contract
tests; it is an ownership seam, not a particular person.

## Error document v1

JSON-mode failures that have migrated to the shared error boundary write one
v1 object to stderr:

```json
{
  "schema_version": 1,
  "code": "network.provider_unreachable",
  "message": "AIShe could not reach the provider.",
  "retryable": true,
  "exit_code": 6,
  "next_action": "Check connectivity and the endpoint with `aishe doctor --probe`, then retry.",
  "detail": null
}
```

`code` is `namespace.snake_case_name`. Namespace exit assignments are stable:
`internal=1`, `cli=2`, `config=3`, `auth=4`, `provider=5`, `network=6`,
`policy=7`, `sandbox=8`, `backend=9`, and `io=10`. `message` and
`next_action` are at most 320 UTF-8 bytes, `detail` is null or at most 2,048
bytes, and source chains retain at most eight entries. Every string is redacted,
terminal-control-free, and valid UTF-8.

`aishe suggest --json` is the one legacy exception: the process keeps aggregate
failure status 1 through pre-1.0, while the error document's `exit_code` retains
the more precise namespace status for migration and diagnosis.

Owner: `src/user_error.rs`; gates: its unit snapshots,
`tests/api_compat.rs`, and JSON-mode CLI tests.

## Public surface inventory

### Setup

| Contract | Details |
| --- | --- |
| Command | `aishe setup --non-interactive … --json`, including `--verify` |
| Shape | v1 object: `applied`, command `exit_code`, `config_path`, `credentials_path`, `backend`, nullable `runtime`, `sandbox.{backend,functional}`, `scope`, `network`, and versioned provider capability report |
| Output | One document on stdout. Diagnostics/failures on stderr. No prompts in non-interactive JSON use. |
| Exit | 0 applied/verified; 10 paused; 20 invalid input; 30 dependency; 40 credential; 50 provider; 60 config write; 70 runtime; 80 policy |
| Nullability/bounds | `runtime` may be null. Paths are OS-sized. Provider strings use the provider-report policy below. No credential values. |
| Owner/gates | `src/setup.rs`; setup unit tests and setup/admin-validation fixtures |

### Doctor and support bundle

| Contract | Details |
| --- | --- |
| Command | `aishe doctor [--probe] [--live] [--fix] --json`; `--bundle PATH` writes the same redacted evidence class to a file |
| Shape | v1 report: `generated_at_ms`, AIShe `version`, `platform`, `paths`, `checks[]`, optional `capability_report`. Check status: `pass|warn|fail|skipped`; severity: `info|warning|critical`. |
| Output | One report on stdout; bundle notice only on stderr when not in JSON mode. Network occurs only with `--probe`/`--live`; writes occur only with `--fix` or `--bundle`. |
| Exit | 0 when every critical check is non-failing; 1 otherwise. |
| Nullability/bounds | `capability_report` is omitted unless requested/available. `changed_paths` is omitted when empty. Details are redacted; support bundles omit credentials and control tokens. |
| Owner/gates | `src/diagnostics.rs`; diagnostics snapshots, support-bundle tests, qualification driver |

### Status

| Contract | Details |
| --- | --- |
| Command | `aishe status --json` |
| Shape | v1 object: `model`, `connection.{id,label,provider,endpoint_host,auth,auth_state,selection_scope}`, `mode`, `backend.{engine,readiness}`, `scope`, `network`, `output`, `reasoning_effort`, `status_line`, `budget_usd`, `audit.{enabled,redact,path}`, nullable `session`, and `metrics` map |
| Output | Exactly one pretty-printed document on stdout; stderr is empty on success. |
| Exit | 0. Configuration/load failures use the CLI's failure path. |
| Nullability/bounds | `session` is string or null. Metrics are string values and may be absent. No API key, OAuth token, backend URL, or backend control token. |
| Owner/gates | `src/cli/status.rs`; `tests/fixtures/api/{v0.5,v0.6,v1}/status.json`, `tests/api_compat.rs`, `tests/cli.rs` |

Migration: AIShe 0.5 and 0.6 emitted the identical object without
`schema_version`. Treat those documents as v1 by inserting
`"schema_version": 1`; field meanings did not change.

### Suggest

| Contract | Details |
| --- | --- |
| Command | `aishe suggest --json REQUEST…` |
| Success shape | v1 object with `kind`, `command`, `explanation`, `risk`, and `reason`. `kind=answer|command`; `risk=safe|dangerous|unknown|n/a`. All five fields are always strings. |
| Output | Success/held result: one compact object on stdout and empty stderr. Failure: empty stdout and one shared v1 error object on stderr. No ANSI on either stream. |
| Exit | 0 for prose answers and safe commands; 20 for `dangerous` or unresolved `unknown` commands still emitted for review; 1 for missing request/provider/backend/provider failure. |
| Bounds | Provider output is subject to provider limits. Public errors use the strict bounds above. `unknown` must be treated as held, never safe. |
| Owner/gates | `src/cli/runtime.rs`, `src/modes/suggest.rs`; prior-minor/v1 fixtures, `tests/api_compat.rs`, `tests/live_contract.py`, CLI and real-model contract tests |

Migration: AIShe 0.5 and 0.6 emitted the same success fields without
`schema_version`. Insert version 1 when ingesting those documents. Structured
JSON failures on stderr are new; scripts that only inspect the process status
remain compatible.

### Provider validation

| Contract | Details |
| --- | --- |
| Command | `aishe provider test [--live] --json` |
| Shape | capability-report schema v2: identity/endpoint/model/transport metadata plus `credential`, `reachability`, `model_list`, `model_available`, `text`, `structured`, `tools`, and `streaming` checks. Check state: `pass|warn|fail|skipped`; `error_kind` is optional. |
| Output | One document on stdout. Probe diagnostics on stderr. `--live` can spend a small number of tokens; without it, generation checks are skipped. |
| Exit | 1 only when credential state is `fail`; otherwise 0. Consumers needing stronger qualification must inspect every required check. |
| Nullability/bounds | `error_kind` omitted when absent; model lists may be empty. Credential source/name may appear, never its value. Endpoint is configuration data and may be local. |
| Owner/gates | `src/capabilities.rs`; cached-report compatibility tests, fake-provider tests, live qualification profile |

### Sessions and persisted tasks

| Contract | Details |
| --- | --- |
| Commands | `aishe sessions --json`; `aishe session show ID --json` |
| List shape | v1 wrapper with `managed[]` and `legacy[]`. Managed session mappings are schema v2; legacy task records are schema v1. |
| Task shape | v1 record with identity/timestamps/status/mode/provider/model/cwd/objective, canonical messages, completed/pending tools, usage, and optional last error. Status: `active|interrupted|completed|failed`. |
| Output | One document on stdout. Missing/corrupt record diagnostics on stderr. |
| Exit | 0 on a readable listing/record; 1 on lookup or persistence failure. |
| Nullability/bounds | Task `name`, `pending_tool`, `last_error_kind`, and `last_error` are optional. Objectives and messages are redacted before persistence. Credentials and environment values are forbidden. Stores cap managed mappings at 10,000. |
| Owner/gates | `src/tasks.rs`, `src/backend/opencode/session.rs`; persistence tests and `tests/fixtures/api/v1/task-record.json` |

Persisted records are data-at-rest contracts as well as CLI output. Writers use
private atomic replacement; readers reject unsupported schema versions. A
record migration must be explicit, idempotent, and covered by an old fixture.

### Backend lifecycle

| Contract | Details |
| --- | --- |
| Commands | `aishe backend status|install|verify|repair --json` |
| Shape | v1 documents. `status` is `{schema_version,runtime,supervisor}`; `install` and `repair` are `{schema_version,runtime}`; `verify` is `{schema_version,runtime,live}`. Runtime status is externally tagged `missing|ready|invalid`. Supervisor output is sanitized and omits private control URLs/tokens. |
| Output | One document on stdout; download/verification failures on stderr. `status` is local; install/repair may use the approved download origin. |
| Exit | Status 0 only for a ready runtime, otherwise 1. Successful install/verify/repair 0; failures 1. |
| Nullability/bounds | Instance rows are bounded by configured `max_instances`; log/control secrets are absent. Paths and hashes are strings. |
| Owner/gates | `src/backend/`, `src/cli/backend.rs`; backend security/contract tests and `tests/cli.rs` |

Migration: legacy `install` and `repair` runtime-status roots move unchanged to
`runtime`; legacy `verify` fields stay at the root and gain `schema_version: 1`.

### Models

| Contract | Details |
| --- | --- |
| Command | `aishe models [--connection ID|--provider NAME] [--refresh] --json` |
| Shape | v1 `{schema_version,models}` where `models` is an array of model-ID strings. Empty list is valid. |
| Output | One object on stdout; lookup/provider error on stderr. `--refresh` may make a provider endpoint request. |
| Exit | 0 on a successfully obtained list; 1 on resolution or provider failure. |
| Bounds | Provider determines model count/ID length; consumers must apply their own ingestion bounds. No model ID authorizes execution. |
| Owner/gates | `src/capabilities.rs::list_models`, `src/cli/connection.rs`; provider and CLI tests |

Migration: wrap the legacy bare array as `{"schema_version":1,"models":OLD}`.
The old-array and v1-wrapper fixtures remain in the API compatibility suite.

### Configuration and settings

| Contract | Details |
| --- | --- |
| Commands | `aishe config --json`; `aishe config --effective --json`; `aishe settings --json` |
| Shape | v1 documents. Raw config is `{schema_version,config}`; the nested config retains config-file `version` (currently 7). Effective config is `{schema_version,config,provenance}`. Settings is `{schema_version,config_path,project_path?,fields}`. |
| Output | One document on stdout; parse/validation errors on stderr. Inspection is read-only. |
| Exit | 0 on valid config; 1 on load/validation failure. |
| Nullability/bounds | Optional config fields follow the config schema. Credential environment-variable names and auth bindings may appear; credential values must not. Project-overlay provenance may contain paths. |
| Owner/gates | `src/config.rs`, `src/settings.rs`; config migration, provenance, project-trust, and secret-non-disclosure tests |

Migration: move a legacy raw-config root unchanged under `config`. Add
`schema_version: 1` to legacy effective-config and settings objects. The nested
config's `version` governs TOML persistence and must not be interpreted as the
public CLI document version.

### Connections and authentication

| Contract | Details |
| --- | --- |
| Commands | `aishe connection list|show --json`; `aishe auth status|list --json` |
| Shape | v1 documents. Connection list is `{schema_version,connections}`; connection show retains its legacy fields and adds `schema_version`. Auth list is `{schema_version,profiles}`. Every auth-status variant—named connection, isolated OAuth profile, or API-key/OAuth resolution—retains its legacy fields and adds `schema_version`. |
| Output | One document on stdout. Missing authentication may intentionally return exit 1 with the inspection document still on stdout. Failures that prevent inspection use the shared error document on stderr. |
| Privacy | Credential and OAuth values are forbidden. Profile names, configured credential-variable names, redacted source metadata, and private-store paths may appear. |
| Owner/gates | `src/cli/connection.rs`, `src/auth.rs`; CLI credential, named-connection, Linux credential, API-compatibility, and static inventory tests |

Migration: wrap legacy connection-list arrays under `connections` and auth-list
arrays under `profiles`; add `schema_version: 1` to the legacy object-shaped
show/status documents.

### Readiness and context

| Contract | Details |
| --- | --- |
| Commands | `aishe readiness --json`; `aishe context [--explain] [--preview TEXT] --json` |
| Shape | Readiness v1 is `{schema_version,ready,checks}`. Context preview was already v1 and contains bounded section metadata, estimates, and provenance without section/request contents. |
| Output | One document on stdout. Readiness exits 0 only when ready; context exits 0 on a valid preview/update. |
| Privacy | Readiness details are control-safe. Context JSON intentionally omits context text and the proposed request. |
| Owner/gates | `src/profiles.rs`, `src/context.rs`, `src/cli/settings.rs`; CLI privacy and JSON-contract tests |

Migration: add `schema_version: 1` to the legacy readiness object. Context needs
no migration because its public preview already carried `schema_version`.

### Usage and audit events

| Contract | Details |
| --- | --- |
| Commands | `aishe usage …` is human-only; `aishe log --json` is the current machine-readable source |
| Shape | `log --json` emits v1 audit records as JSONL, one event per line. Event `kind` controls optional fields such as model, command, tool, duration, usage, and exit. Newly written records carry `schema_version`; the reader additively normalizes legacy stored records before replay. |
| Output | JSONL on stdout. Missing-log and empty-filter notices are stderr; an empty result owns stdout and emits zero lines. |
| Exit | 0 for missing/empty/readable logs; malformed, non-object, and unsupported-version stored lines are ignored by the reader. |
| Bounds/privacy | Audit is opt-in. Redaction is on by default. Prompts, responses, commands, and tool outputs can still contain sensitive user data; treat the file as private. The writer rotates at 256 MiB and retains exactly one `.1` generation; see [Data retention and deletion](data-retention.md). |
| Owner/gates | `src/audit.rs`, `src/cli/history.rs`; audit redaction/event tests |

There is no promised `usage --json` contract yet. Automation should aggregate
the JSONL events and pin the event schema it accepts. Adding `usage --json`
requires a versioned document rather than parsing the human table.

### Route diagnostics

| Contract | Details |
| --- | --- |
| Command | `aishe route --json -- LINE…` |
| Shape | v1 diagnostic with route `kind`, stable `reason`, `source`, normalized input metadata, bounded explanation, and safe evidence. Route kinds are `shell`, `natural_language`, and `builtin`; custom commands and MCP prompts resolve before this classifier and are not falsely represented as route kinds. Empty input and invalid CLI syntax are reasons/errors, not kinds. |
| Output | One pretty-printed document on stdout; no provider/backend startup and no network. Invalid CLI syntax belongs on stderr. |
| Exit | 0 for a classified line, including shell/agent distinctions; CLI misuse 2. This command explains a route and never executes it. |
| Bounds | Debug evidence is bounded, redacted/control-safe, and excludes environment values and command output. Same capability fixture and input must produce the same decision. |
| Owner/gates | `src/dispatcher.rs`, `src/cli/backend.rs`; `tests/fixtures/routing/v1.json`, `tests/fixtures/routing/typo-assistance-v1.json`, and `tests/routing_corpus.rs` |

### Discovery hints

| Contract | Details |
| --- | --- |
| Command | `aishe hints status --json`; `aishe hints reset` is human-only and mutating |
| Shape | v1 object: `enabled`, `launch_hint_seen`, and `first_answer_hint_seen` booleans |
| Output | Status writes one ANSI-free document to stdout. Reset writes a short human confirmation and never alters configuration, sessions, history, or failure/typo throttles. |
| Exit | 0 on readable/writable local state; the shared structured error boundary handles malformed, unsafe, or unwritable state. |
| Privacy/bounds | The on-disk schema contains only the version and two booleans, is bounded to 4 KiB, rejects symlinks/non-files, is atomically replaced, and is mode `0600` on Unix. It never stores prompts, answers, commands, paths, provider data, or timestamps. |
| Owner/gates | `src/hints.rs`, `src/cli/hints.rs`; `tests/discovery_hints.rs` and command-surface tests |

### Inventory completeness

Every public `--json`/JSONL command path now carries an explicit schema version;
the unversioned-public-surface count is zero. `src/cli/json_contract.rs` is the
authoritative 23-path inventory. A static conformance test counts the Clap JSON
flags, requires a unique nonzero-version inventory row for each, verifies JSONL
ownership, and checks machine-output routing. Adding a JSON path without
registering and versioning it fails that gate.

`aishe log --json` is JSONL rather than one JSON array. Never concatenate its
lines and parse them as one document. Generated completion, man-page, and
shell-integration text are compatibility surfaces, but they are not JSON; their
gates remain syntax checks, shellcheck, command-registry conformance, and real
PTY harnesses.

## Fixture and review procedure

1. Add or update a representative fixture under `tests/fixtures/api/vN/`.
2. If the change is additive, retain the schema and prove old fixtures still
   deserialize. If it changes meaning/type/nullability, bump the schema and add
   an explicit migration or a clear unsupported-version error.
3. Update this inventory, command help, and release migration notes in the same
   change.
4. Test exact stream ownership, status, ANSI absence, redaction, bounds, and
   behavior on malformed/unknown versions—not only happy-path deserialization.
5. Run `cargo test --test api_compat --test cli` plus the relevant module tests.
   Release qualification also runs the Python live-contract and harness gates.

Current compatibility fixtures cover the v0.5 and v0.6 unversioned
`status`/`suggest` shapes plus every legacy inspection-family migration above,
their versioned equivalents, a v1 structured error, and a v1 persisted task
record. Add fixtures before changing any of those shapes.
