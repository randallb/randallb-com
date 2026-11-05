/// Trusted native binding for an installed owner. Never construct this from
/// ordinary action input or a renderer's currently selected conversation.
pub struct LocalInstalledActionContext<'a> {
    runtime: &'a LocalRuntime,
    owner_context_ref: String,
    author_actor_ref: String,
    ingress_ref: String,
}

struct OwnedNamedAction<'a> {
    activation: &'a str,
    upstream_owner: Option<&'a str>,
}

// The mode is part of durable request identity. Other action entry points must
// not reconstruct an explicit owner destination from a retained receipt.
const OWNER_NAMED_ACTION_FINGERPRINT_PREFIX: &str = "owner-action-v1:sha256:";

impl LocalRuntime {
    /// Admit one named action at an explicit installed owner destination. The
    /// authenticated host scope and current owner policy supply authority; the
    /// execution record is not a person-created conversation.
    #[expect(
        clippy::too_many_arguments,
        reason = "The authenticated action carries its installed owner and upstream session scope."
    )]
    pub fn admit_owner_action_in_upstream_scope(
        &self,
        integration_ref: &str,
        request_id: &str,
        author_actor_ref: &str,
        owner_context_ref: &str,
        activation: &str,
        input: Value,
        upstream_owner: Option<&str>,
    ) -> CwrResult<LocalAdmission> {
        let binding =
            self.installed_action_context_as(owner_context_ref, author_actor_ref, integration_ref)?;
        let mut state = self.lock()?;
        let graph = upstream_graph(&state, upstream_owner)?;
        let catalog = local_catalog(&graph, author_actor_ref)?;
        let action = catalog
            .action(activation)
            .ok_or_else(|| invalid_error(format!("unknown action activation {activation}")))?;
        let mut routes = graph.routes.iter().filter(|route| {
            route.actor_ref == action.actor_ref
                && route.actor_integration_ref == action.actor_integration_ref
                && route.action_name == action.action_name
        });
        let route = routes
            .next()
            .ok_or_else(|| invalid_error("installed exact action target is unavailable"))?;
        if routes.next().is_some() {
            return invalid("installed action target is ambiguous");
        }
        let target = route_action_target(&graph.runtime_ref, route);
        binding.admit_with_named_locked(
            &mut state,
            request_id,
            &target,
            input,
            Some(OwnedNamedAction {
                activation,
                upstream_owner,
            }),
        )
    }

    /// The native installer/schedules service supplies its authenticated binding.
    /// Binding does not grant authority; first admission rechecks the live policy.
    pub fn installed_action_context_as(
        &self,
        owner_context_ref: &str,
        author_actor_ref: &str,
        ingress_ref: &str,
    ) -> CwrResult<LocalInstalledActionContext<'_>> {
        if owner_context_ref.trim().is_empty() || owner_context_ref.len() > 2048 {
            return invalid("installed owner context is required");
        }
        for reference in [author_actor_ref, ingress_ref] {
            let parsed = CwrRef::parse(reference)?;
            if reference.starts_with("~@") || parsed.fragment().is_some() {
                return invalid("installed action binding requires absolute unfragmented refs");
            }
        }
        Ok(LocalInstalledActionContext {
            runtime: self,
            owner_context_ref: owner_context_ref.into(),
            author_actor_ref: author_actor_ref.into(),
            ingress_ref: ingress_ref.into(),
        })
    }
}

impl LocalInstalledActionContext<'_> {
    /// One ordinary standalone action in a fresh, visible owned conversation.
    /// Request IDs are stable across retries and include no target discovery.
    pub fn admit(
        &self,
        request_id: &str,
        target: &ExactActionTarget,
        input: Value,
    ) -> CwrResult<LocalAdmission> {
        let mut state = self.runtime.lock()?;
        self.admit_locked(&mut state, request_id, target, input)
    }

    fn admit_locked(
        &self,
        state: &mut LocalRuntimeState,
        request_id: &str,
        target: &ExactActionTarget,
        input: Value,
    ) -> CwrResult<LocalAdmission> {
        self.admit_with_named_locked(state, request_id, target, input, None)
    }

    fn admit_with_named_locked(
        &self,
        state: &mut LocalRuntimeState,
        request_id: &str,
        target: &ExactActionTarget,
        input: Value,
        named: Option<OwnedNamedAction<'_>>,
    ) -> CwrResult<LocalAdmission> {
        if request_id.is_empty() || request_id.len() > 256 {
            return invalid("occurrence request identity must contain 1 to 256 bytes");
        }
        let activation = named.as_ref().map_or_else(
            || format!("@scheduled--{}", target.action_name),
            |named| named.activation.to_owned(),
        );
        let text = action_call_text(&activation, &input)?;
        let ingress = ConversationIngress {
            owner_context_ref: Some(self.owner_context_ref.clone()),
            file_refs: Vec::new(),
            integration_ref: self.ingress_ref.clone(),
            provider_event_ref: request_id.into(),
            author_actor_ref: self.author_actor_ref.clone(),
            conversation: ConversationIngressConversation::CreateForAction,
            addressing: ConversationIngressAddressing::Action {
                actor_ref: target.coworker_ref.clone(),
                activation,
                action_name: target.action_name.clone(),
                input,
            },
            text,
        };
        let fingerprint = format!(
            "{}{}",
            if named.is_some() {
                OWNER_NAMED_ACTION_FINGERPRINT_PREFIX
            } else {
                "sha256:"
            },
            hex::encode(Sha256::digest(serde_json::to_vec(&(
                target,
                &self.owner_context_ref
            ))?))
        );
        // Caller authentication applies to receipt recovery. Named owner actions
        // also recheck current authority; schedules retain their receipt rules.
        let graph = match &named {
            Some(named) => upstream_graph(state, named.upstream_owner)?,
            None => state.graph.clone(),
        };
        local_catalog(&graph, &self.author_actor_ref)?;
        let recovered = state.admission.recover_action(&ingress)?;
        if let Some(record) = &recovered {
            if record.source_input_fingerprint.as_deref() != Some(&fingerprint) {
                return invalid("occurrence identity was reused with a different exact target");
            }
        }
        // Existing schedule receipts remain recoverable after removal. Native
        // owner-destination actions also require their current exact authority.
        if named.is_none() {
            if let Some(record) = recovered {
                return owned_action_admission(state, record, None);
            }
        }
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        if target.runtime_ref != graph.runtime_ref {
            return invalid("installed action target belongs to another runtime");
        }
        // An installed context must be explicit in the authoritative host policy.
        // No default-context fallback is permitted, including an unconfigured host.
        if !state
            .admission
            .context_policy
            .as_ref()
            .is_some_and(|policy| {
                !policy.suspended
                    && policy
                        .contexts
                        .get(&self.owner_context_ref)
                        .is_some_and(|actors| {
                            named.is_none()
                                || (self.owner_context_ref != policy.default_context_ref
                                    && actors.contains(&target.coworker_ref))
                        })
            })
        {
            return invalid("installed action context is unavailable");
        }
        let route = graph
            .routes
            .iter()
            .find(|route| {
                route.actor_ref == target.coworker_ref
                    && route.role_ref == target.role_ref
                    && route.actor_integration_ref == target.binding_ref
                    && route.action_name == target.action_name
            })
            .ok_or_else(|| invalid_error("installed exact action target is unavailable"))?;
        if route_action_target(&graph.runtime_ref, route) != *target {
            return invalid("installed action contract changed");
        }
        // Administrative and gateway entry points retain their existing owning
        // operations rather than becoming ordinary installed action targets.
        if route.actor_ref == INTRINSIC_CWR_ACTOR_REF
            || graph
                .gateways
                .iter()
                .any(|gateway| gateway.resource_ref == route.actor_ref)
        {
            return invalid("unsupported installed action target");
        }
        let allowed = action_actor_allowlist(&graph, &self.author_actor_ref, None)?;
        if allowed
            .as_ref()
            .is_some_and(|actors| !actors.contains(&route.actor_ref))
        {
            return invalid("installed action is not authorized");
        }
        route.require_available()?;
        route.validate_input(match &ingress.addressing {
            ConversationIngressAddressing::Action { input, .. } => input,
            _ => unreachable!(),
        })?;
        state.admission.check_ingress_context(&graph, &ingress)?;
        if let Some(record) = recovered {
            return owned_action_admission(
                state,
                record,
                named.as_ref().and_then(|named| named.upstream_owner),
            );
        }
        // Validation, policy fencing and commit share the runtime mutex. A
        // disablement cannot slip between current authority and admission.
        let outcome = if let Some(named) = &named {
            let host_local_actor_refs = named
                .upstream_owner
                .map(|owner| {
                    state.upstream.visible(Some(owner)).map(|routes| {
                        routes
                            .into_iter()
                            .map(|route| route.coworker.actor_ref)
                            .collect::<Vec<_>>()
                    })
                })
                .transpose()?
                .unwrap_or_default();
            state.admission.admit_host_local(
                &graph,
                &ingress,
                Some(&route.actor_integration_ref),
                &host_local_actor_refs,
                None,
                Some(&fingerprint),
            )?
        } else {
            let mut prepared =
                state
                    .admission
                    .prepare(&graph, &ingress, Some(&route.actor_integration_ref))?;
            if let crate::v2026_09::conversation::ConversationPreparationOutcome::New(turn) =
                &mut prepared
            {
                turn.record.source_input_fingerprint = Some(fingerprint);
                turn.conversation.sidebar_eligible = Some(true);
            }
            state.admission.commit(prepared)?
        };
        self.runtime.sync_archive_reservations(state)?;
        let record = match outcome {
            ConversationAdmissionOutcome::Started(turn) => turn.record,
            ConversationAdmissionOutcome::Existing(record) => *record,
        };
        owned_action_admission(
            state,
            record,
            named.as_ref().and_then(|named| named.upstream_owner),
        )
    }
}

fn owned_action_admission(
    state: &mut LocalRuntimeState,
    record: ConversationAdmissionRecord,
    upstream_owner: Option<&str>,
) -> CwrResult<LocalAdmission> {
    if let (Some(invocation), Some(owner)) = (&record.invocation_ref, upstream_owner) {
        state
            .host_local_invocation_owners
            .entry(invocation.clone())
            .or_insert_with(|| owner.to_owned());
    }
    let conversation = state
        .admission
        .full_conversation(&record.conversation_ref)?
        .ok_or_else(|| invalid_error("owned admission has no conversation"))?;
    Ok(LocalAdmission {
        record,
        conversation,
    })
}
