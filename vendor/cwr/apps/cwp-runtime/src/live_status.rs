use std::collections::BTreeMap;

use cwr::v2026_09::host::{ConversationLiveStatus, ConversationLiveStatusName};

#[derive(Debug, Default)]
pub(crate) struct LiveStatusRegistry {
    by_conversation: BTreeMap<String, BTreeMap<String, ConversationLiveStatus>>,
    last_sequence: BTreeMap<(String, String), u64>,
}

impl LiveStatusRegistry {
    pub(crate) fn snapshot(&self) -> Vec<(String, Vec<ConversationLiveStatus>)> {
        self.by_conversation
            .iter()
            .map(|(conversation_ref, statuses)| {
                (
                    conversation_ref.clone(),
                    statuses.values().cloned().collect(),
                )
            })
            .collect()
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The live-status wire update carries one complete leased identity."
    )]
    pub(crate) fn update(
        &mut self,
        conversation_ref: &str,
        subject_actor_ref: String,
        publisher_session_id: String,
        sequence: u64,
        target_actor_refs: Vec<String>,
        status: Option<ConversationLiveStatusName>,
        lease_ms: u64,
        now_unix_ms: u64,
    ) -> Option<Vec<ConversationLiveStatus>> {
        let sequence_key = (conversation_ref.to_owned(), publisher_session_id.clone());
        if self
            .last_sequence
            .get(&sequence_key)
            .is_some_and(|last| sequence <= *last)
        {
            return None;
        }
        self.last_sequence.insert(sequence_key, sequence);
        let statuses = self
            .by_conversation
            .entry(conversation_ref.to_owned())
            .or_default();
        match status {
            Some(status) => {
                statuses.insert(
                    publisher_session_id.clone(),
                    ConversationLiveStatus {
                        subject_actor_ref,
                        publisher_session_id,
                        sequence,
                        target_actor_refs,
                        status,
                        expires_at_unix_ms: now_unix_ms.saturating_add(lease_ms),
                    },
                );
            }
            None => {
                statuses.remove(&publisher_session_id);
            }
        }
        let snapshot = statuses.values().cloned().collect();
        if statuses.is_empty() {
            self.by_conversation.remove(conversation_ref);
        }
        Some(snapshot)
    }

    pub(crate) fn expire(
        &mut self,
        now_unix_ms: u64,
    ) -> Vec<(String, Vec<ConversationLiveStatus>)> {
        let mut changed = Vec::new();
        self.by_conversation.retain(|conversation_ref, statuses| {
            let before = statuses.len();
            statuses.retain(|_, status| status.expires_at_unix_ms > now_unix_ms);
            if statuses.len() != before {
                changed.push((
                    conversation_ref.clone(),
                    statuses.values().cloned().collect(),
                ));
            }
            !statuses.is_empty()
        });
        changed
    }
}
