import { assertEquals, assertRejects } from "@std/assert";
import type { Storage, Transaction } from "./engine.ts";
import { Graph } from "./conversation_graph.ts";
import { Conversations } from "./conversations.ts";
import type { ReadSelectionRequest } from "./read_selection_types.ts";
class Memory implements Storage {
  data = new Map<string, unknown>();
  reads = 0;
  async get<T>(key: string): Promise<T | undefined> {
    this.reads++;
    return this.data.get(key) as T | undefined;
  }
  async put<T>(key: string, value: T): Promise<void> {
    this.data.set(key, structuredClone(value));
  }
  async delete(key: string): Promise<void> {
    this.data.delete(key);
  }
  async transaction<T>(
    work: (transaction: Transaction) => Promise<T>,
  ): Promise<T> {
    return await work(this);
  }
}
const runtime = "cwp://fixture/runtime",
  conversation = "cwp://fixture/conversation";
const scope = {
  runtime_root_ref: runtime,
  context: {
    workspace_ref: "cwp://fixture/workspace",
    caller_ref: "cwp://fixture/caller",
  },
};
function request(): ReadSelectionRequest {
  return {
    scope,
    selection: { operation: "current", conversation_ref: conversation },
    expected_commit_revision: "0",
    deadline_unix_ms: Date.now() + 10000,
    max_bytes: 1048576,
  };
}
Deno.test("finite selection checks domain authorization before reading graph facts", async () => {
  const storage = new Memory();
  const backend = new Conversations(storage, async (_, reference) => {
    if (reference === conversation) throw new Error("domain denied");
  });
  await assertRejects(
    () => backend.readSelection(request()),
    Error,
    "domain denied",
  );
  assertEquals(storage.reads, 0);
});
Deno.test("finite selection refuses publication when domain authority is revoked", async () => {
  const storage = new Memory();
  await new Graph(storage, scope).ensure();
  storage.reads = 0;
  let checks = 0;
  const backend = new Conversations(storage, async (_, reference) => {
    if (reference === conversation && ++checks > 1) {
      throw new Error("domain revoked");
    }
  });
  await assertRejects(
    () => backend.readSelection(request()),
    Error,
    "domain revoked",
  );
  assertEquals(checks, 2);
  assertEquals(storage.reads > 0, true);
});
