# Custom commands and skills

aishe supports two kinds of user-defined extensions, modeled on Claude Code:

- **Custom slash-commands** that you invoke (`/bigfiles`, `/gitsync`).
- **Skills** that the model invokes on its own when a request matches.

Both are plain Markdown files with optional frontmatter. The formats match Claude
Code, so artifacts from that ecosystem drop in unchanged.

## Custom slash-commands

Put Markdown files in either directory. If both define the same name, the **user**
file wins: a project file cannot shadow a command you wrote yourself.

- User: `~/.config/aishe/commands/`
- Project: `<current directory>/.aishe/commands/`

A project command that runs shell (`shell: true`) is also gated, the same way a
project `config.toml` is gated by `aishe trust`: aishe prints the resolved command
and asks for a `y/N` before running it.

The file stem is the command name: `bigfiles.md` becomes `/bigfiles`. Run
`aishe commands` or `/commands` to list what is loaded; they tab-complete.

### Format

```md
---
description: Suggest a command to find the biggest files
mode: suggest            # suggest | auto | yolo (for NL commands); default = current mode
# shell: true            # run the body as a shell command instead of an NL request
---
Show the 10 largest files under $ARGUMENTS, human-readable, largest first.
```

Frontmatter keys aishe uses:

- `description`: shown in listings and completion.
- `mode`: the mode an NL command runs in (`suggest`, `auto`, `yolo`). Optional.
- `shell`: if `true`, the body is run directly as a shell command instead of being
  sent to the model.

Any other keys (for example `allowed-tools` or `model` from Claude Code files) are
ignored, so those files still work.

### Two kinds of command

- **Natural-language commands** (no `shell`): the body is a prompt template. aishe
  expands it and runs it as a request in the chosen mode. Good for reusable asks
  like `/fixup` or `/bigfiles`.
- **Shell commands** (`shell: true`): the expanded body runs directly. Good for
  parameterized aliases like `/gitsync`.

### Templating

- `$ARGUMENTS` expands to all arguments joined by spaces.
- `$1` through `$9` expand to positional arguments (missing ones become empty).

Example invocation: `/deploy 5 the release` makes `$1` = `5`, `$2` = `the`,
`$ARGUMENTS` = `5 the release`.

### Examples in the repo

- [examples/commands/bigfiles.md](../examples/commands/bigfiles.md): an NL command.
- [examples/commands/gitsync.md](../examples/commands/gitsync.md): a shell command.
- [examples/commands/fixup.md](../examples/commands/fixup.md): an NL command.

## Skills (model-invoked)

Where slash-commands are invoked by you, skills are invoked by the model. This is
Claude Code style progressive disclosure: aishe tells the model each skill's
`name` and `description`, and when your request matches, the model calls a
`use_skill` tool to pull that skill's full instructions into context, then
proceeds. Only the descriptions are always in context; bodies load on demand.

Skills apply in yolo mode (where the model drives tools). Run `aishe skills` or
`/skills` to list what is loaded.

Locations (as with commands, a same-named project skill never replaces the
user's — the user's wins):

- User: `~/.config/aishe/skills/`
- Project: `<current directory>/.aishe/skills/`

Each skill is either `<name>/SKILL.md` or a flat `<name>.md`.

### Project skills are trust-gated

A skill body is instructions handed to the model, and the model loads it mid-loop
where there is no moment left to confirm at. So a project skill from
`.aishe/skills/` is only loaded once its file is trusted: until you run
`aishe trust <file>`, it is dropped entirely — it does not appear in `aishe
skills` and the model cannot `use_skill` it. `aishe skills` tells you which files
are waiting on that. User skills in `~/.config/aishe/skills/` are yours by
construction and are never gated. See [SECURITY.md](../SECURITY.md).

### Format

```md
---
name: rust-release
description: How to cut a Rust release (when to bump, tag, publish)
---
Full instructions the model loads on demand.
1. Run the tests and linters.
2. Bump the version and update the changelog.
3. Tag and push.
```

`name` and `description` come from frontmatter (falling back to the file stem).
Extra keys such as `license` are ignored. The body is required.

### Example in the repo

- [examples/skills/rust-release.md](../examples/skills/rust-release.md)

## Claude Code compatibility

Because the formats match, real Claude Code artifacts work without edits:

```sh
# install a real Anthropic Agent Skill
git clone https://github.com/anthropics/skills /tmp/skills
cp -r /tmp/skills/skills/internal-comms ~/.config/aishe/skills/

# install a community slash command
curl -fsSL https://raw.githubusercontent.com/wshobson/commands/main/tools/code-explain.md \
  -o ~/.config/aishe/commands/code-explain.md
```

aishe reads the `name` and `description` frontmatter and ignores keys it does not
use. Sources of ready-made artifacts:

- [anthropics/skills](https://github.com/anthropics/skills)
- [wshobson/commands](https://github.com/wshobson/commands)
- [awesome-claude-code](https://github.com/hesreallyhim/awesome-claude-code)
