# Aishe interactive UX and reliability milestone

Status: implementation contract
Owner: Aishe maintainers
Baseline: v0.2.30
Last updated: 2026-07-29

## 1. Purpose

Aishe already has a capable shell, provider abstraction, safety gate, tool loop,
history, project context, audit log, and broad automated test suite. The product
does not yet expose those capabilities as a coherent user journey. Installation
ends at a version string, first-run setup is a one-shot text questionnaire,
provider compatibility is discovered only after a request fails, configuration
is split across raw TOML and several one-field commands, and advanced recovery
features are difficult to discover.

This milestone makes Aishe feel like a complete product from installation
through long-running daily use. It implements ten connected improvements:

1. A resumable interactive setup experience.
2. Provider/model discovery and live capability validation.
3. A guided first-session tour.
4. Repair-capable diagnostics and redacted support bundles.
5. An interactive settings experience with configuration provenance.
6. Understandable safety profiles and autonomous-mode readiness checks.
7. Proactive command and provider failure recovery.
8. Durable, resumable AI task sessions.
9. Context, privacy, token, and cost previews.
10. A dedicated first-run, upgrade, and UX reliability test gate.
11. Route-aware input highlighting for command/natural-language collisions.

The implementation is complete only when every acceptance criterion in this
document has direct automated or recorded manual evidence. A passing build by
itself is not completion.

## 2. Product principles and reasoning

### 2.1 Preserve the shell

Aishe wraps the user's real zsh in a PTY. Setup and settings may use a richer
standalone terminal UI, but the shell front end must remain a real zsh. The
milestone must not replace zsh line editing, completion, highlighting, job
control, plugins, aliases, functions, or history.

### 2.2 Preserve user state

An install, update, repair, setup rerun, or settings change must never delete
history, config, trust, audit, undo, session, or capability data implicitly.
Destructive resets require an explicit target, a confirmation, and a backup
when the target is configuration. Writes use same-directory temporary files and
atomic rename.

### 2.3 Verify before declaring success

Saving a provider name and model is not a successful setup. A successful setup
must distinguish:

- endpoint reachability;
- credential presence;
- credential acceptance;
- model availability;
- generation compatibility;
- structured-output compatibility;
- function-tool compatibility;
- streaming compatibility;
- endpoint transport and token-limit parameter compatibility.

Offline setup is permitted, but it must finish as "saved, not verified" and say
exactly what remains unverified.

### 2.4 Keep credentials out of config and diagnostics

Environment variables remain the default credential mechanism. Setup may read a
key for a one-process validation but must not echo it, serialize it into
`config.toml`, write it into a support bundle, or put it into shell history.
Local loopback providers must work without a fake or dummy API key. Future OS
keychain support may be added separately; it is not required here.

### 2.5 Prefer capability evidence over model-name guesses

Provider behavior is described by a typed capability record keyed by endpoint,
model, and transport. Known-safe defaults may reduce probes, but a cached
successful request is stronger evidence than a model-name pattern. Rejected
parameters are learned once and persisted so every short-lived PTY child does
not repeat the same failed request.

### 2.6 Preserve privacy in the Responses API

Official OpenAI reasoning and tool calls use `/v1/responses`. Aishe sends
`store: false` unless a future explicit, documented setting changes that
posture. Tool continuations replay provider output items, including encrypted
reasoning items when returned. Durable checkpoints redact plaintext while
retaining provider-generated item/call IDs and opaque encrypted reasoning state
exactly in a private file; support bundles never include task contents. Chat
Completions remains available for compatible third-party endpoints. GPT-5.6
tool use with reasoning must not be routed through Chat Completions.

### 2.7 Progressive disclosure

The default setup asks only decisions needed to reach a verified first success.
Advanced endpoint, sandbox, privacy, logging, fallback, semantic-history, MCP,
and cost controls are available from an Advanced step and from Settings.

### 2.8 One source of truth

Setup, Settings, Doctor, CLI setters, and runtime provider construction use the
same validation, capability, configuration provenance, and atomic persistence
modules. They must not maintain separate provider preset lists or compatibility
rules.

## 3. User journeys

### 3.1 Clean installation

```text
install
  -> installer reports binary and optional dependencies
  -> installer prints exactly: Run `aishe setup`
  -> setup detects OS, shell, paths, and existing environment variables
  -> user chooses service/model/profile
  -> Aishe verifies endpoint/auth/model/capabilities
  -> user reviews a redacted configuration diff
  -> Aishe saves atomically
  -> optional tour proves command, AI, failure recovery, and undo
  -> Aishe starts the real zsh shell
```

The pipe-friendly installer must not open a full-screen UI automatically.
`install.sh --setup` may launch setup when stdin and stdout are terminals.

### 3.2 Existing installation

`aishe setup` detects current config and offers:

- Verify current setup
- Change provider or model
- Change safety profile
- Configure shell/history
- Configure privacy/context
- Advanced settings
- Run the tour
- Exit without changes

No existing value changes merely because the wizard is opened.

### 3.3 Interrupted setup

Setup saves a non-secret draft in the Aishe data directory after each completed
step. On restart it offers Resume, Start over, or Discard draft. Discarding the
draft does not touch the active config. Successful apply removes the draft.

### 3.4 Upgrade

An upgrade preserves the exact config and every data file. On first execution,
schema migration:

1. reads the existing config;
2. validates the source schema;
3. creates a timestamped backup;
4. migrates only known fields;
5. writes atomically;
6. reports the backup path once;
7. never resets unrelated settings to a new default.

### 3.5 Failed command

After a nonzero command, Aishe prints one concise hint:

```text
aishe: exit 1 — `?` explain · Ctrl-X Ctrl-F suggest a fix · `!<cmd>` force shell
```

The hint is suppressed for interrupts and when explicitly disabled. Asking for
a fix includes captured output only when it already exists or a read-only
command can be safely rerun.

### 3.6 Long-running task

Every AI task has a durable ID and status. A task is checkpointed after the user
request, every model response, every tool result, and every approval decision.
If the process or SSH connection ends, `aishe resume [ID]` continues from the
last complete checkpoint.

### 3.7 Ambiguous command names

Some ordinary question words are also real commands or Zsh builtins (`what` on
macOS, `where` in Zsh, and `who` on Unix). Highlighting and Enter-time routing
must use the same conservative question grammar, not only the first token. A
bare `who`, `where ls`, or `what /bin/ls` remains shell input; `who am I`,
`where is the config`, and `what is the capital of France` switch to the Aishe
natural-language style while being typed and route to the model on Enter. `!`
always forces the shell and `?` always forces natural language.

## 4. Architecture

### 4.1 New modules

The exact file split may change if implementation evidence shows a better
boundary, but responsibilities must remain separated:

| Module | Responsibility |
| --- | --- |
| `setup` | Setup state machine, draft, interactive/noninteractive drivers |
| `promptui` | Arrow-key menus, validated text input, back/cancel/help |
| `provider_catalog` | Services, defaults, auth requirements, transport |
| `capabilities` | Model listing, live probe, cache, compatibility policy |
| `diagnostics` | Structured checks, safe fixes, JSON and text rendering |
| `settings` | Effective/user/project provenance and interactive editing |
| `profiles` | Safety-profile definitions, application, readiness |
| `tasks` | Durable task metadata, checkpointing, listing, resume |

The existing `config`, `providers`, `session`, `context`, `fix`, and `pty`
modules remain focused on runtime behavior.

### 4.2 Configuration schema

Add a top-level integer `version`. Missing means schema 1. The current schema
for this milestone is schema 2.

Add these user-visible settings:

```toml
version = 2

[aishe]
safety_profile = "conservative" # conservative|balanced|autonomous|custom
failure_hints = true
reasoning_effort = "auto"       # auto|none|low|medium|high|xhigh|max
context_exclude = []            # history|project_context|project_tasks|host_profile
status_line = true
status_line_position = "right" # right|below|off
status_line_items = ["model", "mode", "session_cost", "requests"]

[providers.openai]
transport = "auto"              # auto|responses|chat
auth_required = true            # false for local/unauthenticated endpoints
```

Migration rules:

- Missing fields take backward-compatible defaults.
- An existing `mode` and safety configuration maps to `custom`; migration must
  not overwrite those values by applying a profile.
- Official OpenAI `transport = "auto"` resolves to Responses.
- Loopback OpenAI-compatible URLs default `auth_required` to false when the
  field is absent.
- Non-loopback services default it to true.
- Existing configs are not rewritten unless a schema migration or explicit
  user change is required.

### 4.3 Configuration provenance

Each effective setting can report:

- compiled default;
- user config;
- environment override;
- command-line override;
- project config;
- whether a project value is deferred pending trust.

Provenance is computed once and used by Settings, Doctor, and
`aishe config --effective`.

### 4.4 Provider catalog

One provider catalog contains:

- stable service key and display name;
- provider family;
- default base URL;
- default credential environment variable;
- default transport;
- whether authentication is normally required;
- a conservative default model;
- whether model listing is supported;
- setup help URL or short help text.

The catalog includes Anthropic, OpenAI, Groq, OpenRouter, Together, Ollama, and
Custom. Model lists from the endpoint supersede the static default. Manual model
entry is always available.

### 4.5 Capability cache

Cache path:

```text
<data>/aishe/capabilities/<endpoint-model-transport-hash>.json
```

Record:

- schema version;
- normalized endpoint hash, never credential;
- model;
- transport;
- checked timestamp;
- reachability/auth/model-list result;
- text, structured output, tools, streaming;
- accepted output-token parameter;
- accepted reasoning effort/shape;
- last classified failure.

Successful capability evidence has a seven-day TTL. A 400 compatibility error
at runtime invalidates only the relevant capability and triggers one negotiated
retry. `aishe doctor --fix` can clear stale cache safely.

### 4.6 Error taxonomy

Extend provider errors with a stable kind:

- `missing_credential`
- `invalid_credential`
- `permission`
- `model_not_found`
- `unsupported_parameter`
- `unsupported_tools`
- `unsupported_format`
- `rate_limited`
- `quota`
- `timeout`
- `network`
- `server`
- `malformed_response`
- `unknown`

Every user-facing provider error contains:

1. a concise cause;
2. the affected provider/model;
3. a safe next action or exact command;
4. the original status and provider message in a secondary detail line.

Secrets and authorization headers are redacted before display or persistence.

## 5. Workstream 1 — interactive, resumable setup

### 5.1 Commands

```text
aishe setup
aishe setup --resume
aishe setup --restart
aishe setup --verify
aishe setup --non-interactive [flags]
```

`aishe` with no config presents a short welcome and enters the same setup state
machine. It does not use a separate legacy wizard.

### 5.2 Interaction requirements

- Arrow Up/Down and `j`/`k` move.
- Enter selects.
- `b` goes back where meaningful.
- `?` shows contextual help.
- Esc or `q` requests cancellation.
- Text input validates and retries; invalid input never silently chooses a
  default.
- Ctrl-C exits without modifying active configuration and leaves a resumable
  draft.
- Non-TTY invocation never blocks and never writes a default config implicitly.
  It exits with an actionable `aishe setup --non-interactive ...` message unless
  another command does not require config.
- Terminal width and `NO_COLOR` are respected.

### 5.3 Apply semantics

The final page shows:

- provider/service/model/transport;
- credential environment variable and presence, never value;
- validation status;
- safety profile;
- shell/history path;
- privacy/logging state;
- config and data paths;
- redacted before/after diff.

Apply creates a backup when config exists, writes atomically, reloads the result,
and runs a local validation pass before reporting success.

### 5.4 Acceptance criteria

- **SETUP-01** Setup is safely rerunnable with an existing config.
- **SETUP-02** Invalid menu/text input retries and preserves the current step.
- **SETUP-03** Back, cancel, Ctrl-C, resume, restart, and draft cleanup work.
- **SETUP-04** Opening/canceling setup causes zero active-config changes.
- **SETUP-05** Non-TTY first run does not silently write defaults.
- **SETUP-06** Apply shows a diff, creates a backup, writes atomically, and
  preserves unrelated settings.
- **SETUP-07** No secret is written or echoed.

## 6. Workstream 2 — provider discovery and capability validation

### 6.1 Commands

```text
aishe models [--provider NAME] [--refresh] [--json]
aishe provider test [--live] [--json]
aishe doctor --probe
aishe doctor --live
```

`--probe` remains token-free. `--live` makes one or more minimal generation
requests and clearly says it may consume tokens before an interactive run.

### 6.2 OpenAI behavior

- Official OpenAI uses Responses for text, structured output, streaming, and
  tools.
- Responses requests include `store: false`.
- Responses uses `max_output_tokens`.
- Reasoning uses `reasoning: { effort }`.
- Tool continuation preserves all returned provider items and call IDs.
- With `store: false`, encrypted reasoning returned by the provider is replayed
  exactly. Durable checkpoints preserve only the opaque encrypted state and
  provider routing IDs verbatim; plaintext fields remain redacted.
- A custom endpoint may explicitly choose Responses.
- Chat Completions uses its accepted output-token parameter and persists the
  learned spelling.
- GPT-5.6 plus Chat Completions plus tools either uses explicitly configured
  effective reasoning `none`, or fails preflight with an instruction to use
  Responses. It must not repeatedly send a known-invalid request.

### 6.3 Local providers

Loopback and explicitly unauthenticated providers:

- construct without an environment key;
- omit Authorization when no key exists;
- still use a configured key when present;
- pass Doctor and Setup credential checks as "not required."

### 6.4 Acceptance criteria

- **CAP-01** Model listing parses OpenAI-compatible `data[].id` responses.
- **CAP-02** Manual model entry works when listing is unavailable.
- **CAP-03** Live validation separately reports auth, model, text, tools,
  structured output, and streaming.
- **CAP-04** Official OpenAI Responses requests are private by default and use
  correct request/response shapes.
- **CAP-05** Learned compatibility survives new Aishe processes and is scoped to
  endpoint/model/transport.
- **CAP-06** Local unauthenticated providers require no dummy key.
- **CAP-07** Errors are classified and actionable.

## 7. Workstream 3 — guided tour

### 7.1 Command

```text
aishe tour [--restart] [--non-interactive]
```

The tour uses a temporary directory under the data directory and never mutates
the invocation directory. It teaches:

1. normal shell command passthrough;
2. natural-language routing;
3. suggest review/edit/cancel;
4. failed-command explanation/fix;
5. a safe file change and undo;
6. modes and safety profile;
7. config/history/log/task locations.

The AI step uses the configured live provider only after verification. If the
provider is unavailable, the rest of the tour remains usable and records the AI
step as skipped.

Tour progress is resumable. Completion is stored as a small versioned marker so
future releases can offer only newly added lessons.

### 7.2 Acceptance criteria

- **TOUR-01** Tour never changes files outside its temporary workspace.
- **TOUR-02** Tour can resume, restart, skip a lesson, and exit.
- **TOUR-03** Fake-provider automation covers the whole tour.
- **TOUR-04** Live-provider failure degrades to a useful offline tour.
- **TOUR-05** Undo lesson proves the created file is restored/removed.

## 8. Workstream 4 — Doctor fixes and support bundles

### 8.1 Commands

```text
aishe doctor [--probe] [--live] [--json]
aishe doctor --fix
aishe doctor --bundle PATH
```

Diagnostics return structured checks with ID, severity, summary, details,
fixability, and changed paths. Text and JSON render from the same objects.

Safe automatic fixes:

- create missing config/data directories;
- repair private file/directory permissions;
- restore the configured Aishe history fallback without truncating it;
- migrate known legacy config/data locations;
- remove stale capability cache entries;
- create a missing default config only through the setup state machine;
- print, but do not automatically execute, privileged package commands;
- offer a previewed shell-integration edit only in an interactive terminal.

Support bundle is a redacted JSON document containing version, platform,
diagnostic results, config with key names but no key values, resolved paths,
capabilities, and recent classified errors. It excludes prompts, command
history, file contents, audit content, environment values, and credentials by
default.

### 8.2 Acceptance criteria

- **DOC-01** Text and JSON diagnostics contain the same check IDs/results.
- **DOC-02** `--fix` is idempotent and reports every changed path.
- **DOC-03** Fix never installs packages or edits shell startup files without
  explicit interactive consent.
- **DOC-04** Support bundles pass the secret-redaction corpus.
- **DOC-05** Missing credential names the exact environment variable.
- **DOC-06** Bubblewrap output explains core versus optional capabilities and
  gives the platform package command when known.

## 9. Workstream 5 — interactive settings and provenance

### 9.1 Commands

```text
aishe settings
aishe settings --json
aishe config
aishe config --effective
aishe config --json
```

Settings uses the setup UI primitives and edits a draft. Sections are Provider,
Shell & history, Mode & safety, Context & privacy, Cost & logging, and Advanced.
Each field shows effective value, source, validation, help, and reset behavior.

Changing provider is transactional: service, endpoint, auth, model, transport,
and capability validation are reviewed together. It must not leave an OpenAI
model under Anthropic or retain an incompatible endpoint accidentally.

### 9.2 Pricing

When the selected model has no built-in or configured price, Setup and Settings
offer:

- Enter input/output USD per million tokens
- Leave price unknown
- Return to model selection

No price is inferred from a related model name. Unknown remains explicit because
an invented rate would make budgets and cost reporting dangerously misleading.

Pricing is also manageable without editing TOML:

```text
aishe price list
aishe price set MODEL --input USD --output USD
aishe price remove MODEL
```

Input/output rates must be finite, non-negative numbers. Changes use the shared
atomic config writer and show the exact model key affected.

### 9.3 Live shell status line

The zsh PTY has a configurable, Codex-style status line. Placement is `right`
(the upgrade-safe default, preserving the current RPROMPT), `below` (a secondary
line adjacent to the next input prompt, suited to narrow terminals), or `off`.
Supported items are:

- `model`
- `mode`
- `last_tokens`
- `last_cost`
- `session_tokens`
- `session_cost`
- `requests`

Setup and Settings show a live preview of the chosen placement and fields before
applying it. The default is compact: model, mode, session cost, and request count. Unknown
pricing displays `cost n/a`; it never hides usage. A status file shared by the
short-lived AI child processes and the parent zsh is updated atomically after
each provider call, so the next prompt reflects the latest call without an
extra process or API request. `NO_COLOR`, narrow terminals, and `status_line =
false` are respected.

### 9.4 Acceptance criteria

- **SET-01** Settings cancel makes no changes.
- **SET-02** Provider change is one validated transaction.
- **SET-03** Effective values identify their source.
- **SET-04** Individual fields and sections can reset without deleting config.
- **SET-05** Existing `mode`, `model`, and `provider` commands delegate to the
  same validation/persistence layer.
- **PRICE-01** Unknown model pricing is offered during setup/settings and can be
  intentionally left unknown.
- **PRICE-02** Price list/set/remove validates values and persists exact model
  keys atomically.
- **STATUS-01** The right prompt updates last/session totals after each call.
- **STATUS-02** Every supported item can be enabled, disabled, and reordered.
- **STATUS-03** Mixed priced/unpriced models and mid-session model changes are
  represented accurately.
- **STATUS-04** Status updates add no provider request and no prompt-time child
  process.
- **STATUS-05** Right, below, and off placements render correctly in wide and
  narrow PTYs, with a setup/settings preview.

## 10. Workstream 6 — safety profiles and readiness

### 10.1 Profiles

Profiles are explicit bundles:

| Setting | Conservative | Balanced | Autonomous |
| --- | --- | --- | --- |
| Mode | suggest | auto | yolo |
| Confirm tier | all | writes | dangerous |
| Plan before tools | on | on | off |
| Preview file edits | on | on | off |
| Sandbox | on | on | on |
| Backend | bwrap if available, else policy | same | bwrap required for Ready |
| Max iterations | 10 | 15 | 25 |

Budget is shown during profile choice but never silently invented or changed.
Changing any profile-owned field later marks the profile `custom`.

### 10.2 Commands

```text
aishe profile [conservative|balanced|autonomous|custom]
aishe readiness [--json]
```

Autonomous readiness checks provider tools, sandbox backend, working directory,
undo journal, redaction, iteration limit, confirmation tier, and optional
budget. A user may override a warning interactively, but Aishe records and shows
that the profile is not fully ready.

### 10.3 Acceptance criteria

- **SAFE-01** Profile mappings are centralized and unit tested.
- **SAFE-02** Applying a profile shows every changed value.
- **SAFE-03** Manual changes mark the profile custom.
- **SAFE-04** Autonomous readiness fails clearly without tool-capable provider
  or requested bwrap isolation.
- **SAFE-05** Existing users migrate to custom without behavior changes.

## 11. Workstream 7 — proactive failure recovery

### 11.1 Shell failures

The zsh PTY integration records command, exit code, and bounded captured output.
On a nonzero non-interrupt exit it prints the discoverability hint. The existing
`?` and Ctrl-X Ctrl-F flows use the same `FailureContext`.

Never automatically rerun a command merely to diagnose it unless the existing
read-only safety predicate approves it and the user explicitly invokes the fix
flow.

### 11.2 Provider failures

Actionable examples:

```text
aishe: OPENAI_API_KEY is not set; LLM features are disabled.
Next: export OPENAI_API_KEY=... and run `aishe provider test --live`
```

```text
aishe: gpt-5.6-luna cannot use reasoning tools through Chat Completions.
Next: run `aishe settings` and choose the Responses transport.
```

Rate-limit and transient errors show retry count and eventual next action.
Authentication and unsupported-parameter errors are never retried as transient.

### 11.3 Acceptance criteria

- **FAIL-01** Failure hint appears once after ordinary nonzero commands.
- **FAIL-02** Hint is absent for success, Ctrl-C, and disabled configuration.
- **FAIL-03** Fix and explain use one bounded, redacted context.
- **FAIL-04** Provider error kinds map to deterministic messages/actions.
- **FAIL-05** Secret-bearing provider messages are redacted.

## 12. Workstream 8 — durable task sessions

### 12.1 Commands

```text
aishe sessions [--json]
aishe session show ID [--json]
aishe session rename ID NAME
aishe session delete ID
aishe resume [ID]
```

Delete is explicit and affects only the selected task record. Completed,
failed, interrupted, and active statuses are distinct.

### 12.2 Storage

```text
<data>/aishe/tasks/<id>.json
```

Versioned task records include:

- ID, optional name, timestamps, status, mode, model/provider, cwd;
- original objective;
- canonical messages;
- provider-native continuation items where required;
- completed tool calls and results;
- pending approval or next phase;
- compact usage summary;
- last classified error.

No environment snapshot or credential is stored. Writes are atomic. File
permissions are user-only.

### 12.3 Resume rules

- Resume defaults to the most recently interrupted task.
- Missing original cwd prompts for a replacement and never creates it silently.
- Already completed tool calls are not repeated.
- A pending side-effecting tool call requires confirmation after resume.
- Changed provider/model triggers a visible compatibility warning.
- Resume can continue with canonical messages when provider-native items cannot
  be reused on a different provider.

### 12.4 Acceptance criteria

- **TASK-01** Task is checkpointed after every state transition.
- **TASK-02** Kill/restart resumes without repeating a completed tool.
- **TASK-03** Lists, show, rename, delete, and default resume work.
- **TASK-04** Records contain no credentials or broad environment values.
- **TASK-05** Cross-provider resume degrades safely to canonical history.
- **TASK-06** Upgrades preserve and migrate task records.

## 13. Workstream 9 — context and privacy preview

### 13.1 Commands

```text
aishe context
aishe context --explain
aishe context --preview TEXT
aishe context --json
aishe context --exclude SECTION
aishe context --include SECTION
```

The context builder returns named sections before rendering. Preview shows:

- included and excluded sections;
- source path where relevant;
- characters and estimated tokens per section;
- redaction count;
- total estimated tokens;
- provider/model;
- estimated input cost when pricing is known;
- project configuration/command/skill sources that will apply.

Exclusions apply to history, project context, project task surface, and host
profile. Core cwd/shell facts remain visible and are labeled required.

### 13.2 Acceptance criteria

- **CTX-01** Runtime context and preview render from the same section objects.
- **CTX-02** Token totals are deterministic and labeled estimates.
- **CTX-03** Preview never exposes text removed by redaction.
- **CTX-04** Include/exclude persists atomically and shows provenance.
- **CTX-05** JSON output is stable enough for admin validation.

## 14. Workstream 10 — UX reliability and upgrade gate

### 14.1 Refactoring requirements

- Main CLI parsing remains in `main.rs`; feature logic moves to library modules.
- Setup is a state machine independent of terminal rendering.
- Diagnostics are structured data independent of text rendering.
- Provider catalog and compatibility rules have one owner.
- Atomic JSON/TOML persistence shares a tested helper.
- New on-disk formats carry a schema version.

### 14.2 Automated test layers

1. Unit tests for state transitions, parsing, profiles, diagnostics, migrations,
   error classification, context sections, and task checkpoints.
2. HTTP mock tests for model listing and every capability/error class.
3. CLI integration tests for all new commands and JSON contracts.
4. PTY tests for arrow menus, invalid input, back/cancel/resume, Ctrl-C,
   failure hints, and tour.
5. Upgrade fixtures from representative old configs and data directories.
6. Installer tests for fresh install versus upgrade and state preservation.
7. Admin validation additions for help/man/completion/docs/config parity.
8. Optional real-model tests gated by environment variables.

### 14.3 Required upgrade fixtures

- schema-1 default config;
- customized OpenAI GPT-5.6 config;
- customized Anthropic config;
- project overlay and trust store;
- shared history plus concurrent-session entries;
- audit, undo, semantic index, capability cache, and task record;
- malformed config with recoverable backup;
- legacy `llmsh` paths.

For every fixture, hash all unrelated data files before and after migration and
assert equality.

### 14.4 Acceptance criteria

- **TEST-01** Existing test suite remains green.
- **TEST-02** Every criterion ID in this document maps to at least one automated
  test or an explicit SSH manual test.
- **TEST-03** Clean setup PTY tests cover every branch and invalid input.
- **TEST-04** Upgrade tests prove config/history/data preservation.
- **TEST-05** Real-model tests cover text, structured suggest, streaming, and a
  yolo function-tool round trip. Explicit scripting is never signal-truncated
  by the native shell-hook responsiveness alarm; native hooks remain bounded by
  the configurable `hook_timeout_secs` budget, and exhausted provider failures
  return a truthful non-zero scripting result.
- **TEST-06** Documentation commands and examples are executable or validated.

## 15. Documentation changes

Update:

- README quickstart;
- installation;
- getting started;
- providers;
- configuration and precedence;
- modes and safety;
- shell integration/history;
- troubleshooting;
- command reference;
- architecture;
- changelog.

Remove instructions that tell users to delete config to rerun setup. Correct the
malformed-config behavior description. Explain that official OpenAI uses
Responses, custom compatible services default to Chat Completions, and local
services can be unauthenticated.

## 16. Installer and upgrade behavior

`install.sh` must:

- distinguish fresh install and replacement;
- report old and new versions;
- never touch config/data/history;
- make a best-effort pre/post state inventory without reading contents;
- print `aishe setup` for a fresh install;
- print `aishe doctor` for an upgrade;
- support `--setup` only on a TTY;
- continue treating bubblewrap as optional for core shell use while clearly
  explaining which safety features require it.

Package-manager installations cannot run interactive setup automatically.

## 17. Local validation gate

Before Linux deployment:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
python3 tests/admin_validation.py
python3 tests/pty_smoke.py
python3 tests/pty_scenarios.py
python3 tests/pty_signals.py
python3 tests/zsh_features.py
python3 tests/pty_fuzz.py
```

Run other repository validation scripts documented by `tests/admin_validation.py`
when their prerequisites are available. Record skipped checks and reason.

## 18. SSH Linux validation gate

Target: the authorized Linux test node supplied by the user.

### 18.1 Safety

- Back up `/root/.config/aishe` and `/root/.local/share/aishe` before replacing
  the binary.
- Record file names, modes, sizes, and hashes without printing secret contents.
- Use a separate temporary config/data root for destructive clean-install tests.
- Do not put an API key into shell history, command output, config, or logs.
- Remove temporary test roots and binaries after evidence is collected; preserve
  the user's real config/data and installed final binary.

### 18.2 Scenarios

1. Clean non-TTY first run does not create defaults.
2. Clean interactive setup with fake provider.
3. Cancel and resume setup.
4. Existing-config settings change/cancel.
5. `doctor`, `--json`, `--probe`, `--fix`, and bundle redaction.
6. History across two simultaneous PTY sessions and restart.
7. Upgrade over v0.2.30; compare hashes of history and unrelated data.
8. Local unauthenticated OpenAI-compatible mock.
9. Real GPT-5.6 Luna text answer.
10. Real GPT-5.6 Luna structured suggest.
11. Real GPT-5.6 Luna streaming.
12. Real GPT-5.6 Luna yolo function-tool round trip in a temporary directory.
13. Provider/model choice persists without repeated compatibility fallback.
14. Failed Linux command displays recovery hint and fix flow.
15. Context preview redacts a seeded fake secret.
16. Interrupt a multi-tool task, list it, and resume without repeating the
    completed action.
17. Guided tour with fake provider and, optionally, live provider.
18. Autonomous readiness with and without bubblewrap.

### 18.3 Evidence

Store a redacted test transcript under a temporary directory on the node, copy
it to a local ignored test-artifacts directory, inspect it, then remove the
remote temporary transcript. Evidence includes command, exit code, relevant
stdout/stderr, before/after hashes, and version/build metadata.

### 18.4 Release-candidate evidence (2026-07-29)

The v0.3.0 candidate was compiled natively on the authorized Ubuntu x86_64 node
with Rust 1.88 and installed over v0.2.30 only after a private backup of the
existing config and data roots.

- The original shared history and undo journal retained their exact SHA-256
  hashes through binary replacement, schema migration, and repeated Doctor
  repair runs. The schema-1 config was backed up byte-for-byte before the
  atomic schema-2 rewrite.
- All config/data directories and regular files are now user-only. Recursive
  repair was idempotent and did not follow a seeded symlink.
- Clean non-TTY behavior, explicit non-interactive setup, the complete setup and
  settings PTY, three status-line layouts, 33 routing/highlighting scenarios,
  339 generated PTY cases, 44 zsh feature cases, seven signal/terminal cases,
  concurrent/restarted history, failure hints, durable task resume, guided tour,
  unauthenticated loopback provider, installer corruption rejection plus upgrade
  preservation, and bubblewrap readiness all passed on Linux.
- The final candidate specifically proved that `what`, `where`, and `who`
  command-name collisions change from command green to the AI route/color only
  after the whole buffer becomes a question; bare and ordinary command forms
  remain shell green.
- A fresh process consumed bounded persistent history as model context while
  redacting a seeded fake secret. Doctor reported the actual source schema,
  named the missing `$OPENAI_API_KEY`, skipped redundant anonymous live probes,
  produced a redacted bundle, and made no further changes on its second `--fix`.
- The right-prompt model field remained literal and control-safe with zsh
  `PROMPT_SUBST` enabled; a model name containing command substitution and a
  terminal erase sequence neither executed nor repainted the terminal.
- The node's 1.9 GiB temporary filesystem could not hold a second full debug
  test graph alongside the Rust toolchain and optimized artifact. The optimized
  binary did build natively; the identical source passed all 420 Rust tests,
  warning-free Clippy, the 455-check admin harness, and every deterministic
  script locally. Linux behavior was then exercised through the optimized
  native binary by the suites above.
- Scenarios 9–13 passed against GPT-5.6 Luna through the Responses transport:
  text, structured output, streaming, tool calling, and capability-cache
  persistence all succeeded without compatibility fallback. The labelled
  classification corpus passed 20/20 with zero contract breaches, and the
  adversarial real-model corpus passed 280/280.
- The complete paid matrix passed all seven gates in 1,331 seconds. Its three
  redacted reports were copied to the ignored local test-artifacts directory
  and independently scanned. The credential was supplied only through hidden
  standard input to the isolated test process, was unset when the process
  exited, and added nothing to command arguments, config, history, logs,
  reports, evidence, or the repository. No API-key file remained on the node.
- The final credential audit found four pre-existing key export records from
  earlier ad hoc testing in both the 53-line shared history and its 06:00 UTC
  safety backup. Only the token text was atomically replaced with a redaction
  marker in both copies; every line and unrelated history entry was retained.
  A recursive rescan of config, data, shell history, and the backup was clean.
- All 18 SSH scenarios passed. The exact-code GitHub Actions run passed Ubuntu,
  macOS, the Rust 1.88 minimum-version build, and the full Linux PTY job.

## 19. Requirement-to-test matrix

The implementation maintains one row per criterion. Rust names below are exact
test names; `script:case` identifies an exact deterministic harness case. SSH
scenario numbers refer to section 18.2 and are recorded as pass/fail after the
release-candidate run.

| Criterion | Exact automated evidence | SSH |
| --- | --- | --- |
| SETUP-01 | `cli::noninteractive_setup_is_rerunnable_and_preserves_existing_fields` | 4 |
| SETUP-02 | `setup_pty.py:full setup flow, invalid input, back, pricing, status, apply` | 2 |
| SETUP-03 | `setup_pty.py:cancel/Ctrl-C preserve config and setup draft resumes` | 3 |
| SETUP-04 | `setup_pty.py:cancel_preserves_active_config` | 4 |
| SETUP-05 | `cli::missing_config_in_non_tty_mode_is_actionable_and_does_not_write_defaults` | 1 |
| SETUP-06 | `config::write_atomic_writes_contents_and_leaves_no_tmp`; `setup_pty.py:full setup flow` | 2, 4 |
| SETUP-07 | `setup::draft_contains_no_environment_values`; `cli::doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets` | 2 |
| CAP-01 | `cli::provider_test_and_model_listing_support_local_unauthenticated_endpoints`; `provider_unauthenticated.py` | 8 |
| CAP-02 | `setup_pty.py:full setup flow, invalid input, back, pricing, status, apply` | 2 |
| CAP-03 | `capabilities::verified_requires_every_live_check`; `cli::provider_test_and_model_listing_support_local_unauthenticated_endpoints` | 9–12 |
| CAP-04 | `providers::openai_compat::official_openai_uses_responses_with_max_output_tokens`; `responses_reasoning_uses_nested_effort_and_never_chat_field`; `responses_tool_request_and_reasoning_replay_use_native_items` | 9–12 |
| CAP-05 | `providers::openai_compat::token_limit_fallback_is_persisted_for_the_endpoint_and_model`; `compatibility_cache_is_scoped_to_endpoint_and_model` | 13 |
| CAP-06 | `provider_catalog::ollama_does_not_require_auth`; `provider_unauthenticated.py:unauthenticated loopback provider needs no dummy API key` | 8 |
| CAP-07 | `providers::recovery_tests::actionable_messages_are_classified_and_redacted` | 9–12 |
| TOUR-01 | `cli::noninteractive_tour_is_isolated_resumable_and_proves_undo` | 17 |
| TOUR-02 | `tour::state_roundtrip_preserves_resume_index`; `setup_pty.py:tour pause/resume/skip/restart/offline/undo flow` | 17 |
| TOUR-03 | `cli::noninteractive_tour_is_isolated_resumable_and_proves_undo`; `setup_pty.py:tour pause/resume/skip/restart/offline/undo flow` | 17 |
| TOUR-04 | `setup_pty.py:tour pause/resume/skip/restart/offline/undo flow` | 17 |
| TOUR-05 | `cli::noninteractive_tour_is_isolated_resumable_and_proves_undo`; `setup_pty.py:tour pause/resume/skip/restart/offline/undo flow` | 17 |
| DOC-01 | `diagnostics::text_uses_the_same_check_ids_as_json`; `cli::doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets` (including actual source-schema reporting) | 5 |
| DOC-02 | `cli::doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets` | 5 |
| DOC-03 | `cli::doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets` | 5 |
| DOC-04 | `diagnostics::redacted_config_keeps_key_name_but_not_secret_values`; `cli::doctor_json_fix_and_support_bundle_share_checks_and_redact_secrets` | 5 |
| DOC-05 | `cli::missing_openai_key_names_the_exact_environment_variable`; `capabilities::missing_required_credential_blocks_provider_network_checks` | 5 |
| DOC-06 | `cli::doctor_reports_environment`; `installer_upgrade.sh:bubblewrap optional messaging` | 5, 18 |
| SET-01 | `setup_pty.py:settings provider cancel is transactional` | 4 |
| SET-02 | `setup_pty.py:reviewed provider apply works` | 4 |
| SET-03 | `cli::effective_config_and_context_json_are_structured_and_content_free` | 4 |
| SET-04 | `settings::context_toggle_is_reversible`; `settings::section_resets_restore_only_the_selected_section` | 4 |
| SET-05 | `cli::settings_subcommands_show_and_persist` | 4 |
| PRICE-01 | `setup_pty.py:full setup flow, pricing`; `setup_pty.py:settings transactional` | 2, 9 |
| PRICE-02 | `cli::price_commands_persist_exact_model_rates_and_validate_values` | 9 |
| STATUS-01 | `statusline_pty.py:statusline right (180 columns)` | 9–11 |
| STATUS-02 | `settings::status_fields_have_stable_order`; `usagelog::status_metrics_are_selectable_and_preserve_unknown_cost` | 9–11 |
| STATUS-03 | `usagelog::status_file_preserves_configured_order_and_mixed_model_accounting` | 9–11, 13 |
| STATUS-04 | `statusline_pty.py:statusline placement and live metrics`; `statusline_pty.py:model text is inert with zsh PROMPT_SUBST enabled`; generated-hook `zsh -n` gate | 9–11 |
| STATUS-05 | `statusline_pty.py:statusline right`; `statusline below`; `statusline off` | 9–11 |
| SAFE-01 | `profiles::profiles_apply_exact_mappings_without_touching_budget` | 18 |
| SAFE-02 | `cli::profile_changes_are_transparent_and_readiness_json_is_stable`; `setup_pty.py:full setup flow` | 18 |
| SAFE-03 | `profiles::custom_changes_only_profile_marker` | 18 |
| SAFE-04 | `profiles::readiness_requires_validated_tools`; `cli::profile_changes_are_transparent_and_readiness_json_is_stable` | 18 |
| SAFE-05 | `config::pre_profile_config_loads_as_custom_without_changing_behavior`; schema-1 CLI/SSH fixtures | 7 |
| FAIL-01 | `pty_scenarios.py:failed command prints one recovery hint` | 14 |
| FAIL-02 | `pty_scenarios.py:failure hint is not repeated on prompt redraw`; `disabled failure hints stay quiet`; `successful command stays hint-free`; `Ctrl-C produced no recovery hint`; `pty_signals.py:Ctrl-C` | 14 |
| FAIL-03 | `fix::build_prompt_includes_context_when_present`; `fix::tail_caps_lines_and_chars`; `context::project_context_secrets_are_redacted_in_runtime_and_preview_metadata` | 14, 15 |
| FAIL-04 | `providers::recovery_tests::actionable_messages_are_classified_and_redacted` | 9–12 |
| FAIL-05 | `providers::recovery_tests::actionable_messages_are_classified_and_redacted` | 9–12 |
| TASK-01 | `modes::yolo_runs_tool_then_finishes`; `durable_task_resume.py:interrupted durable task` | 16 |
| TASK-02 | `durable_task_resume.py:interrupted durable task resumed without repeating its tool` | 16 |
| TASK-03 | `cli::durable_task_cli_lifecycle_is_private_and_redacted` | 16 |
| TASK-04 | `tasks::sanitization_strips_context_and_secrets`; `native_provider_items_preserve_protocol_state_but_redact_content` | 16 |
| TASK-05 | `durable_task_resume.py:provider mutation canonical fallback` | 16 |
| TASK-06 | `installer_upgrade.sh:config, history, tasks, and data preserved` | 7, 16 |
| CTX-01 | `context::preview_uses_runtime_sections_without_serializing_their_text` | 15 |
| CTX-02 | `context::preview_uses_runtime_sections_without_serializing_their_text` | 15 |
| CTX-03 | `context::project_context_secrets_are_redacted_in_runtime_and_preview_metadata`; `context::persisted_history_is_included_and_redacted_for_short_lived_children`; `executor::context_history_merges_memory_and_persistent_entries_newest_first` | 15 |
| CTX-04 | `settings::context_toggle_is_reversible`; `cli::effective_config_and_context_json_are_structured_and_content_free` | 15 |
| CTX-05 | `cli::effective_config_and_context_json_are_structured_and_content_free` | 15 |
| TEST-01 | `cargo test --all-targets` (420 tests); `admin_validation.py:455/455`; all deterministic PTY suites | all |
| TEST-02 | This exact-ID matrix | all |
| TEST-03 | `setup_pty.py:interactive setup PTY`; setup state/validation unit tests | 2–4 |
| TEST-04 | `installer_upgrade.sh:installer rejected corruption and preserved config, history, tasks, and data` | 7 |
| TEST-05 | `cli::explicit_suggest_is_not_cut_off_by_the_shell_hook_budget`; `cli::explicit_suggest_propagates_provider_failure`; `live_contract_test.py`; `live_release.py`; `real_model.py`; `real_fuzz.py`; real `doctor --live`; explicit GPT-5.6 Luna SSH calls | 9–12 |
| TEST-06 | `cli::man_emits_a_roff_page`; `cli::completions_emits_a_script`; `admin_validation.py:examples/config.toml parses` | 5 |

## 20. Implementation sequence

### Phase A — shared foundations

1. Add schema version and backward-compatible migrations.
2. Add provider catalog, transport/auth fields, error taxonomy, capability
   cache, model listing, and live validation.
3. Add reusable prompt UI and setup state machine.
4. Replace the legacy first-run wizard.
5. Add tests before wiring installer handoff.

### Phase B — configuration and diagnostics

1. Add provenance.
2. Add Settings.
3. Refactor Doctor into structured checks.
4. Add safe fixes, JSON, live checks, and support bundle.
5. Add safety profiles/readiness.

### Phase C — daily-use UX

1. Refactor context into sections and add preview/exclusions.
2. Add failure hints and provider recovery messages.
3. Add guided tour.
4. Add durable task checkpoints, session management, and resume.

### Phase D — release-quality validation

1. Complete PTY, HTTP, CLI, migration, installer, and documentation tests.
2. Run the local validation gate.
3. Build the Linux artifact reproducibly.
4. Back up and test on the authorized SSH node.
5. Correct failures and repeat both local and remote gates.
6. Audit every acceptance criterion against direct evidence.

## 21. Definition of done

This milestone is done only when:

- all ten workstreams are implemented, documented, and reachable from help;
- all acceptance IDs have direct evidence;
- config/history/data survive tested upgrades;
- setup reaches either a verified provider or an explicit unverified state;
- GPT-5.6 Luna succeeds for text, structured output, streaming, and tools through
  the correct API transport;
- local unauthenticated services work without dummy credentials;
- all local gates pass;
- all SSH scenarios pass on the authorized Linux node;
- no test credential appears in config, history, logs, bundles, transcripts, git
  diff, or process output;
- no known regression or undocumented skipped gate remains.
