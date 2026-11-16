impl LocalRuntime {
    /// Check a retained root against the exact gateway route selected by the
    /// current graph. The service owner separately checks which writer admitted it.
    pub fn gateway_invocation_matches(
        &self,
        connection_ref: &str,
        integration_ref: &str,
        provider_event_ref: &str,
        invocation_ref: &str,
        kind: crate::v2026_09::service::GatewayInvocationKind,
        input: &serde_json::Value,
    ) -> CwrResult<bool> {
        Ok(self
            .inspect_gateway_invocation(
                connection_ref,
                integration_ref,
                provider_event_ref,
                invocation_ref,
                kind,
                input,
            )?
            .0)
    }

    /// Return a terminal only after checking the issuer, route, and exact
    /// retained input under the same runtime lock. This also works on replay.
    pub fn inspect_gateway_invocation(
        &self,
        connection_ref: &str,
        integration_ref: &str,
        provider_event_ref: &str,
        invocation_ref: &str,
        kind: crate::v2026_09::service::GatewayInvocationKind,
        input: &serde_json::Value,
    ) -> CwrResult<(
        bool,
        Option<crate::v2026_09::service::GatewayInvocationTerminal>,
    )> {
        use crate::v2026_09::service::{GatewayInvocationKind, GatewayInvocationTerminal};

        let state = self.lock()?;
        let mut gateways = state
            .graph
            .gateways
            .iter()
            .filter(|gateway| gateway.connection_ref.as_deref() == Some(connection_ref));
        let Some(gateway) = gateways.next() else {
            return Ok((false, None));
        };
        if gateways.next().is_some() {
            return Ok((false, None));
        }
        let route = match kind {
            GatewayInvocationKind::Request => Some(&gateway.route),
            GatewayInvocationKind::Ingress => gateway.ingress_route.as_ref(),
            GatewayInvocationKind::Delivery => gateway.delivery_route.as_ref(),
        };
        let Some(route) = route else {
            return Ok((false, None));
        };
        let Some(record) = state.admission.invocation_record(invocation_ref) else {
            return Ok((false, None));
        };
        let origin_matches = match kind {
            GatewayInvocationKind::Request => {
                let parent_ref = record.parent_invocation_ref.as_deref();
                let parent =
                    parent_ref.and_then(|reference| state.admission.invocation_record(reference));
                let delivery_ref = input
                    .pointer("/request/delivery_ref")
                    .and_then(Value::as_str);
                matches!((&record.origin, parent),
                    (InvocationOrigin::HostCall { caller_invocation_ref, .. }, Some(parent))
                    if Some(caller_invocation_ref.as_str()) == parent_ref
                        && parent.target_actor_ref == record.requester_actor_ref
                        && input["requester_actor_ref"] == record.requester_actor_ref
                        && input["parent_invocation_ref"] == *caller_invocation_ref
                        && input["resource_ref"].as_str().is_some_and(|reference| gateway.request_refs.iter().any(|accepted| accepted == reference))
                        && input["type"] == "cwp://org.coworkerprotocol/2026-09/schemas/gateway_request"
                        && delivery_ref.is_some_and(|reference| state.admission.admissions().any(|admission| {
                            admission.reply_message_ref.as_deref() == Some(reference)
                                && admission.invocation_ref.as_deref().and_then(|ref_| state.admission.invocation_record(ref_))
                                    .is_some_and(|source| source.target_actor_ref == record.requester_actor_ref
                                        && source.requester_actor_ref == parent.requester_actor_ref
                                        && parent.conversation_ref.is_some()
                                        && source.conversation_ref == parent.conversation_ref)
                        }))
                )
            }
            _ => {
                record.parent_invocation_ref.is_none()
                    && matches!(&record.origin, InvocationOrigin::Ingress {
                integration_ref: origin_integration,
                provider_event_ref: origin_event,
            } if origin_integration == integration_ref && origin_event == provider_event_ref)
            }
        };
        let authorized = origin_matches
            && record.target_actor_ref == route.actor_ref
            && record.actor_integration_ref.as_deref()
                == Some(route.actor_integration_ref.as_str())
            && record.action_name == route.action_name
            && record.input == *input;
        let terminal = authorized
            .then(|| record.terminal_response())
            .flatten()
            .map(|response| GatewayInvocationTerminal {
                status: response.status.as_u16(),
                result: response.result.clone(),
            });
        Ok((authorized, terminal))
    }

}
