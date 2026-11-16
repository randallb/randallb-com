import type {
  MembershipChange,
  TimelineItemWrite,
  WriteRecord,
} from "./conversation_types.ts";

export function timelineRecord(item: TimelineItemWrite): WriteRecord {
  if (!item.content || !["message", "invocation"].includes(item.content.kind)) {
    throw new Error("invalid timeline content kind");
  }
  return {
    reference: item.item_ref,
    kind: "conversation_timeline_item",
    owner_ref: item.conversation_ref,
    properties: { publication_ref: item.publication_ref },
  };
}

export function timelineMemberships(
  item: TimelineItemWrite,
): MembershipChange[] {
  const upsert = (
    source_ref: string,
    kind: string,
    target_ref: string,
    position: number,
  ): MembershipChange => ({
    operation: "upsert",
    source_ref,
    kind,
    member: { target_ref, position, properties: {} },
  });
  const result = [
    upsert(
      item.connection_ref,
      "conversation_timeline_item",
      item.item_ref,
      item.position,
    ),
  ];
  if (item.content.kind === "message") {
    result.push(
      upsert(
        item.item_ref,
        "conversation_timeline_message",
        item.content.message_ref,
        0,
      ),
    );
  } else if (item.content.kind === "invocation") {
    result.push(
      upsert(
        item.item_ref,
        "conversation_timeline_invocation",
        item.content.invocation_ref,
        0,
      ),
    );
  } else throw new Error("invalid timeline content kind");
  if (item.child_connection_ref !== null) {
    result.push(
      upsert(
        item.item_ref,
        "conversation_timeline_child",
        item.child_connection_ref,
        0,
      ),
    );
  }
  return result;
}
