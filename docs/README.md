# aishe documentation

Welcome to the aishe user guide. aishe is a natural-language-aware shell: it
behaves like zsh for real commands, and routes anything that is not a command to
an LLM that either suggests a command or runs one autonomously.

If you are new, start with [Installation](installation.md) and then
[Getting started](getting-started.md).

## Guide

- [Installation](installation.md) - build from source today, prebuilt packages later
- [Getting started](getting-started.md) - first run, the wizard, your first requests
- [Modes](modes.md) - suggest, auto, yolo, streaming, structured output
- [Front-ends](front-ends.md) - the zsh-PTY interactive shell and the native hook
- [Providers](providers.md) - Anthropic, OpenAI, Groq, Ollama, and others
- [Configuration reference](configuration.md) - every field in config.toml
- [Commands and slash-commands](commands.md) - meta commands and input prefixes
- [Custom commands and skills](custom-commands-and-skills.md) - your own /commands and model skills
- [MCP servers](mcp.md) - connect Model Context Protocol tool servers to yolo
- [Per-project context](project-context.md) - feed repo conventions via `.aishe/context.md`
- [Token usage and cost](usage-and-cost.md) - metering, the price table, and budgets
- [Safety gate](safety.md) - how dangerous commands are screened
- [Logging and privacy](logging.md) - secret redaction and the audit log
- [Shell integration and .aishrc](shell-integration.md) - the native hook and startup file
- [Troubleshooting](troubleshooting.md) - common issues and `aishe doctor`
- [Development](development.md) - building, testing, and the validation harness
- [Roadmap](ROADMAP.md) - the tracked checklist of where aishe is headed
- [Master plan](PLAN.md) - the long-form plan: reasoning, sequencing, and acceptance criteria
- [Feature proposals](proposals.md) - detailed specs for the next wave (robustness + differentiators)

## Reference files in the repo

- [examples/config.toml](../examples/config.toml) - a fully annotated config
- [examples/aishrc](../examples/aishrc) - a sample startup file
- [examples/commands/](../examples/commands/) - sample custom slash-commands
- [examples/skills/](../examples/skills/) - sample model-invoked skills
