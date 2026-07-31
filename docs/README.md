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
  [forcing natural language (`?`)](getting-started.md#6-force-a-route-when-needed)
  when a line starts with a real binary (`install`, `find`, …)
- [Commands and slash-commands](commands.md) — CLI surface, `/help`, `/connection` vs `/model`
- [Providers](providers.md) — Anthropic, OpenAI/Codex, xAI/Grok, Groq, Ollama, OAuth
- [Managed agent backend](managed-agent-backend.md) — pinned OpenCode runtime, security boundary, recovery
- [Modes](modes.md) — suggest, auto, yolo, streaming, structured output
- [Front-ends](front-ends.md) — the zsh-PTY interactive shell and the native hook
- [Configuration reference](configuration.md) — every field in config.toml
- [Custom commands and skills](custom-commands-and-skills.md) — your own /commands and model skills
- [MCP servers](mcp.md) — connect Model Context Protocol tool servers to yolo
- [Per-project context](project-context.md) — feed repo conventions via `.aishe/context.md`
- [Token usage and cost](usage-and-cost.md) — metering, the price table, and budgets
- [Safety gate](safety.md) — how dangerous commands are screened
- [Logging and privacy](logging.md) — secret redaction and the audit log
- [Runbooks](runbooks.md) — export a yolo session as a script + markdown runbook
- [Shell integration and .aishrc](shell-integration.md) — native hook, startup
  file, [force-NL / Option-as-Meta on Mac](shell-integration.md#force-nl-and-input-prefixes)
- [Per-project config and trust](project-config.md) — project overlays and `aishe trust`
- [Troubleshooting](troubleshooting.md) — common issues (`install` ran as shell,
  `aishe doctor`, …)
- [Development](development.md) — building, testing, and the validation harness
- [Roadmap](ROADMAP.md) — where aishe is headed
- [Master plan](design/PLAN.md) — long-form plan and acceptance criteria
- [Interactive UX milestone](design/UX_MILESTONE_PLAN.md) — setup, diagnostics, status, durable tasks
- [OpenCode backend implementation](design/OPENCODE_BACKEND_IMPLEMENTATION_PLAN.md) — architecture and release criteria
- [Feature proposals](proposals.md) — specs for the next wave

## Reference files in the repo

- [examples/config.toml](../examples/config.toml) — fully annotated config
- [examples/aishrc](../examples/aishrc) — sample startup file
- [examples/commands/](../examples/commands/) — sample custom slash-commands
- [examples/skills/](../examples/skills/) — sample model-invoked skills
