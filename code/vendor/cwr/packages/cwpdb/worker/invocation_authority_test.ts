import { assertEquals, assertRejects } from "@std/assert";
import type { Storage, Transaction } from "./engine.ts";
import { Invocations, type Mutation } from "./invocations.ts";
class Memory implements Storage {
  data = new Map<string, unknown>();
  async get<T>(key: string): Promise<T | undefined> {
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
const context = {
    workspace_ref: "cwp://test/workspace",
    caller_ref: "cwp://test/coordinator",
  },
  invocation = "cwp://test/invocation";
const assignment = {
  assignment_ref: "cwp://test/assignment",
  executor_ref: "cwp://test/executor",
  generation: 1,
  route_fingerprint: "route",
  expires_at_unix_ms: 1000,
};
function request(
  id: string,
  revision: string | null,
  operation: Mutation["operation"],
): Mutation {
  return {
    context,
    invocation_ref: invocation,
    request_id: id,
    expected_revision: revision,
    operation,
  };
}
Deno.test("stop during append rolls back event cursor and receipt before retry", async () => {
  const storage = new Memory(),
    allowed = new Invocations(storage, async () => {});
  const admitted = await allowed.mutate(
    request("admit", null, {
      operation: "admit",
      conversation_ref: "cwp://test/conversation",
      parent_invocation_ref: null,
      admitted: {},
    }),
    10,
  );
  if (admitted.outcome !== "committed") throw new Error("admission");
  const assigned = await allowed.mutate(
    request("assign", admitted.receipt.revision, {
      operation: "assign",
      assignment,
    }),
    10,
  );
  if (assigned.outcome !== "committed") throw new Error("assignment");
  const append = request("append", assigned.receipt.revision, {
    operation: "append",
    ...assignment,
    event: { sequence: 0, kind: "progress", payload: { text: "private" } },
  });
  let checks = 0;
  const stopped = new Invocations(storage, async () => {
    if (++checks > 1) throw new Error("normal CWR stop fence");
  });
  await assertRejects(() => stopped.mutate(append, 10), Error, "stop fence");
  assertEquals(checks, 2);
  assertEquals(
    (await allowed.get(context, invocation))?.revision,
    assigned.receipt.revision,
  );
  assertEquals((await allowed.get(context, invocation))?.next_sequence, 0);
  assertEquals((await allowed.events(context, invocation, null, 10)).items, []);
  assertEquals(await allowed.receipt(context, invocation, "append"), {
    outcome: "not_committed",
  });
  const accepted = await allowed.mutate(append, 10);
  assertEquals(await allowed.mutate(append, 10), accepted);
});

Deno.test("receipt lookup rechecks authority before publishing retained data", async () => {
  const storage = new Memory(),
    allowed = new Invocations(storage, async () => {});
  await allowed.mutate(
    request("admit", null, {
      operation: "admit",
      conversation_ref: "cwp://test/conversation",
      parent_invocation_ref: null,
      admitted: {},
    }),
    10,
  );
  let checks = 0;
  const revoked = new Invocations(storage, async () => {
    if (++checks > 1) throw new Error("authority revoked");
  });
  await assertRejects(
    () => revoked.receipt(context, invocation, "admit"),
    Error,
    "authority revoked",
  );
  assertEquals(checks, 2);
  assertEquals(
    (await allowed.receipt(context, invocation, "admit") as { outcome: string })
      .outcome,
    "retained",
  );
});
