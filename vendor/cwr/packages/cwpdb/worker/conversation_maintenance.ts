import { rootRef } from "./conversation_graph.ts";
import { type Mutation, object } from "./conversation_types.ts";
import { Writer } from "./conversation_write.ts";
/** Imported legacy layouts are repaired only after the trusted CWR caller has
 * validated their protocol records and supplied canonical publication deltas. */
export async function repair(
  writer: Writer,
  request: Mutation,
): Promise<boolean> {
  const graph = writer.graph;
  const flags = await graph.maintenanceNeeded();
  if (!flags.length) return false;
  if (flags.includes("timeline_projection")) {
    for (
      const edge of await graph.edges(rootRef(request.scope), "conversation")
    ) {
      if ((await graph.edges(edge.target_ref, "parent")).length) continue;
      if (
        !request.changes.publications.some((publication) =>
          publication.conversation_ref === edge.target_ref &&
          publication.from_position === 0
        )
      ) {
        throw new Error(
          "legacy timeline repair requires canonical root publication suffixes",
        );
      }
    }
  }
  const index = ["maintenance", "visualization_metadata"];
  for (let slot = 1, end = await graph.count(index); slot <= end; slot++) {
    const reference = await graph.entry(index, slot);
    if (
      !reference ||
      !await graph.maintenanceActive("visualization_metadata", reference)
    ) continue;
    const node = await graph.get(reference);
    if (!node || node.kind !== "message") continue;
    const record = structuredClone(object(node.properties.record));
    if ("visualizations" in record) {
      for (const edge of await graph.incident(reference)) {
        if (edge.kind === "message" && edge.target_ref === reference) {
          await graph.putEdge(
            edge.source_ref,
            edge.kind,
            reference,
            edge.position,
            { ...edge.properties, visualizations: record.visualizations },
          );
        }
      }
      delete record.visualizations;
      await writer.record({
        reference,
        kind: "message",
        owner_ref: null,
        properties: { record },
      });
    }
    await graph.maintenanceFlag("visualization_metadata", reference, false);
  }
  await writer.metadata("timeline_projection_version", 1);
  return true;
}
