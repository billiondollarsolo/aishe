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

### mode-cycle keybinding

Press **Shift-Tab** to rotate the interaction mode for the session
(`suggest -> auto -> yolo -> suggest`), like Claude Code. The prompt glyph
updates (`❯` suggest, `»` auto, `⚡` yolo) and the new mode is shown. In zsh,
Shift-Tab still navigates an open completion menu first; it only cycles the mode
when no menu is showing. This changes how the *next* natural-language line routes;
the safety gate and `yolo_confirm` tier always still apply, so it never bypasses a
confirmation. Override the key:

```sh
export AISHE_MODE_KEY='^[[Z'   # zsh bindkey sequence (default Shift-Tab)
```

(bash binds the same to `\e[Z`; re-bind with `bind -x` if you need a different key.)

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

This applies whether you launch the zsh-PTY shell or add the hook to your own
shell. Both run your real `~/.zshrc` (or `~/.bashrc`), so a ready-to-copy example
of `.aishrc` is at [examples/aishrc](../examples/aishrc).

### Interactive definitions persist

Because aishe runs your genuine zsh (or bash), aliases, shell options (`setopt`/
`unsetopt`), and functions you define interactively are owned by your real shell
and persist natively — there is nothing for aishe to replay.

The one gap is the non-interactive paths (`aishe -c …`, piped stdin), which run
each line in a fresh shell and so do not see your interactive definitions. Put
anything you want available there in `.aishrc`.

## History and completion

In the zsh-PTY shell and the native hook, history, `Ctrl-R` search, history
expansion (`!!`, `!$`, ...), and tab completion are all your real shell's own —
unmodified, with every plugin.

Separately, aishe keeps a timestamped history file at
`~/.local/share/aishe/history` (zsh `EXTENDED_HISTORY` format) that backs its
own `history` builtin in the `-c` and hook paths. `share_history` (default on)
shares it across sessions; turn it off for per-session (pid-suffixed) files.

### Semantic history search (opt-in)

`Ctrl-R` matches on substrings. Semantic history search matches on *meaning*, so
"the docker run with the prometheus volume" finds the command even if you don't
remember its exact words. It is **off by default** and never embeds anything until
you explicitly index.

Enable it in your config and pick an embedding provider (Anthropic has no
embeddings endpoint, so point it at OpenAI or a local Ollama):

```toml
[aishe]
semantic_history = true
embedding_provider = "openai"            # or a local Ollama block
embedding_model    = "text-embedding-3-small"   # nomic-embed-text for Ollama
```

Then build the index from your history log and search it:

```sh
aishe history index                 # embed new commands (incremental)
aishe history index --rebuild       # re-embed everything from scratch
aishe history search "the kubectl rollout for the api deployment"
aishe history search "docker volume mount" -n 10
```

`index` only embeds commands not already in the store, so re-running it is cheap.
The vectors live in a local, capped, rebuildable store at
`~/.local/share/aishe/history.vec` (the newest ~5000 commands). With a local
Ollama embedder the whole feature stays offline — your history never leaves the
machine. `search` prints the closest past commands with a similarity score; an
interactive key binding that pre-fills the chosen command is a planned follow-up.
