//! Retained publication reads share one storage-owned selection across nested requests.
use super::*;
use crate::v2026_09::conversation::{
    ConversationResource, ConversationResourceSnapshot, ConversationTimelineConnectionPage,
    ConversationTimelineItem, ConversationTimelinePageRequest,
};

/// An operation-local view, not a second timeline store. The connection is
/// read-only; dropping it releases the deferred transaction and its WAL reader.
pub struct ConversationPublicationSnapshot {
    store: ConversationReadStore,
    deadline: Instant,
    cancellation: ConversationReadCancellation,
    actor: String,
    measured_at_unix_ms: u64,
    inspector_bytes: std::cell::Cell<usize>,
    cache: cache::PublicationReadCache,
    finite: cwpdb_store::conversation::ReadSnapshot,
}
#[derive(Debug, Clone)]
pub struct ConversationPublication {
    pub conversation_ref: String,
    pub item: ConversationTimelineItem,
    pub source: Option<Box<ConversationResourceSnapshot>>,
    pub text: String,
}
#[derive(Debug, Clone)]
pub struct ConversationPublicationPage {
    pub page: ConversationTimelineConnectionPage,
    pub publications: Vec<ConversationPublication>,
    pub total_count: Option<usize>,
    pub revision: String,
}
impl ConversationPublicationSnapshot {
    #[cfg(test)]
    pub(crate) fn resource_was_loaded(&self, reference: &str) -> bool {
        self.cache.resources.contains(reference)
    }
    pub(crate) fn open(
        path: &Path,
        runtime_ref: &str,
        actor: &str,
        deadline: Instant,
        cancellation: ConversationReadCancellation,
    ) -> CwrResult<Self> {
        let store = ConversationReadStore::open_until(path, runtime_ref, deadline, &cancellation)?;
        Self::from_store(store, actor, deadline, cancellation)
    }
    pub(crate) fn from_store(
        mut store: ConversationReadStore,
        actor: &str,
        deadline: Instant,
        cancellation: ConversationReadCancellation,
    ) -> CwrResult<Self> {
        let finite = store
            .factory
            .begin(cancellation.storage_control_for_path(deadline, &store.path))?;
        Ok(Self {
            store,
            finite,
            deadline,
            cancellation,
            actor: actor.into(),
            inspector_bytes: std::cell::Cell::new(0),
            cache: Default::default(),
            measured_at_unix_ms: u64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)?
                    .as_millis(),
            )?,
        })
    }
    fn read<T>(
        &self,
        read: impl FnOnce(&ConversationReadOperation<'_>) -> CwrResult<T>,
    ) -> CwrResult<T> {
        let operation = ConversationReadOperation {
            include_execution_conversations: false,
            owner_context_filter: None,
            path: &self.store.path,
            deadline: self.deadline,
            cancellation: self.cancellation.clone(),
            finite: FiniteSnapshot::Borrowed(&self.finite),
        };
        operation.ensure_active()?;
        let value = read(&operation)?;
        operation.ensure_active()?;
        Ok(value)
    }
    pub fn conversation(&self, reference: &str) -> CwrResult<Option<(Option<String>, bool)>> {
        self.read(|op| {
            if !self.visible(op, reference)? {
                return Ok(None);
            }
            op.finite_snapshot()?
                .root_conversation_presentation(reference)
        })
    }
    /// One participant-authorized root summary from this selection's frozen view.
    pub fn conversation_summary(&self, reference: &str) -> CwrResult<Option<ConversationSummary>> {
        self.read(|op| {
            if !self.visible(op, reference)? {
                return Ok(None);
            }
            op.root_conversation_summary(reference)
        })
    }
    /// Lineage is readable only through the fork's participant-authorized view.
    pub fn fork_source(&self, reference: &str) -> CwrResult<Option<(String, String)>> {
        self.read(|op| {
            if !self.visible(op, reference)? {
                return Ok(None);
            }
            op.finite_snapshot()?.fork_source(reference)
        })
    }
    /// Latest addressed message by the requesting actor, independent of the displayed page.
    pub fn last_addressed_actor_refs(&self, conversation: &str) -> CwrResult<Vec<String>> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return Ok(Vec::new());
            }
            let Some(root) = op.timeline_root_ref(conversation)? else {
                return Ok(Vec::new());
            };
            op.canonical_last_addressed_actor_refs(conversation, &root, &self.actor)
        })
    }
    pub fn owner_context_ref(&self, conversation: &str) -> CwrResult<Option<String>> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return invalid("conversation is unavailable");
            }
            op.finite_snapshot()?.owner_context_ref(conversation)
        })
    }
    pub fn conversations(
        &self,
        archived: Option<bool>,
        request: &ConversationSummaryPageRequest,
    ) -> CwrResult<ConversationSummaryPage> {
        self.read(|op| {
            let scoped = ConversationReadOperation {
                include_execution_conversations: false,
                owner_context_filter: request.owner_filter(),
                path: op.path,
                deadline: op.deadline,
                cancellation: op.cancellation.clone(),
                finite: FiniteSnapshot::Borrowed(&self.finite),
            };
            scoped.participant_root_conversations(&self.actor, archived, request.window())
        })
    }
    pub fn legacy_timeline(
        &self,
        conversation: &str,
        request: &ConversationRecordPageRequest,
    ) -> CwrResult<ConversationTimelinePage> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return invalid("conversation is unavailable to the requesting participant");
            }
            op.conversation_timeline(conversation, request.window())
        })
    }
    pub fn resource(
        &self,
        reference: &str,
    ) -> CwrResult<Option<Box<ConversationResourceSnapshot>>> {
        self.read(|op| self.resource_in(op, reference))
    }
    /// The canonical immutable resource object within this actor-bound selection.
    pub fn resource_shared(
        &self,
        reference: &str,
    ) -> CwrResult<Option<std::sync::Arc<ConversationResourceSnapshot>>> {
        self.read(|op| self.shared_resource_in(op, reference))
    }
    fn resource_in(
        &self,
        op: &ConversationReadOperation<'_>,
        reference: &str,
    ) -> CwrResult<Option<Box<ConversationResourceSnapshot>>> {
        // Preserve the owned DTO API for callers that consume its fields.
        Ok(self
            .shared_resource_in(op, reference)?
            .map(|value| Box::new((*value).clone())))
    }
    fn shared_resource_in(
        &self,
        op: &ConversationReadOperation<'_>,
        reference: &str,
    ) -> CwrResult<Option<std::sync::Arc<ConversationResourceSnapshot>>> {
        self.cache.resources.get_or_load(reference, || {
            let resource = op.conversation_resource(reference)?;
            match resource {
                Some(value) if self.visible(op, &value.position.conversation_ref)? => {
                    Ok(Some(std::sync::Arc::new(value)))
                }
                _ => Ok(None),
            }
        })
    }
    fn visible(&self, op: &ConversationReadOperation<'_>, conversation: &str) -> CwrResult<bool> {
        op.ensure_active()?;
        if !canonical_lookup_reference(conversation) {
            return Ok(false);
        }
        self.cache.conversations.get_or_load(conversation, || {
            op.finite_snapshot()?
                .participant_visible(conversation, &self.actor)
        })
    }
    pub fn page(
        &self,
        conversation: &str,
        connection: Option<&str>,
        request: &ConversationTimelinePageRequest,
        count: bool,
    ) -> CwrResult<Option<ConversationPublicationPage>> {
        self.read(|op| {
            if !self.visible(op, conversation)? {
                return Ok(None);
            }
            let Some(root) = op.timeline_root_ref(conversation)? else {
                return Ok(None);
            };
            let connection = connection.unwrap_or(&root);
            let page = op.conversation_timeline_connection(conversation, connection, request)?;
            let publications = page
                .edges
                .iter()
                .map(|edge| self.project(op, conversation, edge.item.clone()))
                .collect::<CwrResult<Vec<_>>>()?;
            let total_count = count
                .then(|| op.conversation_timeline_connection_count(conversation, connection))
                .transpose()?;
            // The persisted conversation view also changes when reachable source data changes.
            let revision = format!(
                "{}:{}",
                connection,
                op.snapshot_position(conversation)?.commit_revision
            );
            Ok(Some(ConversationPublicationPage {
                page,
                publications,
                total_count,
                revision,
            }))
        })
    }
    pub fn item(&self, reference: &str) -> CwrResult<Option<ConversationPublication>> {
        self.read(|op| {
            // Climb retained ownership; knowing a ref does not bypass participation.
            let conversation = op
                .finite_snapshot()?
                .timeline_item_conversation(reference)?;
            let Some(conversation) = conversation else {
                return Ok(None);
            };
            if !self.visible(op, &conversation)? {
                return Ok(None);
            }
            op.conversation_timeline_item(&conversation, reference)?
                .map(|item| self.project(op, &conversation, item))
                .transpose()
        })
    }
    fn project(
        &self,
        op: &ConversationReadOperation<'_>,
        conversation: &str,
        item: ConversationTimelineItem,
    ) -> CwrResult<ConversationPublication> {
        let source = self.resource_in(op, item.content.reference())?;
        let text = match source.as_ref().map(|value| &value.resource) {
            Some(ConversationResource::Message { message, .. }) => message.text.clone(),
            Some(ConversationResource::Invocation { invocation }) => invocation
                .outcome
                .as_ref()
                .map(|outcome| outcome.summary.clone())
                .unwrap_or_else(|| {
                    format!(
                        "Work {}",
                        serde_json::to_value(invocation.state)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                            .unwrap_or_default()
                    )
                }),
            None => "Content unavailable".into(),
        };
        Ok(ConversationPublication {
            conversation_ref: conversation.into(),
            item,
            source,
            text,
        })
    }
}
mod filtered;
pub use filtered::*;
mod legacy;

mod metadata;
pub use metadata::*;

mod attachments;
pub use attachments::*;

mod actors;
pub use actors::ConversationActorFilter;

mod inspector;
pub use inspector::*;

mod controls;
pub use controls::*;

mod cache;
