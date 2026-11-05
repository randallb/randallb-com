import { assert, assertEquals, assertRejects } from "@std/assert";
import { Graph, key, rootRef, schema } from "./conversation_graph.ts";
import { Conversations } from "./conversations.ts";
import type { Storage, Transaction } from "./engine.ts";
import type { Scope } from "./conversation_types.ts";
import type {
  CompletionPageRequest,
  ContentPageRequest,
} from "./conversation_page_types.ts";
class Memory implements Storage {
  data = new Map<string, unknown>();
  reads: string[] = [];
  async get<T>(key: string): Promise<T | undefined> {
    this.reads.push(key);
    return this.data.get(key) as T | undefined;
  }
  async put<T>(key: string, value: T) {
    this.data.set(key, structuredClone(value));
  }
  async delete(key: string) {
    this.data.delete(key);
  }
  async transaction<T>(work: (tx: Transaction) => Promise<T>): Promise<T> {
    const old = structuredClone(this.data);
    try {
      return await work(this);
    } catch (e) {
      this.data = old;
      throw e;
    }
  }
}
const base = "cwp://fixture/pages",
  conversation = `${base}/conversation`,
  root = `${base}/root`;
const scope: Scope = {
  context: { workspace_ref: `${base}/workspace`, caller_ref: `${base}/caller` },
  runtime_root_ref: `${base}/runtime`,
};
const msg = (n: number) => `${base}/message/${n}`,
  inv = (n: number) => `${base}/invocation/${n}`;
function request(extra: Partial<ContentPageRequest> = {}): ContentPageRequest {
  return {
    scope,
    conversation_ref: conversation,
    after: null,
    before: null,
    first: 2,
    last: null,
    max_bytes: 1024 * 1024,
    ...extra,
  };
}
function completion(
  extra: Partial<CompletionPageRequest> = {},
): CompletionPageRequest {
  return {
    scope,
    conversation_ref: conversation,
    after_sequence: 0,
    through_sequence: null,
    cursor_revision: null,
    first: 1,
    max_bytes: 1024 * 1024,
    ...extra,
  };
}
async function seed(canonical = false) {
  const storage = new Memory(), graph = new Graph(storage, scope);
  await graph.ensure();
  await graph.put(conversation, "conversation", {
    commit_revision: 9,
    timeline_revision: 10,
  });
  await graph.putEdge(rootRef(scope), "conversation", conversation, 0, {});
  await graph.putEdge(rootRef(scope), "root_conversation", conversation, 0, {});
  if (canonical) {
    await graph.put(root, "conversation_timeline_connection", {
      revision: 3,
      store_sequence: 0,
    });
    await graph.putEdge(
      conversation,
      "conversation_timeline_root",
      root,
      0,
      {},
    );
  }
  for (let n = 0; n < 5; n++) {
    await graph.put(msg(n), "message", {
      record: {
        message_ref: msg(n),
        text: `message ${n}`,
        ...(n === 1 ? { progress: null } : {}),
      },
      ...(n === 2
        ? {
          streaming: {
            conversation_ref: conversation,
            message: { message_ref: msg(n) },
          },
        }
        : {}),
    });
    await graph.putEdge(conversation, "message", msg(n), n, {});
    if (canonical) {
      const item = `${base}/item/${n}`;
      await graph.put(item, "conversation_timeline_item", {});
      await graph.putEdge(root, "conversation_timeline_item", item, n, {});
      await graph.putEdge(item, "conversation_timeline_message", msg(n), 0, {});
    }
  }
  for (let n = 1; n <= 3; n++) {
    await graph.put(inv(n), "invocation", { record: {} });
    await graph.putEdge(conversation, "completed_invocation", inv(n), n, {});
    if (canonical && n !== 2) {
      const item = `${base}/invitem/${n}`;
      await graph.put(item, "conversation_timeline_item", {});
      await graph.putEdge(root, "conversation_timeline_item", item, 10 + n, {});
      await graph.putEdge(
        item,
        "conversation_timeline_invocation",
        inv(n),
        0,
        {},
      );
    }
  }
  return {
    storage,
    graph,
    backend: new Conversations(storage, async () => {}),
  };
}
Deno.test("content Relay selection keeps zero/both bounds and loads only chosen bodies", async () => {
  for (const canonical of [false, true]) {
    const { storage, backend } = await seed(canonical);
    storage.reads = [];
    const result = (await backend.contentPage(request()))!;
    assertEquals(
      result.items.map((i) => i.record.message_ref),
      canonical ? [msg(0), msg(3)] : [msg(0), msg(2)],
    );
    assertEquals(result.total_count, canonical ? 3 : 4);
    assert(result.has_next_page);
    const bodies = storage.reads.filter((k) =>
      k.includes('"record"') && k.includes("/message/")
    );
    assertEquals(bodies, [
      key(scope.runtime_root_ref, "record", msg(0)),
      key(scope.runtime_root_ref, "record", canonical ? msg(3) : msg(2)),
    ]);
    const empty = (await backend.contentPage(request({ first: 0 })))!;
    assertEquals(empty.items, []);
    assert(empty.has_next_page);
    const both = (await backend.contentPage(request({ first: 3, last: 1 })))!;
    assertEquals(both.items.length, 1);
    assert(both.has_previous_page);
    const tail =
      (await backend.contentPage(request({ first: null, last: 1 })))!;
    assertEquals(tail.items[0].record.message_ref, msg(4));
    assert(tail.has_previous_page);
    const after = {
      position: result.items[0].position,
      revision: result.revision,
    };
    const next = (await backend.contentPage(request({ after, first: 1 })))!;
    assertEquals(
      next.items[0].record.message_ref,
      result.items[1].record.message_ref,
    );
    await assertRejects(
      () =>
        backend.contentPage(
          request({ after: { position: 1, revision: result.revision } }),
        ),
      Error,
      "boundary is not",
    );
    await assertRejects(
      () => backend.contentPage(request({ first: 1, max_bytes: 1 })),
      Error,
      "byte budget",
    );
    if (canonical) {
      await assertRejects(
        () =>
          backend.contentPage(
            request({ after: { ...after, revision: "stale" } }),
          ),
        Error,
        "earlier revision",
      );
    }
  }
});
Deno.test("completion pages fix upper fence and filter canonical invisible invocations", async () => {
  for (const canonical of [false, true]) {
    const { backend } = await seed(canonical);
    const first = (await backend.completionPage(completion()))!;
    assertEquals(first.items, [{ invocation_ref: inv(1), sequence: 1 }]);
    assertEquals(first.through_sequence, 3);
    assert(first.has_next_page);
    const second = (await backend.completionPage(
      completion({
        after_sequence: first.next_after,
        through_sequence: first.through_sequence,
        cursor_revision: first.revision,
      }),
    ))!;
    assertEquals(second.items, [{
      invocation_ref: inv(canonical ? 3 : 2),
      sequence: canonical ? 3 : 2,
    }]);
    if (canonical) {
      await assertRejects(
        () =>
          backend.completionPage(
            completion({ after_sequence: 1, cursor_revision: "stale" }),
          ),
        Error,
        "earlier canonical",
      );
    }
    await assertRejects(
      () => backend.completionPage(completion({ through_sequence: -1 })),
      Error,
      "invalid completion",
    );
  }
});
Deno.test("page authorization precedes reads and is checked before returning results", async () => {
  const { storage } = await seed();
  storage.reads = [];
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        throw new Error("denied");
      }).contentPage(request()),
    Error,
    "denied",
  );
  assertEquals(storage.reads, []);
  let calls = 0;
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        if (++calls === 2) throw new Error("revoked");
      }).completionPage(completion()),
    Error,
    "revoked",
  );
});
Deno.test("message predicate replacement deletion readdition and retained backfill stay current", async () => {
  const { storage, graph, backend } = await seed();
  await graph.put(msg(1), "message", {
    record: { message_ref: msg(1), text: "now public" },
  });
  assertEquals(
    (await backend.contentPage(request()))!.items.map((i) =>
      i.record.message_ref
    ),
    [msg(0), msg(1)],
  );
  await graph.remove(msg(0));
  assertEquals(
    (await backend.contentPage(request({ first: 1 })))!.items[0].record
      .message_ref,
    msg(1),
  );
  await graph.put(msg(0), "message", {
    record: { message_ref: msg(0), text: "returned" },
  });
  await graph.putEdge(conversation, "message", msg(0), 0, {});
  for (const k of storage.data.keys()) {
    if (k.includes('"page-message-v1"')) storage.data.delete(k);
  }
  storage.reads = [];
  await backend.contentPage(request({ first: 1 }));
  assert(
    storage.reads.some((k) =>
      k.includes('"record"') && k.includes("/message/4")
    ),
  );
  storage.reads = [];
  await backend.contentPage(request({ first: 1 }));
  assertEquals(
    storage.reads.filter((k) =>
      k.includes('"record"') && k.includes("/message/")
    ).length,
    1,
  );
});
Deno.test("canonical revision refuses missing or tied explicit order", async () => {
  const { graph, backend } = await seed(true);
  const child = `${base}/child`;
  await graph.put(child, "conversation_timeline_connection", {
    revision: 1,
    store_sequence: 0,
  });
  await graph.putEdge(
    `${base}/item/0`,
    "conversation_timeline_child",
    child,
    0,
    {},
  );
  await assertRejects(
    () => backend.contentPage(request()),
    Error,
    "order migration required",
  );
  await graph.put(child, "conversation_timeline_connection", { revision: 1 });
  await assertRejects(
    () => backend.completionPage(completion()),
    Error,
    "order migration required",
  );
});
Deno.test("retained flag backfill refuses aggregate bytes and rolls back partial indexes", async () => {
  const { storage, graph, backend } = await seed();
  const text = "x".repeat(13 * 1024 * 1024);
  for (const n of [0, 3]) {
    await graph.put(msg(n), "message", {
      record: { message_ref: msg(n), text },
    });
  }
  for (const k of storage.data.keys()) {
    if (k.includes('"page-message-v1"')) storage.data.delete(k);
  }
  await assertRejects(
    () => backend.contentPage(request({ first: 0 })),
    Error,
    "backfill byte limit",
  );
  assertEquals(
    Array.from(storage.data.keys()).filter((k) =>
      k.includes('"page-message-v1"')
    ),
    [],
  );
});
Deno.test("late page denial rolls back backfill and canonical revisions fence replacements", async () => {
  const { storage, graph, backend } = await seed(true);
  const original = (await backend.contentPage(request()))!;
  await graph.put(root, "conversation_timeline_connection", {
    store_sequence: 0,
    revision: 4,
  });
  await assertRejects(
    () =>
      backend.contentPage(
        request({
          after: {
            position: original.items[0].position,
            revision: original.revision,
          },
        }),
      ),
    Error,
    "earlier revision",
  );
  for (const k of storage.data.keys()) {
    if (k.includes('"page-message-v1"')) storage.data.delete(k);
  }
  let calls = 0;
  await assertRejects(
    () =>
      new Conversations(storage, async () => {
        if (++calls === 2) throw new Error("late denial");
      }).contentPage(request({ first: 0 })),
    Error,
    "late denial",
  );
  assertEquals(
    Array.from(storage.data.keys()).filter((k) =>
      k.includes('"page-message-v1"')
    ),
    [],
  );
  const corrupt = key(scope.runtime_root_ref, "page-message-v1", msg(0));
  storage.data.set(corrupt, { version: 99 });
  await assertRejects(
    () => backend.contentPage(request()),
    Error,
    "invalid message page flags",
  );
});
