use super::*;
use crate::adapter::AdapterReadiness;

impl ResolvedCwrGraph {
    pub(crate) fn message_binding_route_snapshot(
        &self,
        actor_ref: &str,
        through_binding_ref: &str,
    ) -> CwrResult<Vec<(String, bool, Option<String>)>> {
        let actor = self
            .actors
            .iter()
            .find(|actor| actor.actor_ref == actor_ref)
            .ok_or_else(|| invalid_error(format!("unknown message recipient {actor_ref}")))?;
        let mut snapshot = Vec::new();
        for reference in &actor.message_binding_preference {
            let binding = self
                .actor_integrations
                .iter()
                .find(|binding| {
                    binding.actor_ref == actor_ref && binding.actor_integration_ref == *reference
                })
                .ok_or_else(|| invalid_error("preferred message binding is unavailable"))?;
            let mut available = binding.is_available();
            let route = if available {
                match binding.process_adapter_route() {
                    Ok(route) => Some(route.fingerprint()),
                    Err(error) if reference == through_binding_ref => return Err(error),
                    Err(_) => {
                        available = false;
                        None
                    }
                }
            } else {
                None
            };
            snapshot.push((reference.clone(), available, route));
            if reference == through_binding_ref {
                return Ok(snapshot);
            }
        }
        invalid("preferred message binding is unavailable")
    }

    /// Resolve only new, implicitly targeted messages. Stored invocations and
    /// exact binding/action targets must never call this selector.
    pub(crate) fn preferred_message_binding(&self, actor_ref: &str) -> CwrResult<Option<String>> {
        self.preferred_message_binding_with(actor_ref, |binding| {
            Ok(binding.process_adapter_route()?.readiness())
        })
    }

    pub(crate) fn preferred_message_binding_with(
        &self,
        actor_ref: &str,
        mut readiness: impl FnMut(&ResolvedCwrActorIntegration) -> CwrResult<AdapterReadiness>,
    ) -> CwrResult<Option<String>> {
        let Some(actor) = self
            .actors
            .iter()
            .find(|actor| actor.actor_ref == actor_ref)
        else {
            return invalid(format!("unknown message recipient {actor_ref}"));
        };
        let mut reasons = Vec::new();
        for reference in &actor.message_binding_preference {
            let binding = self
                .actor_integrations
                .iter()
                .find(|binding| {
                    binding.actor_ref == actor_ref && &binding.actor_integration_ref == reference
                })
                .ok_or_else(|| invalid_error("preferred message binding is unavailable"))?;
            if !binding.is_available() {
                reasons.push("A configured connection needs permission.".to_owned());
                continue;
            }
            let status = match readiness(binding) {
                Ok(status) => status,
                Err(_) => {
                    reasons.push("A configured connection is unavailable.".to_owned());
                    continue;
                }
            };
            if status.is_ready() {
                return Ok(Some(reference.clone()));
            }
            reasons.push(status.message);
        }
        if reasons.is_empty() {
            return Ok(None);
        }
        invalid(format!(
            "{} needs a ready connection before starting this message. {}",
            actor
                .coworker_name
                .as_deref()
                .unwrap_or(&actor.coworker_slug),
            reasons.join(" ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::AdapterReadinessStatus;

    #[test]
    fn preference_selects_first_ready_and_reports_all_setup_requirements() {
        let mut graph = crate::v2026_09::test_fixture::graph().clone();
        let actor = graph
            .actors
            .iter_mut()
            .find(|actor| actor.coworker_slug == "helper")
            .unwrap();
        let actor_ref = actor.actor_ref.clone();
        let first = actor.default_actor_integration_ref.clone().unwrap();
        let second = format!("{actor_ref}/bindings/secondary");
        actor.message_binding_preference = vec![first.clone(), second.clone()];
        let mut binding = graph
            .actor_integrations
            .iter()
            .find(|binding| binding.actor_integration_ref == first)
            .unwrap()
            .clone();
        binding.actor_integration_ref = second.clone();
        graph.actor_integrations.push(binding);
        for ready in [
            vec![first.clone(), second.clone()],
            vec![second.clone()],
            vec![],
        ] {
            let result = graph.preferred_message_binding_with(&actor_ref, |binding| {
                Ok(AdapterReadiness {
                    status: if ready.contains(&binding.actor_integration_ref) {
                        AdapterReadinessStatus::Ready
                    } else {
                        AdapterReadinessStatus::SetupRequired
                    },
                    code: "test".into(),
                    message: format!("Sign in to {}.", binding.actor_integration_ref),
                    repair: None,
                })
            });
            if let Some(expected) = ready.first() {
                assert_eq!(result.unwrap().as_ref(), Some(expected));
            } else {
                let error = result.unwrap_err().to_string();
                assert!(error.contains(&first) && error.contains(&second));
            }
        }
        let mut probed = Vec::new();
        let selected = graph
            .preferred_message_binding_with(&actor_ref, |binding| {
                probed.push(binding.actor_integration_ref.clone());
                if binding.actor_integration_ref == first {
                    return invalid("adapter executable is unavailable");
                }
                Ok(AdapterReadiness {
                    status: AdapterReadinessStatus::Ready,
                    code: "ready".into(),
                    message: "Ready".into(),
                    repair: None,
                })
            })
            .unwrap();
        assert_eq!(selected, Some(second.clone()));
        assert_eq!(probed, [first.clone(), second.clone()]);
        graph
            .actors
            .iter_mut()
            .find(|actor| actor.actor_ref == actor_ref)
            .unwrap()
            .message_binding_preference
            .clear();
        assert_eq!(
            graph
                .preferred_message_binding_with(&actor_ref, |_| panic!(
                    "ordinary defaults must not probe or fall back"
                ))
                .unwrap(),
            None
        );
    }
}
