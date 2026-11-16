import { MAX_WIRE_BYTES, object } from "./conversation_types.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
import { integerOrNull } from "./conversation_resource.ts";
import type {
  CompletionPageFacts,
  CompletionPageRequest,
  ContentPageFacts,
  ContentPageRequest,
} from "./conversation_page_types.ts";
export function pageBudget(max: number): void {
  if (!Number.isSafeInteger(max) || max < 1 || max > MAX_WIRE_BYTES) {
    throw new Error("invalid conversation page byte budget");
  }
}
const bytes = (value: unknown) =>
  new TextEncoder().encode(JSON.stringify(value)).length;
export async function contentPage(
  reader: PageReader,
  request: ContentPageRequest,
): Promise<ContentPageFacts | null> {
  pageBudget(request.max_bytes);
  const { first, last, after, before } = request;
  if (first == null && last == null) {
    throw new Error("conversation record page requires first or last");
  }
  if (
    [first, last].some((n) =>
      n != null && (!Number.isSafeInteger(n) || n < 0 || n > 500)
    )
  ) throw new Error("conversation record page size must not exceed500");
  if (
    [after, before].some((c) => c != null && !Number.isSafeInteger(c.position))
  ) throw new Error("invalid content position cursor");
  if (after && before && after.position >= before.position) {
    throw new Error(
      "conversation record page boundaries do not define a forward range",
    );
  }
  const context = await reader.context(request.conversation_ref);
  if (!context) return null;
  for (const cursor of [after, before]) {
    if (cursor && context.root && cursor.revision !== context.revision) {
      throw new Error(
        "conversation record cursor belongs to an earlier revision",
      );
    }
  }
  const all = await reader.keys(request.conversation_ref, context.root);
  for (const cursor of [after, before]) {
    if (cursor && !all.some((k) => k.position === cursor.position)) {
      throw new Error(
        "conversation record page boundary is not in the requested relation",
      );
    }
  }
  const window = all.filter((k) =>
    (after == null || k.position > after.position) &&
    (before == null || k.position < before.position)
  );
  let keys = window, has_next_page = false, has_previous_page = false;
  if (first != null) {
    has_next_page = keys.length > first;
    keys = keys.slice(0, first);
  }
  if (last != null) {
    has_previous_page = keys.length > last;
    keys = last === 0 ? [] : keys.slice(-last);
  }
  const facts: ContentPageFacts = {
    runtime_root_ref: request.scope.runtime_root_ref,
    conversation_ref: request.conversation_ref,
    commit_revision: context.commit_revision,
    conversation: context.conversation,
    canonical: context.root !== null,
    revision: context.revision,
    items: [],
    has_next_page,
    has_previous_page,
    total_count: all.length,
  };
  let used = bytes(facts);
  if (used > request.max_bytes) {
    throw new Error("conversation content page exceeds byte budget");
  }
  for (const key of keys) {
    const record = await reader.message(
      request.conversation_ref,
      key.reference,
    );
    if (!record) {
      throw new Error("selected content message disappeared from snapshot");
    }
    if (record.message_ref !== key.reference) {
      throw new Error("selected content message identity mismatch");
    }
    let occurred: number | null = null, best = Infinity;
    for (
      const link of await reader.incoming(
        key.reference,
        "field:event.message_ref",
      )
    ) {
      const activity = await reader.node(link.source_ref),
        membership = await reader.graph.edge(
          request.conversation_ref,
          "activity",
          link.source_ref,
        );
      if (activity?.kind !== "activity" || !membership) continue;
      const body = object(activity.properties.record),
        event = object(body.event);
      if (event.kind === "message_recorded" && membership.position < best) {
        best = membership.position;
        occurred = integerOrNull(body.occurred_at_unix_ms);
      }
    }
    const item = {
      position: key.position,
      record,
      occurred_at_unix_ms: occurred,
    };
    used += bytes(item) + (facts.items.length ? 1 : 0);
    if (used > request.max_bytes) {
      throw new Error("conversation content page exceeds byte budget");
    }
    facts.items.push(item);
  }
  if (bytes(facts) > request.max_bytes) {
    throw new Error("conversation content page exceeds byte budget");
  }
  return facts;
}
export async function completionPage(
  reader: PageReader,
  request: CompletionPageRequest,
): Promise<CompletionPageFacts | null> {
  pageBudget(request.max_bytes);
  if (
    !Number.isSafeInteger(request.after_sequence) ||
    request.after_sequence < 0 ||
    (request.through_sequence != null &&
      (!Number.isSafeInteger(request.through_sequence) ||
        request.through_sequence < request.after_sequence)) ||
    !Number.isSafeInteger(request.first) || request.first < 1 ||
    request.first > 200
  ) throw new Error("invalid completion history window");
  const context = await reader.context(request.conversation_ref);
  if (!context) return null;
  if (
    context.root && request.after_sequence > 0 &&
    request.cursor_revision !== context.revision
  ) {
    throw new Error(
      "conversation cursor belongs to an earlier canonical timeline revision",
    );
  }
  const visible = context.root
    ? await reader.visibleInvocations(context.root)
    : null;
  const completions =
    (await reader.edges(request.conversation_ref, "completed_invocation"))
      .filter((e) => !visible || visible.has(e.target_ref));
  const upper = request.through_sequence ??
    completions.reduce((max, e) => Math.max(max, e.position), 0);
  let items = completions.filter((e) =>
    e.position > request.after_sequence && e.position <= upper
  ).slice(0, request.first + 1).map((e) => ({
    invocation_ref: e.target_ref,
    sequence: e.position,
  }));
  let next = items.length > request.first;
  items = items.slice(0, request.first);
  while (bytes(items) > 256 * 1024) {
    items.pop();
    next = true;
  }
  if (next && !items.length) {
    throw new Error("completion reference exceeds response byte limit");
  }
  const result: CompletionPageFacts = {
    runtime_root_ref: request.scope.runtime_root_ref,
    conversation_ref: request.conversation_ref,
    commit_revision: context.commit_revision,
    conversation: context.conversation,
    canonical: context.root !== null,
    revision: context.revision,
    items,
    through_sequence: upper,
    next_after: items.at(-1)?.sequence ?? request.after_sequence,
    has_next_page: next,
  };
  if (bytes(result) > request.max_bytes) {
    throw new Error("conversation completion page exceeds byte budget");
  }
  return result;
}
