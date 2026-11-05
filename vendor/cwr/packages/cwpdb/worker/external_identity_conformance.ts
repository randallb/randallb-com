import { assert, assertEquals } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import type { Mutation, Scope } from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture",
  endpoint = Deno.args[1] ?? "http://127.0.0.1:8797",
  targets = new Set((Deno.args[2] ?? "native,workerd,remote").split(","));
const directory = await Deno.makeTempDir({
  prefix: "cwpdb-external-identity-",
});
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
async function exercise(backend: string, call: Transport, role: string) {
  const base = `cwp://fixture/external-identity/${crypto.randomUUID()}`,
    actor = `${base}/actor`,
    conversation = `${base}/conversation`;
  const scope: Scope = {
    context: { workspace_ref: base, caller_ref: `${base}/caller` },
    runtime_root_ref: `${base}/runtime`,
  };
  const read = async () =>
    await call({
      operation: "read_conversation_graph",
      request: {
        scope,
        selection: { selection: "bootstrap" },
        page_size: 100,
        max_bytes: 1048576,
        continuation: null,
        expected_revision: null,
      },
    }) as { revision: string };
  let revision = (await read()).revision, count = 0;
  const mutate = async (changes: Partial<Mutation["changes"]>) => {
    const result = await call({
      operation: "commit_conversation_graph",
      request: {
        scope,
        request_id: `external-${count++}`,
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
      },
    }) as { revision: string };
    revision = result.revision;
  };
  await mutate({
    records: [{
      reference: conversation,
      kind: "conversation",
      properties: { title: "Fictional" },
      owner_ref: null,
    }],
    memberships: [{
      operation: "upsert",
      source_ref: `${scope.runtime_root_ref}/cwpdb`,
      kind: "conversation",
      member: { target_ref: conversation, position: 0, properties: {} },
    }, {
      operation: "upsert",
      source_ref: conversation,
      kind: role,
      member: { target_ref: actor, position: 0, properties: {} },
    }],
  });
  const before = revision;
  let denied = false;
  try {
    await mutate({
      records: [{
        reference: actor,
        kind: "message",
        properties: { record: { message_ref: actor, text: "Wrong kind" } },
        owner_ref: null,
      }],
    });
  } catch {
    denied = true;
  }
  assert(
    denied,
    "record-only write accepted wrong concrete kind at unchanged external participant binding",
  );
  assertEquals((await read()).revision, before);
  assertEquals(
    await call({
      operation: "conversation_endpoint_kinds",
      scope,
      reference: actor,
    }),
    { node_kind: null, edge_kind: null },
  );
  await mutate({
    memberships: [{
      operation: "remove",
      source_ref: conversation,
      kind: role,
      target_ref: actor,
    }],
  });
  await mutate({
    records: [{
      reference: actor,
      kind: "message",
      properties: { record: { message_ref: actor, text: "Role withdrawn" } },
      owner_ref: null,
    }],
  });
  assertEquals(
    await call({
      operation: "conversation_endpoint_kinds",
      scope,
      reference: actor,
    }),
    { node_kind: "message", edge_kind: null },
  );
  console.log(JSON.stringify({
    backend,
    role,
    result: "PASS",
    covers: [
      "binding-only-participant",
      "record-only-kind-collision-refused",
      "unchanged-revision-on-rejection",
      "no-partial-record",
      "withdrawn-role-does-not-poison-binding",
    ],
  }));
}
try {
  for (const role of ["participant", "participant_version"]) {
    const native = await processTransport([`${directory}/${role}.db`], binary),
      remote = await processTransport(["--remote", endpoint], binary);
    try {
      if (targets.has("native")) await exercise("native", native.call, role);
      if (targets.has("workerd")) await exercise("workerd", hosted, role);
      if (targets.has("remote")) await exercise("remote", remote.call, role);
    } finally {
      await native.close();
      await remote.close();
    }
  }
} finally {
  await Deno.remove(directory, { recursive: true });
}
