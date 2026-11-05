impl HostCommand {
    pub fn request_id(&self) -> &str {
        match self {
            Self::AttachmentCreated { request_id, .. }
            | Self::AdmitIngress { request_id, .. }
            | Self::Upstream { request_id, .. }
            | Self::InspectWorkspace { request_id, .. }
            | Self::SelectActor { request_id, .. }
            | Self::ReadRootConversations { request_id, .. }
            | Self::ReadConversationRecords { request_id, .. }
            | Self::ReadVisualization { request_id, .. }
            | Self::CreateConversation { request_id, .. }
            | Self::ReloadGraph { request_id, .. }
            | Self::ReplyPersonRequest { request_id, .. }
            | Self::ContinueConversation { request_id, .. }
            | Self::ExecuteConversationControl { request_id, .. }
            | Self::RecordHostExchange { request_id, .. }
            | Self::InvokeAction { request_id, .. }
            | Self::StopConversation { request_id, .. }
            | Self::SetConversationLiveStatus { request_id, .. } => request_id,
        }
    }

    pub fn validate(&self) -> CwrResult<()> {
        match self {
            Self::AttachmentCreated { version, request_id, attachment_ref } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("attachment", attachment_ref)
            },
            Self::AdmitIngress { version, request_id, ingress, .. } => {
                validate_ingress_command(*version, request_id, ingress)
            },
            Self::Upstream { version, request_id, .. } => validate_command_header(*version, request_id),
            Self::InspectWorkspace {
                version,
                request_id,
            } => {
                validate_command_header(*version, request_id)
            }
            Self::SelectActor {
                version,
                request_id,
                actor_ref,
            } => {
                validate_command_header(*version, request_id)?;
                validate_ref("selected actor", actor_ref)
            }
            Self::ReadRootConversations {
                version,
                request_id,
                source_ref,
                page,
                timeout_ms,
            } => validate_root_conversation_read_command(
                *version,
                request_id,
                source_ref,
                page,
                *timeout_ms,
            ),
            Self::ReadConversationRecords {
                version,
                request_id,
                source_ref,
                read,
                timeout_ms,
            } => validate_conversation_record_read_command(
                *version,
                request_id,
                source_ref,
                read,
                *timeout_ms,
            ),
            Self::ReadVisualization {
                version,
                request_id,
                source_ref,
                artifact_ref,
            } => validate_visualization_read_command(
                *version,
                request_id,
                source_ref,
                artifact_ref,
            ),
            Self::ReloadGraph { version, request_id, source_ref } =>
                validate_workspace_reload_command(*version, request_id, source_ref.as_deref()),
            Self::CreateConversation {
                version,
                request_id,
                source_ref,
                author_actor_ref,
                text,
            } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_ref("conversation author", author_actor_ref)?;
                require_nonblank("conversation text", text)
            }
            Self::ReplyPersonRequest { version, request_id, source_ref, conversation_ref, invocation_ref, text } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                validate_absolute_ref("invocation", invocation_ref)?;
                require_nonblank("reply", text)
            }
            Self::ContinueConversation {
                version,
                request_id,
                source_ref,
                conversation_ref,
                author_actor_ref,
                text,
                ..
            } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                validate_ref("conversation author", author_actor_ref)?;
                require_nonblank("conversation text", text)
            }
            Self::ExecuteConversationControl {
                version,
                request_id,
                source_ref,
                conversation_ref,
                requesting_actor_ref,
                control,
            } => validate_conversation_control_command(
                *version,
                request_id,
                source_ref,
                conversation_ref.as_deref(),
                requesting_actor_ref,
                control,
            ),
            Self::RecordHostExchange {
                version,
                request_id,
                source_ref,
                requesting_actor_ref,
                host_actor_ref,
                conversation_ref,
                request_text,
                response_text,
            } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_ref("host exchange requester", requesting_actor_ref)?;
                validate_ref("host exchange actor", host_actor_ref)?;
                if let Some(conversation_ref) = conversation_ref {
                    validate_absolute_ref("conversation", conversation_ref)?;
                }
                require_nonblank("host exchange request", request_text)?;
                require_nonblank("host exchange response", response_text)
            }
            Self::InvokeAction {
                version,
                request_id,
                source_ref,
                author_actor_ref,
                conversation_ref,
                activation,
                input,
            } => {
                validate_command_header(*version, request_id)?;
                if let Some(source_ref) = source_ref {
                    validate_absolute_ref("conversation source", source_ref)?;
                } else if conversation_ref.is_some() {
                    return invalid("a conversation action requires a conversation source");
                }
                validate_ref("action author", author_actor_ref)?;
                if let Some(conversation_ref) = conversation_ref {
                    validate_absolute_ref("conversation", conversation_ref)?;
                }
                require_nonblank("action activation", activation)?;
                if !activation.starts_with('@') {
                    return invalid("action activation must start with @");
                }
                if !input.is_object() {
                    return invalid("action input must be a JSON object");
                }
                Ok(())
            }
            Self::StopConversation {
                version,
                request_id,
                source_ref,
                conversation_ref,
                requesting_actor_ref,
                invocation_ref,
            } => {
                validate_command_header(*version, request_id)?;
                validate_absolute_ref("conversation source", source_ref)?;
                validate_absolute_ref("conversation", conversation_ref)?;
                validate_ref("stop requesting actor", requesting_actor_ref)?;
                if let Some(invocation_ref) = invocation_ref {
                    validate_absolute_ref("stop invocation", invocation_ref)?;
                }
                Ok(())
            }
            Self::SetConversationLiveStatus {
                version,
                request_id,
                source_ref,
                conversation_ref,
                subject_actor_ref,
                publisher_session_id,
                sequence,
                target_actor_refs,
                status,
                lease_ms,
            } => validate_live_status_command(
                *version,
                request_id,
                source_ref,
                conversation_ref,
                subject_actor_ref,
                publisher_session_id,
                *sequence,
                target_actor_refs,
                status.is_some(),
                *lease_ms,
            ),
        }
    }
}

#[expect(clippy::too_many_arguments, reason = "mirrors the wire command fields")]
fn validate_live_status_command(
    version: u32,
    request_id: &str,
    source_ref: &str,
    conversation_ref: &str,
    subject_actor_ref: &str,
    publisher_session_id: &str,
    sequence: u64,
    target_actor_refs: &[String],
    active: bool,
    lease_ms: u64,
) -> CwrResult<()> {
    validate_command_header(version, request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    validate_absolute_ref("conversation", conversation_ref)?;
    validate_ref("live status subject actor", subject_actor_ref)?;
    require_nonblank("live status publisher session ID", publisher_session_id)?;
    if sequence == 0 {
        return invalid("live status sequence must be positive");
    }
    let mut targets = BTreeSet::new();
    for actor_ref in target_actor_refs {
        validate_ref("live status target actor", actor_ref)?;
        if !targets.insert(actor_ref) {
            return invalid(format!("live status repeats target actor {actor_ref}"));
        }
    }
    if active {
        if !(1_000..=30_000).contains(&lease_ms) {
            return invalid("active live status lease must be between 1000 and 30000 ms");
        }
    } else if lease_ms != 0 {
        return invalid("cleared live status must have a zero lease");
    }
    Ok(())
}

fn validate_visualization_read_command(
    version: u32,
    request_id: &str,
    source_ref: &str,
    artifact_ref: &str,
) -> CwrResult<()> {
    validate_command_header(version, request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    validate_absolute_ref("visualization artifact", artifact_ref)
}

fn validate_conversation_control_command(
    version: u32,
    request_id: &str,
    source_ref: &str,
    conversation_ref: Option<&str>,
    requesting_actor_ref: &str,
    control: &ConversationControl,
) -> CwrResult<()> {
    validate_command_header(version, request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    if let Some(reference) = conversation_ref {
        validate_absolute_ref("conversation", reference)?;
    }
    if conversation_ref.is_none() && !matches!(control, ConversationControl::Import { .. }) {
        return invalid("Conversation control requires a conversation");
    }
    validate_ref("control requesting actor", requesting_actor_ref)?;
    match control {
        ConversationControl::Export
        | ConversationControl::Archive
        | ConversationControl::Restore => Ok(()),
        ConversationControl::Import { path, import_id } => {
            require_nonblank("import path", path)?;
            require_nonblank("import ID", import_id)
        }
        ConversationControl::Fork { message_ref, fork_id } => {
            validate_absolute_ref("fork message", message_ref)?;
            require_nonblank("fork ID", fork_id)
        }
        ConversationControl::Rename { title } => {
            normalize_conversation_title(title).map(|_| ())
        }
    }
}

fn validate_root_conversation_read_command(
    version: u32,
    request_id: &str,
    source_ref: &str,
    page: &ConversationSummaryPageRequest,
    timeout_ms: u64,
) -> CwrResult<()> {
    if !matches!(version, 1..=3) {
        return invalid(format!(
            "unsupported root conversation read version {version}"
        ));
    }
    require_nonblank("host command request ID", request_id)?;
    validate_absolute_ref("conversation source", source_ref)?;
    page.validate()?;
    if !(1..=30_000).contains(&timeout_ms) {
        return invalid("conversation read timeout must be between 1 and 30000 ms");
    }
    Ok(())
}

fn validate_conversation_record_read_command(
    version: u32,
    request_id: &str,
    source_ref: &str,
    read: &ConversationRecordReadRequest,
    timeout_ms: u64,
) -> CwrResult<()> {
    if matches!(read, ConversationRecordReadRequest::Completions { .. }) {
        if version != 3 { return invalid("completion history requires host command version 3"); }
        require_nonblank("host command request ID", request_id)?;
    } else { validate_command_header(version, request_id)?; }
    validate_absolute_ref("conversation source", source_ref)?;
    match read {
        ConversationRecordReadRequest::Resource { resource_ref } => {
            validate_absolute_ref("conversation resource", resource_ref)?;
        }
        ConversationRecordReadRequest::Summary {
            participant_actor_ref,
            ..
        } => {
            validate_absolute_ref("summary participant", participant_actor_ref)?;
            validate_absolute_ref(
                "conversation",
                read.conversation_ref()
                    .expect("summary reads have a conversation ref"),
            )?;
        }
        _ => validate_absolute_ref(
            "conversation",
            read.conversation_ref()
                .expect("non-resource reads have a conversation ref"),
        )?,
    }
    read.validate()?;
    if !(1..=30_000).contains(&timeout_ms) {
        return invalid("conversation read timeout must be between 1 and 30000 ms");
    }
    Ok(())
}

fn validate_command_header(version: u32, request_id: &str) -> CwrResult<()> {
    if version != HOST_COMMAND_VERSION {
        return invalid(format!("unsupported host command version {version}"));
    }
    require_nonblank("host command request ID", request_id)
}

fn validate_workspace_reload_command(version: u32, request_id: &str, source_ref: Option<&str>) -> CwrResult<()> {
    validate_command_header(version, request_id)?;
    if let Some(source_ref) = source_ref {
        validate_absolute_ref("conversation source", source_ref)?;
    }
    Ok(())
}

fn validate_ingress_command(version: u32, request_id: &str, ingress: &super::conversation::ConversationIngress) -> CwrResult<()> {
    validate_command_header(version, request_id)?;
    validate_absolute_ref("ingress author", &ingress.author_actor_ref)?;
    validate_absolute_ref("ingress integration", &ingress.integration_ref)?;
    require_nonblank("provider event", &ingress.provider_event_ref)
}
