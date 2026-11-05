include!("relationships.rs");

// Pure compatibility projection used by retained lifecycle contract tests.
fn recent_completed_root_work_refs(conversation: &RetainedConversation) -> Vec<String> {
    let mut refs = live_completion_activities(conversation)
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::WorkLifecycle {
                invocation_ref,
                parent_invocation_ref: None,
                state: crate::v2026_09::invocation::ActionInvocationState::Completed,
                ..
            } => Some(invocation_ref.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if refs.len() > 500 {
        refs.drain(..refs.len() - 500);
    }
    refs
}



pub(super) use super::backend::projection::authoritative_projection;
include!("timeline.rs");

