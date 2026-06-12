# Commands and slash-commands

aishe's interactive shell is your real zsh; aishe adds a small set of
subcommands, a few inspection commands, and input prefixes that control routing.

## Subcommands

```
aishe                  launch the interactive zsh-PTY shell
aishe zsh              the same, explicitly
aishe -c '<line>'      run one line non-interactively and exit
aishe init zsh|bash    print the shell-hook snippet (for ~/.zshrc / ~/.bashrc)
aishe doctor           check shell, config, provider, and API key
aishe completions ...  print a shell completion script for aishe itself
aishe trust [--list]   trust this repo's .aishe/config.toml (and list trusted)
aishe untrust [--all]  drop trust for this repo (or all repos)

aishe mode [suggest|auto|yolo]      show or set the interaction mode
aishe model [NAME]                  show or set the model (for the active provider)
aishe provider [anthropic|openai]   show or set the provider
aishe config                        print the active configuration
aishe mcp                           list the MCP tools offered to yolo
aishe commands                      list your custom slash-commands
aishe skills                        list model-invoked skills
```

These are real subcommands, so they work the same in the interactive zsh-PTY
shell, a plain shell, or a script.

## Changing settings

`aishe mode`, `aishe model`, and `aishe provider` show the current value with no
argument, or save a new one to `~/.config/aishe/config.toml` with an argument:

```sh
aishe mode auto         # persist the default mode
aishe provider openai   # switch provider...
aishe model gpt-4o      # ...then set that provider's model
```

The saved value goes to your user config (a project overlay or a `--mode`/
`--provider` flag on the same command is not baked in). You can also set these
per session with the `--mode`/`--model`/`--provider` flags or `$AISHE_MODE`, and
in the interactive shell **Shift-Tab** (or `$AISHE_MODE_KEY`) cycles the mode
`suggest -> auto -> yolo`. Every field is in
[Configuration reference](configuration.md).

## Inspecting things

`aishe config`, `aishe mcp`, `aishe commands`, and `aishe skills` print the active
config and registries. They also work as slash-commands in the `-c` form
(`aishe -c '/config'`, `aishe -c '/usage'`, ...).

## Input prefixes

These are not commands; they control routing of a single line, and work in the
interactive shell and in `-c`:

- `?<text>` forces natural-language. Use it when your request starts with a real
  command name, for example `?find the largest files`.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## Custom slash-commands

You can define your own `/commands` as Markdown files, plus model-invoked skills.
They run via the hook interactively and in the `-c` form. See
[Custom commands and skills](custom-commands-and-skills.md).

## Exiting

Exit with `exit`, `quit`, or `Ctrl-D`.
