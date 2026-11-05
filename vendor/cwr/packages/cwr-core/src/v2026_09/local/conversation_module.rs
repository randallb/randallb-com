use crate::v2026_09::conversation::{ConversationReadOperation, ConversationTimelineConnectionPage, ConversationTimelinePageRequest};

/// The service exported by the independently importable conversations module.
pub const CONVERSATIONS_SERVICE_REF: &str = "cwp://org.coworkerprotocol/cwr/services/conversations";

/// A resolved native service binding. Importing declarations alone does not
/// create this binding or grant access to any retained conversation.
pub struct LocalConversationModule<'a> {
    runtime: &'a LocalRuntime,
    caller: String,
    ingress: String,
}

impl LocalRuntime {
    /// Bind CWR's bundled module for a trusted host that supplies authenticated
    /// actor and ingress identity. This does not change conversation authority.
    /// Product hosts use this when the user's pack predates explicit imports.
    pub fn bundled_conversation_module_as(
        &self,
        ingress: &str,
        caller: &str,
    ) -> CwrResult<LocalConversationModule<'_>> {
        // A registered software author may exist only in an owner-scoped
        // upstream catalog. Each operation checks authority with its full
        // context; binding must not substitute the narrower local graph.
        Ok(LocalConversationModule {
            runtime: self,
            caller: caller.into(),
            ingress: ingress.into(),
        })
    }

    /// Bind the imported conversations module to this host's authenticated actor.
    pub fn conversation_module(&self) -> CwrResult<LocalConversationModule<'_>> {
        let state = self.lock()?;
        let service = state
            .graph
            .resources
            .iter()
            .find(|resource| {
                resource.document.get("ref").and_then(Value::as_str)
                    == Some(CONVERSATIONS_SERVICE_REF)
            })
            .ok_or_else(|| {
                invalid_error("import the conversations module before binding its service")
            })?;
        if service.type_ref != "cwp://org.coworkerprotocol/2026-10-23/schemas/native-service"
            || service.document.get("provider").and_then(Value::as_str)
                != Some("cwr.conversations.v1")
        {
            return invalid("unsupported conversations service binding");
        }
        let expected_service: Value = toml::from_str(include_str!(
            "../../../resources/base/conversations/service.toml"
        ))
        .map_err(|_| invalid_error("invalid bundled conversations service"))?;
        if service.document != expected_service {
            return invalid("conversations service differs from the supported native contract");
        }
        for operation in ["publish", "timeline", "message", "archive", "restore"] {
            if !service
                .document
                .get("operations")
                .and_then(Value::as_array)
                .is_some_and(|operations| {
                    operations
                        .iter()
                        .any(|value| value.as_str() == Some(operation))
                })
            {
                return invalid(format!("conversations service does not export {operation}"));
            }
        }
        for (entity, source) in [
            ("conversation", include_str!("../../../resources/base/conversations/definitions/conversation.toml")),
            ("conversation-message", include_str!("../../../resources/base/conversations/definitions/conversation-message.toml")),
            ("conversation-activity", include_str!("../../../resources/base/conversations/definitions/conversation-activity.toml")),
            ("conversation-transfer-metadata", include_str!("../../../resources/base/conversations/definitions/conversation-transfer-metadata.toml")),
            ("conversation-timeline-connection", include_str!("../../../resources/base/conversations/definitions/conversation-timeline-connection.toml")),
            ("conversation-timeline-item", include_str!("../../../resources/base/conversations/definitions/conversation-timeline-item.toml")),
            ("conversation-participation", include_str!("../../../resources/base/conversations/definitions/conversation-participation.toml")),
        ] {
            let reference = format!("cwp://org.coworkerprotocol/cwr/entities/{entity}");
            let expected: Value = toml::from_str(source)
                .map_err(|_| invalid_error("invalid bundled conversations entity"))?;
            if !state.graph.resources.iter().any(|resource| resource.reference == reference
                && resource.document == expected) {
                return invalid(format!("conversations service requires the supported entity {reference}"));
            }
        }
        let relationships_ref = "cwp://org.coworkerprotocol/cwr/relationships/conversations";
        let expected_relationships: Value = toml::from_str(include_str!(
            "../../../resources/base/conversations/relationships.toml"
        ))
        .map_err(|_| invalid_error("invalid bundled conversations relationships"))?;
        if !state.graph.resources.iter().any(|resource| {
            resource.reference == relationships_ref
                && resource.type_ref
                    == "cwp://org.coworkerprotocol/2026-10-23/schemas/relationship-catalog"
                && resource.document == expected_relationships
        }) {
            return invalid("conversations service requires the supported relationships");
        }
        local_catalog(&state.graph, &state.host_identity.acting_actor_ref)?;
        Ok(LocalConversationModule {
            runtime: self,
            caller: state.host_identity.acting_actor_ref.clone(),
            ingress: state.host_identity.ingress_integration_ref.clone(),
        })
    }
}

impl LocalConversationModule<'_> {
    /// Open one authorized publication selection for the bound caller. The host
    /// connection owner selects upstream routes, not a pack owner context.
    /// Reuse this snapshot across fields to preserve a consistent selection;
    /// open a fresh snapshot after a publication notification.
    pub fn publication_snapshot_in_context(
        &self,
        owner: Option<&str>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
        register: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<crate::v2026_09::conversation::ConversationPublicationSnapshot> {
        self.runtime.publication_snapshot_until(
            &self.caller,
            owner,
            deadline,
            is_cancelled,
            register,
        )
    }

    /// Create or continue a conversation using the normal durable admission path.
    /// Reusing request_id requires the same input and returns the original admission.
    pub fn publish(
        &self,
        request_id: &str,
        conversation_ref: Option<&str>,
        text: &str,
    ) -> CwrResult<LocalAdmission> {
        self.runtime.submit_text_as(
            &self.ingress,
            request_id,
            &self.caller,
            conversation_ref,
            text,
        )
    }

    /// Publish with the host's existing ownership and attachment context.
    /// The bound actor and ingress are supplied only at authenticated host setup.
    pub fn publish_message_in_context(
        &self,
        request_id: &str,
        conversation_ref: Option<&str>,
        text: impl Into<String>,
        file_refs: Vec<String>,
        owner: Option<&str>,
        owner_context_ref: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.runtime.submit_message_in_context(
            &self.ingress,
            request_id,
            &self.caller,
            conversation_ref,
            text,
            file_refs,
            owner,
            owner_context_ref,
        )
    }

    /// Page one retained connection; None selects the conversation's root.
    /// Child connections are returned as item links, never flattened or counted.
    pub fn canonical_timeline(
        &self,
        conversation_ref: &str,
        connection_ref: Option<&str>,
        page: &ConversationTimelinePageRequest,
        deadline: Instant,
    ) -> CwrResult<ConversationTimelineConnectionPage> {
        self.read(conversation_ref, deadline, |operation| {
            match connection_ref {
                Some(connection) => operation.conversation_timeline_connection(conversation_ref, connection, page),
                None => operation.conversation_timeline_root_page(conversation_ref, page),
            }
        })
    }

    /// Discover the stored format without migration or history traversal.
    /// Some(root) selects canonical_timeline; None selects legacy_timeline.
    pub fn timeline_layout(&self, conversation_ref: &str, deadline: Instant) -> CwrResult<Option<String>> {
        self.read(conversation_ref, deadline, |operation| operation.conversation_timeline_layout(conversation_ref))
    }

    /// Page the existing rootless projection without counts or payload expansion.
    pub fn legacy_timeline(
        &self,
        conversation_ref: &str,
        page: &ConversationRecordPageRequest,
        deadline: Instant,
    ) -> CwrResult<crate::v2026_09::conversation::ConversationLegacyTimelinePage> {
        self.read(conversation_ref, deadline, |operation| operation.conversation_legacy_timeline_page(conversation_ref, page))
    }

    /// Read one legacy projected message at an exact returned position/revision.
    pub fn legacy_message(
        &self,
        conversation_ref: &str,
        key: &crate::v2026_09::conversation::ConversationPositionKey,
        message_ref: &str,
        deadline: Instant,
    ) -> CwrResult<Option<ConversationMessage>> {
        self.read(conversation_ref, deadline, |operation| operation.conversation_legacy_timeline_message(conversation_ref, key, message_ref))
    }

    /// Read the message at a reachable placement, without expanding reply chains.
    pub fn message(
        &self,
        conversation_ref: &str,
        item_ref: &str,
        deadline: Instant,
    ) -> CwrResult<Option<ConversationMessage>> {
        self.read(conversation_ref, deadline, |operation| {
            operation.conversation_timeline_placed_message(conversation_ref, item_ref)
        })
    }

    fn read<T>(
        &self,
        conversation_ref: &str,
        deadline: Instant,
        read: impl FnOnce(&ConversationReadOperation<'_>) -> CwrResult<T>,
    ) -> CwrResult<T> {
        let state = self.runtime.read_lock_until(deadline, &|| false)?;
        let all = state.graph.permissions.allows_all();
        let permitted = state.admission.conversation_header(conversation_ref).is_some_and(|conversation| {
            all || (conversation.parent_conversation_ref.is_none()
                && conversation.participant_actor_refs.iter().any(|actor| actor == &self.caller))
        });
        if !permitted {
            return invalid("conversation is unavailable to the requesting participant");
        }
        let (path, root) = state.admission.read_store_identity();
        drop(state);
        let cancellation = ConversationReadCancellation::pending();
        let mut store = ConversationReadStore::open_until(path, &root, deadline, &cancellation)?;
        let operation = store.begin_operation_until(deadline, cancellation)?
            .including_execution_conversations(all);
        read(&operation)
    }

    /// Archive retains history and applies the existing ordered stop semantics.
    pub fn set_archived(
        &self,
        conversation_ref: &str,
        archived: bool,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime
            .set_conversation_archived_as(Some(conversation_ref), &self.caller, archived)
    }

    /// Archive or restore using the authenticated host's upstream connection
    /// scope. The connection owner is distinct from a pack's owner context.
    pub fn set_archived_in_context(
        &self,
        conversation_ref: &str,
        archived: bool,
        owner: Option<&str>,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime.set_conversation_archived_in_upstream_scope_as(
            Some(conversation_ref),
            &self.caller,
            archived,
            owner,
        )
    }
}

include!("conversation_module/operations.rs");

include!("conversation_module/host.rs");
