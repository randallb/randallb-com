import { assert, assertEquals } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import { derived } from "./conversation_graph.ts";
import type {
  Mutation,
  Scope,
  SnapshotPage,
  WriteRecord,
} from "./conversation_types.ts";

type Transport = (body: unknown) => Promise<unknown>;
interface ResourceFacts {
  runtime_root_ref: string;
  conversation_ref: string;
  commit_revision: string;
  conversation: {
    root: boolean;
    parent_ref: string | null;
    participant_actor_refs: string[];
  };
  resource: {
    kind: "invocation" | "message";
    record: Record<string, unknown>;
    child_invocation_refs?: string[];
    wait_records?: Record<string, unknown>[];
    latest_reply?: Record<string, unknown> | null;
    execution?:
      | { generation: number; unpublished_progress: string | null }
      | null;
  };
}
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8817";
const targets = new Set((Deno.args[2] ?? "native,workerd,remote").split(","));
const directory = await Deno.makeTempDir({ prefix: "cwpdb-resource-" });
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
  const workspace = `cwp://fixture/workspaces/resource-${crypto.randomUUID()}`;
  const scope: Scope = {
    context: { workspace_ref: workspace, caller_ref: `${workspace}/host` },
    runtime_root_ref: `${workspace}/runtime`,
  };
  const conversation = `${workspace}/conversation`;
  const execution = `${workspace}/execution`;
  const actor = `${workspace}/actor`;
  const invocation = `${workspace}/invocation`;
  const message = `${workspace}/message`;
  const reply = `${workspace}/reply`;
  const hidden = `${workspace}/hidden`;
  const progress = `${workspace}/progress`;
  const start = `${workspace}/start`;
  const connection = `${workspace}/connection`;
  const sourceItem = `${workspace}/source-item`;
  const turnItem = `${workspace}/turn-item`;
  const replyItem = `${workspace}/reply-item`;
  const wait = await derived("wait", [invocation, "call"]);
  const read = () =>
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
  const resource = (reference: string) =>
    call({ operation: "conversation_resource", scope, reference }) as Promise<
      ResourceFacts | null
    >;
  const record = (
    reference: string,
    kind: WriteRecord["kind"],
    properties: Record<string, unknown>,
  ): WriteRecord => ({ reference, kind, properties, owner_ref: null });
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
  const timelineRecord = (
    reference: string,
    kind: WriteRecord["kind"],
    properties: Record<string, unknown>,
  ): WriteRecord => ({ reference, kind, properties, owner_ref: conversation });
  let revision = (await read()).revision;
  let requestNumber = 0;
  const commit = async (changes: Partial<Mutation["changes"]>) => {
    const outcome = await call({
      operation: "commit_conversation_graph",
      request: {
        scope,
        request_id: `resource-${++requestNumber}`,
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
  const base = {
    invocation_ref: invocation,
    conversation_ref: conversation,
    requester_actor_ref: actor,
    target_actor_ref: actor,
    source_message_ref: message,
    parent_invocation_ref: null,
    forked_from_invocation_ref: null,
    state: "running",
    admitted_at_unix_ms: 1,
    started_at_unix_ms: 2,
    finished_at_unix_ms: null,
    stop_requested_at_unix_ms: null,
    attempt_count: 1,
    attempt_active: true,
    attempt_started_at_unix_ms: 3,
    attempt_started_after_activity_sequence: 0,
    input: { private: "resource-private-ledger" },
    responses: [{
      status: 102,
      summary: "Working",
      result: { private: "resource-private-ledger" },
    }],
  };
  const messageRecord = (reference: string, text: string) => ({
    message_ref: reference,
    author_actor_ref: actor,
    addressed_actor_refs: [],
    text,
  });
  await commit({
    records: [
      timelineRecord(connection, "conversation_timeline_connection", {
        revision: 1,
        next_sequence: 3,
        store_sequence: 0,
      }),
      timelineRecord(sourceItem, "conversation_timeline_item", {
        publication_ref: `${workspace}/source-publication`,
      }),
      timelineRecord(turnItem, "conversation_timeline_item", {
        publication_ref: `${workspace}/turn-publication`,
      }),
      record(conversation, "conversation", {
        owner_context_ref: workspace,
        sidebar_eligible: true,
      }),
      record(execution, "conversation", { sidebar_eligible: false }),
      record(message, "message", { record: messageRecord(message, "Source") }),
      record(hidden, "message", {
        record: messageRecord(hidden, "Undisclosed"),
      }),
      record(invocation, "invocation", { record: base }),
      record(wait, "wait", {
        record: {
          waiter_invocation_ref: invocation,
          awaited_invocation_ref: `${workspace}/awaited`,
          call_id: "call",
          state: "consumed",
          response: { private: "resource-private-ledger" },
        },
      }),
      record(start, "activity", {
        record: {
          activity_ref: start,
          sequence: 1,
          occurred_at_unix_ms: 3,
          event: {
            kind: "turn_started",
            source_message_ref: message,
            invocation_ref: invocation,
          },
        },
      }),
      record(progress, "activity", {
        record: {
          activity_ref: progress,
          sequence: 2,
          occurred_at_unix_ms: 4,
          event: { kind: "progress", summary: "Current attempt" },
        },
      }),
    ],
    memberships: [
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", conversation),
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", execution, 1),
      member(execution, "parent", conversation),
      member(conversation, "participant", actor),
      member(conversation, "conversation_timeline_root", connection),
      member(connection, "conversation_timeline_item", sourceItem, 1),
      member(sourceItem, "conversation_timeline_message", message),
      member(connection, "conversation_timeline_item", turnItem, 2),
      member(turnItem, "conversation_timeline_invocation", invocation),
      member(conversation, "message", message),
      member(conversation, "message", hidden, 1),
      member(invocation, "execution", execution),
      member(execution, "activity", start),
      member(execution, "activity", progress, 1),
    ],
    publications: [{
      conversation_ref: conversation,
      from_position: 0,
      items: [{ kind: "message", message_ref: message, position: 0 }, {
        kind: "turn",
        invocation_ref: invocation,
        position: 1,
      }],
      activities: [],
      remove_activities: [],
    }],
  });
  const active = await resource(invocation);
  assert(active);
  assertEquals(active.conversation.root, true);
  assertEquals(active.conversation.participant_actor_refs, [actor]);
  assertEquals(active.resource.kind, "invocation");
  assertEquals(active.resource.execution?.generation, 1);
  assertEquals(
    active.resource.execution?.unpublished_progress,
    "Current attempt",
  );
  assertEquals(active.resource.record.outcome, null);
  assertEquals(active.resource.wait_records?.[0].response, undefined);
  assert(!JSON.stringify(active).includes("resource-private-ledger"));
  assertEquals((await resource(message))?.resource.record.text, "Source");
  assertEquals(await resource(hidden), null);
  assertEquals(await resource(`${workspace}/missing`), null);
  await commit({
    records: [
      timelineRecord(replyItem, "conversation_timeline_item", {
        publication_ref: `${workspace}/reply-publication`,
      }),
      record(reply, "message", {
        record: {
          ...messageRecord(reply, "Published reply"),
          progress: {
            invocation_ref: invocation,
            source_activity_ref: progress,
          },
        },
      }),
      record(invocation, "invocation", {
        record: {
          ...base,
          state: "completed",
          attempt_active: false,
          finished_at_unix_ms: 5,
          responses: [{
            status: 200,
            summary: "Done",
            result: { private: "resource-private-ledger" },
            error: {
              code: "public-code",
              message: "Public message",
              data: { private: "resource-private-ledger" },
            },
          }],
        },
      }),
    ],
    memberships: [
      member(conversation, "message", reply, 2),
      member(connection, "conversation_timeline_item", replyItem, 3),
      member(replyItem, "conversation_timeline_message", reply),
    ],
    publications: [{
      conversation_ref: conversation,
      from_position: 2,
      items: [{ kind: "message", message_ref: reply, position: 2 }],
      activities: [],
      remove_activities: [],
    }],
  });
  const terminal = await resource(invocation);
  assert(terminal);
  assertEquals(terminal.resource.execution, null);
  assertEquals(terminal.resource.latest_reply?.message_ref, reply);
  assertEquals(terminal.resource.record.outcome, {
    status: 200,
    summary: "Done",
    error: { code: "public-code", message: "Public message" },
    cause: null,
  });
  assert(!JSON.stringify(terminal).includes("resource-private-ledger"));
  assertEquals(terminal.commit_revision, "2");
  const child = `${workspace}/legacy-child`;
  const childActor = `${workspace}/child-actor`;
  const legacyReply = `${workspace}/legacy-reply`;
  const childItem = `${workspace}/legacy-child-item`;
  const legacyReplyItem = `${workspace}/legacy-reply-item`;
  const childRecord = {
    ...base,
    invocation_ref: child,
    target_actor_ref: childActor,
    parent_invocation_ref: invocation,
    state: "completed",
    attempt_active: false,
    finished_at_unix_ms: 6,
    action_name: "respond_to--message",
    responses: [{
      status: 200,
      summary: "Legacy child done",
      result: { private: "resource-private-ledger" },
    }],
  };
  await commit({
    records: [
      record(child, "invocation", { record: childRecord }),
      record(legacyReply, "message", {
        record: {
          ...messageRecord(legacyReply, "Legacy reply"),
          author_actor_ref: childActor,
          reply_to_message_ref: message,
        },
      }),
      timelineRecord(childItem, "conversation_timeline_item", {
        publication_ref: `${workspace}/legacy-child-publication`,
      }),
      timelineRecord(legacyReplyItem, "conversation_timeline_item", {
        publication_ref: `${workspace}/legacy-reply-publication`,
      }),
    ],
    memberships: [
      member(conversation, "message", legacyReply, 3),
      member(connection, "conversation_timeline_item", childItem, 4),
      member(childItem, "conversation_timeline_invocation", child),
      member(connection, "conversation_timeline_item", legacyReplyItem, 5),
      member(legacyReplyItem, "conversation_timeline_message", legacyReply),
    ],
    publications: [{
      conversation_ref: conversation,
      from_position: 3,
      items: [{ kind: "turn", invocation_ref: child, position: 3 }, {
        kind: "message",
        message_ref: legacyReply,
        position: 4,
      }],
      activities: [],
      remove_activities: [],
    }],
  });
  assertEquals(
    (await resource(child))?.resource.latest_reply?.message_ref,
    legacyReply,
  );
  const competitor = `${workspace}/legacy-competitor`;
  await commit({
    records: [
      record(competitor, "invocation", {
        record: { ...childRecord, invocation_ref: competitor },
      }),
    ],
  });
  assertEquals(
    (await resource(child))?.resource.latest_reply,
    null,
    "ambiguous legacy identity must not invent a reply",
  );
  console.log(JSON.stringify({
    backend: name,
    result: "PASS",
    checks: [
      "exact-resource-association",
      "same-snapshot-scope",
      "public-invocation-allowlist",
      "private-wait-response-omitted",
      "current-attempt-progress",
      "terminal-outcome-projection",
      "exact-linked-reply",
      "canonical-hidden-content",
      "missing-resource",
      "unique-legacy-reply-fallback",
      "ambiguous-legacy-reply-refused",
    ],
  }));
}
const native = await processTransport([`${directory}/store.db`], binary);
const remote = await processTransport(["--remote", endpoint], binary);
try {
  if (targets.has("native")) await exercise("native", native.call);
  if (targets.has("workerd")) await exercise("workerd", hosted);
  if (targets.has("remote")) await exercise("remote-store", remote.call);
} finally {
  await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
