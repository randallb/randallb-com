fn reconstruct_claimed_invocation(
    state: &ConversationRuntimeState,
    claim: ActionInvocationClaim,
    model_run_ref: String,
) -> CwrResult<ClaimedConversationInvocation> {
    let conversation_ref = claim
        .invocation
        .conversation_ref
        .as_deref()
        .ok_or_else(|| invalid_error("claimed invocation has no conversation ref"))?;
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .cloned()
        .ok_or_else(|| invalid_error("claimed invocation conversation is unavailable"))?;
    let execution_conversation =
        execution_conversation_in_state(state, &claim.invocation.invocation_ref)?.clone();
    let (source_message_ref, snapshot) = execution_conversation
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref,
                source_message_ref,
                invocation: Some(snapshot),
            } if invocation_ref == &claim.invocation.invocation_ref => {
                Some((source_message_ref.clone(), snapshot.clone()))
            }
            _ => None,
        })
        .ok_or_else(|| invalid_error("claimed invocation has no retained adapter snapshot"))?;
    let mut adapter_invocation: AdapterInvocation = serde_json::from_value(snapshot)?;
    adapter_invocation.model_run_ref = Some(model_run_ref.clone());
    adapter_invocation.thread = context::restore(state, &adapter_invocation.thread)?;
    if let Some(checkpoint) = context::checkpoint(&execution_conversation, &adapter_invocation)? {
        adapter_invocation.continuation = Some(checkpoint);
    }
    Ok(ClaimedConversationInvocation {
        claim,
        model_run_ref,
        adapter_invocation,
        conversation,
        execution_conversation,
        source_message_ref,
    })
}

fn require_execution_snapshot<'a>(
    state: &'a ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<&'a RetainedConversation> {
    let conversation = execution_conversation_in_state(state, invocation_ref)?;
    let snapshot = conversation
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref: started_ref,
                invocation: Some(snapshot),
                ..
            } if started_ref == invocation_ref => Some(snapshot),
            _ => None,
        });
    let snapshot = snapshot.ok_or_else(|| {
        invalid_error(format!(
            "invocation {invocation_ref} execution conversation has no turn start snapshot"
        ))
    })?;
    let invocation: AdapterInvocation = serde_json::from_value(snapshot.clone())?;
    if invocation.invocation_ref != invocation_ref {
        return invalid(format!(
            "invocation {invocation_ref} execution snapshot has a different ref"
        ));
    }
    Ok(conversation)
}

pub(super) fn execution_conversation_in_state<'a>(
    state: &'a ConversationRuntimeState,
    invocation_ref: &str,
) -> CwrResult<&'a RetainedConversation> {
    let execution_ref = state
        .invocation_execution_conversation_refs
        .get(invocation_ref)
        .map(String::as_str)
        .or_else(|| {
            state
                .admissions
                .iter()
                .find(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
                .and_then(|record| record.execution_conversation_ref.as_deref())
        })
        .ok_or_else(|| {
            invalid_error(format!(
                "invocation {invocation_ref} has no execution conversation"
            ))
        })?;
    state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == execution_ref)
        .ok_or_else(|| invalid_error("invocation execution conversation is unavailable"))
}

pub(super) fn push_execution_activity_in_state(
    state: &mut ConversationRuntimeState,
    invocation_ref: &str,
    event: ConversationActivityEvent,
    occurred_at_unix_ms: u64,
) -> CwrResult<ConversationActivity> {
    let execution_ref = state
        .invocation_execution_conversation_refs
        .get(invocation_ref)
        .map(String::as_str)
        .or_else(|| {
            state
                .admissions
                .iter()
                .find(|record| record.invocation_ref.as_deref() == Some(invocation_ref))
                .and_then(|record| record.execution_conversation_ref.as_deref())
        })
        .ok_or_else(|| {
            invalid_error(format!(
                "invocation {invocation_ref} has no execution conversation"
            ))
        })?
        .to_owned();
    let activity_ref = new_ref(&state.runtime_root_ref, "activities");
    let execution = state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == execution_ref)
        .ok_or_else(|| invalid_error("invocation execution conversation is unavailable"))?;
    let activity = ConversationActivity {
        activity_ref,
        sequence: execution.activity.len() as u64 + 1,
        occurred_at_unix_ms,
        event,
    };
    execution.activity.push(activity.clone());
    super::tools::accept(state, invocation_ref, &activity)?;
    Ok(activity)
}

impl ConversationAdmissionStore {
    /// Trusted host read of the already-retained execution input. This never
    /// claims, admits, replays, or advances an invocation.
    pub(crate) fn retained_cloud_execution_input(
        &self,
        invocation_ref: &str,
    ) -> CwrResult<AdapterInvocation> {
        let execution = require_execution_snapshot(&self.state, invocation_ref)?;
        let snapshot = execution
            .activity
            .iter()
            .find_map(|activity| match &activity.event {
                ConversationActivityEvent::TurnStarted {
                    invocation_ref: reference,
                    invocation: Some(snapshot),
                    ..
                } if reference == invocation_ref => Some(snapshot.clone()),
                _ => None,
            })
            .ok_or("Missing retained cloud invocation snapshot")?;
        let mut invocation: AdapterInvocation = serde_json::from_value(snapshot)?;
        invocation.thread = context::restore(&self.state, &invocation.thread)?;
        Ok(invocation)
    }
}
