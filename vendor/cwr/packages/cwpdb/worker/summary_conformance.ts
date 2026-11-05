import { assertEquals, assertNotEquals } from "@std/assert";
import { digest } from "./engine.ts";
import { processTransport } from "./conformance_process.ts";
import type {
  Mutation,
  Relationship,
  Scope,
  SummaryFacts,
} from "./conversation_types.ts";
type Transport = (body: unknown) => Promise<unknown>;
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8797",
  binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const directory = await Deno.makeTempDir({ prefix: "cwpdb-summary-" });
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
  const workspace = `cwp://fixture/summary/${crypto.randomUUID()}`;
  const scope: Scope = {
    context: { workspace_ref: workspace, caller_ref: `${workspace}/host` },
    runtime_root_ref: `${workspace}/runtime`,
  };
  const r = (name: string) => `${workspace}/${name}`,
    conversation = r("conversation"),
    child = r("child"),
    legacy = r("legacy"),
    root = r("timeline"),
    nested = r("nested");
  const actor = r("actor"),
    other = r("other"),
    visible = r("message-visible"),
    latest = r("message-latest"),
    hidden = r("message-hidden"),
    streaming = r("message-streaming"),
    invocation = r("invocation-visible"),
    hiddenInvocation = r("invocation-hidden");
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
        request_id: `summary-${counter++}`,
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
  const summary = async (ref = conversation) =>
    (await call({
      operation: "conversation_summary",
      scope,
      reference: ref,
    })) as SummaryFacts | null;
  const member = (
    source_ref: string,
    kind: Relationship["kind"],
    target_ref: string,
    position = 0,
    properties: Record<string, unknown> = {},
  ) => ({
    operation: "upsert" as const,
    source_ref,
    kind,
    member: { target_ref, position, properties },
  });
  const message = (ref: string, text: string) => ({
    message_ref: ref,
    author_actor_ref: actor,
    addressed_actor_refs: [],
    text,
  });
  const body = message(latest, "Visible 😀 school update");
  const header = {
    title: null,
    owner_context_ref: "owner",
    sidebar_eligible: true,
    last_message_ref: hidden,
    last_message_preview: "WRONG hidden preview",
  };
  const records: Mutation["changes"]["records"] = [
    {
      reference: conversation,
      kind: "conversation",
      properties: header,
      owner_ref: null,
    },
    {
      reference: child,
      kind: "conversation",
      properties: { title: "Child" },
      owner_ref: null,
    },
    {
      reference: legacy,
      kind: "conversation",
      properties: {
        title: "Legacy",
        last_message_ref: visible,
        last_message_preview: "Retained nonvisual legacy preview",
      },
      owner_ref: null,
    },
    ...[root, nested].map((reference) => ({
      reference,
      kind: "conversation_timeline_connection" as const,
      properties: { revision: 1, store_sequence: reference === root ? 1 : 2 },
      owner_ref: null,
    })),
    ...[
      "item-first",
      "item-child",
      "item-last",
      "item-streaming",
      "item-invocation",
    ].map((name) => ({
      reference: r(name),
      kind: "conversation_timeline_item" as const,
      properties: {},
      owner_ref: null,
    })),
    {
      reference: visible,
      kind: "message",
      properties: { record: message(visible, "Older visible") },
      owner_ref: null,
    },
    {
      reference: latest,
      kind: "message",
      properties: { record: body },
      owner_ref: null,
    },
    {
      reference: hidden,
      kind: "message",
      properties: { record: message(hidden, "Unpublished hidden") },
      owner_ref: null,
    },
    {
      reference: streaming,
      kind: "message",
      properties: {
        record: message(streaming, "Hidden streaming"),
        streaming: { active: true },
      },
      owner_ref: null,
    },
    ...[invocation, hiddenInvocation].map((reference) => ({
      reference,
      kind: "invocation" as const,
      properties: {
        record: {
          conversation_ref: conversation,
          state: "completed",
          parent_invocation_ref: null,
        },
      },
      owner_ref: null,
    })),
    ...[invocation, hiddenInvocation].map((invocation_ref, index) => ({
      reference: r(`completion-${index}`),
      kind: "activity" as const,
      properties: {
        record: {
          event: {
            kind: "work_lifecycle",
            state: "completed",
            invocation_ref,
            parent_invocation_ref: null,
          },
        },
      },
      owner_ref: null,
    })),
  ];
  for (const record of records) {
    if (
      record.kind === "conversation_timeline_connection" ||
      record.kind === "conversation_timeline_item"
    ) record.owner_ref = conversation;
    if (record.kind === "conversation_timeline_item") {
      record.properties.publication_ref = `${record.reference}/publication`;
    }
  }
  await mutate({
    records,
    memberships: [
      member(
        `${scope.runtime_root_ref}/cwpdb`,
        "conversation",
        conversation,
        7,
      ),
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", child, 8),
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", legacy, 9),
      member(child, "parent", conversation),
      member(conversation, "participant", actor),
      member(conversation, "participant", other, 1),
      member(conversation, "conversation_timeline_root", root),
      member(root, "conversation_timeline_item", r("item-first")),
      member(root, "conversation_timeline_item", r("item-child"), 1),
      member(root, "conversation_timeline_item", r("item-invocation"), 2),
      member(r("item-first"), "conversation_timeline_message", visible),
      member(r("item-child"), "conversation_timeline_child", nested),
      member(
        r("item-invocation"),
        "conversation_timeline_invocation",
        invocation,
      ),
      member(nested, "conversation_timeline_item", r("item-last")),
      member(nested, "conversation_timeline_item", r("item-streaming"), 1),
      member(r("item-last"), "conversation_timeline_message", latest),
      member(r("item-streaming"), "conversation_timeline_message", streaming),
      ...[visible, latest, hidden, streaming].map((ref, index) =>
        member(conversation, "message", ref, index)
      ),
      member(conversation, "activity", r("completion-0"), 0),
      member(conversation, "activity", r("completion-1"), 1),
    ],
    publications: [{
      conversation_ref: conversation,
      from_position: 0,
      items: [{ kind: "turn", invocation_ref: invocation, position: 0 }, {
        kind: "turn",
        invocation_ref: hiddenInvocation,
        position: 1,
      }],
      activities: [],
      remove_activities: [],
    }],
  });
  await mutate({
    records: [{
      reference: streaming,
      kind: "message",
      properties: { streaming: { active: true } },
      owner_ref: null,
    }],
  });
  const first = (await summary())!;
  const semantic = [
    first.conversation_ref,
    first.creation_order,
    first.title,
    first.archived,
    first.owner_context_ref,
    first.parent_ref,
    first.sidebar_eligible,
    first.root,
    first.participant_actor_refs,
    first.last_message_ref,
    first.last_message_preview,
    first.completed_work,
    first.completed_work_refs,
    first.last_message_record
      ? [
        first.last_message_record.text ?? null,
        first.last_message_record.visualizations ?? null,
      ]
      : null,
  ];
  assertEquals(first.revision, `summary:v1:${await digest(semantic)}`);

  assertEquals(first.root, true);
  assertEquals(first.creation_order, 0);
  assertEquals(first.title, null);
  assertEquals(first.owner_context_ref, "owner");
  assertEquals(first.participant_actor_refs, [actor, other]);
  assertEquals(first.last_message_ref, latest);
  assertEquals(first.last_message_preview, body.text);
  assertEquals(first.last_message_record, body);
  assertEquals(first.completed_work_refs, [invocation]);
  assertEquals(first.completed_work.map((work) => work.invocation_ref), [
    invocation,
  ]);
  const execution = (await summary(child))!;
  assertEquals(execution.root, false);
  assertEquals(execution.parent_ref, conversation);
  assertEquals(
    (await summary(legacy))!.last_message_preview,
    "Retained nonvisual legacy preview",
  );
  assertEquals(await summary(r("missing")), null);
  await mutate({
    records: [{
      reference: latest,
      kind: "message",
      properties: { record: { ...body, progress: { fictional: "update" } } },
      owner_ref: null,
    }],
    streaming_runs: { fictional: "progress" },
  });
  const progress = (await summary())!;
  assertEquals(progress.revision, first.revision);
  await mutate({
    records: [{
      reference: conversation,
      kind: "conversation",
      properties: { ...header, title: "Changed" },
      owner_ref: null,
    }],
  });
  const renamed = (await summary())!;
  assertNotEquals(renamed.revision, progress.revision);
  await mutate({
    memberships: [{
      operation: "remove",
      source_ref: conversation,
      kind: "participant",
      target_ref: other,
    }],
  });
  const removed = (await summary())!;
  assertEquals(removed.participant_actor_refs, [actor]);
  assertNotEquals(removed.revision, renamed.revision);
  await mutate({
    records: [{
      reference: conversation,
      kind: "conversation",
      properties: {
        ...header,
        title: "Changed",
        owner_context_ref: "other-owner",
      },
      owner_ref: null,
    }],
    memberships: [
      member(conversation, "archived", `${scope.runtime_root_ref}/cwpdb`),
    ],
  });
  const changed = (await summary())!;
  assertEquals(changed.owner_context_ref, "other-owner");
  assertEquals(changed.archived, true);
  assertNotEquals(changed.revision, removed.revision);
  await mutate({
    memberships: [{
      operation: "remove",
      source_ref: nested,
      kind: "conversation_timeline_item",
      target_ref: r("item-last"),
    }],
  });
  const detached = (await summary())!;
  assertEquals(detached.last_message_ref, visible);
  assertEquals(detached.last_message_preview, "Older visible");
  assertNotEquals(detached.revision, changed.revision);
  console.log(JSON.stringify({
    backend: name,
    result: "pass",
    checks: 10,
    covers: [
      "exact-root-and-child",
      "current-participants-owner",
      "nested-visible-preview",
      "streaming-and-detached-hidden",
      "only-visible-completions",
      "legacy-nonvisual-preview",
      "progress-stable-revision",
      "title-participant-owner-archive-revision",
      "canonical-detachment",
      "missing-exact-ref",
    ],
  }));
  return first;
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
