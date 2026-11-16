import { isObject } from "./conversation_resource.ts";
import type { ReadSelectionRequest } from "./read_selection_types.ts";
const fields = {
  "content_message": {
    "conversation_ref": "String",
    "message_ref": "String",
    "include_execution": "bool",
  },
  "content_positions": {
    "conversation_ref": "String",
    "window": "PositionWindow",
    "include_execution": "bool",
  },
  "completion_page": {
    "conversation_ref": "String",
    "window": "CompletionWindow",
    "include_execution": "bool",
  },
  "timeline_page": {
    "conversation_ref": "String",
    "window": "PositionWindow",
    "include_execution": "bool",
  },
  "legacy_timeline_page": {
    "conversation_ref": "String",
    "window": "PositionWindow",
    "include_execution": "bool",
  },
  "legacy_timeline_message": {
    "conversation_ref": "String",
    "key": "PositionCursor",
    "message_ref": "String",
    "include_execution": "bool",
  },

  "summary_window": { "request": "SummaryWindowRequest" },
  "summary_row": {
    "reference": "String",
    "workspace": "Option<(Option<String>,String)>",
  },
  "summary_message_record": {
    "conversation_ref": "String",
    "message_ref": "String",
  },
  "position": {},
  "current": {
    "conversation_ref": "String",
  },
  "invocation": {
    "conversation_ref": "String",
    "invocation_ref": "String",
  },
  "resource": {
    "reference": "String",
  },
  "conversation_exists": {
    "conversation_ref": "String",
    "include_execution": "bool",
    "owner": "Option<(String, bool)>",
  },
  "participant_visible": {
    "conversation_ref": "String",
    "actor_ref": "String",
  },
  "root_conversation_presentation": {
    "conversation_ref": "String",
  },
  "fork_source": {
    "conversation_ref": "String",
  },
  "owner_context": {
    "conversation_ref": "String",
  },
  "timeline_item_conversation": {
    "item_ref": "String",
  },
  "actor_identity": {
    "actor_ref": "String",
  },
  "actor_candidate": {
    "actor_ref": "String",
  },
  "actor_conversations": {
    "actor_ref": "String",
  },
  "actor_resources": {
    "actor_ref": "String",
  },
  "artifact_exists": {
    "reference": "String",
  },
  "artifact_messages": {
    "reference": "String",
  },
  "artifact_html": {
    "reference": "String",
  },
  "invocation_reply_ref": {
    "invocation_ref": "String",
  },
  "invocation_fork_destination": {
    "invocation_ref": "String",
    "intrinsic_actor_ref": "String",
  },
  "invocation_stop_facts": {
    "invocation_ref": "String",
    "intrinsic_actor_ref": "String",
  },
  "invocation_visible": {
    "root_ref": "String",
    "invocation_ref": "String",
  },
  "streaming_message_metadata": {
    "message_ref": "String",
  },
  "message_action_invocation": {
    "conversation_ref": "String",
    "message_ref": "String",
  },
  "progress_activity": {
    "invocation_ref": "String",
    "source_activity_ref": "String",
  },
  "progress_in_latest_attempt": {
    "invocation_ref": "String",
    "sequence": "Option<u64>",
  },
  "latest_work_activity": {
    "invocation_ref": "String",
    "activity_ref": "String",
  },
  "timeline_root": {
    "conversation_ref": "String",
    "include_execution": "bool",
  },
  "timeline_connection_metadata": {
    "conversation_ref": "String",
    "root_ref": "String",
    "connection_ref": "String",
  },
  "timeline_cursor_exists": {
    "connection_ref": "String",
    "sequence": "i64",
    "item_ref": "String",
  },
  "timeline_connection_items": {
    "connection_ref": "String",
    "lower": "Option<i64>",
    "upper": "Option<i64>",
    "limit": "usize",
    "backwards": "bool",
  },
  "timeline_connection_count": {
    "conversation_ref": "String",
    "connection_ref": "String",
  },
  "timeline_item": {
    "conversation_ref": "String",
    "item_ref": "String",
  },
  "inspector_capture_available": {
    "invocation_ref": "String",
  },
  "inspector_details_allowed": {
    "invocation_ref": "String",
    "actor_ref": "String",
  },
  "inspector_invocation_root": {
    "invocation_ref": "String",
  },
  "inspector_scope": {
    "actor_ref": "String",
    "invocation_ref": "String",
    "collection": "InspectorCollection",
    "activity_ref": "Option<String>",
  },
  "inspector_page": {
    "request": "InspectorReadRequest",
  },
  "inspector_activity_sequence": {
    "invocation_ref": "String",
    "conversation_ref": "String",
    "activity_ref": "String",
  },
  "inspector_exit_code": {
    "invocation_ref": "String",
    "conversation_ref": "String",
    "actor_ref": "String",
    "activity_ref": "String",
  },
  "inspector_text": {
    "request": "InspectorTextRequest",
  },
  "invocation_target_source": {
    "invocation_ref": "String",
  },
  "message_is_commentary": {
    "message_ref": "String",
    "invocation_ref": "String",
  },
  "message_is_final_reply": {
    "message_ref": "String",
    "target_actor_ref": "String",
    "source_message_ref": "Option<String>",
  },
  "last_addressed_actors": {
    "conversation_ref": "String",
    "root_ref": "String",
    "author_actor_ref": "String",
  },
  "history_read": {
    "conversation_ref": "String",
    "selection": "HistorySelection",
    "participant_actor_ref": "Option<String>",
    "include_execution": "bool",
  },
} as const;
export function ref(value: unknown): asserts value is string {
  if (
    typeof value !== "string" || !value.startsWith("cwp://") ||
    value.length <= 6 || value.length > 4096 || value.includes("#") ||
    /\s/u.test(value)
  ) throw new Error("invalid canonical CWR ref");
}
function shape(
  value: unknown,
  keys: readonly string[],
): asserts value is Record<string, unknown> {
  if (
    !isObject(value) || Object.keys(value).some((key) => !keys.includes(key))
  ) throw new Error("invalid finite read selection shape");
}
export function validateSelection(request: ReadSelectionRequest): void {
  shape(request, [
    "scope",
    "expected_commit_revision",
    "selection",
    "deadline_unix_ms",
    "max_bytes",
  ]);
  shape(request.selection, [
    "operation",
    ...Object.keys(fields[request.selection.operation] ?? {}),
  ]);
  const specification = fields[request.selection.operation];
  if (!specification) {
    throw new Error("unsupported finite read selection operation");
  }
  for (const [key, type] of Object.entries(specification)) {
    const input = request.selection as unknown as Record<string, unknown>;
    if (input[key] === undefined && type.startsWith("Option<")) {
      input[key] = null;
    }
    const value = input[key];
    if (type.startsWith("Option<") && value === null) continue;
    if (
      (type === "String" || type === "Option<String>") &&
      typeof value !== "string"
    ) throw new Error("invalid finite read string");
    if (type === "bool" && typeof value !== "boolean") {
      throw new Error("invalid finite read boolean");
    }
    if (
      ["i64", "u64", "usize", "Option<u64>", "Option<i64>"].includes(type) &&
      (!Number.isSafeInteger(value) ||
        (!["i64", "Option<i64>"].includes(type) && Number(value) < 0))
    ) throw new Error("invalid finite read integer");
    if (
      typeof value === "string" &&
      (key.endsWith("_ref") || key === "reference") &&
      !(request.selection.operation === "latest_work_activity" &&
        key === "activity_ref")
    ) ref(value);
    if (
      type === "InspectorCollection" &&
      !["activities", "calls", "waits", "files"].includes(String(value))
    ) throw new Error("invalid inspector collection");
    if (type === "InspectorReadRequest" || type === "InspectorTextRequest") {
      const text = type === "InspectorTextRequest";
      shape(
        value,
        text
          ? [
            "invocation_ref",
            "conversation_ref",
            "actor_ref",
            "activity_ref",
            "file",
            "field",
          ]
          : [
            "invocation_ref",
            "conversation_ref",
            "actor_ref",
            "collection",
            "activity_ref",
            "details",
            "after",
            "before",
            "size",
            "backwards",
            "remaining_bytes",
          ],
      );
      for (const name of ["invocation_ref", "conversation_ref", "actor_ref"]) {
        ref(value[name]);
      }
      for (const key of text ? ["file"] : ["activity_ref", "after", "before"]) {
        if (value[key] === undefined) value[key] = null;
      }
      if (value.activity_ref !== null) ref(value.activity_ref);
      if (
        text &&
        !["command", "output", "error", "diff"].includes(String(value.field))
      ) throw new Error("invalid inspector text field");
      if (
        !text &&
        !["activities", "calls", "waits", "files"].includes(
          String(value.collection),
        )
      ) throw new Error("invalid inspector collection");
    }
  }
}
