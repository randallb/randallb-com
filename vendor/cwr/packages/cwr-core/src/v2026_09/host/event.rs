impl HostEvent {
    /// Rejects malformed identity and provider-private activity at the process
    /// boundary before a presentation host retains or forwards the event.
    pub fn validate(&self) -> CwrResult<()> {
        match self {
            Self::RuntimeRestarting { generation } => require_nonblank("runtime generation", generation),
            Self::UpstreamRegistered { request_id, provider_session_ref } => {
                require_nonblank("host command request ID", request_id)?;
                validate_absolute_ref("provider session", provider_session_ref)
            }
            Self::Upstream { .. } => Ok(()),
            Self::CommandQueued { request_id } => require_nonblank("host command request ID", request_id),
            Self::WorkspaceInspected {
                request_id,
                workspace,
                default_actor_ref,
            } => {
                require_nonblank("host command request ID", request_id)?;
                if let Some(default_actor_ref) = default_actor_ref {
                    validate_ref("default actor", default_actor_ref)?;
                }
                validate_workspace(workspace)
            }
            Self::ActorSelected {
                request_id,
                actor,
                catalog,
            } => {
                require_nonblank("host command request ID", request_id)?;
                validate_actor(actor)?;
                validate_action_catalog(catalog)
            }
            Self::CatalogUpdated { actor, catalog } => {
                validate_actor(actor)?;
                validate_action_catalog(catalog)
            }
            Self::Ready { sources } => validate_sources(sources),
            Self::ConversationSnapshot {
                source_ref,
                conversations,
            } => validate_conversation_snapshot(source_ref, conversations),
            Self::RootConversationsPage {
                request_id,
                source_ref,
                page,
            } => {
                require_nonblank("host command request ID", request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_root_conversation_page(page)
            }
            Self::ConversationRecordsPage {
                request_id,
                source_ref,
                page,
            } => validate_conversation_record_page_event(request_id, source_ref, page),
            Self::VisualizationRead {
                request_id,
                source_ref,
                artifact,
            } => validate_visualization_read_event(request_id, source_ref, artifact),
            Self::ConversationUpsert {
                source_ref,
                conversation,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_root_conversation(conversation)
            }
            Self::ConversationRemoved {
                source_ref,
                conversation_ref,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)
            }
            Self::ConversationLiveOutput {
                source_ref,
                conversation_ref,
                output,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                if let Some(output) = output {
                    validate_absolute_ref("conversation invocation", &output.invocation_ref)?;
                    validate_ref("conversation target actor", &output.target_actor_ref)?;
                    require_nonblank("conversation live output", &output.text)?;
                }
                Ok(())
            }
            Self::ConversationLiveStatusSnapshot {
                source_ref,
                conversation_ref,
                statuses,
            } => validate_live_status_snapshot(source_ref, conversation_ref, statuses),
            Self::ConversationTurnStatusSnapshot {
                source_ref,
                conversation_ref,
                turns,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                let mut invocations = BTreeSet::new();
                for turn in turns {
                    validate_absolute_ref("conversation invocation", &turn.invocation_ref)?;
                    validate_absolute_ref("turn source message", &turn.source_message_ref)?;
                    validate_ref("turn requester actor", &turn.requester_actor_ref)?;
                    validate_ref("turn target actor", &turn.target_actor_ref)?;
                    for worker in &turn.workers {
                        validate_absolute_ref("worker invocation", &worker.invocation_ref)?;
                        validate_ref("worker target actor", &worker.target_actor_ref)?;
                        for actor_ref in &worker.delegation_path_actor_refs {
                            validate_ref("worker delegation path actor", actor_ref)?;
                        }
                    }
                    if !invocations.insert(&turn.invocation_ref) {
                        return invalid(format!(
                            "turn status snapshot repeats invocation {}",
                            turn.invocation_ref
                        ));
                    }
                }
                Ok(())
            }
            Self::ConversationExecutionFinished {
                source_ref,
                conversation_ref,
                target_actor_ref,
                response,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                validate_ref("execution target actor", target_actor_ref)?;
                validate_absolute_ref("conversation invocation", &response.invocation_ref)?;
                response.validate_fields().map_err(|error| {
                    std::io::Error::new(std::io::ErrorKind::InvalidData, error).into()
                })
            }
            Self::ConversationExecutionFailed {
                source_ref,
                conversation_ref,
                invocation_ref,
                target_actor_ref,
                error,
            } => {
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                validate_absolute_ref("conversation invocation", invocation_ref)?;
                validate_ref("execution target actor", target_actor_ref)?;
                require_nonblank("conversation execution failure", error)
            }
            Self::CommandSucceeded {
                request_id,
                receipt,
            } => {
                require_nonblank("host command request ID", request_id)?;
                validate_absolute_ref("conversation", &receipt.conversation_ref)?;
                if let Some(message_ref) = &receipt.message_ref {
                    validate_absolute_ref("conversation message", message_ref)?;
                }
                if let Some(invocation_ref) = &receipt.invocation_ref {
                    validate_absolute_ref("conversation invocation", invocation_ref)?;
                }
                for invocation_ref in &receipt.stopped_invocation_refs {
                    validate_absolute_ref("stopped conversation invocation", invocation_ref)?;
                }
                Ok(())
            }
            Self::CommandFailed { request_id, error } => {
                require_nonblank("host command request ID", request_id)?;
                require_nonblank("host command error", error)
            }
            Self::IntrinsicResponse { request_id, .. } => {
                require_nonblank("host command request ID", request_id)
            }
        }
    }
}

fn validate_live_status_snapshot(
    source_ref: &str,
    conversation_ref: &str,
    statuses: &[ConversationLiveStatus],
) -> CwrResult<()> {
    validate_absolute_ref("conversation source", source_ref)?;
    validate_absolute_ref("conversation", conversation_ref)?;
    let mut publishers = BTreeSet::new();
    for status in statuses {
        validate_ref("live status subject actor", &status.subject_actor_ref)?;
        require_nonblank(
            "live status publisher session ID",
            &status.publisher_session_id,
        )?;
        if status.sequence == 0 {
            return invalid("live status sequence must be positive");
        }
        if status.expires_at_unix_ms == 0 {
            return invalid("live status expiry must be positive");
        }
        if !publishers.insert(&status.publisher_session_id) {
            return invalid(format!(
                "live status snapshot repeats publisher session {}",
                status.publisher_session_id
            ));
        }
        let mut targets = BTreeSet::new();
        for actor_ref in &status.target_actor_refs {
            validate_ref("live status target actor", actor_ref)?;
            if !targets.insert(actor_ref) {
                return invalid(format!("live status repeats target actor {actor_ref}"));
            }
        }
    }
    Ok(())
}

fn validate_visualization_read_event(
    request_id: &str,
    source_ref: &str,
    artifact: &ConversationVisualizationArtifact,
) -> CwrResult<()> {
    require_nonblank("host command request ID", request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    validate_absolute_ref("visualization artifact", &artifact.artifact_ref)
}

fn validate_conversation_record_page_event(
    request_id: &str,
    source_ref: &str,
    page: &ConversationRecordPage,
) -> CwrResult<()> {
    require_nonblank("host command request ID", request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    validate_conversation_record_page(page)
}

fn validate_conversation_snapshot(
    source_ref: &str,
    conversations: &[RetainedConversation],
) -> CwrResult<()> {
    validate_absolute_ref("conversation source", source_ref)?;
    let mut refs = BTreeSet::new();
    for conversation in conversations {
        validate_root_conversation(conversation)?;
        if !refs.insert(&conversation.conversation_ref) {
            return invalid(format!(
                "conversation snapshot repeats {}",
                conversation.conversation_ref
            ));
        }
    }
    Ok(())
}

fn validate_root_conversation_page(page: &ConversationSummaryPage) -> CwrResult<()> {
    if page.summaries.len() > MAX_CONVERSATION_SUMMARY_PAGE_SIZE {
        return invalid("root conversation page exceeds the maximum page size");
    }
    if page.total_count < page.summaries.len() {
        return invalid("root conversation total count is smaller than the page");
    }
    if page.archived_count > page.total_count {
        return invalid("archived conversation count exceeds the total count");
    }
    let mut refs = BTreeSet::new();
    let mut previous_key: Option<(i64, &str)> = None;
    for summary in &page.summaries {
        validate_conversation_summary(summary)?;
        let key = (
            summary.key.creation_order,
            summary.key.conversation_ref.as_str(),
        );
        if previous_key.is_some_and(|previous| previous <= key) {
            return invalid("root conversation page is not in newest-first order");
        }
        previous_key = Some(key);
        if !refs.insert(&summary.key.conversation_ref) {
            return invalid(format!(
                "root conversation page repeats {}",
                summary.key.conversation_ref
            ));
        }
    }
    Ok(())
}

fn validate_conversation_summary(summary: &ConversationSummary) -> CwrResult<()> {
    validate_absolute_ref("conversation", &summary.key.conversation_ref)?;
    if summary.key.creation_order < 0 {
        return invalid("conversation creation order must be non-negative");
    }
    if let Some(last_message_ref) = &summary.last_message_ref {
        validate_absolute_ref("conversation message", last_message_ref)?;
    }
    if summary.last_message_ref.is_some() != summary.last_message_preview.is_some() {
        return invalid("conversation last message ref and preview must appear together");
    }
    for work_ref in &summary.completed_work_refs {
        validate_absolute_ref("completed conversation work", work_ref)?;
    }
    if let Some(title) = &summary.title {
        if normalize_conversation_title(title)? != *title {
            return invalid("conversation title is not normalized");
        }
    }
    require_nonblank("conversation revision", &summary.revision)?;
    Ok(())
}

fn validate_conversation_record_page(page: &ConversationRecordPage) -> CwrResult<()> {
    let (length, total_count, revision) = match page {
        ConversationRecordPage::Summary { summary } => {
            if let Some(summary) = summary {
                validate_conversation_summary(summary)?;
            }
            return Ok(());
        }
        ConversationRecordPage::Resource { resource } => {
            if let Some(resource) = resource {
                validate_absolute_ref(
                    "conversation resource runtime root",
                    &resource.position.runtime_root_ref,
                )?;
                validate_absolute_ref(
                    "conversation resource conversation",
                    &resource.position.conversation_ref,
                )?;
                require_nonblank(
                    "conversation resource commit revision",
                    &resource.position.commit_revision,
                )?;
                match &resource.resource {
                    crate::v2026_09::conversation::ConversationResource::Message {
                        message,
                        ..
                    } => validate_absolute_ref(
                        "conversation resource message",
                        &message.message_ref,
                    )?,
                    crate::v2026_09::conversation::ConversationResource::Invocation {
                        invocation,
                    } => validate_absolute_ref(
                        "conversation resource invocation",
                        &invocation.invocation_ref,
                    )?,
                }
            }
            return Ok(());
        }
        ConversationRecordPage::Completions { page } => {
            validate_absolute_ref("completion runtime", &page.runtime_ref)?;
            require_nonblank("completion timeline revision", &page.revision)?;
            if page.items.len()>200 || page.through_sequence<0 || page.next_after<0 || page.next_after>page.through_sequence { return invalid("invalid completion page"); }
            let mut previous=None;
            for item in &page.items { validate_record_position(item.sequence,&mut previous)?; validate_absolute_ref("completion invocation",&item.invocation_ref)?; if item.sequence>page.through_sequence { return invalid("completion exceeds upper bound"); } }
            if page.items.last().is_some_and(|item|item.sequence!=page.next_after) || (page.has_next_page && page.items.is_empty()) { return invalid("invalid completion cursor"); }
            return Ok(());
        }
        ConversationRecordPage::Content { page } => {
            let mut previous_position = None;
            for content in &page.content {
                validate_record_position(content.key.position, &mut previous_position)?;
                validate_absolute_ref("conversation message", &content.message.message_ref)?;
                validate_ref("conversation message author", &content.message.author_actor_ref)?;
                for actor_ref in &content.message.addressed_actor_refs {
                    validate_ref("addressed conversation actor", actor_ref)?;
                }
                if let Some(reply_to_message_ref) = &content.message.reply_to_message_ref {
                    validate_absolute_ref("conversation reply message", reply_to_message_ref)?;
                }
            }
            (page.content.len(), page.total_count, &page.revision)
        }
        ConversationRecordPage::Timeline { page } => {
            let mut previous_position = None;
            for anchor in &page.anchors {
                match anchor {
                    ConversationTimelineAnchor::Message { key, message_ref } => {
                        validate_record_position(key.position, &mut previous_position)?;
                        validate_absolute_ref("conversation timeline message", message_ref)?;
                    }
                    ConversationTimelineAnchor::Turn { key, invocation_ref } => {
                        validate_record_position(key.position, &mut previous_position)?;
                        validate_absolute_ref("conversation timeline invocation", invocation_ref)?;
                    }
                }
            }
            for message in &page.messages {
                validate_absolute_ref("conversation message", &message.message_ref)?;
                validate_ref("conversation message author", &message.author_actor_ref)?;
            }
            for activity in &page.activity {
                validate_absolute_ref("conversation activity", &activity.activity_ref)?;
            }
            (page.anchors.len(), page.total_count, &page.revision)
        }
    };
    if length > MAX_CONVERSATION_RECORD_PAGE_SIZE {
        return invalid("conversation record page exceeds the maximum page size");
    }
    if total_count < length {
        return invalid("conversation record total count is smaller than the page");
    }
    require_nonblank("conversation revision", revision)?;
    Ok(())
}

fn validate_record_position(position: i64, previous: &mut Option<i64>) -> CwrResult<()> {
    if position < 0 {
        return invalid("conversation record position must be non-negative");
    }
    if previous.is_some_and(|previous| previous >= position) {
        return invalid("conversation record page is not in oldest-first order");
    }
    *previous = Some(position);
    Ok(())
}
