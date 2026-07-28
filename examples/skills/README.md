# Example aishe skills (model-invoked)

Copy a skill into aishe's `skills/` directory (user) or
`<project>/.aishe/skills/` (project) as either `<name>.md` or `<name>/SKILL.md`,
then restart aishe.

The user directory follows the platform convention — `~/.config/aishe/skills/` on
Linux, `~/Library/Application Support/aishe/skills/` on macOS. A skill left in
the wrong one is silently ignored; `aishe doctor` prints the resolved config
path. See [docs/configuration.md](../../docs/configuration.md#file-locations).

A **project** skill is trust-gated: it is dropped entirely (not listed, not
loadable by the model) until you run
`aishe trust .aishe/skills/<name>/SKILL.md`. User skills are never gated.

Unlike `/commands` (which you invoke), **skills are invoked by the model**: in
yolo mode aishe lists each skill's `name: description`, and the model calls the
`use_skill` tool to load a skill's full instructions when your request matches.
This is Claude Code style progressive disclosure.

```md
---
name: rust-release
description: How to cut a Rust release (when to bump, tag, publish)
---
<full instructions the model loads on demand>
```
