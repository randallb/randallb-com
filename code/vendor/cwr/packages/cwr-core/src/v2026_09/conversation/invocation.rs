use std::collections::BTreeSet;

use serde_json::Value;

use super::*;
use crate::v2026_09::invocation::{
    ActionInvocationClaim, ActionInvocationRecord, ActionInvocationState, ActionResponse,
    ActionResponseStatus, ClaimInvocationOutcome, FinishInvocationOutcome, InvocationOrigin,
    InvocationStopRequest, InvocationTerminal, InvocationWait, StartActionInvocation,
    StartInvocationOutcome, StopInvocationOutcome,
};

/// One durably claimed invocation together with the retained adapter input
/// needed to launch or resume its next attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedConversationInvocation {
    pub claim: ActionInvocationClaim,
    pub model_run_ref: String,
    pub adapter_invocation: AdapterInvocation,
    pub conversation: RetainedConversation,
    pub execution_conversation: RetainedConversation,
    pub source_message_ref: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConversationInvocationClaimOutcome {
    Claimed(Box<ClaimedConversationInvocation>),
    NotRunnable {
        state: ActionInvocationState,
        attempt_active: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum InvocationStopSelection {
    Selected(String),
    None,
    Ambiguous(Vec<String>),
}

include!("invocation/selection.rs");
include!("invocation/control.rs");
include!("invocation/children.rs");
include!("invocation/progress.rs");
include!("invocation/lifecycle.rs");
include!("invocation/commentary.rs");
include!("invocation/root.rs");
include!("invocation/execution.rs");

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use serde_json::json;

    use super::super::*;
    use super::validate_invocation_execution_links;
    use crate::{
        adapter::AdapterInvocation,
        v2026_09::invocation::{
            ActionInvocationState, ActionResponseDetail, ActionResponseStatus,
            FinishInvocationOutcome, InvocationOrigin, InvocationStopRequest, InvocationTerminal,
            StartActionInvocation, StopInvocationOutcome,
        },
    };

    const RUNTIME_ROOT: &str = "cwp://com.example.runtime";
    const CONVERSATION_REF: &str = "cwp://com.example.runtime/conversations/root";
    const EXECUTION_REF: &str = "cwp://com.example.runtime/conversations/root-execution";
    const SOURCE_MESSAGE_REF: &str = "cwp://com.example.runtime/messages/root";
    const ROOT_INVOCATION_REF: &str = "cwp://com.example.runtime/invocations/root";

    include!("invocation/tests/persistence.rs");
    include!("invocation/tests/migration.rs");
    include!("invocation/tests/commentary.rs");
    include!("invocation/tests/wait_recovery.rs");
    include!("invocation/tests/stop.rs");
    include!("invocation/tests/archive.rs");
    include!("invocation/tests/control.rs");
    include!("invocation/tests/diagnostics.rs");
    include!("invocation/tests/root_terminal.rs");
    include!("invocation/tests/child_terminal.rs");
    include!("invocation/tests/activity.rs");
    include!("invocation/tests/transactions.rs");
    include!("invocation/tests/timeline_contract.rs");
    include!("invocation/tests/timeline_rollout.rs");
    include!("invocation/tests/timeline_scaling.rs");
    #[path = "shared_reads.rs"]
    mod shared_reads;
}
