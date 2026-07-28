# Example aishe custom commands (plugins / skills)

Copy any of these into aishe's `commands/` directory (user-wide) or
`<project>/.aishe/commands/` (project-local) and restart aishe. If both define
the same name, the **user** file wins — a project cannot shadow a command you
wrote yourself. The file name is the command name: `bigfiles.md` → `/bigfiles`.

The user directory follows the platform convention, so it is **not** the same on
both. Run `aishe doctor` to see the resolved config path:

- Linux: `~/.config/aishe/commands/`
- macOS: `~/Library/Application Support/aishe/commands/`

A file left in `~/.config/aishe/commands/` on macOS is silently ignored. See
[docs/configuration.md](../../docs/configuration.md#file-locations).

Frontmatter (all optional):
- `description:` shown in `/commands` and tab-completion
- `mode:` `suggest` | `auto` | `yolo` (for NL commands; default = current mode).
  A **project** command may not use this to escalate above your configured mode
  unless you `aishe trust` its file; de-escalation always applies.
- `shell:` `true` to run the body as a shell command instead of an NL request.
  A project command with this set is confirmed before each run until you
  `aishe trust .aishe/commands/<name>.md`; the safety gate applies either way.

Body is a template: `$ARGUMENTS` = all args, `$1`..`$9` = positional args.
