import { assert, assertEquals, assertRejects } from "@std/assert";
import {
  MAX_WIRE_BYTES,
  type Mutation,
  type Scope,
  type SnapshotPage,
} from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
export async function boundedCases(
  call: Transport,
  scope: Scope,
  conversation: string,
  base: Mutation,
): Promise<void> {
  const query = (
    selection: unknown,
    extra: Record<string, unknown> = {},
  ): Promise<SnapshotPage> =>
    call({
      operation: "read_conversation_graph",
      request: {
        scope,
        selection,
        continuation: null,
        page_size: 100,
        max_bytes: MAX_WIRE_BYTES,
        expected_revision: null,
        ...extra,
      },
    }) as Promise<SnapshotPage>;
  const mutate = (request: Mutation) =>
    call({ operation: "commit_conversation_graph", request });
  const get = (reference: string) =>
    call({ operation: "get_conversation_record", scope, reference });
  await assertRejects(
    () => query({ selection: "bootstrap" }, { max_bytes: 32 * 1024 * 1024 }),
    Error,
    "page limits",
  );
  const before = await query({ selection: "bootstrap" });
  const tooLarge: Mutation = {
    ...base,
    request_id: "oversize-atomic",
    expected_revision: before.revision,
    changes: {
      records: [1, 2].map((n) => ({
        reference: `${scope.runtime_root_ref}/oversize/${n}`,
        kind: "message",
        owner_ref: null,
        properties: { record: { text: "z".repeat(13 * 1024 * 1024) } },
      })),
      remove_records: [],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
    },
  };
  await assertRejects(() => mutate(tooLarge), Error, "budget");
  assertEquals(await get(`${scope.runtime_root_ref}/oversize/1`), null);
  assertEquals(
    (await query({ selection: "bootstrap" })).revision,
    before.revision,
  );
  // Two individually allowed files must page without truncating either body.
  const extraFile = `${scope.runtime_root_ref}/files/second`;
  const content = "62".repeat(3 * 1024 * 1024);
  await mutate({
    ...base,
    request_id: "second-retained-file",
    expected_revision: before.revision,
    changes: {
      records: [{
        reference: extraFile,
        kind: "file",
        owner_ref: null,
        properties: {
          record: {
            metadata: {
              file_ref: extraFile,
              owner_actor_ref: scope.context.caller_ref,
              filename: "second.bin",
              media_type: "application/octet-stream",
              size_bytes: 3 * 1024 * 1024,
              sha256: "fixture-only",
            },
            content_hex: content,
          },
        },
      }],
      remove_records: [],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
    },
  });
  await assertRejects(
    () =>
      query({ selection: "records", kind: "file" }, { max_bytes: 1024 * 1024 }),
    Error,
    "requires at least",
  );
  const first = await query({ selection: "records", kind: "file" });
  assert(!first.complete && first.continuation);
  assertEquals(first.records.length, 1);
  assert(
    new TextEncoder().encode(JSON.stringify(first)).length <= MAX_WIRE_BYTES,
  );
  const second = await query({ selection: "records", kind: "file" }, {
    continuation: first.continuation,
    expected_revision: first.revision,
  });
  assert(second.complete);
  assertEquals(second.records.length, 1);
  assertEquals(
    (second.records[0].properties.record as { content_hex: string })
      .content_hex,
    content,
  );
  assert(
    new TextEncoder().encode(JSON.stringify(second)).length <= MAX_WIRE_BYTES,
  );
  const source = `${scope.runtime_root_ref}/activities/progress-source`;
  const message = `${scope.runtime_root_ref}/messages/progress`;
  const later = `${scope.runtime_root_ref}/messages/later`;
  const clock = await query({ selection: "bootstrap" });
  const configured: Mutation = {
    ...base,
    request_id: "configure-and-progress",
    expected_revision: clock.revision,
    changes: {
      records: [{
        reference: message,
        kind: "message",
        owner_ref: null,
        properties: {
          record: {
            text: "Working",
            progress: { source_activity_ref: source },
            optional: null,
          },
        },
      }, {
        reference: later,
        kind: "message",
        owner_ref: null,
        properties: { record: { text: "Later" } },
      }],
      remove_records: [],
      memberships: [{
        operation: "upsert",
        source_ref: conversation,
        kind: "message",
        member: { target_ref: message, position: 3, properties: {} },
      }],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
      timeline_rollout: "new_conversations",
      context_policy: { configured: true, policy: null },
    },
  };
  await mutate(configured);
  const configuredPage = await query({ selection: "bootstrap" });
  assertEquals(configuredPage.metadata.context_policy, {
    configured: true,
    policy: null,
  });
  assertEquals(
    configuredPage.metadata.conversation_timeline_rollout,
    "new_conversations",
  );
  const ordinal = (reference: string) =>
    call({
      operation: "conversation_retained_order",
      scope,
      reference,
    }) as Promise<number | null>;
  const earlierOrder = await ordinal(message);
  const laterOrder = await ordinal(later);
  assert(
    earlierOrder !== null && laterOrder !== null && earlierOrder < laterOrder,
  );
  assertEquals(
    await call({
      operation: "published_progress_conversation",
      scope,
      reference: source,
    }),
    conversation,
  );
  const downgrade: Mutation = {
    ...configured,
    request_id: "rollout-does-not-regress",
    expected_revision: configuredPage.revision,
    changes: {
      records: [],
      remove_records: [],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
      timeline_rollout: "disabled",
    },
  };
  await mutate(downgrade);
  const final = await query({ selection: "bootstrap" });
  assertEquals(
    final.metadata.conversation_timeline_rollout,
    "new_conversations",
  );
  assertEquals(final.revision, configuredPage.revision);
}
