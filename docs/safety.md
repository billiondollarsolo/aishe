# Safety gate

aishe runs a deterministic safety gate before executing any command the model
proposed. The model's output is not taken as authorization: a separate, rule-based
check screens it first and asks you about anything it recognizes as destructive, or
cannot resolve at all.

The gate is a **heuristic screen, not a security boundary**. It matches patterns
against command *text*; it never executes or simulates a command, so it cannot prove
anything about what one would do. What it is good at is catching mistakes — the far
more common failure — without becoming noise. `tests/safety_corpus.rs` pins both
halves of that on every build: an adversarial corpus where every entry (including
every bypass shape found against the gate so far) must be flagged, and an
everyday-command corpus where none may be. Measured against a separately generated
corpus of ordinary commands, it asks about fewer than one in two hundred. What it
structurally cannot see is in
[What the gate does not catch](#what-the-gate-does-not-catch), and the layer
you actually rely on for isolation is the
[sandbox](#a-real-sandbox-sandbox_backend--bwrap).

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

## Three outcomes

Every screened command lands in exactly one of three buckets:

| Outcome | Meaning | What happens |
| --- | --- | --- |
| **safe** | Nothing matched, and the gate could resolve every segment. | Runs per the mode (immediately in `auto`; after your confirmation in `suggest`). |
| **unknown** | The gate could not work out what a segment would run. | Fails closed: a yellow "could not verify" panel and the same confirmation a flagged command gets. Nothing auto-runs. |
| **dangerous** | A rule matched a destructive shape. | Red panel with the reason; you must type the full word `yes`. |

The important asymmetry: **safe means "nothing matched", not "this is safe"**. The
gate has no way to express confidence, only the absence of a match. `unknown` exists
so that "I can't tell" stops being silently reported as `safe` — see
[Unresolvable commands fail closed](#unresolvable-commands-fail-closed).

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

The gate covers a number of common ways of hiding a dangerous command. It strips
leading wrappers and environment assignments before judging the real command, so
`sudo -i rm -rf /`, `FOO=bar rm -rf /`, `env rm -rf /`, `time rm -rf /`,
`nice rm -rf /`, and `timeout 5 rm -rf /` are all caught. It also unquotes `rm`
targets, so `rm -rf "$HOME"` and `rm -rf '/'` do not slip through. A large
adversarial test corpus (`tests/safety_corpus.rs`) guards these.

## Unresolvable commands fail closed

Segmenting a command line can leave a segment whose *head* — the token that should
be a command name — isn't one: it's empty, a redirect operator (`> /etc/passwd`),
a bare flag, a leftover number from a wrapper (`watch -n 5 …`), an unbalanced-quote
fragment, or an expansion the gate can't evaluate (`$(which rm)`, `${RM:-rm}`).
The gate used to treat those as safe. It now reports them as **unknown** and every
caller fails closed:

- **suggest / auto / yolo**: a yellow "could not verify" panel, same confirmation
  as a flagged command; nothing auto-runs.
- **`aishe suggest --json`**: `risk` is `"unknown"` (alongside the existing
  `"safe"`, `"dangerous"`, `"n/a"`), with the reason in `reason`. The exit code is
  the same `20` used for `dangerous`, so a script that only tests `risk == "safe"`
  or `exit == 0` already fails closed. Same for the `--auto-line` shell hook: `20`
  means "pre-fill for review, don't run".
- **`aishe replay`**: non-interactive, so an unverifiable command is skipped.

This is *not* "the head isn't on a denylist" — `ls`, `git`, `uv`, `npm`, and every
other well-formed command name are still `safe`. It only fires when the gate
genuinely cannot tell what would run.

## What the gate does not catch

This is the section to read before trusting the gate with anything that matters.

The limits below are **classes**, not a list of bugs, and they follow from what the
gate is: a matcher over command text, with no execution, no filesystem lookups, and
no knowledge of any program's behavior. Specific rules get added over time (each new
shape lands in `tests/safety_corpus.rs` so it can't regress), but the classes stay —
closing them would require being a different kind of tool.

- **Wrapper and runner binaries beyond the built-in table.** The gate strips a small,
  known set of wrappers to find the real command underneath. Anything outside that
  table is a well-formed command name whose argument list means nothing to the gate —
  it cannot know that a given binary treats its arguments as a program to exec rather
  than as data. Deciding that in general means resolving `$PATH`, reading the binary,
  and knowing its CLI: not something a text matcher can do.
- **Execution that happens somewhere else.** Remote shells, container and cluster
  exec, and job submitters carry a payload to another machine or namespace. The gate
  assesses the payload *text* for the forms it knows (`ssh [opts] host <cmd>`,
  `docker exec`, `kubectl exec`), but that is a judgment about a string: even a
  perfect local reading says nothing about what the far side does with it, and the
  damage lands where aishe cannot see it.
- **Payloads handed to a non-shell interpreter.** The gate scans the inline program
  text of `python`/`perl`/`node`/`ruby`/`php` for a *literal* destructive shell-out,
  so `python3 -c "os.system('rm -rf /')"` is flagged. That is a substring scan, not a
  parser for six languages: a payload that assembles the command at runtime — base64,
  `pack`, `strrev`, plain concatenation — is not caught, and interpreters outside that
  list (`awk 'BEGIN{system("rm -rf /")}'`) are not scanned at all. Judging this
  properly needs real semantics per language.
- **Content piped into a shell from an opaque source.** When a pipeline's sink is a
  shell, the gate reads the upstream: literal text it can see (`echo 'rm -rf /' | bash`)
  is assessed and the pipeline inherits that verdict, and the downloader-into-shell
  shape (`curl … | bash`) has its own rule. Beyond that the left-hand side produces
  bytes that do not exist until the command runs, so `cat deploy.sh | sh` and
  `base64 -d p.b64 | bash` come back `unknown` — they prompt, they are not blocked.
  A shell reading a *process substitution* rather than a pipe
  (`bash <(curl -sL …)`, `source <(…)`) is not covered by this at all and stays
  `safe`.
- **Code fetched or computed at run time.** A command whose body arrives via
  substitution — from the network, a file, or another program — is at best `unknown`.
  It prompts because the head is unresolvable, not because anything inspected what
  would be fetched. Change the spelling so the head resolves and the prompt goes away.

Two consequences worth stating plainly. First, failing closed on an unresolvable head
narrows these holes but does not close them: someone who picks the spelling can pick a
resolvable one. Second, the gate is aimed at *mistakes* — a model proposing something
destructive by accident — and it is not a defense against an adversary who is choosing
their input, whether that adversary is a person or prompt-injected content steering the
model. For autonomous or untrusted use the real control is `sandbox_backend = "bwrap"`
or `aishe dry-run`, not this gate — see
[Defense-in-depth, not a sandbox](#defense-in-depth-not-a-sandbox).

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

Defense in depth here means three layers, and they are not equal:

1. **The sandbox** (`sandbox_backend = "bwrap"`, Linux) — the only layer enforced by
   the OS rather than by aishe's opinion of a string. Writes cannot leave the working
   tree regardless of how a command is spelled. This is the real boundary.
2. **Confirmation** (`yolo_confirm`, and `suggest`/`auto` prompts) — a human reading
   the command before it runs. Independent of whether the gate understood it, which is
   exactly why `unknown` routes here.
3. **The safety gate** — the weakest of the three, and the only one that can be wrong
   silently. It sees text, matches shapes it was taught, and has the blind spots listed
   [above](#what-the-gate-does-not-catch).

Note the platform asymmetry: **macOS has no sandbox backend**, so on macOS layer 1
does not exist and `yolo` falls back to the gate plus whatever confirmation tier you
set. `aishe doctor` shows which backend is actually active — check it rather than
assuming.

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

The gate will keep improving — new shapes get rules and a corpus entry as they are
found — but the guidance is evergreen: it is a backstop, and for autonomous or
untrusted use you should raise the tier and rely on isolation rather than trust
pattern-matching to be exhaustive.

## Secrets in the model context

aishe sends an environment context block (including your recent commands) with
each request. To avoid leaking credentials that appear in those commands, it
redacts likely secrets before sending. This is on by default. See
[Logging and privacy](logging.md).
