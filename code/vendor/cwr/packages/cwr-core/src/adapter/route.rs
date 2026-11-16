/// A fully selected provider-neutral route from an actor integration to a
/// process adapter. The resource graph supplies authored parameters and the
/// execution and coworker artifact directories; the host owns executable resolution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedProcessAdapterRoute {
    adapter_ref: String,
    working_path: PathBuf,
    coworker_directory: PathBuf,
    adapter_params: Value,
    integration_params: Value,
    actor_integration_params: Value,
    authority: Vec<String>,
}

impl ResolvedProcessAdapterRoute {
    pub(crate) fn adapter_ref(&self) -> &str {
        &self.adapter_ref
    }

    pub fn working_path(&self) -> &Path {
        &self.working_path
    }

    pub(crate) fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(
            serde_json::to_vec(&(
                &self.adapter_ref,
                self.working_path.as_os_str().as_encoded_bytes(),
                self.coworker_directory.as_os_str().as_encoded_bytes(),
                &self.adapter_params,
                &self.integration_params,
                &self.actor_integration_params,
                &self.authority,
            ))
            .expect("route parameters and path bytes are JSON serializable"),
        ))
    }

    pub(crate) fn provider_fingerprint(&self) -> String {
        self.provider_fingerprint_with_selective_history(false)
    }

    pub(crate) fn legacy_selective_provider_fingerprint(&self) -> String {
        self.provider_fingerprint_with_selective_history(true)
    }

    fn provider_fingerprint_with_selective_history(
        &self,
        include_selective_history: bool,
    ) -> String {
        use sha2::{Digest, Sha256};
        let mut adapter_params = self.adapter_params.clone();
        if let Some(object) = adapter_params.as_object_mut() {
            // Existing provider histories keep their original registered tools.
            // New preferences apply only when starting a fresh provider history.
            object.remove("cwr_native_tools");
            if self.adapter_ref == CODEX_ADAPTER_REF && !include_selective_history {
                // This changes shared-history delivery, not the Codex session's
                // model, instructions, permissions, or registered tools.
                object.remove("selective_history");
                if let Some(contexts) = object
                    .get_mut("role_contexts")
                    .and_then(Value::as_array_mut)
                {
                    Self::remove_role_context_selective_history(contexts);
                }
            }
        }
        let mut params = self.actor_integration_params.clone();
        if let Some(object) = params.as_object_mut() {
            object.remove("cwr_action_actor_refs");
        }
        hex::encode(Sha256::digest(
            serde_json::to_vec(&(
                &self.adapter_ref,
                self.working_path.as_os_str().as_encoded_bytes(),
                self.coworker_directory.as_os_str().as_encoded_bytes(),
                adapter_params,
                &self.integration_params,
                params,
            ))
            .expect("route is serializable"),
        ))
    }

    fn remove_role_context_selective_history(contexts: &mut [Value]) {
        for context in contexts {
            if let Some(params) = context.get_mut("params").and_then(Value::as_object_mut) {
                params.remove("selective_history");
            }
        }
    }

    pub fn new(
        adapter_ref: impl Into<String>,
        working_path: impl Into<PathBuf>,
        coworker_directory: impl Into<PathBuf>,
        adapter_params: Value,
        integration_params: Value,
        actor_integration_params: Value,
    ) -> Self {
        Self {
            adapter_ref: adapter_ref.into(),
            working_path: working_path.into(),
            coworker_directory: coworker_directory.into(),
            adapter_params,
            integration_params,
            actor_integration_params,
            authority: Vec::new(),
        }
    }

    pub fn with_authority(mut self, authority: Vec<String>) -> Self {
        self.authority = authority;
        self
    }

    pub fn project_invocation(
        &self,
        invocation_ref: impl Into<String>,
        action_name: impl Into<String>,
        input: Value,
        output_schema: Value,
        thread: Value,
    ) -> AdapterInvocation {
        let mut invocation =
            AdapterInvocation::new(invocation_ref, action_name, input, output_schema);
        invocation.thread = thread;
        invocation.adapter_params = self.adapter_params.clone();
        invocation.integration_params = self.integration_params.clone();
        invocation.actor_integration_params = self.actor_integration_params.clone();
        invocation.authority = self.authority.clone();
        invocation.route_fingerprint = Some(self.fingerprint());
        invocation.provider_fingerprint = Some(self.provider_fingerprint());
        invocation.retained_route = Some(self.clone());
        invocation
    }

    /// Whether this host has a process implementation registered for the
    /// selected adapter.
    pub fn is_supported_by_local_host(&self) -> bool {
        self.adapter_ref == PERSON_ADAPTER_REF
            || has_local_process_implementation(&self.adapter_ref)
    }

    pub fn codex_model_configuration(&self) -> Option<(&str, &str)> {
        (self.adapter_ref == CODEX_ADAPTER_REF).then_some(())?;
        let parameters = self.adapter_params.as_object()?;
        Some((
            parameters.get("model")?.as_str()?,
            parameters.get("reasoning_effort")?.as_str()?,
        ))
    }

    pub fn run<F, G>(
        &self,
        invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        on_progress: F,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        G: FnMut(AdapterCall) -> CwrResult<Value>,
    {
        self.run_with_activity(invocation, stop, on_progress, |_| Ok(()), on_host_call)
    }

    pub fn run_with_activity<F, A, G>(
        &self,
        invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        on_progress: F,
        on_activity: A,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        A: FnMut(AdapterActivity) -> CwrResult<()>,
        G: FnMut(AdapterCall) -> CwrResult<Value>,
    {
        let mut on_host_call = on_host_call;
        self.run_with_activity_and_controlled_host_calls(
            invocation,
            stop,
            on_progress,
            on_activity,
            move |call| on_host_call(call).map(AdapterHostCallOutcome::Returned),
        )
    }

    pub fn run_with_controlled_host_calls<F, G>(
        &self,
        invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        on_progress: F,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
    {
        self.run_with_activity_and_controlled_host_calls(
            invocation,
            stop,
            on_progress,
            |_| Ok(()),
            on_host_call,
        )
    }

    pub fn run_with_activity_and_controlled_host_calls<F, A, G>(
        &self,
        invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        on_progress: F,
        on_activity: A,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        A: FnMut(AdapterActivity) -> CwrResult<()>,
        G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
    {
        self.run_with_model_output_activity_and_controlled_host_calls(
            invocation,
            stop,
            &AdapterModelOutputMailbox::discard(),
            on_progress,
            on_activity,
            on_host_call,
        )
    }

    pub fn run_with_model_output_activity_and_controlled_host_calls<F, A, G>(
        &self,
        invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        model_output: &AdapterModelOutputMailbox,
        on_progress: F,
        on_activity: A,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        A: FnMut(AdapterActivity) -> CwrResult<()>,
        G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
    {
        self.run_with_timing(
            invocation,
            stop,
            AdapterOutputSinks {
                model_output,
                timing: &AdapterTimingSink::default(),
            },
            on_progress,
            on_activity,
            on_host_call,
        )
    }

    pub fn run_with_timing<F, A, G>(
        &self,
        mut invocation: AdapterInvocation,
        stop: &ProcessAdapterStopControl,
        outputs: AdapterOutputSinks<'_>,
        on_progress: F,
        on_activity: A,
        on_host_call: G,
    ) -> CwrResult<ProcessAdapterOutcome>
    where
        F: FnMut(AdapterProgress),
        A: FnMut(AdapterActivity) -> CwrResult<()>,
        G: FnMut(AdapterCall) -> CwrResult<AdapterHostCallOutcome>,
    {
        if stop.is_requested() {
            return Ok(ProcessAdapterOutcome::Stopped);
        }
        if self.actor_integration_params["execution_environment"] == "workloop_cloud" {
            validate_cloud_route_inputs(self, &invocation)?;
        }
        let mut private_route = self.clone();
        let handoff = crate::v2026_09::local::connections::prepare_handoff(
            &mut private_route,
            &mut invocation,
            stop,
        )?;
        let allow_host_credentials = !invocation.local_inputs.as_ref().is_some_and(|plan| {
            plan.dependencies.iter().any(|dependency| {
                dependency
                    .destinations
                    .contains(&crate::input_delivery::InputDestination::OpenResponsesApiKey)
            })
        });
        let resolved = resolve_local_adapter(&private_route, &invocation, allow_host_credentials)?;
        if let Some(integration_params) = resolved.integration_params {
            invocation.integration_params = integration_params;
        }
        if self.adapter_ref == OPEN_RESPONSES_ADAPTER_REF
            || self.adapter_ref == DECISIONS_ADAPTER_REF
        {
            // Credential goes to the provider process environment, not its
            // model request, configuration diagnostics, or retained history.
            if let Some(params) = invocation.integration_params.as_object_mut() {
                params.remove("api_key");
            }
        }
        invocation.route_fingerprint = None;
        invocation.provider_fingerprint = None;
        invocation.retained_route = None;
        invocation.local_inputs = None;
        run_process_adapter_guarded(
            &resolved.process,
            invocation,
            stop,
            outputs,
            on_progress,
            on_activity,
            on_host_call,
            handoff,
        )
    }
}

include!("route_process_resolution.rs");

#[cfg(test)]
mod route_compatibility_tests {
    use super::*;
    include!("route_tests.rs");
}
