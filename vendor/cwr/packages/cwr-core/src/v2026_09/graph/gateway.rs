/// A standalone gateway route. Gateway resources are not CWR actors and do not
/// enter the conversational actor catalog.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedGateway {
    pub resource_ref: String,
    /// Mounted references accepted by host gateway requests.
    #[serde(skip)]
    pub request_refs: Vec<String>,
    pub route: ResolvedCwrActionRoute,
    pub ingress_route: Option<ResolvedCwrActionRoute>,
    pub delivery_route: Option<ResolvedCwrActionRoute>,
    pub connection_ref: Option<String>,
}

impl ResolvedGateway {
    /// Versioned digest of the complete resolved gateway and its three routes.
    pub fn approval_fingerprint(&self) -> CwrResult<String> {
        use sha2::{Digest, Sha256};
        let inputs = [
            Some(&self.route),
            self.ingress_route.as_ref(),
            self.delivery_route.as_ref(),
        ]
        .map(|route| route.map(gateway_input_approval));
        let bytes = serde_json::to_vec(&("cwr-gateway-approval-v2", self, inputs))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
}

fn gateway_input_approval(route: &ResolvedCwrActionRoute) -> Value {
    use crate::input_delivery::InputSelection;
    let mut plan = route.input_plan.clone();
    for dependency in &mut plan.dependencies {
        match &mut dependency.selection {
            InputSelection::Workspace {
                revision,
                generation,
                ..
            } => {
                *revision = 0;
                generation.clear();
            }
            InputSelection::Legacy { generation, .. } => generation.clear(),
            InputSelection::Absent | InputSelection::Unavailable => {}
        }
    }
    serde_json::json!({"uses": route.input_uses, "plan": plan})
}

#[derive(Debug, Serialize)]
struct CheckedGateway {
    resource_ref: String,
    coworker_ref: String,
    action_name: String,
    actor_integration_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ingress_action_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delivery_action_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connection_ref: Option<String>,
}

fn resolve_gateway_routes(
    graph: &ResolvedGraph,
    resolver: &dyn CheckResourceResolver,
    enforce_destinations: bool,
    allow_partial: bool,
    warnings: &mut Vec<GraphLoadWarning>,
) -> CwrResult<Vec<ResolvedGateway>> {
    let mut gateways = Vec::new();
    let mut seen = BTreeSet::new();
    for (resource_index, resource) in graph.resources.iter().enumerate() {
        let ResourceKind::Gateway(definition) = &resource.kind else {
            continue;
        };
        if !seen.insert(resource.effective_ref.clone()) {
            return invalid("duplicate gateway definition");
        }
        if graph.disabled.contains(&resource.source) { continue; }
        let resolved = (|| -> CwrResult<ResolvedGateway> {
        if definition.action_name.trim().is_empty() {
            return invalid("gateway action_name must be nonempty");
        }
        if definition
            .ingress_action_name
            .as_deref()
            .is_some_and(|name| name.trim().is_empty())
            || definition
                .delivery_action_name
                .as_deref()
                .is_some_and(|name| name.trim().is_empty())
        {
            return invalid("gateway entry point action names must be nonempty");
        }
        if definition
            .ingress_action_name
            .as_ref()
            .is_some_and(|name| name == &definition.action_name)
            || definition
                .delivery_action_name
                .as_ref()
                .is_some_and(|name| name == &definition.action_name)
            || definition.ingress_action_name.is_some()
                && definition.ingress_action_name == definition.delivery_action_name
        {
            return invalid("gateway entry point actions must be distinct");
        }
        if definition
            .connection_ref
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return invalid("gateway connection_ref must be nonempty");
        }
        if (definition.ingress_action_name.is_some() || definition.delivery_action_name.is_some())
            && definition.connection_ref.is_none()
        {
            return invalid("gateway ingress or delivery requires connection_ref");
        }

        let role_ref = definition
            .role
            .reference
            .as_deref()
            .ok_or_else(|| invalid_error("gateway role must select a resource ref"))?;
        let integration_ref = definition
            .integration
            .reference
            .as_deref()
            .ok_or_else(|| invalid_error("gateway integration must select a resource ref"))?;
        let mut role_locator = definition.role.clone();
        role_locator.reference = Some(resolve_graph_ref(graph, resource.scope_id, role_ref));
        let mut integration_locator = definition.integration.clone();
        integration_locator.reference =
            Some(resolve_graph_ref(graph, resource.scope_id, integration_ref));
        let pseudo = CoworkerFile {
            message_binding_preference: Vec::new(),
            name: Some("Gateway handler".into()),
            slug: None,
            authority: definition.authority.clone(),
            imports: Vec::new(),
            roles: vec![role_locator],
            integrations: vec![IntegrationBinding {
                locator: integration_locator,
                default: false,
                message_mode: ActorIntegrationMessageMode::ActionsOnly,
                params: definition.params.clone(),
            }],
            native_tools: Vec::new(),
        };
        let roles = selected_roles(resource, &pseudo, graph)?;
        let binding = resolve_binding(
            resource,
            &pseudo,
            &pseudo.integrations[0],
            &roles,
            graph,
            resolver,
            enforce_destinations,
        )?;
        let mut request_refs = BTreeSet::new();
        request_refs.insert(resource.effective_ref.clone());
        for scope in &graph.scopes {
            for (reference, target) in &scope.lookup {
                if target == &ResolvedTarget::Resource(resource_index) {
                    request_refs.insert(reference.clone());
                }
            }
        }
        let actions = roles
            .iter()
            .flat_map(|(_, role)| role.actions.iter())
            .collect::<Vec<_>>();
        let make_route = |name: &str| -> CwrResult<ResolvedCwrActionRoute> {
            let matches = actions
                .iter()
                .filter(|action| action.action_name() == name)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return invalid("gateway action must select exactly one action in its role");
            }
            let role = roles
                .iter()
                .find(|(_, role)| {
                    role.actions
                        .iter()
                        .any(|candidate| candidate.action_name() == name)
                })
                .ok_or_else(|| invalid_error("gateway action role is unavailable"))?;
            let route = action_route(matches[0], role.0, &binding, graph, resolver)?;
            if route.response_mode != crate::action_contract::ResponseMode::Sync {
                return invalid("gateway handler must return a synchronous terminal response");
            }
            Ok(route)
        };
        Ok(ResolvedGateway {
            resource_ref: resource.effective_ref.clone(),
            request_refs: request_refs.into_iter().collect(),
            route: make_route(&definition.action_name)?,
            ingress_route: definition
                .ingress_action_name
                .as_deref()
                .map(make_route)
                .transpose()?,
            delivery_route: definition
                .delivery_action_name
                .as_deref()
                .map(make_route)
                .transpose()?,
            connection_ref: definition.connection_ref.clone(),
        })
        })();
        match resolved {
            Ok(gateway) => gateways.push(gateway),
            Err(_error) if allow_partial && resource_index != graph.root_scope => warnings.push(GraphLoadWarning { source: resource.source.clone(), message: "Connection disabled because its configuration or a required dependency is invalid.".into() }),
            Err(error) => return Err(error),
        }
    }
    Ok(gateways)
}

#[cfg(test)]
mod gateway_approval_tests {
    use super::*;
    use crate::input_delivery::{InputDependency, InputDestination, InputSelection, InputUse};

    fn gateway() -> ResolvedGateway {
        let route = ResolvedCwrActionRoute {
            role_ref: String::new(),
            contract_revision: String::new(),
            execution_scope: crate::action_contract::ExecutionScope::default(),
            response_mode: crate::action_contract::ResponseMode::Sync,
            actor_ref: String::new(),
            coworker_ref: String::new(),
            coworker_source: PathBuf::new(),
            working_path: PathBuf::new(),
            action_name: String::new(),
            action_title: String::new(),
            action_description: String::new(),
            input_schema: serde_json::json!({}),
            output_schema: serde_json::json!({}),
            actor_integration_ref: String::new(),
            actor_integration_source: PathBuf::new(),
            integration_ref: String::new(),
            integration_source: PathBuf::new(),
            adapter_ref: String::new(),
            available_authority: vec![],
            missing_required_authority: vec![],
            adapter_params: serde_json::json!({}),
            integration_params: serde_json::json!({}),
            actor_integration_params: serde_json::json!({}),
            input_uses: vec![InputUse {
                source: requirements::InputSource {
                    storage: None,
                    scope: "workspace".into(),
                    alias: "token".into(),
                    type_ref: "secret".into(),
                },
                destination: InputDestination::ShellEnv {
                    name: "TOKEN".into(),
                },
            }],
            input_plan: Default::default(),
        };
        ResolvedGateway {
            resource_ref: "gateway".into(),
            request_refs: vec!["gateway".into()],
            ingress_route: Some(route.clone()),
            delivery_route: Some(route.clone()),
            route,
            connection_ref: None,
        }
    }

    #[test]
    fn gateway_approval_covers_managed_bindings_on_every_route() {
        let original = gateway();
        let fingerprint = original.approval_fingerprint().unwrap();
        for index in 0..3 {
            for change_source in [false, true] {
                let mut changed = original.clone();
                let route = match index {
                    0 => &mut changed.route,
                    1 => changed.ingress_route.as_mut().unwrap(),
                    _ => changed.delivery_route.as_mut().unwrap(),
                };
                if change_source {
                    route.input_uses[0].source.alias = "other".into();
                } else {
                    route.input_uses[0].destination = InputDestination::ShellEnv {
                        name: "OTHER".into(),
                    };
                }
                assert_ne!(fingerprint, changed.approval_fingerprint().unwrap());
            }
        }
    }

    #[test]
    fn gateway_approval_tracks_selection_identity_but_allows_rotation() {
        let mut original = gateway();
        original
            .route
            .input_plan
            .dependencies
            .push(InputDependency {
                source: original.route.input_uses[0].source.clone(),
                destinations: vec![original.route.input_uses[0].destination.clone()],
                selection: InputSelection::Legacy {
                    key_id: "credential-one".into(),
                    generation: "one".into(),
                },
            });
        let fingerprint = original.approval_fingerprint().unwrap();
        let mut changed = original.clone();
        changed.route.input_plan.dependencies[0].selection = InputSelection::Legacy {
            key_id: "credential-one".into(),
            generation: "two".into(),
        };
        assert_eq!(fingerprint, changed.approval_fingerprint().unwrap());
        changed.route.input_plan.dependencies[0].selection = InputSelection::Legacy {
            key_id: "credential-two".into(),
            generation: "two".into(),
        };
        assert_ne!(fingerprint, changed.approval_fingerprint().unwrap());
    }
}
