use serde::{Deserialize, Serialize};

use crate::{
    adapter::ExecutionPhase,
    v2026_09::{
        conversation::{ConversationMessage, WorkInvocationWait},
        invocation::{ActionInvocationState, ActionResponseStatus},
    },
};

/// A persisted read position, NOT a pagination cursor or a promise of replay.
/// The stream layer must add its own runtime epoch and transient-event position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSnapshotPosition {
    pub runtime_root_ref: String,
    pub conversation_ref: String,
    pub commit_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationCurrentSnapshot {
    pub position: ConversationSnapshotPosition,
    /// Every nonterminal invocation, including waiting parents and detached work.
    pub active_invocations: Vec<ConversationInvocation>,
}

/// One canonical timeline resource resolved through CWR's indexed
/// relationships. The enclosing position gives callers a consistent persisted
/// revision; transient stream ordering is added by the subscription owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationResourceSnapshot {
    pub position: ConversationSnapshotPosition,
    pub resource: ConversationResource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConversationResource {
    Message {
        #[serde(default, skip_serializing_if = "is_false")]
        streaming: bool,
        message: Box<ConversationMessage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        occurred_at_unix_ms: Option<i64>,
    },
    Invocation {
        invocation: Box<ConversationInvocation>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationInvocation {
    pub invocation_ref: String,
    pub conversation_ref: String,
    pub requester_actor_ref: String,
    pub target_actor_ref: String,
    pub source_message_ref: Option<String>,
    pub parent_invocation_ref: Option<String>,
    pub child_invocation_refs: Vec<String>,
    /// Explicit waits are independent of parentage. No response payloads.
    pub waits: Vec<WorkInvocationWait>,
    pub state: ActionInvocationState,
    /// Retained terminal status, even when no message was published.
    pub outcome: Option<ConversationInvocationOutcome>,
    pub admitted_at_unix_ms: u64,
    pub started_at_unix_ms: Option<u64>,
    pub finished_at_unix_ms: Option<u64>,
    pub stop_requested_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub stop_settlement_tracked: bool,
    /// The latest explicitly linked published message, retaining its real author.
    pub latest_reply: Option<ConversationMessage>,
    /// None before the first claim or after terminal completion.
    pub current_execution: Option<ConversationExecution>,
}

/// Public terminal fields only; arbitrary result and diagnostic payloads stay private.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationInvocationOutcome {
    pub status: ActionResponseStatus,
    pub summary: String,
    pub error: Option<ConversationOutcomeDetail>,
    pub cause: Option<ConversationOutcomeDetail>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationOutcomeDetail {
    pub code: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationExecution {
    /// The pair (invocation_ref, generation) identifies a claimed attempt.
    pub generation: u64,
    pub attempt_active: bool,
    pub phase: ExecutionPhase,
    pub started_at_unix_ms: Option<u64>,
    pub latest_activity_at_unix_ms: Option<u64>,
    /// Latest retained, unpublished progress. Never a published commentary item.
    pub progress: Option<String>,
    /// Transient output supplied by the runtime, never persisted by this read API.
    pub live_output: Option<ConversationExecutionOutput>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationExecutionOutput {
    pub invocation_ref: String,
    pub generation: u64,
    pub target_actor_ref: String,
    pub phase: ExecutionPhase,
    pub text: String,
}

impl ConversationCurrentSnapshot {
    /// Attach only output belonging to the current, actively claimed execution.
    /// An empty text clears its preview without completing the invocation.
    /// Ordering within an execution is owned by the future stream layer.
    pub fn apply_live_output(&mut self, output: ConversationExecutionOutput) -> bool {
        let Some(invocation) = self.active_invocations.iter_mut().find(|item| {
            item.invocation_ref == output.invocation_ref
                && item.target_actor_ref == output.target_actor_ref
        }) else {
            return false;
        };
        let Some(execution) = invocation.current_execution.as_mut().filter(|execution| {
            execution.attempt_active && execution.generation == output.generation
        }) else {
            return false;
        };
        execution.live_output = (!output.text.is_empty()).then_some(output);
        true
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}
