import { type Authorizer, canonical } from "./engine.ts";
import { Graph, rootRef, schema } from "./conversation_graph.ts";
import {
  type GraphRecord,
  MAX_WIRE_BYTES,
  object,
  type ObjectValue,
  type Relationship,
  type ResourceFacts,
} from "./conversation_types.ts";
export class ResourceReader {
  private work = 0;
  constructor(readonly graph: Graph) {}
  take() {
    if (++this.work > 50000) {
      throw new Error("resource relationship work limit exceeded");
    }
  }
  async edges(source: string, kind: string): Promise<Relationship[]> {
    const result: Relationship[] = [], index = ["edges", source, kind];
    for (
      let slot = 1, end = await this.graph.count(index);
      slot <= end;
      slot++
    ) {
      this.take();
      const target = await this.graph.entry(index, slot);
      if (target) {
        const edge = await this.graph.edge(source, kind, target);
        if (edge) result.push(edge);
      }
    }
    return result.sort((a, b) =>
      a.position - b.position || compare(a.target_ref, b.target_ref)
    );
  }
  async incident(reference: string): Promise<Relationship[]> {
    const result: Relationship[] = [], index = ["incident", reference];
    for (
      let slot = 1, end = await this.graph.count(index);
      slot <= end;
      slot++
    ) {
      this.take();
      const tuple = await this.graph.entry(index, slot);
      if (tuple) {
        const [source, kind, target] = JSON.parse(tuple) as string[];
        const edge = await this.graph.edge(source, kind, target);
        if (edge) result.push(edge);
      }
    }
    return result;
  }
  async one(source: string, kind: string): Promise<string | null> {
    const edges = await this.edges(source, kind);
    if (edges.length > 1) {
      throw new Error(`singleton relationship ${kind} has multiple members`);
    }
    return edges[0]?.target_ref ?? null;
  }
  async node(reference: string): Promise<GraphRecord | null> {
    this.take();
    const node = await this.graph.get(reference);
    if (node && node.schema_ref !== schema(node.kind)) {
      throw new Error("unsupported resource schema");
    }
    return node;
  }
  async incoming(reference: string, kind: string): Promise<Relationship[]> {
    return (await this.incident(reference)).filter((e) =>
      e.target_ref === reference && e.kind === kind
    );
  }
  async visible(
    root: string | null,
    reference: string,
    kind: string,
  ): Promise<boolean> {
    if (root === null) return true;
    const placement = await this.directRootOrPlacements(root, reference, kind);
    if (placement.direct) return true;
    const queue: { connection: string; depth: number }[] = [],
      seen = new Set<string>();
    const place = async (item: string, depth: number) => {
      for (const e of await this.incoming(item, "conversation_timeline_item")) {
        queue.push({ connection: e.source_ref, depth });
      }
    };
    for (const e of placement.placements) {
      await place(e.source_ref, 0);
    }
    let found = false;
    while (queue.length) {
      const { connection, depth } = queue.pop()!;
      // Traverse the bounded snapshot completely, even after finding the root.
      if (depth >= 128) {
        throw new Error("resource visibility work limit exceeded");
      }
      const identity = `${depth}:${connection}`;
      if (seen.has(identity)) continue;
      seen.add(identity);
      this.take();
      if (connection === root) found = true;
      for (
        const e of await this.incoming(
          connection,
          "conversation_timeline_child",
        )
      ) await place(e.source_ref, depth + 1);
    }
    return found;
  }
  private async directRootOrPlacements(
    root: string,
    reference: string,
    kind: string,
  ): Promise<{ direct: boolean; placements: Relationship[] }> {
    const placements: Relationship[] = [], index = ["incident", reference];
    for (
      let slot = 1, end = await this.graph.count(index);
      slot <= end;
      slot++
    ) {
      this.take();
      const tuple = await this.graph.entry(index, slot);
      if (!tuple) continue;
      const [source, relation, target] = JSON.parse(tuple) as string[];
      if (target !== reference || relation !== kind) continue;
      const edge = await this.graph.edge(source, relation, target);
      if (!edge) continue;
      if (
        await this.graph.edge(root, "conversation_timeline_item", source)
      ) return { direct: true, placements: [] };
      placements.push(edge);
    }
    return { direct: false, placements };
  }
  async message(
    conversation: string,
    reference: string,
  ): Promise<ObjectValue | null> {
    const node = await this.node(reference);
    if (node?.kind !== "message") return null;
    const membership = await this.graph.edge(
      conversation,
      "message",
      reference,
    );
    if (!membership) return null;
    const record = { ...object(node.properties.record) };
    if (Object.hasOwn(membership.properties, "visualizations")) {
      record.visualizations = membership.properties.visualizations;
    }
    return record;
  }
  async activities(
    execution: string,
  ): Promise<{ edge: Relationship; record: ObjectValue }[]> {
    const result = [];
    for (const edge of await this.edges(execution, "activity")) {
      const node = await this.node(edge.target_ref);
      if (node?.kind === "activity") {
        result.push({ edge, record: object(node.properties.record) });
      }
    }
    return result;
  }
  async facts(
    reference: string,
    authorize: Authorizer,
  ): Promise<ResourceFacts | null> {
    const metadata = await this.graph.ensure(),
      node = await this.node(reference);
    if (!node || (node.kind !== "message" && node.kind !== "invocation")) {
      return null;
    }
    let conversation: string,
      streaming = false,
      streamRecord: ObjectValue | null = null;
    if (node.kind === "invocation") {
      const body = object(node.properties.record);
      if (typeof body.conversation_ref !== "string") return null;
      conversation = body.conversation_ref;
    } else if (isObject(node.properties.streaming)) {
      const stream = node.properties.streaming;
      if (typeof stream.conversation_ref !== "string") {
        throw new Error("streaming conversation ref required");
      }
      streamRecord = object(stream.message);
      if (streamRecord.message_ref !== reference) {
        throw new Error("streaming message identity mismatch");
      }
      conversation = stream.conversation_ref;
      streaming = true;
    } else {
      const sources = (await this.incoming(reference, "message")).map((e) =>
        e.source_ref
      ).sort(compare);
      if (sources.length > 1) {
        throw new Error(
          "canonical message ref belongs to multiple conversations",
        );
      }
      if (!sources.length) return null;
      conversation = sources[0];
    }
    await authorize(this.graph.scope.context, conversation, false);
    if (
      !await this.graph.edge(
        rootRef(this.graph.scope),
        "conversation",
        conversation,
      )
    ) return null;
    const header = await this.node(conversation);
    if (header?.kind !== "conversation") return null;
    const root = await this.one(conversation, "conversation_timeline_root");
    if (streaming && root === null) return null;
    if (
      root &&
      (await this.node(root))?.kind !== "conversation_timeline_connection"
    ) throw new Error("resource timeline root has invalid kind or schema");
    const kind = node.kind === "invocation"
      ? "conversation_timeline_invocation"
      : "conversation_timeline_message";
    if (!await this.visible(root, reference, kind)) return null;
    const scopeFacts = {
      owner_context_ref: stringOrNull(header.properties.owner_context_ref),
      parent_ref: await this.one(conversation, "parent"),
      sidebar_eligible: typeof header.properties.sidebar_eligible === "boolean"
        ? header.properties.sidebar_eligible
        : true,
      root: !!await this.graph.edge(
        rootRef(this.graph.scope),
        "root_conversation",
        conversation,
      ),
      participant_actor_refs: (await this.edges(conversation, "participant"))
        .map((e) => e.target_ref),
    };
    let resource: ResourceFacts["resource"];
    if (node.kind === "invocation") {
      resource = await invocationFacts(this, conversation, node, root);
    } else {
      const record = streaming
        ? streamRecord
        : await this.message(conversation, reference);
      if (!record) return null;
      let occurred: number | null = null, best = Infinity;
      if (!streaming) {
        for (
          const link of await this.incoming(
            reference,
            "field:event.message_ref",
          )
        ) {
          const activity = await this.node(link.source_ref),
            membership = await this.graph.edge(
              conversation,
              "activity",
              link.source_ref,
            );
          if (activity?.kind !== "activity" || !membership) continue;
          const body = object(activity.properties.record),
            event = object(body.event);
          if (
            event.kind === "message_recorded" &&
            event.message_ref === reference && membership.position < best
          ) {
            best = membership.position;
            occurred = integerOrNull(body.occurred_at_unix_ms);
          }
        }
      }
      resource = {
        kind: "message",
        record,
        streaming,
        occurred_at_unix_ms: streaming ? null : occurred,
      };
    }
    const revision = integerOrNull(metadata.properties.commit_revision);
    if (revision === null) throw new Error("missing commit revision");
    const result: ResourceFacts = {
      runtime_root_ref: this.graph.scope.runtime_root_ref,
      conversation_ref: conversation,
      commit_revision: String(revision),
      conversation: scopeFacts,
      resource,
    };
    if (new TextEncoder().encode(canonical(result)).length > MAX_WIRE_BYTES) {
      throw new Error("resource facts exceed wire limit");
    }
    await authorize(this.graph.scope.context, conversation, false);
    return result;
  }
}
export function compare(a: string, b: string): number {
  return a < b ? -1 : a > b ? 1 : 0;
}
export function isObject(value: unknown): value is ObjectValue {
  return !!value && typeof value === "object" && !Array.isArray(value);
}
export function stringOrNull(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}
export function integerOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value)
    ? value
    : null;
}
import { invocationFacts } from "./conversation_resource_invocation.ts";
