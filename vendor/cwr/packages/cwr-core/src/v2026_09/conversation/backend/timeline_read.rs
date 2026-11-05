//! Protocol projection of one storage-owned timeline snapshot.
use super::*;
pub(crate) struct CanonicalTimelineReader {
    backend: std::sync::Arc<std::sync::Mutex<ConversationBackend>>,
    #[cfg(any(test, feature = "test-read-metrics"))]
    path: std::path::PathBuf,
}
impl ConversationAdmissionStore {
    pub(crate) fn canonical_timeline_reader(&self) -> CanonicalTimelineReader {
        CanonicalTimelineReader {
            backend: self.backend.clone(),
            #[cfg(any(test, feature = "test-read-metrics"))]
            path: self.read_store_identity().0,
        }
    }
    #[cfg(test)]
    pub(crate) fn canonical_timeline_page(
        &self,
        reference: &str,
        page: &ConversationRecordPageRequest,
        participant: Option<&str>,
        include_execution: bool,
        control: &ReadControl,
    ) -> CwrResult<ConversationTimelinePage> {
        self.canonical_timeline_reader().page(
            reference,
            page,
            participant,
            include_execution,
            control,
        )
    }
}
impl CanonicalTimelineReader {
    pub(crate) fn page(
        &self,
        reference: &str,
        page: &ConversationRecordPageRequest,
        participant: Option<&str>,
        include_execution: bool,
        control: &ReadControl,
    ) -> CwrResult<ConversationTimelinePage> {
        CwrRef::parse(reference)?;
        page.validate()?;
        control.check()?;
        #[cfg(any(test, feature = "test-read-metrics"))]
        let control = {
            let mut observed = control.clone();
            observed.vm_steps = super::super::read::step_measurement::current_for_path(&self.path);
            observed
        };
        #[cfg(any(test, feature = "test-read-metrics"))]
        let control = &control;
        let mut backend = loop {
            control.check()?;
            match self.backend.try_lock() {
                Ok(backend) => break backend,
                Err(std::sync::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    return invalid("conversation backend lock poisoned")
                }
            }
        };
        let scope = backend.scope.clone();
        let cursor = |key: &ConversationPositionKey| PositionCursor {
            position: key.position,
            revision: key.revision.clone(),
        };
        let remaining = control
            .deadline
            .saturating_duration_since(std::time::Instant::now());
        let deadline = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .checked_add(remaining)
            .ok_or("read deadline overflow")?;
        let request = TimelineReadRequest {
            scope: scope.clone(),
            conversation_ref: reference.into(),
            after: page.after.as_ref().map(cursor),
            before: page.before.as_ref().map(cursor),
            first: page.first,
            last: page.last,
            participant_actor_ref: participant.map(str::to_owned),
            include_execution,
            deadline_unix_ms: i64::try_from(deadline.as_millis())?,
            max_bytes: MAX_WIRE_BYTES,
        };
        let ConversationBackend {
            storage,
            authorizer,
            ..
        } = &mut *backend;
        let facts = storage
            .timeline_page(&request, control, authorizer.as_ref())?
            .ok_or_else(|| invalid_error(format!("unknown conversation ref {reference}")))?;
        if facts.runtime_root_ref != scope.runtime_root_ref
            || facts.conversation_ref != reference
            || facts.commit_revision.parse::<i64>().is_err()
        {
            return invalid("timeline page snapshot identity mismatch");
        }
        if (!include_execution && facts.conversation.parent_ref.is_some())
            || participant.is_some_and(|actor| {
                !facts
                    .conversation
                    .participant_actor_refs
                    .iter()
                    .any(|p| p == actor)
            })
        {
            return invalid("conversation is unavailable to the requesting participant");
        }
        let anchors = facts
            .anchors
            .into_iter()
            .map(|anchor| match anchor {
                TimelineAnchorFacts::Message {
                    position,
                    message_ref,
                } => ConversationTimelineAnchor::Message {
                    key: ConversationPositionKey {
                        position,
                        revision: Some(facts.revision.clone()),
                    },
                    message_ref,
                },
                TimelineAnchorFacts::Turn {
                    position,
                    invocation_ref,
                } => ConversationTimelineAnchor::Turn {
                    key: ConversationPositionKey {
                        position,
                        revision: Some(facts.revision.clone()),
                    },
                    invocation_ref,
                },
            })
            .collect();
        control.check()?;
        Ok(ConversationTimelinePage {
            anchors,
            messages: facts
                .messages
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()?,
            activity: facts
                .activity
                .into_iter()
                .map(serde_json::from_value)
                .collect::<Result<_, _>>()?,
            has_next_page: facts.has_next_page,
            has_previous_page: facts.has_previous_page,
            total_count: usize::try_from(facts.total_count)?,
            revision: facts.revision,
        })
    }
}
