impl ConversationReadOperation<'_> {
    pub fn root_conversation_summary(&self, reference: &str) -> CwrResult<Option<ConversationSummary>> {
        self.ensure_active()?;
        self.finite_snapshot()?.summary_row(reference, None)?.map(|row| decode_summary_row(self.finite_snapshot()?, row)).transpose()
    }

    pub fn workspace_conversation_summary(
        &self,
        reference: &str,
        workspace_owner_context_ref: Option<&str>,
        participant_actor_ref: &str,
    ) -> CwrResult<Option<ConversationSummary>> {
        self.ensure_active()?;
        self.finite_snapshot()?.summary_row(reference, Some((workspace_owner_context_ref, participant_actor_ref)))?.map(|row| decode_summary_row(self.finite_snapshot()?, row)).transpose()
    }

    pub fn root_conversations(&self, window: ConversationSummaryWindow<'_>) -> CwrResult<ConversationSummaryPage> {
        self.root_conversations_in_scope(window, None, None)
    }

    pub fn participant_root_conversations(&self, actor: &str, archived: Option<bool>, window: ConversationSummaryWindow<'_>) -> CwrResult<ConversationSummaryPage> {
        self.root_conversations_in_scope(window, Some(actor), archived)
    }

    pub(crate) fn root_conversations_in_scope(&self, window: ConversationSummaryWindow<'_>, actor: Option<&str>, archived: Option<bool>) -> CwrResult<ConversationSummaryPage> {
        self.ensure_active()?;
        validate_window(window)?;
        let key = |key: &ConversationSummaryKey| cwpdb_store::conversation::SummaryWindowKey {
            creation_order: key.creation_order,
            conversation_ref: key.conversation_ref.clone(),
        };
        let facts = self.finite_snapshot()?.summary_window(&cwpdb_store::conversation::SummaryWindowRequest {
            participant_actor_ref: actor.map(str::to_owned),
            archived,
            owner_context_filter: self.owner_context_filter.clone(),
            after: window.after.map(key),
            before: window.before.map(key),
            first: window.first,
            last: window.last,
        })?;
        let result = ConversationSummaryPage {
            membership_revision: facts.membership_revision,
            revision: facts.revision,
            summaries: facts.summaries.into_iter().map(|row| decode_summary_row(self.finite_snapshot()?, row)).collect::<CwrResult<_>>()?,
            has_next_page: facts.has_next_page,
            has_previous_page: facts.has_previous_page,
            total_count: facts.total_count,
            archived_count: facts.archived_count,
        };
        self.ensure_active()?;
        Ok(result)
    }
}

fn decode_summary_row(snapshot: &cwpdb_store::conversation::ReadSnapshot, mut row: cwpdb_store::conversation::SummaryWindowRow) -> CwrResult<ConversationSummary> {
    if row.canonical_preview {
        let record = row.last_message_ref.as_deref().map(|reference| snapshot.summary_message_record(&row.key.conversation_ref, reference)).transpose()?;
        if let Some(record) = record {
        let message: super::ConversationMessage = serde_json::from_value(record)?;
        if row.last_message_ref.as_deref() != Some(message.message_ref.as_str()) {
            return invalid("conversation summary message identity mismatch");
        }
        row.last_message_preview = Some(message.nonvisual_text().chars().take(240).collect());
        }
    }
    let mut value = serde_json::to_value(row)?;
    value.as_object_mut().unwrap().remove("canonical_preview");
    Ok(serde_json::from_value(value)?)
}

fn validate_window(window: ConversationSummaryWindow<'_>) -> CwrResult<()> {
    if window.first.is_none() && window.last.is_none() { return invalid("conversation page requires first or last"); }
    for count in [window.first, window.last].into_iter().flatten() {
        if count > MAX_CONVERSATION_SUMMARY_PAGE_SIZE { return invalid(format!("conversation page size must not exceed {MAX_CONVERSATION_SUMMARY_PAGE_SIZE}")); }
    }
    Ok(())
}
