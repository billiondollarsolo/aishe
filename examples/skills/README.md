# Example aishe skills (model-invoked)

Copy a skill into `~/.config/aishe/skills/` (user) or `<project>/.aishe/skills/`
(project) as either `<name>.md` or `<name>/SKILL.md`, then restart aishe.

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
