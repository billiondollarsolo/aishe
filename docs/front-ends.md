# Front-ends

aishe runs as your real zsh with an AI layered on top. There is one interactive
front-end (the zsh-PTY wrapper), plus a hook you can add to your own shell, plus
the non-interactive paths (`-c` and piped stdin).

## zsh-PTY front-end (the interactive shell)

Running `aishe` (or `aishe zsh`) launches your real interactive zsh inside a
pseudo-terminal. It loads your full `~/.zshrc` and every plugin you already use,
completely unmodified: zsh-autosuggestions, zsh-syntax-highlighting,
fast-syntax-highlighting, fzf-tab, powerlevel10k, oh-my-zsh, and your
completions. Nothing is forked or reimplemented, so plugin behavior, job control,
and key bindings are identical to your normal shell.

Your configured zsh/Oh My Zsh `HISTFILE` is preserved. If the account has no
persistent zsh history configured, AIShe uses its timestamped history log as the
native zsh history fallback, so Up-arrow and `Ctrl-R` persist across sessions and
binary upgrades. With the default `share_history = true`, concurrently running
AIShe shells also exchange history entries.

**It requires zsh.** If zsh is not installed, aishe tells you to install it
(rather than falling back to a lesser editor). Without zsh you can still use the
non-interactive paths (`aishe -c …`, piped stdin) and the bash hook below.

```sh
aishe        # launch your zsh under aishe
aishe zsh    # the same, explicitly
```

aishe injects a `command_not_found_handler` so natural-language input is routed
to the managed agent engine. OpenCode stays behind AIShe's private loopback
supervisor; no second process UI or TUI appears. Suggested commands pre-fill
your next prompt. Set `AISHE_MODE` to
`suggest`, `auto`, or `yolo` to control behavior (`pty_prompt = true` shows a
branded prompt whose glyph reflects the mode). The hook ergonomics described in
[Shell integration](shell-integration.md) apply here, since the PTY wrapper
injects the same hook.

**The branded prompt overrides your own.** By default (`pty_prompt = true`) aishe
replaces your zsh prompt with its `<cwd> <glyph>` plus configurable status line,
so any git-aware segments (branch, dirty, ahead/behind) from powerlevel10k or a
similar theme are hidden while you are in aishe. Only the *prompt* is overridden:
everything else is your real zsh, so zsh-autosuggestions, zsh-syntax-highlighting,
your completions, fzf-tab, and oh-my-zsh all behave exactly as usual. To keep your
own prompt, set `pty_prompt = false`. This is recommended for powerlevel10k users
in particular, since p10k's instant-prompt and transient-prompt can otherwise
conflict with the branded prompt.

On a minimal account with no syntax-highlighting plugin, aishe supplies a
route-aware fallback: complete command-shaped input is green and recognized
natural-language questions are magenta. It evaluates the full buffer, so
`what --version` stays a command while `what is the capital of France?` changes
to the LLM route/color even if `what` is installed. It automatically gets out
of the way when zsh-syntax-highlighting or fast-syntax-highlighting is loaded.
Set `AISHE_COMMAND_HIGHLIGHT=0` to disable the fallback.

Color is not the only route signal. Press **Ctrl-X ?** to display
`aishe route: agent` or `aishe route: shell/local` for the current zsh buffer
without submitting or replacing it.

The branded prompt also has a configurable live status display below the
editable command; `off` hides it. Choose its ordered fields during setup or in
`aishe settings`. Fields include model, mode, backend, scope,
network, sandbox, task, elapsed time, latest context tokens, call/session
tokens/cost, budget, and request count.

The router recognizes a conservative set of full-line question forms beginning
with collision-prone commands such as `what`, `where`, and `who`. Ambiguous
imperatives such as `find large files` or `install kubectl please` remain
**shell** commands when the first word is a real binary (`install` is
`/usr/bin/install`). To force any line to the AI, **start it with `?`** (e.g.
`? install kubectl please`); the sigil is stripped before zsh sees it. The
zsh/Rust `#` spelling is a deprecated compatibility alias and remains an
ordinary comment in Bash. `?` is the reliable path on every supported tier.

Optional force-NL key: Meta/Alt+Return on zsh (`AISHE_NL_KEY`, default `^[^M`).
On Mac that is **Option+Return**, only if the terminal treats Option as Meta
(iTerm: Option → Esc+; Terminal.app: “Use Option as Meta key”). Prefer `?` if
keys are unreliable. Details:
[Shell integration — force-NL](shell-integration.md#force-nl-and-input-prefixes).
**Shift-Tab** (or `AISHE_MODE_KEY`) cycles mode
(`suggest -> auto -> yolo`); the prompt glyph updates to match.

When a command **fails**, press **Ctrl-X Ctrl-F** (or `AISHE_FIX_KEY`) to ask the
model for a corrected command — it is pre-filled on your line for review, never
run automatically. Set `AISHE_AUTODIAGNOSE=1` to also print a one-line hint after
any failure pointing at the fix key. Bash key support is version-dependent and
must pass the [Tier-B matrix](bash-compatibility.md).

## Native zsh/bash hook

If you prefer to keep your own shell session rather than launching aishe, add the
hook to your shell startup instead:

```sh
# ~/.zshrc
eval "$(aishe init zsh)"

# ~/.bashrc
eval "$(aishe init bash)"
```

The zsh hook installs the same routing contract used by the zsh-PTY wrapper.
The Bash hook is Tier B on Bash 5.x and reduced Tier B- on Bash 3.2: `#` remains
a Bash comment, its command-not-found and Readline facilities differ by version,
and it must pass its declared interactive matrix before a release claims
support. Separate
hook processes share one durable managed conversation through the
shell/workspace mapping. The Bash hook is the way to use AIShe interactively
without zsh, subject to the tested version matrix. Full details, including how
state changes persist across the subshell handoff, are in
[Shell integration](shell-integration.md); test scope and current evidence are
in [Native Bash hook compatibility](bash-compatibility.md).

## Non-interactive

`aishe -c '<line>'` runs a single line and exits, and piped stdin (`echo … |
aishe`) runs each line like a `-c` invocation. These use aishe's in-process
executor and dispatcher (zsh, falling back to bash), so they work without an
interactive terminal and without zsh present. Natural-language lines are answered
or, in suggest mode, printed as a proposed command. A natural-language turn may
lazy-start the managed backend; a direct shell line never does.
