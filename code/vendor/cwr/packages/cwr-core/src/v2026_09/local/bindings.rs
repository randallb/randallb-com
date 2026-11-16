/// Public configuration metadata. Never contains raw adapter parameters or credentials.
#[derive(Debug, Clone)]
pub struct LocalCoworkerConfiguration {
    pub category: Option<crate::v2026_09::graph::CoworkerCategory>,
    pub default_binding_ref: Option<String>,
    pub bindings: Vec<LocalCoworkerBinding>,
}

#[derive(Debug, Clone)]
pub struct LocalCoworkerBinding {
    pub binding_ref: String,
    pub adapter_ref: String,
    pub adapter_name: String,
    pub model: Option<LocalModelConfiguration>,
}

#[derive(Debug, Clone)]
pub struct LocalModelConfiguration {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

fn public_binding(binding: &ResolvedCwrActorIntegration) -> LocalCoworkerBinding {
    use crate::adapter::*;
    let has_model = matches!(
        binding.adapter_ref.as_str(),
        CODEX_ADAPTER_REF | CLAUDE_ADAPTER_REF | OPEN_RESPONSES_ADAPTER_REF | DECISIONS_ADAPTER_REF
    );
    let configured = |key: &str| {
        binding
            .adapter_params
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_owned)
    };
    LocalCoworkerBinding {
        binding_ref: binding.actor_integration_ref.clone(),
        adapter_ref: binding.adapter_ref.clone(),
        adapter_name: binding.adapter_name.clone(),
        model: has_model.then(|| LocalModelConfiguration {
            provider: None,
            model: configured("model"),
            reasoning_effort: if binding.adapter_ref == CODEX_ADAPTER_REF {
                configured("reasoning_effort")
            } else {
                None
            },
        }),
    }
}

impl LocalRuntime {
    /// Current metadata for one coworker visible to this session, under its read budget.
    pub fn coworker_configuration_as_until(
        &self,
        actor: &str,
        owner: Option<&str>,
        target: &str,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
    ) -> CwrResult<Option<LocalCoworkerConfiguration>> {
        Ok(self
            .coworker_configurations_as_until(actor, owner, deadline, is_cancelled)?
            .remove(target))
    }

    /// One session-scoped graph snapshot shared by all coworker metadata readers.
    pub fn coworker_configurations_as_until(
        &self,
        actor: &str,
        owner: Option<&str>,
        deadline: Instant,
        is_cancelled: impl Fn() -> bool,
    ) -> CwrResult<std::collections::BTreeMap<String, LocalCoworkerConfiguration>> {
        let state = self.read_lock_until(deadline, &is_cancelled)?;
        let graph = upstream_graph(&state, owner)?;
        local_catalog(&graph, actor)?;
        let mut bindings = std::collections::BTreeMap::<String, Vec<LocalCoworkerBinding>>::new();
        for binding in &graph.actor_integrations {
            bindings
                .entry(binding.actor_ref.clone())
                .or_default()
                .push(public_binding(binding));
        }
        Ok(graph
            .actors
            .iter()
            .map(|coworker| {
                let bindings = bindings.remove(&coworker.actor_ref).unwrap_or_default();
                let category =
                    if coworker.default_actor_integration_ref.is_none() && !bindings.is_empty() {
                        None
                    } else {
                        coworker.category
                    };
                (
                    coworker.actor_ref.clone(),
                    LocalCoworkerConfiguration {
                        category,
                        default_binding_ref: coworker.default_actor_integration_ref.clone(),
                        bindings,
                    },
                )
            })
            .collect())
    }
}

#[cfg(test)]
mod binding_metadata_tests {
    use super::*;
    #[test]
    fn binding_metadata_preserves_configuration_and_omits_private_parameters() {
        let graph = crate::v2026_09::graph::resolve_graph_with_resolver(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml"),
            &crate::v2026_09::graph::EmptyResourceResolver,
        )
        .unwrap();
        let mut binding = graph.actor_integrations[0].clone();
        for (adapter_ref, name) in [
            (crate::adapter::RNG_ADAPTER_REF, "RNG"),
            ("cwp://example.test/file-adapter", "Declared adapter name"),
            ("cwp://org.coworkerprotocol/2026-09/adapters/slack", "Slack"),
        ] {
            binding.adapter_ref = adapter_ref.into();
            binding.adapter_name = name.into();
            assert_eq!(public_binding(&binding).adapter_name, name);
        }

        binding.adapter_ref = crate::adapter::CODEX_ADAPTER_REF.into();
        binding.adapter_params = serde_json::json!({"model":"example-model", "reasoning_effort":"future-effort", "api_key":"secret", "entrypoint":"private/path"});
        let value = public_binding(&binding).model.unwrap();
        assert_eq!(value.model.as_deref(), Some("example-model"));
        assert_eq!(value.reasoning_effort.as_deref(), Some("future-effort"));
        assert!(value.provider.is_none());
        binding.adapter_params = serde_json::json!({});
        let value = public_binding(&binding).model.unwrap();
        assert!(value.model.is_none());
        assert!(value.reasoning_effort.is_none());
        binding.adapter_ref = crate::adapter::SH_ADAPTER_REF.into();
        assert!(public_binding(&binding).model.is_none());
        binding.adapter_ref = crate::adapter::OPEN_RESPONSES_ADAPTER_REF.into();
        binding.adapter_params =
            serde_json::json!({"model":"local-model", "reasoning_effort":"unsupported"});
        let value = public_binding(&binding).model.unwrap();
        assert_eq!(value.model.as_deref(), Some("local-model"));
        assert!(value.provider.is_none());
        assert!(value.reasoning_effort.is_none());
    }
    #[test]
    fn configuration_preserves_no_default_and_obeys_deadline() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/contracts/agent-recovery-turn/workspace/CWR.toml");
        let graph = crate::v2026_09::graph::resolve_graph_with_resolver(
            &path,
            &crate::v2026_09::graph::EmptyResourceResolver,
        )
        .unwrap();
        let actor = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "person")
            .unwrap()
            .actor_ref
            .clone();
        let target = graph
            .actors
            .iter()
            .find(|actor| actor.coworker_slug == "helper")
            .unwrap()
            .actor_ref
            .clone();
        let directory = tempfile::tempdir().unwrap();
        let runtime = LocalRuntime::open_with_state_path_and_runtime_ref(
            path,
            directory.path().join("state.sqlite3"),
            "cwp://example.test/runtime",
            LocalHostIdentity::new(&actor, "cwp://example.test/ingress"),
        )
        .unwrap();
        {
            let mut state = runtime.lock().unwrap();
            state
                .graph
                .actors
                .iter_mut()
                .find(|value| value.actor_ref == target)
                .unwrap()
                .default_actor_integration_ref = None;
        }
        let read = || {
            runtime.coworker_configuration_as_until(
                &actor,
                None,
                &target,
                Instant::now() + Duration::from_secs(5),
                || false,
            )
        };
        let value = read().unwrap().unwrap();
        assert!(value.default_binding_ref.is_none());
        assert!(value.category.is_none());
        assert!(!value.bindings.is_empty());
        assert!(runtime
            .coworker_configuration_as_until(
                &actor,
                None,
                "missing",
                Instant::now() + Duration::from_secs(5),
                || false
            )
            .unwrap()
            .is_none());
        assert!(runtime
            .coworker_configuration_as_until(&actor, None, &target, Instant::now(), || false)
            .is_err());
        assert!(runtime
            .coworker_configuration_as_until(
                &actor,
                None,
                &target,
                Instant::now() + Duration::from_secs(5),
                || true
            )
            .is_err());
    }
}
