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
```

## Changing settings

There is no in-session settings command; set things one of these ways:

- **Per session:** the `--mode`, `--model`, and `--provider` flags, or
  `$AISHE_MODE`.
- **In the interactive shell:** **Shift-Tab** (or `$AISHE_MODE_KEY`) cycles the
  mode `suggest -> auto -> yolo`; the prompt glyph follows.
- **Persistently:** edit `~/.config/aishe/config.toml` (every field is in
  [Configuration reference](configuration.md)).

## Inspecting things

The read-only listings work in the non-interactive `-c` form:

```sh
aishe -c '/config'      # print the active config
aishe -c '/usage'       # session token and cost (per process)
aishe -c '/mcp'         # MCP tools offered to yolo
aishe -c '/skills'      # model-invoked skills
aishe -c '/commands'    # your custom slash-commands
```

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
