fn event_request_id(event: &HostEvent) -> Option<&str> {
    match event {
        HostEvent::WorkspaceInspected { request_id, .. }
        | HostEvent::ActorSelected { request_id, .. }
        | HostEvent::CommandSucceeded { request_id, .. }
        | HostEvent::IntrinsicResponse { request_id, .. }
        | HostEvent::CommandFailed { request_id, .. } => Some(request_id),
        _ => None,
    }
}

fn terminal_update(
    event: &HostEvent,
    conversation_ref: &str,
    invocation_ref: &str,
) -> Option<(
    ActionResponseStatus,
    String,
    Option<String>,
    Option<ActionResponse>,
)> {
    match event {
        HostEvent::ConversationExecutionFinished {
            conversation_ref: finished_conversation_ref,
            target_actor_ref,
            response,
            ..
        } if finished_conversation_ref == conversation_ref
            && response.invocation_ref == invocation_ref =>
        {
            Some((
                response.status,
                target_actor_ref.clone(),
                Some(response.summary.clone()),
                Some((**response).clone()),
            ))
        }
        HostEvent::ConversationExecutionFailed {
            conversation_ref: failed_conversation_ref,
            invocation_ref: failed_invocation_ref,
            target_actor_ref,
            error,
            ..
        } if failed_conversation_ref == conversation_ref
            && failed_invocation_ref == invocation_ref =>
        {
            Some((
                ActionResponseStatus::FAILED,
                target_actor_ref.clone(),
                Some(error.clone()),
                None,
            ))
        }
        _ => None,
    }
}

fn unexpected(context: &str, event: &HostEvent) -> Box<dyn Error + Send + Sync> {
    format!("resident runtime returned an unexpected {context}: {event:?}").into()
}
