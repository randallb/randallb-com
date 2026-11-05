import { assertEquals } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import type { HistoryStatus, Mutation, Scope } from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8797",
  binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const directory = await Deno.makeTempDir({ prefix: "cwpdb-history-" });
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
async function exercise(name: string, call: Transport) {
  const workspace = `cwp://fixture/history/${crypto.randomUUID()}`,
    conversation = `${workspace}/conversation`,
    message = `${workspace}/message`,
    activity = `${workspace}/activity`;
  const scope: Scope = {
    context: { workspace_ref: workspace, caller_ref: `${workspace}/host` },
    runtime_root_ref: `${workspace}/runtime`,
  };
  let revision = (await call({
    operation: "read_conversation_graph",
    request: {
      scope,
      selection: { selection: "bootstrap" },
      page_size: 100,
      max_bytes: 1048576,
      continuation: null,
      expected_revision: null,
    },
  }) as { revision: string }).revision;
  let counter = 0;
  const mutate = async (changes: Partial<Mutation["changes"]>) => {
    const result = await call({
      operation: "commit_conversation_graph",
      request: {
        scope,
        request_id: `history-${counter++}`,
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
  const status = (reference = conversation) =>
    call({
      operation: "conversation_history_status",
      scope,
      reference,
    }) as Promise<HistoryStatus | null>;
  const contains = (
    target = message,
    source = conversation,
    kind = "message",
  ) =>
    call({
      operation: "conversation_contains_member",
      scope,
      source_ref: source,
      membership_kind: kind,
      target_ref: target,
    }) as Promise<boolean>;
  const member = (kind: string, target_ref: string) => ({
    operation: "upsert" as const,
    source_ref: conversation,
    kind,
    member: { target_ref, position: 0, properties: {} },
  });
  const body = {
    message_ref: message,
    author_actor_ref: `${workspace}/actor`,
    addressed_actor_refs: [],
    text: "Fictional",
  };
  const check = async (flags: Omit<HistoryStatus, "revision">) =>
    assertEquals(await status(), { ...flags, revision });
  await mutate({
    records: [{
      reference: conversation,
      kind: "conversation",
      properties: { title: "History" },
      owner_ref: null,
    }, {
      reference: message,
      kind: "message",
      properties: { record: body },
      owner_ref: null,
    }, {
      reference: activity,
      kind: "activity",
      properties: { record: { event: { kind: "turn_started" } } },
      owner_ref: null,
    }],
    memberships: [
      {
        operation: "upsert",
        source_ref: `${scope.runtime_root_ref}/cwpdb`,
        kind: "conversation",
        member: { target_ref: conversation, position: 0, properties: {} },
      },
      member("message", message),
      member("activity", activity),
    ],
  });
  const clear = {
    has_timeline_root: false,
    has_streaming: false,
    has_legacy_context: false,
    terminal_activity: false,
  };
  await check(clear);
  assertEquals(await status(), await status());
  assertEquals(await contains(), true);
  assertEquals(await contains(`${workspace}/missing`), false);
  assertEquals(await contains(message, `${workspace}/foreign`), false);
  await mutate({
    records: [{
      reference: message,
      kind: "message",
      properties: { streaming: { active: true } },
      owner_ref: null,
    }],
  });
  await check({ ...clear, has_streaming: true });
  await mutate({
    records: [{
      reference: activity,
      kind: "activity",
      properties: {
        record: {
          event: {
            kind: "turn_started",
            invocation: { resumed_host_call: null },
          },
        },
      },
      owner_ref: null,
    }],
  });
  await check({ ...clear, has_streaming: true, has_legacy_context: true });
  await mutate({
    records: [{
      reference: activity,
      kind: "activity",
      properties: {
        record: {
          event: {
            kind: "turn_started",
            invocation: { thread: { nested: [{ messages: [] }] } },
          },
        },
      },
      owner_ref: null,
    }],
  });
  await check({ ...clear, has_streaming: true, has_legacy_context: true });
  for (const kind of ["turn_completed", "turn_failed", "turn_stopped"]) {
    await mutate({
      records: [{
        reference: activity,
        kind: "activity",
        properties: { record: { event: { kind } } },
        owner_ref: null,
      }],
    });
    await check({ ...clear, has_streaming: true, terminal_activity: true });
  }
  await mutate({
    memberships: [{
      operation: "remove",
      source_ref: conversation,
      kind: "activity",
      target_ref: activity,
    }],
  });
  await check({ ...clear, has_streaming: true });
  assertEquals(await contains(activity, conversation, "activity"), false);
  await mutate({
    records: [{
      reference: activity,
      kind: "activity",
      properties: {
        record: {
          event: {
            kind: "turn_started",
            invocation: { resumed_host_call: null },
          },
        },
      },
      owner_ref: null,
    }],
  });
  await check({ ...clear, has_streaming: true });
  await mutate({ memberships: [member("activity", activity)] });
  await check({ ...clear, has_streaming: true, has_legacy_context: true });
  await mutate({
    memberships: [{
      operation: "remove",
      source_ref: conversation,
      kind: "message",
      target_ref: message,
    }],
  });
  await check({ ...clear, has_legacy_context: true });
  assertEquals(await contains(), false);
  await mutate({ memberships: [member("message", message)] });
  await check({ ...clear, has_streaming: true, has_legacy_context: true });
  assertEquals(await contains(), true);
  const root = `${workspace}/timeline`;
  await mutate({
    records: [{
      reference: root,
      kind: "conversation_timeline_connection",
      properties: { revision: 0, store_sequence: 0 },
      owner_ref: conversation,
    }],
    memberships: [member("conversation_timeline_root", root)],
  });
  await check({
    has_timeline_root: true,
    has_streaming: true,
    has_legacy_context: true,
    terminal_activity: false,
  });
  await mutate({
    records: [{
      reference: message,
      kind: "message",
      properties: { record: body },
      owner_ref: null,
    }],
  });
  await check({
    has_timeline_root: true,
    has_streaming: false,
    has_legacy_context: true,
    terminal_activity: false,
  });
  assertEquals(await status(`${workspace}/missing`), null);
  assertEquals(await status(message), null);
  console.log(JSON.stringify({
    backend: name,
    result: "pass",
    checks: 9,
    covers: [
      "exact-membership-and-isolation",
      "same-snapshot-revision",
      "current-streaming-top-level",
      "null-resumed-call-presence",
      "nested-legacy-messages-key",
      "three-terminal-kinds",
      "removal-readdition-current-flags",
      "timeline-root-and-finalized-stream",
      "missing-nonconversation",
    ],
  }));
}
const native = await processTransport([`${directory}/store.db`], binary),
  remote = await processTransport(["--remote", endpoint], binary);
try {
  await exercise("native", native.call);
  await exercise("actual-local-workerd", hosted);
  await exercise("rust-remote-to-workerd", remote.call);
} finally {
  await native.close();
  await remote.close();
  await Deno.remove(directory, { recursive: true });
}
