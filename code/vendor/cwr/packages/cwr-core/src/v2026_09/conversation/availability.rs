use super::*;

#[derive(Debug)]
pub struct ConversationContextError(pub String);
impl std::fmt::Display for ConversationContextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConversationContextError {}
fn unavailable(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(ConversationContextError(message.into()))
}

impl ConversationAdmissionStore {
    pub(crate) fn check_ingress_context(
        &self,
        graph: &ResolvedCwrGraph,
        ingress: &ConversationIngress,
    ) -> CwrResult<()> {
        let conversation = self.ingress_conversation(ingress)?;
        let target = addressed_actor_ref(graph, &ingress.addressing)?.or_else(|| {
            self.last_addressed_route(&conversation, &ingress.author_actor_ref)
                .map(|route| route.0)
        });
        self.check_context_work(
            graph,
            conversation.owner_context_ref.as_deref(),
            matches!(
                ingress.conversation,
                ConversationIngressConversation::Continue { .. }
            ),
            &ingress.author_actor_ref,
            target.as_deref().map(|actor| (actor, None)),
            None,
        )
    }

    /// One admission rule for preflight, transaction validation, and execution recovery.
    pub(crate) fn check_context_work(
        &self,
        graph: &ResolvedCwrGraph,
        owner: Option<&str>,
        continuation: bool,
        author: &str,
        target: Option<(&str, Option<&str>)>,
        caller: Option<&crate::v2026_09::invocation::ActionInvocationRecord>,
    ) -> CwrResult<()> {
        let (target, action_name) =
            target.map_or((None, None), |(actor, action)| (Some(actor), action));
        check_work(
            &self.state,
            self.context_policy.as_ref(),
            self.context_policy_configured,
            graph,
            ContextWork {
                owner,
                continuation,
                author,
                target,
                action_name,
                caller,
            },
        )
    }

    pub(crate) fn check_context_execution(
        &self,
        graph: &ResolvedCwrGraph,
        record: &crate::v2026_09::invocation::ActionInvocationRecord,
    ) -> CwrResult<()> {
        let conversation = record
            .conversation_ref
            .as_deref()
            .and_then(|reference| self.conversation_header(reference));
        let caller = request_parent(&self.state, record);
        self.check_context_work(
            graph,
            conversation.and_then(|c| c.owner_context_ref.as_deref()),
            true,
            &record.requester_actor_ref,
            Some((&record.target_actor_ref, Some(&record.action_name))),
            caller,
        )
    }
}

fn request_parent<'a>(
    state: &'a ConversationRuntimeState,
    record: &crate::v2026_09::invocation::ActionInvocationRecord,
) -> Option<&'a crate::v2026_09::invocation::ActionInvocationRecord> {
    if let Some(reference) = record.parent_invocation_ref.as_deref() {
        return state.invocation_ledger.invocation(reference);
    }
    let crate::v2026_09::invocation::InvocationOrigin::Ingress {
        integration_ref,
        provider_event_ref,
    } = &record.origin
    else {
        return None;
    };
    if integration_ref == crate::action_contract::INTRINSIC_CWR_INTEGRATION_REF
        && record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
    {
        if let Some(reference) = record
            .input
            .pointer("/provider_question/caller")
            .and_then(Value::as_str)
        {
            return state.invocation_ledger.invocation(reference);
        }
    }
    let admission = state.admissions.iter().find(|admission| {
        &admission.integration_ref == integration_ref
            && &admission.provider_event_ref == provider_event_ref
    })?;
    let conversation = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == admission.conversation_ref)?;
    let reply_to = conversation
        .messages
        .iter()
        .find(|message| message.message_ref == admission.source_message_ref)?
        .reply_to_message_ref
        .as_deref()?;
    let previous = state.admissions.iter().find(|admission| {
        admission.conversation_ref == conversation.conversation_ref
            && admission.source_message_ref == reply_to
    })?;
    state
        .invocation_ledger
        .invocation(previous.invocation_ref.as_deref()?)
}

fn original_requester<'a>(
    state: &'a ConversationRuntimeState,
    mut record: &'a crate::v2026_09::invocation::ActionInvocationRecord,
) -> Option<&'a str> {
    let mut seen = std::collections::BTreeSet::new();
    while let Some(parent) = request_parent(state, record) {
        if !seen.insert(&parent.invocation_ref) {
            return None;
        }
        record = parent;
    }
    Some(&record.requester_actor_ref)
}

struct ContextWork<'a> {
    owner: Option<&'a str>,
    continuation: bool,
    author: &'a str,
    target: Option<&'a str>,
    action_name: Option<&'a str>,
    caller: Option<&'a crate::v2026_09::invocation::ActionInvocationRecord>,
}

fn check_work(
    state: &ConversationRuntimeState,
    policy: Option<&crate::v2026_09::upstream::ConversationContextPolicy>,
    configured: bool,
    graph: &ResolvedCwrGraph,
    work: ContextWork<'_>,
) -> CwrResult<()> {
    let ContextWork {
        owner,
        continuation,
        author,
        target,
        action_name,
        caller,
    } = work;
    let intrinsic = target == Some(crate::action_contract::INTRINSIC_CWR_ACTOR_REF);
    if intrinsic && continuation {
        return Ok(());
    }
    let Some(policy) = policy else {
        return if configured && owner.is_some() {
            Err(unavailable("conversation context requires its host policy"))
        } else {
            Ok(())
        };
    };
    if policy.suspended {
        return Err(unavailable(
            "conversation availability is awaiting current host policy",
        ));
    }
    let actors = policy
        .contexts
        .get(owner.unwrap_or(&policy.default_context_ref))
        .ok_or_else(|| unavailable("conversation context is unavailable"))?;
    let member = |actor: &str| actors.iter().any(|reference| reference == actor);
    // Gateway actors are transport endpoints already governed by gateway authority,
    // not coworkers selected by a host availability policy.
    let gateway = |actor: &str| {
        graph
            .gateways
            .iter()
            .any(|gateway| gateway.resource_ref == actor)
    };
    let person = |actor: &str| {
        graph.actors.iter().any(|known| {
            known.actor_ref == actor
                && (known.category == Some(crate::v2026_09::graph::CoworkerCategory::Human)
                    || !graph
                        .actor_integrations
                        .iter()
                        .any(|binding| binding.actor_ref == actor))
        }) || graph.actor_integrations.iter().any(|binding| {
            binding.actor_ref == actor && binding.adapter_ref == crate::adapter::PERSON_ADAPTER_REF
        })
    };
    let system =
        |actor: &str| actor == crate::action_contract::INTRINSIC_CWR_ACTOR_REF || gateway(actor);
    let permitted_action = |from: &str, to: &str, action: &str| {
        policy.action_permissions.iter().any(|permission| {
            permission.caller_actor_ref == from
                && permission.target_actor_ref == to
                && permission.action_names.iter().any(|name| name == action)
        })
    };
    // A private source may call its narrow host transport only while the retained
    // parent chain still authorizes the source invocation. Never trust an actor
    // identity supplied in action input, or turn this into chat membership.
    let source_caller = caller.is_some_and(|record| {
        record.target_actor_ref == author
            && permitted_action(&record.requester_actor_ref, author, &record.action_name)
    });
    if !member(author) && !system(author) && !person(author) && !source_caller {
        return Err(unavailable(
            "requesting coworker is unavailable in this conversation context",
        ));
    }
    if source_caller
        && !member(author)
        && target.is_some_and(|target| {
            !action_name.is_some_and(|name| permitted_action(author, target, name))
        })
    {
        return Err(unavailable(
            "source caller action is not authorized in this conversation context",
        ));
    }
    let mut ancestor = caller;
    let mut seen = std::collections::BTreeSet::new();
    while let Some(record) = ancestor {
        if !seen.insert(&record.invocation_ref) {
            return Err(unavailable("invalid invocation ancestry"));
        }
        if !member(&record.target_actor_ref)
            && !system(&record.target_actor_ref)
            && !person(&record.target_actor_ref)
            && !permitted_action(
                &record.requester_actor_ref,
                &record.target_actor_ref,
                &record.action_name,
            )
        {
            return Err(unavailable(
                "calling coworker is unavailable in this conversation context",
            ));
        }
        if !member(&record.requester_actor_ref)
            && !system(&record.requester_actor_ref)
            && !person(&record.requester_actor_ref)
            && !permitted_action(
                &record.requester_actor_ref,
                &record.target_actor_ref,
                &record.action_name,
            )
        {
            return Err(unavailable(
                "source ancestor action is no longer authorized in this conversation context",
            ));
        }
        ancestor = request_parent(state, record);
    }
    let requester = caller
        .and_then(|record| original_requester(state, record))
        .unwrap_or(author);
    if target.is_some_and(|target| {
        !(member(target)
            || system(target)
            || target == requester && person(target)
            || action_name.is_some_and(|name| permitted_action(author, target, name)))
    }) {
        return Err(unavailable(
            "coworker is unavailable in this conversation context",
        ));
    }
    Ok(())
}

pub(super) fn validate_new_context_work(
    state: &ConversationRuntimeState,
    graph: &ResolvedCwrGraph,
    policy: Option<&crate::v2026_09::upstream::ConversationContextPolicy>,
    configured: bool,
) -> CwrResult<()> {
    for (_, old, record) in state.admissions.changes() {
        if old.is_some() || record.invocation_ref.is_some() {
            continue;
        }
        let conversation = state
            .conversations
            .iter()
            .find(|conversation| conversation.conversation_ref == record.conversation_ref)
            .ok_or("admission conversation is unavailable")?;
        let author = conversation
            .messages
            .iter()
            .find(|message| message.message_ref == record.source_message_ref)
            .ok_or("admission source is unavailable")?;
        let continuation = !state.conversations.changes().any(|(_, old, new)| {
            old.is_none() && new.conversation_ref == conversation.conversation_ref
        });
        check_work(
            state,
            policy,
            configured,
            graph,
            ContextWork {
                owner: conversation.owner_context_ref.as_deref(),
                continuation,
                author: &author.author_actor_ref,
                target: record.target_actor_ref.as_deref(),
                action_name: None,
                caller: None,
            },
        )?;
    }
    for (_, old, new) in state.invocation_ledger.changes() {
        let Some(record) = new.filter(|_| old.is_none()) else {
            continue;
        };
        let conversation = record.conversation_ref.as_deref().and_then(|reference| {
            state
                .conversations
                .iter()
                .find(|c| c.conversation_ref == reference)
        });
        let continuation = conversation.is_some_and(|conversation| {
            !state.conversations.changes().any(|(_, old, new)| {
                old.is_none() && new.conversation_ref == conversation.conversation_ref
            })
        });
        let caller = request_parent(state, record);
        check_work(
            state,
            policy,
            configured,
            graph,
            ContextWork {
                owner: conversation.and_then(|c| c.owner_context_ref.as_deref()),
                continuation,
                author: &record.requester_actor_ref,
                target: Some(&record.target_actor_ref),
                action_name: Some(&record.action_name),
                caller,
            },
        )?;
    }
    Ok(())
}
