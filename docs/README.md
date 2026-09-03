# AIShe documentation

> **Alpha (pre-1.0).** AIShe is under active development. Commands, config
> schema, and UX may change between releases. Prefer workspace scope and Linux
> isolation for untrusted autonomous work; see [Safety](safety.md).

Welcome to the **AIShe** (**AI Shell**) user guide. The CLI is `aishe`. AIShe is a
natural-language-aware shell: it behaves like zsh for real commands, and routes
anything that is not a command to an LLM that either suggests a command or runs
one autonomously.

If you are new, start with [Installation](installation.md) and then
[Getting started](getting-started.md). Product overview and marketing quickstart
live in the [root README](../README.md).

## Guide

- [Installation](installation.md) — prebuilt releases, packages, or source
- [Getting started](getting-started.md) — guided setup, first requests, and
  [forcing natural language (`?`)](getting-started.md#5-force-a-route-when-needed)
  when a line starts with a real binary (`install`, `find`, …)
- [Daily-driver agent workflows](daily-driver.md) — buffer AI, attachments,
  isolated background tasks, failure recovery, roles, MCP, updates, and status
- [Commands and slash-commands](commands.md) — CLI surface, `/help`, `/connection` vs `/model`
- [Route overrides](route-prefixes.md) — canonical `?`, one-line shell `!`, and the deprecated `#` alias
- [Providers](providers.md) — Anthropic, OpenAI/Codex, xAI/Grok, Groq, Ollama, OAuth
- [Managed agent backend](managed-agent-backend.md) — pinned OpenCode runtime, security boundary, recovery
- [Modes](modes.md) — suggest, auto, yolo, streaming, structured output
- [Front-ends](front-ends.md) — the zsh-PTY interactive shell and the native hook
- [Native Bash compatibility](bash-compatibility.md) — tested Tier B/B- matrix,
  version differences, and deterministic qualification evidence
- [Terminal compatibility](terminal-compatibility.md) — macOS PTY gate,
  multiplexer/latency/resize evidence, SSH limitations, and manual terminal matrix
- [Accessibility and palette review](accessibility.md) — non-color cues,
  static/ASCII fallbacks, keyboard access, and maintained contrast evidence
- [Configuration reference](configuration.md) — every field in config.toml
- [Automation contracts](automation.md) — JSON/JSONL schemas, streams, exit codes, fixtures, and compatibility policy
- [Custom commands and skills](custom-commands-and-skills.md) — your own /commands and model skills
- [MCP servers](mcp.md) — connect Model Context Protocol tool servers to yolo
- [Per-project context](project-context.md) — feed repo conventions via `.aishe/context.md`
- [Token usage and cost](usage-and-cost.md) — metering, the price table, and budgets
- [Safety gate](safety.md) — how dangerous commands are screened
- [Logging and privacy](logging.md) — secret redaction and the audit log
- [Data retention and deletion](data-retention.md) — local state inventory, bounds, export, dry-run cleanup, and preservation guarantees
- [Runbooks](runbooks.md) — export a yolo session as a script + markdown runbook
- [Shell integration and .aishrc](shell-integration.md) — native hook, startup
  file, [force-NL / Option-as-Meta on Mac](shell-integration.md#force-nl-and-input-prefixes)
- [Per-project config and trust](project-config.md) — project overlays and `aishe trust`
- [Troubleshooting](troubleshooting.md) — common issues (`install` ran as shell,
  `aishe doctor`, …)
- [Development](development.md) — building, testing, and the validation harness
- [Release readiness and rollback](release-readiness.md) — required evidence, holds, state compatibility, and failed-rollout response
- [Daily-driver agentic shell plan](design/DAILY_DRIVER_AGENTIC_SHELL_PLAN.md) — implementation contract for buffer AI, background agents, isolation, context, trust, and lifecycle
- [v0.7.0 release record](releases/v0.7.0.md) — complete change summary, migration notes, qualification evidence, and accepted alpha risks
- [Legacy compatibility lifecycle](legacy-compatibility.md) — removed front-end tombstones, migration windows, and retained native/task fields
- [Active product plan](design/NEXT_PRODUCT_UX_RELIABILITY_PLAN.md) — v0.7.0 implementation evidence and post-qualification work queue
- [Design lifecycle index](design/README.md) — authoritative inventory of active, implemented, superseded, historical, and validation documents
- [OpenCode backend implementation](design/OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md) — implemented backend design record

## Reference files in the repo

- [examples/config.toml](../examples/config.toml) — fully annotated config
- [examples/aishrc](../examples/aishrc) — sample startup file
- [examples/commands/](../examples/commands/) — sample custom slash-commands
- [examples/skills/](../examples/skills/) — sample model-invoked skills
