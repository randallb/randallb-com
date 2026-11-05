import { Graph } from "./conversation_graph.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
import { pageBudget } from "./conversation_pages.ts";
import { object, type Scope } from "./conversation_types.ts";
import type { PositionCursor } from "./conversation_page_types.ts";
export type HistorySelection =
  | {
    kind: "page";
    after: PositionCursor | null;
    before: PositionCursor | null;
    first: number | null;
    last: number | null;
  }
  | {
    kind: "archive_page";
    after: PositionCursor | null;
    before: PositionCursor | null;
    first: number | null;
    last: number | null;
  }
  | {
    kind: "page_positions";
    after: PositionCursor | null;
    before: PositionCursor | null;
    first: number | null;
    last: number | null;
  }
  | {
    kind: "scan_positions";
    after_position: number | null;
    after_revision: string | null;
    first: number;
  }
  | { kind: "retained_message"; message_ref: string }
  | { kind: "archive_message"; message_ref: string }
  | {
    kind: "content_positions";
    after: PositionCursor | null;
    before: PositionCursor | null;
    first: number | null;
    last: number | null;
  }
  | { kind: "message"; message_ref: string }
  | {
    kind: "replies";
    message_ref: string;
    after_message_ref: string | null;
    after_revision: string | null;
    first: number;
  }
  | {
    kind: "scan";
    after_position: number | null;
    after_revision: string | null;
    first: number;
  };
export interface HistoryReadRequest {
  scope: Scope;
  conversation_ref: string;
  selection: HistorySelection;
  expected_commit_revision: string | null;
  participant_actor_ref: string | null;
  include_execution: boolean;
  deadline_unix_ms: number;
  max_bytes: number;
}
const bytes = (v: unknown) =>
  new TextEncoder().encode(JSON.stringify(v)).length;
class HistoryReader extends PageReader {
  constructor(graph: Graph, readonly request: HistoryReadRequest) {
    super(graph);
  }
  override take() {
    if (Date.now() >= this.request.deadline_unix_ms) {
      throw new Error("conversation read operation exceeded its deadline");
    }
    super.take();
  }
  async origin(
    message: string,
    record: Record<string, unknown>,
  ): Promise<string | null> {
    const progress = record.progress == null ? {} : object(record.progress);
    if (typeof progress.invocation_ref === "string") {
      return progress.invocation_ref;
    }
    const origins = new Set<string>();
    for (
      const edge of await this.incoming(message, "field:reply_message_ref")
    ) {
      const node = await this.node(edge.source_ref),
        r = node?.properties.record == null
          ? {}
          : object(node.properties.record);
      if (
        (node?.kind === "invocation" || node?.kind === "admission") &&
        r.conversation_ref === this.request.conversation_ref &&
        r.reply_message_ref === message
      ) {
        const origin = node.kind === "invocation"
          ? node.reference
          : r.invocation_ref;
        if (typeof origin === "string") origins.add(origin);
      }
    }
    return origins.size === 1 ? Array.from(origins)[0] : null;
  }
}
export async function historyRead(graph: Graph, request: HistoryReadRequest) {
  pageBudget(request.max_bytes);
  if (
    !Number.isSafeInteger(request.deadline_unix_ms) ||
    Date.now() >= request.deadline_unix_ms
  ) throw new Error("conversation read operation exceeded its deadline");
  const reader = new HistoryReader(graph, request),
    context = await reader.context(request.conversation_ref);
  if (!context) return null;
  if (
    (!request.include_execution && context.conversation.parent_ref !== null) ||
    (request.participant_actor_ref !== null &&
      !context.conversation.participant_actor_refs.includes(
        request.participant_actor_ref,
      ))
  ) {
    throw new Error(
      "conversation is unavailable to the requesting participant",
    );
  }
  if (
    request.expected_commit_revision !== null &&
    request.expected_commit_revision !== context.commit_revision
  ) throw new Error("conversation revision_conflict");
  const all = await reader.keys(
      request.conversation_ref,
      context.root,
      request.selection.kind !== "content_positions",
    ),
    selection = request.selection;
  let selected = all, next = false, previous = false;
  let replies: string[] = [], nextReplies = false;
  const validateFirst = (n: number) => {
    if (!Number.isSafeInteger(n) || n < 1 || n > 500) {
      throw new Error("invalid history page size");
    }
  };
  if (
    selection.kind === "page" || selection.kind === "archive_page" ||
    selection.kind === "page_positions" ||
    selection.kind === "content_positions"
  ) {
    const { after, before, first, last } = selection;
    if (first === null && last === null) {
      throw new Error("conversation record page requires first or last");
    }
    if (
      [first, last].some((n) =>
        n !== null && (!Number.isSafeInteger(n) || n < 0 || n > 500)
      )
    ) throw new Error("conversation record page size must not exceed500");
    if (after && before && after.position >= before.position) {
      throw new Error(
        "conversation record page boundaries do not define a forward range",
      );
    }
    for (const cursor of [after, before]) {
      if (cursor) {
        if (!Number.isSafeInteger(cursor.position)) {
          throw new Error("invalid history position cursor");
        }
        if (context.root && cursor.revision !== context.revision) {
          throw new Error(
            "conversation record cursor belongs to an earlier revision",
          );
        }
        if (!all.some((k) => k.position === cursor.position)) {
          throw new Error(
            "conversation record page boundary is not in the requested relation",
          );
        }
      }
    }
    selected = all.filter((k) =>
      (!after || k.position > after.position) &&
      (!before || k.position < before.position)
    );
    if (first !== null) {
      next = selected.length > first;
      selected = selected.slice(0, first);
    }
    if (last !== null) {
      previous = selected.length > last;
      selected = last === 0 ? [] : selected.slice(-last);
    }
  } else if (selection.kind === "scan" || selection.kind === "scan_positions") {
    validateFirst(selection.first);
    if (
      selection.after_position !== null &&
      !Number.isSafeInteger(selection.after_position)
    ) throw new Error("invalid history position cursor");
    if (
      context.root && selection.after_position !== null &&
      selection.after_revision !== context.revision
    ) {
      throw new Error(
        "conversation cursor belongs to an earlier canonical timeline revision",
      );
    }
    selected = all.filter((k) =>
      selection.after_position === null || k.position > selection.after_position
    );
    next = selected.length > selection.first;
    selected = selected.slice(0, selection.first);
  } else {
    if (
      !context.root &&
      (selection.kind === "message" || selection.kind === "replies")
    ) {
      const publication = await reader.graph.edge(
        request.conversation_ref,
        "timeline_item",
        selection.message_ref,
      );
      if (publication?.properties.item_kind !== "message") {
        throw new Error("message is unavailable in the requested conversation");
      }
    }
    const anchor = all.find((k) => k.reference === selection.message_ref);
    if (!anchor) {
      throw new Error("message is unavailable in the requested conversation");
    }
    selected = [anchor];
    if (selection.kind === "replies") {
      validateFirst(selection.first);
      if (
        context.root && selection.after_message_ref !== null &&
        selection.after_revision !== context.revision
      ) {
        throw new Error(
          "conversation cursor belongs to an earlier canonical timeline revision",
        );
      }
      const matching = [];
      for (const k of all) {
        const node = await reader.node(k.reference);
        if (
          object(node?.properties.record).reply_to_message_ref ===
            selection.message_ref
        ) matching.push(k);
      }
      let lower = -Infinity;
      if (selection.after_message_ref !== null) {
        const after = matching.find((k) =>
          k.reference === selection.after_message_ref
        );
        if (!after) {
          throw new Error(
            "reply cursor is not a reply to the requested message",
          );
        }
        lower = after.position;
      }
      const window = matching.filter((k) => k.position > lower);
      next = window.length > selection.first;
      replies = window.slice(0, selection.first).map((k) => k.reference);
    }
  }
  const result = {
    runtime_root_ref: request.scope.runtime_root_ref,
    conversation_ref: request.conversation_ref,
    commit_revision: context.commit_revision,
    conversation: context.conversation,
    canonical: context.root !== null,
    revision: context.revision,
    items: [] as {
      position: number;
      message_ref: string;
      record: Record<string, unknown> | null;
      payload_bytes: number;
      origin_invocation_ref: string | null;
    }[],
    has_next_page: next,
    has_previous_page: previous,
    total_count:
      ["page", "archive_page", "page_positions", "content_positions"].includes(
          selection.kind,
        )
        ? all.length
        : 0,
    reply_message_refs: replies,
  };
  let used = bytes(result);
  for (const key of selected) {
    reader.take();
    if (
      selection.kind === "page_positions" ||
      selection.kind === "scan_positions" ||
      selection.kind === "content_positions"
    ) {
      const item = {
        position: key.position,
        message_ref: key.reference,
        record: null,
        payload_bytes: 0,
        origin_invocation_ref: null,
      };
      used += bytes(item) + (result.items.length ? 1 : 0);
      if (used > request.max_bytes) {
        throw new Error("history response exceeds byte budget");
      }
      result.items.push(item);
      continue;
    }
    const record = await reader.message(
      request.conversation_ref,
      key.reference,
    );
    if (!record || record.message_ref !== key.reference) {
      throw new Error("selected history message identity mismatch");
    }
    const size = bytes(record),
      item = {
        position: key.position,
        message_ref: key.reference,
        record: !["archive_page", "archive_message"].includes(selection.kind) &&
            size > 8 * 1024 * 1024
          ? null
          : record,
        payload_bytes: size,
        origin_invocation_ref: await reader.origin(key.reference, record),
      };
    used += bytes(item) + (result.items.length ? 1 : 0);
    if (used > request.max_bytes) {
      throw new Error("history response exceeds byte budget");
    }
    result.items.push(item);
  }
  reader.take();
  if (bytes(result) > request.max_bytes) {
    throw new Error("history response exceeds byte budget");
  }
  return result;
}
