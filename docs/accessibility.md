# Terminal accessibility policy and palette review

AIShe never relies on color alone. Route, authorship, selection, progress,
approval, warning, failure, and safety-bypass states each retain a textual
label, distinct glyph, or explicit focus marker after ANSI is stripped.
`ui.motion = "static"` replaces erased/spinning status with durable phase
lines; `ui.unicode = "ascii"` replaces box drawing and symbolic state glyphs;
`ui.theme = "none"`, `NO_COLOR`, `TERM=dumb`, redirected output, and every JSON
surface suppress styling entirely. That includes the zsh prompt and statusline:
they paint from the same palette through `AISHE_COLOR_*`, exported by the PTY
front end, so a colorless policy reaches them too.

## Maintained palette review

Review date: 2026-07-31. Palette contract: v1. Reviewer/owner: AIShe
maintainers. Automated gate: `ui::tests::maintained_truecolor_palettes_meet_text_contrast_floor`.

The owned truecolor foregrounds were measured with the WCAG relative-luminance
formula against the light reference background `#ffffff` and dark reference
background `#12161c`. Every normal-size text token must be at least 4.5:1.

| Semantic group | Light minimum | Dark minimum |
| --- | ---: | ---: |
| accent / assistant label / code label | 7.67:1 | 8.35:1 |
| shell / success / added | 5.98:1 | 7.14:1 |
| agent / policy | 8.00:1 | 7.67:1 |
| activity / muted | 6.36:1 | 8.36:1 |
| proposed command | 15.80:1 | 16.91:1 |
| warning | 5.92:1 | 11.42:1 |
| danger / removed | 6.93:1 | 5.99:1 |

ANSI16 and ANSI256 values map each semantic token to a conventional terminal
index, but the terminal emulator owns the actual RGB palette and background.
AIShe therefore cannot claim a numeric contrast ratio for an arbitrary custom
terminal palette. Use `ui.theme = "mono"` for attribute-only emphasis or
`ui.theme = "none"`/`NO_COLOR=1` for plain output when an emulator palette is
not legible. Doctor reports the effective theme, depth, Unicode, and motion
policy; emulator-specific contrast remains part of the manual terminal matrix.

## Keyboard access

Interactive actions have non-letter alternatives and do not depend on pointer
input. The authoritative active bindings and terminal-specific Option/Meta
caveats live in `/help session` and [shell integration](shell-integration.md).
Picker filtering always accepts printable characters; arrows, Page Up/Page
Down, Home, End, Enter, Escape, Ctrl-C, and numbered static choices provide the
navigation and fallback paths.
