import { assertEquals, assertRejects } from "@std/assert";
import { Graph, key, rootRef, schema } from "./conversation_graph.ts";
import type { Transaction } from "./engine.ts";
import type { GraphRecord, Relationship, Scope } from "./conversation_types.ts";
import { contentPage } from "./conversation_pages.ts";
import { PageReader } from "./conversation_pages_visibility.ts";
const base = "cwp://fixture/page-scale",
  conversation = `${base}/conversation`,
  message = `${base}/message`;
const scope: Scope = {
  context: { workspace_ref: `${base}/workspace`, caller_ref: `${base}/caller` },
  runtime_root_ref: `${base}/runtime`,
};
const conn = (depth: number) => `${base}/connection/${depth}`;
const item = (depth: number, index: number) => `${base}/item/${depth}/${index}`;
class VirtualGraph extends Graph {
  rootItemScans = 0;
  rootItemEntries = 0;
  messageBodies = 0;
  constructor(
    private readonly size: number,
    private readonly dagDepth: number | null = null,
  ) {
    const tx: Transaction = {
      get: async <T>(reference: string): Promise<T | undefined> =>
        reference === key(scope.runtime_root_ref, "page-message-v1", message)
          ? {
            version: 1,
            record_revision: "fixture-revision",
            schema: schema("message"),
            progress: false,
            streaming: false,
          } as T
          : undefined,
      put: async () => {
        throw new Error("unexpected page fixture write");
      },
      delete: async () => {
        throw new Error("unexpected page fixture delete");
      },
    };
    super(tx, scope);
  }
  private record(
    reference: string,
    kind: GraphRecord["kind"],
    properties: Record<string, unknown>,
  ): GraphRecord {
    return {
      reference,
      kind,
      schema_ref: schema(kind),
      properties,
      revision: "fixture-revision",
    };
  }
  override async ensure(): Promise<GraphRecord> {
    return this.record(rootRef(scope), "runtime", { commit_revision: 1 });
  }
  override async get(reference: string): Promise<GraphRecord | null> {
    if (reference === conversation) {
      return this.record(reference, "conversation", { commit_revision: 1 });
    }
    if (reference.startsWith(`${base}/connection/`)) {
      const depth = Number(reference.split("/").at(-1));
      return this.record(reference, "conversation_timeline_connection", {
        revision: 1,
        store_sequence: depth,
      });
    }
    if (reference === message) {
      this.messageBodies++;
      return this.record(reference, "message", {
        record: { message_ref: message, text: "Selected body" },
      });
    }
    return null;
  }
  override async count(index: unknown[]): Promise<number> {
    const [, source, kind] = index;
    if (source === conversation && kind === "conversation_timeline_root") {
      return 1;
    }
    if (
      typeof source === "string" && source.startsWith(`${base}/connection/`) &&
      kind === "conversation_timeline_item"
    ) {
      if (source === conn(0)) this.rootItemScans++;
      return this.dagDepth === null ? this.size : 2;
    }
    if (typeof source === "string" && source.startsWith(`${base}/item/`)) {
      if (kind === "conversation_timeline_message") return 1;
      const depth = Number(source.split("/").at(-2));
      if (
        kind === "conversation_timeline_child" && this.dagDepth !== null &&
        depth < this.dagDepth
      ) return 1;
    }
    return 0;
  }
  override async entry(
    index: unknown[],
    slot: number,
  ): Promise<string | undefined> {
    const [, source, kind] = index;
    if (source === conversation && kind === "conversation_timeline_root") {
      return conn(0);
    }
    if (
      typeof source === "string" && source.startsWith(`${base}/connection/`)
    ) {
      if (source === conn(0)) this.rootItemEntries++;
      return item(Number(source.split("/").at(-1)), slot);
    }
    if (typeof source === "string" && source.startsWith(`${base}/item/`)) {
      if (kind === "conversation_timeline_message") return message;
      if (kind === "conversation_timeline_child") {
        return conn(Number(source.split("/").at(-2)) + 1);
      }
    }
    return undefined;
  }
  override async edge(
    source_ref: string,
    kind: string,
    target_ref: string,
  ): Promise<Relationship | null> {
    return {
      reference: `${base}/edge/${encodeURIComponent(source_ref)}/${kind}/${
        encodeURIComponent(target_ref)
      }`,
      source_ref,
      kind,
      target_ref,
      position: target_ref.startsWith(`${base}/item/`)
        ? Number(target_ref.split("/").at(-1))
        : 0,
      properties: {},
      revision: "fixture-revision",
    };
  }
}
const request = {
  scope,
  conversation_ref: conversation,
  after: null,
  before: null,
  first: 1,
  last: null,
  max_bytes: 1024 * 1024,
};
Deno.test("page snapshot reuses revision metadata for a valid 13000-item graph", async () => {
  const graph = new VirtualGraph(13000);
  const facts = await contentPage(new PageReader(graph), request);
  assertEquals(facts?.total_count, 1);
  assertEquals(facts?.items[0].record.text, "Selected body");
  assertEquals(graph.rootItemScans, 1);
  assertEquals(graph.rootItemEntries, 13000);
  assertEquals(graph.messageBodies, 1);
});
Deno.test("cached repeated subtrees retain an explicit occurrence work refusal", async () => {
  const graph = new VirtualGraph(2, 18);
  await assertRejects(
    () => contentPage(new PageReader(graph), request),
    Error,
    "walk work limit",
  );
  assertEquals(graph.messageBodies, 0);
});
