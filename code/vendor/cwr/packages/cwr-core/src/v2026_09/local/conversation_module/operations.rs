// External conversation operations bind identity once, then delegate to the
// owning admission/control implementation. Execution and recovery stay native.
impl LocalConversationModule<'_> {
    pub fn publish_text_in_context(
        &self,
        request: &str,
        conversation: Option<&str>,
        text: &str,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.runtime.submit_text_in_upstream_scope(
            &self.ingress,
            request,
            &self.caller,
            conversation,
            text,
            owner,
        )
    }

    pub fn replay_text_in_context(
        &self,
        request: &str,
        conversation: &str,
        text: &str,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        self.runtime.replay_text_in_upstream_scope(
            &self.ingress,
            request,
            &self.caller,
            conversation,
            text,
            owner,
        )
    }

    pub fn admit_provider_ingress(
        &self,
        ingress: ConversationIngress,
        actor_integration: Option<&str>,
        owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        if ingress.author_actor_ref != self.caller || ingress.integration_ref != self.ingress {
            return invalid("ingress differs from the bound conversation caller");
        }
        self.runtime
            .admit_ingress_in_scope(ingress, actor_integration, owner)
    }

    pub fn list(
        &self,
        page: &ConversationSummaryPageRequest,
        archived: Option<bool>,
        deadline: Instant,
    ) -> CwrResult<ConversationSummaryPage> {
        self.runtime
            .participant_root_conversation_summaries(&self.caller, page, archived, deadline)
    }

    /// Compatibility command presentation; Desktop uses selective publication
    /// reads instead. This preserves the existing intrinsic command contract.
    pub fn read_command_timeline(
        &self,
        conversation: &str,
        page: &ConversationRecordPageRequest,
        deadline: Instant,
    ) -> CwrResult<ConversationTimelinePage> {
        self.runtime
            .participant_conversation_timeline(conversation, &self.caller, page, deadline)
    }

    pub fn browse(
        &self,
        conversation: &str,
        request: &crate::intrinsic_cwr::ConversationBrowseRequest,
        deadline: Instant,
    ) -> CwrResult<crate::intrinsic_cwr::ConversationBrowseResponse> {
        self.runtime
            .participant_conversation_browse(conversation, &self.caller, request, deadline)
    }

    /// Carry host request identifiers into a private diagnostic if a retained
    /// message is too large for a browse read.
    pub fn browse_with_request_context(
        &self,
        conversation: &str,
        request: &crate::intrinsic_cwr::ConversationBrowseRequest,
        deadline: Instant,
        request_id: &str,
        operation_id: Option<&str>,
    ) -> CwrResult<crate::intrinsic_cwr::ConversationBrowseResponse> {
        self.runtime.participant_conversation_browse_with_context(
            conversation,
            &self.caller,
            request,
            deadline,
            BrowseDiagnosticContext {
                invocation_ref: None,
                request_id: Some(request_id),
                operation_id,
            },
        )
    }

    pub fn rename(
        &self,
        conversation: Option<&str>,
        title: &str,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime
            .rename_conversation_as(conversation, &self.caller, title)
    }

    pub fn export(&self, conversation: Option<&str>) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime
            .export_conversation_as(conversation, &self.caller)
    }

    pub fn import(&self, path: &str, request: Option<&str>) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime
            .import_conversation_as(&self.caller, path, request)
    }

    pub fn fork(
        &self,
        conversation: Option<&str>,
        message: &str,
        request: &str,
    ) -> CwrResult<IntrinsicCwrResponse> {
        self.runtime
            .fork_conversation_as(conversation, &self.caller, message, request)
    }

    /// Stop through the owning request control path; retained stop receipts
    /// keep retries from cancelling work admitted after the original request.
    pub fn stop(
        &self,
        conversation: &str,
        invocation: Option<&str>,
        request: &str,
        event_at_unix_ms: Option<u64>,
        reason: Option<String>,
    ) -> CwrResult<LocalStopOutcome> {
        match invocation {
            Some(invocation) => self.runtime.stop_selected_request_as(
                conversation, invocation, &self.caller, request, reason,
            ),
            None => self.runtime.stop_conversation_request_as(
                conversation, &self.caller, request, event_at_unix_ms, reason,
            ),
        }
    }

    /// Reserve in host command-reader order before queuing an archive worker.
    /// Authority is rechecked by the owning operation when the ticket executes.
    pub fn reserve_archive(&self, conversation: &str) -> CwrResult<Arc<LocalArchiveReservation>> {
        self.runtime.reserve_conversation_archive(conversation)
    }

    pub fn set_archived_with_stops(
        &self,
        conversation: Option<&str>,
        archived: bool,
        reservation: Option<Arc<LocalArchiveReservation>>,
    ) -> CwrResult<(IntrinsicCwrResponse, Vec<String>)> {
        match reservation {
            Some(reservation) => self.runtime.set_conversation_archived_reserved_as(
                conversation,
                &self.caller,
                archived,
                reservation,
            ),
            None => self.runtime.set_conversation_archived_with_stops_as(
                conversation,
                &self.caller,
                archived,
            ),
        }
    }
}
