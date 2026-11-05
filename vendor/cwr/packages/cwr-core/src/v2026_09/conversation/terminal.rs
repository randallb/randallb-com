use super::*;
use crate::v2026_09::invocation::{
    ActionInvocationState, ActionResponse, ActionResponseDetail, ActionResponseStatus,
    ClaimInvocationOutcome, FinishInvocationOutcome, InvocationOrigin, InvocationStopRequest,
    InvocationTerminal, StopInvocationOutcome,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PublishInvocationTerminalOutcome {
    Accepted {
        response: ActionResponse,
        message: Option<ConversationMessage>,
        woken_invocation_refs: Vec<String>,
    },
    AlreadyTerminal {
        response: ActionResponse,
        message: Option<ConversationMessage>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PublishInvocationStopOutcome {
    Accepted {
        response: ActionResponse,
        message: Option<ConversationMessage>,
        stopped_invocation_refs: Vec<String>,
        woken_invocation_refs: Vec<String>,
    },
    TooLate {
        response: ActionResponse,
        message: Option<ConversationMessage>,
    },
    Unknown,
}

enum RootReplyKind {
    None,
    Message,
    CompletedAction(Value),
    FailedAction,
}

include!("terminal/root.rs");
include!("terminal/child.rs");
include!("terminal/legacy.rs");
include!("terminal/existing.rs");
include!("terminal/reconciliation.rs");
include!("terminal/projection.rs");
include!("terminal/status.rs");

include!("terminal/suspension_migration.rs");

include!("terminal/archive.rs");
