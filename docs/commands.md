# Commands and slash-commands

aishe has a set of built-in meta commands for inspecting and changing settings.
Each one also works as a slash-command, Claude Code style.

## Meta commands

```
aishe mode [suggest|auto|yolo]    show or set the interaction mode
aishe model [NAME]                show or set the model
aishe provider [anthropic|openai] show or set the provider
aishe editor [emacs|vi]           show or set the line-editor keymap
aishe frontend [auto|reedline|zsh-pty]  show or set the front-end
aishe stream [on|off]             show or toggle token streaming
aishe structured [schema|json|prompt]   output-format strategy
aishe theme [PRESET]              show or set the color preset
aishe usage                       session token and cost usage
aishe reset                       clear conversation memory
aishe ghost [on|off]              inline AI ghost-text autosuggestion
aishe plan [on|off]               yolo plan-first dry run
aishe commands                    list custom slash-commands
aishe skills                      list model-invoked skills
aishe mcp                         list MCP tools (yolo)
aishe config                      print the active config
aishe rehash                      rebuild the command cache
aishe help                        show help
```

Run a meta command with no argument to see the current value, or with an argument
to set it. Settings that change behavior persist to your config.

## Slash-commands

Every meta command also works with a leading slash:

```
/mode auto
/config
/help
/usage
```

Slash-commands are tab-completable. A `/`-prefixed path such as `/usr/bin/x` is
still treated as a normal command, because only known meta names and your own
custom commands intercept the slash.

The read-only listings (`/commands`, `/skills`, `/mcp`, `/config`, `/usage`) also
work in the non-interactive `-c` form:

```sh
aishe -c "/skills"
aishe -c "/usage"
```

## Input prefixes

These are not meta commands, but they control routing of a single line:

- `?<text>` forces natural-language. Use it when your request starts with a real
  command name, for example `?find the largest files`.
- `!<cmd>` forces shell and bypasses the safety gate, for example `!rm -rf build`.

After a command fails, type `?` alone on the next line to ask the model to
diagnose the error.

## Custom slash-commands

Beyond the built-ins, you can define your own `/commands` as Markdown files, and
model-invoked skills. See
[Custom commands and skills](custom-commands-and-skills.md).

## Exiting

Exit with `exit`, `quit`, or `Ctrl-D`.
