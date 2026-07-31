> **Superseded (product truth as of 0.6.x):** account switching is `/connection`;
> `/model` lists models for the **active** connection only. See
> [Commands — connection vs model](../commands.md#connection-vs-model) and
> the root README. This document remains historical context.

# Named Provider Connections and Unified Model Switching

## Summary

Aishe must let an operator configure, distinguish, and switch among multiple credentials for the same AI provider as easily as they switch models. A connection is the durable unit that binds a provider, authentication method, credential or OAuth profile, endpoint, transport, and default model. The normal `/model` experience exposes those connections and their models in one concise picker, applies a choice to the current shell by default, and offers an explicit action to save it as the global default.

This work replaces the single active credential per provider and the single OAuth account per provider limitations while automatically migrating existing v0.5 configuration.

## Release Target

- Next minor release, recommended `v0.6.0`.
- Configuration schema version 6.
- Existing v0.5.3 configurations migrate automatically with a byte-for-byte backup and atomic replacement.

## Goals

- Support any number of named connections, including multiple connections to the same provider using different credentials.
- Support API-key, OAuth, unauthenticated local, and legacy automatically resolved authentication.
- Isolate OAuth state by user-provided profile label so two accounts for one provider can remain logged in simultaneously.
- Make connection and model switching obvious through `/model`, `/provider`, `/auth`, `aishe status`, setup, and settings.
- Make the default interaction session-scoped so experimentation does not silently rewrite durable configuration.
- Preserve exact connection/model selection across shell sessions and agent continuations.
- Attribute audit and usage records to a safe connection identity without exposing secrets.
- Preserve compatibility with existing configurations and unambiguous legacy commands.

## Non-goals

- Synchronizing credentials or OAuth tokens between machines.
- Importing every account from third-party credential managers automatically.
- Using `models.dev` as a model catalog.
- Exposing, logging, or embedding raw API keys or OAuth tokens in connection IDs, status, sessions, audit logs, or supervisor identifiers.
- Automatically selecting a cheaper credential, billing account, or model without an explicit operator policy.
- Releasing before the full qualification gates in this document pass.

## Configuration Model

Schema 6 introduces named connection records and top-level selection:

```toml
version = 6

[aishe]
connection = "openai-work"
connection_fallback = "openai-work"

[connections.openai-work]
provider = "openai"
label = "OpenAI work"
base_url = "https://api.openai.com/v1"
model = "gpt-5.4"
transport = "auto"

[connections.openai-work.auth]
type = "oauth"
profile = "work"

[connections.openai-personal]
provider = "openai"
label = "OpenAI personal"
base_url = "https://api.openai.com/v1"
model = "gpt-5.4"
transport = "auto"

[connections.openai-personal.auth]
type = "oauth"
profile = "personal"

[connections.openai-api]
provider = "openai"
label = "OpenAI API key"
base_url = "https://api.openai.com/v1"
model = "gpt-5.4"
transport = "auto"

[connections.openai-api.auth]
type = "api_key"
credential = "openai/work-api"
api_key_env = "OPENAI_API_KEY"
```

Authentication types are:

- `api_key`: resolves only the configured named credential or environment variable.
- `oauth`: resolves only the configured provider/profile OAuth store and ignores API-key environment variables.
- `none`: performs no credential lookup, for local or explicitly unauthenticated endpoints.
- `auto`: compatibility behavior used for migrated legacy configuration; retains the v0.5 key-first, then OAuth resolution order until the operator chooses an explicit method.

Connection IDs are stable machine identifiers. Labels are user-facing and may contain spaces. OAuth profile labels are user-provided, normalized safely for filesystem storage, and collision-checked.

## User Experience

### `/model`

Running `/model` without arguments opens a filterable connection-and-model picker:

```text
Select a connection and model

  OpenAI work       OAuth · work       gpt-5.4
  OpenAI personal   OAuth · personal   gpt-5.4
  OpenAI API key    API key · work-api gpt-5.4
  xAI production    OAuth · prod       grok-4
  Ollama local      No auth            qwen3:14b

Type to filter · Enter use in this shell · d save as default · Esc cancel
```

- Enter applies the connection and model atomically to the current shell.
- `d` saves the highlighted connection and model as the durable default and also applies it to the current shell.
- `/model <model>` changes the model on the current connection for the current shell.
- `/model <connection>/<model>` selects both when the input is unambiguous.
- A non-interactive form is available for scripts.
- Model discovery is performed per selected connection. When the pinned OpenCode runtime cannot enumerate OAuth models, Aishe presents configured and recently used models and permits a validated typed model ID.

### Related Commands

- `/provider` opens or reports connection selection; the term remains for familiarity even though the selected object is a connection.
- `/auth` reports the active connection's authentication state and offers profile-aware login/logout actions.
- `aishe connection list|add|edit|remove|use|show` manages durable connection records.
- `aishe auth login <provider> --profile <label>` logs into a specific isolated OAuth profile.
- `aishe auth logout <provider> --profile <label>` logs out only that profile.
- `aishe auth status [provider] [--profile <label>]` reports profile-aware status.
- `aishe models [--connection <id>]` lists models for one connection.
- Existing provider/model commands remain accepted when their target is unambiguous; ambiguity produces a concise list of exact choices.

### Status and Settings

`aishe status`, setup, settings, and the compact status line show:

- connection label and safe ID;
- provider and endpoint host;
- authentication type and profile/credential label;
- selected model and reasoning level;
- whether the choice is shell-local or the durable default;
- backend readiness, usage, and spend data already available to Aishe.

They never show secret material.

## Functional Requirements

1. Aishe loads, validates, serializes, and documents schema 6 named connections.
2. Migration from schema 5 creates deterministic connection IDs for configured providers, preserves the active provider/model/endpoint/transport/auth behavior, writes a byte-for-byte backup, and atomically replaces the configuration.
3. All connection selection flows use one central resolver that produces a fully resolved launch description.
4. Explicit `api_key` connections never consume OAuth state; explicit `oauth` connections never consume API-key state; `none` performs neither lookup; only `auto` uses compatibility precedence.
5. OAuth state is isolated in a complete profile-specific OpenCode HOME/XDG root, not merely separate `auth.json` files.
6. OAuth endpoint binding remains exact and fail-closed.
7. OAuth commands accept and display labels and allow at least two simultaneous profiles for one provider.
8. Backend supervisors are keyed by a non-secret launch identity that includes the connection and authentication profile. Multiple connection runtimes can coexist subject to a bounded instance limit, recommended default 8.
9. Credential or connection mutation invalidates affected supervisor instances without interrupting unrelated instances.
10. Backend sessions are keyed by shell, workspace, connection, model, mode, scope, and network policy. Switching back resumes the matching selection when valid.
11. Shell integration can apply connection and model atomically without rewriting durable configuration.
12. `/model` is a primary discoverable command and implements the picker and direct forms described above.
13. `/provider`, `/auth`, command completion, help, setup, settings, diagnostics, status, and documentation expose the connection model consistently.
14. Project overlays and policy may constrain allowed connection IDs/providers/models without embedding credentials.
15. Audit and usage events include safe connection ID/label, provider, auth type/profile label, model, and reasoning level where applicable.
16. Reasoning levels remain model-aware and are stored/selected with the effective connection/model settings.
17. Existing schema 5 installations and unambiguous v0.5 CLI workflows continue to operate after migration.

## User Stories and Acceptance Criteria

### 1. Connection schema

As an operator, I can define named connections so provider access is explicit and reusable.

- Schema 6 supports the fields and authentication variants in this PRD.
- Validation rejects missing providers, invalid IDs, duplicate normalized OAuth profile paths, incompatible auth fields, and unsafe endpoints.
- Serialization round-trips without secret expansion.
- Example configuration and reference documentation are updated.

### 2. Automatic schema migration

As an existing user, I am upgraded without losing configuration or behavior.

- Schema 5 migrates automatically on first write/load where migration is currently performed.
- The original file receives the existing byte-for-byte migration backup treatment.
- Generated connection IDs are deterministic and collision-safe.
- Active provider, model, endpoint, credential reference, environment fallback, transport, and authentication requirement are preserved.
- Migration and repeated-load idempotence tests pass.

### 3. Central connection and authentication resolver

As a maintainer, every launch follows identical credential rules.

- One resolver consumes a connection ID and returns provider/model/endpoint/transport/auth/runtime identity.
- Explicit auth types enforce strict separation.
- Resolution errors identify the connection and remediation without printing secrets.
- Unit tests cover keys, OAuth, none, auto, missing credentials, endpoint mismatch, and duplicate-provider connections.

### 4. Isolated labeled OAuth profiles

As an operator, I can stay logged into multiple accounts for one provider.

- Each provider/profile uses an isolated full OpenCode HOME/XDG root.
- Labels are normalized, collision-checked, and mapped reversibly for display.
- Token files and directories retain restrictive permissions.
- Two OpenAI profiles can be logged in, queried, and logged out independently.
- Exact endpoint binding tests remain green.

### 5. Profile-aware authentication CLI

As an operator, I can understand and manage the authentication behind a connection.

- Login, logout, and status accept `--profile` where relevant.
- Connection-aware shortcuts select the configured provider/profile automatically.
- Status distinguishes OAuth, API-key, none, expired/missing, and legacy auto states.
- Help and completion make the forms discoverable.

### 6. Connection-safe backend supervisors

As an operator, I can use different credentials concurrently without cross-account reuse or needless interruption.

- Supervisor identity includes a safe hash of launch-affecting connection/auth configuration.
- Runtimes for different connections can coexist up to the configured bounded limit.
- No raw secrets appear in keys, process arguments, logs, or filesystem names.
- Mutating one connection invalidates only its runtime.
- Concurrency and isolation tests cover same-provider/same-model/different-credential connections.

### 7. Connection/model-aware sessions

As an operator, switching selections preserves the right conversation and identity.

- Session storage schema records connection and model.
- Session lookup cannot reuse a session created with another connection or model.
- Switching back resumes the matching valid session.
- Older session records migrate or fail safe.

### 8. Per-shell selection overrides

As an operator, I can try another account or model without changing everyone else's default.

- The shell handoff stores connection and model atomically.
- Enter in the picker affects only the current shell.
- `d` explicitly updates the durable default.
- Nested and concurrent shells do not overwrite each other's selections.
- Reset returns the shell to the durable default.

### 9. Connection-specific model discovery

As an operator, I see models appropriate to the credential and endpoint I selected.

- Discovery is scoped to a connection and cache keys include safe connection identity.
- Static provider catalogs and backend enumeration are merged deterministically.
- OAuth enumeration limitations fall back to configured/recent/typed model IDs.
- Aishe does not depend on `models.dev`.

### 10. Unified `/model` picker

As an operator, I can switch account and model from one obvious interface.

- `/model` opens the filterable picker shown in this PRD on an interactive TTY.
- Rows show label, auth method/profile, and model with plain glyphs/text and no emoji.
- Enter, `d`, Esc, typing/filtering, direct arguments, cancellation, and non-TTY behavior are covered by PTY/CLI tests.
- Selection is atomic and errors leave the previous selection unchanged.

### 11. Provider and connection management commands

As an operator, I can create and manage connections without hand-editing TOML.

- `aishe connection` supports list, show, add, edit, remove, and use.
- `/provider` and legacy provider forms resolve unique connections and explain ambiguity.
- Destructive removal requires an explicit target and does not delete shared credentials unless separately requested.
- Command help and completion include all primary forms.

### 12. Setup, settings, status, diagnostics, and policy

As an operator, all Aishe surfaces describe the same active identity.

- Setup can create key, OAuth, none, and auto connections.
- Settings edits connection/auth/model/reasoning selections coherently.
- Status and compact disclosure show connection/model/auth and shell-local/default state.
- Diagnostics check the selected connection without mutating auth.
- Project policy can allow or deny safe connection identifiers.

### 13. Audit and usage attribution

As an administrator, I can attribute actions and spend to a safe connection identity.

- New events record connection ID/label, provider, auth type/profile label, model, and reasoning where available.
- Historical log readers remain compatible.
- Redaction tests prove keys and tokens cannot enter audit/history/usage outputs.
- `aishe status` and usage views aggregate or filter by connection without exposing secrets.

### 14. Multi-connection qualification

As a release manager, I have evidence the feature is safe under real shell and SSH use.

- Automated tests create two same-provider connections with distinct credentials and prove isolation.
- Manual bounded acceptance logs two OpenAI OAuth profiles in independently and proves selection/logout isolation.
- All quality, PTY, concurrency, soak, runtime-contract, and SSH gates below pass.
- The release is not cut until failures are fixed or explicitly documented as an external manual prerequisite.

## Technical Implementation Map

Likely implementation areas include:

- `src/config.rs`: schema, validation, migration, durable default.
- `src/auth.rs`, `src/credentials.rs`, `src/oauth.rs`: connection-aware credential resolution and profile isolation.
- `src/backend/opencode/config.rs`, `src/backend/runtime.rs`: profile-specific roots and launch configuration.
- `src/backend/supervisor.rs`: safe keyed supervisor pool and targeted invalidation.
- `src/backend/opencode/session.rs`, `src/session.rs`: connection/model session identity and migration.
- `src/main.rs`, `src/integration.rs`, `src/promptui.rs`, `src/settings.rs`, `src/setup.rs`: CLI and interactive picker flows.
- `src/capabilities.rs`, `src/diagnostics.rs`, `src/providers/mod.rs`: connection-aware discovery and status.
- `src/audit.rs`, `src/usagelog.rs`, `src/usage.rs`, `src/histlog.rs`: attribution and redaction.
- `src/policy.rs`, `src/overlay.rs`: connection constraints.
- Shell integration, completion, examples, README, configuration/provider/command documentation, and release notes.

## Quality and Release Gates

All gates are required:

### Rust quality

- `cargo fmt --all -- --check`
- formatting produces no diff
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- minimum supported Rust 1.88 build/test
- `cargo deny check`
- `cargo build --release --locked`
- `cargo package --locked --allow-dirty`

### Shell and PTY qualification

- all existing setup, mode, signal, scenario, status-line, zsh-feature, persistence, admin-validation, live-contract, and release PTY suites
- `/model` picker PTY coverage for filter, select, save-default, cancel, error rollback, and no-emoji output
- two concurrent shells selecting different same-provider connections

### Backend durability and isolation

- OpenCode runtime contract and host-scope suites
- 1,000-turn soak
- 100 concurrent operations
- 1,000 direct shell commands
- credential/profile isolation tests with same provider and model
- supervisor bound/eviction behavior

### SSH qualification

Run the complete release qualification on the configured SSH node, including installation/upgrade, migration from a v0.5.3 fixture, interactive zsh flows, connection isolation, runtime tests, soak/concurrency/direct-command suites, and uninstall/rollback where already required by the release process.

### Manual OAuth acceptance

In a bounded operator-assisted test:

1. Login to OpenAI OAuth profile `work`.
2. Login to OpenAI OAuth profile `personal`.
3. Select and make a model request through each connection.
4. Verify audit/status identifies only safe profile labels.
5. Logout one profile and prove the other remains functional.
6. Confirm neither OAuth connection uses an available `OPENAI_API_KEY`.

Qualification note (2026-07-30): this is an external, operator-assisted
prerequisite because it requires two operator-supplied subscription accounts and
interactive browser/device authorization. No such credentials were available to
the automated local or SSH release environment, so this release does not claim a
live two-account provider acceptance run. Automated qualification covers
profile-isolated full runtime roots, exact endpoint binding, strict API-key/OAuth
separation, independent status/logout, same-provider credential isolation, and
redacted audit attribution. Operators performing the bounded acceptance should
retain no token material in its evidence.

## Success Metrics

- An operator can switch between two accounts for the same provider and a model in one `/model` interaction.
- Shell-local switching requires no durable config edit and does not affect another shell.
- No tested request crosses credential or OAuth profile boundaries.
- Status and audit records always identify the safe connection used.
- Existing v0.5.3 configuration migrates without semantic loss.
- Full local and SSH release gates pass.

## Open Questions and Chosen Defaults

- Confirm the exact model-enumeration capability of the pinned OpenCode OAuth runtime during implementation. If it cannot enumerate, use configured, recent, static-catalog, and typed IDs as specified; do not add `models.dev`.
- Default the concurrent backend supervisor limit to 8, make it configurable, and document deterministic idle eviction.
- Preserve legacy `auto` only for migration and compatibility; setup should steer new connections to an explicit authentication type.
