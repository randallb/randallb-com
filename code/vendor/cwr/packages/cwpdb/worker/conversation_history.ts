import { digest } from "./engine.ts";
import { Graph, rootRef } from "./conversation_graph.ts";
import { historyFlags } from "./conversation_history_index.ts";
import type { HistoryStatus } from "./conversation_types.ts";
export const membershipKinds = [
  "conversation",
  "parent",
  "participant",
  "archived",
  "message",
  "activity",
  "transfer_metadata",
  "admission",
  "wait",
  "execution",
  "participant_version",
  "conversation_timeline_root",
  "conversation_timeline_item",
  "conversation_timeline_child",
  "conversation_timeline_message",
  "conversation_timeline_invocation",
  "tool_attempt",
  "tool_operation",
  "tool_file",
  "child_invocation",
  "upstream_declaration",
] as const;
export async function historyStatus(
  graph: Graph,
  reference: string,
): Promise<HistoryStatus | null> {
  const root = await graph.ensure();
  const node = await graph.get(reference);
  if (node?.kind !== "conversation") return null;
  if (
    node.schema_ref !==
      "cwp://org.coworkerprotocol/cwpdb/schemas/conversation/2026-09"
  ) throw new Error("unsupported conversation schema");
  const index = ["edges", reference, "conversation_timeline_root"];
  const count = await graph.count(index);
  if (count > 50000) throw new Error("history root lookup work limit exceeded");
  let has_timeline_root = false;
  for (let slot = 1; slot <= count; slot++) {
    const target = await graph.entry(index, slot);
    if (
      target &&
      await graph.edge(reference, "conversation_timeline_root", target)
    ) {
      has_timeline_root = true;
      break;
    }
  }
  return {
    has_timeline_root,
    ...await historyFlags(graph, reference),
    revision: await digest([
      rootRef(graph.scope),
      root.properties.commit_revision,
    ]),
  };
}
export async function containsMember(
  graph: Graph,
  source: string,
  kind: string,
  target: string,
): Promise<boolean> {
  if (!membershipKinds.some((value) => value === kind)) {
    throw new Error("unsupported CWR membership kind");
  }
  await graph.ensure();
  const edge = await graph.edge(source, kind, target);
  if (!edge) return false;
  if (edge.position < 0) {
    throw new Error("invalid retained CWR membership position");
  }
  const identity = await graph.tx.get<{ schema: string }>(
    `identity:${edge.reference}`,
  );
  if (
    identity?.schema !==
      "cwp://org.coworkerprotocol/cwpdb/schemas/relationship/2026-09"
  ) throw new Error("invalid retained CWR membership schema");
  if (kind === "message" || kind === "activity") {
    return (await graph.get(target))?.kind === kind;
  }
  return true;
}
