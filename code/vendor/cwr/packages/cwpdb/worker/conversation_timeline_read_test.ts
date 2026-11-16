import { assert, assertEquals, assertRejects } from "@std/assert";
import { derived, Graph, key, rootRef } from "./conversation_graph.ts";
import { Conversations } from "./conversations.ts";
import type { Storage, Transaction } from "./engine.ts";
import type { Scope } from "./conversation_types.ts";
import type { TimelineReadRequest } from "./conversation_timeline_read.ts";
class Memory implements Storage {
  data = new Map<string, unknown>();
  get<T>(key: string): Promise<T | undefined> {
    return Promise.resolve(this.data.get(key) as T | undefined);
  }
  put<T>(key: string, value: T) {
    this.data.set(key, structuredClone(value));
    return Promise.resolve();
  }
  delete(key: string) {
    this.data.delete(key);
    return Promise.resolve();
  }
  async transaction<T>(run: (tx: Transaction) => Promise<T>) {
    const before = structuredClone(this.data);
    try {
      return await run(this);
    } catch (error) {
      this.data = before;
      throw error;
    }
  }
}
Deno.test("timeline retained projected lookup backfill is scoped and rolls back on late denial", async () => {
  const base = "cwp://fixture/timeline-retained",
    conversation = `${base}/conversation`,
    connection = `${base}/connection`,
    message = `${base}/message`,
    invocation = `${base}/invocation`,
    activity = `${base}/activity`,
    actor = `${base}/actor`;
  const scope: Scope = {
    context: { workspace_ref: `${base}/workspace`, caller_ref: `${base}/host` },
    runtime_root_ref: `${base}/runtime`,
  };
  const storage = new Memory(), graph = new Graph(storage, scope);
  await graph.ensure();
  await graph.put(conversation, "conversation", {
    commit_revision: 1,
    timeline_revision: 1,
  });
  await graph.putEdge(rootRef(scope), "conversation", conversation, 0, {});
  await graph.putEdge(conversation, "participant", actor, 0, {});
  await graph.put(message, "message", {
    record: {
      message_ref: message,
      author_actor_ref: actor,
      addressed_actor_refs: [],
      text: "source",
    },
  });
  await graph.putEdge(conversation, "message", message, 0, {});
  await graph.put(connection, "conversation_timeline_connection", {
    store_sequence: 0,
    revision: 1,
  });
  await graph.putEdge(
    conversation,
    "conversation_timeline_root",
    connection,
    0,
    {},
  );
  for (
    const [position, kind, reference] of [[0, "message", message], [
      1,
      "invocation",
      invocation,
    ]] as const
  ) {
    const item = `${base}/item${position}`;
    await graph.put(item, "conversation_timeline_item", {
      publication_ref: `${item}/publication`,
    });
    await graph.putEdge(
      connection,
      "conversation_timeline_item",
      item,
      position,
      {},
    );
    await graph.putEdge(
      item,
      `conversation_timeline_${kind}`,
      reference,
      0,
      {},
    );
  }
  const record = {
    activity_ref: activity,
    sequence: 1,
    occurred_at_unix_ms: 1,
    event: {
      kind: "work_lifecycle",
      source_message_ref: message,
      invocation_ref: invocation,
      parent_invocation_ref: null,
      state: "completed",
    },
  };
  await graph.putEdge(conversation, "timeline_activity", activity, 1, {
    record,
  });
  const projected = await derived("timeline_activity", [
    conversation,
    activity,
  ]);
  await graph.putEdge(invocation, "timeline_invocation", projected, 1, {});
  await graph.putEdge(message, "timeline_source", projected, 1, {});
  const locator = key(scope.runtime_root_ref, "edge-reference", projected);
  await storage.delete(locator);
  const request: TimelineReadRequest = {
    scope,
    conversation_ref: conversation,
    after: null,
    before: null,
    first: 10,
    last: null,
    participant_actor_ref: actor,
    include_execution: false,
    deadline_unix_ms: Date.now() + 30000,
    max_bytes: 1048576,
  };
  let calls = 0;
  const denied = new Conversations(storage, () => {
    if (++calls === 2) throw new Error("revoked");
    return Promise.resolve();
  });
  await assertRejects(() => denied.timelinePage(request), Error, "revoked");
  assertEquals(await storage.get(locator), undefined);
  const allowed = new Conversations(storage, () => Promise.resolve());
  const page = await allowed.timelinePage(request);
  assert(page);
  assertEquals(page.anchors.length, 2);
  assertEquals(page.activity, [record]);
  assertEquals(await storage.get(locator), [
    conversation,
    "timeline_activity",
    activity,
  ]);
  await assertRejects(
    () =>
      allowed.timelinePage({
        ...request,
        participant_actor_ref: `${base}/foreign`,
      }),
    Error,
    "requesting participant",
  );
});
