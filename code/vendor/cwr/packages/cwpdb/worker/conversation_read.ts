import { canonical, digest, type Transaction } from "./engine.ts";
import { derived, Graph, key, rootRef } from "./conversation_graph.ts";
import {
  type GraphRecord,
  MAX_WIRE_BYTES,
  type ReadRequest,
  type RecordKind,
  type Relationship,
  type SnapshotPage,
} from "./conversation_types.ts";
const bootRecords: RecordKind[] = [
  "runtime",
  "conversation",
  "admission",
  "invocation",
  "wait",
  "tool_attempt",
  "tool_operation",
  "tool_file",
  "conversation_timeline_connection",
  "conversation_timeline_item",
  "upstream_declaration",
  "local_actor_identity",
  "message",
];
const coldRecords: RecordKind[] = [
  "conversation",
  "message",
  "activity",
  "conversation_transfer_metadata",
];
const bootRelations = [
  "conversation",
  "root_conversation",
  "admission",
  "wait",
  "parent",
  "participant",
  "archived",
  "participant_version",
  "conversation_timeline_root",
  "execution",
  "tool_attempt",
  "tool_operation",
  "tool_file",
  "child_invocation",
  "conversation_timeline_item",
  "conversation_timeline_child",
  "conversation_timeline_message",
  "conversation_timeline_invocation",
  "upstream_declaration",
];
const coldRelations = [
  "parent",
  "participant",
  "archived",
  "message",
  "activity",
  "transfer_metadata",
  "conversation_timeline_root",
];
interface Cursor {
  fingerprint: string;
  revision: string;
  phase: number;
  group: number;
  slot: number;
  expires: number;
}
export async function readPage(
  graph: Graph,
  request: ReadRequest,
): Promise<SnapshotPage> {
  if (
    !Number.isSafeInteger(request.page_size) || request.page_size < 1 ||
    request.page_size > 1000 || !Number.isSafeInteger(request.max_bytes) ||
    request.max_bytes < 1 || request.max_bytes > MAX_WIRE_BYTES
  ) throw new Error("invalid CWR page limits");
  const root = await graph.ensure();
  const revision = await digest([
    rootRef(request.scope),
    root.properties.commit_revision,
  ]);
  if (
    request.expected_revision != null && request.expected_revision !== revision
  ) throw new Error("conversation revision_conflict");
  const fingerprint = await digest([
    request.scope,
    request.selection,
    request.page_size,
    request.max_bytes,
  ]);
  let cursor: Cursor = {
    fingerprint,
    revision,
    phase: 0,
    group: 0,
    slot: 0,
    expires: Date.now() + 300000,
  };
  if (request.continuation) {
    const saved = await graph.tx.get<Cursor>(
      key("cursor", request.continuation),
    );
    if (
      !saved || saved.fingerprint !== fingerprint || saved.expires < Date.now()
    ) throw new Error("invalid CWR read continuation");
    if (saved.revision !== revision) {
      throw new Error("conversation revision_conflict");
    }
    cursor = saved;
  }
  const page: SnapshotPage = {
    revision,
    metadata: {
      ...root.properties,
      storage_maintenance: await graph.maintenanceNeeded(),
    },
    records: [],
    relationships: [],
    continuation: null,
    complete: false,
  };
  let bytes = new TextEncoder().encode(canonical(page)).length + 128;
  if (bytes > request.max_bytes) {
    throw new Error("CWR metadata exceeds page byte budget");
  }
  const records = request.selection.selection === "bootstrap"
    ? bootRecords
    : request.selection.selection === "conversation"
    ? coldRecords
    : request.selection.selection === "conversation_metadata"
    ? [
      "conversation",
      "message",
      "conversation_transfer_metadata",
    ] as RecordKind[]
    : request.selection.selection === "commentary"
    ? ["activity"] as RecordKind[]
    : [request.selection.kind];
  let scans = 0;
  while (cursor.phase === 0 && cursor.group < records.length) {
    const kind = records[cursor.group];
    const firstMessage =
      request.selection.selection === "conversation_metadata" &&
      kind === "message";
    const index = firstMessage ? ["first-messages"] : ["records", kind];
    const count = await graph.count(index);
    while (cursor.slot < count) {
      const next = cursor.slot + 1;
      const indexed = await graph.entry(index, next);
      const reference = indexed && firstMessage
        ? await graph.firstMessage(indexed)
        : indexed;
      const record = reference ? await graph.get(reference) : null;
      if (record && await includeRecord(graph, request, record)) {
        const size = new TextEncoder().encode(canonical(record)).length;
        if (!fits(page, request, bytes, size)) {
          return await finish(graph.tx, page, cursor);
        }
        bytes += size + 1;
        page.records.push(record);
      }
      cursor.slot = next;
      if (++scans >= 10000) return await finish(graph.tx, page, cursor);
    }
    cursor.group++;
    cursor.slot = 0;
  }
  if (cursor.phase === 0) {
    cursor.phase = 1;
    cursor.group = 0;
    cursor.slot = 0;
  }
  const kinds = request.selection.selection === "bootstrap"
    ? bootRelations
    : request.selection.selection === "conversation"
    ? coldRelations
    : request.selection.selection === "conversation_metadata"
    ? ["parent", "message", "transfer_metadata"]
    : request.selection.selection === "commentary"
    ? ["activity"]
    : [];
  while (cursor.group < kinds.length) {
    const kind = kinds[cursor.group];
    const cold = request.selection.selection === "conversation"
      ? request.selection.conversation_ref
      : null;
    const index = cold ? ["edges", cold, kind] : ["relations", kind];
    const count = await graph.count(index);
    while (cursor.slot < count) {
      const next = cursor.slot + 1;
      const entry = await graph.entry(index, next);
      let edge: Relationship | null = null;
      if (entry) {
        if (cold) edge = await graph.edge(cold, kind, entry);
        else {
          const [source, k, target] = JSON.parse(entry) as string[];
          edge = await graph.edge(source, k, target);
        }
      }
      if (edge && await includeEdge(graph, request, edge)) {
        const size = new TextEncoder().encode(canonical(edge)).length;
        if (!fits(page, request, bytes, size)) {
          return await finish(graph.tx, page, cursor);
        }
        bytes += size + 1;
        page.relationships.push(edge);
      }
      cursor.slot = next;
      if (++scans >= 10000) return await finish(graph.tx, page, cursor);
    }
    cursor.group++;
    cursor.slot = 0;
  }
  if (request.continuation) {
    await graph.tx.delete(key("cursor", request.continuation));
  }
  page.complete = true;
  return page;
}
async function includeRecord(
  graph: Graph,
  request: ReadRequest,
  record: GraphRecord,
): Promise<boolean> {
  if (request.selection.selection === "records") return true;
  if (request.selection.selection === "commentary") {
    const s = request.selection;
    const lookup = await derived("commentary", [
      s.provider_integration_ref,
      s.response_or_turn_id,
      s.item_id,
    ]);
    return !!await graph.edge(lookup, "commentary", record.reference);
  }
  if (request.selection.selection === "conversation_metadata") {
    const selected = request.selection.conversation_ref;
    if (record.kind === "conversation") {
      return !selected || selected === record.reference;
    }
    for (const edge of await graph.incident(record.reference)) {
      if (
        edge.target_ref !== record.reference ||
        (selected && edge.source_ref !== selected)
      ) continue;
      if (edge.kind === "transfer_metadata") return true;
      if (
        edge.kind === "message" &&
        (await graph.edges(edge.source_ref, "message"))[0]?.target_ref ===
          record.reference
      ) return true;
    }
    return false;
  }
  if (request.selection.selection === "bootstrap") {
    if (record.kind === "runtime") {
      return record.reference === rootRef(request.scope);
    }
    if (record.kind === "conversation") {
      return !!await graph.edge(
        rootRef(request.scope),
        "conversation",
        record.reference,
      ) || !!await graph.tx.get(key("claim", record.reference));
    }
    if (record.kind === "message") {
      return !!record.properties.streaming &&
        typeof record.properties.streaming === "object";
    }
    return true;
  }
  const conversation = request.selection.conversation_ref;
  if (record.kind === "conversation") return record.reference === conversation;
  for (const kind of ["message", "activity", "transfer_metadata"]) {
    if (await graph.edge(conversation, kind, record.reference)) return true;
  }
  return false;
}
async function includeEdge(
  graph: Graph,
  request: ReadRequest,
  edge: Relationship,
): Promise<boolean> {
  if (request.selection.selection === "conversation") {
    return edge.source_ref === request.selection.conversation_ref;
  }
  if (request.selection.selection === "conversation_metadata") {
    if (
      request.selection.conversation_ref &&
      edge.source_ref !== request.selection.conversation_ref
    ) return false;
    return edge.kind !== "message" ||
      await graph.firstMessage(edge.source_ref) === edge.target_ref;
  }
  if (request.selection.selection === "commentary") {
    const s = request.selection;
    const lookup = await derived("commentary", [
      s.provider_integration_ref,
      s.response_or_turn_id,
      s.item_id,
    ]);
    return !!await graph.edge(lookup, "commentary", edge.target_ref);
  }
  if (
    ["conversation", "root_conversation", "admission", "wait"].includes(
      edge.kind,
    )
  ) {
    return edge.source_ref === rootRef(request.scope);
  }
  if (edge.kind === "upstream_declaration") {
    return (await graph.get(edge.target_ref))?.kind === "upstream_declaration";
  }
  const source = await graph.get(edge.source_ref);
  if (source?.kind === "conversation") {
    return await includeRecord(graph, request, source);
  }
  return !!source;
}
function fits(
  page: SnapshotPage,
  request: ReadRequest,
  bytes: number,
  size: number,
): boolean {
  const count = page.records.length + page.relationships.length;
  if (count === 0 && bytes + size + 1 > request.max_bytes) {
    throw new Error(`CWR record requires at least ${bytes + size} page bytes`);
  }
  return count < request.page_size && bytes + size + 1 <= request.max_bytes;
}
async function finish(
  tx: Transaction,
  page: SnapshotPage,
  cursor: Cursor,
): Promise<SnapshotPage> {
  const token = crypto.randomUUID();
  await tx.put(key("cursor", token), cursor);
  page.continuation = token;
  return page;
}
