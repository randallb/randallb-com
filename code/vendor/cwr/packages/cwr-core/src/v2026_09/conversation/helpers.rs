/// Returns whether a projected work update belongs to a root invocation's
/// execution tree.
pub fn work_invocation_belongs_to_root(
    conversation: &RetainedConversation,
    invocation_ref: &str,
    root_invocation_ref: &str,
) -> bool {
    let mut current = invocation_ref;
    let mut remaining = conversation.activity.len() + 1;
    while remaining > 0 {
        if current == root_invocation_ref {
            return true;
        }
        let Some(parent) =
            conversation
                .activity
                .iter()
                .find_map(|activity| match &activity.event {
                    ConversationActivityEvent::WorkUpdate {
                        invocation_ref,
                        parent_invocation_ref: Some(parent_invocation_ref),
                        ..
                    } if invocation_ref == current => Some(parent_invocation_ref.as_str()),
                    _ => None,
                })
        else {
            return false;
        };
        current = parent;
        remaining -= 1;
    }
    false
}

impl RetainedConversation {
    /// Sidebar membership does not restrict direct history or work-detail reads.
    pub fn is_sidebar_eligible(&self) -> bool {
        // Unmarked roots may be person-created even when their first message is an action.
        self.parent_conversation_ref.is_none() && self.sidebar_eligible.unwrap_or(true)
    }

    fn record_message(&mut self, message: ConversationMessage) {
        self.activity.push(ConversationActivity {
            activity_ref: format!("{}/recorded", message.message_ref),
            sequence: self.activity.len() as u64 + 1,
            occurred_at_unix_ms: current_unix_ms(),
            event: ConversationActivityEvent::MessageRecorded {
                message_ref: message.message_ref.clone(),
            },
        });
        self.messages.push(message);
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "The persistence helper receives every field of the action message it commits atomically."
)]
fn push_action_message_in_state(
    state: &mut ConversationRuntimeState,
    message_ref: String,
    conversation_ref: &str,
    author_actor_ref: &str,
    addressed_actor_ref: &str,
    text: String,
    reply_to_message_ref: Option<String>,
    action: ConversationActionEvent,
) -> CwrResult<ConversationMessage> {
    if text.trim().is_empty() {
        return invalid("conversation action message text must not be empty");
    }
    let host_controls = [author_actor_ref, addressed_actor_ref]
        .into_iter()
        .filter(|actor_ref| {
            state.conversations.iter().any(|conversation| {
                conversation.activity.iter().any(|activity| {
                    if let ConversationActivityEvent::TurnStarted {
                        invocation: Some(snapshot),
                        ..
                    } = &activity.event
                    {
                        return snapshot
                            .get("adapter_params")
                            .and_then(|params| params.get("upstream_route"))
                            .and_then(|route| route.get("coworker"))
                            .and_then(|coworker| coworker.get("actor_ref"))
                            .and_then(Value::as_str)
                            == Some(*actor_ref);
                    }
                    false
                })
            })
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let conversation = state
        .conversations
        .find_mut(|conversation| conversation.conversation_ref == conversation_ref)
        .ok_or_else(|| invalid_error(format!("unknown conversation ref {conversation_ref}")))?;
    for actor_ref in [author_actor_ref, addressed_actor_ref] {
        if host_controls.iter().any(|actor| actor == actor_ref) {
            continue;
        }
        if !conversation
            .participant_actor_refs
            .iter()
            .any(|participant| participant == actor_ref)
        {
            conversation.participant_actor_refs.push(actor_ref.into());
        }
    }
    let message = ConversationMessage {
        file_refs: Vec::new(),
        progress: None,
        message_ref,
        author_actor_ref: author_actor_ref.into(),
        addressed_actor_refs: vec![addressed_actor_ref.into()],
        addressing_mode: Some(if matches!(action, ConversationActionEvent::Call { .. }) {
            ConversationAddressingMode::Explicit
        } else {
            ConversationAddressingMode::Implicit
        }),
        text,
        visualizations: Vec::new(),
        reply_to_message_ref,
        action: Some(action),
    };
    conversation.record_message(message.clone());
    Ok(message)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RespondToMessageResult {
    text: String,
    #[serde(default, rename = "answers_response")]
    _answers_response: Option<crate::adapter::user_input::UserInputResponse>,
}

fn validate_runtime_root(reference: &str) -> CwrResult<()> {
    let parsed = CwrRef::parse(reference)
        .map_err(|error| invalid_error(format!("invalid runtime root ref {reference}: {error}")))?;
    if reference.starts_with("~@") || parsed.fragment().is_some() {
        return invalid("conversation admission runtime root must be an absolute unfragmented ref");
    }
    Ok(())
}

fn validate_ingress(graph: &ResolvedCwrGraph, ingress: &ConversationIngress) -> CwrResult<()> {
    if ingress.integration_ref.is_empty()
        || ingress.provider_event_ref.is_empty()
        || (ingress.text.trim().is_empty() && ingress.file_refs.is_empty())
    {
        return invalid("conversation ingress refs and text must not be empty");
    }
    if matches!(
        ingress.conversation,
        ConversationIngressConversation::CreateForAction
    ) && !matches!(
        ingress.addressing,
        ConversationIngressAddressing::Action { .. }
    ) {
        return invalid("create_for_action conversation ingress requires action addressing");
    }
    validate_ref("integration", &ingress.integration_ref)?;
    validate_ref("author actor", &ingress.author_actor_ref)?;
    if !graph
        .actors
        .iter()
        .any(|actor| actor.actor_ref == ingress.author_actor_ref)
    {
        return invalid(format!(
            "conversation ingress author actor {} is not in the resolved graph",
            ingress.author_actor_ref
        ));
    }
    if let ConversationIngressConversation::Continue { conversation_ref } = &ingress.conversation {
        validate_ref("conversation", conversation_ref)?;
    }
    Ok(())
}

fn addressed_actor_ref(
    graph: &ResolvedCwrGraph,
    addressing: &ConversationIngressAddressing,
) -> CwrResult<Option<String>> {
    let actor_ref = match addressing {
        ConversationIngressAddressing::Addressed { actor_refs } if actor_refs.len() == 1 => {
            actor_refs.first().cloned()
        }
        ConversationIngressAddressing::Addressed { .. } => {
            return invalid("the first conversation slice requires exactly one addressed actor")
        }
        ConversationIngressAddressing::Action { actor_ref, .. } => Some(actor_ref.clone()),
        ConversationIngressAddressing::UnresolvedLeadingHandle { handle } => {
            return invalid(format!(
                "leading handle {handle} must resolve to an exact actor ref before admission"
            ))
        }
        ConversationIngressAddressing::Unaddressed => None,
    };
    if let Some(actor_ref) = &actor_ref {
        validate_addressed_actor_ref(graph, actor_ref)?;
    }
    Ok(actor_ref)
}

fn message_addressing_mode(
    addressing: &ConversationIngressAddressing,
) -> ConversationAddressingMode {
    match addressing {
        ConversationIngressAddressing::Addressed { .. }
        | ConversationIngressAddressing::Action { .. } => ConversationAddressingMode::Explicit,
        ConversationIngressAddressing::Unaddressed => ConversationAddressingMode::Implicit,
        ConversationIngressAddressing::UnresolvedLeadingHandle { .. } => {
            unreachable!("unresolved handles fail before message retention")
        }
    }
}

fn validate_addressed_actor_ref(graph: &ResolvedCwrGraph, actor_ref: &str) -> CwrResult<()> {
    validate_ref("addressed actor", actor_ref)?;
    let gateway_handler = graph.gateways.iter().any(|gateway| {
        [
            Some(&gateway.route),
            gateway.ingress_route.as_ref(),
            gateway.delivery_route.as_ref(),
        ]
        .into_iter()
        .flatten()
        .any(|route| route.actor_ref == actor_ref)
    });
    if !gateway_handler
        && !graph
            .actors
            .iter()
            .any(|actor| actor.actor_ref == actor_ref)
    {
        return invalid(format!(
            "addressed actor {actor_ref} is not in the resolved graph"
        ));
    }
    Ok(())
}

fn select_action_route<'a>(
    graph: &'a ResolvedCwrGraph,
    actor_ref: &str,
    actor_integration_ref: &str,
    action_name: &str,
) -> CwrResult<&'a crate::v2026_09::graph::ResolvedCwrActionRoute> {
    validate_ref("actor integration", actor_integration_ref)?;
    let selected = graph
        .routes
        .iter()
        .chain(graph.gateways.iter().flat_map(|gateway| {
            [Some(&gateway.route), gateway.ingress_route.as_ref(), gateway.delivery_route.as_ref()]
                .into_iter()
                .flatten()
        }))
        .find(|route| {
            route.actor_ref == actor_ref
                && route.actor_integration_ref == actor_integration_ref
                && route.action_name == action_name
        })
        .ok_or_else(|| {
            invalid_error(format!(
                "actor {actor_ref} has no action {action_name} on integration {actor_integration_ref}"
            ))
        })?;
    selected.require_available()?;
    Ok(selected)
}

fn select_actor_integration<'a>(
    graph: &'a ResolvedCwrGraph,
    actor_ref: &str,
    actor_integration_ref: &str,
) -> CwrResult<&'a ResolvedCwrActorIntegration> {
    validate_ref("actor integration", actor_integration_ref)?;
    let selected = graph
        .actor_integrations
        .iter()
        .find(|integration| integration.actor_integration_ref == actor_integration_ref)
        .ok_or_else(|| {
            invalid_error(format!(
                "actor integration {actor_integration_ref} is not in the resolved graph"
            ))
        })?;
    if selected.actor_ref != actor_ref {
        return invalid(format!(
            "actor integration {actor_integration_ref} belongs to {}, not {actor_ref}",
            selected.actor_ref
        ));
    }
    selected.require_available()?;
    Ok(selected)
}

fn validate_ref(label: &str, reference: &str) -> CwrResult<()> {
    CwrRef::parse(reference)
        .map(|_| ())
        .map_err(|error| invalid_error(format!("invalid {label} ref {reference}: {error}")))
}

pub(crate) fn respond_to_message_output_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["text"],
        "properties": {"text": {"type": "string", "minLength": 1}}
    })
}

fn conversation_thread(conversation: &RetainedConversation) -> Value {
    let messages = conversation
        .messages
        .iter()
        .map(ConversationMessage::clone_for_external_delivery)
        .collect::<Vec<_>>();
    let mut thread = json!({
        "version": "cwr.thread-observation/2",
        "conversation_ref": conversation.conversation_ref,
        "messages": messages,
    });
    if let Some(title) = &conversation.title {
        thread["title"] = json!(title);
    }
    thread
}

fn push_unique(participants: &mut Vec<String>, actor_ref: &str) {
    if !participants
        .iter()
        .any(|participant| participant == actor_ref)
    {
        participants.push(actor_ref.into());
    }
}

fn new_ref(runtime_root_ref: &str, collection: &str) -> String {
    format!(
        "{runtime_root_ref}/{collection}/{}",
        Uuid::new_v4().simple()
    )
}

fn invalid<T>(message: impl Into<String>) -> CwrResult<T> {
    Err(invalid_error(message))
}

fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn invalid_error(message: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into()).into()
}
