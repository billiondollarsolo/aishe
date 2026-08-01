# MCP servers (Model Context Protocol)

In yolo mode, aishe can connect to [Model Context
Protocol](https://modelcontextprotocol.io) servers and offer their tools to the
managed agent. This plugs the whole MCP ecosystem (filesystem, git, databases,
web search, your own servers) into the agentic loop without handing execution
to OpenCode: the managed engine sees only AIShe proxy-tool schemas, and each MCP
call returns through the authenticated foreground lease owned by the requesting
AIShe shell.

## Configuring servers

Add an `[mcp_servers]` table to your `config.toml` — `~/.config/aishe/` on Linux,
`~/Library/Application Support/aishe/` on macOS; `aishe doctor` prints the
resolved path (see [File locations](configuration.md#file-locations)). Each entry
is keyed by a short name that is used to namespace the server's tools:

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

### HTTP servers (Streamable HTTP)

A server reached over HTTP uses `url` instead of `command`. Set `headers` for
anything the endpoint needs (for example an `Authorization` bearer token):

```toml
[mcp_servers.remote]
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer ${TOKEN}" }   # extra request headers
# enabled = false                                  # keep configured but disabled
```

- A server is treated as **HTTP** when `url` is set; otherwise it is a stdio
  server launched from `command`. A server with neither `command` nor `url` is
  invalid and is skipped with an error message.
- `headers` are sent on every request to the endpoint. The value is used as-is,
  so expand any environment variables yourself before writing the config.
- `args` and `env` apply only to stdio servers.

## How tools are exposed

For a server named `filesystem` advertising a `read_file` tool, the model sees a
tool named `mcp__<server>__<tool>`, e.g. `mcp__filesystem__read_file`. The name is
sanitized to the character set the model APIs accept. When the model calls it,
the trusted plugin sends the stable tool-call ID to the AIShe supervisor. The
owning foreground AIShe process applies mode, scope, organization policy,
output bounds, cancellation, and idempotency before it proxies `tools/call` to
that server and feeds the bounded result back into the managed conversation.
Each call is recorded in the [audit log](logging.md) as
`yolo:mcp__filesystem__read_file`.

List what is connected:

```sh
aishe mcp        # list available MCP tools
```

```
MCP tools (yolo mode):
  mcp__filesystem__read_file  —  Read the complete contents of a file.
  mcp__filesystem__write_file —  Create or overwrite a file.
  ...
```

## Resources

If a server declares the `resources` capability, aishe offers the yolo loop two
extra tools per server:

- `mcp__<server>__list_resources` - list the server's resources (uri, name,
  description).
- `mcp__<server>__read_resource` - read a resource by its `uri` and return its
  text (binary `blob` contents are noted, not dumped).

So the agent can discover and pull in server-provided data (files, database rows,
docs) as it works.

## Prompts

If a server declares the `prompts` capability, its prompts are loaded at startup
and exposed as slash-commands named `/<server>:<prompt>`. Running one calls
`prompts/get` (mapping your positional arguments to the prompt's declared
argument names), flattens the returned messages to text, and runs that as a
request in your current mode. List them with `aishe mcp`:

```
MCP prompts (run as /<server>:<prompt>):
  /docs:summarize  -  Summarize a document
```

```sh
/docs:summarize report.md
```

## Transport and limits

- Two transports are supported:
  - **stdio**: newline-delimited JSON-RPC 2.0 over the server's stdin/stdout.
  - **Streamable HTTP**: JSON-RPC 2.0 POSTed to a `url`. Each request sends
    `Accept: application/json, text/event-stream`; the server may answer with a
    single JSON object or with a `text/event-stream` (SSE) stream, which aishe
    reads until the event whose id matches the request arrives (unrelated
    notifications and ids are ignored). The `Mcp-Session-Id` returned by the
    `initialize` response is captured and echoed on every later request and
    notification. Notifications expect any 2xx (typically `202 Accepted`).
- aishe performs the `initialize` handshake, sends `notifications/initialized`,
  then `tools/list`, and (per the server's declared capabilities) `prompts/list`.
  **Tools**, **resources**, and **prompts** are all consumed (see below), on
  either transport.
- A stdio request waits up to 30 seconds for its response, so a wedged server
  can't hang the shell; stdio servers are terminated when aishe exits. HTTP
  calls use a 10s connect and 30s read timeout.
- Server-initiated requests (for example sampling) are ignored.

## Security

MCP tools run with the privileges of the server process you launch, and the
model decides when to call them. Only configure servers you trust, and scope
them (for example point a filesystem server at a single project directory).
AIShe authenticates and audits the call boundary, but it cannot inspect or
kernel-confine the internal actions of an arbitrary MCP server. Provider
credentials and AIShe control tokens are removed from stdio MCP environments;
explicit values you put in an MCP `env` or HTTP `headers` entry remain your
responsibility.
