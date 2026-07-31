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

The zsh hook also catches an unknown plain-language question ending in `?`
before zsh's `NOMATCH` glob check runs. The check is deliberately narrow: if the
first word is a real command or an explicit path, zsh still handles `?`, `*`,
and other glob syntax normally.

If the account has no syntax-highlighting plugin, the hook provides a
route-aware fallback over the whole edit buffer. Complete command-shaped input
is green and natural-language questions are magenta. It deliberately
distinguishes collisions such as `what --version` (command) from `what is my IP
address?` (LLM), rather than permanently coloring by the first token. Full
syntax highlighting remains the job of zsh-syntax-highlighting or
fast-syntax-highlighting, and either plugin automatically takes precedence. Set
`AISHE_COMMAND_HIGHLIGHT=0` to turn the fallback off.

### Behavior by mode

- **suggest**: zsh pre-fills your next prompt so you can confirm or edit; bash
  prints the suggestion (recall it with `Ctrl-X Ctrl-R`).
- **auto**: a command the safety gate deems safe runs in your real shell, so `cd`
  and `export` persist and it lands in your history. A dangerous command is
  offered for review instead.
- **yolo**: the managed agent runs inline. The first yolo turn in each new shell
  asks once for workspace/host scope; accepted yolo does not prompt per action.

### Primary slash commands

Inside an Aishe zsh session, `/help` lists the short command surface. `/model`
opens the filterable connection/model picker; Enter changes only this shell,
`d` also saves the durable default, and Esc cancels without changing either.
`/model MODEL` and `/model CONNECTION/MODEL` are direct forms. `/provider` opens
the same picker, `/auth` reports the selected connection's exact auth binding,
and `/status` shows identity, backend readiness, usage, and spend. The shell
handoff writes connection, model, and reasoning together, and a main-shell
prompt hook applies it even when the branded Aishe prompt is disabled.

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
updates (`❯` suggest, `»` auto, `*` yolo) and the new mode is shown. In zsh,
Shift-Tab still navigates an open completion menu first; it only cycles the mode
when no menu is showing. This changes how the *next* natural-language line routes;
changing mode does not grant authority: the first yolo turn still requires
per-shell scope acceptance. Override the key:

```sh
export AISHE_MODE_KEY='^[[Z'   # zsh bindkey sequence (default Shift-Tab)
```

(bash binds the same to `\e[Z`; re-bind with `bind -x` if you need a different key.)

### fix-the-last-command keybinding

When a command fails, press **Ctrl-X Ctrl-F** to ask the model for a corrected
command; it's pre-filled on the line for review and never auto-runs. Override the
key with `AISHE_FIX_KEY` (a zsh `bindkey` sequence). Set `AISHE_AUTODIAGNOSE=1` to
have the prompt hint at it after a non-zero exit.

For sharper fixes, set `fix_capture_stderr = true`: aishe then re-runs the failed
command once to capture its **actual error output** ("unknown option", "no such
file", "not a git repository") and feeds that into the correction prompt. Only
commands the safety gate deems read-only and safe are re-run (bounded by a
timeout), so a destructive or network command is never re-executed — and the
diagnostic run doesn't touch your recorded history.

### semantic-recall keybinding

With [semantic history search](#semantic-history-search-opt-in) enabled, type a
few words describing a past command ("the docker run with the prometheus volume")
and press **Ctrl-X Ctrl-R** to replace the line with the closest past command by
*meaning* — pre-filled for review, never auto-run. It needs `semantic_history =
true` and a built index (`aishe history index`); with the feature off or no match
it leaves the line untouched and shows a brief message. Override the key:

```sh
export AISHE_RECALL_KEY='^X^R'   # zsh bindkey sequence (default Ctrl-X Ctrl-R)
```

(In bash, suggest mode can't pre-fill the line, so `Ctrl-X Ctrl-R` there recalls
the last printed AI suggestion instead — same "recall" mnemonic, per shell.)

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

aishe sources `~/.aishrc` and an `aishrc` in its config directory (in that order)
into every delegated command, so aliases, functions, and exports you define there
are available to all commands and recognized at the prompt. The config directory
is `~/.config/aishe/` on Linux and `~/Library/Application Support/aishe/` on
macOS (see [File locations](configuration.md#file-locations)), so `~/.aishrc` is
the portable choice.

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
expansion (`!!`, `!$`, ...), and tab completion use your real shell and remain
compatible with its plugins. If your zsh/Oh My Zsh configuration sets
`HISTFILE`, Aishe preserves it unchanged.

Separately, Aishe keeps its own timestamped history log at
`~/.local/share/aishe/history.ext` (zsh `EXTENDED_HISTORY` format). The
interactive zsh front-end records each command there via a `preexec` hook, and
the `-c`/hook paths record through aishe's executor, so `aishe history` and
semantic search have real data. The semantic-history index filters out
history-management commands such as `history` and `fc`. The log is capped on
exit so it can't grow without bound. `share_history` (default on) shares it
across sessions; turn it off for per-session (pid-suffixed) files.

On a minimal account where zsh starts with `HISTFILE` unset (and typically
`SAVEHIST=0`), the PTY wrapper adopts this same Aishe log as zsh's native history
file and enables `EXTENDED_HISTORY`, `APPEND_HISTORY`, and—when
`share_history=true`—`SHARE_HISTORY`. Native Up-arrow, `Ctrl-R`, and history
expansion therefore survive shell restarts and exchange entries with concurrent
Aishe sessions. The log lives in Aishe's user data directory; binary/managed
runtime installers and package upgrades do not remove it.

## Branded prompt status line

With `pty_prompt = true`, aishe can render its live status in the right prompt,
on a separate line above the input prompt, or nowhere. Configure it in `aishe
setup` / `aishe settings`, or set `status_line_position = "right"`, `"below"`,
or `"off"`. The default `identity` field compactly shows the safe connection
label/ID, provider/endpoint host, authentication label, model/reasoning, and
whether the choice is shell-local or the durable default. Individual fields
include `connection`, `provider`, `endpoint`, `auth`, `selection`, `model`,
`reasoning`, `mode`, `backend`, `scope`, `task`, `elapsed`, `context`,
`last_tokens`, `last_cost`, `session_tokens`, `session_cost`, and `requests`. The below-prompt
layout is usually more readable when you select the detailed metrics. Status
text is passed through zsh's non-recursive `psvar` prompt escape, so even with a
theme's `PROMPT_SUBST` option enabled, model names and provider text cannot
become shell substitutions.

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
machine. `search` prints the closest past commands with a similarity score; for
live recall while typing, the [semantic-recall keybinding](#semantic-recall-keybinding)
(**Ctrl-X Ctrl-R**) pre-fills the best match onto the line.

To skip the manual `index` step, set `semantic_history_autoindex = true`: the
interactive shell then re-runs the incremental index automatically when you exit,
so newly run commands are searchable next session. It's off by default because it
embeds new commands on the provider (free with a local Ollama, metered on a paid
API).
