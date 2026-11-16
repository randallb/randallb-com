/// A live synchronous host call keeps its parent attempt running. Its retained
/// call, child contract, and result tell us who is actually executing.
fn synchronous_parent<'a>(
    store: &'a ConversationAdmissionStore,
    child: &'a crate::v2026_09::invocation::ActionInvocationRecord,
) -> Option<(&'a str, &'a str)> {
    synchronous_parent_from(
        child,
        |invocation_ref| store.execution_conversation(invocation_ref),
        |invocation_ref| store.invocation_record(invocation_ref).is_some(),
    )
}

pub(crate) fn synchronous_parent_from<'record, 'conversation>(
    child: &'record crate::v2026_09::invocation::ActionInvocationRecord,
    execution_conversation: impl Fn(&str) -> Option<&'conversation RetainedConversation>,
    invocation_exists: impl Fn(&str) -> bool,
) -> Option<(&'record str, &'record str)> {
    let InvocationOrigin::HostCall {
        caller_invocation_ref,
        call_id,
    } = &child.origin
    else {
        return None;
    };
    let execution = execution_conversation(caller_invocation_ref)?;
    let operation = execution
        .activity
        .iter()
        .find_map(|item| match &item.event {
            ConversationActivityEvent::HostCall {
                call_id: id,
                operation,
                ..
            } if id == call_id => Some(operation.as_str()),
            _ => None,
        })?;
    if operation == "cwr.invoke_action" {
        let asynchronous = execution_conversation(&child.invocation_ref)
            .is_some_and(|execution| {
                execution.activity.iter().any(|item| matches!(
            &item.event, ConversationActivityEvent::TurnStarted { invocation: Some(snapshot), .. }
                if snapshot.get("response_mode").and_then(Value::as_str) == Some("async")
        ))
            });
        if asynchronous {
            return None;
        }
    } else if operation != "cwr.send_message" {
        return None;
    }
    (invocation_exists(caller_invocation_ref)
        && execution.activity.iter().any(|item| {
            matches!(
                &item.event,
                ConversationActivityEvent::HostCall { call_id: id, .. } if id == call_id
            )
        }))
    .then_some((caller_invocation_ref.as_str(), call_id.as_str()))
}

fn parent_waits_for_child(store: &ConversationAdmissionStore, parent: &str) -> bool {
    store.invocation_records().any(|child| !child.state.is_terminal()
        && synchronous_parent(store, child).is_some_and(|(caller, call_id)| caller == parent
            && store.execution_conversation(parent).is_some_and(|execution| !execution.activity.iter().any(|item| matches!(
                &item.event, ConversationActivityEvent::HostResult { call_id: id, .. } if id == call_id
            )))))
}
