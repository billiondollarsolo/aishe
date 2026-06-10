# Prompt and theming

These settings apply to the built-in reedline front-end. The zsh-PTY front-end
renders your real zsh prompt (powerlevel10k, oh-my-zsh, and so on) unchanged.

## Left prompt

By default the left prompt shows the current directory. Customize it with
`prompt_format` using placeholders:

```toml
[aishe]
prompt_format = "[{mode}] {cwd}"
```

Placeholders:

- `{cwd}`: current working directory.
- `{mode}`: current interaction mode.
- `{model}`: active model.
- `{exit}`: exit code of the last command.

For a full powerlevel10k or oh-my-zsh prompt, use the zsh-PTY front-end.

## Right prompt

- `show_right_prompt = true` shows "model and mode" on the right.
- `git_prompt = true` shows the current git branch (for example `⎇ main`), read
  directly from `.git/HEAD` without spawning git.
- `git_status = true` adds a dirty marker (`*`) and ahead/behind counts
  (`⇡N`/`⇣N`) to the git segment, via one short, time-limited `git status` call
  per prompt. Turn it off in very large repos.
- `report_time` shows the **last command's duration** on the right when it ran at
  least that many seconds (default 3; `0` disables it). For example `3.2s`,
  `1m05s`.

```toml
[aishe]
show_right_prompt = true
git_prompt = true
git_status = true
report_time = 3
```

A full right prompt looks like: `⎇ main*⇡1  3.2s  claude-sonnet · suggest`.

Note: reedline hides the right prompt when it would not fit alongside your input,
so on a narrow terminal (or with a long branch and model name) parts may not
appear.

## Mode glyphs

The prompt glyph reflects the mode: `❯` for suggest, `»` for auto, and `⚡` for
yolo. In vi keymap, the prompt also shows `[I]` or `[N]` for insert or normal.

## Theming

Colors for the prompt and syntax highlighter live in a `[theme]` section. Pick a
preset and override any role.

```toml
[theme]
preset = "nord"     # default | vivid | mono | nord | gruvbox
cwd = "bright-cyan"
known_cmd = "#98c379"
unknown_cmd = "red"
flag = "yellow"
string = "green"
operator = "magenta"
path = "blue"
```

Switch presets with `aishe theme nord` (applies on the next session).

### Color formats

A color may be:

- a name: `red`, `bright-green`, `purple`,
- a palette index: `0` through `255`,
- a hex value: `#ff8800`.

### Roles

`cwd`, `glyph_ok`, `glyph_err`, `right_prompt`, `known_cmd`, `unknown_cmd`,
`flag`, `string`, `operator`, `path`, `assignment`, `sigil_nl`, `sigil_shell`.

The highlighter colors the command head by whether it is a known command, with
distinct colors for flags, quoted strings, operators (`| && ; > <`), paths, env
assignments, and the `?` and `!` sigils.
