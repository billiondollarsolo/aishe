# Agentic task workflows

AIShe keeps long work available without taking over your shell. `/tasks` or
**Ctrl-X b** opens a compact drawer; `/inbox` contains questions and exact action
approvals. Names, pins, reviewed results, and archived history persist across
shells. The drawer shows running work, queued dependencies, and recorded results
without starting a provider merely to inspect them.

## Move a conversation between foreground and background

During a native agent turn in the AIShe zsh front-end, press **Ctrl-X d** to
queue a background handoff. The current provider request or tool can finish
first. The agent parks at an execution boundary after saving its transcript,
pending-tool decisions, cumulative usage, and remaining allowances. It completes
the returned tool-call batch with explicit not-executed results rather than
replaying its unstarted calls in the new process.

The shell reports **queued** first; a parked checkpoint records the received
handoff. A kernel-held execution lease prevents two processes from continuing
the same checkpoint. The request names one exact lease, so a stale shell cannot
retarget a later continuation. Cancellation, policy, and exhausted budgets still
take precedence. Provider calls and opaque tools already in flight are not
interrupted merely to make a handoff immediate.

To bring a task back, open its details and press **g**, or use:

```sh
aishe task fg TASK_ID
# In the AIShe shell:
/tasks fg TASK_ID
```

The drawer asks before claiming the terminal. Continuation restores the saved
connection/model, workspace root, execution scope, transcript, and cumulative
limits, then rechecks current policy and admission. Answer an unanswered inbox
request first. A completed checkpoint cannot be presented as unfinished work.
While the claimed task is running in the foreground, **Ctrl-X d** sends it back
to the same background identity, keeping its mailbox and review history.

From a second terminal, `aishe task bg NATIVE_TASK_ID` queues a transfer for an
active foreground checkpoint. `/tasks bg` can use the current shell's private
control file. Standalone `aishe agent` invocations do not have the interactive
zsh shortcut; use the task-ID command from another terminal.

Handoff continues the existing workspace. It does not create a new isolated
worktree around a task that started in the source directory. Native preview
workspaces and turns without durable persistence cannot transfer. Filtered
exports needed for a claim travel in a private, single-lease transfer file;
environment values are not added to task history or workflow templates.

## Review files and hunks alongside actual checks

Press **p** for read-only changes or choose **apply** / **a** in task details to
review and select files or individual text hunks. Selections start empty. The
final Apply choice asks for confirmation and is bound to the exact task patch,
source preimages, and review revision. A later source or patch change requires a
new review. Partial application keeps the remaining work and stable original
selection identifiers available.

```sh
aishe task review TASK_ID --json
# Use the revision and identifiers returned by that review:
aishe task apply TASK_ID --revision REVIEW_REVISION --file 1
aishe task apply TASK_ID --revision NEW_REVIEW_REVISION --hunk 4
```

Binary changes, renames, creations, deletions, and mode changes require whole-file
selection. Unsupported changes explain their limitation. Review needs an
isolated Git task worktree; source-directory tasks retain their normal stop,
resume, and follow-up controls. Review disables Git hooks and external diff,
textconv, and filter helpers and bounds patch capture and display.

The review shows checks actually recorded through `run_check`, with failures,
uncertainty, skipped checks, and stale results visible. Check freshness covers
recorded task effects; it does not monitor every other process. Checks ran in
the task workspace. **A selected subset has not been checked separately.**
Neither an agent's final answer nor a successful patch application certifies
the selected result. Run your checks in the source workspace after applying.

## Inspect the execution timeline

Press **t** in task details for the timeline. **Ctrl-F** filters tools, recorded
checks, questions/follow-ups, handoffs, or workflow transitions. **Esc** returns
to details; **Ctrl-C** closes the drawer. Scriptable snapshots are available:

```sh
aishe task timeline TASK_ID
aishe task timeline TASK_ID --json
```

Events record observed transitions: tool planning, effect admission, tool starts
and results, actual check exits, durable human responses, queued/received
follow-ups, ownership transfers, and workflow release. A provider reservation
means budget was reserved before a request; it does not claim the request
completed. Unexecuted, declined, cancelled, and uncertain outcomes remain
distinct. Human/model-written plan notes are labeled separately from effects
and check evidence.

Each source retains its monotonic sequence and recorded timestamp. The drawer
merges the native checkpoint and independent human-action journal by timestamp;
equal timestamps preserve deterministic source order rather than claiming
sub-millisecond causality. Up to 256 events are shown, with omitted history and
unavailable journal warnings disclosed. Argument/result previews are bounded,
redacted, and safe for terminal display. The timeline excludes provider-private
continuation items, encrypted reasoning, reasoning summaries, and launch
environment values. Secret detection remains heuristic; do not put credentials
in task objectives or follow-ups.

## Save a reusable task graph

`/workflow` or `aishe workflow browse` inspects saved workflows and recorded runs.
Choosing a template shows its parameters, stage scopes, budgets, dependencies,
and required checks before starting it. Templates are validated local data;
inspection and saving do not call a provider.

Save this as `change.json` for a Rust repository:

```json
{
  "schema_version": 1,
  "name": "code-change",
  "description": "Implement, test, and review a bounded change",
  "max_parallel": 2,
  "parameters": [
    {"name": "goal", "description": "The requested change"}
  ],
  "stages": [
    {
      "key": "implement",
      "name": "Implement",
      "objective": "Implement {{goal}}. Keep the change focused.",
      "scope": "workspace",
      "network": "deny"
    },
    {
      "key": "test",
      "name": "Test",
      "objective": "Inspect the inherited change. Run the exact required command with run_check and resolve failures.",
      "depends_on": ["implement"],
      "required_checks": ["cargo fmt --check && cargo test"],
      "scope": "workspace",
      "network": "deny"
    },
    {
      "key": "review",
      "name": "Review",
      "objective": "Review the inherited change and actual check results. Resolve issues, then run the exact required command with run_check. Report remaining concerns.",
      "depends_on": ["test"],
      "required_checks": ["cargo fmt --check && cargo test"],
      "scope": "workspace",
      "network": "deny"
    }
  ]
}
```

```sh
aishe workflow save code-change --file change.json
aishe workflow show code-change
aishe workflow run code-change --param 'goal=make startup faster'
aishe workflow runs
aishe workflow show RUN_ID
aishe workflow cancel RUN_ID
aishe workflow resume RUN_ID
```

The extension `.toml` selects TOML input instead of JSON. This smaller template
uses the same validated schema and demonstrates a complete explicit budget:

```toml
schema_version = 1
name = "inspect-rust"
description = "Inspect a Rust project and record its checks"
max_parallel = 1

[[parameters]]
name = "focus"
default = "startup performance"

[[stages]]
key = "inspect"
name = "Inspect and check"
objective = "Inspect {{focus}}. Run the exact required check with run_check and explain unresolved issues."
scope = "workspace"
network = "deny"
required_checks = ["cargo fmt --check && cargo test"]

[stages.budget]
max_minutes = 15
max_provider_turns = 20
max_cost_usd = 0.0
max_tool_calls = 100
max_changed_files = 20
max_changed_bytes = 1048576
max_network_calls = 10
```

Parameter substitution is literal and single-pass in stage objectives. It does
not execute shell substitutions or interpret inserted placeholders. Required
check commands are literal and cannot contain parameter placeholders. A model
still reads the objective, so parameter text can influence its requested work;
scope, policy, and finite budgets remain the execution boundary.

Each stage has its own detached Git worktree. Completed dependency changes are
captured in private snapshot commits and passed to successor worktrees, including
bounded result/check context. Independent stages can run in parallel without
sharing a writable task worktree. The source branch stays unchanged until you
explicitly apply reviewed work. Merge conflicts block the successor and explain
the problem; they are not resolved silently.

Templates allow 1–32 stages and 1–8 parallel workers; parallelism defaults to 2.
A stage may name one required check command. Combine related checks with `&&`
or split them into dependent stages. A required check must have an exact fresh
recorded pass through `run_check` before the stage releases successors. Missing,
failed, stale, or uncertain evidence blocks release. An optional unconfigured
check is not manufactured from an agent's claims.

Omitted stage budgets default to 30 active minutes, 40 provider turns, 200 tool
calls, 100 changed files, 10 MiB changed content, 50 recognized network-capable
tool calls, and no explicit task cost cap. A zero cost cap does not mean free
model use. Paused questions keep their workflow slot; waiting time does not
consume active execution time. Queued dependencies are counted as queued rather
than Needs you; a failed dependency is attention.

The scheduler survives the shell closing. `workflow resume RUN_ID` restarts a
stopped scheduler using existing stage identities and checkpoints; it does not
retry a failed effect blindly or restart a cancelled run. Resolve a failed
stage with its task controls, then resume scheduling. Cancellation stops active
stages and prevents later dependency release while retaining their work. The
graph itself always requires isolation; `--no-isolation` is rejected for a
workflow.

Workspace scope and network denial are the defaults. Linux workspace execution
requires functional bubblewrap; macOS reports policy-only restrictions. Ensure
your build tools and offline dependencies are accessible inside the workspace.
If the workflow needs host scope, explicitly select host scope first and edit
the stage scope accordingly; a template cannot grant host access silently.
Likewise, enable network explicitly before using a stage with `network =
"allow"`. Current organization policy can further restrict either request.
Host scope does not provide OS-enforced network denial.

Saved templates and run journals are private under the AIShe data directory in
`workflows/templates/` and `workflows/runs/`. Removing a template retains its
existing runs; archived tasks retain results and workspaces. Budget, approval,
and review controls apply to every stage through the same native task runtime.
