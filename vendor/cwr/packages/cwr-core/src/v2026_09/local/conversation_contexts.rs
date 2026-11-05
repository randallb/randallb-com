fn load_context_policy(
    path: &Path,
) -> CwrResult<Option<crate::v2026_09::upstream::ConversationContextPolicy>> {
    match fs::read(path) {
        Ok(bytes) => {
            let mut policy: Option<crate::v2026_09::upstream::ConversationContextPolicy> =
                serde_json::from_slice(&bytes)?;
            if let Some(policy) = &mut policy {
                policy.suspended = true;
            }
            validate_context_policy(&policy)?;
            Ok(policy)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
fn validate_context_policy(
    policy: &Option<crate::v2026_09::upstream::ConversationContextPolicy>,
) -> CwrResult<()> {
    if policy.as_ref().is_some_and(|policy| {
        !policy.contexts.contains_key(&policy.default_context_ref)
            || policy
                .contexts
                .keys()
                .any(|reference| reference.trim().is_empty())
            || policy
                .contexts
                .values()
                .flatten()
                .any(|reference| CwrRef::parse(reference).is_err())
            || policy.action_permissions.iter().any(|permission| {
                CwrRef::parse(&permission.caller_actor_ref).is_err()
                    || CwrRef::parse(&permission.target_actor_ref).is_err()
                    || permission.action_names.iter().any(|name| name.trim().is_empty())
            })
    }) {
        return invalid("invalid conversation context policy");
    }
    Ok(())
}
impl LocalRuntime {
    /// Close admissions without changing the retained navigation contexts.
    pub fn suspend_conversation_contexts(&self) -> CwrResult<()> {
        let mut policy = self
            .lock()?
            .admission
            .context_policy
            .clone()
            .unwrap_or_else(|| crate::v2026_09::upstream::ConversationContextPolicy {
                action_permissions: Vec::new(),
                suspended: true,
                default_context_ref: "cwr:availability-refresh".into(),
                contexts: BTreeMap::from([("cwr:availability-refresh".into(), Vec::new())]),
            });
        policy.suspended = true;
        self.configure_conversation_contexts(Some(policy))
    }

    /// Trusted native-host configuration. Shared by the runtime, not owned by a connection.
    /// Missing/removed contexts deny new work in retained owned conversations.
    pub fn configure_conversation_contexts(
        &self,
        policy: Option<crate::v2026_09::upstream::ConversationContextPolicy>,
    ) -> CwrResult<()> {
        let mut policy = policy;
        if let Some(policy) = &mut policy {
            if policy
                .default_context_ref
                .contains("/host-service-delegations/")
            {
                return invalid("host delegation contexts cannot be a root context");
            }
            // A new native policy revokes old connection-derived contexts. They
            // can be created only by a fresh admitted host-service request.
            policy
                .contexts
                .retain(|key, _| !key.contains("/host-service-delegations/"));
        }
        validate_context_policy(&policy)?;
        let mut state = self.lock()?;
        if policy.is_none()
            && !state.admission.context_policy_configured
            && state.admission.context_policy.is_none()
        {
            return Ok(());
        }
        state.admission.persist_context_policy(&policy)?;
        state.admission.context_policy = policy;
        state.admission.context_policy_configured = true;
        Ok(())
    }
    pub fn default_conversation_context(&self) -> CwrResult<Option<String>> {
        Ok(self
            .lock()?
            .admission
            .context_policy
            .as_ref()
            .map(|policy| policy.default_context_ref.clone()))
    }
    pub fn conversation_context_targets(
        &self,
        context: Option<&str>,
    ) -> CwrResult<Option<Vec<String>>> {
        let state = self.lock()?;
        if state
            .admission
            .context_policy
            .as_ref()
            .is_some_and(|policy| policy.suspended)
        {
            return Ok(Some(Vec::new()));
        }
        match state.admission.context_policy.as_ref() {
            Some(policy) => Ok(Some(
                policy
                    .contexts
                    .get(context.unwrap_or(&policy.default_context_ref))
                    .cloned()
                    .unwrap_or_default(),
            )),
            None if context.is_some() && state.admission.context_policy_configured => {
                Ok(Some(Vec::new()))
            }
            None => Ok(None),
        }
    }
}

fn scope_discovery_graph(
    state: &LocalRuntimeState,
    conversation_ref: Option<&str>,
    caller: &str,
    invocation: Option<&crate::v2026_09::invocation::ActionInvocationRecord>,
    graph: &mut ResolvedCwrGraph,
) -> CwrResult<()> {
    let context = conversation_ref
        .and_then(|reference| state.admission.conversation_header(reference))
        .and_then(|conversation| conversation.owner_context_ref.as_deref());
    let source = graph.clone();
    let allowed = |actor: &str, action: Option<&str>| {
        state
            .admission
            .check_context_work(
                &source,
                context,
                conversation_ref.is_some(),
                caller,
                Some((actor, action)),
                invocation,
            )
            .is_ok()
    };
    // Message delivery is a synthetic action, so it has no graph route to
    // preserve during discovery. A caller with a scoped response permission
    // still needs to see the coworker and its message binding.
    let visible_actor = |actor: &str| {
        allowed(actor, None) || allowed(actor, Some(RESPOND_TO_MESSAGE_ACTION_NAME))
    };
    graph.routes.retain(|route| allowed(&route.actor_ref, Some(&route.action_name)));
    let action_actors = graph.routes.iter().map(|route| route.actor_ref.clone()).collect::<BTreeSet<_>>();
    graph
        .actor_integrations
        .retain(|binding| {
            visible_actor(&binding.actor_ref) || action_actors.contains(&binding.actor_ref)
        });
    graph
        .actors
        .retain(|actor| {
            actor.actor_ref == caller
                || visible_actor(&actor.actor_ref)
                || action_actors.contains(&actor.actor_ref)
        });
    Ok(())
}

fn context_scope_error(message: &str) -> Box<dyn std::error::Error + Send + Sync> {
    Box::new(ConversationContextError(message.into()))
}
