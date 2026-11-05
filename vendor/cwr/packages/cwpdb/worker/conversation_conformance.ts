import { timelineWriteCases } from "./timeline_write_cases.ts";
import { assert, assertEquals, assertRejects } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import { boundedCases } from "./conversation_fixture_cases.ts";
import type {
  Mutation,
  ReadRequest,
  Scope,
  SnapshotPage,
} from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8797";
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const directory = await Deno.makeTempDir({ prefix: "cwpdb-conversations-" });
const hosted: Transport = async (body) => {
  if ((body as { operation?: string }).operation === "fixture_reopen") {
    return null;
  }
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: "Bearer fictional-local-cwpdb-conformance-only",
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  const value = await response.json();
  if (!value.ok) throw new Error(value.error);
  return value.result;
};
async function exercise(name: string, call: Transport): Promise<void> {
  const workspace =
    `cwp://fixture/workspaces/conversations-${crypto.randomUUID()}`;
  const scope: Scope = {
    context: {
      workspace_ref: workspace,
      caller_ref: "cwp://fixture/actors/coordinator",
    },
    runtime_root_ref: `${workspace}/runtime`,
  };
  const conversation = `${workspace}/conversation/one`;
  const message = `${workspace}/message/one`;
  const second = `${workspace}/message/two`;
  const invocation = `${workspace}/invocation/one`;
  const activity = `${workspace}/activity/one`;
  const actor = `${workspace}/actors/remote`;
  const declaration = `${workspace}/declaration/one`;
  const query = (
    selection: ReadRequest["selection"],
    extra: Partial<ReadRequest> = {},
  ): Promise<SnapshotPage> =>
    call({
      operation: "read_conversation_graph",
      request: {
        scope,
        selection,
        continuation: null,
        page_size: 100,
        max_bytes: 1048576,
        expected_revision: null,
        ...extra,
      },
    }) as Promise<SnapshotPage>;
  const get = (reference: string) =>
    call({ operation: "get_conversation_record", scope, reference }) as Promise<
      { properties: Record<string, unknown> } | null
    >;
  const initial = await query({ selection: "bootstrap" });
  const changes: Mutation["changes"] = {
    records: [{
      reference: conversation,
      kind: "conversation",
      properties: {
        title: "Family",
        owner_context_ref: null,
        sidebar_eligible: true,
      },
      owner_ref: null,
    }, {
      reference: message,
      kind: "message",
      properties: { record: { text: "School closes early" } },
      owner_ref: null,
    }, {
      reference: second,
      kind: "message",
      properties: { record: { text: "Private later message" } },
      owner_ref: null,
    }, {
      reference: invocation,
      kind: "invocation",
      properties: {
        record: {
          input: null,
          enabled: false,
          items: [],
          nested: { kept: null },
          conversation_ref: conversation,
          state: "running",
          parent_invocation_ref: null,
          forked_from_invocation_ref: null,
        },
      },
      owner_ref: null,
    }, {
      reference: activity,
      kind: "activity",
      properties: {
        record: {
          event: {
            kind: "commentary_progress",
            provider_integration_ref: `${workspace}/provider`,
            response_or_turn_id: "turn-one",
            item_id: "item-one",
          },
        },
      },
      owner_ref: null,
    }, {
      reference: declaration,
      kind: "upstream_declaration",
      properties: { record: { actor_ref: actor } },
      owner_ref: null,
    }],
    remove_records: [],
    memberships: [{
      operation: "upsert",
      source_ref: `${scope.runtime_root_ref}/cwpdb`,
      kind: "conversation",
      member: { target_ref: conversation, position: 0, properties: {} },
    }, {
      operation: "upsert",
      source_ref: conversation,
      kind: "message",
      member: { target_ref: message, position: 0, properties: {} },
    }, {
      operation: "upsert",
      source_ref: conversation,
      kind: "message",
      member: { target_ref: second, position: 1, properties: {} },
    }, {
      operation: "upsert",
      source_ref: conversation,
      kind: "activity",
      member: { target_ref: activity, position: 2, properties: {} },
    }, {
      operation: "upsert",
      source_ref: actor,
      kind: "upstream_declaration",
      member: { target_ref: declaration, position: 0, properties: {} },
    }],
    publications: [{
      conversation_ref: conversation,
      from_position: 0,
      items: [{ kind: "message", message_ref: message, position: 0 }, {
        kind: "turn",
        invocation_ref: invocation,
        position: 1,
      }],
      activities: [{
        activity_ref: activity,
        position: 2,
        record: {
          event: {
            kind: "work_update",
            invocation_ref: invocation,
            source_message_ref: message,
            parent_invocation_ref: null,
          },
        },
      }],
      remove_activities: [],
    }],
    state_version: null,
    streaming_runs: null,
    repair_legacy: false,
  };
  const request: Mutation = {
    scope,
    request_id: "source-admission",
    expected_revision: initial.revision,
    changes,
    journal: null,
    authority_guards: [],
  };
  const mutate = (request: Mutation) =>
    call({ operation: "commit_conversation_graph", request });
  const result = await mutate(request);
  assertEquals(await mutate(request), result);
  await assertRejects(() =>
    call({
      operation: "mutate_document",
      request: {
        context: scope.context,
        reference: conversation,
        request_id: "cross-domain-collision",
        expected_revision: null,
        operation: {
          operation: "create",
          schema_ref: `${workspace}/schemas/collision`,
          value: { bad: true },
        },
      },
    })
  );
  await call({ operation: "fixture_reopen", scope });
  const bootstrap = await query({ selection: "bootstrap" });
  const occupied = (reference: string) =>
    call({ operation: "conversation_identity_in_use", scope, reference });
  const endpointKinds = (reference: string) =>
    call({ operation: "conversation_endpoint_kinds", scope, reference });
  assertEquals(await endpointKinds(conversation), {
    node_kind: "conversation",
    edge_kind: null,
  });
  assertEquals(await endpointKinds(actor), {
    node_kind: null,
    edge_kind: null,
  });
  assertEquals(await endpointKinds(`${workspace}/missing`), {
    node_kind: null,
    edge_kind: null,
  });
  assertEquals(
    await endpointKinds(
      bootstrap.relationships.find((r) => r.kind === "conversation")!.reference,
    ),
    { node_kind: null, edge_kind: "conversation" },
  );
  assertEquals(await occupied(conversation), true);
  assertEquals(
    await occupied(
      bootstrap.relationships.find((r) => r.kind === "conversation")!.reference,
    ),
    true,
  );
  assertEquals(
    await occupied(actor),
    false,
    "binding-only actor is not a retained record",
  );
  assertEquals(await occupied(`${workspace}/missing`), false);
  assert(!bootstrap.records.some((r) => r.kind === "message"));
  assert(
    bootstrap.relationships.some((r) =>
      r.kind === "upstream_declaration" && r.source_ref === actor
    ),
  );
  assertEquals((await get(invocation))?.properties.record, {
    input: null,
    enabled: false,
    items: [],
    nested: { kept: null },
    conversation_ref: conversation,
    state: "running",
    parent_invocation_ref: null,
    forked_from_invocation_ref: null,
  });
  const metadata = await query({
    selection: "conversation_metadata",
    conversation_ref: conversation,
  });
  assert(metadata.records.some((r) => r.reference === message));
  assert(!metadata.records.some((r) => r.reference === second));
  const commentary = await query({
    selection: "commentary",
    provider_integration_ref: `${workspace}/provider`,
    response_or_turn_id: "turn-one",
    item_id: "item-one",
  });
  assertEquals(commentary.records.map((r) => r.reference), [activity]);
  assertEquals(commentary.relationships.length, 1);
  let continuation: string | null = null;
  const records = [];
  do {
    const page = await query({
      selection: "conversation",
      conversation_ref: conversation,
    }, { page_size: 1, continuation, expected_revision: bootstrap.revision });
    records.push(...page.records);
    continuation = page.continuation;
  } while (continuation);
  assert(records.some((r) => r.reference === second));
  const stale = { ...request, request_id: "stale" };
  await assertRejects(() => mutate(stale), Error, "revision_conflict");
  const broken = structuredClone(request);
  broken.request_id = "rollback";
  broken.expected_revision = bootstrap.revision;
  broken.changes.memberships.push({
    operation: "upsert",
    source_ref: conversation,
    kind: "message",
    member: { target_ref: `${workspace}/missing`, position: 3, properties: {} },
  });
  broken.journal = {
    request: {
      context: scope.context,
      invocation_ref: `${workspace}/cloud-invocation`,
      request_id: "atomic-event",
      expected_revision: null,
      operation: {
        operation: "admit",
        conversation_ref: conversation,
        parent_invocation_ref: null,
        admitted: { action: "respond" },
      },
    },
    now_unix_ms: 10,
  };
  await assertRejects(() => mutate(broken), Error, "invalid endpoint");
  assertEquals(
    (await query({ selection: "bootstrap" })).revision,
    bootstrap.revision,
  );
  broken.changes.memberships.pop();
  const grant = `${workspace}/source-grant`;
  const createGrant = await call({
    operation: "mutate_document",
    request: {
      context: scope.context,
      reference: grant,
      request_id: "grant",
      expected_revision: null,
      operation: {
        operation: "create",
        schema_ref: `${workspace}/schema/grant`,
        value: { active: true },
      },
    },
  }) as { record: { revision: string } };
  broken.authority_guards = [{
    reference: grant,
    expected_revision: createGrant.record.revision,
  }];
  await call({
    operation: "mutate_document",
    request: {
      context: scope.context,
      reference: grant,
      request_id: "revoke-grant",
      expected_revision: createGrant.record.revision,
      operation: { operation: "replace", value: { active: false } },
    },
  });
  await assertRejects(
    () => mutate(broken),
    Error,
    "source authority revision conflict",
  );
  assertEquals(
    (await query({ selection: "bootstrap" })).revision,
    bootstrap.revision,
  );
  broken.authority_guards = [];
  const joint = await mutate(broken) as { journal_receipt?: unknown };
  assert(joint.journal_receipt);
  assertEquals(await mutate(broken), joint);
  // Preserve the normal CWR retained-file limit (10MiB bytes, hex on wire).
  const file = `${workspace}/files/limit`;
  const fileBefore = await query({ selection: "bootstrap" });
  const contentHex = "61".repeat(10 * 1024 * 1024);
  const fileMutation: Mutation = {
    ...request,
    request_id: "retained-file-limit",
    expected_revision: fileBefore.revision,
    changes: {
      records: [{
        reference: file,
        kind: "file",
        owner_ref: null,
        properties: {
          record: {
            metadata: {
              file_ref: file,
              owner_actor_ref: scope.context.caller_ref,
              filename: "fixture.txt",
              media_type: "text/plain",
              size_bytes: 10 * 1024 * 1024,
              sha256: "fixture-only",
            },
            content_hex: contentHex,
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
  };
  await mutate(fileMutation);
  const filePage = await query({ selection: "records", kind: "file" }, {
    page_size: 1,
    max_bytes: 24 * 1024 * 1024,
  });
  assertEquals(
    (filePage.records[0].properties.record as { content_hex: string })
      .content_hex.length,
    contentHex.length,
  );
  const legacyBefore = await query({ selection: "bootstrap" });
  const legacy: Mutation = {
    ...request,
    request_id: "legacy-visualizations",
    expected_revision: legacyBefore.revision,
    changes: {
      records: [{
        reference: second,
        kind: "message",
        owner_ref: null,
        properties: {
          record: {
            text: "Legacy visual",
            nullable: null,
            visualizations: [{ artifact_ref: `${workspace}/artifact/one` }],
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
  };
  await mutate(legacy);
  const needsRepair = await query({ selection: "bootstrap" });
  assert(
    (needsRepair.metadata.storage_maintenance as string[]).includes(
      "visualization_metadata",
    ),
  );
  const repair: Mutation = {
    ...request,
    request_id: "validated-layout-repair",
    expected_revision: needsRepair.revision,
    changes: {
      records: [],
      remove_records: [],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: true,
    },
  };
  await mutate(repair);
  const repaired = await query({ selection: "bootstrap" });
  assertEquals(repaired.metadata.storage_maintenance, []);
  const repairedMessage = await get(second);
  assertEquals(
    (repairedMessage?.properties.record as { nullable: unknown }).nullable,
    null,
  );
  assert(!("visualizations" in (repairedMessage?.properties.record as object)));
  const repairedHistory = await query({
    selection: "conversation",
    conversation_ref: conversation,
  });
  assert(
    Array.isArray(
      repairedHistory.relationships.find((edge) =>
        edge.kind === "message" && edge.target_ref === second
      )?.properties.visualizations,
    ),
  );
  await boundedCases(call, scope, conversation, request);
  const current = await query({ selection: "bootstrap" });
  const remove: Mutation = {
    ...request,
    request_id: "erase-message",
    expected_revision: current.revision,
    changes: {
      records: [],
      remove_records: [{ reference: message, kind: "message" }],
      memberships: [],
      publications: [],
      state_version: null,
      streaming_runs: null,
      repair_legacy: false,
    },
  };
  const messageEdge =
    repairedHistory.relationships.find((edge) =>
      edge.kind === "message" && edge.target_ref === message
    )!.reference;
  await mutate(remove);
  assertEquals(await endpointKinds(message), {
    node_kind: null,
    edge_kind: null,
  });
  assertEquals(await endpointKinds(messageEdge), {
    node_kind: null,
    edge_kind: null,
  });
  assertEquals(await get(message), null);
  assertEquals(await mutate(request), { outcome: "committed_result_expired" });
  console.log(JSON.stringify({
    backend: name,
    result: "PASS",
    checks: [
      "typed-timeline-atomic-replay-rollback-CAS",
      "canonical-reopen",
      "cold-paging",
      "opaque-null",
      "first-message-metadata",
      "commentary-dedup",
      "binding-only-actor",
      "semantic-turn-vs-activity",
      "CAS",
      "atomic-journal-rollback",
      "joint-receipt-replay",
      "receipt-erasure",
      "current-source-revision-guard",
      "retained-file10MiB",
      "legacy-layout-repair",
      "cross-domain-identity-collision",
      "24MiB-atomic-multirecord-refusal",
      "whole-page-wire-bound",
      "configured-rollout-monotonic",
      "retained-context-policy",
      "retained-order-and-progress-lookup",
      "current-endpoint-types-including-erasure",
    ],
  }));
}
const typedNative = await processTransport([`${directory}/typed.db`], binary);
const typedRemote = await processTransport(["--remote", endpoint], binary);
try {
  await timelineWriteCases(typedNative.call);
  await timelineWriteCases(hosted);
  await timelineWriteCases(typedRemote.call);
} finally {
  await typedNative.close();
  await typedRemote.close();
}
const native = await processTransport([`${directory}/store.db`], binary);
const remote = await processTransport(["--remote", endpoint], binary);
try {
  await exercise("native-canonical-graph", native.call);
  await exercise("local-workerd-canonical-graph", hosted);
  await exercise("rust-remote-store-to-workerd-graph", remote.call);
} finally {
  await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
