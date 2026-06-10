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

```toml
[aishe]
show_right_prompt = true
git_prompt = true
```

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
