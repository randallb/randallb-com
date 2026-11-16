import { assert, assertEquals, assertRejects } from "@std/assert";
import type { Mutation, Scope, SnapshotPage } from "./conversation_types.ts";

export async function timelineWriteCases(
  call: (body: unknown) => Promise<unknown>,
) {
  const workspace = `cwp://fixture/timeline-writes/${crypto.randomUUID()}`;
  const scope: Scope = {
    context: { workspace_ref: workspace, caller_ref: `${workspace}/actor` },
    runtime_root_ref: `${workspace}/runtime`,
  };
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
  const commit = (request: Mutation) =>
    call({ operation: "commit_conversation_graph", request });
  const initial = await query();
  const conversation = `${workspace}/conversation`,
    connection = `${workspace}/connection`,
    child = `${workspace}/child`,
    message = `${workspace}/message`,
    invocation = `${workspace}/invocation`;
  const request: Mutation = {
    scope,
    request_id: "typed-items",
    expected_revision: initial.revision,
    changes: {
      records: [
        {
          reference: conversation,
          kind: "conversation",
          owner_ref: null,
          properties: { title: "Timeline", sidebar_eligible: true },
        },
        {
          reference: message,
          kind: "message",
          owner_ref: null,
          properties: { record: { text: "Retained" } },
        },
        {
          reference: invocation,
          kind: "invocation",
          owner_ref: null,
          properties: {
            record: { conversation_ref: conversation, state: "running" },
          },
        },
        ...[connection, child].map((reference) => ({
          reference,
          kind: "conversation_timeline_connection" as const,
          owner_ref: conversation,
          properties: { revision: 1, next_sequence: 3, store_sequence: 0 },
        })),
      ],
      remove_records: [],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
      timeline_items: [
        {
          conversation_ref: conversation,
          connection_ref: connection,
          item_ref: `${workspace}/item-one`,
          publication_ref: `${workspace}/publication-one`,
          position: 1,
          content: { kind: "message", message_ref: message },
          child_connection_ref: child,
        },
        {
          conversation_ref: conversation,
          connection_ref: child,
          item_ref: `${workspace}/item-two`,
          publication_ref: `${workspace}/publication-two`,
          position: 2,
          content: { kind: "invocation", invocation_ref: invocation },
          child_connection_ref: null,
        },
      ],
    },
  };
  request.changes.memberships.push(
    {
      operation: "upsert",
      source_ref: `${scope.runtime_root_ref}/cwpdb`,
      kind: "conversation",
      member: { target_ref: conversation, position: 0, properties: {} },
    },
    {
      operation: "upsert",
      source_ref: conversation,
      kind: "conversation_timeline_root",
      member: { target_ref: connection, position: 0, properties: {} },
    },
  );
  const result = await commit(request);
  assertEquals(await commit(request), result);
  const page = await query();
  for (
    const [source, kind, target] of [
      [connection, "conversation_timeline_item", `${workspace}/item-one`],
      [`${workspace}/item-one`, "conversation_timeline_message", message],
      [`${workspace}/item-one`, "conversation_timeline_child", child],
      [`${workspace}/item-two`, "conversation_timeline_invocation", invocation],
    ]
  ) {
    assert(
      page.relationships.some((edge) =>
        edge.source_ref === source && edge.kind === kind &&
        edge.target_ref === target
      ),
    );
  }
  const broken = structuredClone(request);
  broken.request_id = "rollback-typed";
  broken.expected_revision = page.revision;
  broken.changes.timeline_items = [{
    ...request.changes.timeline_items![0],
    item_ref: `${workspace}/bad-item`,
    publication_ref: `${workspace}/bad-publication`,
    content: { kind: "message", message_ref: `${workspace}/missing` },
  }];
  await assertRejects(() => commit(broken), Error, "invalid endpoint");
  assertEquals((await query()).revision, page.revision);
  assertEquals(
    await call({
      operation: "get_conversation_record",
      scope,
      reference: `${workspace}/bad-item`,
    }),
    null,
  );
  await assertRejects(
    () => commit({ ...request, request_id: "stale-typed" }),
    Error,
    "revision_conflict",
  );
}
