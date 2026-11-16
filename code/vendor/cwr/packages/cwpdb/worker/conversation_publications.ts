import { canonical } from "./engine.ts";
import { derived, rootRef } from "./conversation_graph.ts";
import { object, type Publication } from "./conversation_types.ts";
import { Writer } from "./conversation_write.ts";
/** Semantic anchors and projected work activity are distinct canonical facts. */
export async function publish(
  writer: Writer,
  publication: Publication,
): Promise<boolean> {
  const graph = writer.graph;
  const conversation = publication.conversation_ref;
  if ((await graph.get(conversation))?.kind !== "conversation") {
    throw new Error("timeline conversation missing");
  }
  const desired = publication.items.map((item) => ({
    reference: item.kind === "message" ? item.message_ref : item.invocation_ref,
    kind: item.kind,
  }));
  if (new Set(desired.map((item) => item.reference)).size !== desired.length) {
    throw new Error("duplicate timeline publication");
  }
  const old = (await graph.edges(conversation, "timeline_item")).filter((
    edge,
  ) => edge.position >= publication.from_position);
  let changed = canonical(
    old.map((edge) => ({
      reference: edge.target_ref,
      kind: edge.properties.item_kind,
    })),
  ) !== canonical(desired);
  const anchorsChanged = changed;
  for (const edge of old) {
    if (!desired.some((item) => item.reference === edge.target_ref)) {
      await graph.removeEdge(conversation, "timeline_item", edge.target_ref);
      if (edge.properties.item_kind === "message") {
        await graph.removeEdge(
          conversation,
          "timeline_message",
          edge.target_ref,
        );
      }
    }
  }
  for (const [offset, item] of publication.items.entries()) {
    const desiredItem = desired[offset];
    await graph.putEdge(
      conversation,
      "timeline_item",
      desiredItem.reference,
      publication.from_position + offset,
      { item_kind: desiredItem.kind },
    );
    if (item.kind === "message") {
      await graph.putEdge(
        conversation,
        "timeline_message",
        item.message_ref,
        item.position,
        {},
      );
    }
  }
  for (const reference of publication.remove_activities ?? []) {
    changed =
      await graph.removeEdge(conversation, "timeline_activity", reference) ||
      changed;
  }
  for (const activity of publication.activities ?? []) {
    changed = await graph.putEdge(
      conversation,
      "timeline_activity",
      activity.activity_ref,
      activity.position,
      { record: activity.record },
    ) || changed;
    const edgeRef = await derived("timeline_activity", [
      conversation,
      activity.activity_ref,
    ]);
    const event = object(object(activity.record).event);
    for (
      const [kind, field] of [["timeline_invocation", "invocation_ref"], [
        "timeline_source",
        "source_message_ref",
      ], ["timeline_parent", "parent_invocation_ref"]]
    ) {
      const wanted =
        ["work_update", "work_lifecycle"].includes(String(event.kind)) &&
          typeof event[field] === "string"
          ? event[field] as string
          : null;
      // Each activity lookup has one known source. Store that private source so
      // changing it doesn't scan unrelated conversations or execution history.
      const lookupKey = `cwr-lookup:${edgeRef}:${kind}`;
      const previous = await graph.tx.get<string>(lookupKey);
      if (previous && previous !== wanted) {
        await graph.removeEdge(previous, kind, edgeRef);
      }
      if (wanted) {
        await graph.putEdge(wanted, kind, edgeRef, activity.position, {});
        await graph.tx.put(lookupKey, wanted);
      } else await graph.tx.delete(lookupKey);
    }
  }
  const completionsChanged = await refreshCompletions(writer, conversation);
  changed = completionsChanged || changed;
  if (anchorsChanged || completionsChanged) {
    const node = (await graph.get(conversation))!;
    await graph.put(conversation, "conversation", {
      ...node.properties,
      timeline_revision: writer.revision,
    });
  }
  return changed;
}
export async function refreshCompletions(
  writer: Writer,
  conversation: string,
): Promise<boolean> {
  const graph = writer.graph;
  if ((await graph.edges(conversation, "parent")).length) return false;
  let lineage = -1;
  for (const edge of await graph.edges(conversation, "transfer_metadata")) {
    const record = await graph.get(edge.target_ref);
    if (
      record && object(record.properties.record).event &&
      object(object(record.properties.record).event).kind ===
        "conversation_forked_from"
    ) lineage = Math.max(lineage, edge.position);
  }
  for (const edge of await graph.edges(conversation, "activity")) {
    if (edge.position <= lineage) continue;
    const record = await graph.get(edge.target_ref);
    if (!record) continue;
    const event = object(object(record.properties.record).event);
    if (
      event.kind !== "work_lifecycle" || event.state !== "completed" ||
      event.parent_invocation_ref != null ||
      typeof event.invocation_ref !== "string"
    ) continue;
    if (
      !await graph.edge(
        conversation,
        "completed_invocation",
        event.invocation_ref,
      )
    ) {
      await graph.putEdge(
        conversation,
        "completed_invocation",
        event.invocation_ref,
        await writer.increment("completion_order"),
        {},
      );
    }
  }
  const completed = (await graph.edges(conversation, "completed_invocation"))
    .slice(-500).map((edge) => edge.target_ref);
  const node = await graph.get(conversation);
  if (!node) return false;
  if (
    canonical(node.properties.completed_work_refs ?? []) ===
      canonical(completed)
  ) return false;
  return (await graph.put(conversation, "conversation", {
    ...node.properties,
    completed_work_refs: completed,
  })).changed;
}
