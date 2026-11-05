import type {
  ObjectValue,
  ResourceFacts,
  Scope,
} from "./conversation_types.ts";
export interface PositionCursor {
  position: number;
  revision: string | null;
}
export interface ContentPageRequest {
  scope: Scope;
  conversation_ref: string;
  after: PositionCursor | null;
  before: PositionCursor | null;
  first: number | null;
  last: number | null;
  max_bytes: number;
}
export interface ContentPageFacts {
  runtime_root_ref: string;
  conversation_ref: string;
  commit_revision: string;
  conversation: ResourceFacts["conversation"];
  canonical: boolean;
  revision: string;
  items: {
    position: number;
    record: ObjectValue;
    occurred_at_unix_ms: number | null;
  }[];
  has_next_page: boolean;
  has_previous_page: boolean;
  total_count: number;
}
export interface CompletionPageRequest {
  scope: Scope;
  conversation_ref: string;
  after_sequence: number;
  through_sequence: number | null;
  cursor_revision: string | null;
  first: number;
  max_bytes: number;
}
export interface CompletionPageFacts {
  runtime_root_ref: string;
  conversation_ref: string;
  commit_revision: string;
  conversation: ResourceFacts["conversation"];
  canonical: boolean;
  revision: string;
  items: { invocation_ref: string; sequence: number }[];
  through_sequence: number;
  next_after: number;
  has_next_page: boolean;
}
