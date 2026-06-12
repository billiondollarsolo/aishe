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

**It requires zsh.** If zsh is not installed, aishe tells you to install it
(rather than falling back to a lesser editor). Without zsh you can still use the
non-interactive paths (`aishe -c …`, piped stdin) and the bash hook below.

```sh
aishe        # launch your zsh under aishe
aishe zsh    # the same, explicitly
```

aishe injects a `command_not_found_handler` so natural-language input is routed
to the LLM. Suggested commands pre-fill your next prompt. Set `AISHE_MODE` to
`suggest`, `auto`, or `yolo` to control behavior (`pty_prompt = true` shows a
branded prompt whose glyph reflects the mode). The hook ergonomics described in
[Shell integration](shell-integration.md) apply here, since the PTY wrapper
injects the same hook.

**The branded prompt overrides your own.** By default (`pty_prompt = true`) aishe
replaces your zsh prompt with its `<cwd> <glyph>` + `model · mode` line, so any
git-aware segments (branch, dirty, ahead/behind) from powerlevel10k or a similar
theme are hidden while you are in aishe. Only the *prompt* is overridden:
everything else is your real zsh, so zsh-autosuggestions, zsh-syntax-highlighting,
your completions, fzf-tab, and oh-my-zsh all behave exactly as usual. To keep your
own prompt, set `pty_prompt = false`. This is recommended for powerlevel10k users
in particular, since p10k's instant-prompt and transient-prompt can otherwise
conflict with the branded prompt.

Because routing is by command name, a question whose first word is a real command
(`who`, `which`, `find`, `time`, `test`, `make`) would otherwise run that command.
To force a line to the AI, **start it with `?` or `#`** (e.g. `? who was the first
man on the moon`); the sigil is stripped by the line editor before zsh sees it, so
the shell's comment and glob rules never apply. The force-NL key (Alt-Enter, or
`AISHE_NL_KEY`) does the same for the line you are editing. **Shift-Tab** (or
`AISHE_MODE_KEY`) cycles the interaction mode for the session
(`suggest -> auto -> yolo`); the prompt glyph updates to match.

## Native zsh/bash hook

If you prefer to keep your own shell session rather than launching aishe, add the
hook to your shell startup instead:

```sh
# ~/.zshrc
eval "$(aishe init zsh)"

# ~/.bashrc
eval "$(aishe init bash)"
```

This installs a not-found handler that routes anything that is not a command to
aishe, while your shell's line editor and every native plugin stay untouched. It
is the same hook the zsh-PTY wrapper injects, so the `?`/`#` sigils, the force-NL
key, and the Shift-Tab mode cycle all work here too. The bash hook is the way to
use aishe interactively without zsh. Full details, including how state changes
persist across the subshell handoff, are in
[Shell integration](shell-integration.md).

## Non-interactive

`aishe -c '<line>'` runs a single line and exits, and piped stdin (`echo … |
aishe`) runs each line like a `-c` invocation. These use aishe's in-process
executor and dispatcher (zsh, falling back to bash), so they work without an
interactive terminal and without zsh present. Natural-language lines are answered
or, in suggest mode, printed as a proposed command.
