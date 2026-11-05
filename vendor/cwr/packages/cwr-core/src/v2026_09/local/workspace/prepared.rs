/// One checked executable selection. The resident opens exactly this graph,
/// including activated prompt and role contents, without resolving it again.
#[derive(Clone)]
pub struct PreparedConfiguredRuntime {
    selected: LocalConfiguredRuntime,
    graph: ResolvedCwrGraph,
    options: crate::v2026_09::graph::GraphLoadOptions,
    pub acting_actor_ref: String,
    pub fingerprint: String,
}

impl LocalConfiguredRuntime {
    pub fn prepare_resident(&self) -> CwrResult<PreparedConfiguredRuntime> {
        let (graph, options) = self.prepare()?;
        let acting_actor_ref = self.acting_actor_ref(&graph)?;
        local_catalog(
            &runtime_graph(graph.clone(), &self.cwr_toml)?,
            &acting_actor_ref,
        )?;
        // Keep resident fingerprints compatible with the historical graph wire field.
        let mut fingerprint_graph = serde_json::to_value(&graph)?;
        let fields = fingerprint_graph
            .as_object_mut()
            .expect("serialized graph object");
        let runtime_ref = fields
            .remove("runtime_ref")
            .expect("graph runtime reference");
        fields.insert("configured_loop_ref".into(), runtime_ref);
        let executable = json!({
            "graph": fingerprint_graph,
            "acting_actor_ref": acting_actor_ref,
            "working_directory": self.working_directory,
            "source_roots": self.source_roots,
            "primary_volume": self.volume,
            "volume_access": self.volume_access,
            "volume_mappings": self.volume_mappings,
            "volumes": self.workspace_volumes,
            "allow_reconfiguration": self.allow_reconfiguration,
        });
        let fingerprint = hex::encode(Sha256::digest(serde_json::to_vec(&executable)?));
        Ok(PreparedConfiguredRuntime {
            selected: self.clone(),
            graph,
            options,
            acting_actor_ref,
            fingerprint,
        })
    }
}

impl PreparedConfiguredRuntime {
    pub fn warnings(&self) -> &[crate::v2026_09::graph::GraphLoadWarning] {
        &self.graph.warnings
    }

    pub fn selected(&self) -> &LocalConfiguredRuntime {
        &self.selected
    }

    pub fn catalog(&self) -> CwrResult<LocalCatalog> {
        local_catalog(
            &runtime_graph(self.graph.clone(), &self.selected.cwr_toml)?,
            &self.acting_actor_ref,
        )
    }

    pub fn open(&self, ingress_ref: &str, options: LocalRuntimeOptions) -> CwrResult<LocalRuntime> {
        if self.acting_actor_ref != self.selected.acting_actor_ref(&self.graph)? {
            return invalid("configured runtime requires its configured acting coworker");
        }
        let runtime = LocalRuntime::open_prepared_runtime(
            &self.selected,
            LocalHostIdentity::new(&self.acting_actor_ref, ingress_ref),
            options,
            self.graph.clone(),
            self.options.clone(),
        )?;
        // Configured residents retain their admitted configuration until exit.
        {
            let mut state = runtime.lock()?;
            state.allow_reconfiguration = false;
            state.resident_configuration = Some(self.fingerprint.clone());
        }
        Ok(runtime)
    }

    pub fn create_resident_directories(&self) -> CwrResult<()> {
        let state_home = platform_state_home()?;
        fs::create_dir_all(&state_home)?;
        let root = state_home.join(application_directory_name());
        create_private_directory(&root)?;
        create_private_directory(&root.join("loops"))?;
        create_private_directory(&self.selected.state_directory)?;
        let runtime_home = platform_runtime_home()?;
        fs::create_dir_all(&runtime_home)?;
        create_private_directory(&runtime_home.join("cwr"))?;
        create_private_directory(&runtime_home.join("cwr/loops"))?;
        create_private_directory(&self.selected.runtime_directory()?)
    }
}
