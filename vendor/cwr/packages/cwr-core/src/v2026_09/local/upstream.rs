use super::upstream::{
    UpstreamCoworker, UpstreamEvent, UpstreamRegistry, UpstreamRoute, UpstreamWork,
};

const UPSTREAM_ADAPTER_REF: &str = "cwp://org.coworkerprotocol.local/adapters/upstream";
type UpstreamWriter = Arc<dyn Fn(UpstreamEvent) -> CwrResult<()> + Send + Sync>;

struct LocalUpstreamProvider {
    owner: String,
    writer: UpstreamWriter,
    deadline_stop_sender: std::sync::mpsc::SyncSender<UpstreamEvent>,
}

enum UpstreamStopDelivery {
    Idle,
    Retrying,
}

struct PendingUpstreamWork {
    route: UpstreamRoute,
    terminal: Option<AdapterTerminal>,
    disconnected: bool,
    publication_error: Option<String>,
    stop_requested: bool,
    stop_acknowledged: bool,
    stop_delivery: UpstreamStopDelivery,
}

impl LocalRuntime {
    /// Local-operator bootstrap may bind reads/admission to a live native provider.
    /// This does not grant provider completion or registration ownership.
    pub fn upstream_provider_scope(&self, provider: &str) -> CwrResult<String> {
        let state = self.lock()?;
        state
            .upstream_providers
            .get(provider)
            .map(|entry| entry.owner.clone())
            .ok_or_else(|| std::io::Error::other("upstream provider is unavailable").into())
    }

    pub fn register_upstream_provider(
        &self,
        owner: &str,
        coworkers: Vec<UpstreamCoworker>,
        writer: UpstreamWriter,
        deadline_stop_sender: std::sync::mpsc::SyncSender<UpstreamEvent>,
    ) -> CwrResult<String> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        let mut registry = state.upstream.clone();
        let known = state
            .catalog
            .known_actors
            .iter()
            .map(|actor| (actor.actor_ref.clone(), actor.handle.clone()))
            .collect::<Vec<_>>();
        let session = registry.register(
            state.admission.runtime_root_ref(),
            owner,
            coworkers.clone(),
            &known,
        )?;
        // Display evidence belongs to the same retained identity store used by
        // publication reads. This does not add actors to the graph, contexts,
        // participants, or connection-scoped action availability.
        let identities = coworkers
            .iter()
            .map(|coworker| LocalActorIdentity {
                actor_ref: coworker.actor_ref.clone(),
                coworker_ref: coworker.coworker_ref.clone(),
                handle: coworker.handle.clone(),
                name: coworker.name.clone(),
                category: Some(crate::v2026_09::graph::CoworkerCategory::Software),
                provider_model: None,
            })
            .collect::<Vec<_>>();
        // Live catalogs omit disconnected providers and removed local actors.
        // Check durable identity evidence before changing either retained store:
        // an actor ref must never relabel historical activity as another coworker.
        let retained = state.admission.retain_local_identities(&[])?;
        for identity in &identities {
            if retained
                .iter()
                .any(|actor| actor.actor_ref == identity.actor_ref && actor != identity)
            {
                return invalid("upstream actor ref conflicts with a retained identity");
            }
        }
        state.admission.retain_upstream_declarations(&coworkers)?;
        state.admission.retain_local_identities(&identities)?;
        state.upstream = registry;
        state.upstream_providers.insert(
            session.clone(),
            LocalUpstreamProvider {
                owner: owner.into(),
                writer,
                deadline_stop_sender,
            },
        );
        Ok(session)
    }

    pub fn enable_upstream_coworker(
        &self,
        owner: &str,
        session: &str,
        actor: &str,
        host_actor: &str,
        enabled: bool,
    ) -> CwrResult<()> {
        let mut state = self.lock()?;
        // Person authorship comes from the selected checked-graph actor.
        local_catalog(&state.graph, host_actor)?;
        let mut registry = state.upstream.clone();
        let known = state
            .catalog
            .known_actors
            .iter()
            .map(|actor| (actor.actor_ref.clone(), actor.handle.clone()))
            .collect::<Vec<_>>();
        registry.enable(owner, session, actor, enabled, &known)?;
        if !enabled {
            remove_delegation_contexts(&mut state, &[session.to_owned()], Some(actor))?;
        }
        state.upstream = registry;
        Ok(())
    }

    pub fn upstream_catalog_as(&self, actor: &str, owner: Option<&str>) -> CwrResult<LocalCatalog> {
        let state = self.lock()?;
        let mut catalog = local_catalog(&upstream_graph(&state, owner)?, actor)?;
        catalog.retained_actors = state.catalog.retained_actors.clone();
        Ok(catalog)
    }

    pub fn disconnect_upstream_provider(&self, owner: &str) -> CwrResult<()> {
        let mut state = self.lock()?;
        let mut registry = state.upstream.clone();
        let removed = registry.disconnect(owner);
        remove_delegation_contexts(&mut state, &removed, None)?;
        state.upstream = registry;
        state
            .gateway_handlers
            .retain(|(handler_owner, _), _| handler_owner != owner);
        for session in &removed {
            state.upstream_providers.remove(session);
        }
        for pending in state.upstream_work.values_mut() {
            if removed.contains(&pending.route.provider_session_ref) {
                pending.disconnected = true;
            }
        }
        state
            .host_local_invocation_owners
            .retain(|_, pending_owner| pending_owner != owner);
        drop(state);
        self.fail_unavailable_upstream()
    }

    pub fn complete_upstream_work(
        &self,
        owner: &str,
        invocation: &str,
        terminal: AdapterTerminal,
    ) -> CwrResult<()> {
        if terminal.invocation_ref != invocation {
            return invalid("upstream terminal names another invocation");
        }
        let status = adapter_terminal(&terminal)?.status;
        if terminal.continuation.is_some()
            || terminal.sequence != 1
            || (status.is_completed() && terminal.error.is_some())
            || (!status.is_completed() && terminal.result.is_some())
        {
            return invalid("upstream terminal has unsupported or conflicting fields");
        }
        let mut state = self.lock()?;
        if state
            .admission
            .invocation_record(invocation)
            .is_some_and(|record| record.state.is_terminal())
        {
            let route = retained_upstream_route(&state, invocation)
                .ok_or_else(|| invalid_error("not an upstream invocation"))?;
            if !state.upstream.owns_route(owner, &route) {
                return invalid("only the selected provider may complete upstream work");
            }
            state.settle_stopped_invocation(invocation, current_unix_ms())?;
            if let Some(pending) = state.upstream_work.get_mut(invocation) {
                pending.stop_acknowledged = true;
            }
            if !state.active.contains_key(invocation) {
                state.upstream_work.remove(invocation);
            }
            return Ok(());
        }
        let pending = state
            .upstream_work
            .get(invocation)
            .ok_or_else(|| invalid_error("unknown upstream invocation"))?;
        if !state.upstream.owns_route(owner, &pending.route) {
            return invalid("only the selected provider may complete upstream work");
        }
        let record = state
            .admission
            .invocation_record(invocation)
            .ok_or_else(|| invalid_error("upstream invocation is unavailable"))?;
        if status.is_completed() {
            require_native_gateway_result_current(&state, record)?;
        }
        if let Some(result) = &terminal.result {
            let schema = if record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
                crate::v2026_09::conversation::respond_to_message_output_schema()
            } else {
                pending
                    .route
                    .coworker
                    .actions
                    .iter()
                    .find(|action| action.action_name == record.action_name)
                    .ok_or_else(|| invalid_error("upstream action is unavailable"))?
                    .output_schema
                    .clone()
            };
            jsonschema::validator_for(&schema)?
                .validate(result)
                .map_err(|error| invalid_error(error.to_string()))?;
            if record.action_name == RESPOND_TO_MESSAGE_ACTION_NAME
                && result["text"]
                    .as_str()
                    .is_some_and(|text| text.trim().is_empty())
            {
                return invalid("respond_to--message text must not be blank");
            }
        }
        let pending = state.upstream_work.get_mut(invocation).unwrap();
        if pending.terminal.is_none() {
            pending.terminal = Some(terminal);
        }
        drop(state);
        loop {
            let state = self.lock()?;
            if state
                .admission
                .invocation_record(invocation)
                .is_some_and(|record| record.state.is_terminal())
            {
                return Ok(());
            }
            if let Some(error) = state
                .upstream_work
                .get(invocation)
                .and_then(|pending| pending.publication_error.as_ref())
            {
                return invalid(error.clone());
            }
            drop(state);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

include!("upstream/execution.rs");
include!("upstream/catalog.rs");
include!("upstream/stop.rs");
include!("upstream/tests.rs");
