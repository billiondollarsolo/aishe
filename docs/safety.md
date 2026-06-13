# Safety gate

aishe runs a deterministic safety gate before executing any command the model
proposed. The model's output is never trusted to be safe; a separate, rule-based
check in aishe decides what actually runs.

## When the gate applies

- **suggest**: a dangerous proposed command is flagged before you confirm.
- **auto**: safe commands run immediately; dangerous ones stop and require you to
  type the full word `yes`.
- **yolo**: the loop pauses for tool calls according to the `yolo_confirm` tier
  (`"never"` / `"dangerous"` / `"writes"` / `"all"`; default `"dangerous"`). The
  legacy `yolo_confirm_dangerous` boolean is still honored when `yolo_confirm` is
  left at its default. See [Modes: yolo](modes.md#yolo).

The gate does not apply to commands you type yourself, nor to `!`-forced lines.
It is a shell, not a nanny: if you type a destructive command directly, it runs.

## What is flagged

The gate screens for irreversible or high-impact operations, including:

- recursive force deletes (`rm -rf`) of risky targets (see below),
- raw device writes and disk tools (`dd of=/dev/...`, `> /dev/sd...`, `mkfs`,
  `fdisk`, `parted`, `wipefs`, `shred /dev/...`, `diskutil erase`),
- recursive permission or ownership changes on root (`chmod -R ... /`),
- fork bombs,
- piping a download straight into a shell (`curl ... | sh`, `wget ... | bash`),
- git history or working-tree loss (`git push --force` to main/master,
  `git reset --hard`, `git clean -f`),
- system power changes (`shutdown`, `reboot`, `halt`, `kill -9 1`),
- mass delete (`find / ... -delete`) and mass truncation with globs,
- and more.

The gate is robust against common ways of hiding a dangerous command. It strips
leading wrappers and environment assignments before judging the real command, so
`sudo -i rm -rf /`, `FOO=bar rm -rf /`, `env rm -rf /`, `time rm -rf /`,
`nice rm -rf /`, and `timeout 5 rm -rf /` are all caught. It also unquotes `rm`
targets, so `rm -rf "$HOME"` and `rm -rf '/'` do not slip through. A large
adversarial test corpus (`tests/safety_corpus.rs`) guards these.

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

## Sandbox (policy-based, best-effort)

yolo mode has an optional sandbox (`yolo_sandbox = true`, off by default; toggle
with `aishe sandbox on`). When on, before a `run_command` runs, aishe classifies
the command and refuses it - feeding the reason back to the model as the tool
result instead of executing - if it:

- **accesses the network**: `curl`, `wget`, `ssh`, `scp`, `sftp`, `nc`/`ncat`,
  `telnet`, `ftp`, `rsync`, and the network subcommands of package managers and
  git (`git clone`/`fetch`/`pull`/`push`, `npm install`, `pip install`,
  `cargo install`, `apt-get install`, and similar), or
- **writes outside the working tree**: redirection (`> /etc/x`, `>> ~/y`) or an
  obvious out-of-tree write command (`cp`/`mv`/`tee`/`touch`/`dd of=`/...) whose
  target is an absolute path, a `~` home path, or a `..`-escaping path.

A refusal looks like `Refused by sandbox: <reason>. Sandbox mode is on
(yolo_sandbox).`, so the model can adapt (for example by staying in-tree or by
asking you to run the network step yourself).

This is a **policy-based, best-effort** check on the command text the model
proposed. It is **not a kernel sandbox**: it cannot stop a determined escape via
a wrapper script, an alias, a path hidden in a shell variable, or anything that
does not look like the patterns above. It also does **not** affect the zsh-PTY /
real-shell front-end paths (those run your own typed commands, not model tool
calls). It is one more guardrail for an autonomous loop, not a security boundary.

### A real sandbox: `sandbox_backend = "bwrap"`

For genuine isolation, set `sandbox_backend = "bwrap"` (with `yolo_sandbox =
true`). When [bubblewrap](https://github.com/containers/bubblewrap) (`bwrap`) is
installed, every `run_command` then runs with a **read-only root** and only the
**working tree** and `/tmp` writable — so a yolo command *physically cannot*
modify the system (`/etc`, `/usr`, your home), no matter how it's written. This
replaces the advisory policy check above with OS-enforced isolation. Network is
left intact (reads and lookups still work); the guarantee is that **writes can't
escape the working tree**, which is the damage that matters.

If `bwrap` isn't installed, aishe degrades to the policy gate and says so (`aishe
doctor` shows which backend is active). The bwrap backend is Linux-only; on macOS,
use the policy backend. (Because the root is read-only, commands that install
system packages will fail inside it — that's the point; run those outside the
sandbox or with the policy backend.)

## Reversible preview: `aishe dry-run`

To see *exactly* what a command would change before letting it touch anything:

```sh
aishe dry-run "make build && ./configure"     # preview file changes, discard them
aishe dry-run --apply "sed -i s/foo/bar/ *.c"  # preview, then keep the changes
```

`dry-run` copies the working tree to a throwaway staging dir and runs the command
there under bubblewrap — a **read-only root with the network disabled** — so the
command really executes but its writes are confined to the copy and it has no
external side effects. aishe then diffs the copy against the real tree and prints
the added / modified / deleted files with unified diffs. By default the changes
are **discarded** (nothing was touched); pass `--apply` to copy them onto the real
tree. An applied dry-run is journaled, so `aishe undo` reverts the whole batch.

Notes: it needs `bubblewrap` (`aishe doctor` shows availability) and is Linux-only.
Because there's no kernel overlay it copies the tree, so a very large tree (or a
huge `.git`) is refused — run it in a smaller subdirectory. Symlinks aren't copied.
This is the reversible-preview building block the agentic loop will grow into
(plan → preview → apply/undo).

## Defense-in-depth, not a sandbox

The destructive-command gate is conservative pattern plus path-aware matching. It
is a **backstop against mistakes**, not a security boundary. The segmentation is
quote- and nesting-aware and recurses into substitutions — command substitution
(`$(…)`, backticks), subshells, and process substitution (`<(…)`, `>(…)`) all
have their inner commands assessed, so a dangerous command hidden inside one
(`cat <(rm -rf /)`) is still caught. Here-documents are handled too: a body fed to
a shell (`bash <<EOF … rm -rf / … EOF`) is assessed as commands, while a body
written verbatim by `cat`/`tee` is treated as data (so writing an install script
that itself contains `curl … | sh` isn't a false positive). But it can still be
evaded: an unusual interpreter, or sufficient obfuscation, can slip past patterns
that look for the obvious shapes. Treat it as defense-in-depth, not a sandbox.

This matters most in `auto` and `yolo`, where commands can run without a
per-command confirmation. In `suggest` mode nothing runs until you explicitly
confirm it, so the gate is only an extra warning. For untrusted input or a
fully-autonomous loop, do not rely on the gate alone — raise the confirmation tier
and/or turn on the sandbox:

- Raise [`yolo_confirm`](configuration.md#aishe-section) to `"writes"` (confirm
  any state-modifying command) or `"all"` (confirm every command).
- Enable [`yolo_sandbox`](configuration.md#aishe-section) (`= true`), the
  policy sandbox that refuses network access and out-of-tree writes (see the
  previous section; toggle live with `aishe sandbox on`).

The gate may improve over time (for example, inspecting inside `$(…)`), but the
guidance is evergreen: it is a backstop, and for autonomous or untrusted use you
should raise the tier rather than trust pattern-matching to be exhaustive.

## Secrets in the model context

aishe sends an environment context block (including your recent commands) with
each request. To avoid leaking credentials that appear in those commands, it
redacts likely secrets before sending. This is on by default. See
[Logging and privacy](logging.md).
