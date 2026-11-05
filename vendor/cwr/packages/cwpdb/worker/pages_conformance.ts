import { assert, assertEquals, assertRejects } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import type {
  Mutation,
  Scope,
  SnapshotPage,
  WriteRecord,
} from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
interface Cursor {
  position: number;
  revision: string | null;
}
interface Content {
  revision: string;
  canonical: boolean;
  items: { position: number; record: { message_ref: string; text: string } }[];
  total_count: number;
  has_next_page: boolean;
  has_previous_page: boolean;
  commit_revision: string;
}
interface Completions {
  revision: string;
  items: { invocation_ref: string; sequence: number }[];
  through_sequence: number;
  next_after: number;
  has_next_page: boolean;
}
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8817";
const targets = new Set((Deno.args[2] ?? "native,workerd,remote").split(","));
const directory = await Deno.makeTempDir({ prefix: "cwpdb-pages-" });
const hosted: Transport = async (body) => {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: "Bearer fictional-local-cwpdb-conformance-only",
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const envelope = await response.json();
  if (!envelope.ok) throw new Error(envelope.error);
  return envelope.result;
};
async function exercise(name: string, call: Transport): Promise<void> {
  const workspace = `cwp://fixture/workspaces/pages-${crypto.randomUUID()}`;
  const scope: Scope = {
    context: { workspace_ref: workspace, caller_ref: `${workspace}/host` },
    runtime_root_ref: `${workspace}/runtime`,
  };
  const conversation = `${workspace}/canonical`,
    legacy = `${workspace}/legacy`,
    actor = `${workspace}/actor`;
  const connection = "cwp://fixture/page-canonical/root",
    childConnection = "cwp://fixture/page-canonical/child";
  const a = `${workspace}/a`,
    b = `${workspace}/b`,
    progress = `${workspace}/progress`,
    streaming = `${workspace}/streaming`,
    hidden = `${workspace}/hidden`;
  const v1 = `${workspace}/v1`, v2 = `${workspace}/v2`, v3 = `${workspace}/v3`;
  const item = (index: number) => `${workspace}/items/${index}`;
  const record = (
    reference: string,
    kind: WriteRecord["kind"],
    properties: Record<string, unknown>,
    owner_ref: string | null = null,
  ): WriteRecord => ({ reference, kind, properties, owner_ref });
  const member = (
    source_ref: string,
    kind: string,
    target_ref: string,
    position = 0,
  ): Mutation["changes"]["memberships"][number] => ({
    operation: "upsert",
    source_ref,
    kind,
    member: { target_ref, position, properties: {} },
  });
  const body = (reference: string, text: string) => ({
    message_ref: reference,
    author_actor_ref: actor,
    addressed_actor_refs: [],
    text,
  });
  const query = () =>
    call({
      operation: "read_conversation_graph",
      request: {
        scope,
        selection: { selection: "bootstrap" },
        continuation: null,
        page_size: 100,
        max_bytes: 1048576,
        expected_revision: null,
      },
    }) as Promise<SnapshotPage>;
  let revision = (await query()).revision;
  let serial = 0;
  const commit = async (changes: Partial<Mutation["changes"]>) => {
    const outcome = await call({
      operation: "commit_conversation_graph",
      request: {
        scope,
        request_id: `pages-${++serial}`,
        expected_revision: revision,
        changes: {
          records: [],
          remove_records: [],
          memberships: [],
          publications: [],
          state_version: null,
          streaming_runs: null,
          repair_legacy: false,
          ...changes,
        },
        journal: null,
        authority_guards: [],
      },
    }) as { revision: string };
    revision = outcome.revision;
  };
  const content = (
    conversation_ref: string,
    window: {
      after?: Cursor | null;
      before?: Cursor | null;
      first?: number | null;
      last?: number | null;
      max_bytes?: number;
    },
  ) =>
    call({
      operation: "conversation_content_page",
      request: {
        scope,
        conversation_ref,
        after: null,
        before: null,
        first: null,
        last: null,
        max_bytes: 1048576,
        ...window,
      },
    }) as Promise<Content | null>;
  const completions = (
    conversation_ref: string,
    window: {
      after_sequence?: number;
      through_sequence?: number | null;
      cursor_revision?: string | null;
      first?: number;
      max_bytes?: number;
    } = {},
  ) =>
    call({
      operation: "conversation_completion_page",
      request: {
        scope,
        conversation_ref,
        after_sequence: 0,
        through_sequence: null,
        cursor_revision: null,
        first: 1,
        max_bytes: 1048576,
        ...window,
      },
    }) as Promise<Completions | null>;
  const work = (reference: string, owner: string) =>
    record(reference, "invocation", {
      record: {
        invocation_ref: reference,
        conversation_ref: owner,
        state: "completed",
        parent_invocation_ref: null,
        forked_from_invocation_ref: null,
      },
    });
  const records: WriteRecord[] = [
    record(conversation, "conversation", { sidebar_eligible: true }),
    record(legacy, "conversation", { sidebar_eligible: true }),
    record(a, "message", { record: body(a, "First — 🐎") }),
    record(b, "message", { record: body(b, "Second é") }),
    record(hidden, "message", { record: body(hidden, "Hidden") }),
    record(progress, "message", {
      record: {
        ...body(progress, "Working"),
        progress: {
          invocation_ref: v1,
          source_activity_ref: `${workspace}/activity/progress`,
        },
      },
    }),
    record(streaming, "message", {
      streaming: {
        conversation_ref: conversation,
        message: body(streaming, "Typing"),
      },
    }),
    record(connection, "conversation_timeline_connection", {
      revision: 3,
      next_sequence: 8,
      store_sequence: 0,
    }, conversation),
    record(childConnection, "conversation_timeline_connection", {
      revision: 7,
      next_sequence: 2,
      store_sequence: 1,
    }, conversation),
    ...[1, 2, 3, 4, 5, 6, 7, 8].map((index) =>
      record(item(index), "conversation_timeline_item", {
        publication_ref: `${workspace}/publication/${index}`,
      }, conversation)
    ),
    work(v1, conversation),
    work(v2, conversation),
    work(v3, conversation),
    ...[1, 2, 3].map((index) => work(`${legacy}/v${index}`, legacy)),
    ...[1, 2, 3].map((index) =>
      record(`${legacy}/m${index}`, "message", {
        record: body(`${legacy}/m${index}`, `Legacy${index}`),
      })
    ),
    record(`${legacy}/progress`, "message", {
      record: {
        ...body(`${legacy}/progress`, "Legacy working"),
        progress: {
          invocation_ref: `${legacy}/v1`,
          source_activity_ref: `${workspace}/activity/legacy`,
        },
      },
    }),
  ];
  const memberships = [
    member(`${scope.runtime_root_ref}/cwpdb`, "conversation", conversation),
    member(`${scope.runtime_root_ref}/cwpdb`, "conversation", legacy, 1),
    member(conversation, "participant", actor),
    member(legacy, "participant", actor),
    ...[a, b, progress, streaming, hidden].map((reference, index) =>
      member(conversation, "message", reference, index)
    ),
    member(conversation, "conversation_timeline_root", connection),
    ...[1, 2, 3, 4, 5, 6, 7].map((index) =>
      member(connection, "conversation_timeline_item", item(index), index)
    ),
    member(childConnection, "conversation_timeline_item", item(8), 1),
    member(item(1), "conversation_timeline_message", a),
    member(item(2), "conversation_timeline_invocation", v1),
    member(item(3), "conversation_timeline_message", a),
    member(item(4), "conversation_timeline_message", progress),
    member(item(5), "conversation_timeline_invocation", v2),
    member(item(5), "conversation_timeline_child", childConnection),
    member(item(6), "conversation_timeline_message", streaming),
    member(item(7), "conversation_timeline_message", b),
    member(item(8), "conversation_timeline_message", b),
    ...[1, 2, 3].map((index) =>
      member(legacy, "message", `${legacy}/m${index}`, index * 10)
    ),
    member(legacy, "message", `${legacy}/progress`, 40),
  ];
  await commit({ records, memberships });
  const all = await content(conversation, { first: 500 });
  assert(all);
  assertEquals(
    all.revision,
    "canonical:v1:5218ee5d77b6c95ab55b9e8cfbf3f47b8538c2a1e160f43c18912cdc33f2845b:1",
  );
  assertEquals(
    all.items.map((item) => [item.position, item.record.message_ref]),
    [[0, a], [5, b]],
  );
  assertEquals(all.total_count, 2);
  assertEquals(all.items.map((item) => item.record.text), [
    "First — 🐎",
    "Second é",
  ]);
  const after: Cursor = { position: 0, revision: all.revision },
    before: Cursor = { position: 5, revision: all.revision };
  const next = await content(conversation, { after, first: 1 });
  assert(next);
  assertEquals(next.items.map((item) => item.position), [5]);
  assertEquals(next.has_previous_page, false);
  const previous = await content(conversation, { before, last: 1 });
  assert(previous);
  assertEquals(previous.items.map((item) => item.position), [0]);
  assertEquals(previous.has_next_page, false);
  const combined = await content(conversation, { first: 2, last: 1 });
  assert(combined);
  assertEquals(combined.items.map((item) => item.position), [5]);
  assertEquals(combined.has_previous_page, true);
  const zero = await content(conversation, { first: 0, last: 2 });
  assert(zero);
  assertEquals(zero.items, []);
  assertEquals(zero.has_next_page, true);
  await assertRejects(
    () =>
      content(conversation, {
        after: { position: 3, revision: all.revision },
        first: 1,
      }),
    Error,
    "boundary",
  );
  await assertRejects(
    () =>
      content(conversation, {
        after: { ...after, revision: "stale" },
        first: 1,
      }),
    Error,
    "revision",
  );
  await assertRejects(
    () => content(conversation, { first: 500, max_bytes: 20 }),
    Error,
    "byte",
  );
  const legacyPage = await content(legacy, { first: 500 });
  assert(legacyPage);
  assertEquals(legacyPage.canonical, false);
  assertEquals(legacyPage.items.map((item) => item.position), [10, 20, 30]);
  assertEquals(legacyPage.total_count, 3);
  const legacyNext = await content(legacy, {
    after: { position: 10, revision: "stale-is-ignored" },
    first: 1,
  });
  assertEquals(legacyNext?.items[0].position, 20);
  const firstCompletion = await completions(conversation);
  assert(firstCompletion);
  assertEquals(firstCompletion.items, [{ invocation_ref: v1, sequence: 1 }]);
  assertEquals(firstCompletion.through_sequence, 2);
  assertEquals(firstCompletion.has_next_page, true);
  const lastCompletion = await completions(conversation, {
    after_sequence: 1,
    through_sequence: 2,
    cursor_revision: firstCompletion.revision,
    first: 200,
  });
  assertEquals(lastCompletion?.items, [{ invocation_ref: v2, sequence: 2 }]);
  await assertRejects(
    () =>
      completions(conversation, {
        after_sequence: 1,
        cursor_revision: "stale",
      }),
    Error,
    "revision",
  );
  assertEquals(
    (await completions(legacy, {
      after_sequence: 4,
      cursor_revision: "ignored",
      first: 200,
    }))?.items.map((item) => item.sequence),
    [5, 6],
  );
  await commit({
    records: [
      record(progress, "message", {
        record: {
          ...body(progress, "New working progress"),
          progress: {
            invocation_ref: v1,
            source_activity_ref: `${workspace}/activity/progress`,
          },
        },
      }),
    ],
  });
  assertEquals(
    (await content(conversation, { first: 500 }))?.revision,
    all.revision,
    "progress-only update must keep public cursor stable",
  );
  await commit({
    records: [
      record(
        connection,
        "conversation_timeline_connection",
        { revision: 4 },
        conversation,
      ),
    ],
  });
  await assertRejects(
    () => content(conversation, { after, first: 1 }),
    Error,
    "revision",
  );
  assertEquals(await content(`${workspace}/missing`, { first: 1 }), null);
  await commit({
    records: [
      record(childConnection, "conversation_timeline_connection", {
        store_sequence: 0,
      }, conversation),
    ],
  });
  await assertRejects(
    () => content(conversation, { first: 1 }),
    Error,
    "order",
  );
  await commit({
    records: [
      record(childConnection, "conversation_timeline_connection", {
        store_sequence: null,
      }, conversation),
    ],
  });
  await assertRejects(
    () => content(conversation, { first: 1 }),
    Error,
    "order",
  );

  console.log(JSON.stringify({
    backend: name,
    result: "PASS",
    checks: [
      "canonical-revision-wire-golden",
      "streaming-hidden-before-flat-position",
      "duplicate-first-placement",
      "progress-filtered-cursor-membership",
      "relay-first-last-and-zero",
      "legacy-retained-order",
      "legacy-revision-ignored",
      "same-snapshot-visible-completions",
      "fixed-through-fence",
      "stale-revision-refusal",
      "progress-revision-stability",
      "whole-page-byte-refusal",
      "missing-conversation",
      "duplicate-connection-order-refusal",
      "missing-connection-order-migration-required",
    ],
  }));
}
const native = await processTransport([`${directory}/store.db`], binary),
  remote = await processTransport(["--remote", endpoint], binary);
try {
  if (targets.has("native")) await exercise("native", native.call);
  if (targets.has("workerd")) await exercise("workerd", hosted);
  if (targets.has("remote")) await exercise("remote-store", remote.call);
} finally {
  await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
