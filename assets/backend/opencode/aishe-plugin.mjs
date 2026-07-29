/**
 * Trusted Aishe/OpenCode bridge.
 *
 * This plugin never performs host I/O. Every model-requested effect is sent to
 * the authenticated Aishe supervisor, which routes it to the foreground Aishe
 * client holding the session lease.
 */
import { tool } from "@opencode-ai/plugin"

const bridgeUrl = process.env.AISHE_BRIDGE_URL
const bridgeToken = process.env.AISHE_BRIDGE_TOKEN

async function invoke(tool, args, context) {
  if (!bridgeUrl || !bridgeToken) {
    throw new Error("Aishe foreground tool bridge is unavailable")
  }
  const response = await fetch(`${bridgeUrl}/v1/plugin/tool`, {
    method: "POST",
    redirect: "error",
    headers: {
      "authorization": `Bearer ${bridgeToken}`,
      "content-type": "application/json",
    },
    body: JSON.stringify({
      tool,
      args,
      session_id: context.sessionID,
      message_id: context.messageID,
      call_id: args._aishe_call_id,
      agent: context.agent,
      directory: context.directory,
      worktree: context.worktree,
    }),
    signal: context.abort,
  })
  const body = await response.text()
  if (!response.ok) {
    throw new Error(`Aishe tool bridge rejected ${tool}: ${response.status} ${body}`)
  }
  const decoded = JSON.parse(body)
  if (decoded.success === false) {
    const message = typeof decoded.output === "string"
      ? decoded.output
      : JSON.stringify(decoded.output)
    throw new Error(`Aishe tool failed: ${message}`)
  }
  return decoded.output
}

function proxy(name, description, args) {
  return tool({
    description,
    args,
    async execute(value, context) {
      return invoke(name, value, context)
    },
  })
}

export const AisheBridge = async () => ({
  event: async ({ event }) => {
    const info = event?.properties?.info
    if (event?.type === "session.created" && info?.id && info?.parentID) {
      const response = await fetch(`${bridgeUrl}/v1/plugin/child`, {
        method: "POST",
        redirect: "error",
        headers: {
          "authorization": `Bearer ${bridgeToken}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          parent_session_id: info.parentID,
          child_session_id: info.id,
        }),
      })
      if (!response.ok) {
        throw new Error(`Aishe child-session lease registration failed: ${response.status}`)
      }
      return
    }
    if (
      event?.type === "message.updated" &&
      info?.role === "assistant" &&
      info?.id &&
      (info?.sessionID || event?.properties?.sessionID) &&
      info?.time?.completed
    ) {
      const sessionID = info.sessionID || event.properties.sessionID
      const response = await fetch(`${bridgeUrl}/v1/plugin/usage`, {
        method: "POST",
        redirect: "error",
        headers: {
          "authorization": `Bearer ${bridgeToken}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          session_id: sessionID,
          message_id: info.id,
          input_tokens: Math.max(0, Math.floor(info.tokens?.input ?? 0)),
          output_tokens: Math.max(0, Math.floor(info.tokens?.output ?? 0)),
          cost_usd: Number.isFinite(info.cost) ? Math.max(0, info.cost) : null,
        }),
      })
      if (!response.ok) {
        throw new Error(`Aishe usage accounting rejected an event: ${response.status}`)
      }
    }
  },
  tool: {
    aishe_run_command: proxy(
      "run_command",
      "Run one command through Aishe policy, sandbox, audit, and the foreground terminal.",
      {
        command: tool.schema.string().min(1).max(65536),
        cwd: tool.schema.string().max(4096).optional(),
        timeout_secs: tool.schema.number().int().min(1).max(3600).optional(),
      },
    ),
    aishe_read_file: proxy(
      "read_file",
      "Read a bounded file through Aishe workspace policy.",
      { path: tool.schema.string().min(1).max(4096) },
    ),
    aishe_write_file: proxy(
      "write_file",
      "Atomically write a file through Aishe policy and undo journaling.",
      {
        path: tool.schema.string().min(1).max(4096),
        content: tool.schema.string().max(4194304),
      },
    ),
    aishe_edit_file: proxy(
      "edit_file",
      "Replace exact text through Aishe policy and undo journaling.",
      {
        path: tool.schema.string().min(1).max(4096),
        old: tool.schema.string().min(1).max(4194304),
        new: tool.schema.string().max(4194304),
      },
    ),
    aishe_apply_patch: proxy(
      "apply_patch",
      "Apply a bounded validated patch transactionally through Aishe policy and undo journaling.",
      { patch: tool.schema.string().min(1).max(4194304) },
    ),
    aishe_list_dir: proxy(
      "list_dir",
      "List bounded directory metadata through Aishe workspace policy.",
      { path: tool.schema.string().min(1).max(4096) },
    ),
    aishe_search_files: proxy(
      "search_files",
      "Search files with bounded output through Aishe workspace policy.",
      {
        query: tool.schema.string().min(1).max(8192),
        path: tool.schema.string().max(4096).optional(),
      },
    ),
    aishe_fetch_url: proxy(
      "fetch_url",
      "Fetch an approved HTTP(S) URL through Aishe network policy.",
      { url: tool.schema.string().min(1).max(8192) },
    ),
    aishe_use_skill: proxy(
      "use_skill",
      "Load one Aishe-approved skill through the trusted progressive-disclosure registry.",
      { name: tool.schema.string().min(1).max(256) },
    ),
    aishe_mcp_call: proxy(
      "mcp_call",
      "Call one user-configured and Aishe-approved MCP tool.",
      {
        server: tool.schema.string().min(1).max(256),
        tool: tool.schema.string().min(1).max(256),
        arguments: tool.schema.record(tool.schema.string(), tool.schema.unknown()),
      },
    ),
    aishe_ask_user: proxy(
      "ask_user",
      "Ask the foreground user a non-approval question needed to continue.",
      { prompt: tool.schema.string().min(1).max(16384) },
    ),
  },
  "tool.execute.before": async (input, output) => {
    if (input.tool.startsWith("aishe_")) {
      // Never trust a call identity supplied by the model. OpenCode owns this
      // value and the bridge uses it as its durable idempotency key.
      output.args._aishe_call_id = input.callID
    }
  },
  "chat.params": async (input, output) => {
    if (!bridgeUrl || !bridgeToken) throw new Error("Aishe budget bridge is unavailable")
    const response = await fetch(`${bridgeUrl}/v1/plugin/provider-turn`, {
      method: "POST",
      redirect: "error",
      headers: {
        "authorization": `Bearer ${bridgeToken}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({
        session_id: input.sessionID,
        requested_max_output_tokens: output.maxOutputTokens,
      }),
    })
    if (!response.ok) throw new Error(`Aishe budget gate rejected provider turn: ${response.status}`)
    const decision = await response.json()
    if (typeof decision.max_output_tokens === "number") {
      output.maxOutputTokens = decision.max_output_tokens
    }
  },
  "permission.ask": async (_input, output) => {
    // Aishe is the only approval UI. Managed config denies built-in host tools;
    // the trusted proxy tools are allowed without OpenCode prompting.
    output.status = "deny"
  },
})

export default AisheBridge
