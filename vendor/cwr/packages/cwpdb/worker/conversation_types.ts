import type { Context } from "./client.ts";
import type { Mutation as InvocationMutation, Receipt } from "./invocations.ts";
export const MAX_WIRE_BYTES = 24 * 1024 * 1024;
export const MAX_RECORD_BYTES = MAX_WIRE_BYTES - 16384;
export type ObjectValue = Record<string, unknown>;
export const recordKinds = [
  "runtime",
  "conversation",
  "message",
  "activity",
  "conversation_transfer_metadata",
  "admission",
  "invocation",
  "wait",
  "file",
  "visualization_artifact",
  "conversation_timeline_connection",
  "conversation_timeline_item",
  "tool_attempt",
  "tool_operation",
  "tool_file",
  "upstream_declaration",
  "local_actor_identity",
] as const;
export type RecordKind = typeof recordKinds[number];
export interface Scope {
  context: Context;
  runtime_root_ref: string;
}
export interface GraphRecord {
  reference: string;
  kind: RecordKind;
  schema_ref: string;
  properties: ObjectValue;
  revision: string;
}
export interface Relationship {
  reference: string;
  source_ref: string;
  kind: string;
  target_ref: string;
  position: number;
  properties: ObjectValue;
  revision: string;
}
export interface WriteRecord {
  reference: string;
  kind: RecordKind;
  properties: ObjectValue;
  owner_ref: string | null;
}
export interface Member {
  target_ref: string;
  position: number;
  properties: ObjectValue;
}
export interface Membership {
  source_ref: string;
  kind: string;
  members: Member[];
}
export type MembershipChange = {
  operation: "upsert";
  source_ref: string;
  kind: string;
  member: Member;
} | {
  operation: "remove";
  source_ref: string;
  kind: string;
  target_ref: string;
} | { operation: "replace"; membership: Membership };
export type PublishedItem = {
  kind: "message";
  message_ref: string;
  position: number;
} | { kind: "turn"; invocation_ref: string; position: number };
export interface Publication {
  conversation_ref: string;
  from_position: number;
  items: PublishedItem[];
  activities: { activity_ref: string; position: number; record: unknown }[];
  remove_activities: string[];
}
export interface TimelineItemWrite {
  conversation_ref: string;
  connection_ref: string;
  item_ref: string;
  publication_ref: string;
  position: number;
  content: { kind: "message"; message_ref: string } | {
    kind: "invocation";
    invocation_ref: string;
  };
  child_connection_ref: string | null;
}
export interface Mutation {
  scope: Scope;
  request_id: string;
  expected_revision: string;
  changes: {
    timeline_items?: TimelineItemWrite[];
    records: WriteRecord[];
    remove_records: { reference: string; kind: RecordKind }[];
    memberships: MembershipChange[];
    publications: Publication[];
    state_version: string | null;
    streaming_runs: ObjectValue | null;
    timeline_rollout?: string | null;
    context_policy?: ObjectValue | null;
    repair_legacy: boolean;
  };
  journal?: { request: InvocationMutation; now_unix_ms: number } | null;
  authority_guards?: { reference: string; expected_revision: string }[];
}
export type Outcome = {
  outcome: "committed";
  revision: string;
  commit_revision: number;
  journal_receipt?: Receipt;
} | { outcome: "committed_result_expired" };
export type Selection =
  | { selection: "bootstrap" }
  | {
    selection: "conversation";
    conversation_ref: string;
  }
  | { selection: "records"; kind: RecordKind }
  | { selection: "conversation_metadata"; conversation_ref: string | null }
  | {
    selection: "commentary";
    provider_integration_ref: string;
    response_or_turn_id: string;
    item_id: string;
  };
export interface ReadRequest {
  scope: Scope;
  selection: Selection;
  continuation: string | null;
  page_size: number;
  max_bytes: number;
  expected_revision: string | null;
}
export interface SnapshotPage {
  revision: string;
  metadata: ObjectValue;
  records: GraphRecord[];
  relationships: Relationship[];
  continuation: string | null;
  complete: boolean;
}
export function object(value: unknown): ObjectValue {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("CWR record must be an object");
  }
  return value as ObjectValue;
}
export function position(value: number): void {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error("invalid CWR position");
  }
}
export interface SummaryFacts {
  conversation_ref: string;
  creation_order: number;
  title: string | null;
  archived: boolean;
  owner_context_ref: string | null;
  parent_ref: string | null;
  sidebar_eligible: boolean;
  root: boolean;
  participant_actor_refs: string[];
  last_message_ref: string | null;
  last_message_preview: string | null;
  last_message_record: ObjectValue | null;
  completed_work: { invocation_ref: string; sequence: number }[];
  completed_work_refs: string[];
  revision: string;
}

export interface HistoryStatus {
  has_timeline_root: boolean;
  has_streaming: boolean;
  has_legacy_context: boolean;
  terminal_activity: boolean;
  revision: string;
}

export interface ResourceFacts {
  runtime_root_ref: string;
  conversation_ref: string;
  commit_revision: string;
  conversation: {
    owner_context_ref: string | null;
    parent_ref: string | null;
    sidebar_eligible: boolean;
    root: boolean;
    participant_actor_refs: string[];
  };
  resource: ResourceRecordFacts;
}
export type ResourceRecordFacts = {
  kind: "message";
  record: ObjectValue;
  streaming: boolean;
  occurred_at_unix_ms: number | null;
} | {
  kind: "invocation";
  record: ObjectValue;
  child_invocation_refs: string[];
  wait_records: ObjectValue[];
  latest_reply: ObjectValue | null;
  execution: ResourceExecutionFacts | null;
};
export interface ResourceExecutionFacts {
  generation: number;
  attempt_active: boolean;
  started_at_unix_ms: number | null;
  latest_activity_at_unix_ms: number | null;
  latest_phase_event: ObjectValue | null;
  unpublished_progress: string | null;
}
