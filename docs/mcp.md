# MCP servers (Model Context Protocol)

In yolo mode, aishe can connect to [Model Context
Protocol](https://modelcontextprotocol.io) servers and offer their tools to the
model alongside the built-in ones. This plugs the whole MCP ecosystem
(filesystem, git, databases, web search, your own servers) into the agentic
loop.

## Configuring servers

Add an `[mcp_servers]` table to `~/.config/aishe/config.toml`. Each entry is keyed
by a short name that is used to namespace the server's tools:

```toml
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/home/me/projects"]

[mcp_servers.git]
command = "uvx"
args = ["mcp-server-git", "--repository", "/home/me/projects/aishe"]
# env = { GIT_AUTHOR_NAME = "me" }   # extra environment for the server process
# enabled = false                    # keep configured but disabled
```

- `command` / `args` launch the server (any stdio MCP server works: `npx`,
  `uvx`, a Docker container, an absolute path to a binary, ...).
- `env` adds environment variables for the server process.
- `enabled` (default `true`) lets you keep a server configured but turned off.

Servers connect at startup. A server that fails to launch or handshake is
reported and skipped; it never blocks the shell.

## How tools are exposed

For a server named `filesystem` advertising a `read_file` tool, the model sees a
tool named `mcp__<server>__<tool>`, e.g. `mcp__filesystem__read_file`. The name is
sanitized to the character set the model APIs accept. When the model calls it,
aishe proxies a `tools/call` request to that server and feeds the text result
back into the loop. Each call is recorded in the [audit log](logging.md) as
`yolo:mcp__filesystem__read_file`.

List what is connected:

```sh
aishe mcp        # or /mcp in the REPL
```

```
MCP tools (yolo mode):
  mcp__filesystem__read_file  —  Read the complete contents of a file.
  mcp__filesystem__write_file —  Create or overwrite a file.
  ...
```

## Transport and limits

- The transport is the MCP **stdio** transport: newline-delimited JSON-RPC 2.0
  over the server's stdin/stdout. (HTTP/SSE transports are not supported yet.)
- aishe performs the `initialize` handshake, sends `notifications/initialized`,
  then `tools/list`. Only **tools** are consumed today (not prompts or
  resources).
- Each request waits up to 30 seconds for its response, so a wedged server can't
  hang the shell. Servers are terminated when aishe exits.
- Server-initiated requests (for example sampling) are ignored.

## Security

MCP tools run with the privileges of the server process you launch, and the
model decides when to call them. Only configure servers you trust, and scope
them (for example point a filesystem server at a single project directory). The
safety gate screens `run_command`, but it does not inspect the internal actions
of an MCP tool.
