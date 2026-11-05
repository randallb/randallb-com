const base = Deno.args[0];
const encoder = new TextEncoder();
let calls = 0;
const server = Deno.serve({
  hostname: "127.0.0.1",
  port: 0,
  onListen: ({ port }) => {
    Deno.writeTextFileSync(base + "/mock-port", String(port));
    console.log(JSON.stringify({ ready: true, port }));
  },
}, async (req) => {
  const path = new URL(req.url).pathname;
  if (
    path.startsWith("/hang/") && path.endsWith("/v1/messages") &&
    req.method === "POST"
  ) {
    Deno.writeTextFileSync(base + "/hang-request", "received");
    await new Promise((resolve) => setTimeout(resolve, 30000));
  }
  if (
    path.startsWith("/authfail/") && path.endsWith("/v1/messages") &&
    req.method === "POST"
  ) {
    return Response.json({
      type: "error",
      error: {
        type: "authentication_error",
        message: "Intentional local probe failure",
      },
    }, { status: 401 });
  }
  if (!path.includes("/messages") || req.method !== "POST") {
    console.log(JSON.stringify({ path, method: req.method }));
    return Response.json({});
  }
  const body = await req.json();
  if (path.includes("count_tokens")) {
    return Response.json({ input_tokens: 100 });
  }
  if (path.startsWith("/model-error/")) {
    return Response.json({
      type: "error",
      error: {
        type: "invalid_request_error",
        message: "selected model is unavailable",
      },
    }, { status: 400 });
  }
  const invalidSchema = (body.tools ?? []).some(
    (tool: { input_schema: Record<string, unknown> }) =>
      tool.input_schema?.type !== "object" ||
      ["oneOf", "anyOf", "allOf"].some((key) => key in tool.input_schema),
  );
  if (
    invalidSchema ||
    JSON.stringify(body.system).includes("CWR_UNTRUSTED_PROJECT_INSTRUCTIONS")
  ) {
    return Response.json({
      type: "error",
      error: {
        type: "invalid_request_error",
        message:
          "Invalid tool schema or ambient project instructions reached the API",
      },
    }, { status: 400 });
  }
  const tools = (body.tools ?? []).map((t: { name: string }) => t.name);
  const messages = body.messages ?? [];
  const last = messages.at(-1);
  const results = Array.isArray(last?.content)
    ? last.content.filter((b: { type: string }) => b.type === "tool_result")
    : [];
  const tool = tools.find((t: string) =>
    t ===
      (path.startsWith("/structured/")
        ? "StructuredOutput"
        : path.startsWith("/bashcase/")
        ? "Bash"
        : path.startsWith("/start-action/")
        ? "mcp__cwp__cwr--start--action"
        : "mcp__cwr__probe")
  );
  const chooseTool = tool && results.length === 0;
  console.log(
    JSON.stringify({
      call: ++calls,
      path,
      stream: body.stream,
      tools,
      toolResults: results,
    }),
  );
  const content = chooseTool
    ? {
      type: "tool_use",
      id: "toolu_probe_" + calls,
      name: tool,
      input: path.startsWith("/structured/")
        ? {
          status: 200,
          summary: "Fixture completed.",
          result: { text: "schema confirmed" },
        }
        : path.startsWith("/bashcase/")
        ? {
          command: "sleep 30",
          description: "Local cancellation probe.",
          timeout: 120000,
        }
        : path.startsWith("/start-action/")
        ? { activation: "@probe--double", input: { value: 7 } }
        : { value: 7 },
    }
    : { type: "text", text: "Probe finished: host result received." };
  const stopReason = chooseTool ? "tool_use" : "end_turn";
  const message = {
    id: "msg_probe_" + calls,
    type: "message",
    role: "assistant",
    model: body.model,
    content: [content],
    stop_reason: stopReason,
    stop_sequence: null,
    usage: { input_tokens: 100, output_tokens: 20 },
  };
  if (!body.stream) return Response.json(message);
  const start = {
    ...message,
    content: [],
    stop_reason: null,
    usage: { input_tokens: 100, output_tokens: 0 },
  };
  const initial = chooseTool
    ? { ...content, input: {} }
    : { type: "text", text: "" };
  const delta = chooseTool
    ? { type: "input_json_delta", partial_json: JSON.stringify(content.input) }
    : { type: "text_delta", text: content.text };
  const events = [
    ["message_start", { type: "message_start", message: start }],
    ["content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: initial,
    }],
    ["content_block_delta", { type: "content_block_delta", index: 0, delta }],
    ["content_block_stop", { type: "content_block_stop", index: 0 }],
    ["message_delta", {
      type: "message_delta",
      delta: { stop_reason: stopReason, stop_sequence: null },
      usage: { output_tokens: 20 },
    }],
    ["message_stop", { type: "message_stop" }],
  ];
  const data = events.map(([name, value]) =>
    "event: " + name + "\ndata: " + JSON.stringify(value) + "\n\n"
  ).join("");
  return new Response(encoder.encode(data), {
    headers: {
      "content-type": "text/event-stream",
      "request-id": "req_probe_" + calls,
    },
  });
});
await server.finished;
