import { rootRef } from "./conversation_graph.ts";
import {
  compare,
  integerOrNull,
  stringOrNull,
} from "./conversation_resource.ts";
import type { GraphRecord } from "./conversation_types.ts";
import type { SelectionReader } from "./read_selection_reader.ts";
import { SelectionTimeline } from "./read_selection_timeline.ts";
export interface SummaryWindowKey {
  creation_order: number;
  conversation_ref: string;
}
export interface SummaryWindowRequest {
  participant_actor_ref: string | null;
  archived: boolean | null;
  owner_context_filter: [string, boolean] | null;
  after: SummaryWindowKey | null;
  before: SummaryWindowKey | null;
  first: number | null;
  last: number | null;
}
interface Root {
  node: GraphRecord;
  key: SummaryWindowKey;
  archived: boolean;
}
class Hash {
  chunks: Uint8Array[] = [];
  constructor(seed: string) {
    this.chunks.push(new TextEncoder().encode(seed));
  }
  integer(value: number) {
    if (!Number.isSafeInteger(value) || value < 0) {
      throw new Error("invalid summary revision integer");
    }
    const bytes = new Uint8Array(8);
    new DataView(bytes.buffer).setBigUint64(0, BigInt(value));
    this.chunks.push(bytes);
  }
  string(value: string) {
    const bytes = new TextEncoder().encode(value);
    this.integer(bytes.length);
    this.chunks.push(bytes);
  }
  boolean(value: boolean) {
    this.chunks.push(new Uint8Array([value ? 1 : 0]));
  }
  async finish() {
    const bytes = new Uint8Array(
      this.chunks.reduce((size, part) => size + part.length, 0),
    );
    let offset = 0;
    for (const part of this.chunks) {
      bytes.set(part, offset);
      offset += part.length;
    }
    return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
      .map((n) => n.toString(16).padStart(2, "0")).join("");
  }
}
const cmp = (a: SummaryWindowKey, b: SummaryWindowKey) =>
  a.creation_order - b.creation_order ||
  compare(a.conversation_ref, b.conversation_ref);
export class SelectionSummary {
  readonly timeline: SelectionTimeline;
  constructor(readonly reader: SelectionReader) {
    this.timeline = new SelectionTimeline(reader);
  }
  async roots(
    request: Pick<
      SummaryWindowRequest,
      "participant_actor_ref" | "archived" | "owner_context_filter"
    >,
  ): Promise<Root[]> {
    const rows: Root[] = [];
    for (
      const edge of await this.reader.edges(
        rootRef(this.reader.graph.scope),
        "root_conversation",
      )
    ) {
      const node = await this.reader.node(edge.target_ref);
      if (
        node?.kind !== "conversation" ||
        node.properties.sidebar_eligible === false
      ) continue;
      const archived =
        (await this.reader.edges(node.reference, "archived")).length > 0;
      if (request.archived !== null && request.archived !== archived) continue;
      if (
        request.participant_actor_ref !== null &&
        !(await this.reader.edges(node.reference, "participant")).some((p) =>
          p.target_ref === request.participant_actor_ref
        )
      ) continue;
      const owner = request.owner_context_filter;
      if (owner) {
        const actual = stringOrNull(node.properties.owner_context_ref);
        if (
          owner[0] === "" && owner[1]
            ? actual !== null
            : actual !== owner[0] && !(owner[1] && actual === null)
        ) continue;
      }
      rows.push({
        node,
        key: {
          creation_order: edge.position,
          conversation_ref: node.reference,
        },
        archived,
      });
    }
    return rows.sort((a, b) => cmp(b.key, a.key));
  }
  async connections(root: string) {
    const pending = [root], seen = new Set<string>(), rows: GraphRecord[] = [];
    while (pending.length) {
      const connection = pending.pop()!;
      if (seen.has(connection)) continue;
      seen.add(connection);
      this.reader.take();
      const node = await this.reader.node(connection);
      if (node?.kind === "conversation_timeline_connection") rows.push(node);
      for (
        const item of await this.reader.edges(
          connection,
          "conversation_timeline_item",
        )
      ) {
        for (
          const child of await this.reader.edges(
            item.target_ref,
            "conversation_timeline_child",
          )
        ) pending.push(child.target_ref);
      }
    }
    const ordinal = new Map<string, number>();
    for (const row of rows) {
      ordinal.set(
        row.reference,
        await this.reader.graph.tx.get<number>(`ordinal:${row.reference}`) ?? 0,
      );
    }
    rows.sort((a, b) =>
      Number(a.properties.store_sequence ?? -1) -
        Number(b.properties.store_sequence ?? -1) ||
      ordinal.get(a.reference)! - ordinal.get(b.reference)!
    );
    return rows;
  }
  async flatRevision(root: string) {
    const rows = await this.connections(root);
    if (!rows.length || rows[0].reference !== root) {
      throw new Error("conversation timeline root is unavailable");
    }
    const hash = new Hash("cwr-canonical-flat-timeline-v1\0");
    for (const row of rows) {
      hash.string(row.reference);
      hash.integer(Number(row.properties.revision));
    }
    return `canonical:v1:${await hash.finish()}`;
  }
  async latestMessageRef(conversation: string, root: string) {
    const path = new Set<string>();
    const walk = async (
      connection: string,
      depth: number,
    ): Promise<string | null> => {
      if (path.has(connection) || depth >= 128) {
        throw new Error("conversation content walk work limit exceeded");
      }
      path.add(connection);
      for (
        const item
          of (await this.reader.edges(connection, "conversation_timeline_item"))
            .reverse()
      ) {
        for (
          const child of (await this.reader.edges(
            item.target_ref,
            "conversation_timeline_child",
          )).reverse()
        ) {
          const found = await walk(child.target_ref, depth + 1);
          if (found) {
            path.delete(connection);
            return found;
          }
        }
        for (
          const content of await this.reader.edges(
            item.target_ref,
            "conversation_timeline_message",
          )
        ) {
          if ((await this.timeline.flags.get(content.target_ref))?.streaming) {
            continue;
          }
          if (
            await this.reader.graph.edge(
              conversation,
              "message",
              content.target_ref,
            )
          ) {
            path.delete(connection);
            return content.target_ref;
          }
        }
      }
      path.delete(connection);
      return null;
    };
    return await walk(root, 0);
  }
  async row(root: Root) {
    const props = root.node.properties, reference = root.node.reference;
    const canonical = await this.reader.one(
      reference,
      "conversation_timeline_root",
    );
    let completed = (await this.reader.edges(reference, "completed_invocation"))
      .reverse();
    if (canonical) {
      const visible = [];
      for (const edge of completed) {
        if (
          await this.reader.visible(
            canonical,
            edge.target_ref,
            "conversation_timeline_invocation",
          )
        ) visible.push(edge);
        if (visible.length === 500) break;
      }
      completed = visible;
    } else completed = completed.slice(0, 1);
    completed.reverse();
    const completedWork = completed.slice(-1).map((edge) => ({
      invocation_ref: edge.target_ref,
      sequence: edge.position,
    }));
    const completedRefs = canonical
      ? completed.map((edge) => edge.target_ref)
      : Array.isArray(props.completed_work_refs)
      ? props.completed_work_refs
      : [];
    const revision = String(
      Math.max(
        integerOrNull(props.commit_revision) ?? 0,
        integerOrNull(props.timeline_revision) ?? 0,
      ),
    );
    return {
      canonical_preview: canonical !== null,
      key: root.key,
      title: stringOrNull(props.title),
      archived: root.archived,
      last_message_ref: canonical
        ? await this.latestMessageRef(reference, canonical)
        : stringOrNull(props.last_message_ref),
      last_message_preview: canonical
        ? null
        : stringOrNull(props.last_message_preview),
      ...(completedWork.length ? { completed_work: completedWork } : {}),
      ...(completedRefs.length ? { completed_work_refs: completedRefs } : {}),
      revision: canonical
        ? `${await this.flatRevision(canonical)}:${revision}`
        : revision,
    };
  }
  async one(reference: string, workspace: [string | null, string] | null) {
    const request = {
      participant_actor_ref: workspace?.[1] ?? null,
      archived: null,
      owner_context_filter: workspace
        ? [workspace[0] ?? "", true] as [string, boolean]
        : null,
    };
    const root = (await this.roots(request)).find((root) =>
      root.node.reference === reference
    );
    return root ? await this.row(root) : null;
  }
  async window(request: SummaryWindowRequest) {
    request = Object.assign({
      participant_actor_ref: null,
      archived: null,
      owner_context_filter: null,
      after: null,
      before: null,
      first: null,
      last: null,
    }, request);
    if (request.first === null && request.last === null) {
      throw new Error("conversation page requires first or last");
    }
    for (const size of [request.first, request.last]) {
      if (
        size !== null && (!Number.isSafeInteger(size) || size < 0 || size > 200)
      ) throw new Error("conversation page size must not exceed 200");
    }
    const roots = await this.roots(request);
    for (const key of [request.after, request.before]) {
      if (key && !roots.some((root) => cmp(root.key, key) === 0)) {
        throw new Error(
          "conversation page boundary is not in the root conversation relation",
        );
      }
    }
    if (
      request.after && request.before && cmp(request.after, request.before) <= 0
    ) {
      throw new Error(
        "conversation page boundaries do not define a forward range",
      );
    }
    const range = roots.filter((root) =>
      (!request.after || cmp(root.key, request.after) < 0) &&
      (!request.before || cmp(root.key, request.before) > 0)
    );
    let selected = request.first !== null
      ? range.slice(0, Math.max(request.first, request.last ?? 0) + 1)
      : range.slice(-(request.last! + 1)).reverse();
    let has_next_page = request.first !== null &&
      selected.length > request.first;
    let has_previous_page: boolean;
    if (request.first !== null) {
      selected = selected.slice(0, request.first);
      has_previous_page = request.last !== null &&
        selected.length > request.last;
    } else {
      has_previous_page = selected.length > request.last!;
      selected = selected.slice(0, request.last!).reverse();
    }
    if (request.last !== null && selected.length > request.last) {
      selected = request.last === 0 ? [] : selected.slice(-request.last);
    }
    if (request.last === null) {
      has_previous_page = !!selected.length &&
        roots.some((root) =>
          cmp(root.key, selected[0].key) > 0 &&
          (!request.after || cmp(root.key, request.after) < 0)
        );
    }
    if (request.first === null) {
      has_next_page = !!selected.length &&
        roots.some((root) =>
          cmp(root.key, selected.at(-1)!.key) < 0 &&
          (!request.before || cmp(root.key, request.before) > 0)
        );
    }
    const membership = new Hash("cwr-summary-membership-v1\0");
    for (const root of roots) {
      membership.string(root.node.reference);
      membership.boolean(root.archived);
    }
    const metadata = (await this.reader.graph.ensure()).properties,
      base = `${metadata.summary_membership_revision ?? 0}:${
        metadata.completion_order ?? 0
      }`;
    const connections: { conversation: string; node: GraphRecord }[] = [];
    for (
      const root of roots.slice().sort((a, b) =>
        compare(a.node.reference, b.node.reference)
      )
    ) {
      const canonical = await this.reader.one(
        root.node.reference,
        "conversation_timeline_root",
      );
      if (canonical) {
        for (const node of await this.connections(canonical)) {
          connections.push({ conversation: root.node.reference, node });
        }
      }
    }
    const page = new Hash("cwr-canonical-summary-page-v1\0");
    for (const row of connections) {
      page.string(row.conversation);
      page.string(row.node.reference);
      page.integer(Number(row.node.properties.revision));
    }
    const summaries = [];
    for (const root of selected) summaries.push(await this.row(root));
    return {
      membership_revision: `membership:v1:${await membership.finish()}`,
      revision: connections.length
        ? `${base}:canonical:v1:${await page.finish()}`
        : base,
      summaries,
      has_next_page,
      has_previous_page,
      total_count: roots.length,
      archived_count: roots.filter((root) => root.archived).length,
    };
  }
}
