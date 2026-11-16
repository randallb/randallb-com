import { assertEquals, assertRejects } from "@std/assert";
import type { Storage, Transaction } from "./engine.ts";
import { Graph } from "./conversation_graph.ts";
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
const scope: Scope = {
  context: {
    workspace_ref: "cwp://fixture/history-index/workspace",
    caller_ref: "cwp://fixture/history-index/host",
  },
  runtime_root_ref: "cwp://fixture/history-index/runtime",
};
const conversation = "cwp://fixture/history-index/conversation",
  message = "cwp://fixture/history-index/message",
  activity = "cwp://fixture/history-index/activity";
async function seed(storage: Memory) {
  const graph = new Graph(storage, scope);
  await graph.ensure();
  await graph.put(conversation, "conversation", { title: "Closed" });
  await graph.put(message, "message", {
    record: { text: "Fictional" },
    streaming: { active: true },
  });
  await graph.put(activity, "activity", {
    record: {
      event: {
        kind: "turn_completed",
        invocation: { thread: { nested: { messages: [] } } },
      },
    },
  });
  await graph.putEdge(conversation, "message", message, 0, {});
  await graph.putEdge(conversation, "activity", activity, 0, {});
  return graph;
}
Deno.test("history flags explicitly backfill retained graphs then avoid all history bodies", async () => {
  const storage = new Memory(), graph = await seed(storage);
  for (const key of storage.data.keys()) {
    if (key.includes('"history-v1"')) storage.data.delete(key);
  }
  const backend = new Conversations(storage, async () => {});
  const first = (await backend.historyStatus(scope, conversation))!;
  assertEquals([
    first.has_streaming,
    first.has_legacy_context,
    first.terminal_activity,
  ], [true, true, true]);
  storage.reads = [];
  assertEquals(await backend.historyStatus(scope, conversation), first);
  assertEquals(
    storage.reads.filter((key) =>
      key.includes('"record"') &&
      (key.includes(message) || key.includes(activity))
    ),
    [],
  );
  await graph.put(message, "message", { record: { text: "Fictional" } });
  const changed = (await backend.historyStatus(scope, conversation))!;
  assertEquals(changed.has_streaming, false);
  await graph.removeEdge(conversation, "activity", activity);
  const removed = (await backend.historyStatus(scope, conversation))!;
  assertEquals([removed.has_legacy_context, removed.terminal_activity], [
    false,
    false,
  ]);
  await graph.putEdge(conversation, "activity", activity, 0, {});
  assertEquals(
    (await backend.historyStatus(scope, conversation))!.terminal_activity,
    true,
  );
});
Deno.test("history authorization denial precedes reads and late denial rolls back backfill", async () => {
  const storage = new Memory();
  await seed(storage);
  for (const key of storage.data.keys()) {
    if (key.includes('"history-v1"')) storage.data.delete(key);
  }
  storage.reads = [];
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        throw new Error("denied");
      }).historyStatus(scope, conversation),
    Error,
    "denied",
  );
  assertEquals(storage.reads, []);
  const before = structuredClone([...storage.data]);
  let checks = 0;
  const backend = new Conversations(storage, async () => {
    if (++checks === 2) throw new Error("revoked");
  });
  await assertRejects(
    () => backend.historyStatus(scope, conversation),
    Error,
    "revoked",
  );
  assertEquals([...storage.data], before);
});
