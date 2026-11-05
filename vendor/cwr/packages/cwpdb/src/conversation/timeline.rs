//! Finite canonical timeline writes; physical records and edges stay in cwpdb.
use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineContent {
    Message { message_ref: String },
    Invocation { invocation_ref: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimelineItemWrite {
    pub conversation_ref: String,
    pub connection_ref: String,
    pub item_ref: String,
    pub publication_ref: String,
    pub position: i64,
    pub content: TimelineContent,
    pub child_connection_ref: Option<String>,
}

impl TimelineItemWrite {
    pub fn record(&self) -> WriteRecord {
        WriteRecord {
            reference: self.item_ref.clone(),
            kind: RecordKind::ConversationTimelineItem,
            properties: serde_json::json!({"publication_ref":self.publication_ref}),
            owner_ref: Some(self.conversation_ref.clone()),
        }
    }

    pub fn memberships(&self) -> Vec<MembershipChange> {
        let upsert = |source: &str, kind, target: &str, position| MembershipChange::Upsert {
            source_ref: source.into(),
            kind,
            member: Member {
                target_ref: target.into(),
                position,
                properties: serde_json::json!({}),
            },
        };
        let mut changes = vec![upsert(
            &self.connection_ref,
            MembershipKind::ConversationTimelineItem,
            &self.item_ref,
            self.position,
        )];
        let (kind, target) = match &self.content {
            TimelineContent::Message { message_ref } => {
                (MembershipKind::ConversationTimelineMessage, message_ref)
            }
            TimelineContent::Invocation { invocation_ref } => (
                MembershipKind::ConversationTimelineInvocation,
                invocation_ref,
            ),
        };
        changes.push(upsert(&self.item_ref, kind, target, 0));
        if let Some(child) = &self.child_connection_ref {
            changes.push(upsert(
                &self.item_ref,
                MembershipKind::ConversationTimelineChild,
                child,
                0,
            ));
        }
        changes
    }
}
