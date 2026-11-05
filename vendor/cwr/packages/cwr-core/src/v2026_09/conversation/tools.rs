//! Canonical tool lifecycle accepted at the execution boundary. Retained provider
//! events are evidence; inspector reads never reconstruct operations from them.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct ToolAttempt {
    pub reference: String,
    pub invocation_ref: String,
    pub attempt: u64,
    pub capture_version: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(super) struct ToolOperation {
    pub invocation_ref: String,
    pub attempt_ref: String,
    pub provider: String,
    pub provider_item_id: String,
    #[serde(default)]
    pub host_call_id: Option<String>,
    pub first_sequence: u64,
    pub sequence: u64,
    pub activity: WorkActivity,
}
pub(super) fn begin_attempt(
    state: &mut ConversationRuntimeState,
    invocation: &str,
    attempt: u64,
    capture: bool,
) {
    // Use the existing canonical-timeline rollout boundary. Manual timeline
    // owners opt in by explicitly disclosing this invocation as well.
    let enabled = capture
        && state.timeline_store.iter().any(|(_, timeline)| {
            timeline
                .connections
                .iter()
                .flat_map(|c| &c.items)
                .any(|item| {
                    matches!(&item.content,
        ConversationTimelineContent::Invocation { invocation_ref } if invocation_ref == invocation)
                })
        });
    let reference = crate::cwpdb::derived_ref("tool_attempt", &[invocation, &attempt.to_string()]);
    state.tool_attempts.insert(
        reference.clone(),
        ToolAttempt {
            reference,
            invocation_ref: invocation.into(),
            attempt,
            capture_version: u32::from(enabled),
        },
    );
}
pub(super) fn accept(
    state: &mut ConversationRuntimeState,
    invocation: &str,
    event: &ConversationActivity,
) -> CwrResult<()> {
    let Some(claim) = state.invocation_ledger.invocation(invocation) else {
        return Ok(());
    };
    if !claim.attempt_active {
        return Ok(());
    }
    let attempt_ref = crate::cwpdb::derived_ref(
        "tool_attempt",
        &[invocation, &claim.attempt_count.to_string()],
    );
    if state
        .tool_attempts
        .get(&attempt_ref)
        .is_none_or(|a| a.capture_version != 1)
    {
        return Ok(());
    }
    if let ConversationActivityEvent::HostActionStarted {
        call_id,
        nested_invocation_ref,
        ..
    } = &event.event
    {
        let updates: Vec<_> = state
            .tool_operations
            .iter()
            .filter(|(_, operation)| {
                operation.attempt_ref == attempt_ref
                    && operation.host_call_id.as_deref() == Some(call_id)
            })
            .map(|(reference, operation)| (reference.clone(), operation.clone()))
            .collect();
        for (reference, mut operation) in updates {
            operation.activity.child_invocation_ref = Some(nested_invocation_ref.clone());
            operation.sequence = event.sequence;
            state.tool_operations.insert(reference, operation);
        }
        return Ok(());
    }
    let ConversationActivityEvent::ProviderEvent { provider, data, .. } = &event.event else {
        return Ok(());
    };
    let Some(id) = data.pointer("/item/id").and_then(Value::as_str) else {
        return Ok(());
    };
    let reference = crate::cwpdb::derived_ref("tool_operation", &[&attempt_ref, provider, id]);
    let previous = state.tool_operations.get(&reference);
    if previous.is_some_and(|p| p.activity.state != "running")
        && matches!(&event.event, ConversationActivityEvent::ProviderEvent {method, ..} if method=="item/started")
    {
        return Ok(());
    }
    let mut working = BTreeMap::new();
    if let Some(previous) = previous {
        working.insert(id.to_owned(), previous.activity.clone());
    }
    let Some(mut activity) =
        crate::v2026_09::host::project_tool_activity(event, None, &mut working)
    else {
        return Ok(());
    };
    let host_call_id = matches!(
        data.pointer("/item/type").and_then(Value::as_str),
        Some("dynamicToolCall" | "mcpToolCall")
    )
    .then(|| {
        data.pointer("/item/callId")
            .or_else(|| data.pointer("/item/call_id"))
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned()
    });
    if let Some(call_id) = &host_call_id {
        let child = state
            .invocation_ledger
            .invocations()
            .find_map(|record| match &record.origin {
                crate::v2026_09::invocation::InvocationOrigin::HostCall {
                    caller_invocation_ref,
                    call_id: candidate,
                } if caller_invocation_ref == invocation && candidate == call_id => {
                    Some(record.invocation_ref.clone())
                }
                _ => None,
            });
        if child.is_some() {
            activity.child_invocation_ref = child;
        }
    }
    activity.activity_ref = reference.clone();
    let first_sequence = previous.map_or(event.sequence, |p| p.first_sequence);
    state.tool_operations.insert(
        reference,
        ToolOperation {
            invocation_ref: invocation.into(),
            attempt_ref,
            provider: provider.clone(),
            provider_item_id: id.into(),
            host_call_id,
            first_sequence,
            sequence: event.sequence,
            activity,
        },
    );
    Ok(())
}
