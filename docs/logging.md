# Logging and privacy

aishe has two related features for safety and observability: secret redaction
(on by default) and an audit log (off by default).

## Secret redaction

Before each request, aishe builds an environment context block that includes your
recent commands. Those commands can contain credentials (`export TOKEN=...`,
`mysql -p...`, a URL with an embedded password). When `redact_secrets` is on
(the default), aishe scrubs likely secrets from that block before sending it to
the model, replacing them with `<redacted>`.

It is heuristic and deliberately conservative, catching common shapes:

- assignments whose name looks secret (`*_PASSWORD`, `*_TOKEN`, `*_SECRET`,
  `*_API_KEY`, and similar),
- long-form credential flags (`--password=...`, `--token ...`, `--api-key=...`),
- credentials embedded in URLs (`scheme://user:pass@host`),
- `Authorization:` headers,
- known key shapes (OpenAI `sk-...`, GitHub `ghp_...`, Groq `gsk_...`, AWS
  `AKIA...`, Slack `xox...`, Google `AIza...`),
- long high-entropy tokens that mix letters and digits.

Turn it off in config if it gets in your way:

```toml
[aishe]
redact_secrets = false
```

The same redaction is applied to the audit log (see below). Redaction reduces
risk; it is not a guarantee. Treat any endpoint you point aishe at as something
that can see your prompts.

## Audit log

When enabled, aishe appends one JSON object per line (JSONL) to a log file.
Native and managed calls share the same event stream. It records:

- safe connection ID/label, provider, auth type/profile label, model, and
  reasoning effort on every new event,

- `session_start`: a marker with the aishe version,
- `ai_request`: each model call, including the complete bounded user prompt,
  model, provider, mode, scope, Aishe turn ID, and durable managed-session
  identity,
- `ai_response`: the complete bounded visible response, any reasoning text the
  provider exposes, token/cache usage, cost, and duration,
- `ai_error`: a failed model call,
- `tool_call`: tool name, recursively redacted arguments, actual command/path,
  workspace, and turn/session/message/call IDs,
- `tool_result`: success, exit code, bounded output, final arguments after any
  user edit, and duration,
- `tool_approval`: requested and final approval decisions,
- `file_change`: changed path and bounded patch,
- `agent_event`: compaction, reconnect/reconciliation, subagent, todo, waiting,
  completion, cancellation, and failure lifecycle records,
- `action`: the compatibility record for actual shell commands, used by
  existing filters and runbook generation.

Connection attribution never contains a key, token, OAuth payload, local
control password, or launch secret. Profile labels are identifiers, not secret
values. The final JSON write boundary recursively redacts every string field.

The log records what Aishe receives and does. It cannot record private model
chain-of-thought that a provider does not expose.

It is off by default because it writes prompts, responses, and commands to disk.

### Enable it

In config:

```toml
[logging]
enabled = true
# file = "/custom/path/audit.jsonl"   # default: $XDG_DATA_HOME/aishe/audit.jsonl
redact = true                          # scrub secrets from logged text (default)
```

Or with environment variables (handy for a one-off session):

```sh
AISHE_LOG=1 aishe
AISHE_LOG=1 AISHE_LOG_FILE=/tmp/aishe.jsonl aishe
```

`aishe doctor` and `aishe status` show whether redaction and logging are on and
where the log goes. `/log` shows the latest 20 records inside an Aishe shell.

### Example entries

```json
{"ts_ms":1781120968535,"session":"4845-1781120968526","kind":"ai_request","connection_id":"openai-work","connection_label":"OpenAI work","provider":"openai","auth_type":"oauth","auth_profile":"work","reasoning_effort":"high","turn_id":"turn_f3...","backend_session":"ses_123","mode":"suggest","model":"gpt-5.6-luna","prompt":"list files by size"}
{"ts_ms":1781120974432,"session":"4845-1781120968526","kind":"ai_response","turn_id":"turn_f3...","backend_session":"ses_123","mode":"suggest","model":"openai/gpt-oss-120b","response":"command: ls -lS","tokens_in":438,"tokens_out":159}
{"ts_ms":1781120974911,"session":"4872-1781120974436","kind":"action","source":"yolo","command":"sh -c 'echo hi > z.txt && cat z.txt'","exit":0}
{"ts_ms":1781120975000,"session":"4872-1781120974436","kind":"tool_call","backend":"opencode","turn_id":"turn_f3...","backend_session":"ses_123","message_id":"msg_tool","call_id":"call_123","tool":"write_file","path":"README.md","args":{"path":"README.md","content":"hello"},"status":"started"}
{"ts_ms":1781120975012,"session":"4872-1781120974436","kind":"tool_result","backend":"opencode","turn_id":"turn_f3...","backend_session":"ses_123","message_id":"msg_tool","call_id":"call_123","tool":"write_file","success":true,"duration_ms":12,"output":"Wrote README.md"}
```

### Working with the log

It is plain JSONL, so standard tools work:

```sh
# pretty-print
cat ~/.local/share/aishe/audit.jsonl | jq

# every command the AI ran, with exit codes
jq 'select(.kind=="action") | {command, exit}' ~/.local/share/aishe/audit.jsonl

# total tokens this file
jq -s 'map(.tokens_in // 0 + (.tokens_out // 0)) | add' ~/.local/share/aishe/audit.jsonl
```

Prompt/response/argument/diff strings are bounded at 64 KiB each; captured tool
output is bounded at 16 KiB. Truncation is marked in the value. The log grows
over time; rotate or delete it yourself if needed.

### Built-in queries: `aishe log` and `aishe usage`

You don't need `jq` for the common questions. `aishe log` reads the same file
(`$AISHE_LOG_FILE`, else `[logging] file`, else the default) and prints a table:

```sh
aishe log                       # all entries, newest last
aishe log --action action       # only commands the AI ran (with exit codes)
aishe log --action tool_call    # model-requested tools and redacted arguments
aishe log --action tool_result  # outcomes, exits, output, and duration
aishe log --model gpt-4o        # only calls to a model matching this substring
aishe log --session <id>        # audit-process or managed backend session
aishe log --since 2h -n 50      # last 50 entries from the past 2 hours
aishe log --json                # raw JSONL (pipe to jq for more)
```

`--since` accepts `30m` / `2h` / `3d` / `1w` (a bare number means minutes).

`aishe usage` aggregates token counts and estimated cost (using the same price
table as the live `/usage` line, overridable in `[pricing]`):

```sh
aishe usage                     # totals per model
aishe usage --by day            # per calendar day (UTC)
aishe usage --by session        # per session
aishe usage --by connection     # safe connection attribution
aishe usage --since 1w          # only the last week
```

Both commands are **read-only** and never un-redact: secrets are already scrubbed
when the log is written, so nothing sensitive is reconstructed on read.
