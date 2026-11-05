import {
  compare,
  integerOrNull,
  stringOrNull,
} from "./conversation_resource.ts";
import { MessageFlagReader } from "./conversation_page_index.ts";
import type { SelectionReader } from "./read_selection_reader.ts";
import { type ObjectValue } from "./conversation_types.ts";

export class SelectionTimeline {
  readonly flags: MessageFlagReader;
  constructor(readonly reader: SelectionReader) {
    this.flags = new MessageFlagReader(reader.graph);
  }
  async root(
    conversation: string,
    includeExecution: boolean,
  ): Promise<string | null> {
    const header = await this.reader.header(conversation);
    if (!header || (!includeExecution && header.parent !== null)) return null;
    return await this.reader.one(conversation, "conversation_timeline_root");
  }
  async reachable(root: string, reference: string) {
    const pending = [reference], seen = new Set<string>();
    let found = false;
    while (pending.length) {
      const current = pending.pop()!;
      if (seen.has(current)) continue;
      seen.add(current);
      this.reader.take();
      if (current === root) found = true;
      for (
        const child of await this.reader.incoming(
          current,
          "conversation_timeline_child",
        )
      ) {
        for (
          const placement of await this.reader.incoming(
            child.source_ref,
            "conversation_timeline_item",
          )
        ) pending.push(placement.source_ref);
      }
    }
    return found;
  }
  async metadata(conversation: string, root: string, connection: string) {
    if (
      await this.root(conversation, true) !== root ||
      !await this.reachable(root, connection)
    ) {
      throw new Error(
        `timeline connection ${connection} is outside the requested conversation`,
      );
    }
    const node = await this.reader.node(connection),
      revision = node && integerOrNull(node.properties.revision);
    if (
      node?.kind !== "conversation_timeline_connection" || revision === null ||
      revision < 0
    ) throw new Error("invalid timeline connection revision");
    let owner: ObjectValue;
    if (connection === root) {
      owner = { kind: "conversation", conversation_ref: conversation };
    } else {
      const owners = await this.reader.incoming(
        connection,
        "conversation_timeline_child",
      );
      if (owners.length !== 1) {
        throw new Error(
          "timeline child connection must have exactly one owner item",
        );
      }
      owner = { kind: "timeline_item", item_ref: owners[0].source_ref };
    }
    return [owner, revision];
  }
  async cursorExists(connection: string, sequence: number, item: string) {
    const edge = await this.reader.graph.edge(
      connection,
      "conversation_timeline_item",
      item,
    );
    return edge?.position === sequence;
  }
  async decode(connection: string, item: string, position: number) {
    const node = await this.reader.node(item);
    if (node?.kind !== "conversation_timeline_item") return null;
    const messages = await this.reader.edges(
        item,
        "conversation_timeline_message",
      ),
      invocations = await this.reader.edges(
        item,
        "conversation_timeline_invocation",
      ),
      children = await this.reader.edges(item, "conversation_timeline_child");
    if (messages.length + invocations.length !== 1) {
      throw new Error("timeline item must point to exactly one content record");
    }
    let content: ObjectValue;
    if (messages.length) {
      if (children.length) {
        throw new Error("message timeline item owns a child connection");
      }
      content = { kind: "message", message_ref: messages[0].target_ref };
    } else {
      if (children.length !== 1) {
        throw new Error(
          "invocation timeline item must own one child connection",
        );
      }
      content = {
        kind: "invocation",
        invocation_ref: invocations[0].target_ref,
      };
    }
    const publication = stringOrNull(node.properties.publication_ref);
    if (
      publication === null || !Number.isSafeInteger(position) || position < 0
    ) throw new Error("invalid timeline item identity");
    return {
      item_ref: item,
      publication_ref: publication,
      connection_ref: connection,
      sibling_sequence: position,
      content,
      child_connection_ref: children[0]?.target_ref ?? null,
    };
  }
  async items(
    connection: string,
    lower: number | null,
    upper: number | null,
    limit: number,
    backwards: boolean,
  ) {
    if (!Number.isSafeInteger(limit) || limit < 0 || limit > 500) {
      throw new Error("timeline page size exceeds500");
    }
    let edges =
      (await this.reader.edges(connection, "conversation_timeline_item"))
        .filter((edge) =>
          (lower === null || edge.position > lower) &&
          (upper === null || edge.position < upper)
        );
    if (backwards) edges.reverse();
    edges = edges.slice(0, limit + 1);
    const result = [];
    for (const edge of edges) {
      const item = await this.decode(
        connection,
        edge.target_ref,
        edge.position,
      );
      if (item) result.push(item);
    }
    return result;
  }
  async count(conversation: string, connection: string) {
    const root = await this.root(conversation, true);
    if (!root) throw new Error("conversation has no retained timeline");
    await this.metadata(conversation, root, connection);
    return (await this.reader.edges(connection, "conversation_timeline_item"))
      .length;
  }
  async item(conversation: string, reference: string) {
    const root = await this.root(conversation, true);
    if (!root) return null;
    const incoming = await this.reader.incoming(
      reference,
      "conversation_timeline_item",
    );
    const edge = incoming[0];
    if (!edge) return null;
    await this.metadata(conversation, root, edge.source_ref);
    return await this.decode(edge.source_ref, reference, edge.position);
  }
  async firstMessages(conversation: string, root: string) {
    const path = new Set<string>(),
      result: { position: number; reference: string }[] = [],
      seen = new Set<string>();
    let position = 0;
    const walk = async (connection: string, depth: number): Promise<void> => {
      if (depth >= 128 || path.has(connection)) {
        throw new Error("conversation content walk work limit exceeded");
      }
      path.add(connection);
      for (
        const item of await this.reader.edges(
          connection,
          "conversation_timeline_item",
        )
      ) {
        const messages = await this.reader.edges(
          item.target_ref,
          "conversation_timeline_message",
        );
        let streaming = false;
        if (messages.length) {
          streaming =
            (await this.flags.get(messages[0].target_ref))?.streaming === true;
          if (!streaming && !seen.has(messages[0].target_ref)) {
            seen.add(messages[0].target_ref);
            result.push({ position, reference: messages[0].target_ref });
          }
        }
        if (!streaming) position++;
        for (
          const child of await this.reader.edges(
            item.target_ref,
            "conversation_timeline_child",
          )
        ) await walk(child.target_ref, depth + 1);
      }
      path.delete(connection);
    };
    await walk(root, 0);
    // Membership remains the source of a retained public message, rather than
    // a placement manufacturing a message in another conversation.
    const members = [];
    for (const item of result) {
      if (
        await this.reader.graph.edge(conversation, "message", item.reference)
      ) members.push(item);
    }
    return members.sort((a, b) =>
      a.position - b.position || compare(a.reference, b.reference)
    );
  }
  async lastAddressed(conversation: string, root: string, author: string) {
    for (
      const { reference } of (await this.firstMessages(conversation, root))
        .reverse()
    ) {
      const message = await this.reader.message(conversation, reference);
      if (
        message?.author_actor_ref === author &&
        Array.isArray(message.addressed_actor_refs) &&
        message.addressed_actor_refs.length === 1
      ) return message.addressed_actor_refs;
    }
    return [];
  }
}
