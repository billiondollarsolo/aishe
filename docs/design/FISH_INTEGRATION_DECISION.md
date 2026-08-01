> **Lifecycle: Active.** Baseline: AIShe v0.6.5 (`4a2c7e4`), 2026-08-01.

# Fish native integration decision

## Decision

**No-build for a native Fish hook in the current milestone.** AIShe will keep
shipping Fish command completions, but it will not claim interactive Fish
routing, `aishe init fish`, agent-suggestion staging, or state persistence.

This is a deliberate scope decision, not evidence that Fish cannot be
supported. Reconsider only after the entry gates below are met and a Fish-native
prototype passes its own PTY matrix.

## What exists now

Releases generate `aishe completions fish`, and packages may install the emitted
completion file. That file completes AIShe's CLI subcommands and options. It
does not intercept Fish command lines or implement the zsh/Bash integration
contract.

The current generated native hooks are only `aishe init zsh` and
`aishe init bash`; unsupported shell names fail. Documentation must keep the
words **completion** and **interactive hook** separate.

## Evaluation

### User value

Fish users would gain native editing, history, and shell-state persistence
without launching zsh. The value is real, but the current product has two
qualified integration tiers already: flagship zsh PTY/native zsh and the
explicitly reduced Bash matrix. Adding a third parser/editor contract before
the existing terminal and accessibility work is stable would increase the
release surface more than the present demand evidence justifies.

### Architecture

Fish is not a POSIX-compatible syntax variant of the existing hooks. A credible
implementation would be generated Fish code using Fish's own `commandline` and
`bind` facilities, not a translated zsh/Bash string. Fish documents that
`commandline` reads or replaces its editor buffer and that custom bindings can
be installed in `fish_user_key_bindings`. Fish also exposes `fish_preexec` and
`fish_postexec` events, while command-not-found behavior is a named function.
Those are useful primitives but do not by themselves reproduce AIShe's route,
staged-buffer, or parent-shell state-handoff semantics.

Relevant upstream references:

- [Fish interactive use and custom bindings](https://fishshell.com/docs/current/interactive.html)
- [Fish language event handlers](https://fishshell.com/docs/current/language.html#event-handlers)
- [`commandline` buffer API](https://fishshell.com/docs/current/cmds/commandline.html)
- [`fish_command_not_found`](https://fishshell.com/docs/current/cmds/fish_command_not_found.html)

The architecture spike must decide whether routing happens in an Enter binding,
command-not-found function, or a bounded combination. It must also define
interaction with Emacs/vi modes, user bindings, abbreviations, multiline input,
syntax errors, autosuggestions, and Fish's evolving keyboard protocol.

### Security and privacy

A Fish hook must preserve the same invariants as the qualified zsh path:

- direct shell commands and explicit paths remain local and are never submitted
  to a model merely because Fish syntax differs;
- `?` is stripped only after an explicit agent route, while ordinary Fish
  comments and escapes retain native meaning;
- suggest mode stages editable buffer text and never executes on the first
  agent submission;
- auto/yolo authority cannot be broadened by a binding or command-not-found
  fallback;
- state handoff files are private, session-scoped, atomic, and cleaned up; and
- shell history, secrets, substitutions, redirections, and multiline buffers
  are not sent or logged beyond documented policy.

Reusing Bash quoting or `eval` behavior would fail this review.

### Packaging

If built later, Fish integration needs a first-class generated hook, CLI
validation for `init fish`, uninstall behavior, docs, completion coexistence,
and release-package placement. The existing `aishe.fish` completion path must
not be overwritten by or confused with a hook file. Installers must not modify
`config.fish` without explicit user action.

### Test and maintenance cost

A build decision requires an interactive Fish harness covering at least:

- supported minimum/current Fish versions, including both sides of major
  binding-protocol changes;
- Emacs and vi insert/normal modes and existing user bindings;
- direct commands, functions, builtins, abbreviations, explicit paths, globs,
  pipes, redirections, substitutions, multiline syntax, and syntax errors;
- collision-prone English, explicit route prefixes, staged editing, history,
  completion, cancellation, Ctrl-C/Ctrl-Z, resize, and 300 ms escape latency;
- `cd`, `export`/Fish variable equivalents, connection/model selection, and
  other parent-shell state handoff;
- tmux, screen, macOS/Linux PTYs, `TERM=dumb`, `NO_COLOR`, and narrow widths;
- install, upgrade, uninstall, shell-startup latency, and no-network direct
  command behavior.

This matrix is independent of Fish CLI completion generation.

## Reconsideration gates

Open a build proposal only when all of these are true:

1. Demand evidence identifies enough Fish users and their required interaction
   modes to justify a maintained tier.
2. A short architecture record selects the Fish-native interception and state
   model, including collision and security analysis.
3. A disposable prototype proves editable suggest staging without first-Enter
   execution and preserves direct Fish semantics.
4. CI can install at least the declared minimum and current Fish versions on
   Linux, with a named macOS qualification path.
5. Documentation and packaging can label completion versus hook unambiguously.

Until then, the product statement is: **Fish completions are available; a Fish
interactive hook is not supported.**
