impl LocalRuntime {
    /// Only the selected host connection may post as its enabled, actionless software delegate.
    #[expect(
        clippy::too_many_arguments,
        reason = "The host binds origin, provider ownership and the exact recipient separately."
    )]
    pub fn submit_delegated_text(
        &self,
        owner: &str,
        host_actor: &str,
        provider: &str,
        author: &str,
        target: &str,
        source: super::upstream::DelegationSource,
        text: String,
        request_id: &str,
    ) -> CwrResult<LocalAdmission> {
        if source.actor_ref != author
            || source.installation_ref.trim().is_empty()
            || source.release_digest.trim().is_empty()
            || text.trim().is_empty()
            || text.len() > 80_000
        {
            return invalid("delegation needs an attributed source and nonblank task under 80 KiB");
        }
        crate::CwrRef::parse(&source.invocation_ref)?;
        let (integration, context) = {
            let mut state = self.lock()?;
            let registered = state
                .upstream
                .visible(Some(owner))?
                .into_iter()
                .any(|route| {
                    route.provider_session_ref == provider
                        && route.coworker.actor_ref == author
                        && route.coworker.actions.is_empty()
                });
            if !registered {
                return invalid("delegated author is not enabled on this provider connection");
            }
            let catalog = local_catalog(&state.graph, host_actor)?;
            let recipient = catalog
                .actors
                .iter()
                .find(|actor| {
                    actor.actor_ref == target
                        && actor.category == Some(super::graph::CoworkerCategory::Ai)
                })
                .ok_or_else(|| {
                    invalid_error("delegation recipient is not a current permitted AI coworker")
                })?;
            let integration = recipient.actor_integration_ref.clone();
            let context = match state.admission.context_policy.clone() {
                Some(mut policy) => {
                    if policy.suspended {
                        return invalid("delegation is awaiting current host availability");
                    }
                    let current = policy
                        .contexts
                        .get(&policy.default_context_ref)
                        .ok_or_else(|| invalid_error("host context is unavailable"))?;
                    if !current.iter().any(|actor| actor == target) {
                        return invalid(
                            "delegation recipient is unavailable in the current host context",
                        );
                    }
                    let mut actors = current.clone();
                    if !actors.iter().any(|actor| actor == author) {
                        actors.push(author.into());
                    }
                    let retained = state
                        .admission
                        .admission(INTRINSIC_CWR_HOST_INTEGRATION_REF, request_id)
                        .and_then(|record| state.admission.conversation_header(&record.conversation_ref))
                        .and_then(|conversation| conversation.owner_context_ref.clone());
                    let context = match retained {
                        Some(context) if policy.contexts.contains_key(&context) => context,
                        Some(_) => return invalid("delegated request context was revoked"),
                        None => format!(
                            "{}/{}",
                            delegation_context_prefix(provider, Some(author)),
                            Uuid::new_v4().simple()
                        ),
                    };
                    if policy.contexts.get(&context) != Some(&actors) {
                        policy.contexts.insert(context.clone(), actors);
                        let policy = Some(policy);
                        state.admission.persist_context_policy(&policy)?;
                        state.admission.context_policy = policy;
                    }
                    Some(context)
                }
                None => None,
            };
            (integration, context)
        };
        let envelope = json!({
            "type": "cwp://org.coworkerprotocol.local/host-delegation/1",
            "source": source,
            "authority": "Delegated software task, not a new person authorization. The selected worker retains its configured tools and independent policy. No landing, release, destructive cleanup, environment removal or arbitrary command execution is authorized by this delegation.",
            "task": text,
        });
        self.admit_ingress_in_scope(
            ConversationIngress {
                owner_context_ref: context,
                file_refs: Vec::new(),
                integration_ref: INTRINSIC_CWR_HOST_INTEGRATION_REF.into(),
                provider_event_ref: request_id.into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Addressed {
                    actor_refs: vec![target.into()],
                },
                text: serde_json::to_string(&envelope)?,
            },
            Some(&integration),
            Some(owner),
        )
    }
}

fn delegation_context_prefix(provider: &str, author: Option<&str>) -> String {
    let base = format!("{provider}/host-service-delegations/");
    author.map_or(base.clone(), |author| {
        format!("{base}{}", hex::encode(Sha256::digest(author.as_bytes())))
    })
}
fn remove_delegation_contexts(
    state: &mut LocalRuntimeState,
    providers: &[String],
    author: Option<&str>,
) -> CwrResult<()> {
    let Some(mut policy) = state.admission.context_policy.clone() else {
        return Ok(());
    };
    let before = policy.contexts.len();
    policy.contexts.retain(|context, _| {
        !providers
            .iter()
            .any(|provider| context.starts_with(&delegation_context_prefix(provider, author)))
    });
    if policy.contexts.len() != before {
        let policy = Some(policy);
        state.admission.persist_context_policy(&policy)?;
        state.admission.context_policy = policy;
    }
    Ok(())
}
