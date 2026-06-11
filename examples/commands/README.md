# Example aishe custom commands (plugins / skills)

Copy any of these into `~/.config/aishe/commands/` (user-wide) or
`<project>/.aishe/commands/` (project-local, overrides user) and restart aishe.
The file name is the command name: `bigfiles.md` → `/bigfiles`.

Frontmatter (all optional):
- `description:` shown in `/commands` and tab-completion
- `mode:` `suggest` | `auto` | `yolo` (for NL commands; default = current mode)
- `shell:` `true` to run the body as a shell command instead of an NL request

Body is a template: `$ARGUMENTS` = all args, `$1`..`$9` = positional args.
