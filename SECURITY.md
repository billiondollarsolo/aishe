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

aishe is built so that the language model is never trusted to decide what runs:

- **Deterministic safety gate.** Every command the model proposes is checked by a
  separate, rule-based gate before it can execute. The model's output is treated
  as untrusted input, not as an authorization. See [docs/safety.md](docs/safety.md).
- **Graduated confirmation.** `suggest` proposes and waits for you; `auto` runs
  safe commands but stops on dangerous ones; `yolo` pauses according to the
  `yolo_confirm` tier (`never` / `dangerous` / `writes` / `all`). See
  [docs/modes.md](docs/modes.md).
- **Best-effort policy sandbox.** `yolo_sandbox` can refuse commands that reach
  the network or write outside the working tree. This is a heuristic policy fed
  back to the model, **not** a kernel sandbox; do not rely on it as a security
  boundary against a determined adversary.

### Threats you should be aware of

- **Prompt injection.** The model reads context that may be attacker-controlled:
  command output, files, fetched URLs (`fetch_url`), and MCP tool results. Hostile
  content can try to steer the model into proposing harmful commands. The safety
  gate and confirmation tiers are the mitigation; for untrusted repositories or
  data, prefer `suggest` mode, keep `yolo_sandbox` on, and review commands before
  approving them.
- **The safety gate is heuristic.** It blocks known-dangerous patterns but cannot
  prove an arbitrary command is safe. Treat it as a seatbelt, not a vault.
- **Third parties receive your data.** Whatever provider (Anthropic, OpenAI, Groq,
  a local endpoint, ...) and whatever MCP servers you configure receive the
  context aishe sends them, subject to their own policies. You choose them.
- **Project config from untrusted repos.** A cloned repository can contain a
  `.aishe/config.toml`. aishe applies only safe, cosmetic keys from it
  automatically; sensitive keys (provider/endpoint, `[mcp_servers]`, audit
  logging, the safety toggles, and `mode = "yolo"`) are ignored until you run
  `aishe trust` in that repo. Only trust repositories you vouch for. See
  [docs/project-config.md](docs/project-config.md).

## Data handling and privacy

- **API keys are never written to the config file.** They are read at runtime from
  the environment variable named by `api_key_env`. Keep them in your shell
  environment or a secrets manager, not in `config.toml`.
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
- Keep `yolo_sandbox` on and confirmation at `dangerous` or stricter for agentic
  work.
- Set a `budget_usd` cap so a runaway loop cannot spend without bound.
- Treat MCP servers and `fetch_url` targets as you would any third-party
  dependency; only configure ones you trust.
- Keep audit logs (if enabled) on an encrypted, access-controlled volume.
