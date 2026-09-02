/**
 * Trusted Aishe/OpenCode bridge.
 *
 * This plugin never performs host I/O. Every model-requested effect is sent to
 * the authenticated Aishe supervisor, which routes it to the foreground Aishe
 * client holding the session lease.
 */
const bridgeUrl = process.env.AISHE_BRIDGE_URL
const bridgeToken = process.env.AISHE_BRIDGE_TOKEN

const stringSchema = (minLength, maxLength) => ({
  type: "string",
  minLength,
  maxLength,
})

const integerSchema = (minimum, maximum) => ({
  type: "integer",
  minimum,
  maximum,
})

const booleanSchema = { type: "boolean" }

const objectSchema = (properties, required, additionalProperties = false) => ({
  type: "object",
  properties,
  required,
  additionalProperties,
})

function validate(schema, value, path) {
  if (schema.type === "string") {
    if (typeof value !== "string") throw new Error(`${path} must be a string`)
    if (value.length < schema.minLength || value.length > schema.maxLength) {
      throw new Error(
        `${path} length must be between ${schema.minLength} and ${schema.maxLength}`,
      )
    }
    return value
  }
  if (schema.type === "integer") {
    if (
      !Number.isSafeInteger(value) ||
      value < schema.minimum ||
      value > schema.maximum
    ) {
      throw new Error(
        `${path} must be an integer between ${schema.minimum} and ${schema.maximum}`,
      )
    }
    return value
  }
  if (schema.type === "boolean") {
    if (typeof value !== "boolean") throw new Error(`${path} must be a boolean`)
    return value
  }
  if (schema.type === "object") {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      throw new Error(`${path} must be an object`)
    }
    for (const name of schema.required) {
      if (!Object.prototype.hasOwnProperty.call(value, name)) {
        throw new Error(`${path}.${name} is required`)
      }
    }
    for (const [name, child] of Object.entries(value)) {
      const childSchema = schema.properties[name]
      if (childSchema) {
        validate(childSchema, child, `${path}.${name}`)
      } else if (schema.additionalProperties !== true) {
        throw new Error(`${path}.${name} is not allowed`)
      }
    }
    return value
  }
  throw new Error(`${path} uses an unsupported bridge schema`)
}

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

function proxy(name, description, properties, required) {
  // OpenCode 1.18.27 accepts dependency-free JSON Schema values in plugin tool
  // definitions. It treats every top-level entry as required, so the public
  // contract deliberately has one `input` object and expresses optional fields
  // inside it. Aishe validates the same schema again before crossing the bridge.
  const inputSchema = objectSchema(properties, required)
  return {
    description,
    args: { input: inputSchema },
    async execute(value, context) {
      if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new Error("tool arguments must be an object")
      }
      const input = validate(inputSchema, value.input, "input")
      const callID = value._aishe_call_id
      if (typeof callID !== "string" || callID.length === 0 || callID.length > 512) {
        throw new Error("OpenCode tool call identity is invalid")
      }
      return invoke(name, { ...input, _aishe_call_id: callID }, context)
    },
  }
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
      "Run one command through Aishe policy, sandbox, audit, and the foreground terminal. Set interactive=true when the command may prompt through a TTY (for example sudo, ssh, passwd, GPG, or a terminal UI).",
      {
        command: stringSchema(1, 65536),
        cwd: stringSchema(0, 4096),
        timeout_secs: integerSchema(1, 3600),
        interactive: booleanSchema,
      },
      ["command"],
    ),
    aishe_read_file: proxy(
      "read_file",
      "Read a bounded file through Aishe workspace policy.",
      { path: stringSchema(1, 4096) },
      ["path"],
    ),
    aishe_write_file: proxy(
      "write_file",
      "Atomically write a file through Aishe policy and undo journaling.",
      {
        path: stringSchema(1, 4096),
        content: stringSchema(0, 4194304),
      },
      ["path", "content"],
    ),
    aishe_edit_file: proxy(
      "edit_file",
      "Replace exact text through Aishe policy and undo journaling.",
      {
        path: stringSchema(1, 4096),
        old: stringSchema(1, 4194304),
        new: stringSchema(0, 4194304),
      },
      ["path", "old", "new"],
    ),
    aishe_apply_patch: proxy(
      "apply_patch",
      "Apply a bounded validated patch transactionally through Aishe policy and undo journaling.",
      { patch: stringSchema(1, 4194304) },
      ["patch"],
    ),
    aishe_list_dir: proxy(
      "list_dir",
      "List bounded directory metadata through Aishe workspace policy.",
      { path: stringSchema(1, 4096) },
      ["path"],
    ),
    aishe_search_files: proxy(
      "search_files",
      "Search files with bounded output through Aishe workspace policy.",
      {
        query: stringSchema(1, 8192),
        path: stringSchema(0, 4096),
      },
      ["query"],
    ),
    aishe_fetch_url: proxy(
      "fetch_url",
      "Fetch an approved HTTP(S) URL through Aishe network policy.",
      { url: stringSchema(1, 8192) },
      ["url"],
    ),
    aishe_use_skill: proxy(
      "use_skill",
      "Load one Aishe-approved skill through the trusted progressive-disclosure registry.",
      { name: stringSchema(1, 256) },
      ["name"],
    ),
    aishe_mcp_call: proxy(
      "mcp_call",
      "Call one user-configured and Aishe-approved MCP tool.",
      {
        server: stringSchema(1, 256),
        tool: stringSchema(1, 256),
        arguments: objectSchema({}, [], true),
      },
      ["server", "tool", "arguments"],
    ),
    aishe_ask_user: proxy(
      "ask_user",
      "Ask the foreground user a non-approval question needed to continue.",
      { prompt: stringSchema(1, 16384) },
      ["prompt"],
    ),
  },
  "tool.execute.before": async (input, output) => {
    if (
      typeof input?.tool === "string" &&
      input.tool.startsWith("aishe_") &&
      typeof output?.args === "object" &&
      output.args !== null
    ) {
      // Never trust a call identity supplied by the model. OpenCode owns this
      // value and the bridge uses it as its durable idempotency key.
      output.args._aishe_call_id = input.callID
    }
  },
  "chat.params": async (input, output) => {
    if (!bridgeUrl || !bridgeToken) throw new Error("AIShe provider-turn bridge is unavailable")
    if (typeof input?.sessionID !== "string" || input.sessionID.length === 0) {
      throw new Error("OpenCode provider turn is missing its session identity")
    }
    const requestedMaxOutputTokens =
      Number.isSafeInteger(output.maxOutputTokens) && output.maxOutputTokens > 0
        ? output.maxOutputTokens
        : null
    let decision
    // Retries are safe: authorize is pre-provider (no model call started yet).
    // Long multi-tool turns can race a just-renewed lease or a brief control
    // blip; wait for the foreground keepalive before failing closed.
    for (let attempt = 0; attempt < 5; attempt += 1) {
      const response = await fetch(`${bridgeUrl}/v1/plugin/provider-turn`, {
        method: "POST",
        redirect: "error",
        headers: {
          "authorization": `Bearer ${bridgeToken}`,
          "content-type": "application/json",
        },
        body: JSON.stringify({
          session_id: input.sessionID,
          requested_max_output_tokens: requestedMaxOutputTokens,
        }),
      })
      if (response.ok) {
        decision = await response.json()
        break
      }
      let code = "unknown"
      try {
        const body = await response.json()
        if (typeof body?.error?.code === "string" && /^[a-z0-9_]{1,64}$/.test(body.error.code)) {
          code = body.error.code
        }
      } catch {}
      const retriable =
        (response.status === 400 && code === "invalid_request") ||
        (response.status === 503 &&
          (code === "foreground_unavailable" || code === "lease_expired"))
      if (attempt < 4 && retriable) {
        await new Promise((resolve) => setTimeout(resolve, 250 * (attempt + 1)))
        continue
      }
      throw new Error(
        `AIShe refused the next model step: ${response.status} (${code}). ` +
          `If tools already ran, check host state and say "continue" — this is a ` +
          `foreground lease/control issue, not proof the install failed.`,
      )
    }
    // ChatGPT/Codex and SuperGrok OAuth use subscription transports that reject
    // `max_output_tokens` (OpenCode's built-in openai/xai OAuth hooks clear it).
    // Never re-apply a budget token cap as maxOutputTokens for those providers.
    // API-key launches use aishe-* provider IDs and still accept the cap.
    const providerId =
      typeof input?.model?.providerID === "string" ? input.model.providerID : ""
    const oauthOmitsMaxOutputTokens =
      providerId === "openai" || providerId === "xai"
    if (oauthOmitsMaxOutputTokens) {
      output.maxOutputTokens = undefined
    } else if (typeof decision?.max_output_tokens === "number") {
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
