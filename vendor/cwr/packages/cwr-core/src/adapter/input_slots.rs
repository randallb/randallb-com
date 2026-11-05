//! Exact managed slots on a private adapter route; never recursive substitution.
use super::*;

impl ResolvedProcessAdapterRoute {
    /// Older retained invocations have no typed plan. Accept their resolved
    /// parameters, but never send unresolved old or current placeholders.
    pub(crate) fn has_unresolved_inputs(&self, invocation: &AdapterInvocation) -> bool {
        let unresolved = |value: &Value| {
            value.as_str().is_some_and(|value| {
                value.starts_with("managed-input:") || value.starts_with("cwr-local-key:")
            })
        };
        [&self.integration_params, &invocation.integration_params]
            .into_iter()
            .any(|params| match self.adapter_ref.as_str() {
                SH_ADAPTER_REF => params
                    .get("env")
                    .and_then(Value::as_object)
                    .is_some_and(|env| env.values().any(unresolved)),
                OPEN_RESPONSES_ADAPTER_REF | DECISIONS_ADAPTER_REF => {
                    params.get("api_key").is_some_and(unresolved)
                }
                _ => false,
            })
    }

    pub(crate) fn omit_input(
        &mut self,
        destination: &crate::input_delivery::InputDestination,
    ) -> CwrResult<()> {
        omit_input(&mut self.integration_params, destination)
    }

    pub(crate) fn materialize_input(
        &mut self,
        destination: &crate::input_delivery::InputDestination,
        value: &str,
    ) -> CwrResult<()> {
        use crate::input_delivery::InputDestination;
        match destination {
            InputDestination::ShellEnv { name } if self.adapter_ref == SH_ADAPTER_REF => {
                crate::workspace_environment::validate_environment_name(name)?
            }
            InputDestination::OpenResponsesApiKey
                if (self.adapter_ref == OPEN_RESPONSES_ADAPTER_REF
                    || self.adapter_ref == DECISIONS_ADAPTER_REF) => {}
            _ => return Err("Unsupported input destination.".into()),
        }
        let slot = self
            .integration_params
            .pointer_mut(&destination.pointer())
            .ok_or("Input destination changed.")?;
        *slot = json!(value);
        Ok(())
    }

    pub(crate) fn retain_input_slots(
        &mut self,
        invocation: &mut AdapterInvocation,
    ) -> CwrResult<()> {
        if let Some(plan) = &invocation.local_inputs {
            for dependency in &plan.dependencies {
                for destination in &dependency.destinations {
                    let marker = format!("managed-input:{}", dependency.source.alias);
                    self.materialize_input(destination, &marker)?;
                    *invocation
                        .integration_params
                        .pointer_mut(&destination.pointer())
                        .ok_or("Input destination changed.")? = json!(marker);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn same_input_template(&self, expected: &Self) -> bool {
        let clean = |route: &Self| {
            let mut route = route.clone();
            if let Some(params) = route.adapter_params.as_object_mut() {
                params.remove("cwr_native_tools");
            }
            if let Some(params) = route.actor_integration_params.as_object_mut() {
                params.remove("cwr_action_actor_refs");
            }
            route
        };
        clean(self) == clean(expected)
    }

    pub(crate) fn matches_invocation_parameters(&self, invocation: &AdapterInvocation) -> bool {
        self.adapter_params == invocation.adapter_params
            && self.integration_params == invocation.integration_params
            && self.actor_integration_params == invocation.actor_integration_params
    }
}

pub(crate) fn omit_input(
    params: &mut Value,
    destination: &crate::input_delivery::InputDestination,
) -> CwrResult<()> {
    use crate::input_delivery::InputDestination;
    match destination {
        InputDestination::ShellEnv { name } => {
            params
                .get_mut("env")
                .and_then(Value::as_object_mut)
                .ok_or("Input destination changed.")?
                .remove(name);
        }
        InputDestination::OpenResponsesApiKey => {
            params
                .as_object_mut()
                .ok_or("Input destination changed.")?
                .remove("api_key");
        }
    }
    Ok(())
}

pub(super) fn open_responses_key(
    route: &ResolvedProcessAdapterRoute,
    allow_host_credentials: bool,
) -> Option<OsString> {
    route
        .integration_params
        .get("api_key")
        .and_then(Value::as_str)
        .map(OsString::from)
        .or_else(|| {
            allow_host_credentials
                .then(|| {
                    std::env::var_os("OPEN_RESPONSES_API_KEY")
                        .filter(|value| !value.is_empty())
                        .or_else(|| {
                            std::env::var_os("OPENROUTER_API_KEY").filter(|value| !value.is_empty())
                        })
                })
                .flatten()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_managed_api_key_cannot_fall_back_to_the_host_environment() {
        let mut route = ResolvedProcessAdapterRoute::new(
            OPEN_RESPONSES_ADAPTER_REF,
            ".",
            ".",
            json!({}),
            json!({"api_key":"managed"}),
            json!({}),
        );
        assert_eq!(open_responses_key(&route, false), Some("managed".into()));
        route
            .omit_input(&crate::input_delivery::InputDestination::OpenResponsesApiKey)
            .unwrap();
        assert_eq!(open_responses_key(&route, false), None);
    }
    #[test]
    fn decisions_keys_use_only_the_declared_private_slot() {
        let mut route = ResolvedProcessAdapterRoute::new(
            DECISIONS_ADAPTER_REF,
            ".",
            ".",
            json!({}),
            json!({"api_key":"managed-input:router"}),
            json!({}),
        );
        let mut invocation = AdapterInvocation::new(
            "cwp://com.example/invocations/grade",
            "grade--result",
            json!({}),
            json!({}),
        );
        invocation.integration_params = route.integration_params.clone();
        assert!(route.has_unresolved_inputs(&invocation));
        route
            .materialize_input(
                &crate::input_delivery::InputDestination::OpenResponsesApiKey,
                "private-key",
            )
            .unwrap();
        assert_eq!(
            open_responses_key(&route, false),
            Some("private-key".into())
        );
        route
            .omit_input(&crate::input_delivery::InputDestination::OpenResponsesApiKey)
            .unwrap();
        assert_eq!(open_responses_key(&route, false), None);
    }
}
