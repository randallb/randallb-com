use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{
    ActionInvocationRecord, ActionInvocationState, ActionResponse, ActionResponseDetail,
    ActionResponseStatus,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InvocationWaitState {
    Pending,
    Ready,
    Resuming,
    Consumed,
    Abandoned,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvocationWait {
    pub waiter_invocation_ref: String,
    pub awaited_invocation_ref: String,
    pub call_id: String,
    pub state: InvocationWaitState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<ActionResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_after_activity_sequence: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InvocationStopRequest {
    pub requested_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by_actor_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl InvocationStopRequest {
    pub fn new(requested_at_unix_ms: u64) -> Self {
        Self {
            requested_at_unix_ms,
            requested_by_actor_ref: None,
            request_id: None,
            reason: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InvocationTerminal {
    pub status: ActionResponseStatus,
    pub summary: String,
    pub result: Option<Value>,
    pub error: Option<ActionResponseDetail>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StartInvocationOutcome {
    Started(ActionInvocationRecord),
    Existing(ActionInvocationRecord),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActionInvocationClaim {
    pub invocation: ActionInvocationRecord,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ClaimInvocationOutcome {
    Claimed(Box<ActionInvocationClaim>),
    NotRunnable {
        state: ActionInvocationState,
        attempt_active: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FinishInvocationOutcome {
    Accepted {
        response: ActionResponse,
        woken_invocation_refs: Vec<String>,
    },
    AlreadyTerminal {
        response: ActionResponse,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum StopInvocationOutcome {
    Accepted {
        response: ActionResponse,
        stopped_invocation_refs: Vec<String>,
        woken_invocation_refs: Vec<String>,
        /// True when the same durable stop request was already accepted.
        replayed: bool,
    },
    TooLate {
        response: ActionResponse,
    },
    Unknown,
}
