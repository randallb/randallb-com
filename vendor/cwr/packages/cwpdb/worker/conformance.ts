import { assert, assertEquals, assertRejects } from "@std/assert";
import type { Context, Mutation, Outcome, StoredRecord } from "./client.ts";
import type { Mutation as InvocationMutation, Receipt } from "./invocations.ts";
type Transport = (body: unknown) => Promise<unknown>;
let context: Context = {
  workspace_ref: `cwp://fixture/workspaces/${crypto.randomUUID()}`,
  caller_ref: "cwp://fixture/actors/coordinator",
};
let root = `${context.workspace_ref}/documents/family`;
function document(
  request_id: string,
  expected_revision: string | null,
  operation: Mutation["operation"],
): unknown {
  return {
    operation: "mutate_document",
    request: {
      context,
      reference: root,
      request_id,
      expected_revision,
      operation,
    },
  };
}
function record(value: unknown): StoredRecord {
  const outcome = value as Outcome;
  assertEquals(outcome.outcome, "committed");
  assert(outcome.outcome === "committed" && outcome.record);
  return outcome.record;
}
async function exercise(name: string, call: Transport): Promise<void> {
  context = {
    workspace_ref: `cwp://fixture/workspaces/${crypto.randomUUID()}`,
    caller_ref: "cwp://fixture/actors/coordinator",
  };
  root = `${context.workspace_ref}/documents/family`;
  const create = document("create", null, {
    operation: "create",
    schema_ref: "cwp://fixture/schemas/family",
    value: { record: { name: "family", old: true } },
  });
  const first = record(await call(create));
  assertEquals(await call(create), { outcome: "committed", record: first });
  const replace = document("replace", first.revision, {
    operation: "replace",
    value: { record: { name: "corrected" } },
  });
  const changed = record(await call(replace));
  assert(changed.revision !== first.revision);
  assertEquals(changed.value, { record: { name: "corrected" } });
  assertEquals(record(await call(replace)), changed);
  await assertRejects(
    () =>
      call(
        document("replace", first.revision, {
          operation: "replace",
          value: { secret: "changed" },
        }),
      ),
    Error,
    "request conflict",
  );
  await assertRejects(
    () =>
      call(
        document("stale", first.revision, {
          operation: "replace",
          value: { bad: true },
        }),
      ),
    Error,
    "revision_conflict",
  );
  const race = await Promise.allSettled([
    call(
      document("race-a", changed.revision, {
        operation: "replace",
        value: { winner: "a" },
      }),
    ),
    call(
      document("race-b", changed.revision, {
        operation: "replace",
        value: { winner: "b" },
      }),
    ),
  ]);
  assertEquals(race.filter((r) => r.status === "fulfilled").length, 1);
  const current = await call({
    operation: "get_document",
    context,
    reference: root,
  }) as StoredRecord;
  const noOp = record(
    await call(
      document("no-op", current.revision, {
        operation: "replace",
        value: current.value,
      }),
    ),
  );
  assertEquals(noOp.revision, current.revision);
  const foreign = {
    ...context,
    workspace_ref: `${context.workspace_ref}-foreign`,
  };
  assertEquals(
    await call({
      operation: "get_document",
      context: foreign,
      reference: root,
    }),
    null,
  );
  const deletion = document("delete", noOp.revision, { operation: "delete" });
  await call(deletion);
  assertEquals(await call(create), { outcome: "committed_result_expired" });
  await assertRejects(
    () =>
      call(
        document("new-create", null, {
          operation: "create",
          schema_ref: "cwp://fixture/schemas/family",
          value: {},
        }),
      ),
    Error,
    "retired",
  );
  assertEquals(await call(deletion), {
    outcome: "committed",
    record: null,
    deleted_revision: noOp.revision,
  });
  const invocation_ref = `${context.workspace_ref}/invocations/one`;
  const mutate = (
    request_id: string,
    revision: string | null,
    operation: InvocationMutation["operation"],
  ) => ({
    operation: "mutate_invocation",
    now_unix_ms: 10,
    invocation_request: {
      context,
      invocation_ref,
      request_id,
      expected_revision: revision,
      operation,
    },
  });
  const receipt = (outcome: unknown): Receipt => {
    const value = outcome as { outcome: string; receipt: Receipt };
    assertEquals(value.outcome, "committed");
    return value.receipt;
  };
  const admitted = receipt(
    await call(
      mutate("admit", null, {
        operation: "admit",
        conversation_ref: `${context.workspace_ref}/conversations/one`,
        parent_invocation_ref: null,
        admitted: { action: "respond" },
      }),
    ),
  );
  const assignment = {
    executor_ref: "cwp://fixture/executors/one",
    assignment_ref: "cwp://fixture/assignments/one",
    generation: 1,
    route_fingerprint: "route-one",
    expires_at_unix_ms: 1000,
  };
  const assigned = receipt(
    await call(
      mutate("assign", admitted.revision, { operation: "assign", assignment }),
    ),
  );
  const append = (
    revision: string,
    sequence: number,
    kind: "progress" | "completed",
    extra: Partial<typeof assignment> = {},
  ) =>
    mutate(`event-${sequence}`, revision, {
      operation: "append",
      assignment_ref: assignment.assignment_ref,
      executor_ref: assignment.executor_ref,
      generation: 1,
      ...extra,
      event: { sequence, kind, payload: { text: "useful result" } },
    });
  await assertRejects(
    () =>
      call(
        append(assigned.revision, 0, "progress", {
          executor_ref: "cwp://fixture/executors/foreign",
        }),
      ),
    Error,
    "foreign",
  );
  const event = append(assigned.revision, 0, "progress");
  const progress = receipt(await call(event));
  const retained = await call({
    operation: "invocation_receipt",
    context,
    invocation_ref,
    request_id: "event-0",
  }) as { outcome: string; request: InvocationMutation; receipt: Receipt };
  assertEquals(retained.outcome, "retained");
  assertEquals(
    retained.request,
    (event as { invocation_request: InvocationMutation }).invocation_request,
  );
  assertEquals(retained.receipt, progress);

  assertEquals(receipt(await call(event)), progress);
  const complete = receipt(
    await call(append(progress.revision, 1, "completed")),
  );
  assertEquals(complete.terminal, "completed");
  await assertRejects(
    () => call(append(complete.revision, 2, "progress")),
    Error,
    "terminal",
  );
  const events = (after: number | null) =>
    call({
      operation: "invocation_events",
      context,
      invocation_ref,
      after,
      limit: 1,
    }) as Promise<
      {
        items: { sequence: number }[];
        continuation: number | null;
        complete: boolean;
      }
    >;
  const page = await events(null);
  assertEquals(page.items[0].sequence, 0);
  assertEquals(page.continuation, 0);
  assertEquals(page.complete, false);
  const last = await events(page.continuation);
  assertEquals(last.items[0].sequence, 1);
  assertEquals(last.complete, true);
  console.log(JSON.stringify({
    backend: name,
    result: "PASS",
    checks: [
      "CAS",
      "concurrent-revision-conflict",
      "duplicate-receipt",
      "request-conflict",
      "no-op-revision",
      "workspace-isolation",
      "delete-erasure",
      "no-resurrection",
      "assignment-fencing",
      "ordered-events",
      "terminal-barrier",
    ],
  }));
}
const binary = Deno.args[0] ?? "target/debug/examples/storage_fixture";
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8797";
const directory = await Deno.makeTempDir({ prefix: "cwpdb-conformance-" });
const native: Transport = async (body) => {
  const process = new Deno.Command(binary, {
    args: [`${directory}/store.db`],
    stdin: "piped",
    stdout: "piped",
    stderr: "piped",
  }).spawn();
  const writer = process.stdin.getWriter();
  await writer.write(new TextEncoder().encode(JSON.stringify(body) + "\n"));
  await writer.close();
  const output = await process.output();
  if (!output.success) throw new Error(new TextDecoder().decode(output.stderr));
  const response = JSON.parse(new TextDecoder().decode(output.stdout));
  if (!response.ok) throw new Error(response.error);
  return response.result;
};
const hosted: Transport = async (body) => {
  const response = await fetch(endpoint, {
    method: "POST",
    headers: {
      authorization: "Bearer fictional-local-cwpdb-conformance-only",
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) throw new Error(`HTTP ${response.status}`);
  const result = await response.json();
  if (!result.ok) throw new Error(result.error);
  return result.result;
};
try {
  await exercise("native-sqlite-reopen-each-call", native);
  await exercise("local-workerd-durable-object", hosted);
  const remote: Transport = async (body) => {
    const process = new Deno.Command(binary, {
      args: ["--remote", endpoint],
      stdin: "piped",
      stdout: "piped",
      stderr: "piped",
    }).spawn();
    const writer = process.stdin.getWriter();
    await writer.write(new TextEncoder().encode(JSON.stringify(body) + "\n"));
    await writer.close();
    const output = await process.output();
    if (!output.success) {
      throw new Error(new TextDecoder().decode(output.stderr));
    }
    const response = JSON.parse(new TextDecoder().decode(output.stdout));
    if (!response.ok) throw new Error(response.error);
    return response.result;
  };
  await exercise("rust-remote-store-to-local-workerd", remote);
} finally {
  await Deno.remove(directory, { recursive: true });
}
