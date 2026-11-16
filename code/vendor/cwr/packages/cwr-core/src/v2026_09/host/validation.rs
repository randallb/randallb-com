fn validate_sources(sources: &[ConversationSource]) -> CwrResult<()> {
    if sources.is_empty() {
        return invalid("host ready event must declare at least one conversation source");
    }
    let mut refs = BTreeSet::new();
    for source in sources {
        validate_absolute_ref("conversation source", &source.source_ref)?;
        validate_absolute_ref("conversation runtime", &source.runtime_ref)?;
        if source.display_name.trim().is_empty() {
            return invalid("conversation source display name must not be blank");
        }
        if let Some(path) = &source.performance_database_path {
            if path.contains('\0') || !Path::new(path).is_absolute() {
                return invalid(
                    "conversation source performance database path must be absolute and contain no NUL bytes",
                );
            }
        }
        if !source.capabilities.can_read {
            return invalid("a declared conversation source must support reading");
        }
        if !refs.insert(&source.source_ref) {
            return invalid(format!(
                "host ready event repeats conversation source {}",
                source.source_ref
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_root_conversation(conversation: &RetainedConversation) -> CwrResult<()> {
    validate_absolute_ref("conversation", &conversation.conversation_ref)?;
    if let Some(title) = &conversation.title {
        if normalize_conversation_title(title)? != *title {
            return invalid("conversation title must already be normalized");
        }
    }
    if conversation.parent_conversation_ref.is_some() {
        return invalid(format!(
            "host conversation snapshot must contain roots, but {} has a parent",
            conversation.conversation_ref
        ));
    }
    for participant_ref in &conversation.participant_actor_refs {
        validate_ref("conversation participant", participant_ref)?;
    }
    let mut message_refs = BTreeSet::new();
    for message in &conversation.messages {
        validate_absolute_ref("conversation message", &message.message_ref)?;
        validate_ref("conversation message author", &message.author_actor_ref)?;
        for actor_ref in &message.addressed_actor_refs {
            validate_ref("addressed conversation actor", actor_ref)?;
        }
        if !message_refs.insert(&message.message_ref) {
            return invalid(format!(
                "conversation {} repeats message {}",
                conversation.conversation_ref, message.message_ref
            ));
        }
    }
    if let Some(timeline_connection_ref) = &conversation.timeline_connection_ref {
        validate_absolute_ref("conversation timeline connection", timeline_connection_ref)?;
        if conversation.parent_conversation_ref.is_some() {
            return invalid("private execution conversations cannot name a shared timeline");
        }
    }
    let mut activity_refs = BTreeSet::new();
    for activity in &conversation.activity {
        validate_absolute_ref("conversation activity", &activity.activity_ref)?;
        if !activity_refs.insert(&activity.activity_ref) {
            return invalid(format!(
                "conversation {} repeats activity {}",
                conversation.conversation_ref, activity.activity_ref
            ));
        }
        match &activity.event {
            ConversationActivityEvent::WorkUpdate {
                invocation_ref,
                source_message_ref,
                parent_invocation_ref,
                wait,
                actor_ref,
                activity,
                ..
            } => {
                validate_absolute_ref("work invocation", invocation_ref)?;
                validate_absolute_ref("work source message", source_message_ref)?;
                validate_ref("work actor", actor_ref)?;
                if !message_refs.contains(source_message_ref) {
                    return invalid("work update refers to a message outside its conversation");
                }
                if let Some(parent_invocation_ref) = parent_invocation_ref {
                    validate_absolute_ref("parent work invocation", parent_invocation_ref)?;
                    if parent_invocation_ref == invocation_ref {
                        return invalid("work update refers to itself as its parent invocation");
                    }
                }
                if let Some(wait) = wait {
                    validate_absolute_ref(
                        "work wait waiter invocation",
                        &wait.waiter_invocation_ref,
                    )?;
                    validate_absolute_ref(
                        "work wait awaited invocation",
                        &wait.awaited_invocation_ref,
                    )?;
                    if Some(wait.waiter_invocation_ref.as_str())
                        != parent_invocation_ref.as_deref()
                        || wait.awaited_invocation_ref != *invocation_ref
                    {
                        return invalid("work wait does not match its invocation relationship");
                    }
                }
                if let Some(child_invocation_ref) = activity
                    .as_ref()
                    .and_then(|activity| activity.child_invocation_ref.as_deref())
                {
                    validate_absolute_ref("child work invocation", child_invocation_ref)?;
                }
            }
            ConversationActivityEvent::ProviderThread { .. }
            | ConversationActivityEvent::ProviderEvent { .. }
            | ConversationActivityEvent::CommentaryProgress { .. } => {
                return invalid("host conversation event contains provider-private activity")
            }
            ConversationActivityEvent::TurnStarted {
                invocation: Some(_),
                ..
            } => {
                return invalid(
                    "host conversation event contains private adapter invocation parameters",
                )
            }
            ConversationActivityEvent::ConversationForkedFrom {
                source_conversation_ref,
                source_message_ref,
                forked_by_actor_ref,
                operation_id,
                ..
            } => {
                validate_absolute_ref("fork source conversation", source_conversation_ref)?;
                validate_absolute_ref("fork source message", source_message_ref)?;
                validate_ref("fork actor", forked_by_actor_ref)?;
                require_nonblank("fork operation ID", operation_id)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_absolute_ref(label: &str, reference: &str) -> CwrResult<()> {
    let parsed = CwrRef::parse(reference)
        .map_err(|error| invalid_error(format!("invalid {label} ref {reference}: {error}")))?;
    if reference.starts_with("~@") || parsed.fragment().is_some() {
        return invalid(format!("{label} ref must be absolute and unfragmented"));
    }
    Ok(())
}

fn validate_ref(label: &str, reference: &str) -> CwrResult<()> {
    CwrRef::parse(reference)
        .map(|_| ())
        .map_err(|error| invalid_error(format!("invalid {label} ref {reference}: {error}")))
}

fn require_nonblank(label: &str, value: &str) -> CwrResult<()> {
    if value.trim().is_empty() {
        return invalid(format!("{label} must not be blank"));
    }
    Ok(())
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}
