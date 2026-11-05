import { rootRef, schema } from "./conversation_graph.ts";
import {
  integerOrNull,
  ResourceReader,
  stringOrNull,
} from "./conversation_resource.ts";
import { MessageFlagReader } from "./conversation_page_index.ts";
import type {
  GraphRecord,
  Relationship,
  ResourceFacts,
} from "./conversation_types.ts";
export class PageReader extends ResourceReader {
  // All lookups share one storage transaction. Cache immutable graph facts so
  // revision calculation and page selection do not reread the same branches.
  private readonly edgeFacts = new Map<string, Promise<Relationship[]>>();
  private readonly nodeFacts = new Map<string, Promise<GraphRecord | null>>();
  override async edges(source: string, kind: string): Promise<Relationship[]> {
    const identity = JSON.stringify([source, kind]);
    let result = this.edgeFacts.get(identity);
    if (!result) {
      result = super.edges(source, kind);
      this.edgeFacts.set(identity, result);
    }
    return await result;
  }
  override async node(reference: string): Promise<GraphRecord | null> {
    let result = this.nodeFacts.get(reference);
    if (!result) {
      result = super.node(reference);
      this.nodeFacts.set(reference, result);
    }
    return await result;
  }

  readonly flags = new MessageFlagReader(this.graph);
  async context(reference: string): Promise<PageContext | null> {
    const metadata = await this.graph.ensure();
    if (
      !await this.graph.edge(
        rootRef(this.graph.scope),
        "conversation",
        reference,
      )
    ) return null;
    const header = await this.node(reference);
    if (header?.kind !== "conversation") return null;
    const root = await this.one(reference, "conversation_timeline_root");
    const retained = Math.max(
      integerOrNull(header.properties.commit_revision) ?? 0,
      integerOrNull(header.properties.timeline_revision) ?? 0,
    );
    const revision = root
      ? `${await this.flatRevision(root)}:${retained}`
      : String(retained);
    const commit = integerOrNull(metadata.properties.commit_revision);
    if (commit === null) throw new Error("missing commit revision");
    return {
      root,
      revision,
      commit_revision: String(commit),
      conversation: {
        owner_context_ref: stringOrNull(header.properties.owner_context_ref),
        parent_ref: await this.one(reference, "parent"),
        sidebar_eligible:
          typeof header.properties.sidebar_eligible === "boolean"
            ? header.properties.sidebar_eligible
            : true,
        root: !!await this.graph.edge(
          rootRef(this.graph.scope),
          "root_conversation",
          reference,
        ),
        participant_actor_refs: (await this.edges(reference, "participant"))
          .map((e) => e.target_ref),
      },
    };
  }
  async flatRevision(root: string): Promise<string> {
    const queue = [root], seen = new Set<string>();
    const nodes: { reference: string; sequence: number; revision: number }[] =
      [];
    while (queue.length) {
      const reference = queue.pop()!;
      if (seen.has(reference)) continue;
      seen.add(reference);
      const node = await this.node(reference);
      if (node?.kind !== "conversation_timeline_connection") {
        throw new Error("conversation timeline root is unavailable");
      }
      const sequence = integerOrNull(node.properties.store_sequence),
        revision = integerOrNull(node.properties.revision);
      if (
        sequence === null || revision === null || sequence < 0 || revision < 0
      ) throw new Error("timeline connection order migration required");
      nodes.push({ reference, sequence, revision });
      for (
        const item of await this.edges(reference, "conversation_timeline_item")
      ) {
        for (
          const child of await this.edges(
            item.target_ref,
            "conversation_timeline_child",
          )
        ) queue.push(child.target_ref);
      }
    }
    nodes.sort((a, b) => a.sequence - b.sequence);
    if (
      nodes[0]?.reference !== root ||
      nodes.some((node, i) => i > 0 && node.sequence === nodes[i - 1].sequence)
    ) throw new Error("timeline connection order migration required");
    const encoder = new TextEncoder(),
      prefix = encoder.encode("cwr-canonical-flat-timeline-v1\0"),
      chunks: Uint8Array[] = [prefix];
    for (const node of nodes) {
      const ref = encoder.encode(node.reference),
        size = new Uint8Array(8),
        revision = new Uint8Array(8);
      new DataView(size.buffer).setBigUint64(0, BigInt(ref.length));
      new DataView(revision.buffer).setBigUint64(0, BigInt(node.revision));
      chunks.push(size, ref, revision);
    }
    const bytes = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.length;
    }
    const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
    return `canonical:v1:${
      Array.from(hash, (n) => n.toString(16).padStart(2, "0")).join("")
    }`;
  }
  async keys(
    conversation: string,
    root: string | null,
    includeProgress = false,
  ): Promise<Key[]> {
    if (!root) {
      const result: Key[] = [];
      for (const edge of await this.edges(conversation, "message")) {
        const flags = await this.flags.get(edge.target_ref);
        if (!flags || (!includeProgress && flags.progress)) continue;
        if (flags.schema !== schema("message")) {
          throw new Error("unsupported message schema");
        }
        result.push({ position: edge.position, reference: edge.target_ref });
      }
      return result;
    }
    const keys = new Map<string, Key>(), path = new Set<string>();
    let position = 0, occurrences = 0;
    const walk = async (connection: string, depth: number): Promise<void> => {
      if (depth >= 128 || path.has(connection)) {
        throw new Error("conversation content walk work limit exceeded");
      }
      path.add(connection);
      for (
        const item of await this.edges(connection, "conversation_timeline_item")
      ) {
        // Reused subtrees can expand many times while their physical facts
        // remain cached. Bound occurrence work independently of storage reads.
        if (++occurrences > 50000) {
          throw new Error("conversation content walk work limit exceeded");
        }
        const messages = await this.edges(
          item.target_ref,
          "conversation_timeline_message",
        );
        const flags = await Promise.all(
          messages.map((e) => this.flags.get(e.target_ref)),
        );
        if (!flags.some((f) => f?.streaming)) {
          for (let i = 0; i < messages.length; i++) {
            const ref = messages[i].target_ref, f = flags[i];
            if (
              !keys.has(ref) && f && (includeProgress || !f.progress) &&
              await this.graph.edge(conversation, "message", ref)
            ) {
              if (f.schema !== schema("message")) {
                throw new Error("unsupported message schema");
              }
              keys.set(ref, { position, reference: ref });
            }
          }
          position++;
        }
        // Descendants remain in the flat relation even if their parent streams.
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
    return Array.from(keys.values()).sort((a, b) => a.position - b.position);
  }
  async visibleInvocations(root: string): Promise<Set<string>> {
    const result = new Set<string>(), queue = [root], seen = new Set<string>();
    while (queue.length) {
      const ref = queue.pop()!;
      if (seen.has(ref)) continue;
      seen.add(ref);
      this.take();
      for (const item of await this.edges(ref, "conversation_timeline_item")) {
        for (
          const edge of await this.edges(
            item.target_ref,
            "conversation_timeline_invocation",
          )
        ) result.add(edge.target_ref);
        for (
          const child of await this.edges(
            item.target_ref,
            "conversation_timeline_child",
          )
        ) queue.push(child.target_ref);
      }
    }
    return result;
  }
}
export interface Key {
  position: number;
  reference: string;
}
export interface PageContext {
  root: string | null;
  revision: string;
  commit_revision: string;
  conversation: ResourceFacts["conversation"];
}
