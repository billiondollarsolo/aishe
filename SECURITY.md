# Security Policy

aishe is an AI-aware shell: it runs real shell commands and sends context to a
language model. That combination has a real security surface, so we take reports
seriously and try to be explicit about what aishe does and does not protect
against.

## Reporting a vulnerability

Please report security issues privately. Do **not** open a public GitHub issue
for a vulnerability.

- Preferred: open a private report via GitHub Security Advisories
  ("Security" tab > "Report a vulnerability") on
  `billiondollarsolo/aishe`.
- Or email **mj@alphabravo.io** with "aishe security" in the subject.

Please include, as best you can:

- a description of the issue and its impact,
- the aishe version (`aishe --version`) and platform,
- minimal steps or a proof of concept to reproduce,
- any relevant config (with secrets removed).

What to expect:

- an acknowledgement within a few business days,
- a follow-up with an assessment and, where applicable, a fix or mitigation
  plan,
- coordinated disclosure: we will agree on a timeline with you before any public
  write-up, and credit you in the release notes unless you prefer otherwise.

Please act in good faith: do not access data that is not yours, do not run
denial-of-service or destructive tests against shared infrastructure, and give
us reasonable time to respond before any public disclosure.

## Supported versions

aishe is pre-1.0 and ships from `main`. Security fixes land on `main` and in the
next tagged release; there are no separate maintenance branches yet. Always run
the latest release. This policy will tighten once 1.0 ships.

| Version | Supported |
| ------- | --------- |
| latest release / `main` | yes |
| older releases | no (upgrade) |

## Security model

aishe treats model output as untrusted input, never as authorization. The
managed OpenCode process plans and maintains conversation state, but it has no
direct host-effecting tools. Aishe starts it with private HOME/XDG directories,
authenticated loopback endpoints, an exact checksum-verified runtime, and a
dependency-free trusted plugin. OpenCode's own shell/file/web tools are denied
for primary and child agents. Every effect crosses an authenticated,
session-bound foreground lease and is executed by Aishe under the active
mode/scope/policy.

The controls below are listed **strongest first**, because they are not equally
strong and it matters which one you are actually relying on:

- **OS-enforced sandbox (Linux only).** With `yolo_sandbox = true` and
  `sandbox_backend = "bwrap"`, every model-run command executes under
  [bubblewrap](https://github.com/containers/bubblewrap) with a read-only root and
  only the working tree and `/tmp` writable. This is the one layer that holds
  regardless of how a command is spelled. `aishe dry-run` uses the same machinery to
  preview changes before they touch the real tree. **There is no sandbox backend on
  macOS**; there, `yolo` falls back to the layers below. `aishe doctor` reports what
  is actually active.
- **Mode and scope admission.** `suggest` does not expose effecting tools;
  `auto` can perform safe/read-only work but holds risky actions for review;
  `yolo` requires one explicit acceptance for the current shell and selected
  `workspace` or `host` scope. Once accepted, yolo does not interrupt every
  action with another approval—the accepted scope, organization policy,
  deterministic safety classification, budget, network policy, and sandbox are
  enforced at each tool lease. A new shell must accept yolo again. See
  [docs/modes.md](docs/modes.md).
- **Deterministic safety gate.** Every command the model proposes is screened by a
  separate, rule-based check first: it either matches a destructive shape
  (`dangerous`), fails to resolve what would run (`unknown` — fails closed to a
  confirmation), or matches nothing (`safe`). This is the **weakest** of the three
  and the only one that can be wrong silently: `safe` means "nothing matched", not
  "this is safe". See [docs/safety.md](docs/safety.md).
- **Best-effort policy sandbox.** `yolo_sandbox` without `bwrap` can refuse commands
  that reach the network or write outside the working tree. This is a heuristic
  policy fed back to the model, **not** a kernel sandbox; do not rely on it as a
  security boundary against a determined adversary.

## Threat model

Being blunt about this, because the answer decides whether you should run aishe on a
work machine.

**What aishe is designed to defend against:** a language model — possibly one that
has read hostile content — proposing a command that would destroy data or the system
by mistake. The gate catches many such shapes deterministically, confirmation puts a
human in front of the rest, and on Linux the sandbox makes the damage physically
impossible rather than merely unlikely.

**What aishe does not defend against:**

- **A determined adversary choosing the command text.** The safety gate is a pattern
  matcher over command text. It unwraps a deliberately incomplete table of runner and
  exec binaries — anything outside it hides its payload — scans non-shell interpreter
  code only for a literal shell-out, judges a remote payload as a string without
  knowing what the far side does with it, and cannot know what an opaque producer will
  pipe into a shell. Anyone who picks the spelling can route around it;
  an unlucky command shape can route around it by accident. The classes and the
  reasoning are in
  [docs/safety.md](docs/safety.md#what-the-gate-does-not-catch).
- **Prompt injection reaching the model at all.** aishe reduces the blast radius of
  a bad proposal; it does not stop hostile content from producing one.
- **A host-scope or policy-only yolo session is intentionally powerful.** On
  macOS there is no OS sandbox in this release, and Aishe warns once before
  accepting yolo for that shell. On Linux, `host` scope is also intentionally
  unsandboxed so administration can work. Do not accept either scope on a
  machine whose contents you are not prepared for the agent to change.

If you need a hard guarantee, run aishe on Linux with `sandbox_backend = "bwrap"`,
or inside a VM or container you are willing to lose.

### Other threats you should be aware of

- **Prompt injection.** The model reads context that may be attacker-controlled:
  command output, files, fetched URLs (`fetch_url`), and MCP tool results. Hostile
  content can try to steer the model into proposing harmful commands. The safety
  gate and confirmation tiers are the mitigation; for untrusted repositories or
  data, prefer `suggest` mode, keep `yolo_sandbox` on, and review commands before
  approving them.
- **Third parties receive your data.** Whatever provider (Anthropic, OpenAI, Groq,
  a local endpoint, ...) and whatever MCP servers you configure receive the
  context aishe sends them, subject to their own policies. You choose them.
- **Repo-supplied content is gated by trust.** A cloned repository can ship a
  `.aishe/` directory, and `cd`-ing into it must not hand the repo your shell:
  - `.aishe/config.toml`: only safe, cosmetic keys apply automatically. Sensitive
    keys (provider/endpoint, `[mcp_servers]` — which can launch arbitrary commands —
    audit logging, the safety toggles, and `mode = "yolo"`) are ignored until you run
    `aishe trust` in that repo. See [docs/project-config.md](docs/project-config.md).
  - `.aishe/commands/`: a project command never shadows a user command of the same
    name. An untrusted project command with `shell: true` shows you the resolved
    command line and asks before running it, and its `mode:` frontmatter cannot
    escalate you into a more autonomous mode than the one you are in.
  - `.aishe/skills/`: a project skill never shadows a user skill of the same name
    (the user's wins), and it *is* trust-gated. Skill text is never executed as a
    command, but it is instructions handed to the model, and the model pulls a
    skill in mid-loop where there is no moment left to confirm at — so an
    untrusted project skill is dropped at load: absent from the catalog, absent
    from `use_skill`, until you run `aishe trust <file>`. Trusting one means
    accepting its body as instructions, so treat a cloned repo's skills as a
    prompt-injection surface.

  Only trust repositories you vouch for.

## Data handling and privacy

- **API keys are never written to ordinary config or backend state.** A named
  credential profile is stored separately in private `credentials.toml`
  (directory `0700`, file `0600`) through hidden `aishe auth set` or setup
  input. The environment variable named by `api_key_env` remains a
  higher-precedence, process-local override. Provider secrets are injected only
  into the managed provider process and are stripped from model-controlled
  command, skill, and MCP tool environments. They are never written to OpenCode
  config, session mappings, journals, drafts, logs, or support bundles.
- **OAuth tokens use a separate private runtime store.** OpenAI and xAI login
  delegates to Aishe's exact pinned OpenCode runtime and writes only its
  isolated mode-`0600` auth file. OAuth is accepted only for the provider's
  exact official host, API keys retain precedence, and status, Doctor, logs,
  and support bundles report metadata without displaying token material. Treat
  that auth file like a password: do not commit, copy, or share it.
- **What is sent to the model.** For an AI request, aishe sends your prompt plus a
  context block (working directory, recent commands, and, if enabled, a per-project
  `.aishe/context.md`). It is sent only when you trigger an AI action, not on every
  keystroke (ghost text is the one opt-in exception and is off by default).
- **Secret redaction.** `redact_secrets` (on by default) scrubs likely secrets
  (tokens, passwords, URL credentials) from the context block before it is sent.
  It is pattern-based and best-effort; it is not a guarantee that no secret ever
  leaves the machine.
- **Audit logging is off by default.** When enabled (`[logging] enabled = true` or
  `AISHE_LOG=1`), aishe writes prompts, model responses, and AI-initiated actions
  to disk. The log can contain sensitive data; protect the file accordingly. See
  [docs/logging.md](docs/logging.md).
- **No telemetry.** aishe sends no usage analytics or phone-home traffic. The only
  outbound network calls are to the model provider, MCP servers, and the `fetch_url`
  web tool, all of which you configure.

## Hardening recommendations

- Run untrusted projects in `suggest` mode and review every command.
- On Linux, install `bubblewrap` and use `workspace` scope for agentic work—it
  is the only local guardrail here that an unusual command shape cannot talk
  its way past. Setup offers a consented package-manager install and verifies
  the sandbox functionally; `aishe doctor` reports the effective result.
- On macOS, prefer suggest/auto for untrusted work, or run Aishe inside a VM or
  container. Treat acceptance of policy-only yolo as granting the agent your
  user's effective filesystem and process privileges.
- Set a `budget_usd` cap so a runaway loop cannot spend without bound.
- Treat MCP servers and `fetch_url` targets as you would any third-party
  dependency; only configure ones you trust.
- Keep audit logs (if enabled) on an encrypted, access-controlled volume.
