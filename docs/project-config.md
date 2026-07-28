# Per-project config (`.aishe/config.toml`)

A repository can ship a project-local config overlay so different repos get
different models, tools, budgets, and (when you trust them) MCP servers and
providers, without you editing your global config each time you `cd` in.

This is the config sibling of [`.aishe/context.md`](project-context.md): the
context file gives the model repo conventions; the config file changes how aishe
itself behaves in that repo.

## How it works

When aishe starts (or runs a hooked natural-language line), it looks for
`.aishe/config.toml` starting in the current directory and walking up through
parent directories. The nearest one is merged over your user config. So a single
file at the repo root applies to the whole project.

Precedence, lowest to highest:

```
compiled defaults  <  user config.toml  <  project .aishe/config.toml  <  CLI flags
```

The user config is `config.toml` in aishe's config directory —
`~/.config/aishe/` on Linux, `~/Library/Application Support/aishe/` on macOS.
`aishe doctor` prints both the resolved user config path and the project overlay
it found; see [File locations](configuration.md#file-locations).

The overlay is a partial merge: only the keys present in the project file change;
everything else keeps your global value. (Unlike the user config, a project file
*may* set a partial `[providers.<name>]` block, e.g. just `model`.)

## Trust: safe vs sensitive keys

A `.aishe/config.toml` can come from any repository you clone, so aishe will not
let an untrusted file change anything security-relevant. Keys fall into two
tiers:

**Safe (always applied).** Cosmetic and behavioral keys, and a per-provider
`model`: `mode` (for `suggest`/`auto`), `stream`, `structured`, `memory`,
`cache`/`cache_ttl_secs`, `budget_usd`, `max_yolo_iterations`, `yolo_plan`,
`yolo_verbose`, `file_tools`, `web_tool`, `auto_pushd`, `cdpath`,
`share_history`, `pty_prompt`, `project_context`, `[named_dirs]`, `[pricing]`,
and `[providers.<name>].model`.

**Sensitive (applied only when you trust the file).** Anything that could
exfiltrate prompts, run code, or weaken safety: `provider`, a
`[providers.<name>]` `base_url`/`api_key_env`, `[mcp_servers]` (which can launch
arbitrary commands), `[logging]`, `redact_secrets`, `yolo_sandbox`,
`yolo_confirm`/`yolo_confirm_dangerous`, and `mode = "yolo"` (a repo must not
silently put you into autonomous command-running).

Until you trust a file, its sensitive keys are reported but **not** applied:

```
aishe: 2 sensitive key(s) in /repo/.aishe/config.toml need trust to apply
(provider, [mcp_servers]). Run `aishe trust`.
```

## Trusting a project

```sh
aishe trust          # trust this repo's .aishe/config.toml
aishe trust --list   # list every trusted file
aishe untrust        # drop trust for this repo
aishe untrust --all  # drop trust for every trusted file
```

(Each also works as a slash-command in the REPL: `/trust`, `/untrust`.)

### Trusting a single project file

Both commands take an optional path, so trust is not all-or-nothing. This is how
you enable a project **skill** (`.aishe/skills/<name>/SKILL.md`) or a project
**command** (`.aishe/commands/<name>.md`) — neither is covered by the bare
`aishe trust`, which only applies to `.aishe/config.toml`:

```sh
aishe trust .aishe/skills/deploy/SKILL.md   # load this project skill at all
aishe trust .aishe/commands/gitsync.md      # stop prompting for this shell command
aishe untrust .aishe/commands/gitsync.md
```

A project skill is *dropped entirely* until trusted (the model loads it mid-loop,
so there is no moment to confirm at); a project `shell: true` command prompts on
every run until trusted. Details in
[Custom commands and skills](custom-commands-and-skills.md#trusting-an-individual-project-file).

Trusting a command file waives the *trust* prompt and nothing else: the resolved
body still passes through the [safety gate](safety.md), so a destructive command
still stops and asks for confirmation.

Trust is keyed by the file's absolute path **and** a hash of its contents. If the
file changes later (you edit it, or a `git pull` updates it), trust is dropped
and the sensitive keys defer again until you re-run `aishe trust`. The hash
detects changes; it is not a tamper-proof signature, so only trust repositories
you vouch for.

`aishe doctor` shows the active project config, whether it is trusted, and how
many keys are applied vs deferred.

## Example

```toml
# .aishe/config.toml at a repo root

[aishe]
# Safe: applies immediately in this repo.
mode = "auto"
budget_usd = 2.00
web_tool = false

# Safe: use a cheaper model here (per-provider model override).
[providers.anthropic]
model = "claude-haiku-4-5-20251001"

# Sensitive: only after `aishe trust`.
[mcp_servers.git]
command = "uvx"
args = ["mcp-server-git", "--repository", "."]
```

See also [SECURITY.md](../SECURITY.md) for the threat model behind the trust
gate, and [configuration.md](configuration.md) for the full key reference.
