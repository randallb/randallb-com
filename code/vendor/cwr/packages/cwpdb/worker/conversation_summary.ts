import { canonical, digest } from "./engine.ts";
import { Graph, rootRef } from "./conversation_graph.ts";
import {
  MAX_WIRE_BYTES,
  object,
  type Relationship,
  type SummaryFacts,
} from "./conversation_types.ts";
class Work {
  private count = 0;
  take() {
    if (++this.count > 50000) {
      throw new Error("conversation summary relationship work limit exceeded");
    }
  }
}
async function edges(
  graph: Graph,
  source: string,
  kind: string,
  work: Work,
): Promise<Relationship[]> {
  const result: Relationship[] = [];
  const index = ["edges", source, kind];
  for (let slot = 1, end = await graph.count(index); slot <= end; slot++) {
    work.take();
    const target = await graph.entry(index, slot);
    if (target) {
      const edge = await graph.edge(source, kind, target);
      if (edge) result.push(edge);
    }
  }
  return result.sort((a, b) =>
    a.position - b.position ||
    (a.target_ref < b.target_ref ? -1 : a.target_ref > b.target_ref ? 1 : 0)
  );
}
async function incident(
  graph: Graph,
  reference: string,
  work: Work,
): Promise<Relationship[]> {
  const result: Relationship[] = [];
  const index = ["incident", reference];
  for (let slot = 1, end = await graph.count(index); slot <= end; slot++) {
    work.take();
    const tuple = await graph.entry(index, slot);
    if (tuple) {
      const [source, kind, target] = JSON.parse(tuple) as string[];
      const edge = await graph.edge(source, kind, target);
      if (edge) result.push(edge);
    }
  }
  return result;
}
async function visibleInvocation(
  graph: Graph,
  root: string,
  reference: string,
  work: Work,
): Promise<boolean> {
  const queue: { connection: string; depth: number }[] = [];
  const addPlacements = async (item: string, depth: number) => {
    for (const placement of await incident(graph, item, work)) {
      if (
        placement.kind === "conversation_timeline_item" &&
        placement.target_ref === item
      ) queue.push({ connection: placement.source_ref, depth });
    }
  };
  for (const content of await incident(graph, reference, work)) {
    if (
      content.kind === "conversation_timeline_invocation" &&
      content.target_ref === reference
    ) await addPlacements(content.source_ref, 0);
  }
  const seen = new Set<string>();
  while (queue.length) {
    const { connection, depth } = queue.pop()!;
    if (connection === root) return true;
    if (depth >= 128) {
      throw new Error("conversation summary timeline depth exceeded");
    }
    if (seen.has(connection)) continue;
    seen.add(connection);
    for (const child of await incident(graph, connection, work)) {
      if (
        child.kind === "conversation_timeline_child" &&
        child.target_ref === connection
      ) await addPlacements(child.source_ref, depth + 1);
    }
  }
  return false;
}
async function latest(
  graph: Graph,
  conversation: string,
  root: string,
  work: Work,
): Promise<{ reference: string; record: Record<string, unknown> } | null> {
  const path = new Set<string>();
  const walk = async (
    connection: string,
    depth: number,
  ): Promise<{ reference: string; record: Record<string, unknown> } | null> => {
    if (depth >= 128 || path.has(connection)) {
      throw new Error("conversation summary timeline depth/cycle exceeded");
    }
    path.add(connection);
    const items = await edges(
      graph,
      connection,
      "conversation_timeline_item",
      work,
    );
    for (const item of items.reverse()) {
      for (
        const child of (await edges(
          graph,
          item.target_ref,
          "conversation_timeline_child",
          work,
        )).reverse()
      ) {
        const found = await walk(child.target_ref, depth + 1);
        if (found) {
          path.delete(connection);
          return found;
        }
      }
      const messages = await edges(
        graph,
        item.target_ref,
        "conversation_timeline_message",
        work,
      );
      const records = await Promise.all(
        messages.map((message) => graph.get(message.target_ref)),
      );
      if (
        records.some((record) =>
          record?.properties.streaming !== null &&
          typeof record?.properties.streaming === "object" &&
          !Array.isArray(record?.properties.streaming)
        )
      ) continue;
      for (let i = messages.length - 1; i >= 0; i--) {
        const node = records[i];
        if (node?.kind !== "message") continue;
        const membership = await graph.edge(
          conversation,
          "message",
          messages[i].target_ref,
        );
        if (!membership) continue;
        const record = { ...object(node.properties.record) };
        if (Object.hasOwn(membership.properties, "visualizations")) {
          record.visualizations = membership.properties.visualizations;
        }
        path.delete(connection);
        return { reference: messages[i].target_ref, record };
      }
    }
    path.delete(connection);
    return null;
  };
  return await walk(root, 0);
}
export async function summary(
  graph: Graph,
  reference: string,
): Promise<SummaryFacts | null> {
  await graph.ensure();
  const node = await graph.get(reference);
  if (node?.kind !== "conversation") return null;
  const work = new Work(), props = node.properties;
  const roots = await edges(
    graph,
    reference,
    "conversation_timeline_root",
    work,
  );
  if (roots.length > 1) {
    throw new Error("conversation has multiple timeline roots");
  }
  const canonicalRoot = roots[0]?.target_ref;
  if (
    canonicalRoot &&
    (await graph.get(canonicalRoot))?.kind !==
      "conversation_timeline_connection"
  ) throw new Error("conversation timeline root is unavailable");
  const rootMembership = await graph.edge(
    rootRef(graph.scope),
    "root_conversation",
    reference,
  );
  const parents = await edges(graph, reference, "parent", work);
  if (parents.length > 1) throw new Error("conversation has multiple parents");
  const selected = canonicalRoot
    ? await latest(graph, reference, canonicalRoot, work)
    : null;
  const completed = [];
  for (
    const completion
      of (await edges(graph, reference, "completed_invocation", work)).reverse()
  ) {
    if (
      canonicalRoot &&
      !await visibleInvocation(
        graph,
        canonicalRoot,
        completion.target_ref,
        work,
      )
    ) continue;
    completed.push({
      invocation_ref: completion.target_ref,
      sequence: completion.position,
    });
    if (completed.length >= (canonicalRoot ? 500 : 1)) break;
  }
  completed.reverse();
  const text = selected?.record.text;
  const facts: SummaryFacts = {
    conversation_ref: reference,
    creation_order: rootMembership?.position ??
      (typeof props.creation_order === "number" ? props.creation_order : 0),
    title: typeof props.title === "string" ? props.title : null,
    archived: (await edges(graph, reference, "archived", work)).length > 0,
    owner_context_ref: typeof props.owner_context_ref === "string"
      ? props.owner_context_ref
      : null,
    parent_ref: parents[0]?.target_ref ?? null,
    sidebar_eligible: typeof props.sidebar_eligible === "boolean"
      ? props.sidebar_eligible
      : true,
    root: rootMembership !== null,
    participant_actor_refs: (await edges(graph, reference, "participant", work))
      .map((edge) => edge.target_ref),
    last_message_ref: canonicalRoot
      ? selected?.reference ?? null
      : typeof props.last_message_ref === "string"
      ? props.last_message_ref
      : null,
    last_message_preview: canonicalRoot
      ? (selected
        ? Array.from(typeof text === "string" ? text : "").slice(0, 240).join(
          "",
        )
        : null)
      : typeof props.last_message_preview === "string"
      ? props.last_message_preview
      : null,
    last_message_record: selected?.record ?? null,
    completed_work: completed.slice(-1),
    completed_work_refs: canonicalRoot
      ? completed.map((work) => work.invocation_ref)
      : Array.isArray(props.completed_work_refs)
      ? props.completed_work_refs as string[]
      : [],
    revision: "",
  };
  const message = facts.last_message_record
    ? [
      facts.last_message_record.text ?? null,
      facts.last_message_record.visualizations ?? null,
    ]
    : null;
  facts.revision = `summary:v1:${await digest([
    facts.conversation_ref,
    facts.creation_order,
    facts.title,
    facts.archived,
    facts.owner_context_ref,
    facts.parent_ref,
    facts.sidebar_eligible,
    facts.root,
    facts.participant_actor_refs,
    facts.last_message_ref,
    facts.last_message_preview,
    facts.completed_work,
    facts.completed_work_refs,
    message,
  ])}`;
  if (new TextEncoder().encode(canonical(facts)).length > MAX_WIRE_BYTES) {
    throw new Error("conversation summary exceeds24MiB wire budget");
  }
  return facts;
}
