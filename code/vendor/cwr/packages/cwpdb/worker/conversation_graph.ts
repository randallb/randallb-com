import {
  pageMessageRecord,
  removePageMessage,
} from "./conversation_page_index.ts";
import {
  historyMembership,
  historyRecord,
} from "./conversation_history_index.ts";
import { canonical, digest, type Transaction, validContext } from "./engine.ts";
import { bindIdentity, retainReference } from "./identity.ts";
import {
  type GraphRecord,
  MAX_RECORD_BYTES,
  type ObjectValue,
  type RecordKind,
  recordKinds,
  type Relationship,
  type Scope,
} from "./conversation_types.ts";
export const STATE =
  "cwp://org.coworkerprotocol/cwpdb/schemas/runtime-state/2026-09";
export const TIMELINE_STATE = `${STATE}/conversation-timeline-v1`;
export const schema = (kind: string): string =>
  `cwp://org.coworkerprotocol/cwpdb/schemas/${kind}/2026-09`;
export const rootRef = (scope: Scope): string =>
  `${scope.runtime_root_ref}/cwpdb`;
export function key(...parts: unknown[]): string {
  return `cwr:${canonical(parts)}`;
}
/** Native-compatible deterministic graph identities, with UTF-8 byte lengths. */
export async function derived(kind: string, parts: string[]): Promise<string> {
  const chunks = [kind, ...parts].map((part) => new TextEncoder().encode(part));
  const bytes = new Uint8Array(
    chunks.reduce((sum, chunk) => sum + 8 + chunk.length, 0),
  );
  let offset = 0;
  for (const chunk of chunks) {
    new DataView(bytes.buffer).setBigUint64(offset, BigInt(chunk.length));
    bytes.set(chunk, offset + 8);
    offset += 8 + chunk.length;
  }
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  return `cwp://org.coworkerprotocol/cwpdb/derived/${
    [...hash].map((n) => n.toString(16).padStart(2, "0")).join("")
  }`;
}
interface Stored<T> {
  counter: number;
  value: T;
}
/** Append-only physical index slots keep canonical bodies independent. Updating
 * one message never serializes the growing conversation or its history prefix. */
export class Graph {
  constructor(readonly tx: Transaction, readonly scope: Scope) {}
  private k(...parts: unknown[]): string {
    return key(this.scope.runtime_root_ref, ...parts);
  }
  async ensure(): Promise<GraphRecord> {
    validContext(this.scope.context, this.scope.runtime_root_ref);
    const pinned = await this.tx.get<string>(this.k("workspace"));
    if (pinned && pinned !== this.scope.context.workspace_ref) {
      throw new Error("conversation storage scope mismatch");
    }
    if (!pinned) {
      await this.tx.put(this.k("workspace"), this.scope.context.workspace_ref);
    }
    const root = await this.get(rootRef(this.scope));
    if (root) return root;
    return (await this.put(rootRef(this.scope), "runtime", {
      schema_ref: "cwp://org.coworkerprotocol/cwpdb/schemas/store/2026-09",
      runtime_state_version: STATE,
      runtime_root_ref: this.scope.runtime_root_ref,
      cloud_workspace_ref: this.scope.context.workspace_ref,
      commit_revision: 0,
      timeline_projection_version: 1,
    })).record;
  }
  async indexAdd(index: unknown[], reference: string): Promise<void> {
    const marker = this.k("slot", index, reference);
    if (await this.tx.get<number>(marker) !== undefined) return;
    const next = (await this.tx.get<number>(this.k("count", index)) ?? 0) + 1;
    if (!Number.isSafeInteger(next)) throw new Error("CWR index exhausted");
    await this.tx.put(marker, next);
    await this.tx.put(this.k("entry", index, next), reference);
    await this.tx.put(this.k("count", index), next);
  }
  async count(index: unknown[]): Promise<number> {
    return await this.tx.get<number>(this.k("count", index)) ?? 0;
  }
  async entry(index: unknown[], slot: number): Promise<string | undefined> {
    return await this.tx.get<string>(this.k("entry", index, slot));
  }
  async get(reference: string): Promise<GraphRecord | null> {
    return (await this.tx.get<Stored<GraphRecord>>(this.k("record", reference)))
      ?.value ?? null;
  }
  async put(
    reference: string,
    kind: RecordKind,
    properties: ObjectValue,
  ): Promise<{ record: GraphRecord; changed: boolean }> {
    validContext(this.scope.context, reference);
    if (
      new TextEncoder().encode(canonical(properties)).length > MAX_RECORD_BYTES
    ) throw new Error("CWR record exceeds24MiB wire budget");
    if (!recordKinds.includes(kind)) throw new Error("invalid CWR record kind");
    if (kind !== "runtime") {
      // Internal CWR record kinds cannot occupy an unchanged external actor role.
      const incident = ["incident", reference];
      const incidentCount = await this.count(incident);
      if (incidentCount > 50000) {
        throw new Error("external actor binding work limit exceeded");
      }
      for (let slot = 1; slot <= incidentCount; slot++) {
        const tuple = await this.entry(incident, slot);
        if (!tuple) continue;
        const [source, role, target] = JSON.parse(tuple) as string[];
        if (
          target !== reference ||
          !["participant", "participant_version"].includes(role)
        ) continue;
        if (
          await this.edge(source, role, target) &&
          await this.edge(rootRef(this.scope), "conversation", source)
        ) {
          throw new Error(
            "CWR record conflicts with retained external actor binding",
          );
        }
      }
    }
    if (
      await this.tx.get(`node:${reference}`) ||
      await this.tx.get(`retired:${reference}`)
    ) throw new Error("CWR record schema or kind conflict");
    await bindIdentity(
      this.tx,
      reference,
      `node:${kind}`,
      this.scope.runtime_root_ref,
      schema(kind),
    );
    const old = await this.tx.get<Stored<GraphRecord>>(
      this.k("record", reference),
    );
    if (old && old.value.kind !== kind) {
      throw new Error("CWR record kind conflict");
    }
    const changed = !old ||
      canonical(old.value.properties) !== canonical(properties);
    const counter = old ? old.counter + (changed ? 1 : 0) : 0;
    const record: GraphRecord = {
      reference,
      kind,
      schema_ref: schema(kind),
      properties,
      revision: await digest([reference, counter]),
    };
    if (changed) {
      await this.tx.put(this.k("record", reference), {
        counter,
        value: record,
      });
      await this.indexAdd(["records", kind], reference);
    }
    await pageMessageRecord(this, record);
    if (changed) await historyRecord(this, record);
    return { record, changed };
  }
  async endpointKinds(
    reference: string,
  ): Promise<{ node_kind: string | null; edge_kind: string | null }> {
    const node = await this.get(reference);
    const locator = await this.tx.get<string[] | null>(
      this.k("edge-reference", reference),
    );
    if (locator === undefined) {
      const identity = await this.tx.get<{ kind: string }>(
        `identity:${reference}`,
      );
      if (identity?.kind.startsWith("edge:")) {
        throw new Error("retained edge reference index requires migration");
      }
    }
    const edge = locator
      ? await this.edge(locator[0], locator[1], locator[2])
      : null;
    return { node_kind: node?.kind ?? null, edge_kind: edge?.kind ?? null };
  }
  async edge(
    source: string,
    kind: string,
    target: string,
  ): Promise<Relationship | null> {
    const edge = (await this.tx.get<Stored<Relationship>>(
      this.k("edge", source, kind, target),
    ))?.value ?? null;
    // Exact reads backfill the finite reference index for retained graphs.
    if (edge && !await this.tx.get(this.k("edge-reference", edge.reference))) {
      await this.tx.put(this.k("edge-reference", edge.reference), [
        source,
        kind,
        target,
      ]);
    }
    return edge;
  }
  async putEdge(
    source: string,
    kind: string,
    target: string,
    position: number,
    properties: ObjectValue,
  ): Promise<boolean> {
    validContext(this.scope.context, source);
    validContext(this.scope.context, target);
    await retainReference(this.tx, source);
    await retainReference(this.tx, target);
    const old = await this.tx.get<Stored<Relationship>>(
      this.k("edge", source, kind, target),
    );
    if (
      old && old.value.position === position &&
      canonical(old.value.properties) === canonical(properties)
    ) return false;
    const reference = await derived(kind, [source, target]);
    const counter = old ? old.counter + 1 : 0;
    await bindIdentity(
      this.tx,
      reference,
      `edge:${kind}`,
      this.scope.runtime_root_ref,
      "cwp://org.coworkerprotocol/cwpdb/schemas/relationship/2026-09",
    );
    await this.tx.put(this.k("edge", source, kind, target), {
      counter,
      value: {
        reference,
        source_ref: source,
        kind,
        target_ref: target,
        position,
        properties,
        revision: await digest([reference, counter]),
      },
    });
    await this.tx.put(this.k("edge-reference", reference), [
      source,
      kind,
      target,
    ]);
    await this.indexAdd(["edges", source, kind], target);
    await this.indexAdd(
      ["incident", source],
      canonical([source, kind, target]),
    );
    await this.indexAdd(
      ["incident", target],
      canonical([source, kind, target]),
    );
    await this.indexAdd(["relations", kind], canonical([source, kind, target]));
    if (kind === "message") {
      const first = await this.tx.get<{ reference: string; position: number }>(
        this.k("first-message", source),
      );
      if (!first || position < first.position || first.reference === target) {
        await this.tx.put(this.k("first-message", source), {
          reference: target,
          position,
        });
      }
      await this.indexAdd(["first-messages"], source);
    }
    await historyMembership(this, source, kind, target, true);
    return true;
  }
  async edges(source: string, kind: string): Promise<Relationship[]> {
    const result: Relationship[] = [];
    const index = ["edges", source, kind];
    for (let slot = 1, end = await this.count(index); slot <= end; slot++) {
      const target = await this.entry(index, slot);
      if (target) {
        const edge = await this.edge(source, kind, target);
        if (edge) result.push(edge);
      }
    }
    return result.sort((a, b) =>
      a.position - b.position || a.target_ref.localeCompare(b.target_ref)
    );
  }
  async maintenanceFlag(
    flag: string,
    reference: string,
    active: boolean,
  ): Promise<void> {
    const marker = this.k("maintenance-active", flag, reference);
    const was = await this.tx.get<boolean>(marker) ?? false;
    if (was === active) return;
    const count =
      (await this.tx.get<number>(this.k("maintenance-count", flag)) ?? 0) +
      (active ? 1 : -1);
    await this.tx.put(marker, active);
    await this.tx.put(this.k("maintenance-count", flag), count);
    if (active) await this.indexAdd(["maintenance", flag], reference);
  }
  async maintenanceActive(flag: string, reference: string): Promise<boolean> {
    return await this.tx.get<boolean>(
      this.k("maintenance-active", flag, reference),
    ) ?? false;
  }
  async maintenanceNeeded(): Promise<string[]> {
    const root = await this.ensure();
    const version = Number(root.properties.timeline_projection_version ?? 0);
    if (version > 1) throw new Error("unsupported timeline projection version");
    const flags: string[] = [];
    if (
      (await this.tx.get<number>(
        this.k("maintenance-count", "visualization_metadata"),
      ) ?? 0) > 0
    ) flags.push("visualization_metadata");
    if (version < 1) flags.push("timeline_projection");
    return flags;
  }
  async firstMessage(conversation: string): Promise<string | null> {
    return (await this.tx.get<{ reference: string }>(
      this.k("first-message", conversation),
    ))?.reference ?? null;
  }
  async incident(reference: string): Promise<Relationship[]> {
    const edges: Relationship[] = [];
    const index = ["incident", reference];
    for (let slot = 1, end = await this.count(index); slot <= end; slot++) {
      const tuple = await this.entry(index, slot);
      if (!tuple) continue;
      const [source, kind, target] = JSON.parse(tuple) as string[];
      const edge = await this.edge(source, kind, target);
      if (edge) edges.push(edge);
    }
    return edges;
  }
  async removeEdge(
    source: string,
    kind: string,
    target: string,
  ): Promise<boolean> {
    const old = await this.edge(source, kind, target);
    if (!old) return false;
    await this.tx.put(this.k("edge-reference", old.reference), null);
    await this.tx.delete(this.k("edge", source, kind, target));
    await historyMembership(this, source, kind, target, false);
    if (kind === "message") {
      const first = await this.tx.get<{ reference: string; position: number }>(
        this.k("first-message", source),
      );
      if (first?.reference === target) {
        const next = (await this.edges(source, kind))[0];
        if (next) {
          await this.tx.put(this.k("first-message", source), {
            reference: next.target_ref,
            position: next.position,
          });
        } else await this.tx.delete(this.k("first-message", source));
      }
    }
    return true;
  }
  async remove(reference: string): Promise<boolean> {
    const old = await this.get(reference);
    if (!old) return false;
    const index = ["incident", reference];
    for (let slot = 1, end = await this.count(index); slot <= end; slot++) {
      const tuple = await this.entry(index, slot);
      if (tuple) {
        const [source, kind, target] = JSON.parse(tuple) as string[];
        await this.removeEdge(source, kind, target);
      }
    }
    await this.maintenanceFlag("visualization_metadata", reference, false);
    await expireDependencies(this.tx, reference);
    await this.tx.delete(this.k("record", reference));
    await removePageMessage(this, reference);
    return true;
  }
  async claim(reference: string, role: string, owner: string): Promise<void> {
    const claim = await this.tx.get<{ role: string; owner: string }>(
      key("claim", reference),
    );
    if (claim && (claim.role !== role || claim.owner !== owner)) {
      throw new Error("CWR identity ownership conflict");
    }
    if (!claim) await this.tx.put(key("claim", reference), { role, owner });
  }
}
export async function addDependency(
  tx: Transaction,
  reference: string,
  receipt: string,
): Promise<void> {
  const marker = key("dependency", reference, receipt);
  if (await tx.get<boolean>(marker)) return;
  const count =
    (await tx.get<number>(key("dependency-count", reference)) ?? 0) + 1;
  await tx.put(key("dependency-entry", reference, count), receipt);
  await tx.put(marker, true);
  await tx.put(key("dependency-count", reference), count);
}
export async function expireDependencies(
  tx: Transaction,
  reference: string,
): Promise<void> {
  const count = await tx.get<number>(key("dependency-count", reference)) ?? 0;
  for (let slot = 1; slot <= count; slot++) {
    const receipt = await tx.get<string>(
      key("dependency-entry", reference, slot),
    );
    if (receipt) {
      await tx.put(receipt, {});
      await tx.delete(key("dependency", reference, receipt));
      await tx.delete(key("dependency-entry", reference, slot));
    }
  }
  await tx.delete(key("dependency-count", reference));
}
