/** Finite indexed operations inside the enclosing revision-pinned transaction. */
import { historyRead } from "./conversation_history_read.ts";
import { completionPage } from "./conversation_pages.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
import { timelinePage } from "./conversation_timeline_read.ts";
import type { PositionCursor } from "./conversation_page_types.ts";
import type { SelectionReader } from "./read_selection_reader.ts";
import type { ReadSelectionRequest } from "./read_selection_types.ts";
export interface PositionWindow {
  after: PositionCursor | null;
  before: PositionCursor | null;
  first: number | null;
  last: number | null;
}
export interface CompletionWindow {
  after: number;
  through: number | null;
  revision: string | null;
  first: number;
}
function exact(
  value: unknown,
  keys: string[],
): asserts value is Record<string, unknown> {
  if (
    value === null || typeof value !== "object" || Array.isArray(value) ||
    Object.keys(value).some((k) => !keys.includes(k))
  ) throw new Error("invalid indexed selection shape");
}
function positionCursor(cursor: PositionCursor) {
  exact(cursor, ["position", "revision"]);
  if (cursor.revision === undefined) cursor.revision = null;
  if (
    !Number.isSafeInteger(cursor.position) ||
    (cursor.revision !== null && typeof cursor.revision !== "string")
  ) throw new Error("invalid indexed position cursor");
}
function positionWindow(window: PositionWindow) {
  exact(window, ["after", "before", "first", "last"]);
  for (const name of ["after", "before"] as const) {
    if (window[name] === undefined) window[name] = null;
    const c = window[name];
    if (c !== null) positionCursor(c);
  }
  for (const name of ["first", "last"] as const) {
    if (window[name] === undefined) window[name] = null;
    const n = window[name];
    if (n !== null && (!Number.isSafeInteger(n) || n < 0)) {
      throw new Error("invalid indexed page count");
    }
  }
}
function completionWindow(window: CompletionWindow) {
  exact(window, ["after", "through", "revision", "first"]);
  if (window.through === undefined) window.through = null;
  if (window.revision === undefined) window.revision = null;
  if (
    !Number.isSafeInteger(window.after) ||
    !Number.isSafeInteger(window.first) ||
    (window.through !== null && !Number.isSafeInteger(window.through)) ||
    (window.revision !== null && typeof window.revision !== "string")
  ) throw new Error("invalid completion window");
}
class SelectedPageReader extends PageReader {
  constructor(readonly owner: SelectionReader) {
    super(owner.graph);
  }
  override take() {
    this.owner.take();
  }
}
export async function selectionContentPositions(
  reader: SelectionReader,
  reference: string,
  window: PositionWindow,
  execution: boolean,
  request: ReadSelectionRequest,
) {
  positionWindow(window);
  return await historyRead(reader.graph, {
    scope: request.scope,
    conversation_ref: reference,
    selection: { kind: "content_positions", ...window },
    expected_commit_revision: request.expected_commit_revision,
    participant_actor_ref: null,
    include_execution: execution,
    deadline_unix_ms: request.deadline_unix_ms,
    max_bytes: request.max_bytes,
  });
}
export async function selectionCompletionPage(
  reader: SelectionReader,
  reference: string,
  window: CompletionWindow,
  execution: boolean,
  request: ReadSelectionRequest,
) {
  completionWindow(window);
  const pages = new SelectedPageReader(reader),
    context = await pages.context(reference);
  if (!context || (!execution && context.conversation.parent_ref !== null)) {
    return null;
  }
  return await completionPage(pages, {
    scope: request.scope,
    conversation_ref: reference,
    after_sequence: window.after,
    through_sequence: window.through,
    cursor_revision: window.revision,
    first: window.first,
    max_bytes: request.max_bytes,
  });
}
export async function selectionTimelinePage(
  reader: SelectionReader,
  reference: string,
  window: PositionWindow,
  execution: boolean,
  request: ReadSelectionRequest,
) {
  positionWindow(window);
  return await timelinePage(reader.graph, {
    scope: request.scope,
    conversation_ref: reference,
    ...window,
    participant_actor_ref: null,
    include_execution: execution,
    deadline_unix_ms: request.deadline_unix_ms,
    max_bytes: request.max_bytes,
  });
}
async function legacy(
  reader: SelectionReader,
  reference: string,
  execution: boolean,
) {
  if (await reader.one(reference, "conversation_timeline_root")) {
    throw new Error(
      "conversation has a canonical timeline; select its retained layout again",
    );
  }
  const pages = new SelectedPageReader(reader),
    context = await pages.context(reference);
  if (!context || (!execution && context.conversation.parent_ref !== null)) {
    throw new Error("unknown conversation");
  }
  const node = await reader.node(reference),
    revision = node?.properties.timeline_revision;
  if (typeof revision === "number" && Number.isSafeInteger(revision)) {
    context.revision = String(revision);
  }
  return { context, anchors: await reader.edges(reference, "timeline_item") };
}
function key(reference: PositionCursor, revision: string, positions: number[]) {
  if (!Number.isSafeInteger(reference.position)) {
    throw new Error("invalid legacy position cursor");
  }
  if (reference.revision !== revision) {
    throw new Error(
      "conversation record cursor belongs to an earlier revision",
    );
  }
  if (!positions.includes(reference.position)) {
    throw new Error(
      "conversation record page boundary is not in the requested relation",
    );
  }
}
export async function selectionLegacyTimelinePage(
  reader: SelectionReader,
  reference: string,
  window: PositionWindow,
  execution: boolean,
  request: ReadSelectionRequest,
) {
  positionWindow(window);
  const { context, anchors } = await legacy(reader, reference, execution);
  let count: number, reverse: boolean;
  if (window.first !== null && window.last === null) {
    count = window.first;
    reverse = false;
  } else if (window.first === null && window.last !== null) {
    count = window.last;
    reverse = true;
  } else {throw new Error(
      "legacy timeline page requires exactly one positive first or last",
    );}
  if (!Number.isSafeInteger(count) || count < 1 || count > 500) {
    throw new Error(
      "legacy timeline page requires exactly one positive first or last",
    );
  }
  for (const cursor of [window.after, window.before]) {
    if (cursor) {
      key(
        cursor,
        context.revision,
        anchors.map((a) => a.position),
      );
    }
  }
  const lower = window.after?.position ?? null,
    upper = window.before?.position ?? null;
  if (lower !== null && upper !== null && lower >= upper) {
    throw new Error(
      "conversation record page boundaries do not define a forward range",
    );
  }
  const candidates = anchors.filter((a) =>
    (lower === null || a.position > lower) &&
    (upper === null || a.position < upper)
  );
  if (reverse) candidates.reverse();
  const overflow = candidates.length > count;
  const selected = candidates.slice(0, count);
  if (reverse) selected.reverse();
  const first = selected[0]?.position ?? upper,
    last = selected.at(-1)?.position ?? lower;
  const previous = (!selected.length && lower !== null) ||
    (reverse && overflow) ||
    (first !== null && anchors.some((a) => a.position < first));
  const next = (!selected.length && upper !== null) || (!reverse && overflow) ||
    (last !== null && anchors.some((a) => a.position > last));
  const result = {
    runtime_root_ref: request.scope.runtime_root_ref,
    conversation_ref: reference,
    commit_revision: context.commit_revision,
    conversation: context.conversation,
    revision: context.revision,
    anchors: selected.map((a) => {
      if (a.properties.item_kind === "message") {
        return {
          kind: "message",
          position: a.position,
          message_ref: a.target_ref,
        };
      }
      if (a.properties.item_kind === "turn") {
        return {
          kind: "turn",
          position: a.position,
          invocation_ref: a.target_ref,
        };
      }
      throw new Error("timeline item has unsupported content");
    }),
    messages: [],
    activity: [],
    has_next_page: next,
    has_previous_page: previous,
    total_count: 0,
  };
  reader.take();
  return result;
}
export async function selectionLegacyTimelineMessage(
  reader: SelectionReader,
  reference: string,
  cursor: PositionCursor,
  message: string,
  execution: boolean,
  _request: ReadSelectionRequest,
) {
  positionCursor(cursor);
  const { context, anchors } = await legacy(reader, reference, execution);
  key(cursor, context.revision, anchors.map((a) => a.position));
  const anchor = anchors.find((a) =>
    a.position === cursor.position && a.target_ref === message &&
    a.properties.item_kind === "message"
  );
  if (
    !anchor || !await reader.graph.edge(reference, "timeline_message", message)
  ) return null;
  const record = await reader.message(reference, message);
  reader.take();
  return record;
}

/** Retained content stays independent of a newer streaming presentation. */
export async function selectionContentMessage(
  reader: SelectionReader,
  reference: string,
  message: string,
  execution: boolean,
  request: ReadSelectionRequest,
) {
  const facts = await historyRead(reader.graph, {
    scope: request.scope,
    conversation_ref: reference,
    selection: { kind: "archive_message", message_ref: message },
    expected_commit_revision: request.expected_commit_revision,
    participant_actor_ref: null,
    include_execution: execution,
    deadline_unix_ms: request.deadline_unix_ms,
    max_bytes: request.max_bytes,
  });
  if (!facts) return null;
  const item = facts.items[0];
  if (!item?.record) {
    throw new Error("retained content record exceeds wire budget");
  }
  let occurred: number | null = null, best = Infinity;
  for (
    const link of await reader.incoming(message, "field:event.message_ref")
  ) {
    const node = await reader.node(link.source_ref),
      membership = await reader.graph.edge(
        reference,
        "activity",
        link.source_ref,
      );
    if (node?.kind !== "activity" || !membership) continue;
    const body = node.properties.record as {
      occurred_at_unix_ms?: number;
      event?: { kind?: string; message_ref?: string };
    };
    if (
      body.event?.kind === "message_recorded" &&
      body.event.message_ref === message && membership.position < best
    ) {
      best = membership.position;
      occurred = Number.isSafeInteger(body.occurred_at_unix_ms)
        ? body.occurred_at_unix_ms!
        : null;
    }
  }
  reader.take();
  return {
    position: item.position,
    record: item.record,
    occurred_at_unix_ms: occurred,
  };
}
