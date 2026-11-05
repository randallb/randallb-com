import { Graph, rootRef } from "./conversation_graph.ts";
import {
  compare,
  integerOrNull,
  isObject,
  ResourceReader,
  stringOrNull,
} from "./conversation_resource.ts";
import { invocationFacts } from "./conversation_resource_invocation.ts";
import {
  type GraphRecord,
  MAX_WIRE_BYTES,
  object,
  type ObjectValue,
  type RecordKind,
} from "./conversation_types.ts";

/** One work counter is shared by every nested read in a finite selection. */
export class SelectionReader extends ResourceReader {
  constructor(graph: Graph, readonly deadline: number) {
    super(graph);
  }
  override take() {
    if (Date.now() >= this.deadline) {
      throw new Error("conversation read operation exceeded its deadline");
    }
    super.take();
  }
  async *records(kind: RecordKind): AsyncGenerator<GraphRecord> {
    const index = ["records", kind];
    for (
      let slot = 1, end = await this.graph.count(index);
      slot <= end;
      slot++
    ) {
      this.take();
      const reference = await this.graph.entry(index, slot);
      const node = reference ? await this.node(reference) : null;
      if (node?.kind === kind) yield node;
    }
  }
  async header(conversation: string) {
    if (
      !await this.graph.edge(
        rootRef(this.graph.scope),
        "conversation",
        conversation,
      )
    ) return null;
    const node = await this.node(conversation);
    if (node?.kind !== "conversation") return null;
    return {
      node,
      parent: await this.one(conversation, "parent"),
      participants: (await this.edges(conversation, "participant")).map((
        edge,
      ) => edge.target_ref),
    };
  }
  async participantVisible(
    conversation: string,
    actor: string,
  ): Promise<boolean> {
    const header = await this.header(conversation);
    return !!header && header.parent === null &&
      header.participants.includes(actor);
  }
  async current(conversation: string) {
    const values: { reference: string; admitted: number }[] = [];
    for (
      const edge of await this.incoming(conversation, "field:conversation_ref")
    ) {
      const node = await this.node(edge.source_ref);
      if (node?.kind !== "invocation") continue;
      const record = object(node.properties.record);
      if (
        record.conversation_ref === conversation &&
        ["admitted", "awaiting_person", "running", "waiting", "stopping"]
          .includes(String(record.state))
      ) {
        values.push({
          reference: node.reference,
          admitted: Number(record.admitted_at_unix_ms),
        });
      }
    }
    values.sort((a, b) =>
      a.admitted - b.admitted || compare(a.reference, b.reference)
    );
    const result = [];
    let bytes = 2;
    for (const node of values) {
      const body = await this.node(node.reference);
      if (!body) throw new Error("active invocation disappeared from snapshot");
      const facts = await invocationFacts(this, conversation, body, null);
      bytes += new TextEncoder().encode(JSON.stringify(facts)).length + 1;
      if (bytes > MAX_WIRE_BYTES) {
        throw new Error("current invocation facts exceed wire limit");
      }
      result.push(facts);
    }
    return result;
  }
  async invocation(conversation: string, reference: string) {
    const node = await this.node(reference);
    if (
      node?.kind !== "invocation" ||
      object(node.properties.record).conversation_ref !== conversation
    ) return null;
    return await invocationFacts(this, conversation, node, null);
  }
  async conversationExists(
    conversation: string,
    execution: boolean,
    owner: [string, boolean] | null,
  ) {
    const header = await this.header(conversation);
    if (!header || (!execution && header.parent !== null)) return false;
    if (!owner) return true;
    const actual = stringOrNull(header.node.properties.owner_context_ref);
    return actual === null ? owner[1] : owner[0] !== "" && actual === owner[0];
  }
  async rootPresentation(conversation: string) {
    const header = await this.header(conversation);
    return header && header.parent === null
      ? [
        stringOrNull(header.node.properties.title),
        (await this.edges(conversation, "archived")).length > 0,
      ]
      : null;
  }
  async forkSource(conversation: string) {
    const activities = (await this.activities(conversation)).reverse();
    for (const { record } of activities) {
      const event = object(record.event);
      if (event.kind === "conversation_forked_from") {
        return [
          String(event.source_conversation_ref),
          String(event.source_message_ref),
        ];
      }
    }
    return null;
  }
  async itemConversation(reference: string): Promise<string | null> {
    const pending = [reference], seen = new Set<string>();
    while (pending.length) {
      const reference = pending.shift()!;
      if (seen.has(reference)) continue;
      seen.add(reference);
      this.take();
      const header = await this.header(reference);
      if (header && header.parent === null) return reference;
      for (const edge of await this.incident(reference)) {
        if (
          edge.target_ref === reference &&
          [
            "conversation_timeline_item",
            "conversation_timeline_child",
            "conversation_timeline_root",
          ].includes(edge.kind)
        ) pending.push(edge.source_ref);
      }
    }
    return null;
  }
  async actorIdentity(actor: string) {
    for await (const node of this.records("local_actor_identity")) {
      const body = object(node.properties.record);
      if (body.actor_ref === actor) return body;
    }
    return null;
  }
  async actorCandidate(actor: string) {
    const retained = await this.graph.tx.get(`ordinal:${actor}`);
    return retained !== undefined && await this.node(actor) === null;
  }
  async actorConversations(actor: string) {
    const references: string[] = [];
    for (const edge of await this.incoming(actor, "participant")) {
      const header = await this.header(edge.source_ref);
      if (header && header.parent === null) references.push(edge.source_ref);
    }
    return references;
  }
  async actorResources(actor: string) {
    const references: string[] = [];
    for (const kind of ["message", "invocation"] as const) {
      for await (const node of this.records(kind)) {
        const body = isObject(node.properties.record)
          ? node.properties.record
          : {};
        const stream = isObject(node.properties.streaming)
          ? node.properties.streaming
          : {};
        const streamMessage = isObject(stream.message) ? stream.message : {};
        if (
          (kind === "message" &&
            (body.author_actor_ref === actor ||
              streamMessage.author_actor_ref === actor ||
              (Array.isArray(body.addressed_actor_refs) &&
                body.addressed_actor_refs.includes(actor)))) ||
          (kind === "invocation" &&
            (body.requester_actor_ref === actor ||
              body.target_actor_ref === actor))
        ) references.push(node.reference);
      }
    }
    return references.sort(compare);
  }
  async artifactExists(reference: string) {
    return (await this.node(reference))?.kind === "visualization_artifact";
  }
  async artifactMessages(reference: string) {
    const result: string[] = [];
    for await (const node of this.records("message")) {
      const body = isObject(node.properties.record)
        ? node.properties.record
        : null;
      if (!body) continue;
      for (const edge of await this.incoming(node.reference, "message")) {
        const record = await this.message(edge.source_ref, node.reference);
        if (
          record && Array.isArray(record.visualizations) &&
          record.visualizations.some((v) =>
            isObject(v) && v.artifact_ref === reference
          )
        ) {
          result.push(node.reference);
          break;
        }
      }
    }
    return [...new Set(result)].sort(compare);
  }
  async artifactHtml(reference: string) {
    const node = await this.node(reference);
    return node?.kind === "visualization_artifact"
      ? stringOrNull(object(node.properties.record).html)
      : null;
  }
  async invocationRecord(reference: string): Promise<ObjectValue | null> {
    const node = await this.node(reference);
    return node?.kind === "invocation" ? object(node.properties.record) : null;
  }
  async replyRef(reference: string) {
    const body = await this.invocationRecord(reference);
    if (!body) return null;
    if (typeof body.reply_message_ref === "string") {
      return body.reply_message_ref;
    }
    for (const edge of await this.incoming(reference, "field:invocation_ref")) {
      const node = await this.node(edge.source_ref);
      if (node?.kind === "admission") {
        const reply = object(node.properties.record).reply_message_ref;
        if (typeof reply === "string") return reply;
      }
    }
    return null;
  }
  async forkDestination(reference: string, intrinsic: string) {
    const body = await this.invocationRecord(reference),
      last = body && Array.isArray(body.responses)
        ? body.responses.at(-1)
        : null;
    const result = isObject(last) && isObject(last.result) ? last.result : null;
    return body?.action_name === "fork--conversation" &&
        body.target_actor_ref === intrinsic && body.state === "completed" &&
        result?.kind === "conversation_forked"
      ? stringOrNull(result.conversation_ref)
      : null;
  }
  async stopFacts(reference: string, intrinsic: string) {
    const body = await this.invocationRecord(reference);
    if (
      body?.action_name !== "stop--invocation" ||
      body.target_actor_ref !== intrinsic
    ) return null;
    const input = isObject(body.input) ? body.input : {},
      last = Array.isArray(body.responses) ? body.responses.at(-1) : null;
    const result = isObject(last) && isObject(last.result) ? last.result : {};
    const error = isObject(last) && isObject(last.error) ? last.error : {};
    const details = isObject(error.details) ? error.details : {};
    return {
      input: stringOrNull(input.invocation_ref),
      result: stringOrNull(result.kind) ?? stringOrNull(details.kind),
      resolved: stringOrNull(result.invocation_ref) ??
        stringOrNull(details.invocation_ref),
      state: String(body.state),
    };
  }
  async actionInvocation(conversation: string, message: string) {
    const result: string[] = [];
    for (
      const edge of await this.incoming(conversation, "field:conversation_ref")
    ) {
      const body = await this.invocationRecord(edge.source_ref);
      if (!body || body.conversation_ref !== conversation) continue;
      if (
        body.reply_message_ref === message ||
        body.source_message_ref === message ||
        await this.replyRef(edge.source_ref) === message
      ) result.push(edge.source_ref);
      if (result.length === 2) break;
    }
    return result.length === 1 ? result[0] : null;
  }
  async progressActivity(invocation: string, source: string) {
    const execution = await this.one(invocation, "execution");
    const node = await this.node(source);
    return execution && node?.kind === "activity" &&
        await this.graph.edge(execution, "activity", source)
      ? object(node.properties.record)
      : null;
  }
  async progressInLatestAttempt(invocation: string, sequence: number | null) {
    if (sequence === null) return false;
    const body = await this.invocationRecord(invocation);
    if (!body) throw new Error("invocation unavailable");
    const boundary = integerOrNull(
      body.attempt_started_after_activity_sequence,
    );
    return boundary === null
      ? (integerOrNull(body.attempt_count) ?? 0) === 1
      : sequence > boundary;
  }
  async latestWorkActivity(invocation: string, activity: string) {
    const execution = await this.one(invocation, "execution");
    if (!execution) return null;
    for (const { record } of (await this.activities(execution)).reverse()) {
      const event = object(record.event);
      if (
        event.kind === "work_update" && isObject(event.activity) &&
        event.activity.activity_ref === activity
      ) return event.activity;
    }
    return null;
  }
  async targetSource(invocation: string) {
    const body = await this.invocationRecord(invocation);
    return body
      ? [String(body.target_actor_ref), stringOrNull(body.source_message_ref)]
      : null;
  }
  async isCommentary(message: string, invocation: string) {
    const node = await this.node(message);
    if (node?.kind !== "message") return false;
    const body = isObject(node.properties.record) ? node.properties.record : {},
      stream = isObject(node.properties.streaming)
        ? node.properties.streaming
        : {};
    return (stream.phase === "commentary" &&
      stream.invocation_ref === invocation) ||
      (isObject(body.progress) && body.progress.invocation_ref === invocation);
  }
  async isFinalReply(message: string, target: string, source: string | null) {
    const node = await this.node(message);
    if (node?.kind !== "message") return false;
    const stream = isObject(node.properties.streaming)
      ? node.properties.streaming
      : null;
    const body = stream
      ? object(stream.message)
      : object(node.properties.record);
    return (stream ? stream.phase !== "commentary" : !isObject(body.progress) &&
      (!isObject(body.action) || body.action.kind !== "call")) &&
      body.author_actor_ref === target &&
      (source === null || body.reply_to_message_ref === source);
  }
}
