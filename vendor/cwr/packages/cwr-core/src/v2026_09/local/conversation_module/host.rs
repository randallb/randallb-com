/// Trusted embedding adapter for the conversation service.
///
/// Host bootstrap, broadcast receipts and execution status transport already
/// operate with runtime authority. They are not participant read APIs. Keep
/// their existing projection and disclosure owner rather than manufacturing a
/// participant identity for server-internal publication work.
pub struct LocalConversationHost<'a>(&'a LocalRuntime);

/// One authoritative status capture and its matching transient execution cursors.
/// This remains an embedding result, without a serialized or persistent contract.
pub struct ExecutionTurnStatusPublication {
    pub statuses: BTreeMap<String, Vec<crate::v2026_09::host::ConversationTurnStatus>>,
    pub replace_all: bool,
    pub activity_cursors: BTreeMap<String, Option<String>>,
}

impl LocalRuntime {
    pub fn conversation_host(&self) -> LocalConversationHost<'_> {
        LocalConversationHost(self)
    }
}

impl LocalConversationHost<'_> {
    pub fn publish_exchange(
        &self,
        conversation: Option<&str>,
        requester: &str,
        host_actor: &str,
        request: String,
        response: String,
    ) -> CwrResult<crate::v2026_09::conversation::HostConversationExchange> {
        self.0
            .record_host_exchange(conversation, requester, host_actor, request, response)
    }

    pub fn conversation(&self, reference: &str) -> CwrResult<Option<RetainedConversation>> {
        self.0.host_conversation(reference)
    }
    pub fn metadata(&self, reference: &str) -> CwrResult<Option<RetainedConversation>> {
        self.0.host_conversation_metadata(reference)
    }
    pub fn conversations(&self) -> CwrResult<Vec<RetainedConversation>> {
        self.0.host_conversations()
    }
    pub fn bootstrap(&self, include_lineage: bool) -> CwrResult<Vec<RetainedConversation>> {
        self.0.host_conversation_bootstrap(include_lineage)
    }
    pub fn turn_statuses(
        &self,
    ) -> CwrResult<BTreeMap<String, Vec<crate::v2026_09::host::ConversationTurnStatus>>> {
        self.0.conversation_turn_statuses()
    }
    /// Capture execution status and every checked ancestry cursor from one state.
    /// Callers advance their ephemeral cursors only after publishing this result.
    pub fn turn_status_publication_for_execution(
        &self,
        invocation_refs: &BTreeSet<String>,
        conversation_refs: &BTreeSet<String>,
        published_activity: &BTreeMap<String, Option<String>>,
    ) -> CwrResult<ExecutionTurnStatusPublication> {
        let state = self.0.lock()?;
        let store = &state.admission;
        let mut pending = invocation_refs.clone();
        let mut checked = BTreeSet::new();
        let mut activity_cursors = BTreeMap::new();
        let mut replace_all = false;
        while let Some(reference) = pending.pop_first() {
            if !checked.insert(reference.clone()) {
                continue;
            }
            if let Some(record) = store.invocation_record(&reference) {
                pending.extend(record.parent_invocation_ref.iter().cloned());
            }
            let (requires_full, cursor) = execution_status_publication_cursor(
                store,
                &reference,
                published_activity
                    .get(&reference)
                    .and_then(Option::as_deref),
            );
            replace_all |= requires_full;
            activity_cursors.insert(reference, cursor);
        }
        let statuses = if replace_all {
            crate::v2026_09::host::project_conversation_turn_statuses(store)
        } else {
            let selected = crate::v2026_09::host::affected_turn_status_conversations(
                store,
                invocation_refs,
                conversation_refs,
            );
            crate::v2026_09::host::project_conversation_turn_statuses_for(store, &selected)
        };
        Ok(ExecutionTurnStatusPublication {
            statuses,
            replace_all,
            activity_cursors,
        })
    }
    pub fn turn_statuses_affected_by(
        &self,
        invocation_refs: &BTreeSet<String>,
        conversation_refs: &BTreeSet<String>,
    ) -> CwrResult<BTreeMap<String, Vec<crate::v2026_09::host::ConversationTurnStatus>>> {
        self.0
            .conversation_turn_statuses_affected_by(invocation_refs, conversation_refs)
    }
    pub fn roots(
        &self,
        page: &ConversationSummaryPageRequest,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
        register: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationSummaryPage> {
        self.0
            .root_conversation_summaries(page, deadline, cancelled, register)
    }
    pub fn records(
        &self,
        read: &ConversationRecordReadRequest,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
        register: impl FnOnce(ConversationReadCancellation),
    ) -> CwrResult<ConversationRecordPage> {
        self.0
            .conversation_records(read, deadline, cancelled, register)
    }
}

/// Inspect only a checked invocation's owned execution. Unknown cursors scan its
/// related history again rather than risking omission of an independent effect.
fn execution_status_publication_cursor(
    store: &ConversationAdmissionStore,
    invocation_ref: &str,
    since_activity_ref: Option<&str>,
) -> (bool, Option<String>) {
    let Some(record) = store.invocation_record(invocation_ref) else {
        return (false, None);
    };
    let intrinsic_control = record.target_actor_ref == INTRINSIC_CWR_ACTOR_REF
        && !matches!(
            record.action_name.as_str(),
            "list--coworkers"
                | "list--actions"
                | "show--help"
                | "debug--invocation"
                | "list--conversations"
                | "read--conversation"
                | "browse--conversation"
                | "export--conversation"
                | "list--schedules"
                | "get--schedule"
                | "list--schedule_runs"
        );
    let Some(execution) = store.hydrated_execution_conversation(invocation_ref) else {
        return (intrinsic_control, None);
    };
    let start = since_activity_ref
        .and_then(|reference| {
            execution
                .activity
                .iter()
                .position(|item| item.activity_ref == reference)
        })
        .map_or(0, |index| index + 1);
    let independent_host_effect = execution.activity[start..].iter().any(|activity| {
        match &activity.event {
            ConversationActivityEvent::HostCall { operation, .. } => {
                host_operation_has_independent_status_effects(operation)
            }
            // A child may publish while its parent is still executing a call.
            // The result retained afterward must trigger a second refresh even
            // when the call itself precedes the already-published cursor.
            ConversationActivityEvent::HostResult { call_id, .. } => execution
                .activity
                .iter()
                .find_map(|item| match &item.event {
                    ConversationActivityEvent::HostCall {
                        call_id: id,
                        operation,
                        ..
                    } if id == call_id => Some(operation.as_str()),
                    _ => None,
                })
                .is_none_or(host_operation_has_independent_status_effects),
            _ => false,
        }
    });
    (
        intrinsic_control || independent_host_effect,
        execution
            .activity
            .last()
            .map(|activity| activity.activity_ref.clone()),
    )
}

fn host_operation_has_independent_status_effects(operation: &str) -> bool {
    !matches!(
        operation,
        "cwr.list_coworkers"
            | "cwr.list_actions"
            | "cwr.list_conversation_contexts"
            | "cwr.read_file"
    )
}
