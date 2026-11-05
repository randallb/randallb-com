//! Typed row metadata projected from the actor-authorized runtime summary.
use async_graphql::{SimpleObject, ID};
use cwr::v2026_09::conversation::ConversationSummary;

#[derive(SimpleObject)]
pub(super) struct ConversationRowSummary {
    last_message_ref: Option<ID>,
    last_message_preview: Option<String>,
    completed_work_refs: Vec<ID>,
    completed_work: Vec<ConversationCompletion>,
    revision: String,
}
#[derive(SimpleObject)]
struct ConversationCompletion {
    invocation_ref: ID,
    sequence: i64,
}
impl From<ConversationSummary> for ConversationRowSummary {
    fn from(value: ConversationSummary) -> Self {
        Self {
            last_message_ref: value.last_message_ref.map(ID),
            last_message_preview: value.last_message_preview,
            completed_work_refs: value.completed_work_refs.into_iter().map(ID).collect(),
            completed_work: value
                .completed_work
                .into_iter()
                .map(|item| ConversationCompletion {
                    invocation_ref: ID(item.invocation_ref),
                    sequence: item.sequence,
                })
                .collect(),
            revision: value.revision,
        }
    }
}
