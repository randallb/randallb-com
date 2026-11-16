/** Lightweight current message predicates. Never includes message text. */
import { canonical } from "./engine.ts";
import type { Graph } from "./conversation_graph.ts";
import type { GraphRecord, Scope } from "./conversation_types.ts";
import { isObject } from "./conversation_resource.ts";
export interface MessageFlags {
  version: 1;
  record_revision: string;
  schema: string;
  progress: boolean;
  streaming: boolean;
}
const key = (scope: Scope, reference: string) =>
  `cwr:${canonical([scope.runtime_root_ref, "page-message-v1", reference])}`;
export async function pageMessageRecord(
  graph: Graph,
  record: GraphRecord,
): Promise<void> {
  if (record.kind !== "message") return;
  const body = isObject(record.properties.record)
    ? record.properties.record
    : {};
  await graph.tx.put<MessageFlags>(key(graph.scope, record.reference), {
    version: 1,
    record_revision: record.revision,
    schema: record.schema_ref,
    progress: Object.hasOwn(body, "progress"),
    streaming: isObject(record.properties.streaming),
  });
}
export async function removePageMessage(
  graph: Graph,
  reference: string,
): Promise<void> {
  await graph.tx.delete(key(graph.scope, reference));
}
export class MessageFlagReader {
  private backfillBytes = 0;
  constructor(private readonly graph: Graph) {}
  async get(reference: string): Promise<MessageFlags | null> {
    let flags = await this.graph.tx.get<MessageFlags>(
      key(this.graph.scope, reference),
    );
    if (flags === undefined) {
      // Retained graphs have no sidecar. One finite backfill is charged against
      // a separate 24MiB body-read budget; later reads use only the sidecar.
      const node = await this.graph.get(reference);
      if (node?.kind !== "message") return null;
      this.backfillBytes +=
        new TextEncoder().encode(JSON.stringify(node)).length;
      if (this.backfillBytes > 24 * 1024 * 1024) {
        throw new Error("message page flag backfill byte limit exceeded");
      }
      await pageMessageRecord(this.graph, node);
      flags = await this.graph.tx.get<MessageFlags>(
        key(this.graph.scope, reference),
      );
    }
    if (
      !flags || flags.version !== 1 || typeof flags.schema !== "string" ||
      typeof flags.record_revision !== "string" ||
      typeof flags.progress !== "boolean" ||
      typeof flags.streaming !== "boolean"
    ) throw new Error("invalid message page flags");
    return flags;
  }
}
