# Shell integration and .aishrc

This page covers the native zsh/bash hook and the `.aishrc` startup file.

## Native hook: eval "$(aishe init zsh)"

If you want to keep your own shell and its line editor, with your real
zsh-autosuggestions, zsh-syntax-highlighting, oh-my-zsh theme, completions, and
ZLE widgets, add the hook to your shell startup:

```sh
# ~/.zshrc
eval "$(aishe init zsh)"

# ~/.bashrc
eval "$(aishe init bash)"
```

This installs a `command_not_found_handler` (zsh) or `command_not_found_handle`
(bash) that routes anything that is not a command to aishe. Your shell's line
editor is untouched, so every native plugin works exactly as before. Set the mode
with:

```sh
export AISHE_MODE=suggest    # suggest | auto | yolo
```

### Behavior by mode

- **suggest**: zsh pre-fills your next prompt so you can confirm or edit; bash
  prints the suggestion (recall it with `Ctrl-X Ctrl-R`).
- **auto**: a command the safety gate deems safe runs in your real shell, so `cd`
  and `export` persist and it lands in your history. A dangerous command is
  offered for review instead.
- **yolo**: the agentic loop runs directly.

### force-NL keybinding

Sometimes your input is a valid command but you mean it as natural language.
Press Alt-Enter (zsh) or Ctrl-G (bash) to send the current line to the LLM as a
request and replace it with the suggestion. Override the zsh key with a `bindkey`
sequence:

```sh
export AISHE_NL_KEY='^o'
```

### How it works

Shells run the not-found handler in a subshell, so it cannot touch the line editor
or shell state directly. aishe writes the suggested action to a temporary file,
and a `precmd` (zsh) or `PROMPT_COMMAND` (bash) hook acts on it in the main shell.
That is what makes prefill and state changes such as `cd` work.

This hook approach is intentionally chosen over wrapping zsh in a PTY for this
mode: it gives the same real ZLE and native plugins without a second terminal
layer. If you would rather have aishe own the loop and drive your zsh in a PTY,
use the zsh-PTY front-end instead (see [Front-ends](front-ends.md)).

## Startup file (.aishrc)

aishe sources `~/.aishrc` and `~/.config/aishe/aishrc` (in that order) into every
delegated command, so aliases, functions, and exports you define there are
available to all commands and recognized at the prompt.

```sh
# ~/.aishrc
alias gs='git status'
alias ll='ls -lah'
export EDITOR=nvim
gco() { git checkout "$@"; }
```

This is shell-agnostic setup that applies in both front-ends. The zsh-PTY
front-end also runs your real `~/.zshrc`. A ready-to-copy example is at
[examples/aishrc](../examples/aishrc).

### Interactive definitions persist too

In the reedline front-end, aliases, shell options (`setopt` and `unsetopt`), and
functions you define interactively persist to later commands in the same session.
aishe replays the definition through the same mechanism. Multi-line function
bodies continue until the braces close, then become callable.

The remaining gap is functions and aliases created by a file you `source` at
runtime: only that file's environment changes are captured. Put such definitions
in `.aishrc`, or use the zsh-PTY front-end.

## History and completion

In the reedline front-end:

- History is stored at `~/.local/share/aishe/history` and persists across
  restarts.
- `Ctrl-R` opens a browsable, filterable history menu.
- History expansion supports `!!`, `!$`, `!^`, `!*`, `!-N`, `!!:N` word
  selection, and `^old^new` quick substitution. Note that `!cmd` stays aishe's
  force-shell prefix, so `!`-prefix history matching is intentionally not used.
- Tab completion is context-aware. See [Front-ends](front-ends.md) for the full
  list.
