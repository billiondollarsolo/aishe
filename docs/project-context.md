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

## Project- and host-aware context

Beyond `.aishe/context.md`, aishe automatically adds two compact, derived blocks
so suggestions fit *this* repo and *this* machine — no setup required:

- **Project tasks** (`project_tasks = true`, default on): aishe reads the build
  files and lists their entry points — `justfile` / `Makefile` targets,
  `package.json` and `composer.json` scripts, `compose` services, and
  Cargo/Python/CI markers. So "run the tests" resolves to the project's real
  command (`just test`, `npm test`, `cargo test`, …) instead of a guess. It walks
  up to the repo root (nearest `.git`), so it still works from a subdirectory; a
  subdirectory with its own task surface takes precedence, and the resolved root
  is noted when it differs from your cwd.
- **Installed tools + host facts** (`host_profile = true`, default on): a one-line
  list of the tools present on `$PATH` (package manager, `docker`/`podman`,
  `kubectl`, …) so the model proposes commands that exist here — `apt install` on
  Debian, `dnf` on Fedora, `brew` on macOS — plus operational facts that change
  which command is correct: the **init system** (`systemd` vs `openrc` vs
  `launchd`, so service control is right) and the **active Kubernetes context**
  (so cluster ops target the intended place). The kube context reads only local
  kubeconfig (no cluster contact).

Both are cheap (cached file reads / `which` lookups), capped, and contain only
names — no file contents, no secrets. Disable either with
`project_tasks = false` / `host_profile = false`.

Preview exactly what aishe sends with:

```sh
aishe context
```

It prints the full (redacted) context block — the OS/shell, cwd, installed tools,
project tasks, directory listing, recent commands, and any `.aishe/context.md`.
