import type { Authorizer } from "./engine.ts";
import {
  compare,
  integerOrNull,
  isObject,
  stringOrNull,
} from "./conversation_resource.ts";
import { derived } from "./conversation_graph.ts";
import { object, type ObjectValue } from "./conversation_types.ts";
import type { SelectionReader } from "./read_selection_reader.ts";
import type {
  InspectorCollection,
  InspectorReadRequest,
  InspectorTextRequest,
} from "./read_selection_types.ts";
// Cursor scope uses the same immutable query identities as native. These are
// implementation constants, never caller-provided expressions or selectors.
import { inspectorQueryIdentity } from "./read_selection_inspector_identity.ts";
const chars = (value: unknown, size: number) => {
  if (typeof value !== "string") return null;
  let result = "", count = 0;
  for (const char of value) {
    if (count++ === size) break;
    result += char;
  }
  return result;
};
const longer = (value: unknown, size: number) => {
  if (typeof value !== "string") return false;
  let count = 0;
  for (const _char of value) if (++count > size) return true;
  return false;
};
const byteLength = (value: string) => new TextEncoder().encode(value).length;
interface Entry {
  position: number;
  reference: string;
  payload: ObjectValue;
}
export class SelectionInspector {
  constructor(
    readonly reader: SelectionReader,
    readonly authorize: Authorizer,
  ) {}
  async root(invocation: string) {
    const facts = await this.reader.facts(invocation, this.authorize);
    return facts?.resource.kind === "invocation"
      ? facts.conversation_ref
      : null;
  }
  async capture(invocation: string) {
    for (const edge of await this.reader.edges(invocation, "tool_attempt")) {
      const node = await this.reader.node(edge.target_ref);
      if (
        node?.kind === "tool_attempt" &&
        object(node.properties.record).capture_version === 1
      ) return true;
    }
    return false;
  }
  async details(invocation: string, actor: string) {
    const pending = [invocation], seen = new Set<string>();
    while (pending.length) {
      const reference = pending.pop()!;
      if (seen.has(reference)) continue;
      seen.add(reference);
      this.reader.take();
      const record = await this.reader.invocationRecord(reference);
      if (!record) continue;
      if (
        record.requester_actor_ref === actor ||
        (reference === invocation && record.target_actor_ref === actor)
      ) return true;
      if (typeof record.parent_invocation_ref === "string") {
        pending.push(record.parent_invocation_ref);
      }
    }
    return false;
  }
  async owner(
    invocation: string,
    conversation: string,
    actor: string,
    details: boolean,
  ) {
    if (
      await this.root(invocation) !== conversation ||
      !await this.reader.participantVisible(conversation, actor)
    ) throw new Error("inspector invocation is unavailable");
    if (details && !await this.details(invocation, actor)) {
      throw new Error("inspector details are unavailable");
    }
  }
  async scope(
    actor: string,
    invocation: string,
    collection: InspectorCollection,
    activity: string | null,
  ) {
    return await derived("inspector_scope", [
      actor,
      invocation,
      activity ?? "",
      inspectorQueryIdentity(collection),
    ]);
  }
  async activity(invocation: string, reference: string) {
    if (
      !await this.reader.graph.edge(invocation, "tool_operation", reference)
    ) return null;
    const node = await this.reader.node(reference);
    return node?.kind === "tool_operation"
      ? object(node.properties.record)
      : null;
  }
  async sequence(invocation: string, activity: string) {
    return integerOrNull((await this.activity(invocation, activity))?.sequence);
  }
  async exitCode(
    invocation: string,
    conversation: string,
    actor: string,
    activity: string,
  ) {
    await this.owner(invocation, conversation, actor, true);
    const record = await this.activity(invocation, activity);
    return record && isObject(record.activity)
      ? integerOrNull(record.activity.exit_code)
      : null;
  }
  async *activities(
    invocation: string,
    details: boolean,
  ): AsyncGenerator<Entry> {
    const body = await this.reader.invocationRecord(invocation);
    for (const edge of await this.reader.edges(invocation, "tool_operation")) {
      const node = await this.reader.node(edge.target_ref);
      if (node?.kind !== "tool_operation") continue;
      const record = object(node.properties.record),
        activity = object(record.activity);
      const attempt = typeof record.attempt_ref === "string"
        ? await this.reader.node(record.attempt_ref)
        : null;
      if (attempt?.kind !== "tool_attempt") continue;
      const started = integerOrNull(activity.started_at_unix_ms);
      yield {
        position: edge.position,
        reference: node.reference,
        payload: {
          kind: "activity",
          activity_ref: node.reference,
          activity_kind: chars(activity.kind, 256),
          label: details ? chars(activity.label, 1024) : "Tool activity",
          label_truncated: details && longer(activity.label, 1024),
          state: activity.state ?? null,
          active: activity.state === "running" &&
            body?.attempt_active === true &&
            object(attempt.properties.record).attempt === body.attempt_count,
          started_at: started === 0 ? null : started,
          duration_ms: integerOrNull(activity.duration_ms),
          child_invocation_ref: stringOrNull(activity.child_invocation_ref),
          details_available: details,
        },
      };
    }
  }
  async *calls(invocation: string): AsyncGenerator<Entry> {
    for (
      const edge of await this.reader.incoming(
        invocation,
        "field:parent_invocation_ref",
      )
    ) {
      const body = await this.reader.invocationRecord(edge.source_ref);
      if (
        !body || !isObject(body.origin) || body.origin.kind !== "host_call" ||
        body.origin.caller_invocation_ref !== invocation
      ) continue;
      yield {
        position: integerOrNull(body.admission_sequence) ?? 0,
        reference: String(body.origin.call_id),
        payload: {
          kind: "call",
          call_id: body.origin.call_id,
          callee_invocation_ref: edge.source_ref,
        },
      };
    }
  }
  async *waits(invocation: string): AsyncGenerator<Entry> {
    for (
      const edge of await this.reader.incoming(
        invocation,
        "field:waiter_invocation_ref",
      )
    ) {
      const node = await this.reader.node(edge.source_ref);
      if (node?.kind !== "wait") continue;
      const body = object(node.properties.record);
      yield {
        position: 0,
        reference: String(body.call_id),
        payload: {
          kind: "wait",
          call_id: body.call_id,
          awaited_invocation_ref: body.awaited_invocation_ref ?? null,
          state: body.state,
        },
      };
    }
  }
  async *files(activity: string): AsyncGenerator<Entry> {
    const owner = await this.reader.node(activity),
      sequence = owner &&
        integerOrNull(object(owner.properties.record).sequence);
    if (sequence === null) return;
    for (const edge of await this.reader.edges(activity, "tool_file")) {
      const node = await this.reader.node(edge.target_ref);
      if (node?.kind !== "tool_file") continue;
      const record = object(node.properties.record);
      yield {
        position: edge.position,
        reference: `${sequence}:${edge.position}`,
        payload: {
          kind: "file",
          path: chars(record.path, 4096),
          path_truncated: longer(record.path, 4096),
          change_kind: chars(record.kind, 256),
        },
      };
    }
  }
  async page(request: InspectorReadRequest) {
    if (
      !Number.isSafeInteger(request.size) || request.size < 1 ||
      request.size > 100 || !Number.isSafeInteger(request.remaining_bytes) ||
      request.remaining_bytes < 0 || request.remaining_bytes > 512 * 1024
    ) throw new Error("invalid inspector page budget");
    await this.owner(
      request.invocation_ref,
      request.conversation_ref,
      request.actor_ref,
      request.details,
    );
    if (
      request.collection === "files" &&
      (!request.details || !request.activity_ref)
    ) throw new Error("file inspector requires activity details");
    const scope = await this.scope(
      request.actor_ref,
      request.invocation_ref,
      request.collection,
      request.activity_ref,
    );
    const source = request.collection === "activities"
      ? this.activities(request.invocation_ref, request.details)
      : request.collection === "calls"
      ? this.calls(request.invocation_ref)
      : request.collection === "waits"
      ? this.waits(request.invocation_ref)
      : this.files(request.activity_ref!);
    for (const key of [request.after, request.before]) {
      if (key && key.scope !== scope) {
        throw new Error("Invalid inspector cursor scope");
      }
    }
    const cmp = (
      a: { position: number; reference: string },
      b: { position: number; reference: string },
    ) => a.position - b.position || compare(a.reference, b.reference);
    const order = request.backwards ? -1 : 1;
    let foundAfter = request.after === null,
      foundBefore = request.before === null;
    const entries: Entry[] = [];
    for await (const entry of source) {
      if (request.after && cmp(entry, request.after) === 0) foundAfter = true;
      if (request.before && cmp(entry, request.before) === 0) {
        foundBefore = true;
      }
      if (
        (request.after && cmp(entry, request.after) <= 0) ||
        (request.before && cmp(entry, request.before) >= 0)
      ) continue;
      let index = entries.findIndex((current) =>
        order * cmp(entry, current) < 0
      );
      if (index < 0) index = entries.length;
      if (index <= request.size) {
        entries.splice(index, 0, entry);
        if (entries.length > request.size + 1) entries.pop();
      }
    }
    if (!foundAfter || !foundBefore) {
      throw new Error(
        "Inspector cursor entry unavailable; request a fresh page",
      );
    }
    let total = 0;
    return entries.slice(0, request.size + 1).map((entry) => {
      const serialized_bytes = byteLength(JSON.stringify(entry.payload)) +
        byteLength(entry.reference) + 64;
      total += serialized_bytes;
      if (total > request.remaining_bytes) {
        throw new Error("Inspector selection exceeds 512 KiB response budget");
      }
      return {
        key: { scope, position: entry.position, reference: entry.reference },
        payload: entry.payload,
        serialized_bytes,
      };
    });
  }
  async text(request: InspectorTextRequest) {
    await this.owner(
      request.invocation_ref,
      request.conversation_ref,
      request.actor_ref,
      true,
    );
    const record = await this.activity(
      request.invocation_ref,
      request.activity_ref,
    );
    if (!record) return null;
    let text: unknown;
    if (request.field === "diff") {
      const key = request.file;
      if (!key) throw new Error("File identity required for diff");
      if (key.position < 0) throw new Error("Invalid file position");
      const sequence = integerOrNull(record.sequence);
      if (key.reference !== `${sequence}:${key.position}`) {
        throw new Error("File entry changed; request a fresh page");
      }
      for (
        const edge of await this.reader.edges(request.activity_ref, "tool_file")
      ) {
        if (edge.position !== key.position) continue;
        const file = await this.reader.node(edge.target_ref);
        if (file?.kind === "tool_file") {
          text = object(file.properties.record).diff;
          break;
        }
      }
    } else text = object(record.activity)[request.field];
    if (typeof text !== "string") return null;
    const bytes = new TextEncoder().encode(text);
    return {
      bytes: Array.from(bytes.slice(0, 16384)),
      full_bytes: bytes.length,
    };
  }
}
