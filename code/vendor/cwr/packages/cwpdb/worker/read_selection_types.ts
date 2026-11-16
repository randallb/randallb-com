import type { PositionCursor } from "./conversation_page_types.ts";
import type {
  CompletionWindow,
  PositionWindow,
} from "./read_selection_pages.ts";
import type { SummaryWindowRequest } from "./read_selection_summary.ts";
import type { HistorySelection } from "./conversation_history_read.ts";
import type { Scope } from "./conversation_types.ts";
export type InspectorCollection = "activities" | "calls" | "waits" | "files";
export interface InspectorKey {
  scope: string;
  position: number;
  reference: string;
}
export interface InspectorReadRequest {
  invocation_ref: string;
  conversation_ref: string;
  actor_ref: string;
  collection: InspectorCollection;
  activity_ref: string | null;
  details: boolean;
  after: InspectorKey | null;
  before: InspectorKey | null;
  size: number;
  backwards: boolean;
  remaining_bytes: number;
}
export interface InspectorTextRequest {
  invocation_ref: string;
  conversation_ref: string;
  actor_ref: string;
  activity_ref: string;
  file: InspectorKey | null;
  field: "command" | "output" | "error" | "diff";
}
export interface ReadSelectionRequest {
  scope: Scope;
  expected_commit_revision: string | null;
  selection: ReadSelectionOperation;
  deadline_unix_ms: number;
  max_bytes: number;
}
export interface ReadSelectionFacts {
  runtime_root_ref: string;
  commit_revision: string;
  result: unknown;
}
export type ReadSelectionOperation =
  | {
    operation: "content_message";
    conversation_ref: string;
    message_ref: string;
    include_execution: boolean;
  }
  | {
    operation: "content_positions";
    conversation_ref: string;
    window: PositionWindow;
    include_execution: boolean;
  }
  | {
    operation: "completion_page";
    conversation_ref: string;
    window: CompletionWindow;
    include_execution: boolean;
  }
  | {
    operation: "timeline_page";
    conversation_ref: string;
    window: PositionWindow;
    include_execution: boolean;
  }
  | {
    operation: "legacy_timeline_page";
    conversation_ref: string;
    window: PositionWindow;
    include_execution: boolean;
  }
  | {
    operation: "legacy_timeline_message";
    conversation_ref: string;
    key: PositionCursor;
    message_ref: string;
    include_execution: boolean;
  }
  | { operation: "summary_window"; request: SummaryWindowRequest }
  | {
    operation: "summary_row";
    reference: string;
    workspace: [string | null, string] | null;
  }
  | {
    operation: "summary_message_record";
    conversation_ref: string;
    message_ref: string;
  }
  | {
    operation: "history_read";
    conversation_ref: string;
    selection: HistorySelection;
    participant_actor_ref: string | null;
    include_execution: boolean;
  }
  | { operation: "position" }
  | { operation: "current"; conversation_ref: string }
  | {
    operation: "invocation";
    conversation_ref: string;
    invocation_ref: string;
  }
  | { operation: "resource"; reference: string }
  | {
    operation: "conversation_exists";
    conversation_ref: string;
    include_execution: boolean;
    owner: [string, boolean] | null;
  }
  | {
    operation: "participant_visible";
    conversation_ref: string;
    actor_ref: string;
  }
  | { operation: "root_conversation_presentation"; conversation_ref: string }
  | { operation: "fork_source"; conversation_ref: string }
  | { operation: "owner_context"; conversation_ref: string }
  | { operation: "timeline_item_conversation"; item_ref: string }
  | { operation: "actor_identity"; actor_ref: string }
  | { operation: "actor_candidate"; actor_ref: string }
  | { operation: "actor_conversations"; actor_ref: string }
  | { operation: "actor_resources"; actor_ref: string }
  | { operation: "artifact_exists"; reference: string }
  | { operation: "artifact_messages"; reference: string }
  | { operation: "artifact_html"; reference: string }
  | { operation: "invocation_reply_ref"; invocation_ref: string }
  | {
    operation: "invocation_fork_destination";
    invocation_ref: string;
    intrinsic_actor_ref: string;
  }
  | {
    operation: "invocation_stop_facts";
    invocation_ref: string;
    intrinsic_actor_ref: string;
  }
  | {
    operation: "invocation_visible";
    root_ref: string;
    invocation_ref: string;
  }
  | { operation: "streaming_message_metadata"; message_ref: string }
  | {
    operation: "message_action_invocation";
    conversation_ref: string;
    message_ref: string;
  }
  | {
    operation: "progress_activity";
    invocation_ref: string;
    source_activity_ref: string;
  }
  | {
    operation: "progress_in_latest_attempt";
    invocation_ref: string;
    sequence: number | null;
  }
  | {
    operation: "latest_work_activity";
    invocation_ref: string;
    activity_ref: string;
  }
  | {
    operation: "timeline_root";
    conversation_ref: string;
    include_execution: boolean;
  }
  | {
    operation: "timeline_connection_metadata";
    conversation_ref: string;
    root_ref: string;
    connection_ref: string;
  }
  | {
    operation: "timeline_cursor_exists";
    connection_ref: string;
    sequence: number;
    item_ref: string;
  }
  | {
    operation: "timeline_connection_items";
    connection_ref: string;
    lower: number | null;
    upper: number | null;
    limit: number;
    backwards: boolean;
  }
  | {
    operation: "timeline_connection_count";
    conversation_ref: string;
    connection_ref: string;
  }
  | { operation: "timeline_item"; conversation_ref: string; item_ref: string }
  | { operation: "inspector_capture_available"; invocation_ref: string }
  | {
    operation: "inspector_details_allowed";
    invocation_ref: string;
    actor_ref: string;
  }
  | { operation: "inspector_invocation_root"; invocation_ref: string }
  | {
    operation: "inspector_scope";
    actor_ref: string;
    invocation_ref: string;
    collection: InspectorCollection;
    activity_ref: string | null;
  }
  | { operation: "inspector_page"; request: InspectorReadRequest }
  | {
    operation: "inspector_activity_sequence";
    invocation_ref: string;
    conversation_ref: string;
    activity_ref: string;
  }
  | {
    operation: "inspector_exit_code";
    invocation_ref: string;
    conversation_ref: string;
    actor_ref: string;
    activity_ref: string;
  }
  | { operation: "inspector_text"; request: InspectorTextRequest }
  | { operation: "invocation_target_source"; invocation_ref: string }
  | {
    operation: "message_is_commentary";
    message_ref: string;
    invocation_ref: string;
  }
  | {
    operation: "message_is_final_reply";
    message_ref: string;
    target_actor_ref: string;
    source_message_ref: string | null;
  }
  | {
    operation: "last_addressed_actors";
    conversation_ref: string;
    root_ref: string;
    author_actor_ref: string;
  };
