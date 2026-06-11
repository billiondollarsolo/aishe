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
- [Front-ends](front-ends.md) - zsh-PTY, the reedline editor, and the native hook
- [Inline AI ghost text](ghost-text.md) - predicted command completions as you type
- [Providers](providers.md) - Anthropic, OpenAI, Groq, Ollama, and others
- [Configuration reference](configuration.md) - every field in config.toml
- [Commands and slash-commands](commands.md) - meta commands and input prefixes
- [Custom commands and skills](custom-commands-and-skills.md) - your own /commands and model skills
- [MCP servers](mcp.md) - connect Model Context Protocol tool servers to yolo
- [Per-project context](project-context.md) - feed repo conventions via `.aishe/context.md`
- [Token usage and cost](usage-and-cost.md) - metering, the price table, and budgets
- [Safety gate](safety.md) - how dangerous commands are screened
- [Logging and privacy](logging.md) - secret redaction and the audit log
- [Prompt and theming](prompt-and-theming.md) - prompts, the git segment, colors
- [Shell integration and .aishrc](shell-integration.md) - the native hook and startup file
- [Troubleshooting](troubleshooting.md) - common issues and `aishe doctor`
- [Development](development.md) - building, testing, and the validation harness
- [Roadmap](ROADMAP.md) - where aishe is headed

## Reference files in the repo

- [examples/config.toml](../examples/config.toml) - a fully annotated config
- [examples/aishrc](../examples/aishrc) - a sample startup file
- [examples/commands/](../examples/commands/) - sample custom slash-commands
- [examples/skills/](../examples/skills/) - sample model-invoked skills
