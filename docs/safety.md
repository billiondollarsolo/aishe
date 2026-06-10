# Safety gate

aishe runs a deterministic safety gate before executing any command the model
proposed. The model's output is never trusted to be safe; a separate, rule-based
check in aishe decides what actually runs.

## When the gate applies

- **suggest**: a dangerous proposed command is flagged before you confirm.
- **auto**: safe commands run immediately; dangerous ones stop and require you to
  type the full word `yes`.
- **yolo**: when `yolo_confirm_dangerous = true`, the loop pauses for dangerous
  tool calls.

The gate does not apply to commands you type yourself, nor to `!`-forced lines.
It is a shell, not a nanny: if you type a destructive command directly, it runs.

## What is flagged

The gate screens for irreversible or high-impact operations, including:

- recursive force deletes (`rm -rf`) of risky targets (see below),
- raw device writes (`dd of=/dev/...`, `> /dev/sd...`),
- filesystem creation (`mkfs`),
- fork bombs,
- piping a download straight into a shell (`curl ... | sh`, `wget ... | bash`),
- force pushes to main or master (`git push --force`),
- system power changes (`shutdown`, `reboot`),
- mass truncation with globs,
- and more.

`sudo` prefixes are recognized so wrapping a dangerous command in `sudo` does not
slip past the gate.

## Path-aware rm -rf

Recursive force deletes are judged by their targets, lexically, without touching
the filesystem:

- An in-tree relative path such as `rm -rf node_modules`, `rm -rf build dist`, or
  `rm -rf ./target` is treated as your own project files and runs without fuss.
- Anything catastrophic or out-of-tree is flagged: an absolute path (`/var`), a
  home path (`~`, `$HOME`), a variable, a bare glob (`*`), or an escaping `..`
  path.

This keeps everyday cleanup smooth while still catching the dangerous cases.

## How a flagged command looks

A dangerous command prints a red panel describing why it was flagged and requires
you to type `yes` to proceed. Anything else cancels it.

## Forcing past the gate

If you are certain, prefix the line with `!` to run it as shell and skip the gate:

```
!rm -rf /tmp/scratch
```

Use this deliberately. The gate exists to catch mistakes, especially commands a
model proposed.

## Secrets in the model context

aishe sends an environment context block (including your recent commands) with
each request. To avoid leaking credentials that appear in those commands, it
redacts likely secrets before sending. This is on by default. See
[Logging and privacy](logging.md).
