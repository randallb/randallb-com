export interface Installed {
  schedule_ref: string;
  owner_context_ref: string;
  author_actor_ref: string;
  definition: Record<string, unknown>;
  availability: string;
  paused: boolean;
  next_due_unix_ms: number;
  active_occurrence_ref: string | null;
  last_admission_error: string | null;
}
export interface Occurrence {
  occurrence_ref: string;
  schedule_ref: string;
  definition: Record<string, unknown>;
  due_unix_ms: number;
  outcome: string;
  invocation_ref: string | null;
  conversation_ref: string | null;
  archived: boolean;
}
export interface Versioned<T> {
  value: T;
  revision: number;
}
export interface Advance {
  schedule_ref: string;
  expected: number;
  next_due_unix_ms: number;
  active_occurrence_ref: string | null;
  last_admission_error: string | null;
}
export type ScheduleRequest =
  | {
    action: "installed" | "archive_candidates";
    runtime_root_ref: string;
    owner?: string | null;
  }
  | {
    action: "get_schedule" | "get_occurrence";
    reference: string;
    owner?: string | null;
  }
  | {
    action: "occurrences";
    reference: string;
    selection: { Recent: number } | "UnarchivedTerminal";
  }
  | {
    action: "install";
    request: {
      runtime_root_ref: string;
      owner_context_ref: string;
      author_actor_ref: string;
      definition: Record<string, unknown>;
      next_due_unix_ms: number;
    };
  }
  | {
    action: "pause";
    owner: string;
    reference: string;
    expected: number;
    paused: boolean;
    next_due_unix_ms: number;
  }
  | {
    action: "availability";
    owner: string;
    changes: {
      schedule_ref: string;
      expected: number;
      availability: string;
      next_due_unix_ms: number;
    }[];
  }
  | { action: "advance"; request: Advance }
  | {
    action: "reserve";
    request: Omit<Advance, "last_admission_error"> & { occurrence: Occurrence };
  }
  | {
    action: "settle";
    request: {
      occurrence_ref: string;
      expected: number;
      outcome: string;
      invocation_ref: string | null;
      conversation_ref: string | null;
      archived: boolean;
    };
  };
