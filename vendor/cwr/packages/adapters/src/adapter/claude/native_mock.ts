// Native CLI history probe. The model endpoint checks the actual history sent
// by Claude after a process restart; it never contacts a remote provider.
const directory = Deno.args[0];
let sequence = 0;
const server = Deno.serve({
  hostname: "127.0.0.1",
  port: 0,
  onListen: ({ port }) =>
    Deno.writeTextFileSync(`${directory}/mock-port`, String(port)),
}, async (request) => {
  const path = new URL(request.url).pathname;
  if (!path.includes("messages") || request.method !== "POST") {
    return Response.json({});
  }
  if (path.includes("count_tokens")) {
    return Response.json({ input_tokens: 200 });
  }
  const body = await request.json();
  const messages = body.messages ?? [];
  const serialized = JSON.stringify(messages);
  const followup = serialized.includes("FOLLOWUP correction-blue");
  const hasPrivateResult = messages.some((
    message: { role: string; content: unknown },
  ) =>
    message.role === "user" &&
    JSON.stringify(message.content).includes("tool_result") &&
    JSON.stringify(message.content).includes("native-private-marker")
  );
  if (followup && !hasPrivateResult) {
    return Response.json({
      type: "error",
      error: {
        type: "invalid_request_error",
        message: "Private tool history was lost",
      },
    }, { status: 400 });
  }
  const tool = !followup && !hasPrivateResult;
  const content = tool
    ? {
      type: "tool_use",
      id: "private-probe",
      name: "mcp__cwp__test--probe--value",
      input: { value: 7 },
    }
    : {
      type: "text",
      text: followup ? "native-private-marker correction-blue" : "Ready",
    };
  const message = {
    id: `msg_${++sequence}`,
    type: "message",
    role: "assistant",
    model: body.model,
    content: [content],
    stop_reason: tool ? "tool_use" : "end_turn",
    stop_sequence: null,
    usage: { input_tokens: 200, output_tokens: 20 },
  };
  Deno.writeTextFileSync(
    `${directory}/native-requests.jsonl`,
    JSON.stringify({ followup, hasPrivateResult, tool }) + "\n",
    { append: true },
  );
  if (!body.stream) return Response.json(message);
  const events = [
    ["message_start", {
      type: "message_start",
      message: { ...message, content: [], stop_reason: null },
    }],
    ["content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: tool
        ? { ...content, input: {} }
        : { type: "text", text: "" },
    }],
    ["content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: tool
        ? {
          type: "input_json_delta",
          partial_json: JSON.stringify(content.input),
        }
        : { type: "text_delta", text: content.text },
    }],
    ["content_block_stop", { type: "content_block_stop", index: 0 }],
    ["message_delta", {
      type: "message_delta",
      delta: { stop_reason: message.stop_reason, stop_sequence: null },
      usage: { output_tokens: 20 },
    }],
    ["message_stop", { type: "message_stop" }],
  ];
  return new Response(
    events.map(([name, event]) =>
      `event: ${name}\ndata: ${JSON.stringify(event)}\n\n`
    ).join(""),
    { headers: { "content-type": "text/event-stream" } },
  );
});
await server.finished;
