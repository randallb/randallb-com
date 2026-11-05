import { canonical, validContext } from "./engine.ts";
import { derived, Graph, key, rootRef } from "./conversation_graph.ts";
import {
  type MembershipChange,
  object,
  type ObjectValue,
  position,
  type Scope,
  type WriteRecord,
} from "./conversation_types.ts";
export function merge(target: ObjectValue, patch: ObjectValue): ObjectValue {
  const result = structuredClone(target);
  for (const [name, value] of Object.entries(patch)) {
    if (value === null) delete result[name];
    else if (value && typeof value === "object" && !Array.isArray(value)) {
      result[name] = merge(
        result[name] && typeof result[name] === "object" &&
          !Array.isArray(result[name])
          ? object(result[name])
          : {},
        object(value),
      );
    } else {Object.defineProperty(result, name, {
        value: structuredClone(value),
        enumerable: true,
        writable: true,
        configurable: true,
      });}
  }
  return result;
}
export class Writer {
  constructor(
    readonly graph: Graph,
    readonly scope: Scope,
    readonly revision: number,
  ) {}
  async metadata(name: string, value: unknown): Promise<boolean> {
    const root = await this.graph.ensure();
    return (await this.graph.put(root.reference, "runtime", {
      ...root.properties,
      [name]: value,
    })).changed;
  }
  async increment(name: string): Promise<number> {
    const root = await this.graph.ensure();
    const next = Number(root.properties[name] ?? 0) + 1;
    position(next);
    await this.metadata(name, next);
    return next;
  }
  async rollout(stage: string): Promise<boolean> {
    const stages = ["disabled", "new_conversations", "migrate_legacy"];
    const requested = stages.indexOf(stage);
    if (requested < 0) throw new Error("unsupported timeline rollout");
    const root = await this.graph.ensure();
    const retained = stages.indexOf(
      String(root.properties.conversation_timeline_rollout ?? "disabled"),
    );
    if (retained < 0) throw new Error("unsupported retained timeline rollout");
    if (requested <= retained) return false;
    const index = ["relations", "conversation_timeline_invocation"];
    const seen = new Set<string>();
    for (
      let slot = 1, end = await this.graph.count(index);
      slot <= end;
      slot++
    ) {
      const tuple = await this.graph.entry(index, slot);
      if (!tuple) continue;
      const [source, kind, target] = JSON.parse(tuple) as string[];
      if (!await this.graph.edge(source, kind, target)) continue;
      const invocation = await this.graph.get(target);
      if (
        !invocation ||
        object(invocation.properties.record).forked_from_invocation_ref != null
      ) continue;
      if (seen.has(target)) {
        throw new Error(
          "timeline rollout cannot manage repeated invocation placements",
        );
      }
      seen.add(target);
    }
    return await this.metadata("conversation_timeline_rollout", stage);
  }
  async summary(): Promise<void> {
    await this.increment("summary_membership_revision");
  }
  async record(input: WriteRecord): Promise<boolean> {
    validContext(this.scope.context, input.reference);
    object(input.properties);
    if (input.kind === "runtime") {
      throw new Error("runtime metadata uses its named update fields");
    }
    const old = await this.graph.get(input.reference);
    if (old && old.kind !== input.kind) {
      throw new Error("CWR record schema or kind conflict");
    }
    if (
      [
        "conversation",
        "conversation_timeline_connection",
        "conversation_timeline_item",
      ].includes(input.kind)
    ) {
      const owner = input.kind === "conversation"
        ? input.reference
        : input.owner_ref;
      if (!owner) {
        throw new Error("timeline record requires conversation owner");
      }
      await this.graph.claim(input.reference, `node:${input.kind}`, owner);
    }
    const patch = structuredClone(input.properties);
    if (input.kind === "admission") {
      const record = object(patch.record);
      const integration = String(record.integration_ref ?? "");
      const provider = String(record.provider_event_ref ?? "");
      if (
        !integration || !provider ||
        input.reference !== await derived("admission", [integration, provider])
      ) throw new Error("admission identity mismatch");
      if (old) {
        const previous = object(old.properties.record);
        for (
          const field of [
            "integration_ref",
            "provider_event_ref",
            "conversation_ref",
            "source_message_ref",
          ]
        ) {
          if (
            canonical(previous[field] ?? null) !==
              canonical(record[field] ?? null)
          ) throw new Error("immutable admission identity changed");
        }
      }
      if ("source_input_fingerprint" in record) {
        patch.source_input_fingerprint = record.source_input_fingerprint;
        delete record.source_input_fingerprint;
      }
    }
    if (
      input.kind === "upstream_declaration" && old &&
      canonical(old.properties.record) !== canonical(patch.record)
    ) {
      throw new Error(
        "upstream declaration changed without a new coworker version",
      );
    }
    const properties = merge(old?.properties ?? {}, patch);
    for (const name of ["record", "streaming"]) {
      if (name in patch) properties[name] = structuredClone(patch[name]);
    }
    if (input.kind === "conversation") {
      for (
        const name of [
          "creation_order",
          "commit_revision",
          "timeline_revision",
          "completed_work_refs",
        ]
      ) {
        if (name in patch) {
          throw new Error(
            "caller cannot choose storage-owned conversation clocks or summaries",
          );
        }
      }
      if (!old) {
        properties.creation_order =
          (await this.increment("next_creation_order")) - 1;
      }
    }
    if (input.kind === "message" && "record" in patch) {
      delete properties.streaming;
    }
    if (
      input.kind === "file" && old &&
      canonical(old.properties.record) !== canonical(properties.record)
    ) throw new Error("retained files are immutable");
    if (old && canonical(old.properties) === canonical(properties)) {
      return false;
    }
    if (input.kind === "conversation") {
      properties.commit_revision = this.revision;
    }
    await this.graph.put(input.reference, input.kind, properties);
    if (input.kind === "message") {
      await this.graph.maintenanceFlag(
        "visualization_metadata",
        input.reference,
        !!properties.record && typeof properties.record === "object" &&
          "visualizations" in object(properties.record),
      );
    }
    if ("record" in properties) {
      await this.references(input.reference, object(properties.record));
    }
    if (input.kind === "activity") {
      const event = object(object(properties.record).event);
      if (event.kind === "commentary_progress") {
        const names = [
          "provider_integration_ref",
          "response_or_turn_id",
          "item_id",
        ].map((name) => {
          if (typeof event[name] !== "string") {
            throw new Error("commentary identity missing");
          }
          return event[name] as string;
        });
        const identity = await derived("commentary", names);
        const existing = await this.graph.edges(identity, "commentary");
        if (existing.some((edge) => edge.target_ref !== input.reference)) {
          throw new Error("provider commentary item already retained");
        }
        await this.graph.putEdge(
          identity,
          "commentary",
          input.reference,
          0,
          {},
        );
      }
    }
    if (input.kind === "conversation_timeline_item") {
      const publication = properties.publication_ref;
      if (typeof publication !== "string" || !input.owner_ref) {
        throw new Error("timeline publication missing");
      }
      await this.graph.claim(
        publication,
        "conversation_timeline_publication",
        input.owner_ref,
      );
    }
    if (input.kind === "invocation") {
      await this.completion(input.reference, object(properties.record));
    }
    if (input.kind === "conversation") await this.summary();
    return true;
  }
  async references(reference: string, record: ObjectValue): Promise<void> {
    const desired = new Map<string, string[]>();
    for (
      const [prefix, section] of [["", record], [
        "event.",
        record.event,
      ], ["progress.", record.progress]] as const
    ) {
      if (!section || typeof section !== "object" || Array.isArray(section)) {
        continue;
      }
      for (const [name, value] of Object.entries(section)) {
        const refs = name.endsWith("_ref")
          ? [value]
          : name.endsWith("_refs") && Array.isArray(value)
          ? value
          : null;
        if (!refs) continue;
        desired.set(
          `field:${prefix}${name}`,
          refs.filter((value): value is string =>
            typeof value === "string" && value.startsWith("cwp://") &&
            value !== reference
          ),
        );
      }
    }
    const known =
      await this.graph.tx.get<string[]>(key("field-kinds", reference)) ?? [];
    for (const kind of known) if (!desired.has(kind)) desired.set(kind, []);
    for (const [kind, targets] of desired) {
      for (const edge of await this.graph.edges(reference, kind)) {
        if (!targets.includes(edge.target_ref)) {
          await this.graph.removeEdge(reference, kind, edge.target_ref);
        }
      }
      for (const [position, target] of targets.entries()) {
        await this.graph.putEdge(reference, kind, target, position, {});
      }
    }
    await this.graph.tx.put(key("field-kinds", reference), [...desired.keys()]);
  }
  async completion(reference: string, record: ObjectValue): Promise<void> {
    const conversation = typeof record.conversation_ref === "string"
      ? record.conversation_ref
      : null;
    if (
      conversation && record.timeline_publications &&
      typeof record.timeline_publications === "object"
    ) {
      for (const publication of Object.keys(record.timeline_publications)) {
        await this.graph.claim(
          publication,
          "conversation_timeline_publication",
          conversation,
        );
      }
    }
    if (
      record.state === "completed" && record.parent_invocation_ref == null &&
      record.forked_from_invocation_ref == null
    ) {
      const node = (await this.graph.get(reference))!;
      const order = typeof node.properties.completion_order === "number"
        ? node.properties.completion_order
        : await this.increment("completion_order");
      await this.graph.put(reference, "invocation", {
        ...node.properties,
        completion_order: order,
      });
      if (conversation) {
        await this.graph.putEdge(
          conversation,
          "completed_invocation",
          reference,
          order,
          {},
        );
      }
    } else if (conversation) {
      await this.graph.removeEdge(
        conversation,
        "completed_invocation",
        reference,
      );
    }
  }
  async membership(change: MembershipChange): Promise<boolean> {
    const membership = change.operation === "replace"
      ? change.membership
      : null;
    const source = membership?.source_ref ??
      ("source_ref" in change ? change.source_ref : "");
    const kind = membership?.kind ?? ("kind" in change ? change.kind : "");
    let changed = false;
    if (change.operation === "remove") {
      changed = await this.graph.removeEdge(source, kind, change.target_ref);
    } else if (change.operation === "upsert") {
      changed = await this.member(source, kind, change.member);
    } else {
      const wanted = new Set(
        change.membership.members.map((member) => member.target_ref),
      );
      if (wanted.size !== change.membership.members.length) {
        throw new Error("duplicate CWR membership");
      }
      for (const old of await this.graph.edges(source, kind)) {
        if (!wanted.has(old.target_ref)) {
          changed = await this.graph.removeEdge(source, kind, old.target_ref) ||
            changed;
        }
      }
      for (const member of change.membership.members) {
        changed = await this.member(source, kind, member) || changed;
      }
    }
    if (kind === "parent") await this.rootProjection(source);
    if (kind === "conversation") {
      for (const edge of await this.graph.edges(source, kind)) {
        await this.rootProjection(edge.target_ref);
      }
    }
    if (changed && ["parent", "participant", "archived"].includes(kind)) {
      await this.summary();
    }
    return changed;
  }
  async member(
    source: string,
    kind: string,
    member: { target_ref: string; position: number; properties: ObjectValue },
  ): Promise<boolean> {
    position(member.position);
    object(member.properties);
    const shape = membershipShape[kind];
    if (!shape) throw new Error("unsupported CWR membership kind");
    const [sourceKind, targetKind, singleton] = shape;
    for (
      const [reference, expected] of [[source, sourceKind], [
        member.target_ref,
        targetKind,
      ]]
    ) {
      validContext(this.scope.context, reference);
      if (expected === "external") continue;
      const actual = await this.graph.get(reference);
      if (
        kind === "child_invocation" && reference === member.target_ref &&
        !actual
      ) continue;
      if (actual?.kind !== expected) {
        throw new Error(
          `CWR ${kind} membership has invalid endpoint ${reference}`,
        );
      }
      if (expected === "runtime" && reference !== rootRef(this.scope)) {
        throw new Error("CWR membership names another runtime");
      }
    }
    if (
      kind === "upstream_declaration" &&
      object((await this.graph.get(member.target_ref))!.properties.record)
          .actor_ref !== source
    ) throw new Error("upstream declaration actor mismatch");
    if (
      singleton &&
      (await this.graph.edges(source, kind)).some((edge) =>
        edge.target_ref !== member.target_ref
      )
    ) throw new Error("CWR singleton membership requires explicit replacement");
    return await this.graph.putEdge(
      source,
      kind,
      member.target_ref,
      member.position,
      member.properties,
    );
  }
  async rootProjection(conversation: string): Promise<void> {
    const node = await this.graph.get(conversation);
    if (!node || node.kind !== "conversation") return;
    if ((await this.graph.edges(conversation, "parent")).length) {
      await this.graph.removeEdge(
        rootRef(this.scope),
        "root_conversation",
        conversation,
      );
    } else {await this.graph.putEdge(
        rootRef(this.scope),
        "root_conversation",
        conversation,
        Number(node.properties.creation_order ?? 0),
        {},
      );}
  }
}
const membershipShape: Record<string, readonly [string, string, boolean]> = {
  conversation: ["runtime", "conversation", false],
  parent: ["conversation", "conversation", true],
  participant: ["conversation", "external", false],
  archived: ["conversation", "runtime", true],
  message: ["conversation", "message", false],
  activity: ["conversation", "activity", false],
  transfer_metadata: ["conversation", "conversation_transfer_metadata", false],
  admission: ["runtime", "admission", false],
  wait: ["runtime", "wait", false],
  execution: ["invocation", "conversation", true],
  participant_version: ["conversation", "external", false],
  conversation_timeline_root: [
    "conversation",
    "conversation_timeline_connection",
    true,
  ],
  conversation_timeline_item: [
    "conversation_timeline_connection",
    "conversation_timeline_item",
    false,
  ],
  conversation_timeline_child: [
    "conversation_timeline_item",
    "conversation_timeline_connection",
    true,
  ],
  conversation_timeline_message: [
    "conversation_timeline_item",
    "message",
    true,
  ],
  conversation_timeline_invocation: [
    "conversation_timeline_item",
    "invocation",
    true,
  ],
  tool_attempt: ["invocation", "tool_attempt", false],
  tool_operation: ["invocation", "tool_operation", false],
  tool_file: ["tool_operation", "tool_file", false],
  child_invocation: ["tool_operation", "invocation", true],
  upstream_declaration: ["external", "upstream_declaration", false],
};
