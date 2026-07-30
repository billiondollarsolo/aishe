# Agent output progressive disclosure

Status: implemented on `feature/opencode-backend`.

## Problem

Foreground tools streamed commands and raw output directly to the terminal even
when `backend.output = "focus"`. That bypassed the event renderer, made transient
status rows permanent, duplicated commands in detailed output, and exposed every
recovered attempt as a red failure.

Primary slash commands also existed across several paths without one obvious
index or a unified live-status command.

## UX contract

- `focus` remains the default and least-verbose mode. One width-bounded live row
  shows the current action or command. On completion it leaves one activity
  summary and the final answer in scrollback.
- `compact` keeps one completed row per action, followed by the activity summary
  and final answer. It never streams raw child output.
- `detailed` streams raw command output, diffs, usage, and agent events. Each
  command is introduced once.
- A tool failure followed by a successful turn is a recovered attempt (amber),
  not a terminal failure (red).
- Ctrl-O and `/details` toggle `focus`/`detailed` for following turns in the
  current shell. `aishe output ...` persists a default.
- `/help` and `aishe commands` show the curated primary command surface and any
  installed custom commands. `/status`/`aishe status` show active session
  settings and spend.

The inline shell cannot erase arbitrary historical scrollback safely. Expansion
therefore applies to following turns; the summary says so explicitly.

## Implementation plan

1. Stop the foreground worker from streaming unless the selected output mode is
   `detailed`.
2. Track active tool labels, durations, recovered attempts, file changes,
   subagents, and reconnects in the renderer.
3. Render a width-bounded current-command status in `focus`, one completion row
   in `compact`, and raw output only in `detailed`.
4. Add a curated slash-command index and unified live status command.
5. Cover mode routing, summaries, command labels, shell integration, and live
   status with automated tests, then validate a real Docker/PostgreSQL turn over
   SSH in focus and detailed modes.

## Acceptance criteria

- A focus-mode PostgreSQL Docker task does not leave Docker pull layers, command
  stdout, call IDs, escaped `\\x0a`, or intermediate tool failures in scrollback.
- The current command remains visible while it runs and is truncated rather than
  wrapped on a narrow terminal.
- The final focus transcript contains one activity summary and the final answer.
- Detailed mode retains raw diagnostics and valid/invalid login evidence.
- `/help`, `/status`, `/usage`, `/details`, `/settings`, `/reset`, and
  `/commands` are visible from the primary command index.
