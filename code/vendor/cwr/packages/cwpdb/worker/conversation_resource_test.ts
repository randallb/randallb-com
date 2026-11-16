import { ResourceReader } from "./conversation_resource.ts";
import { assertEquals, assertRejects } from "@std/assert";
import type { Storage, Transaction } from "./engine.ts";
import { Graph, rootRef } from "./conversation_graph.ts";
import { Conversations } from "./conversations.ts";
import type { Scope } from "./conversation_types.ts";
class Memory implements Storage {
  data = new Map<string, unknown>();
  reads: string[] = [];
  async get<T>(key: string): Promise<T | undefined> {
    this.reads.push(key);
    return this.data.get(key) as T | undefined;
  }
  async put<T>(key: string, value: T): Promise<void> {
    this.data.set(key, structuredClone(value));
  }
  async delete(key: string): Promise<void> {
    this.data.delete(key);
  }
  async transaction<T>(work: (tx: Transaction) => Promise<T>): Promise<T> {
    const before = structuredClone(this.data);
    try {
      return await work(this);
    } catch (error) {
      this.data = before;
      throw error;
    }
  }
}

const base = "cwp://fixture/resource-boundaries",
  conversation = `${base}/conversation`,
  message = `${base}/message`,
  root = `${base}/timeline`,
  item = `${base}/item`;
const scope: Scope = {
  context: { workspace_ref: `${base}/workspace`, caller_ref: `${base}/caller` },
  runtime_root_ref: `${base}/runtime`,
};
async function seed(storage: Memory) {
  const graph = new Graph(storage, scope);
  await graph.ensure();
  await graph.put(conversation, "conversation", {});
  await graph.putEdge(rootRef(scope), "conversation", conversation, 0, {});
  await graph.putEdge(rootRef(scope), "root_conversation", conversation, 0, {});
  await graph.put(root, "conversation_timeline_connection", {});
  await graph.put(item, "conversation_timeline_item", {});
  await graph.putEdge(conversation, "conversation_timeline_root", root, 0, {});
  await graph.putEdge(root, "conversation_timeline_item", item, 0, {});
  await graph.putEdge(item, "conversation_timeline_message", message, 0, {});
  await graph.put(message, "message", {
    streaming: {
      conversation_ref: conversation,
      message: { message_ref: message, text: "Stream" },
    },
  });
  return graph;
}
Deno.test("resource streaming requires current owned canonical placement and matching identity", async () => {
  const storage = new Memory(),
    graph = await seed(storage),
    backend = new Conversations(storage, async () => {});
  const result = await backend.resource(scope, message);
  assertEquals(result?.resource, {
    kind: "message",
    record: { message_ref: message, text: "Stream" },
    streaming: true,
    occurred_at_unix_ms: null,
  });
  await graph.removeEdge(root, "conversation_timeline_item", item);
  assertEquals(await backend.resource(scope, message), null);
  await graph.putEdge(root, "conversation_timeline_item", item, 0, {});
  await graph.removeEdge(rootRef(scope), "conversation", conversation);
  assertEquals(await backend.resource(scope, message), null);
  await graph.putEdge(rootRef(scope), "conversation", conversation, 0, {});
  await graph.put(message, "message", {
    streaming: {
      conversation_ref: conversation,
      message: { message_ref: `${base}/wrong` },
    },
  });
  await assertRejects(
    () => backend.resource(scope, message),
    Error,
    "identity mismatch",
  );
});
Deno.test("resource authorization checks precede reads and recheck before returning body", async () => {
  const storage = new Memory();
  await seed(storage);
  storage.reads = [];
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        throw new Error("denied");
      }).resource(scope, message),
    Error,
    "denied",
  );
  assertEquals(storage.reads, []);
  let count = 0;
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        if (++count === 3) throw new Error("revoked");
      }).resource(scope, message),
    Error,
    "revoked",
  );
  const other = {
    ...scope,
    context: { ...scope.context, workspace_ref: `${base}/other` },
  };
  await assertRejects(
    () => new Conversations(storage, async () => {}).resource(other, message),
    Error,
    "scope mismatch",
  );
});
Deno.test("legacy child reply fallback requires a unique invocation identity", async () => {
  const storage = new Memory(),
    graph = await seed(storage),
    backend = new Conversations(storage, async () => {});
  await graph.removeEdge(conversation, "conversation_timeline_root", root);
  const invocation = `${base}/invocation`,
    other = `${base}/other-invocation`,
    source = `${base}/source`,
    reply = `${base}/reply`,
    actor = `${base}/actor`;
  const body = {
    invocation_ref: invocation,
    conversation_ref: conversation,
    parent_invocation_ref: `${base}/parent`,
    state: "completed",
    source_message_ref: source,
    target_actor_ref: actor,
    action_name: "respond_to--message",
  };
  await graph.put(invocation, "invocation", { record: body });
  await graph.putEdge(invocation, "field:target_actor_ref", actor, 0, {});
  await graph.put(reply, "message", {
    record: {
      message_ref: reply,
      reply_to_message_ref: source,
      author_actor_ref: actor,
      text: "Public reply",
    },
  });
  await graph.putEdge(conversation, "message", reply, 1, {});
  let result = await backend.resource(scope, invocation);
  assertEquals(result?.resource.kind, "invocation");
  if (result?.resource.kind !== "invocation") {
    throw new Error("invocation expected");
  }
  assertEquals(result.resource.latest_reply?.message_ref, reply);
  await graph.put(other, "invocation", {
    record: { ...body, invocation_ref: other },
  });
  await graph.putEdge(other, "field:target_actor_ref", actor, 0, {});
  result = await backend.resource(scope, invocation);
  if (result?.resource.kind !== "invocation") {
    throw new Error("invocation expected");
  }
  assertEquals(result.resource.latest_reply, null);
  await graph.put(invocation, "invocation", {
    record: { ...body, reply_message_ref: reply },
  });
  result = await backend.resource(scope, invocation);
  if (result?.resource.kind !== "invocation") {
    throw new Error("invocation expected");
  }
  assertEquals(result.resource.latest_reply?.message_ref, reply);
});

Deno.test("resource timestamps verify the boundary payload after a stale lookup edge", async () => {
  const storage = new Memory(), graph = await seed(storage);
  await graph.put(message, "message", {
    record: { message_ref: message, text: "Retained" },
  });
  await graph.putEdge(conversation, "message", message, 0, {});
  for (
    const [name, time, target, position] of [
      ["wrong-boundary", 2, `${base}/other-message`, 0],
      ["right-boundary", 90, message, 1],
    ] as const
  ) {
    const reference = `${base}/${name}`;
    await graph.put(reference, "activity", {
      record: {
        activity_ref: reference,
        occurred_at_unix_ms: time,
        event: { kind: "message_recorded", message_ref: target },
      },
    });
    await graph.putEdge(conversation, "activity", reference, position, {});
    await graph.putEdge(reference, "field:event.message_ref", message, 0, {});
  }
  const result = await new Conversations(storage, async () => {}).resource(
    scope,
    message,
  );
  assertEquals(result?.resource.kind, "message");
  if (result?.resource.kind === "message") {
    assertEquals(result.resource.occurred_at_unix_ms, 90);
  }
});

Deno.test("legacy child reply uses the source from its retained execution start", async () => {
  const storage = new Memory(), graph = await seed(storage);
  const invocation = `${base}/child`,
    execution = `${base}/execution`,
    start = `${base}/start`,
    actor = `${base}/actor`,
    reply = `${base}/reply`;
  await graph.put(invocation, "invocation", {
    record: {
      invocation_ref: invocation,
      conversation_ref: conversation,
      parent_invocation_ref: `${base}/parent`,
      source_message_ref: null,
      target_actor_ref: actor,
      action_name: "respond_to--message",
      state: "completed",
      responses: [{ status: 200, summary: "Finished" }],
    },
  });
  await graph.putEdge(invocation, "field:target_actor_ref", actor, 0, {});
  await graph.putEdge(invocation, "execution", execution, 0, {});
  await graph.put(start, "activity", {
    record: {
      event: { kind: "turn_started", source_message_ref: message },
    },
  });
  await graph.putEdge(execution, "activity", start, 0, {});
  await graph.put(reply, "message", {
    record: {
      message_ref: reply,
      text: "Exact reply",
      author_actor_ref: actor,
      reply_to_message_ref: message,
    },
  });
  await graph.putEdge(conversation, "message", reply, 0, {});
  for (
    const [reference, content, kind, position] of [
      [`${base}/turn-item`, invocation, "conversation_timeline_invocation", 1],
      [`${base}/reply-item`, reply, "conversation_timeline_message", 2],
    ] as const
  ) {
    await graph.put(reference, "conversation_timeline_item", {});
    await graph.putEdge(
      root,
      "conversation_timeline_item",
      reference,
      position,
      {},
    );
    await graph.putEdge(reference, kind, content, 0, {});
  }
  const result = await new Conversations(storage, async () => {}).resource(
    scope,
    invocation,
  );
  assertEquals(result?.resource.kind, "invocation");
  if (result?.resource.kind === "invocation") {
    assertEquals(result.resource.record.source_message_ref, message);
    assertEquals(result.resource.latest_reply?.message_ref, reply);
  }
});

Deno.test("direct root visibility does not scan repeated canonical placements", async () => {
  const measurements: number[] = [];
  for (const count of [20, 2000]) {
    const storage = new Memory(), graph = await seed(storage);
    for (let index = 0; index < count; index++) {
      const placement = `${base}/repeated/${index}`;
      await graph.put(placement, "conversation_timeline_item", {});
      await graph.putEdge(
        root,
        "conversation_timeline_item",
        placement,
        index + 1,
        {},
      );
      await graph.putEdge(
        placement,
        "conversation_timeline_message",
        message,
        0,
        {},
      );
    }
    storage.reads = [];
    assertEquals(
      await new ResourceReader(graph).visible(
        root,
        message,
        "conversation_timeline_message",
      ),
      true,
    );
    measurements.push(storage.reads.length);
  }
  assertEquals(measurements[1] < measurements[0] * 2, true);
});
