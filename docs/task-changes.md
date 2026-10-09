# Review and apply task changes

Open `/tasks`, select an isolated task, and choose **apply** from its actions to open **Review task changes**. The review shows files, individual text hunks, and actual recorded checks. Select a file to inspect its diff, toggle individual hunks or the whole file, then choose **Apply selected changes**. The final confirmation defaults to leaving the source repository unchanged.

Binary files, renames, file creation or deletion, and mode changes must be selected as whole files. Symlinks, submodules, unsafe paths, non-UTF-8 patches, and changes exceeding the bounded review limits require manual review and application. The task's isolated workspace stays available after partial application.

## Scripted review

```sh
aishe task review TASK_ID
aishe task review TASK_ID --json > task-review.json
```

The JSON includes a `revision`, numbered `files` and `hunks`, their `applied` and `selectable` flags, the recorded `check_summary` and `checks`, and any unresolved application issues. Numbers are stable for the exact original patch. Displayed paths and diff text are redacted; application uses the original patch bytes.

Copy the exact `revision` from the review into a selection:

```sh
aishe task apply TASK_ID --revision REVIEW_REVISION --file 2
aishe task apply TASK_ID --revision REVIEW_REVISION --hunk 1 --hunk 3
```

File selectors include the whole file. Hunk selectors apply only those modified-text hunks. Without file or hunk selectors, application selects all remaining changes:

```sh
aishe task apply TASK_ID --revision REVIEW_REVISION
```

Refresh the review after every application. Reusing an old revision, selecting an already applied hunk, or applying after either workspace changes is rejected. A partial application leaves other changes reviewable and keeps the source index and commit untouched. Finish or discard the remaining changes before starting a new task; a partially applied task cannot be resumed or reworked.

## What checks establish

Checks show commands actually executed, observed exit codes, and whether their results became stale after later task effects. Model-written claims never become recorded passing checks. The review calls out absent, failed, stale, cancelled, and uncertain results.

Checks ran in the task workspace. A selected subset has not been checked separately, and recorded freshness does not establish that unrelated processes left the workspace alone. Applying changes is an explicit user action; passing checks do not cause automatic application.

Application revalidates the reviewed patch and source preimages under task and source locks. It first checks the exact selected patch and refuses overlap instead of creating merge conflicts. Git hooks, external diff and text conversion tools, filesystem monitors, and configured clean/smudge/process filters are disabled for this flow.

If application is interrupted after it starts, its private journal reports an uncertain result and blocks automatic retries. Inspect the source files and begin a new task when needed; the shell does not replay an operation whose effects are unknown.
