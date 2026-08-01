> **Lifecycle: Implemented.** Baseline: v0.4.0. Retained as the credential-store
> design record; use [Commands](../commands.md),
> [Configuration](../configuration.md), and
> [Providers](../providers.md) for current behavior.

# AWS-style credentials and configuration

## 1. Outcome

Aishe will separate ordinary configuration from API credentials:

- `config.toml` keeps provider, endpoint, model, transport, prices, shell
  behavior, and a non-secret credential-profile name.
- `credentials.toml` keeps API keys in named profiles. It is created with mode
  `0600` inside Aishe's private config directory.
- The configured API-key environment variable remains supported and has the
  highest precedence. This preserves CI, container, secret-manager, and
  one-command override workflows.
- Interactive users no longer need to type `export OPENAI_API_KEY=...`, which
  can enter shell history.

The initial implementation deliberately follows the portable AWS CLI model: a
private local credentials file works over SSH and on headless Linux. Native
Keychain/Secret Service backends can be added later behind the same profile
interface without changing provider configuration.

## 2. What Aishe is adopting from AWS CLI

The implementation is based on the AWS CLI's documented behavior and its
current source, rather than merely copying its filenames:

- AWS separates sensitive values into `~/.aws/credentials` and ordinary
  settings into `~/.aws/config`. A named profile is assembled from the matching
  sections in both files.
- Environment credentials override shared-file credentials without modifying
  the file. AWS also supports explicit location overrides through
  `AWS_SHARED_CREDENTIALS_FILE` and `AWS_CONFIG_FILE`.
- `aws configure` and `aws configure set` route recognized credential fields
  to the shared credentials file and ordinary fields to the config file.
- `aws configure list` reports masked credential state together with its
  source/location, so users can understand which layer won.
- AWS creates new shared files with mode `0600` and checks for permissive
  credential-file permissions.
- AWS's larger credential-provider chain leaves room for short-lived
  credentials and `credential_process`. Aishe's first version implements the
  two relevant local layers—environment and shared file—behind one resolver so
  an external process or native secret store can be added later.

Primary references:

- [AWS CLI configuration and credential
  files](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-files.html)
- [AWS CLI environment-variable
  precedence](https://docs.aws.amazon.com/cli/latest/userguide/cli-configure-envvars.html)
- [AWS shared-file
  format](https://docs.aws.amazon.com/sdkref/latest/guide/file-format.html)
- [AWS CLI authentication and credential-provider
  order](https://docs.aws.amazon.com/cli/latest/userguide/cli-chap-authentication.html)
- [`aws configure set`
  implementation](https://github.com/aws/aws-cli/blob/develop/awscli/customizations/configure/set.py)
- [AWS CLI shared-file
  writer](https://github.com/aws/aws-cli/blob/develop/awscli/customizations/configure/writer.py)
- [`aws configure list`
  implementation](https://github.com/aws/aws-cli/blob/develop/awscli/customizations/configure/list.py)

Aishe deliberately improves four details instead of reproducing AWS CLI
literally:

1. Secret entry is no-echo; a key is never accepted as a command-line argument.
2. Writes are atomic and durable rather than editing the credentials file in
   place.
3. Insecure files fail closed with a repair instruction instead of only
   warning.
4. Status output shows profile and provenance but no suffix or other fragment
   of the key.

The shared idea is an AWS-style interface, not byte-compatible AWS files. Aishe
uses TOML for both files because its existing config, parser, migrations, and
diagnostics are TOML-based.

## 3. File format and locations

`credentials.toml` lives next to `config.toml`:

| Platform | Default path |
| --- | --- |
| Linux | `~/.config/aishe/credentials.toml` |
| macOS | `~/Library/Application Support/aishe/credentials.toml` |

`AISHE_CONFIG_DIR` relocates both files. `AISHE_CREDENTIALS_FILE` may override
only the credentials path for an external private volume or test fixture.

The versioned format is:

```toml
version = 1

[profiles.openai]
api_key = "..."

[profiles.groq]
api_key = "..."
```

Profile names are case-insensitive at the CLI and are normalized to lowercase.
They may contain ASCII letters, digits, `.`, `_`, and `-`, must begin with a
letter or digit, and are capped at 64 characters.

`config.toml` provider blocks gain a non-secret field:

```toml
[providers.openai]
credential = "openai"
api_key_env = "OPENAI_API_KEY"
```

The profile is tied to the service, not merely the API family. Selecting Groq,
OpenRouter, Together, or a custom OpenAI-compatible endpoint changes the
profile reference along with the endpoint and environment-variable name. This
prevents an OpenAI key from being sent to a different compatible endpoint after
a service switch.

## 4. Credential resolution

For a provider that requires authentication:

1. A non-empty configured environment variable wins.
2. A staged in-memory setup credential is used only inside the setup process.
3. The configured profile in `credentials.toml` is used.
4. Otherwise the credential is missing.

For an unauthenticated endpoint, a missing credential is valid. A configured
environment override or stored profile may still be sent when explicitly
present.

The resolver returns both the secret and non-secret provenance:

- `environment:OPENAI_API_KEY`
- `credentials_file:openai`
- `not_required`
- `missing`

Only the provider transport receives the secret. Config printing, JSON
diagnostics, capability caches, logs, support bundles, setup drafts, history,
and error messages receive provenance only.

## 5. CLI

```text
aishe auth set [PROFILE]
aishe auth set [PROFILE] --stdin
aishe auth set [PROFILE] --from-env VARIABLE
aishe auth status [PROFILE] [--json]
aishe auth list [--json]
aishe auth remove [PROFILE] [--yes]
aishe auth path
```

When `PROFILE` is omitted, Aishe uses the active user-config provider's
credential profile. Project config never chooses the default target of a
credential-writing command.

`auth set` never accepts the secret as a command-line argument:

- on a TTY it reads through a no-echo prompt with cancel/backspace support;
- `--stdin` reads one bounded value from standard input;
- `--from-env` copies an existing environment value without printing it.

`auth status` and `auth list` expose names and provenance only. `auth remove`
requires an interactive confirmation or `--yes` in automation. Removing the
last profile leaves a valid empty, private credentials file rather than
touching `config.toml`.

## 6. Setup and settings

Interactive setup's Credential step will offer:

1. use an existing saved profile when present;
2. enter and save a key locally (recommended for interactive users);
3. use the configured environment variable only;
4. go back or cancel.

The secret is held only in memory until final Apply. It is never serialized in
the resumable setup draft. If setup resumes after the credential step and no
saved/environment credential exists, it returns to the Credential step.
Provider model discovery and live validation can use the in-memory staged key.

The review displays the credential profile and intended source, never the
value. Applying setup writes the private credential store and ordinary config
separately. Cancelling setup changes neither.

Settings edits the profile reference and environment fallback, and links to the
dedicated `aishe auth` flow for secret replacement/removal. Secret writes are
not hidden inside a transactional ordinary-config draft.

## 7. Migration and compatibility

The ordinary config schema advances from 2 to 3. Migration:

- preserves every existing provider/model/endpoint setting;
- derives a profile name from known environment names (`OPENAI_API_KEY` →
  `openai`, `GROQ_API_KEY` → `groq`, and so on);
- creates the existing timestamped byte-for-byte config backup before the
  atomic rewrite;
- never copies an environment value automatically;
- never creates `credentials.toml` merely because Aishe was upgraded.

Schema-2 files remain read-only usable by Doctor. The runtime resolver derives
the same profile name in memory until a normal command performs migration.

The installer must continue treating both config files and the complete data
tree as user-owned state. Upgrade tests will seed `credentials.toml` and prove
its hash is unchanged after binary replacement.

## 8. File security

- New directories are mode `0700`; `credentials.toml` and atomic temporary
  files are mode `0600`.
- Writes use create-new temporary files, `fsync`, atomic rename, and directory
  sync.
- Reads reject symlinks, non-regular files, files larger than 1 MiB, unsupported
  schema versions, malformed TOML, empty stored keys, and group/world-readable
  modes on Unix.
- Errors name the path/profile and recovery command but never include file
  contents or a key.
- Doctor reports the credential path and permissions. `doctor --fix` can repair
  permissions through the existing private-tree repair, but never invents,
  imports, removes, or prints credentials.
- Secret inputs are bounded to 16 KiB and reject whitespace/control characters.
- The store and entry types do not implement `Debug`.

## 9. Diagnostics and user-facing errors

Doctor's provider credential check will distinguish:

- environment override active;
- saved profile active;
- authentication not required;
- missing credential, with `aishe auth set PROFILE` as the primary fix and the
  environment variable as the automation alternative;
- unreadable/insecure/malformed credentials store.

The generic “LLM not configured” path, model listing, provider test, semantic
history, fallbacks, reachability probes, and live validation all use the same
resolver. No feature may independently read only `std::env`.

## 10. Acceptance criteria

- **CRED-01** A key saved by `aishe auth set` works with no exported variable.
- **CRED-02** Environment values override, but never overwrite, saved profiles.
- **CRED-03** Setup can stage, validate, apply, cancel, and resume without
  serializing a secret into its draft.
- **CRED-04** Changing service presets changes the credential profile and does
  not reuse a different service's key.
- **CRED-05** Config/Doctor/status/list/support output never contains a key.
- **CRED-06** Malformed, oversized, symlinked, or over-permissive credential
  files fail closed with actionable errors.
- **CRED-07** Atomic writes leave no temporary secret file after success or
  failure.
- **CRED-08** `auth remove` changes only the exact selected profile.
- **CRED-09** Schema-2 migration preserves behavior, creates a backup, and does
  not import environment secrets.
- **CRED-10** Installer replacement preserves config, credentials, history,
  tasks, and unrelated state byte-for-byte.
- **CRED-11** Project overlays cannot silently target a credential-writing CLI
  command; explicit user config or explicit CLI profile chooses the target.
- **CRED-12** Existing environment-only configs and all unauthenticated local
  providers continue to work.

## 11. Validation matrix

### Rust and CLI tests

- store round-trip, permissions, path overrides, aliases, schema, size, symlink,
  malformed input, exact-profile removal, and atomic-temp cleanup;
- resolver precedence and provenance;
- provider construction/model listing/probe with file-only credentials;
- auth command hidden/stdin/from-env/status/list/remove contracts;
- config JSON and support-bundle secret absence;
- schema-2 migration with no credential import;
- missing-key errors name both `aishe auth set` and the environment alternative.

### PTY tests

- setup saved-profile, hidden-key, environment-only, cancel, and resumed-draft
  branches;
- hidden input never appears in the terminal transcript;
- settings shows the profile without showing a key;
- interactive auth removal confirmation.

### Installer and Linux tests

- installer fixture includes a synthetic `credentials.toml` and compares the
  complete pre/post config-root hash;
- Linux verifies modes `0700/0600`, Doctor provenance, config/history hash
  preservation, and no credential in process arguments/history/reports;
- an isolated live call runs with only `credentials.toml`, then an environment
  override proves precedence without modifying the file.

## 12. Deliberate first-release boundary

This milestone does not implement AWS SSO, role assumption, metadata services,
or `credential_process`. Those mechanisms issue and refresh temporary
credentials with provider-specific semantics, while Aishe providers currently
accept one opaque API key. The single resolver and profile reference are
designed so a process-backed or native-keychain source can be inserted between
the environment and shared file later without changing provider transports or
ordinary config.

## 13. Delivery order

1. Add the credential-store module and schema-3 provider reference.
2. Route every provider/capability/diagnostic credential read through it.
3. Add `aishe auth`.
4. Integrate setup and settings.
5. Update docs/examples/man/completions and installer fixtures.
6. Run format, strict Clippy, all Rust tests, PTY/admin/installer suites.
7. Build and exercise the candidate on the authorized Linux node using isolated
   config/data roots; do not alter the node's real credentials while testing.

## 14. Validation result

Completed on 2026-07-29:

- strict format and Clippy gates passed;
- all Rust targets passed (326 library tests plus binary and integration
  suites);
- 39 CLI end-to-end tests passed, including real local HTTP requests with a
  file-only key and an environment override;
- hidden setup/auth PTY entry, cancel/resume/apply, 339 generated PTY cases,
  44 native zsh features, and 7 signal/terminal cases passed;
- the admin harness passed 455/455 checks;
- installer replacement preserved synthetic config, credentials, history,
  tasks, and unrelated data byte-for-byte;
- a native x86-64 Linux candidate passed the isolated credentials contract on
  the authorized test node, and pre/post hashes proved every real Aishe
  config/data file on that node was unchanged;
- the temporary Linux source, Rust toolchain, and build tree were removed after
  validation.
