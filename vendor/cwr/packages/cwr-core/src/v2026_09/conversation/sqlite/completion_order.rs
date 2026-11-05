// Completion order is a retained runtime fact, independent of wall clocks and
// host attention policy. Assign it in the same transaction as terminal state.
/// Only activities after the latest fork boundary belong to work done here.
fn live_completion_activities(conversation: &RetainedConversation) -> &[ConversationActivity] {
    let live_start = conversation.activity.iter().rposition(|activity| {
        matches!(activity.event, ConversationActivityEvent::ConversationForkedFrom { .. })
    }).map_or(0, |position| position + 1);
    &conversation.activity[live_start..]
}
