import { Graph, key } from "./conversation_graph.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
import { pageBudget } from "./conversation_pages.ts";
import {
  object,
  type ObjectValue,
  type Relationship,
  type Scope,
} from "./conversation_types.ts";
import type { PositionCursor } from "./conversation_page_types.ts";
export interface TimelineReadRequest {
  scope: Scope;
  conversation_ref: string;
  after: PositionCursor | null;
  before: PositionCursor | null;
  first: number | null;
  last: number | null;
  participant_actor_ref: string | null;
  include_execution: boolean;
  deadline_unix_ms: number;
  max_bytes: number;
}
export type TimelineAnchorFacts = {
  kind: "message";
  position: number;
  message_ref: string;
} | { kind: "turn"; position: number; invocation_ref: string };
function compareRefs(a: string, b: string): number {
  const left = Array.from(a), right = Array.from(b);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const delta = left[i].codePointAt(0)! - right[i].codePointAt(0)!;
    if (delta) return delta;
  }
  return left.length - right.length;
}
const bytes = (value: unknown) =>
  new TextEncoder().encode(JSON.stringify(value)).length;
class TimelineReader extends PageReader {
  constructor(graph: Graph, readonly request: TimelineReadRequest) {
    super(graph);
  }
  override take() {
    if (Date.now() >= this.request.deadline_unix_ms) {
      throw new Error("conversation read operation exceeded its deadline");
    }
    super.take();
  }
  async anchors(root: string | null): Promise<TimelineAnchorFacts[]> {
    if (!root) {
      return (await this.edges(this.request.conversation_ref, "timeline_item"))
        .map((edge) => {
          if (edge.properties.item_kind === "message") {
            return {
              kind: "message",
              position: edge.position,
              message_ref: edge.target_ref,
            };
          }
          if (edge.properties.item_kind === "turn") {
            return {
              kind: "turn",
              position: edge.position,
              invocation_ref: edge.target_ref,
            };
          }
          throw new Error("timeline item has unsupported content");
        });
    }
    const all: TimelineAnchorFacts[] = [], path = new Set<string>();
    let occurrences = 0;
    const walk = async (connection: string, depth: number): Promise<void> => {
      this.take();
      if (depth >= 128 || path.has(connection)) {
        throw new Error("conversation content walk work limit exceeded");
      }
      path.add(connection);
      for (
        const item of await this.edges(connection, "conversation_timeline_item")
      ) {
        if (++occurrences > 50000) {
          throw new Error("conversation content walk work limit exceeded");
        }
        const messages = await this.edges(
            item.target_ref,
            "conversation_timeline_message",
          ),
          turns = await this.edges(
            item.target_ref,
            "conversation_timeline_invocation",
          );
        if (messages.length + turns.length !== 1) {
          throw new Error(
            "timeline item must point to exactly one content record",
          );
        }
        const streaming = messages.length &&
          (await this.flags.get(messages[0].target_ref))?.streaming;
        if (!streaming) {
          all.push(
            messages.length
              ? {
                kind: "message",
                position: all.length,
                message_ref: messages[0].target_ref,
              }
              : {
                kind: "turn",
                position: all.length,
                invocation_ref: turns[0].target_ref,
              },
          );
        }
        for (
          const child of await this.edges(
            item.target_ref,
            "conversation_timeline_child",
          )
        ) await walk(child.target_ref, depth + 1);
      }
      path.delete(connection);
    };
    await walk(root, 0);
    return all;
  }
  async projected(
    reference: string,
    kind: string,
  ): Promise<{ position: number; record: ObjectValue }[]> {
    const result = [];
    for (const lookup of await this.edges(reference, kind)) {
      const locator = await this.graph.tx.get<string[] | null>(
        key(
          this.graph.scope.runtime_root_ref,
          "edge-reference",
          lookup.target_ref,
        ),
      );
      let edge: Relationship | null = null;
      if (!locator) {
        // Rebuild retained private locators from this conversation only.
        edge =
          (await this.edges(this.request.conversation_ref, "timeline_activity"))
            .find((candidate) => candidate.reference === lookup.target_ref) ??
            null;
      } else if (
        locator[0] === this.request.conversation_ref &&
        locator[1] === "timeline_activity"
      ) {
        edge = await this.graph.edge(locator[0], locator[1], locator[2]);
      }
      if (edge) {
        result.push({
          position: edge.position,
          record: object(edge.properties.record),
        });
      }
    }
    return result.sort((a, b) => a.position - b.position);
  }
  async recorded(reference: string): Promise<ObjectValue | null> {
    let found: ObjectValue | null = null, best = Infinity;
    for (
      const edge of await this.incoming(reference, "field:event.message_ref")
    ) {
      const member = await this.graph.edge(
        this.request.conversation_ref,
        "activity",
        edge.source_ref,
      );
      if (!member || member.position >= best) continue;
      const node = await this.node(edge.source_ref);
      if (node?.kind !== "activity") continue;
      const record = object(node.properties.record);
      if (object(record.event).kind !== "message_recorded") continue;
      found = record;
      best = member.position;
    }
    return found;
  }
}
export async function timelinePage(graph: Graph, request: TimelineReadRequest) {
  pageBudget(request.max_bytes);
  if (
    !Number.isSafeInteger(request.deadline_unix_ms) ||
    Date.now() >= request.deadline_unix_ms
  ) throw new Error("conversation read operation exceeded its deadline");
  if (request.first == null && request.last == null) {
    throw new Error("conversation record page requires first or last");
  }
  if (
    [request.first, request.last].some((n) =>
      n !== null && (!Number.isSafeInteger(n) || n < 0 || n > 500)
    )
  ) throw new Error("conversation record page size must not exceed 500");
  if (
    [request.after, request.before].some((c) =>
      c && !Number.isSafeInteger(c.position)
    )
  ) throw new Error("invalid timeline position cursor");
  if (
    request.after && request.before &&
    request.after.position >= request.before.position
  ) {
    throw new Error(
      "conversation record page boundaries do not define a forward range",
    );
  }
  const reader = new TimelineReader(graph, request),
    context = await reader.context(request.conversation_ref);
  if (!context) return null;
  if (
    (!request.include_execution && context.conversation?.parent_ref !== null) ||
    request.participant_actor_ref !== null &&
      !context.conversation?.participant_actor_refs.includes(
        request.participant_actor_ref,
      )
  ) {
    throw new Error(
      "conversation is unavailable to the requesting participant",
    );
  }
  const header = await reader.node(request.conversation_ref);
  const revision = context.root
    ? await reader.flatRevision(context.root)
    : String(
      header!.properties.timeline_revision ??
        header!.properties.commit_revision ?? 0,
    );
  for (const cursor of [request.after, request.before]) {
    if (
      cursor &&
      ((context.root && cursor.revision !== revision) ||
        (!context.root && cursor.revision !== null &&
          cursor.revision !== revision))
    ) {
      throw new Error(
        "conversation record cursor belongs to an earlier revision",
      );
    }
  }
  const all = await reader.anchors(context.root);
  for (const cursor of [request.after, request.before]) {
    if (cursor && !all.some((a) => a.position === cursor.position)) {
      throw new Error(
        "conversation record page boundary is not in the requested relation",
      );
    }
  }
  let anchors = all.filter((a) =>
    (!request.after || a.position > request.after.position) &&
    (!request.before || a.position < request.before.position)
  );
  let next = false, previous = false;
  if (request.first !== null) {
    next = anchors.length > request.first;
    anchors = anchors.slice(0, request.first);
  }
  if (request.last !== null) {
    previous = anchors.length > request.last;
    anchors = request.last === 0 ? [] : anchors.slice(-request.last);
  }
  previous ||= !!request.after &&
    all.some((a) => a.position < request.after!.position);
  next ||= !!request.before &&
    all.some((a) => a.position > request.before!.position);
  const { messages, activity } = await dependencies(
    reader,
    all,
    anchors,
    context.root,
  );
  const result = {
    runtime_root_ref: request.scope.runtime_root_ref,
    conversation_ref: request.conversation_ref,
    commit_revision: context.commit_revision,
    conversation: context.conversation,
    revision,
    anchors,
    messages,
    activity,
    has_next_page: next,
    has_previous_page: previous,
    total_count: all.length,
  };
  reader.take();
  if (bytes(result) > request.max_bytes) {
    throw new Error("timeline page exceeds byte budget");
  }
  return result;
}
async function dependencies(
  reader: TimelineReader,
  all: TimelineAnchorFacts[],
  anchors: TimelineAnchorFacts[],
  root: string | null,
) {
  const messageRefs: string[] = [],
    workRefs: string[] = [],
    contextRefs: string[] = [];
  for (const anchor of anchors) {
    if (anchor.kind === "message") messageRefs.push(anchor.message_ref);
    else workRefs.push(anchor.invocation_ref);
  }
  const messagePositions = new Map<string, number>();
  for (const anchor of all) {
    if (
      anchor.kind === "message" && !messagePositions.has(anchor.message_ref)
    ) messagePositions.set(anchor.message_ref, anchor.position);
  }
  const messages = new Map<number, ObjectValue>(),
    activities = new Map<string, ObjectValue>(),
    seenMessages = new Set<string>(),
    seenWork = new Set<string>(),
    seenContext = new Set<string>();
  let used = 0;
  const retain = (map: Map<string, ObjectValue>, record: ObjectValue) => {
    const identity = String(record.activity_ref);
    used += bytes(record) - (map.has(identity) ? bytes(map.get(identity)) : 0);
    if (used > reader.request.max_bytes) {
      throw new Error("timeline page exceeds byte budget");
    }
    map.set(identity, record);
  };
  const enqueue = (record: ObjectValue) => {
    const event = object(record.event);
    if (event.kind !== "work_lifecycle" && event.kind !== "work_update") return;
    if (typeof event.source_message_ref === "string") {
      messageRefs.push(event.source_message_ref);
    }
    if (typeof event.parent_invocation_ref === "string") {
      contextRefs.push(event.parent_invocation_ref);
    }
    if (event.kind === "work_update" && event.wait) {
      const wait = object(event.wait);
      for (const field of ["waiter_invocation_ref", "awaited_invocation_ref"]) {
        if (typeof wait[field] === "string") {
          contextRefs.push(wait[field] as string);
        }
      }
    }
  };
  while (messageRefs.length || workRefs.length || contextRefs.length) {
    reader.take();
    while (workRefs.length) {
      reader.take();
      const reference = workRefs.shift()!;
      if (seenWork.has(reference)) continue;
      seenWork.add(reference);
      if (
        !await reader.visible(
          root,
          reference,
          "conversation_timeline_invocation",
        )
      ) continue;
      for (
        const { record } of await reader.projected(reference, "timeline_parent")
      ) {
        const event = object(record.event);
        if (typeof event.invocation_ref === "string") {
          workRefs.push(event.invocation_ref);
        }
      }
      for (
        const { record } of await reader.projected(
          reference,
          "timeline_invocation",
        )
      ) {
        enqueue(record);
        const event = object(record.event);
        if (event.kind === "work_update" && event.activity) {
          const work = object(event.activity);
          if (typeof work.child_invocation_ref === "string") {
            workRefs.push(work.child_invocation_ref);
          }
        }
        retain(activities, record);
      }
    }
    while (messageRefs.length) {
      reader.take();
      const reference = messageRefs.shift()!;
      if (seenMessages.has(reference)) continue;
      seenMessages.add(reference);
      const canonicalPosition = messagePositions.get(reference);
      const edge = root ? null : await reader.graph.edge(
        reader.request.conversation_ref,
        "timeline_message",
        reference,
      );
      const position = root ? canonicalPosition : edge?.position;
      if (root && position === undefined) continue;
      const message = position === undefined
        ? null
        : await reader.message(reader.request.conversation_ref, reference);
      if (message && position !== undefined) {
        if (typeof message.reply_to_message_ref === "string") {
          messageRefs.push(message.reply_to_message_ref);
        }
        used += bytes(message) -
          (messages.has(position) ? bytes(messages.get(position)) : 0);
        if (used > reader.request.max_bytes) {
          throw new Error("timeline page exceeds byte budget");
        }
        messages.set(position, message);
      }
      const recorded = await reader.recorded(reference);
      if (recorded) retain(activities, recorded);
      const preferred = new Map<
        string,
        { priority: number; record: ObjectValue }
      >();
      for (
        const { position, record } of await reader.projected(
          reference,
          "timeline_source",
        )
      ) {
        const event = object(record.event);
        if (typeof event.invocation_ref !== "string") continue;
        const priority = event.kind === "work_lifecycle" ? -1 : position;
        const old = preferred.get(event.invocation_ref);
        if (!old || priority < old.priority) {
          preferred.set(event.invocation_ref, { priority, record });
        }
      }
      contextRefs.push(...Array.from(preferred.keys()).sort(compareRefs));
    }
    while (contextRefs.length) {
      reader.take();
      const reference = contextRefs.shift()!;
      if (seenWork.has(reference) || seenContext.has(reference)) continue;
      seenContext.add(reference);
      if (
        !await reader.visible(
          root,
          reference,
          "conversation_timeline_invocation",
        )
      ) continue;
      const rows = await reader.projected(reference, "timeline_invocation");
      rows.sort((a, b) =>
        (object(a.record.event).kind === "work_lifecycle" ? -1 : a.position) -
        (object(b.record.event).kind === "work_lifecycle" ? -1 : b.position)
      );
      if (!rows.length) continue;
      const activity = structuredClone(rows[0].record);
      enqueue(activity);
      const event = object(activity.event);
      if (event.kind === "work_update") {
        event.text = null;
        event.activity = null;
      }
      if (!activities.has(String(activity.activity_ref))) {
        retain(activities, activity);
      }
    }
  }
  return {
    messages: Array.from(messages.entries()).sort((a, b) => a[0] - b[0]).map((
      [, record],
    ) => record),
    activity: Array.from(activities.values()).sort((a, b) =>
      Number(a.occurred_at_unix_ms) - Number(b.occurred_at_unix_ms) ||
      Number(a.sequence) - Number(b.sequence) ||
      compareRefs(String(a.activity_ref), String(b.activity_ref))
    ),
  };
}
