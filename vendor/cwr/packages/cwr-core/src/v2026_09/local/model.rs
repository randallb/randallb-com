/// The actor and ingress identity established by a local host for one runtime.
///
/// The acting actor belongs to the resolved CWR graph. The ingress integration
/// identifies the host boundary that admitted messages; it is not an adapter
/// route and does not need to be declared by the acting actor's coworker.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalHostIdentity {
    pub acting_actor_ref: String,
    pub ingress_integration_ref: String,
}

impl LocalHostIdentity {
    pub fn new(
        acting_actor_ref: impl Into<String>,
        ingress_integration_ref: impl Into<String>,
    ) -> Self {
        Self {
            acting_actor_ref: acting_actor_ref.into(),
            ingress_integration_ref: ingress_integration_ref.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalActor {
    pub actor_ref: String,
    pub coworker_ref: String,
    pub actor_integration_ref: String,
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<crate::v2026_09::graph::CoworkerCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalAction {
    pub activation: String,
    pub actor_ref: String,
    pub actor_integration_ref: String,
    pub action_name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LocalActorIdentity {
    pub actor_ref: String,
    pub coworker_ref: String,
    pub handle: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<crate::v2026_09::graph::CoworkerCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalCatalog {
    pub acting_actor: LocalActorIdentity,
    #[serde(default)]
    pub default_conversation_target_ref: Option<String>,
    pub known_actors: Vec<LocalActorIdentity>,
    #[serde(default)]
    pub retained_actors: Vec<LocalActorIdentity>,
    pub actors: Vec<LocalActor>,
    pub actions: Vec<LocalAction>,
}

impl LocalCatalog {
    pub fn known_actor_by_ref(&self, actor_ref: &str) -> Option<&LocalActorIdentity> {
        self.known_actors
            .iter()
            .find(|actor| actor.actor_ref == actor_ref)
    }

    pub fn known_actor_by_handle(&self, handle: &str) -> Option<&LocalActorIdentity> {
        self.known_actors
            .iter()
            .find(|actor| actor.handle == handle)
    }

    pub fn actor_by_handle(&self, handle: &str) -> Option<&LocalActor> {
        self.actors.iter().find(|actor| actor.handle == handle)
    }

    pub fn actor_by_ref(&self, actor_ref: &str) -> Option<&LocalActor> {
        self.actors
            .iter()
            .find(|actor| actor.actor_ref == actor_ref)
    }

    pub fn action(&self, activation: &str) -> Option<&LocalAction> {
        self.actions
            .iter()
            .find(|action| action.activation == activation)
    }

    pub fn intrinsic_cwr_catalog(&self) -> IntrinsicCwrCatalog {
        IntrinsicCwrCatalog {
            coworkers: self
                .actors
                .iter()
                .filter(|actor| actor.actor_ref != INTRINSIC_CWR_ACTOR_REF)
                .map(|actor| IntrinsicCwrCatalogCoworker {
                    details: Default::default(),
                    handle: actor.handle.clone(),
                    name: actor.name.clone(),
                    actions: self
                        .actions
                        .iter()
                        .filter(|action| action.actor_ref == actor.actor_ref)
                        .map(|action| action.activation.clone())
                        .collect(),
                })
                .collect(),
        }
    }
}

pub fn load_local_catalog(
    cwr_toml: impl AsRef<Path>,
    acting_actor_ref: &str,
) -> CwrResult<LocalCatalog> {
    let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
    local_catalog(
        &connections::resolve_local_graph(&cwr_toml)?,
        acting_actor_ref,
    )
}

/// Returns the compatibility default used by hosts when no actor was selected.
///
/// A host may auto-select only when exactly one graph actor has no actor
/// integrations. The selected ref is still passed explicitly to
/// [`LocalRuntime::open`], so runtime behavior never depends on inference.
pub fn load_default_local_acting_actor_ref(cwr_toml: impl AsRef<Path>) -> CwrResult<String> {
    let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
    default_local_acting_actor_ref(&connections::resolve_local_graph(&cwr_toml)?)
}

/// Returns the compatibility default when exactly one graph actor has no actor
/// integration, without requiring a host to choose an actor during inspection.
pub fn load_optional_default_local_acting_actor_ref(
    cwr_toml: impl AsRef<Path>,
) -> CwrResult<Option<String>> {
    let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
    Ok(optional_default_local_acting_actor_ref(
        &connections::resolve_local_graph(&cwr_toml)?,
    ))
}

/// Inspects the actors exposed by a local runtime, including its intrinsic CWR
/// actor, without opening runtime-owned state.
pub fn inspect_local_runtime_workspace(
    cwr_toml: impl AsRef<Path>,
) -> CwrResult<crate::v2026_09::host::HostWorkspace> {
    let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
    let graph = runtime_graph(connections::resolve_local_graph(&cwr_toml)?, &cwr_toml)?;
    crate::v2026_09::host::inspect_host_workspace(&graph)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalAdmission {
    pub record: ConversationAdmissionRecord,
    pub conversation: RetainedConversation,
}

impl LocalAdmission {
    pub fn conversation_ref(&self) -> &str {
        &self.record.conversation_ref
    }

    pub fn invocation_ref(&self) -> Option<&str> {
        self.record.invocation_ref.as_deref()
    }
}

/// Host projection added to the intrinsic CWR catalog for local text ingress.
///
/// The shared list, help, and discovery commands are always available. A host
/// advertises only the optional actions it can execute and may add its own
/// projected coworkers, such as the attended TUI's `@tui` coworker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalIntrinsicCwrHost {
    pub capabilities: IntrinsicCwrHostCapabilities,
    pub additional_coworkers: Vec<IntrinsicCwrCatalogCoworker>,
}

/// Result of submitting text through the standard local host ingress.
#[derive(Debug, Clone, PartialEq)]
pub enum LocalTextOutcome {
    Conversation(Box<LocalAdmission>),
    Intrinsic(IntrinsicCwrDispatch),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocalTurn {
    pub admission: ConversationAdmissionRecord,
    pub conversation: RetainedConversation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<ActionInvocationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LocalRuntimeEvent {
    InvocationStarted {
        invocation_ref: String,
        generation: u64,
        target_actor_ref: String,
    },
    Activity {
        invocation_ref: String,
        target_actor_ref: String,
        activity: Box<ConversationActivity>,
    },
    Terminal {
        target_actor_ref: String,
        response: Box<ActionResponse>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<Box<ConversationMessage>>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum LocalStopOutcome {
    Accepted {
        response: ActionResponse,
        stopped_invocation_refs: Vec<String>,
        woken_invocation_refs: Vec<String>,
    },
    TooLate {
        response: ActionResponse,
    },
    Unknown,
}

#[derive(Clone)]
pub struct LocalRuntime {
    inner: Arc<Mutex<LocalRuntimeState>>,
    execution: Arc<LocalExecutionCoordinator>,
    control_execution: Arc<LocalExecutionCoordinator>,
    archive_execution: Arc<LocalExecutionCoordinator>,
    archive_mutations: Arc<LocalExecutionCoordinator>,
    archive_order: Arc<LocalArchiveOrder>,
    metrics: crate::metrics::MetricsRecorder,
    diagnostics: crate::diagnostics::DiagnosticsRecorder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRuntimeOptions {
    pub max_concurrent_conversations: NonZeroUsize,
    pub conversation_timeline_rollout: ConversationTimelineRollout,
}

impl LocalRuntimeOptions {
    /// Canonical publications and legacy migration are part of normal startup.
    pub fn configured() -> CwrResult<Self> {
        Ok(Self::default())
    }
}

impl Default for LocalRuntimeOptions {
    fn default() -> Self {
        Self {
            max_concurrent_conversations: NonZeroUsize::new(16)
                .expect("the default local execution limit is nonzero"),
            conversation_timeline_rollout: ConversationTimelineRollout::MigrateLegacy,
        }
    }
}

struct LocalExecutionCoordinator {
    active_conversation_refs: Mutex<BTreeSet<String>>,
    changed: Condvar,
    max_concurrent_conversations: usize,
}

impl LocalExecutionCoordinator {
    fn new(max_concurrent_conversations: NonZeroUsize) -> Self {
        Self {
            active_conversation_refs: Mutex::new(BTreeSet::new()),
            changed: Condvar::new(),
            max_concurrent_conversations: max_concurrent_conversations.get(),
        }
    }

    fn acquire(
        self: &Arc<Self>,
        conversation_ref: &str,
        mut ready: impl FnMut() -> CwrResult<bool>,
    ) -> CwrResult<LocalExecutionPermit> {
        let mut active = self
            .active_conversation_refs
            .lock()
            .map_err(|_| invalid_error("local execution coordinator is unavailable"))?;
        while active.contains(conversation_ref)
            || active.len() >= self.max_concurrent_conversations
            || !ready()?
        {
            (active, _) = self
                .changed
                .wait_timeout(active, Duration::from_millis(25))
                .map_err(|_| invalid_error("local execution coordinator is unavailable"))?;
        }
        active.insert(conversation_ref.to_owned());
        Ok(LocalExecutionPermit {
            coordinator: Arc::clone(self),
            conversation_ref: conversation_ref.to_owned(),
        })
    }

    fn try_acquire(
        self: &Arc<Self>,
        conversation_ref: &str,
        ready: impl FnOnce() -> CwrResult<bool>,
    ) -> CwrResult<Option<LocalExecutionPermit>> {
        let mut active = self
            .active_conversation_refs
            .lock()
            .map_err(|_| invalid_error("local execution coordinator is unavailable"))?;
        if active.contains(conversation_ref)
            || active.len() >= self.max_concurrent_conversations
            || !ready()?
        {
            return Ok(None);
        }
        active.insert(conversation_ref.to_owned());
        Ok(Some(LocalExecutionPermit {
            coordinator: Arc::clone(self),
            conversation_ref: conversation_ref.to_owned(),
        }))
    }
}

struct LocalExecutionPermit {
    coordinator: Arc<LocalExecutionCoordinator>,
    conversation_ref: String,
}

impl Drop for LocalExecutionPermit {
    fn drop(&mut self) {
        if let Ok(mut active) = self.coordinator.active_conversation_refs.lock() {
            active.remove(&self.conversation_ref);
            self.coordinator.changed.notify_all();
        }
    }
}

struct LocalRuntimeState {
    schedule_worker_state: ScheduleWorkerState,
    next_schedule_archive_scan_unix_ms: u64,
    schedule_archive_history_scan_count: u64,
    pending_schedule_archives: BTreeMap<String, PendingScheduleArchive>,
    archive_reservations: BTreeMap<String, Arc<LocalArchiveReservation>>,
    archiving_conversations: BTreeMap<String, usize>,
    stopping_roots: BTreeMap<String, usize>,
    settled_stop_invocations: Vec<String>,
    upstream: UpstreamRegistry,
    upstream_providers: BTreeMap<String, LocalUpstreamProvider>,
    gateway_handlers: BTreeMap<(String, String), NativeGatewayHandler>,
    upstream_work: BTreeMap<String, PendingUpstreamWork>,
    protected_upstream_results: BTreeMap<String, BTreeMap<String, UpstreamRoute>>,
    upstream_result_validations: BTreeMap<String, (String, Option<NativeResultValidation>)>,
    protected_publication_authority: BTreeMap<String,Vec<cwpdb_store::conversation::AuthorityGuard>>,
    host_local_invocation_owners: BTreeMap<String, String>,
    _state_lease: File,
    cwr_toml: PathBuf,
    graph: ResolvedCwrGraph,
    graph_options: crate::v2026_09::graph::GraphLoadOptions,
    allow_reconfiguration: bool,
    resident_configuration: Option<String>,
    host_identity: LocalHostIdentity,
    catalog: LocalCatalog,
    admission: ConversationAdmissionStore,
    pending_invocation_refs: Vec<String>,
    active: BTreeMap<String, ProcessAdapterStopControl>,
    shutting_down: bool,
    draining: Option<BTreeSet<String>>,
    preserve_pending: bool,
}

#[derive(Clone)]
struct NativeResultValidation {allowed:bool,authority_guards:Vec<cwpdb_store::conversation::AuthorityGuard>}
