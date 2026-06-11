# Front-ends

A front-end is the input loop aishe presents. There are three, in increasing
order of real-zsh fidelity. The active one is chosen by `front_end` in the config
(default `auto`), or per session with `--pty` / `--no-pty`.

```toml
[aishe]
front_end = "auto"   # auto | reedline | zsh-pty
```

- `auto` (default): the zsh-PTY front-end. **It requires zsh**; if zsh is not
  installed, aishe tells you to install it rather than silently falling back.
- `zsh-pty`: same as `auto`.
- `reedline`: the opt-in built-in editor (also `aishe --no-pty`), for the rare
  environment where zsh cannot be installed. It reimplements shell features and
  is more sensitive to terminal quirks, so it is not the default.

## zsh-PTY front-end

This launches your real interactive zsh inside a pseudo-terminal. It loads your
full `~/.zshrc` and every plugin you already use, completely unmodified:
zsh-autosuggestions, zsh-syntax-highlighting, fast-syntax-highlighting, fzf-tab,
powerlevel10k, oh-my-zsh, and your completions. Nothing is forked or
reimplemented, so plugin behavior is identical to your normal shell.

```sh
aishe              # auto picks zsh-PTY when zsh is present
aishe zsh          # force it
aishe --pty        # same as above for one session
```

aishe injects a `command_not_found_handler` so natural-language input is still
routed to the LLM. Suggested commands pre-fill your next prompt. Set `AISHE_MODE`
to `suggest`, `auto`, or `yolo` to control behavior. The hook ergonomics
described in [Shell integration](shell-integration.md) apply here too, since the
PTY wrapper injects the same hook.

Because that routing is by command name, a question whose first word is a real
command (`who`, `which`, `find`, `time`, `test`, `make`) would otherwise run that
command. To force a line to the AI, **start it with `?` or `#`** (e.g.
`? who was the first man on the moon`); the sigil is stripped by the line editor
before zsh sees it, so the shell's comment and glob rules never apply. The
force-NL key (Alt-Enter, or `AISHE_NL_KEY`) does the same for the line you are
editing.

Use this front-end when you want your real shell experience with aishe layered on
top. It also gives you real job control, because it is genuinely your zsh.

## Built-in reedline editor

The reedline front-end is a self-contained line editor (the editor behind
nushell). It reimplements the most-loved zsh niceties natively. Force it with:

```sh
aishe --no-pty           # or front_end = "reedline"
```

Features:

- **Tab completion** that is context-aware: command names at the command
  position (including after a pipe), environment variables for `$VAR` (with
  values shown), directories-only for `cd`, `pushd`, and `rmdir`, subcommands for
  `git`, `cargo`, `docker`, and `npm` (plus live branch names for git checkout,
  switch, merge, and rebase), aishe meta subcommands and their values, and file
  paths elsewhere. Matching is case-insensitive with a fuzzy subsequence fallback
  (`gco` matches `git-checkout`).
- **History autosuggestions**, fish-style inline hints from your history.
- **History search** with `Ctrl-R`, a browsable filterable menu.
- **History expansion**: `!!`, `!$`, `!^`, `!*`, `!-N`, `!!:N` word selection,
  and `^old^new` quick substitution.
- **Multi-line continuation** for unterminated shell lines (open quote, trailing
  backslash, unbalanced parens, an open function body, or an open control
  structure). Natural-language input is never trapped, so `what's eating my disk`
  still submits.
- **Syntax highlighting** of the command head, flags, quoted strings, operators,
  paths, env assignments, and the `?` and `!` sigils. Themeable, see
  [Prompt and theming](prompt-and-theming.md).
- **autocd**: type a bare directory name to cd into it.
- **Directory stack**: `pushd`, `popd`, and `dirs` persist across commands. With
  `auto_pushd = true` (zsh `AUTO_PUSHD`), every `cd` pushes the previous directory
  onto the stack; navigate it with `cd -N` / `cd +N` and list it with `dirs -v`
  (numbered).
- **cdpath**: `cd <name>` also searches the `cdpath` base directories (or
  `$CDPATH`) when the name is not under the current directory.
- **Named directories**: `cd ~proj` / `cd ~proj/app` when `[named_dirs]` maps
  `proj` to a path.
- **Spelling correction** (`correct = true`, zsh `CORRECT`): when the first word
  is a near-miss of a known command (for example `gti status`), aishe offers
  `correct 'gti' to 'git'? [Y/n]` instead of sending the line to the LLM.
- **History filtering**: consecutive-duplicate removal, ignore-space, and
  `HISTIGNORE` glob patterns (see
  [Shell integration and .aishrc](shell-integration.md)).
- **Command duration and git status in the prompt** (see
  [Prompt and theming](prompt-and-theming.md)).
- **emacs or vi keymap** via `edit_mode` (or `aishe editor vi`). In vi mode the
  prompt shows `[I]` and `[N]` for insert and normal. Takes effect next session.
- **Custom prompt** via `prompt_format`, and a git branch segment in the right
  prompt.

Limitations of this front-end: no job control (`Ctrl-Z`, `bg`, `fg`) for
delegated processes, because each command runs as a fresh shell invocation.
`Ctrl-C` reaches the foreground child, and aishe itself survives. For full job
control, use the zsh-PTY front-end.

## Native zsh/bash hook

If you prefer to keep your own shell and its editor, add the hook to your shell
startup instead of running aishe as the front-end:

```sh
# ~/.zshrc
eval "$(aishe init zsh)"

# ~/.bashrc
eval "$(aishe init bash)"
```

This installs a not-found handler that routes anything that is not a command to
aishe, while your shell's line editor and every native plugin stay untouched.
Full details, including the force-NL keybinding and how state changes persist, are
in [Shell integration](shell-integration.md).
