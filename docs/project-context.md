# Per-project context (`.aishe/context.md`)

aishe can feed the model a per-project notes file so it knows your repo's
conventions without you repeating them every request.

## How it works

When aishe builds the environment context for a natural-language request, it
looks for a file named `.aishe/context.md` starting in the current directory and
walking up through parent directories. The first one found is included in the
context block under a `Project context (.aishe/context.md):` heading, so its
contents reach the model alongside the cwd, directory listing, and recent
commands.

Because the search walks upward, a single file at the repository root applies to
every subdirectory of the project.

## What to put in it

Short, durable, repo-specific guidance. For example:

```markdown
# Project conventions

- This is a Rust workspace; build with `cargo build`, test with `cargo test`.
- Never edit files under `vendor/` - they are generated.
- Use `just lint` before committing.
- The deploy script is `scripts/deploy.sh` and requires `ENV=staging|prod`.
```

Keep it focused. The file is capped at 4000 characters when included (the rest is
truncated with a note), so it is a place for conventions and pointers, not full
documentation.

## Controlling it

It is on by default. Disable it with:

```toml
[aishe]
project_context = false
```

The file is plain context, like the directory listing: it is never executed, and
it is sent only when you make a natural-language request. If you keep secrets in
the repo, do not put them in `.aishe/context.md`, since its contents are sent to
the model (the secret-redaction pass scrubs recent commands, not this file).
