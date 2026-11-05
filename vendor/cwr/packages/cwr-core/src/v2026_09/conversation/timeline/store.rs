use super::*;

/// Canonical retained timelines, keyed by their owning conversation.
///
/// Invocation records remain in the invocation ledger. Timeline items retain
/// only typed references to those records or to canonical messages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ConversationTimelineStore {
    timelines: crate::journal::JournalMap<String, ConversationTimeline>,
}

impl ConversationTimelineStore {
    #[cfg(test)]
    pub(crate) fn fixture_replace(&mut self, replacement: &Self) {
        self.timelines.fixture_replace(&replacement.timelines);
    }

    pub fn is_empty(&self) -> bool {
        self.timelines.is_empty()
    }

    pub fn timeline(&self, conversation_ref: &str) -> Option<&ConversationTimeline> {
        self.timelines.get(conversation_ref)
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (&String, &ConversationTimeline)> {
        self.timelines.iter()
    }

    pub fn create(
        &mut self,
        conversation_ref: impl Into<String>,
        root_connection_ref: impl Into<String>,
    ) -> CwrResult<bool> {
        let conversation_ref = conversation_ref.into();
        let root_connection_ref = root_connection_ref.into();
        let timeline =
            ConversationTimeline::new(conversation_ref.clone(), root_connection_ref.clone())?;
        if let Some(existing) = self.timelines.get(&conversation_ref) {
            if existing.root_connection_ref == root_connection_ref {
                return Ok(false);
            }
            return invalid(format!(
                "conversation {conversation_ref} already has a different timeline"
            ));
        }
        if self
            .timelines
            .values()
            .any(|existing| existing.identity_in_use(&conversation_ref))
        {
            return invalid(format!(
                "timeline conversation {conversation_ref} is already retained by another timeline"
            ));
        }
        if self
            .timelines
            .values()
            .any(|existing| existing.identity_in_use(&root_connection_ref))
        {
            return invalid(format!(
                "timeline root connection {root_connection_ref} is already retained by another timeline"
            ));
        }
        self.timelines.insert(conversation_ref, timeline);
        Ok(true)
    }

    pub fn publish(
        &mut self,
        conversation_ref: &str,
        publication_ref: impl Into<String>,
        content: ConversationTimelineContent,
        disposition: ConversationTimelineDisposition,
    ) -> CwrResult<ConversationTimelinePublishOutcome> {
        if self.timelines.get(conversation_ref).is_none() {
            return Err(invalid_error(format!(
                "conversation {conversation_ref} has no retained timeline"
            )));
        }
        let publication_ref = publication_ref.into();
        let (item_ref, child_connection_ref) =
            ConversationTimeline::placement_refs(&publication_ref, &content);
        let disclosed = disposition != ConversationTimelineDisposition::Undisclosed;
        let conflicts = self
            .timelines
            .iter()
            .filter(|(existing_ref, _)| existing_ref.as_str() != conversation_ref)
            .any(|(_, existing)| {
                existing.identity_in_use(&publication_ref)
                    || disclosed
                        && (existing.identity_in_use(&item_ref)
                            || child_connection_ref
                                .as_deref()
                                .is_some_and(|reference| existing.identity_in_use(reference))
                            || existing.structural_identity_in_use(content.reference()))
            });
        if conflicts {
            return invalid(
                "timeline publication would reuse an identity retained by another timeline",
            );
        }
        self.timelines
            .get_mut(conversation_ref)
            .expect("timeline existence checked before identity validation")
            .publish(publication_ref, content, disposition)
    }

    /// Only the streaming lifecycle may withdraw unpublished message placements.
    pub(crate) fn withdraw_streaming(
        &mut self,
        conversation: &str,
        reference: &str,
    ) -> CwrResult<()> {
        let Some(timeline) = self.timelines.get_mut(conversation) else {
            return Ok(());
        };
        for connection in &mut timeline.connections {
            let before = connection.items.len();
            connection
                .items
                .retain(|item| item.content.reference() != reference);
            let removed = u64::try_from(before - connection.items.len())?;
            connection.revision = connection
                .revision
                .checked_add(removed)
                .ok_or("timeline revision overflow")?;
        }
        Ok(())
    }

    pub(crate) fn begin(&mut self) {
        self.timelines.begin();
    }

    pub(crate) fn commit(&mut self) {
        self.timelines.commit();
    }

    pub(crate) fn rollback(&mut self) {
        self.timelines.rollback();
    }

    pub(crate) fn changes(
        &self,
    ) -> impl Iterator<
        Item = (
            &String,
            Option<&ConversationTimeline>,
            Option<&ConversationTimeline>,
        ),
    > {
        self.timelines.changes()
    }
}

impl From<BTreeMap<String, ConversationTimeline>> for ConversationTimelineStore {
    fn from(timelines: BTreeMap<String, ConversationTimeline>) -> Self {
        Self {
            timelines: timelines.into(),
        }
    }
}

impl ConversationAdmissionStore {
    pub fn retained_timeline(&self, conversation_ref: &str) -> Option<&ConversationTimeline> {
        self.state.timeline_store.timeline(conversation_ref)
    }

    /// Creates an authoritative empty timeline without changing any existing
    /// message or invocation writer.
    pub fn create_retained_timeline(
        &mut self,
        conversation_ref: &str,
        root_connection_ref: &str,
    ) -> CwrResult<bool> {
        if self
            .state
            .timeline_store
            .timeline(conversation_ref)
            .is_none()
        {
            validate_absolute_ref("timeline root connection", root_connection_ref)?;
            let existing_record = self
                .backend
                .lock()
                .map_err(|_| invalid_error("conversation backend lock poisoned"))?
                .identity_in_use(root_connection_ref)?;
            if root_connection_ref == self.state.runtime_root_ref || existing_record {
                return invalid(
                    "timeline root connection is already retained by another CWPDB record",
                );
            }
        }
        self.transact(|state| {
            let index = state
                .conversations
                .iter()
                .position(|conversation| conversation.conversation_ref == conversation_ref)
                .ok_or_else(|| invalid_error("unknown conversation for retained timeline"))?;
            let conversation = &state.conversations[index];
            if conversation.parent_conversation_ref.is_some() {
                return invalid("private execution conversations cannot own a shared timeline");
            }
            if conversation
                .timeline_connection_ref
                .as_deref()
                .is_some_and(|existing| existing != root_connection_ref)
            {
                return invalid("conversation already names another retained timeline");
            }
            let created = state
                .timeline_store
                .create(conversation_ref, root_connection_ref)?;
            state.conversations[index].timeline_connection_ref =
                Some(root_connection_ref.to_owned());
            Ok(created)
        })
    }

    pub fn publish_timeline_message(
        &mut self,
        conversation_ref: &str,
        publication_ref: &str,
        message_ref: &str,
        disposition: ConversationTimelineDisposition,
    ) -> CwrResult<ConversationTimelinePublishOutcome> {
        if disposition == ConversationTimelineDisposition::Undisclosed {
            return invalid(
                "message timeline publications cannot be undisclosed without a durable decision",
            );
        }
        self.transact(|state| {
            let conversation = state
                .conversations
                .iter()
                .find(|conversation| conversation.conversation_ref == conversation_ref)
                .ok_or_else(|| invalid_error("unknown conversation for timeline publication"))?;
            if !conversation
                .messages
                .iter()
                .any(|message| message.message_ref == message_ref)
            {
                return invalid("timeline message is not retained by its conversation");
            }
            state.timeline_store.publish(
                conversation_ref,
                publication_ref,
                ConversationTimelineContent::Message {
                    message_ref: message_ref.to_owned(),
                },
                disposition,
            )
        })
    }

    /// Retains an invocation's causal source, disclosure decision, and optional
    /// timeline placement in one CWPDB transaction.
    pub fn retain_invocation_timeline_publication(
        &mut self,
        invocation_ref: &str,
        source_message_ref: &str,
        publication_ref: &str,
        disposition: ConversationTimelineDisposition,
    ) -> CwrResult<ConversationTimelinePublishOutcome> {
        self.transact(|state| {
            let conversation_ref = state
                .invocation_ledger
                .invocation(invocation_ref)
                .and_then(|record| record.conversation_ref.clone())
                .ok_or_else(|| {
                    invalid_error("invocation has no conversation for timeline publication")
                })?;
            if retained_invocation_source_message_ref(state, invocation_ref)? != source_message_ref
            {
                return invalid("invocation source message does not match its retained turn start");
            }
            let conversation = state
                .conversations
                .iter()
                .find(|conversation| conversation.conversation_ref == conversation_ref)
                .ok_or_else(|| invalid_error("invocation conversation is unavailable"))?;
            if !conversation
                .messages
                .iter()
                .any(|message| message.message_ref == source_message_ref)
            {
                return invalid("invocation source message is outside its conversation");
            }
            if state.timeline_store.timeline(&conversation_ref).is_none() {
                return invalid("invocation conversation has no retained timeline");
            }
            state.invocation_ledger.retain_timeline_publication(
                invocation_ref,
                source_message_ref,
                publication_ref,
                disposition.clone(),
            )?;
            state.timeline_store.publish(
                &conversation_ref,
                publication_ref,
                ConversationTimelineContent::Invocation {
                    invocation_ref: invocation_ref.to_owned(),
                },
                disposition,
            )
        })
    }
}
