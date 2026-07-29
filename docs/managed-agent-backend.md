# Managed agent backend

Aishe uses a private, compatibility-pinned OpenCode runtime as its agent engine.
This is an implementation layer, not a second user interface: no OpenCode TUI
opens, users do not run an OpenCode command, and ordinary shell commands never
start the backend.

## What runs when

- A command such as `git status`, `ssh host`, or `vim file` stays in the user's
  real zsh. It performs no backend startup or provider request.
- A natural-language turn lazy-starts a private per-user supervisor and
  OpenCode server on random IPv4 loopback ports.
- Aishe subscribes before admitting the prompt, renders normalized events in the
  existing terminal, and keeps one durable conversation per shell/workspace.
- The supervisor exits after its idle timeout. The next AI turn starts it again;
  the durable session remains available.

The exact OpenCode version supported by each Aishe build is embedded in a
manifest. The current compatibility pin is **OpenCode 1.18.9**. Aishe downloads
only the platform asset named in that manifest, enforces bounded download and
extraction limits, verifies its SHA-256, verifies the executable version, and
installs it under Aishe's user data directory. An unrelated system or Homebrew
OpenCode installation is never substituted.

## Ownership and security boundary

OpenCode owns conversation orchestration, provider interaction, reasoning,
compaction, and agent/subagent state. Aishe remains authoritative for everything
that can affect the machine:

- input routing and terminal UI;
- provider/model/credential preferences and price data;
- execution mode, workspace/host scope, network policy, and budgets;
- shell commands, file edits, web fetches, MCP tools, and skills;
- approvals, bubblewrap confinement, redaction, audit, undo, and runbooks.

The managed server is isolated from normal OpenCode config and plugins by a
private `HOME` and private XDG directories. Aishe writes one embedded,
hash-verified plugin into that environment. OpenCode's host-effecting built-in
tools are hidden or denied for primary agents and subagents. The plugin exposes
only Aishe proxy tools.

Each AI turn creates a short-lived authenticated foreground lease. A tool call is
accepted only when its session, message, call ID, workspace, mode, scope, network
policy, and foreground lease agree. The supervisor journals call state before
execution; a duplicate completed call receives the recorded result and is not
run twice. A call that was running when the foreground disappeared is marked
outcome-unknown rather than repeated.

Provider credentials are passed only to the managed server process for the
duration of the request. Aishe strips provider variables, `AISHE_*`,
`OPENCODE_*`, and other likely secrets from every model-controlled command/tool
environment. Credentials, loopback authentication tokens, and raw environments
are never written to a session mapping, tool journal, or support bundle.

## Scope and mode behavior

`suggest` has no tools. It may answer or propose one command for review.

`auto` exposes Aishe tools, but Aishe remains action-gated: safe reads can run
and risky or state-changing effects require the applicable approval.

`yolo` requires one explicit acceptance in each new shell:

- `workspace` confines effects to the selected workspace. On Linux, functional
  bubblewrap is required for the OS-isolated workspace profile when configured
  by setup or organization policy.
- `host` grants host-wide agent authority for that shell only and carries a
  clear warning. An organization policy can disable it.

After yolo scope acceptance, neither Aishe nor OpenCode asks again for each
action. Acceptance is deliberately not written to config, so a new shell must
accept again. Commands typed directly by the user remain ordinary zsh commands
and do not inherit agent authority.

macOS does not have the Linux bubblewrap boundary. Setup and Doctor label its
agent execution as policy-only; Aishe never describes that state as OS
sandboxed.

## Runtime operations

Setup normally owns runtime installation. These commands are available for
automation, offline installs, support, and recovery:

```sh
aishe backend status [--json]
aishe backend install [--from PATH] [--force] [--json]
aishe backend verify [--live] [--json]
aishe backend repair [--from PATH] [--json]
aishe backend rollback
aishe backend stop
aishe backend logs [--tail N]
aishe backend gc [--dry-run]
```

`verify` checks manifest identity, metadata, checksum, license/notices, and
executable version. `--live` additionally starts the authenticated loopback
server and verifies its health/config/plugin/tool restrictions without making a
provider call. `repair` stages and verifies a replacement before activation.
`rollback` swaps to the immediately previous compatible, already-verified
runtime. `gc` removes only abandoned download/staging paths.

For an offline or mirrored setup:

```sh
aishe setup --non-interactive \
  --backend opencode --install-backend \
  --runtime-file /media/opencode-1.18.9.tar.gz \
  ...other required setup flags...

# or
aishe setup --non-interactive \
  --backend opencode --install-backend \
  --runtime-base-url https://mirror.example/aishe/runtime \
  ...other required setup flags...
```

The archive must still match the checksum and size in Aishe's embedded
compatibility manifest. An organization policy can require one mirror and
approved hash set.

## Fallback and failure semantics

The native compatibility engine is allowed only if the managed backend fails
before OpenCode admits a prompt. This keeps suggest/chat/auto usable during
repair. Aishe never starts a second provider turn after admission, partial
output, or a tool effect: those failures become interrupted managed sessions
that can be inspected and resumed.

```sh
aishe sessions
aishe session show ses_...
aishe resume ses_...
```

Outside an Aishe shell, `resume` launches the real zsh in the recorded workspace
and binds it directly to that durable conversation. Legacy native task records
remain listed and resumable during the compatibility window.

## Paths and upgrades

With `<data>` meaning Aishe's platform data directory:

```text
<data>/runtime/opencode/<version>/      verified managed executable/notices
<data>/backend/opencode/                isolated HOME/XDG/plugin/server data
<data>/backend/sessions/index.json      Aishe shell/workspace/session mapping
<data>/backend/journal/tool-calls.json  idempotency and usage journal
```

Binary and managed-runtime upgrades never delete or rewrite Aishe config,
credentials, shell history, session mappings, task records, audit logs, or undo
journals.

`aishe uninstall --dry-run` shows exact paths. Plain `aishe uninstall` selects
only the replaceable binary/completion/man and managed-runtime layers. User data
is available only through separate flags (`--sessions`, `--config`, `--history`,
`--audit-undo`, or `--all`) and requires explicit confirmation. Aishe reports
that selected user-state deletion is permanent.

## Diagnostics

Start with:

```sh
aishe doctor
aishe doctor --fix
aishe doctor --live
aishe doctor --json
aishe doctor --bundle ./aishe-support.json
```

Doctor reports the engine, pinned runtime presence/version/hash/license,
supervisor identity and loopback/auth health, isolated backend config, trusted
plugin hash, tool restrictions/bridge health, event stream, provider/model,
credential isolation, session/journal state, and bubblewrap's functional
self-test. `--fix` performs only bounded local repairs; it never installs a
system package or changes an API key.

The complete architectural and release acceptance contract is
[OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md](design/OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md).
