/// Admitted gateway request. Identity is supplied by the runtime, not the caller.
#[derive(Debug, Clone, Serialize)]
pub struct GatewayRequest {
    pub r#type: String,
    pub r#ref: String,
    pub requester_actor_ref: String,
    pub parent_invocation_ref: String,
    pub resource_ref: String,
    pub request: Value,
}

const GATEWAY_REQUEST_TYPE: &str = "cwp://org.coworkerprotocol/2026-09/schemas/gateway_request";

fn gateway_route_for_record<'a>(
    graph: &'a ResolvedCwrGraph,
    record: &ActionInvocationRecord,
) -> Option<&'a ResolvedCwrActionRoute> {
    graph.gateways.iter().flat_map(|gateway| [
        Some(&gateway.route), gateway.ingress_route.as_ref(), gateway.delivery_route.as_ref()
    ]).flatten().find(|route| route.actor_ref == record.target_actor_ref
        && Some(route.actor_integration_ref.as_str()) == record.actor_integration_ref.as_deref()
        && route.action_name == record.action_name)
}

fn send_request_operation() -> AdapterHostOperation {
    AdapterHostOperation {
        name: "cwr.send_request".into(),
        description: "Send a structured request to the host and receive its correlated response. Currently supports gateway_request. The host supplies caller identity.".into(),
        input_schema: json!({"type":"object", "additionalProperties":false,
            "required":["type", "resource_ref", "request"], "properties":{
                "type":{"const":GATEWAY_REQUEST_TYPE},
                "resource_ref":{"type":"string", "minLength":1},
                "request":{"type":"object"}}}),
        output_schema: serde_json::from_str(include_str!("../../../../../packages/cwr-core/contracts/cwp/schemas/2026-09/gateway_response/schema.json")).expect("gateway response schema"),
    }
}

fn gateway_payload_schema(schema: &Value) -> Value {
    let Some(payload) = schema.get("properties").and_then(|properties| properties.get("request")) else {
        return schema.clone();
    };
    let mut payload = payload.clone();
    if let Some(object) = payload.as_object_mut() {
        for keyword in ["$schema", "$id", "$defs", "definitions"] {
            if !object.contains_key(keyword) {
                if let Some(value) = schema.get(keyword) {
                    object.insert(keyword.to_owned(), value.clone());
                }
            }
        }
    }
    payload
}

fn permitted_gateway_operation(graph: &ResolvedCwrGraph, allowed: Option<&BTreeSet<String>>) -> AdapterHostOperation {
    let mut operation = send_request_operation();
    let choices: Vec<Value> = graph.gateways.iter()
        .filter(|gateway| allowed.is_none_or(|refs| refs.contains(&gateway.resource_ref)))
        .map(|gateway| json!({"const":gateway.resource_ref,
            "title":gateway.route.action_title, "description":gateway.route.action_description}))
        .collect();
    let contracts: Vec<Value> = graph.gateways.iter()
        .filter(|gateway| allowed.is_none_or(|refs| refs.contains(&gateway.resource_ref)))
        .map(|gateway| json!({"resource_ref":gateway.resource_ref,"title":gateway.route.action_title,"input_schema":gateway_payload_schema(&gateway.route.input_schema)}))
        .collect();
    operation.input_schema["properties"]["request"]["description"] = Value::String(format!("Use only the operation payload from the selected gateway's input_schema as request. The public tool arguments are exactly type, resource_ref and request. The runtime supplies every identity field. Permitted contracts: {}", serde_json::to_string(&contracts).expect("gateway contracts serialize")));
    operation.input_schema["properties"]["resource_ref"] = if choices.is_empty() {
        json!(false)
    } else { json!({"oneOf":choices}) };
    operation
}

// Use the same permitted contract for continuation compatibility and live tools.
pub(crate) fn gateway_operation_for_record(
    graph: &ResolvedCwrGraph,
    record: &ActionInvocationRecord,
) -> CwrResult<AdapterHostOperation> {
    let allowed = action_actor_allowlist(
        graph, &record.target_actor_ref, record.actor_integration_ref.as_deref(),
    )?;
    Ok(permitted_gateway_operation(graph, allowed.as_ref()))
}

impl LocalRuntime {
    fn handle_gateway_request(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<AdapterHostCallOutcome> {
        let (request_ref, resource_ref, route, permitted) = {
            let mut state = self.lock()?;
            require_active_caller(&state.admission, call)?;
            require_no_stop_fence(&state, &call.invocation_ref)?;
            ensure_host_call(&mut state.admission, call)?;
            let existing = state.admission.invocation_records().find(|record| matches!(
                &record.origin, InvocationOrigin::HostCall { caller_invocation_ref, call_id }
                if caller_invocation_ref == &call.invocation_ref && call_id == &call.call_id
            )).and_then(|record| record.input["ref"].as_str()).map(str::to_owned);
            let resource = call.input["resource_ref"].as_str().unwrap_or_default();
            let resource_ref = if let Some(path) = resource.strip_prefix("~@") {
                format!("{}{path}", state.graph.reference)
            } else {
                resource.to_owned()
            };
            let allowed = action_actor_allowlist(&state.graph, &caller.target_actor_ref,
                caller.actor_integration_ref.as_deref())?;
            let gateway = state.graph.gateways.iter()
                .find(|gateway| gateway.request_refs.contains(&resource_ref));
            // Resolve aliases to the canonical gateway before checking authority.
            let permitted = gateway.is_none_or(|gateway| allowed.as_ref()
                .is_none_or(|refs| refs.contains(&gateway.resource_ref)));
            (
                existing.unwrap_or_else(|| {
                    format!(
                        "{}/gateway-requests/{}",
                        state.admission.runtime_root_ref(),
                        Uuid::new_v4()
                    )
                }),
                gateway.map(|gateway| gateway.resource_ref.clone()).unwrap_or_else(|| resource_ref.clone()),
                gateway.filter(|_| permitted).map(|gateway| gateway.route.clone()),
                permitted,
            )
        };
        let error = |status, code: &str, summary: &str| {
            AdapterHostCallOutcome::Returned(json!({
                "request_ref":request_ref, "sequence":1, "status":status,
                "summary":summary, "error":{"code":code, "message":summary}
            }))
        };
        if !jsonschema::validator_for(&send_request_operation().input_schema)?.is_valid(&call.input)
            || CwrRef::parse(call.input["resource_ref"].as_str().unwrap_or_default()).is_err()
            || CwrRef::parse(&resource_ref).is_err()
        {
            return Ok(error(
                400,
                "invalid_request",
                "Unsupported request type or malformed gateway request.",
            ));
        }
        if !permitted {
            return Ok(error(403, "gateway_unavailable", "The gateway is not available to this caller."));
        }
        let Some(route) = route else {
            return Ok(error(
                404,
                "unsupported_contract",
                "The runtime does not support this gateway contract.",
            ));
        };
        let request = serde_json::to_value(GatewayRequest {
            r#type: GATEWAY_REQUEST_TYPE.into(),
            r#ref: request_ref.clone(),
            requester_actor_ref: caller.target_actor_ref.clone(),
            parent_invocation_ref: caller.invocation_ref.clone(),
            resource_ref,
            request: call.input["request"].clone(),
        })?;
        if route.validate_input(&request).is_err() {
            return Ok(error(
                400,
                "invalid_payload",
                "The request does not match the gateway contract.",
            ));
        }
        // Gateway routes are executable without entering ordinary coworker
        // discovery. Admission still retains the caller and child relationship.
        let admission = self.start_gateway_action(call, caller, queue, &route, request);
        let Ok(AdapterHostCallOutcome::Returned(admission)) = admission else {
            return Ok(error(
                403,
                "gateway_unavailable",
                "The gateway is not available to this caller.",
            ));
        };
        let admission: StartActionResult = serde_json::from_value(admission)?;
        let group = {
            let state = self.lock()?;
            execution_group_ref(&state.admission, caller)
                .ok_or_else(|| invalid_error("caller invocation has no execution group"))?
                .to_owned()
        };
        let response = self.execute_until_terminal(
            &group,
            &admission.invocation_ref,
            queue,
            model_output,
            observer,
            None,
        )?;
        if response.status.is_completed() {
            Ok(AdapterHostCallOutcome::Returned(json!({
                "request_ref":request_ref, "sequence":1, "status":200,
                "summary":"Gateway request completed.", "result":response.result
            })))
        } else if response.status.as_u16() == 499 {
            Ok(AdapterHostCallOutcome::Returned(json!({
                "request_ref":request_ref, "sequence":1, "status":499,
                "summary":"Gateway request stopped.", "cause":{"code":"stopped"}
            })))
        } else {
            Ok(error(
                502,
                "gateway_failed",
                "The gateway could not complete the request.",
            ))
        }
    }

    fn start_gateway_action(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        route: &ResolvedCwrActionRoute,
        input: Value,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        require_active_caller(&state.admission, call)?;
        require_no_stop_fence(&state, &call.invocation_ref)?;
        // Recheck under the admission lock, including replay of an existing call.
        let allowed = action_actor_allowlist(&state.graph, &caller.target_actor_ref,
            caller.actor_integration_ref.as_deref())?;
        if allowed.as_ref().is_some_and(|refs| !refs.contains(&route.actor_ref)) {
            return invalid("gateway request is not authorized");
        }
        let conversation_ref = state.admission.working_conversation_ref(caller)?.to_owned();
        let context = state.admission.conversation_header(&conversation_ref)
            .and_then(|conversation| conversation.owner_context_ref.as_deref());
        // Apply the existing caller/ancestry rule before probing executor
        // availability. Approved gateway endpoints remain transport resources,
        // rather than coworkers required to belong to this context.
        state.admission.check_context_work(
            &state.graph, context, true, &caller.target_actor_ref,
            Some((&route.actor_ref, Some(&route.action_name))), Some(caller),
        )?;
        let existing = state.admission.invocation_records().find(|record| matches!(
            &record.origin, InvocationOrigin::HostCall { caller_invocation_ref, call_id }
            if caller_invocation_ref == &call.invocation_ref && call_id == &call.call_id
        )).cloned();
        if let Some(existing) = existing {
            if existing.input != input || existing.conversation_ref.as_deref() != Some(conversation_ref.as_str()) {
                return invalid("gateway call id was reused with different input");
            }
            ensure_host_call(&mut state.admission, call)?;
            Self::enqueue_with_current_worker(&state.admission, &existing, queue);
            return Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
                StartActionResult::admitted(existing.invocation_ref),
            )?));
        }
        let native_route = native_gateway_route(&state, caller, route)?;
        let route = native_route.as_ref().unwrap_or(route);
        route.require_available()?;
        route.validate_input(&input)?;
        require_action_depth(&state.admission, caller)?;
        let invocation_ref = new_invocation_ref(state.admission.runtime_root_ref());
        let mut invocation = route.project_adapter_invocation(
            invocation_ref.clone(),
            input.clone(),
            state.admission.thread_observation(&conversation_ref)?,
        )?;
        invocation.source_actor_ref = Some(caller.target_actor_ref.clone());
        let request = StartActionInvocation {
            invocation_ref: invocation_ref.clone(),
            origin: InvocationOrigin::HostCall {
                caller_invocation_ref: call.invocation_ref.clone(),
                call_id: call.call_id.clone(),
            },
            requester_actor_ref: caller.target_actor_ref.clone(),
            target_actor_ref: route.actor_ref.clone(),
            conversation_ref: Some(conversation_ref.clone()),
            actor_integration_ref: Some(route.actor_integration_ref.clone()),
            action_name: route.action_name.clone(),
            input: input.clone(),
            parent_invocation_ref: Some(call.invocation_ref.clone()),
            admitted_at_unix_ms: current_unix_ms(),
        };
        let activation = format!("gateway--{}", route.action_name);
        let text = action_call_text(&activation, &input)?;
        let (_, started) = state.admission.start_gateway_child_from_host_call(
            call,
            ConversationHostActionCall {
                conversation_ref: &conversation_ref,
                caller_invocation_ref: &call.invocation_ref,
                host_call_id: &call.call_id,
                nested_invocation_ref: &invocation_ref,
                caller_actor_ref: &caller.target_actor_ref,
                target_actor_ref: &route.actor_ref,
                activation: &activation,
                input,
                text,
            },
            request,
            invocation,
        )?;
        let record = match started {
            StartInvocationOutcome::Started(record) | StartInvocationOutcome::Existing(record) => record,
        };
        if let Some(owner) = state.host_local_invocation_owners.get(&caller.invocation_ref).cloned() {
            state.host_local_invocation_owners.insert(record.invocation_ref.clone(), owner);
        }
        Self::enqueue_with_current_worker(&state.admission, &record, queue);
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            StartActionResult::admitted(record.invocation_ref),
        )?))
    }
}
