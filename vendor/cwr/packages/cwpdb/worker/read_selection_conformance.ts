import { assert, assertEquals, assertRejects } from "@std/assert";
import { processTransport } from "./conformance_process.ts";
import type { Mutation, Scope, WriteRecord } from "./conversation_types.ts";
import type { ReadSelectionOperation } from "./read_selection_types.ts";
type Call = (body: unknown) => Promise<unknown>;
const binary = Deno.args[0] ?? "target/debug/examples/conversation_fixture";
const endpoint = Deno.args[1] ?? "http://127.0.0.1:8831";
const remoteEndpoint = Deno.args[3] ?? endpoint;
const targets = new Set((Deno.args[2] ?? "native,workerd,remote").split(","));
const directory = await Deno.makeTempDir({ prefix: "cwpdb-selection-" });
const run = crypto.randomUUID();
const results: Record<string, unknown[]> = {};
const callWorker: Call = async (body) => {
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
async function exercise(name: string, call: Call) {
  const base = `cwp://fixture/workspaces/selection-${run}`;
  const ref = (part: string) => `${base}/${part}`;
  const scope: Scope = {
    runtime_root_ref: ref("runtime"),
    context: { workspace_ref: base, caller_ref: ref("host") },
  };
  const conversation = ref("conversation"),
    actor = ref("actor"),
    recipient = ref("recipient"),
    message = ref("message"),
    invocation = ref("invocation"),
    execution = ref("execution"),
    root = ref("timeline"),
    item = ref("item"),
    turn = ref("turn"),
    child = ref("child"),
    stream = ref("stream"),
    streamItem = ref("stream-item"),
    artifact = ref("artifact"),
    start = ref("start"),
    update = ref("update"),
    attempt = ref("attempt"),
    operation = ref("operation"),
    file = ref("file"),
    fork = ref("fork"),
    stop = ref("stop"),
    intrinsic = ref("intrinsic"),
    identity = ref("identity");
  const bootstrap = await call({
    operation: "read_conversation_graph",
    request: {
      scope,
      selection: { selection: "bootstrap" },
      continuation: null,
      page_size: 100,
      max_bytes: 1048576,
      expected_revision: null,
    },
  }) as { revision: string };
  let revision = bootstrap.revision, number = 0;
  const record = (
    reference: string,
    kind: WriteRecord["kind"],
    properties: Record<string, unknown>,
  ): WriteRecord => ({
    reference,
    kind,
    properties,
    owner_ref: kind.startsWith("conversation_timeline_") ? conversation : null,
  });
  const member = (
    source_ref: string,
    kind: string,
    target_ref: string,
    position = 0,
  ): Mutation["changes"]["memberships"][number] => ({
    operation: "upsert",
    source_ref,
    kind: kind as Mutation["changes"]["memberships"][number] extends
      { kind: infer K } ? K : never,
    member: { target_ref, position, properties: {} },
  });
  const commit = async (changes: Partial<Mutation["changes"]>) => {
    const outcome = await call({
      operation: "commit_conversation_graph",
      request: {
        scope,
        request_id: `selection-${++number}`,
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
  const invocationRecord = {
    invocation_ref: invocation,
    conversation_ref: conversation,
    requester_actor_ref: actor,
    target_actor_ref: actor,
    action_name: "respond_to--message",
    source_message_ref: message,
    parent_invocation_ref: null,
    state: "running",
    admitted_at_unix_ms: 1,
    started_at_unix_ms: 2,
    finished_at_unix_ms: null,
    stop_requested_at_unix_ms: null,
    attempt_count: 1,
    attempt_active: true,
    attempt_started_at_unix_ms: 3,
    attempt_started_after_activity_sequence: 0,
    input: { secret: "LEDGER_SECRET" },
    responses: [{
      status: 102,
      summary: "Working",
      result: { secret: "LEDGER_SECRET" },
    }],
    reply_message_ref: message,
  };
  const work = {
    activity_ref: "fixture-tool",
    kind: "command",
    label: "Working",
    state: "running",
    started_at_unix_ms: 10,
    duration_ms: null,
    child_invocation_ref: null,
    command: "printf café😀",
    output: "Ready",
    error: null,
    exit_code: 0,
    files: [],
  };
  await commit({
    state_version:
      "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09/conversation-timeline-v1",
    records: [
      record(conversation, "conversation", {
        owner_context_ref: base,
        title: "Family calendar",
        sidebar_eligible: true,
      }),
      record(execution, "conversation", { sidebar_eligible: false }),
      record(root, "conversation_timeline_connection", {
        revision: 1,
        next_sequence: 2,
        store_sequence: 0,
      }),
      record(child, "conversation_timeline_connection", {
        revision: 1,
        next_sequence: 1,
        store_sequence: 1,
      }),
      record(item, "conversation_timeline_item", {
        publication_ref: ref("publication"),
      }),
      record(turn, "conversation_timeline_item", {
        publication_ref: ref("turn-publication"),
      }),
      record(streamItem, "conversation_timeline_item", {
        publication_ref: ref("stream-publication"),
      }),
      record(message, "message", {
        record: {
          message_ref: message,
          author_actor_ref: actor,
          addressed_actor_refs: [recipient],
          text: "School concert",
          visualizations: [{ artifact_ref: artifact, title: "Concert" }],
        },
      }),
      record(stream, "message", {
        streaming: {
          conversation_ref: conversation,
          invocation_ref: invocation,
          phase: "commentary",
          message: {
            message_ref: stream,
            author_actor_ref: actor,
            addressed_actor_refs: [],
            text: "Checking",
            reply_to_message_ref: message,
          },
        },
      }),
      record(invocation, "invocation", { record: invocationRecord }),
      record(artifact, "visualization_artifact", {
        record: { artifact_ref: artifact, html: "<p>Concert</p>" },
      }),
      record(identity, "local_actor_identity", {
        record: { actor_ref: actor, name: "Fixture", handle: "fixture" },
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
      record(update, "activity", {
        record: {
          activity_ref: update,
          sequence: 2,
          occurred_at_unix_ms: 10,
          event: { kind: "work_update", activity: work },
        },
      }),
      record(attempt, "tool_attempt", {
        record: {
          attempt_ref: attempt,
          invocation_ref: invocation,
          attempt: 1,
          capture_version: 1,
        },
      }),
      record(operation, "tool_operation", {
        record: {
          activity_ref: operation,
          sequence: 0,
          attempt_ref: attempt,
          activity: work,
        },
      }),
      record(file, "tool_file", {
        record: { path: "notes.txt", kind: "update", diff: "line\n" },
      }),
      record(fork, "invocation", {
        record: {
          ...invocationRecord,
          invocation_ref: fork,
          source_message_ref: null,
          target_actor_ref: intrinsic,
          action_name: "fork--conversation",
          state: "completed",
          responses: [{
            result: {
              kind: "conversation_forked",
              conversation_ref: conversation,
            },
          }],
        },
      }),
      record(stop, "invocation", {
        record: {
          ...invocationRecord,
          invocation_ref: stop,
          source_message_ref: null,
          target_actor_ref: intrinsic,
          action_name: "stop--invocation",
          state: "completed",
          input: { invocation_ref: invocation },
          responses: [{
            result: { kind: "invocation_stopped", invocation_ref: invocation },
          }],
        },
      }),
    ],
    memberships: [
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", conversation),
      member(`${scope.runtime_root_ref}/cwpdb`, "conversation", execution, 1),
      member(execution, "parent", conversation),
      member(conversation, "participant", actor),
      member(conversation, "conversation_timeline_root", root),
      member(root, "conversation_timeline_item", item),
      member(item, "conversation_timeline_message", message),
      member(root, "conversation_timeline_item", turn, 1),
      member(turn, "conversation_timeline_invocation", invocation),
      member(turn, "conversation_timeline_child", child),
      member(child, "conversation_timeline_item", streamItem),
      member(streamItem, "conversation_timeline_message", stream),
      member(conversation, "message", message),
      member(invocation, "execution", execution),
      member(execution, "activity", start),
      member(execution, "activity", update, 1),
      member(invocation, "tool_attempt", attempt),
      member(invocation, "tool_operation", operation),
      member(operation, "tool_file", file),
    ],
  });
  let pinned: string | null = null;
  const observed: unknown[] = [];
  const select = async (
    selection: ReadSelectionOperation,
    max_bytes = 1048576,
    expected = pinned,
  ) => {
    const result = await call({
      operation: "read_conversation_selection",
      request: {
        scope,
        expected_commit_revision: expected,
        selection,
        deadline_unix_ms: Date.now() + 30000,
        max_bytes,
      },
    }) as {
      runtime_root_ref: string;
      commit_revision: string;
      result: unknown;
    };
    assertEquals(result.runtime_root_ref, scope.runtime_root_ref);
    observed.push({ operation: selection.operation, result: result.result });
    return result;
  };
  pinned = (await select({ operation: "position" })).commit_revision;
  assertEquals(
    (await select({
      operation: "conversation_exists",
      conversation_ref: conversation,
      include_execution: false,
      owner: null,
    })).result,
    true,
  );
  assertEquals(
    (await select({
      operation: "participant_visible",
      conversation_ref: conversation,
      actor_ref: recipient,
    })).result,
    false,
  );
  assertEquals(
    (await select({
      operation: "root_conversation_presentation",
      conversation_ref: conversation,
    })).result,
    ["Family calendar", false],
  );
  await select({ operation: "fork_source", conversation_ref: conversation });
  await select({ operation: "owner_context", conversation_ref: conversation });
  await select({
    operation: "timeline_item_conversation",
    item_ref: streamItem,
  });
  await select({ operation: "actor_identity", actor_ref: actor });
  await select({ operation: "actor_candidate", actor_ref: actor });
  await select({ operation: "actor_conversations", actor_ref: actor });
  await select({ operation: "actor_resources", actor_ref: actor });
  await select({ operation: "artifact_exists", reference: artifact });
  await select({ operation: "artifact_messages", reference: artifact });
  await select({ operation: "artifact_html", reference: artifact });
  await select({
    operation: "invocation_reply_ref",
    invocation_ref: invocation,
  });
  await select({
    operation: "invocation_fork_destination",
    invocation_ref: fork,
    intrinsic_actor_ref: intrinsic,
  });
  await select({
    operation: "invocation_stop_facts",
    invocation_ref: stop,
    intrinsic_actor_ref: intrinsic,
  });
  await select({
    operation: "streaming_message_metadata",
    message_ref: stream,
  });
  await select({
    operation: "message_action_invocation",
    conversation_ref: conversation,
    message_ref: message,
  });
  await select({
    operation: "progress_activity",
    invocation_ref: invocation,
    source_activity_ref: update,
  });
  await select({
    operation: "progress_in_latest_attempt",
    invocation_ref: invocation,
    sequence: 2,
  });
  await select({
    operation: "latest_work_activity",
    invocation_ref: invocation,
    activity_ref: "fixture-tool",
  });
  await select({
    operation: "timeline_root",
    conversation_ref: conversation,
    include_execution: false,
  });
  await select({
    operation: "timeline_connection_metadata",
    conversation_ref: conversation,
    root_ref: root,
    connection_ref: child,
  });
  await select({
    operation: "timeline_cursor_exists",
    connection_ref: root,
    sequence: 0,
    item_ref: item,
  });
  await select({
    operation: "timeline_connection_items",
    connection_ref: root,
    lower: -1,
    upper: 100,
    limit: 1,
    backwards: false,
  });
  await select({
    operation: "timeline_connection_items",
    connection_ref: root,
    lower: null,
    upper: null,
    limit: 1,
    backwards: false,
  });
  await select({
    operation: "timeline_connection_count",
    conversation_ref: conversation,
    connection_ref: root,
  });
  await select({
    operation: "timeline_item",
    conversation_ref: conversation,
    item_ref: turn,
  });
  await select({
    operation: "inspector_capture_available",
    invocation_ref: invocation,
  });
  await select({
    operation: "inspector_details_allowed",
    invocation_ref: invocation,
    actor_ref: actor,
  });
  await select({
    operation: "inspector_invocation_root",
    invocation_ref: invocation,
  });
  await select({
    operation: "inspector_scope",
    actor_ref: actor,
    invocation_ref: invocation,
    collection: "activities",
    activity_ref: null,
  });
  const inspectorRequest = {
    invocation_ref: invocation,
    conversation_ref: conversation,
    actor_ref: actor,
    collection: "activities" as const,
    activity_ref: null,
    details: true,
    after: null,
    before: null,
    size: 10,
    backwards: false,
    remaining_bytes: 524288,
  };
  await select({ operation: "inspector_page", request: inspectorRequest });
  await select({
    operation: "inspector_page",
    request: {
      ...inspectorRequest,
      collection: "files",
      activity_ref: operation,
    },
  });
  await select({
    operation: "inspector_activity_sequence",
    invocation_ref: invocation,
    conversation_ref: conversation,
    activity_ref: operation,
  });
  await select({
    operation: "inspector_exit_code",
    invocation_ref: invocation,
    conversation_ref: conversation,
    actor_ref: actor,
    activity_ref: operation,
  });
  await select({
    operation: "inspector_text",
    request: {
      invocation_ref: invocation,
      conversation_ref: conversation,
      actor_ref: actor,
      activity_ref: operation,
      file: null,
      field: "command",
    },
  });
  await select({
    operation: "invocation_target_source",
    invocation_ref: invocation,
  });
  await select({
    operation: "message_is_commentary",
    message_ref: stream,
    invocation_ref: invocation,
  });
  await select({
    operation: "message_is_final_reply",
    message_ref: message,
    target_actor_ref: actor,
    source_message_ref: null,
  });
  await select({
    operation: "last_addressed_actors",
    conversation_ref: conversation,
    root_ref: root,
    author_actor_ref: actor,
  });
  await select({
    operation: "summary_row",
    reference: conversation,
    workspace: null,
  });
  await select({
    operation: "summary_row",
    reference: conversation,
    workspace: [base, actor],
  });
  await select({
    operation: "summary_message_record",
    conversation_ref: conversation,
    message_ref: message,
  });
  await select({
    operation: "summary_window",
    request: {
      participant_actor_ref: actor,
      archived: null,
      owner_context_filter: [base, true],
      after: null,
      before: null,
      first: 10,
      last: null,
    },
  });
  const current = await select({
    operation: "current",
    conversation_ref: conversation,
  });
  assert(!JSON.stringify(current).includes("LEDGER_SECRET"));
  await select({
    operation: "invocation",
    conversation_ref: conversation,
    invocation_ref: invocation,
  });
  await select({ operation: "resource", reference: message });
  await select({
    operation: "invocation_visible",
    root_ref: root,
    invocation_ref: invocation,
  });
  await assertRejects(
    () => select({ operation: "current", conversation_ref: conversation }, 16),
    Error,
    "byte budget",
  );
  await assertRejects(
    () =>
      select(
        { operation: "current", conversation_ref: conversation },
        1048576,
        "0",
      ),
    Error,
    "revision conflict",
  );
  await assertRejects(
    () =>
      select({
        operation: "inspector_page",
        request: { ...inspectorRequest, actor_ref: recipient },
      }),
    Error,
    "unavailable",
  );
  await commit({
    records: [
      record(conversation, "conversation", {
        owner_context_ref: base,
        title: "Changed",
        sidebar_eligible: true,
      }),
    ],
  });
  await assertRejects(
    () =>
      select({
        operation: "root_conversation_presentation",
        conversation_ref: conversation,
      }),
    Error,
    "revision conflict",
  );
  // Four independently retained sources exceed one wire envelope together.
  // Summary windows must return references, then fetch one selected body.
  const largeText = "x".repeat(7 * 1024 * 1024);
  for (let index = 0; index < 4; index++) {
    const conv = ref(`large-conversation-${index}`),
      msg = ref(`large-message-${index}`),
      connection = ref(`large-timeline-${index}`),
      placement = ref(`large-item-${index}`);
    const timelineRecord = (
      reference: string,
      kind: WriteRecord["kind"],
      properties: Record<string, unknown>,
    ): WriteRecord => ({ reference, kind, properties, owner_ref: conv });
    await commit({
      records: [
        record(conv, "conversation", {
          owner_context_ref: base,
          title: `Large ${index}`,
          sidebar_eligible: true,
        }),
        record(msg, "message", {
          record: {
            message_ref: msg,
            author_actor_ref: actor,
            addressed_actor_refs: [],
            text: largeText,
          },
        }),
        timelineRecord(connection, "conversation_timeline_connection", {
          revision: 1,
          next_sequence: 1,
          store_sequence: index + 2,
        }),
        timelineRecord(placement, "conversation_timeline_item", {
          publication_ref: ref(`large-publication-${index}`),
        }),
      ],
      memberships: [
        member(
          `${scope.runtime_root_ref}/cwpdb`,
          "conversation",
          conv,
          index + 2,
        ),
        member(conv, "participant", actor),
        member(conv, "message", msg),
        member(conv, "conversation_timeline_root", connection),
        member(connection, "conversation_timeline_item", placement),
        member(placement, "conversation_timeline_message", msg),
      ],
    });
  }
  pinned =
    (await select({ operation: "position" }, 1048576, null)).commit_revision;
  const largeWindow = (await select({
    operation: "summary_window",
    request: {
      participant_actor_ref: actor,
      archived: null,
      owner_context_filter: [base, true],
      after: null,
      before: null,
      first: 4,
      last: null,
    },
  })).result as {
    summaries: {
      key: { conversation_ref: string };
      last_message_ref: string;
    }[];
  };
  assertEquals(largeWindow.summaries.length, 4);
  assert(JSON.stringify(largeWindow).length < 16384);
  for (const row of largeWindow.summaries) {
    const facts = await call({
      operation: "read_conversation_selection",
      request: {
        scope,
        expected_commit_revision: pinned,
        selection: {
          operation: "summary_message_record",
          conversation_ref: row.key.conversation_ref,
          message_ref: row.last_message_ref,
        },
        deadline_unix_ms: Date.now() + 30000,
        max_bytes: 24 * 1024 * 1024,
      },
    }) as { result: { text: string } };
    assertEquals(facts.result.text.length, largeText.length);
  }
  results[name] = JSON.parse(
    JSON.stringify(observed).split(base).join(
      "cwp://fixture/workspaces/selection",
    ),
  );
  console.log(
    `${name}: ${observed.length} finite selection results, 4 refusal checks, and four 7 MiB selected-message checks passed`,
  );
}
try {
  if (targets.has("native")) {
    const native = await processTransport(
      [`${directory}/state.sqlite`],
      binary,
    );
    try {
      await exercise("native", native.call);
    } finally {
      await native.close();
    }
  }
  if (targets.has("workerd")) await exercise("workerd", callWorker);
  if (targets.has("remote")) {
    const remote = await processTransport(["--remote", remoteEndpoint], binary);
    try {
      await exercise("remote", remote.call);
    } finally {
      await remote.close();
    }
  }
  const names = Object.keys(results);
  for (const name of names.slice(1)) {
    assertEquals(
      results[name],
      results[names[0]],
      `${name} finite facts differ from ${names[0]}`,
    );
  }
  console.log(`finite selection exact parity: ${names.join(",")}`);
} finally {
  await Deno.remove(directory, { recursive: true });
}
