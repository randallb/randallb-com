impl LocalRuntime {
    pub fn open(cwr_toml: impl AsRef<Path>, host_identity: LocalHostIdentity) -> CwrResult<Self> {
        Self::open_with_options(cwr_toml, host_identity, LocalRuntimeOptions::configured()?)
    }

    pub fn open_with_options(
        cwr_toml: impl AsRef<Path>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
    ) -> CwrResult<Self> {
        let paths = LocalWorkspacePaths::resolve(cwr_toml)?;
        Self::open_configured(
            paths.cwr_toml.clone(),
            paths.database_path(),
            paths.runtime_root_ref.clone(),
            host_identity,
            options,
            Some(paths.binding()),
        )
    }

    pub fn open_with_state_path(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        host_identity: LocalHostIdentity,
    ) -> CwrResult<Self> {
        Self::open_with_state_path_and_options(
            cwr_toml,
            state_path,
            host_identity,
            LocalRuntimeOptions::configured()?,
        )
    }

    pub fn open_with_state_path_and_options(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
    ) -> CwrResult<Self> {
        let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
        let runtime_root_ref = path_derived_runtime_root_ref(&cwr_toml);
        Self::open_with_state_path_and_runtime_ref_and_options(
            cwr_toml,
            state_path,
            runtime_root_ref,
            host_identity,
            options,
        )
    }

    pub fn open_with_state_path_and_runtime_ref(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        host_identity: LocalHostIdentity,
    ) -> CwrResult<Self> {
        Self::open_with_state_path_and_runtime_ref_and_options(
            cwr_toml,
            state_path,
            runtime_root_ref,
            host_identity,
            LocalRuntimeOptions::configured()?,
        )
    }

    pub fn open_with_state_path_and_runtime_ref_and_options(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
    ) -> CwrResult<Self> {
        Self::open_configured(
            cwr_toml,
            state_path,
            runtime_root_ref,
            host_identity,
            options,
            None,
        )
    }

    pub fn open_bound_with_options(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
        binding: LocalWorkspaceBinding,
    ) -> CwrResult<Self> {
        Self::open_configured(
            cwr_toml,
            state_path,
            runtime_root_ref,
            host_identity,
            options,
            Some(binding),
        )
    }

    fn open_configured(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
        binding: Option<LocalWorkspaceBinding>,
    ) -> CwrResult<Self> {
        let path = canonical_cwr_path(cwr_toml.as_ref())?;
        let graph_options = connections::graph_options(&path)?;
        Self::open_graph_configured(
            path,
            state_path,
            runtime_root_ref,
            host_identity,
            options,
            binding,
            graph_options,
        )
    }

    pub fn open_configured_runtime(
        selected: &LocalConfiguredRuntime,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
    ) -> CwrResult<Self> {
        use crate::v2026_09::startup_profile::{time, StartupPhase};

        let _open = StartupPhase::new("runtime.configured.total");
        let (graph, graph_options) = time("runtime.configured.prepare", || selected.prepare())?;
        if selected.acting_coworker.is_some() {
            selected.acting_actor_ref(&graph)?;
        }
        Self::open_prepared_runtime(selected, host_identity, options, graph, graph_options)
    }

    fn open_prepared_runtime(
        selected: &LocalConfiguredRuntime,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
        graph: ResolvedCwrGraph,
        graph_options: crate::v2026_09::graph::GraphLoadOptions,
    ) -> CwrResult<Self> {
        use crate::v2026_09::startup_profile::{time, StartupPhase};

        let _open = StartupPhase::new("runtime.prepared.total");
        // Validate before creating private state, including reserved host tool names.
        validate_local_host_identity(&host_identity)?;
        reject_retired_shared_folder(&selected.cwr_toml)?;
        let graph = time("runtime.prepared.graph", || {
            runtime_graph(graph, &selected.cwr_toml)
        })?;
        let catalog = time("runtime.prepared.catalog", || {
            local_catalog(&graph, &host_identity.acting_actor_ref)
        })?;
        let parent = selected
            .state_directory
            .parent()
            .ok_or_else(|| invalid_error("runtime state has no parent"))?;
        let state_home = platform_state_home()?;
        let root = state_home.join(application_directory_name());
        let loops = root.join("loops");
        if parent == loops {
            // Only platform-managed ancestors belong to CWR; overrides may use shared parents.
            fs::create_dir_all(&state_home)?;
            create_private_directory(&root)?;
            create_private_directory(&loops)?;
        } else {
            fs::create_dir_all(parent)?;
        }
        create_private_directory(&selected.state_directory)?;
        time("runtime.prepared.identity_before_database", || {
            selected.persist_identity_before_database()
        })?;
        let runtime = time("runtime.prepared.open_graph", || {
            Self::open_prepared_graph(
                selected.cwr_toml.clone(),
                selected.database_path(),
                selected.runtime_ref.clone(),
                host_identity,
                options,
                None,
                graph_options,
                graph,
                catalog,
            )
        })?;
        time("runtime.prepared.identity_after_database", || {
            selected.persist_identity_after_database()
        })?;
        runtime.lock()?.allow_reconfiguration = selected.allow_reconfiguration;
        Ok(runtime)
    }

    fn open_graph_configured(
        cwr_toml: impl AsRef<Path>,
        state_path: impl Into<PathBuf>,
        runtime_root_ref: impl Into<String>,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
        binding: Option<LocalWorkspaceBinding>,
        mut graph_options: crate::v2026_09::graph::GraphLoadOptions,
    ) -> CwrResult<Self> {
        let cwr_toml = canonical_cwr_path(cwr_toml.as_ref())?;
        reject_retired_shared_folder(&cwr_toml)?;
        validate_local_host_identity(&host_identity)?;
        let runtime_root_ref = runtime_root_ref.into();
        graph_options.runtime_ref = Some(runtime_root_ref.clone());
        let graph = runtime_graph(
            crate::v2026_09::graph::resolve_graph_with_options(
                &cwr_toml,
                &crate::v2026_09::graph::EmptyResourceResolver,
                &graph_options,
            )?,
            &cwr_toml,
        )?;
        let catalog = local_catalog(&graph, &host_identity.acting_actor_ref)?;
        Self::open_prepared_graph(
            cwr_toml,
            state_path.into(),
            runtime_root_ref,
            host_identity,
            options,
            binding,
            graph_options,
            graph,
            catalog,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Carry the validated graph and catalog into state acquisition without resolving inputs again."
    )]
    fn open_prepared_graph(
        cwr_toml: PathBuf,
        state_path: PathBuf,
        runtime_root_ref: String,
        host_identity: LocalHostIdentity,
        options: LocalRuntimeOptions,
        binding: Option<LocalWorkspaceBinding>,
        graph_options: crate::v2026_09::graph::GraphLoadOptions,
        graph: ResolvedCwrGraph,
        catalog: LocalCatalog,
    ) -> CwrResult<Self> {
        use crate::v2026_09::startup_profile::{time, StartupPhase};

        let _open = StartupPhase::new("runtime.graph.total");
        let state_path = canonical_local_state_path(state_path)?;
        let state_lease = time("runtime.graph.state_lease", || {
            acquire_local_state_lease(&state_path)
        })?;
        let mut context_policy_name = state_path.as_os_str().to_os_string();
        context_policy_name.push(".contexts.json");
        let context_policy_path = PathBuf::from(context_policy_name);
        let conversation_contexts = load_context_policy(&context_policy_path)?;
        let metrics = crate::metrics::MetricsRecorder::new(&state_path);
        let diagnostics = crate::diagnostics::DiagnosticsRecorder::new(&state_path);
        let mut admission = time("runtime.graph.conversation_store", || match binding {
            Some(binding) => ConversationAdmissionStore::open_bound_with_timeline_rollout_deferred(
                state_path,
                runtime_root_ref,
                &binding.workspace_id,
                &binding.workspace_directory,
                options.conversation_timeline_rollout,
            ),
            None => ConversationAdmissionStore::open_with_timeline_rollout_deferred(
                state_path,
                runtime_root_ref,
                options.conversation_timeline_rollout,
            ),
        })?;
        if let Some((configured,mut policy))=admission.stored_context_policy()? {
            if let Some(policy)=&mut policy {policy.suspended=true;}
            validate_context_policy(&policy)?;
            admission.context_policy=policy.clone();admission.context_policy_configured=configured;
            if configured {admission.persist_context_policy(&policy)?;}
        } else if context_policy_path.exists() {
            admission.persist_context_policy(&conversation_contexts)?;
        }
        let mut catalog = catalog;
        catalog.retained_actors = time("runtime.graph.identity_retention", || {
            admission.retain_local_identities(&catalog.known_actors)
        })?;
        admission.metrics = Some(metrics.clone());
        for record in admission
            .invocation_records()
            .filter(|record| !record.state.is_terminal() || record.attempt_active)
        {
            metrics.observe_invocation(admission.runtime_root_ref(), record, None, true);
        }
        admission.response_graph = Some(graph.clone());
        time("runtime.graph.clear_provider_questions", || {
            admission.clear_provider_questions(true)
        })?;
        let pending_invocation_refs = time("runtime.graph.recover_invocations", || {
            admission.recover_interrupted_invocations()
        })?;
        time("runtime.graph.migrate_legacy_timelines", || {
            admission.migrate_eligible_legacy_timelines()
        })?;
        let runtime = Self {
            metrics,
            diagnostics,
            control_execution: Arc::new(LocalExecutionCoordinator::new(NonZeroUsize::MIN)),
            archive_execution: Arc::new(LocalExecutionCoordinator::new(
                options.max_concurrent_conversations,
            )),
            archive_order: Arc::new(LocalArchiveOrder::default()),
            archive_mutations: Arc::new(LocalExecutionCoordinator::new(NonZeroUsize::MAX)),
            execution: Arc::new(LocalExecutionCoordinator::new(
                options.max_concurrent_conversations,
            )),
            inner: Arc::new(Mutex::new(LocalRuntimeState {
                schedule_worker_state: ScheduleWorkerState::Uninitialized,
                next_schedule_archive_scan_unix_ms: 0,
                schedule_archive_history_scan_count: 0,
                pending_schedule_archives: BTreeMap::new(),
                archive_reservations: BTreeMap::new(),
                archiving_conversations: BTreeMap::new(),
                stopping_roots: BTreeMap::new(),
                settled_stop_invocations: Vec::new(),
                upstream: UpstreamRegistry::default(),
                upstream_providers: BTreeMap::new(),
                gateway_handlers: BTreeMap::new(),
                upstream_work: BTreeMap::new(),
                protected_upstream_results: BTreeMap::new(),
                upstream_result_validations: BTreeMap::new(),
                protected_publication_authority:BTreeMap::new(),
                host_local_invocation_owners: BTreeMap::new(),
                _state_lease: state_lease,
                cwr_toml,
                graph,
                graph_options,
                allow_reconfiguration: false,
                resident_configuration: None,
                host_identity,
                catalog,
                admission,
                pending_invocation_refs,
                active: BTreeMap::new(),
                shutting_down: false,
                draining: None,
                preserve_pending: false,
            })),
        };
        time("runtime.graph.fail_unavailable_upstream", || {
            runtime.fail_unavailable_upstream()
        })?;
        Ok(runtime)
    }

    pub fn catalog(&self) -> CwrResult<LocalCatalog> {
        Ok(self.lock()?.catalog.clone())
    }

    pub fn catalog_as(&self, acting_actor_ref: &str) -> CwrResult<LocalCatalog> {
        let state = self.lock()?;
        let mut catalog = local_catalog(&state.graph, acting_actor_ref)?;
        catalog.retained_actors = state.catalog.retained_actors.clone();
        Ok(catalog)
    }

    pub fn graph(&self) -> CwrResult<ResolvedCwrGraph> {
        Ok(self.lock()?.graph.clone())
    }

    pub fn reload_graph(&self) -> CwrResult<(LocalCatalog, LocalCatalog)> {
        let acting_actor_ref = self.lock()?.host_identity.acting_actor_ref.clone();
        self.reload_graph_as(&acting_actor_ref)
    }

    pub fn reload_graph_as(
        &self,
        acting_actor_ref: &str,
    ) -> CwrResult<(LocalCatalog, LocalCatalog)> {
        self.reload_graph_as_validated(acting_actor_ref, |_, _| Ok(()))
    }

    pub fn reload_graph_as_validated(
        &self,
        acting_actor_ref: &str,
        validate: impl FnOnce(&ResolvedCwrGraph, &LocalCatalog) -> CwrResult<()>,
    ) -> CwrResult<(LocalCatalog, LocalCatalog)> {
        self.reload_graph_for_actor(acting_actor_ref, validate, true)
    }

    pub fn reload_graph_for_connected_actor_validated(
        &self,
        acting_actor_ref: &str,
        validate: impl FnOnce(&ResolvedCwrGraph, &LocalCatalog) -> CwrResult<()>,
    ) -> CwrResult<(LocalCatalog, LocalCatalog)> {
        self.reload_graph_for_actor(acting_actor_ref, validate, false)
    }

    fn reload_graph_for_actor(
        &self,
        acting_actor_ref: &str,
        validate: impl FnOnce(&ResolvedCwrGraph, &LocalCatalog) -> CwrResult<()>,
        replace_host_catalog: bool,
    ) -> CwrResult<(LocalCatalog, LocalCatalog)> {
        let (cwr_toml, before, mut graph_options, inputs, volumes) = {
            let state = self.lock()?;
            if state.resident_configuration.is_some() {
                return invalid(
                    "configured resident retains its executable configuration until exit",
                );
            }
            (
                state.cwr_toml.clone(),
                local_catalog(&state.graph, acting_actor_ref)?,
                state.graph_options.clone(),
                requirements::retained_input_bindings(
                    &state.graph,
                    &state.cwr_toml,
                    &state.graph_options.inputs,
                )?,
                state
                    .graph
                    .resources
                    .iter()
                    .find(|resource| resource.source == state.cwr_toml)
                    .and_then(|resource| resource.document.get("volumes"))
                    .cloned(),
            )
        };
        let imported_volumes = imported_volumes::load(&cwr_toml)?;
        if graph_options.imported_volumes != imported_volumes {
            return invalid(
                "imported volume grants changed; reopen the workspace runtime to apply them",
            );
        }
        graph_options.imported_volumes = imported_volumes;
        let graph = crate::v2026_09::graph::resolve_bound_graph(&cwr_toml, &graph_options, inputs)?;
        let requested_volumes = graph
            .resources
            .iter()
            .find(|resource| resource.source == cwr_toml)
            .and_then(|resource| resource.document.get("volumes"));
        if requested_volumes != volumes.as_ref() {
            return invalid("volume requirements changed; reopen the configured runtime through its workspace to validate the grants");
        }
        let graph = runtime_graph(graph, &cwr_toml)?;
        let mut after = local_catalog(&graph, acting_actor_ref)?;
        validate(&graph, &after)?;
        let mut state = self.lock()?;
        state.upstream.validate_graph(
            &after
                .known_actors
                .iter()
                .map(|actor| (actor.actor_ref.clone(), actor.handle.clone()))
                .collect::<Vec<_>>(),
        )?;
        after.retained_actors = state
            .admission
            .retain_local_identities(&after.known_actors)?;
        let mut host_catalog = if replace_host_catalog {
            after.clone()
        } else {
            local_catalog(&graph, &state.host_identity.acting_actor_ref)
                .unwrap_or_else(|_| after.clone())
        };
        host_catalog.retained_actors = after.retained_actors.clone();
        let mut removed_work = state
            .admission
            .invocation_records()
            .filter(|record| {
                !record.state.is_terminal()
                    && (state
                        .graph
                        .actors
                        .iter()
                        .any(|actor| actor.actor_ref == record.target_actor_ref)
                        || gateway_route_for_record(&state.graph, record).is_some())
                    && !(graph
                        .actors
                        .iter()
                        .any(|actor| actor.actor_ref == record.target_actor_ref)
                        || gateway_route_for_record(&graph, record).is_some())
            })
            .map(|record| record.invocation_ref.clone())
            .collect::<BTreeSet<_>>();
        state.admission.response_graph = Some(graph.clone());
        // A withdrawn or changed gateway executor cannot become valid again if
        // a later graph restores the old definition. New registration gets a new
        // retained provider route, while old invocations remain invalid.
        state.gateway_handlers.retain(|(_, resource), handler| {
            graph
                .gateways
                .iter()
                .find(|gateway| &gateway.resource_ref == resource)
                .is_some_and(|gateway| {
                    gateway
                        .approval_fingerprint()
                        .is_ok_and(|fingerprint| fingerprint == handler.fingerprint)
                })
        });
        state.graph = graph;
        state.graph_options = graph_options;
        state.host_identity.acting_actor_ref = host_catalog.acting_actor.actor_ref.clone();
        state.catalog = host_catalog;
        // The retained provider route identifies old native work even when its
        // target still exists. Keep the old provider control route until it has
        // acknowledged cleanup; only the handler lookup was withdrawn above.
        removed_work.extend(
            state
                .admission
                .invocation_records()
                .filter(|record| {
                    !record.state.is_terminal()
                        && retained_upstream_route(&state, &record.invocation_ref)
                            .is_some_and(|route| route.gateway_resource_ref.is_some())
                        && require_current_request_authority(&state, record).is_err()
                })
                .map(|record| record.invocation_ref.clone()),
        );
        // A completed protected child may already have informed its consumer.
        // Stop that still-live consumer now so restoring caller permissions can
        // never revive the old derived result. Unprotected history is untouched.
        let revoked_consumers = state
            .protected_upstream_results
            .iter()
            .filter(|(_, guards)| {
                guards.keys().any(|reference| {
                    state
                        .admission
                        .invocation_record(reference)
                        .is_some_and(|record| {
                            record.state.is_terminal()
                                && require_native_gateway_result_current(&state, record).is_err()
                        })
                })
            })
            .map(|(root, _)| root.clone())
            .collect::<BTreeSet<_>>();
        removed_work.retain(|reference| {
            !revoked_consumers
                .iter()
                .any(|root| reference != root && descends_from(&state, reference, root))
        });
        removed_work.extend(revoked_consumers);
        let stops = removed_work
            .into_iter()
            .map(|invocation_ref| {
                self.prepare_invocation_stop_locked(
                    &mut state,
                    &invocation_ref,
                    InvocationStopRequest {
                        requested_at_unix_ms: current_unix_ms(),
                        requested_by_actor_ref: None,
                        request_id: None,
                        reason: Some(
                            "Coworker or gateway authority changed in this workspace".into(),
                        ),
                    },
                )
            })
            .collect::<CwrResult<Vec<_>>>()?;
        drop(state);
        for stop in stops {
            self.deliver_invocation_stop(stop, false)?;
        }
        Ok((before, after))
    }
}

include!("runtime/conversations.rs");
