import {
  type GraphRecord,
  object,
  type ObjectValue,
  type ResourceExecutionFacts,
  type ResourceRecordFacts,
} from "./conversation_types.ts";
import {
  compare,
  integerOrNull,
  isObject,
  ResourceReader,
  stringOrNull,
} from "./conversation_resource.ts";
const terminal = (record: ObjectValue) =>
  ["completed", "blocked", "failed", "stopped"].includes(String(record.state));
function fields(record: ObjectValue): ObjectValue {
  const result: ObjectValue = {};
  for (
    const name of [
      "invocation_ref",
      "conversation_ref",
      "requester_actor_ref",
      "target_actor_ref",
      "source_message_ref",
      "parent_invocation_ref",
      "state",
      "admitted_at_unix_ms",
      "started_at_unix_ms",
      "finished_at_unix_ms",
      "stop_requested_at_unix_ms",
    ]
  ) result[name] = record[name] ?? null;
  result.stop_settlement_tracked =
    typeof record.stop_settlement_tracked === "boolean"
      ? record.stop_settlement_tracked
      : false;
  const response = Array.isArray(record.responses)
    ? record.responses.at(-1)
    : null;
  const safe = isObject(response) ? response : {};
  const detail = (name: string) =>
    isObject(safe[name])
      ? { code: safe[name].code ?? null, message: safe[name].message ?? null }
      : null;
  result.outcome = terminal(record)
    ? {
      status: safe.status ?? null,
      summary: safe.summary ?? null,
      error: detail("error"),
      cause: detail("cause"),
    }
    : null;
  return result;
}
async function latestReply(
  reader: ResourceReader,
  conversation: string,
  node: GraphRecord,
  sourceMessage: string | null,
): Promise<ObjectValue | null> {
  const record: ObjectValue = {
    ...object(node.properties.record),
    source_message_ref: sourceMessage,
  };
  let messages:
    | { reference: string; position: number; record: ObjectValue }[]
    | undefined;
  const allMessages = async () => {
    if (messages) return messages;
    messages = [];
    for (const edge of await reader.edges(conversation, "message")) {
      const node = await reader.node(edge.target_ref);
      if (node?.kind === "message") {
        messages.push({
          reference: edge.target_ref,
          position: edge.position,
          record: object(node.properties.record),
        });
      }
    }
    return messages;
  };
  if (typeof record.parent_invocation_ref === "string" && terminal(record)) {
    if (typeof record.reply_message_ref === "string") {
      return await reader.message(conversation, record.reply_message_ref);
    }
    const candidates = (await allMessages()).filter((m) =>
      m.record.reply_to_message_ref === (record.source_message_ref ?? "") &&
      m.record.author_actor_ref === (record.target_actor_ref ?? "") &&
      !Object.hasOwn(m.record, "progress") &&
      ((isObject(m.record.action) && m.record.action.kind === "result") ||
        record.action_name === "respond_to--message")
    );
    let shared = false;
    if (candidates.length === 1) {
      // Restrict competing identity checks by the exact retained target actor lookup.
      for (
        const edge of await reader.incoming(
          String(record.target_actor_ref ?? ""),
          "field:target_actor_ref",
        )
      ) {
        if (edge.source_ref === node.reference) continue;
        const other = await reader.node(edge.source_ref);
        if (other?.kind !== "invocation") continue;
        const body = object(other.properties.record);
        if (
          body.conversation_ref !== conversation ||
          body.target_actor_ref !== record.target_actor_ref
        ) continue;
        if (body.source_message_ref === record.source_message_ref) {
          shared = true;
          break;
        }
        const execution = await reader.one(other.reference, "execution");
        if (
          execution &&
          (await reader.activities(execution)).some((a) =>
            isObject(a.record.event) &&
            a.record.event.kind === "turn_started" &&
            a.record.event.source_message_ref === record.source_message_ref
          )
        ) {
          shared = true;
          break;
        }
      }
      if (!shared) {
        return await reader.message(conversation, candidates[0].reference);
      }
    }
  }
  const admissionReplies = new Set<string>();
  for (
    const edge of await reader.incoming(node.reference, "field:invocation_ref")
  ) {
    const admission = await reader.node(edge.source_ref);
    if (admission?.kind !== "admission") continue;
    const reply = object(admission.properties.record).reply_message_ref;
    if (typeof reply === "string") admissionReplies.add(reply);
  }
  const candidate =
    (await allMessages()).filter((m) =>
      (isObject(m.record.progress) &&
        m.record.progress.invocation_ref === node.reference) ||
      admissionReplies.has(m.reference)
    ).sort((a, b) => b.position - a.position)[0];
  return candidate
    ? await reader.message(conversation, candidate.reference)
    : null;
}
async function executionFacts(
  reader: ResourceReader,
  conversation: string,
  record: ObjectValue,
  execution: string,
): Promise<ResourceExecutionFacts | null> {
  if (
    terminal(record) ||
    (record.state === "stopping" && record.stop_execution_pending !== true)
  ) return null;
  const generation = integerOrNull(record.attempt_count) ?? 0;
  if (generation < 1) return null;
  const lower = integerOrNull(record.attempt_started_after_activity_sequence) ??
    (generation === 1 ? -1 : Number.MAX_SAFE_INTEGER);
  const selected = (await reader.activities(execution)).filter((a) =>
    typeof a.record.sequence === "number" && a.record.sequence > lower
  );
  let latest: number | null = null,
    phase: ObjectValue | null = null,
    progress: string | null = null;
  for (const { record: activity } of selected) {
    const time = integerOrNull(activity.occurred_at_unix_ms);
    if (time !== null) {
      if (time < 0) throw new Error("negative execution activity timestamp");
      latest = Math.max(latest ?? time, time);
    }
  }
  for (const { record: activity } of selected.toReversed()) {
    const event = object(activity.event);
    if (phase === null && event.kind === "execution_activity") {
      const value = isObject(event.activity) ? event.activity : {};
      phase = { state: value.state ?? null, phase: value.phase ?? null };
    }
  }
  for (const { edge, record: activity } of selected.toReversed()) {
    const event = object(activity.event);
    if (event.kind !== "progress") continue;
    let published = false;
    for (
      const lookup of await reader.incoming(
        edge.target_ref,
        "field:progress.source_activity_ref",
      )
    ) {
      if (
        (await reader.node(lookup.source_ref))?.kind === "message" &&
        await reader.graph.edge(conversation, "message", lookup.source_ref)
      ) {
        published = true;
        break;
      }
    }
    if (!published) {
      progress = stringOrNull(event.summary);
      break;
    }
  }
  const started = integerOrNull(record.attempt_started_at_unix_ms);
  return {
    generation,
    attempt_active: record.attempt_active === true,
    started_at_unix_ms: started !== null && started >= 0 ? started : null,
    latest_activity_at_unix_ms: latest,
    latest_phase_event: phase,
    unpublished_progress: progress,
  };
}
export async function invocationFacts(
  reader: ResourceReader,
  conversation: string,
  node: GraphRecord,
  root: string | null,
): Promise<ResourceRecordFacts> {
  const children: string[] = [], waits: ObjectValue[] = [];
  for (
    const edge of await reader.incoming(
      node.reference,
      "field:parent_invocation_ref",
    )
  ) {
    const child = await reader.node(edge.source_ref);
    if (
      child?.kind === "invocation" &&
      object(child.properties.record).conversation_ref === conversation
    ) children.push(child.reference);
  }
  children.sort(compare);
  for (
    const edge of await reader.incoming(
      node.reference,
      "field:waiter_invocation_ref",
    )
  ) {
    const wait = await reader.node(edge.source_ref);
    if (wait?.kind !== "wait") continue;
    const body = object(wait.properties.record), safe: ObjectValue = {};
    for (
      const name of [
        "waiter_invocation_ref",
        "awaited_invocation_ref",
        "call_id",
        "state",
        "resume_after_activity_sequence",
      ]
    ) safe[name] = body[name] ?? null;
    waits.push(safe);
  }
  waits.sort((a, b) => compare(String(a.call_id), String(b.call_id)));
  const body = object(node.properties.record),
    record = fields(body),
    executionRef = await reader.one(node.reference, "execution");
  let execution: ResourceExecutionFacts | null = null;
  if (executionRef) {
    const start = (await reader.activities(executionRef)).find((a) =>
      isObject(a.record.event) && a.record.event.kind === "turn_started"
    );
    record.source_message_ref = start && isObject(start.record.event)
      ? stringOrNull(start.record.event.source_message_ref)
      : null;
    execution = await executionFacts(reader, conversation, body, executionRef);
  }
  let latest_reply = await latestReply(
    reader,
    conversation,
    node,
    stringOrNull(record.source_message_ref),
  );
  if (latest_reply) {
    if (typeof latest_reply.message_ref !== "string") {
      throw new Error("reply message ref required");
    }
    if (
      !await reader.visible(
        root,
        latest_reply.message_ref,
        "conversation_timeline_message",
      )
    ) latest_reply = null;
  }
  return {
    kind: "invocation",
    record,
    child_invocation_refs: children,
    wait_records: waits,
    latest_reply,
    execution,
  };
}
