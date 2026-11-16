impl LocalRuntime {
    fn install_lifecycle_operations(
        &self,
        invocation: &mut AdapterInvocation,
        record: &ActionInvocationRecord,
    ) -> CwrResult<()> {
        let (working_ref, owner, gateway_handler) = {
            let state = self.lock()?;
            let working_ref = state.admission.working_conversation_ref(record)?.to_owned();
            let owner = state
                .host_local_invocation_owners
                .get(&record.invocation_ref)
                .cloned();
            if invocation.execution_scope == crate::action_contract::ExecutionScope::OwnConversation
                && invocation.thread.get("conversation_ref").and_then(Value::as_str) != Some(working_ref.as_str()) {
                let requesting_context = invocation.thread.clone();
                invocation.thread = state.admission.thread_observation(&working_ref)?;
                invocation.thread["requesting_context"] = requesting_context;
            }
            let gateway_handler = gateway_route_for_record(&state.graph, record).is_some();
            (working_ref, owner, gateway_handler)
        };
        // A Shell gateway may use an explicitly registered host action (Gmail).
        // It does not gain ordinary coworker messaging or discovery.
        if gateway_handler {
            if invocation.host_operations.is_empty() {
                invocation.host_operations.push(invoke_action_operation(&BTreeMap::new()));
                invocation.host_operations.extend(discovery_operations().into_iter()
                    .filter(|operation| operation.name == LIST_ACTIONS_HOST_OPERATION));
            }
            return Ok(());
        }
        let conversation_ref = working_ref.as_str();
        let routes = {
            let state = self.lock()?;
            let allowlist = action_actor_allowlist(
                &state.graph,
                &record.target_actor_ref,
                record.actor_integration_ref.as_deref(),
            )?;
            visible_action_routes_in_upstream_scope(
                &state,
                conversation_ref,
                &record.target_actor_ref,
                allowlist.as_ref(),
                None,
                owner.as_deref(),
            )?
        };
        if invocation.thread.get("coworkers").is_none() {
            if let Some(response) = self.discovery_as_in_host_scope(Some(conversation_ref), &record.target_actor_ref,
                record.actor_integration_ref.as_deref(), Some(record), owner.as_deref(), &IntrinsicCwrCommand::ListCoworkers)? {
                invocation.thread["coworkers"] = serde_json::to_value(response)?;
            }
        }
        if invocation.host_operations.is_empty() {
            invocation.host_operations.push(send_message_operation());
            let state = self.lock()?;
            let allowed = action_actor_allowlist(&state.graph, &record.target_actor_ref, record.actor_integration_ref.as_deref())?;
            invocation.host_operations.push(permitted_gateway_operation(&state.graph, allowed.as_ref()));
            drop(state);
            invocation.host_operations.push(invoke_action_operation(&routes));
            invocation.host_operations.extend(discovery_operations());
            invocation.host_operations.push(AdapterHostOperation {
                name: STOP_INVOCATION_HOST_OPERATION.into(),
                description: "Stop this invocation or one of its direct children.".into(),
                input_schema: stop_invocation_input_schema(),
                output_schema: stop_invocation_output_schema(),
            });
            self.install_native_actions(invocation, record, &routes)?;
        }
        if !invocation.host_operations.iter().any(|op| op.name == CREATE_CONVERSATION_HOST_OPERATION) {
            invocation.host_operations.push(create_conversation_operation());
        }
        if !invocation.host_operations.iter().any(|op| op.name == LIST_CONTEXTS_HOST_OPERATION) {
            invocation.host_operations.push(list_contexts_operation());
        }
        if invocation.retained_route.as_ref().is_some_and(|route| route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF)
            && !invocation.host_operations.iter().any(|op| op.name == READ_FILE_HOST_OPERATION) {
            invocation.host_operations.push(read_file_operation());
        }
        if invocation.retained_route.as_ref().is_some_and(|route| route.adapter_ref() == crate::adapter::CODEX_ADAPTER_REF)
            && !invocation.host_operations.iter().any(|op| op.name == crate::adapter::user_input::USER_INPUT_OPERATION) {
            invocation.host_operations.push(crate::adapter::user_input::host_operation());
        }
        Ok(())
    }

    fn handle_host_call(
        &self,
        call: AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<AdapterHostCallOutcome> {
        if call.invocation_ref != caller.invocation_ref {
            return invalid("adapter host call belongs to another invocation");
        }
        if call.operation == READ_FILE_HOST_OPERATION {
            return self.handle_read_file(&call, caller);
        }
        if call.operation == crate::adapter::user_input::USER_INPUT_OPERATION {
            return self.handle_provider_question(&call, caller);
        }
        if let Some(result) = self.retained_host_result(&call)? {
            return result
                .map(AdapterHostCallOutcome::Returned)
                .map_err(invalid_error);
        }
        let outcome = match call.operation.as_str() {
            LIST_COWORKERS_HOST_OPERATION | LIST_ACTIONS_HOST_OPERATION => self.handle_discovery_call(&call, caller),
            "cwr.send_request" => self.handle_gateway_request(&call, caller, queue, model_output, observer),
            LIST_CONTEXTS_HOST_OPERATION => self.handle_list_contexts(&call, caller),
            CREATE_CONVERSATION_HOST_OPERATION => self.handle_create_conversation(&call, caller),
            SEND_MESSAGE_HOST_OPERATION => self.handle_send_message(&call, caller, queue, model_output, observer),
            START_ACTION_HOST_OPERATION => self.handle_start_action(&call, caller, queue),
            INVOKE_ACTION_HOST_OPERATION => {
                self.handle_invoke_action(&call, caller, queue, model_output, observer)
            }
            STOP_INVOCATION_HOST_OPERATION => self.handle_stop_invocation(&call, caller, queue),
            operation => invalid(format!(
                "adapter requested unsupported host operation {operation}"
            )),
        };
        match &outcome {
            Ok(AdapterHostCallOutcome::Returned(result)) => {
                self.lock()?.admission.retain_host_call_result(
                    &call,
                    Some(result.clone()),
                    None,
                    current_unix_ms(),
                )?;
            }
            Err(error) => {
                let _ = self.lock().and_then(|mut state| {
                    state.admission.retain_host_call_result(
                        &call,
                        None,
                        Some(error.to_string()),
                        current_unix_ms(),
                    )
                });
            }
        }
        outcome
    }

    fn handle_invoke_action(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        model_output: &AdapterModelOutputMailbox,
        observer: &mut dyn FnMut(LocalRuntimeEvent),
    ) -> CwrResult<AdapterHostCallOutcome> {
        let input: InvokeActionInput = parse_input("invoke-action", &call.input)?;
        let deadline = input.timeout_ms.map(|timeout_ms| {
            (
                timeout_ms,
                Instant::now() + Duration::from_millis(timeout_ms),
            )
        });
        let admission = self.start_action(
            call,
            caller,
            queue,
            StartActionInput {
                activation: String::new(),
                input: input.input,
            },
            Some(&input.target),
        )?;
        let AdapterHostCallOutcome::Returned(admission) = admission;
        let admission: StartActionResult = serde_json::from_value(admission)?;
        let is_async = {
            let state = self.lock()?;
            state.admission.execution_conversation(&admission.invocation_ref)
                .is_some_and(|execution| execution.activity.iter().any(|activity| matches!(
                    &activity.event,
                    crate::v2026_09::conversation::ConversationActivityEvent::TurnStarted { invocation: Some(snapshot), .. }
                        if snapshot.get("response_mode").and_then(Value::as_str) == Some("async")
                )))
        };
        if is_async {
            return Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(admission)?));
        }
        let caller_execution_group_ref = {
            let state = self.lock()?;
            execution_group_ref(&state.admission, caller)
                .ok_or_else(|| invalid_error("caller invocation has no execution group"))?
                .to_owned()
        };
        let deadline_failure = Arc::new(Mutex::new(None));
        let deadline = deadline.map(|(timeout_ms, expires_at)| {
            let (cancel, wait) = std::sync::mpsc::channel();
            let runtime = self.clone();
            let invocation_ref = admission.invocation_ref.clone();
            let actor_ref = caller.target_actor_ref.clone();
            let deadline_failure = deadline_failure.clone();
            let worker = thread::spawn(move || -> CwrResult<bool> {
                let remaining = expires_at.saturating_duration_since(Instant::now());
                let timed_out = matches!(
                    wait.recv_timeout(remaining),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                );
                if timed_out {
                    if let Err(error) = runtime.stop_invocation_as_without_upstream_wait(
                        &invocation_ref,
                        &actor_ref,
                        Some(format!("Nested action exceeded its {timeout_ms} ms deadline")),
                    ) {
                        let message = format!(
                            "nested action deadline could not stop {invocation_ref}: {error}"
                        );
                        *deadline_failure.lock().unwrap() = Some(message);
                        runtime.stop_active_deadline_descendants(&invocation_ref)?;
                        return Err(error);
                    }
                }
                Ok(timed_out)
            });
            (timeout_ms, cancel, worker)
        });
        let response = self.execute_until_terminal(
            &caller_execution_group_ref,
            &admission.invocation_ref,
            queue,
            model_output,
            observer,
            Some(&deadline_failure),
        );
        let mut timed_out = None;
        if let Some((timeout_ms, cancel, worker)) = deadline {
            let _ = cancel.send(());
            timed_out = Some((
                timeout_ms,
                worker
                .join()
                    .map_err(|_| invalid_error("nested action deadline worker panicked"))??,
            ));
        }
        let response = response?;
        if timed_out.is_some_and(|(_, expired)| expired) && response.status.is_completed() {
            let timeout_ms = timed_out.unwrap().0;
            return invalid(format!(
                "nested action exceeded its {timeout_ms} ms deadline before completing"
            ));
        }
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            response,
        )?))
    }

    fn stop_active_deadline_descendants(&self, invocation_ref: &str) -> CwrResult<()> {
        let stop_wait = Instant::now() + Duration::from_secs(2);
        loop {
            let (controls, terminal) = {
                let state = self.lock()?;
                let controls = state
                    .active
                    .iter()
                    .filter(|(candidate_ref, _)| {
                        state
                            .admission
                            .invocation_record(candidate_ref)
                            .is_some_and(|candidate| {
                                is_attached_descendant_or_self(
                                    &state.admission,
                                    candidate,
                                    invocation_ref,
                                )
                            })
                    })
                    .map(|(_, control)| control.clone())
                    .collect::<Vec<_>>();
                let terminal = state
                    .admission
                    .invocation_record(invocation_ref)
                    .is_some_and(|record| record.state.is_terminal());
                (controls, terminal)
            };
            if !controls.is_empty() {
                for control in controls {
                    control.stop();
                }
                return Ok(());
            }
            if terminal || Instant::now() >= stop_wait {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn handle_start_action(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
    ) -> CwrResult<AdapterHostCallOutcome> {
        if call.input.get("target").is_some() {
            let input: InvokeActionInput = parse_input("start-action", &call.input)?;
            self.start_action(call, caller, queue,
                StartActionInput { activation: String::new(), input: input.input }, Some(&input.target))
        } else {
            let input: StartActionInput = parse_input("start-action", &call.input)?;
            self.start_action(call, caller, queue, input, None)
        }
    }

    fn start_action(
        &self,
        call: &AdapterCall,
        caller: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
        mut input: StartActionInput,
        exact_target: Option<&ExactActionTarget>,
    ) -> CwrResult<AdapterHostCallOutcome> {
        let working_ref = self.lock()?.admission.working_conversation_ref(caller)?.to_owned();
        let conversation_ref = working_ref.as_str();
        let mut state = self.lock()?;
        if state.shutting_down {
            return invalid("local runtime is shutting down");
        }
        require_no_stop_fence(&state, &call.invocation_ref)?;
        require_active_caller(&state.admission, call)?;
        let existing = state.admission.invocation_records().find(|record| {
            matches!(
                &record.origin,
                InvocationOrigin::HostCall {
                    caller_invocation_ref,
                    call_id,
                } if caller_invocation_ref == &call.invocation_ref && call_id == &call.call_id
            )
        }).cloned();
        if let Some(existing) = existing {
            if existing.input != input.input
                || existing.conversation_ref.as_deref() != Some(conversation_ref)
            {
                return invalid("start-action call id was reused with different input");
            }
            let invocation_ref = existing.invocation_ref.clone();
            let runnable = existing.is_runnable();
            if runnable { require_current_request_authority(&state, &existing)?; }
            ensure_host_call(&mut state.admission, call)?;
            Self::enqueue_with_current_worker(&state.admission, &existing, queue);
            return Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
                StartActionResult::admitted(invocation_ref),
            )?));
        }
        let owner = state
            .host_local_invocation_owners
            .get(&caller.invocation_ref)
            .cloned();
        let route = if let Some(target) = exact_target {
            let route = resolve_exact_action(&state, caller, target, owner.as_deref())?;
            let graph = upstream_graph(&state, owner.as_deref())?;
            let actor = graph.actors.iter().find(|actor| actor.actor_ref == route.actor_ref)
                .ok_or_else(|| invalid_error("target coworker is unavailable"))?;
            input.activation = format!("{}--{}", runtime_actor_identity(&graph, actor)?.handle, route.action_name);
            route
        } else {
            let allowlist = action_actor_allowlist(&state.graph, &caller.target_actor_ref, caller.actor_integration_ref.as_deref())?;
            visible_action_routes_in_upstream_scope(&state, conversation_ref, &caller.target_actor_ref,
                allowlist.as_ref(), None, owner.as_deref())?.get(&input.activation).cloned()
                .ok_or_else(|| invalid_error(format!("action {} is unavailable", input.activation)))?
        };
        route.validate_input(&input.input)?;
        require_action_depth(&state.admission, caller)?;
        let invocation_ref = new_invocation_ref(state.admission.runtime_root_ref());
        let thread = state.admission.thread_observation(conversation_ref)?;
        let mut invocation = route.project_adapter_invocation(
            invocation_ref.clone(),
            input.input.clone(),
            thread,
        )?;
        let requesting_actor_ref = if gateway_route_for_record(&state.graph, caller).is_some() {
            caller.requester_actor_ref.clone()
        } else {
            caller.target_actor_ref.clone()
        };
        invocation.source_actor_ref = Some(requesting_actor_ref.clone());
        let request = StartActionInvocation {
            invocation_ref: invocation_ref.clone(),
            origin: InvocationOrigin::HostCall {
                caller_invocation_ref: call.invocation_ref.clone(),
                call_id: call.call_id.clone(),
            },
            requester_actor_ref: requesting_actor_ref.clone(),
            target_actor_ref: route.actor_ref.clone(),
            conversation_ref: Some(conversation_ref.into()),
            actor_integration_ref: Some(route.actor_integration_ref.clone()),
            action_name: route.action_name.clone(),
            input: input.input.clone(),
            parent_invocation_ref: Some(call.invocation_ref.clone()),
            admitted_at_unix_ms: current_unix_ms(),
        };
        let text = action_call_text(&input.activation, &input.input)?;
        let (_, started) = state.admission.start_child_from_host_action_call(
            call,
            ConversationHostActionCall {
                conversation_ref,
                caller_invocation_ref: &call.invocation_ref,
                host_call_id: &call.call_id,
                nested_invocation_ref: &invocation_ref,
                caller_actor_ref: &requesting_actor_ref,
                target_actor_ref: &route.actor_ref,
                activation: &input.activation,
                input: input.input,
                text,
            },
            request,
            invocation,
        )?;
        let record = match started {
            StartInvocationOutcome::Started(record) | StartInvocationOutcome::Existing(record) => {
                record
            }
        };
        self.sync_archive_reservations(&mut state)?;
        if let Some(owner) = owner {
            state
                .host_local_invocation_owners
                .insert(record.invocation_ref.clone(), owner);
        }
        Self::enqueue_with_current_worker(&state.admission, &record, queue);
        Ok(AdapterHostCallOutcome::Returned(serde_json::to_value(
            StartActionResult::admitted(record.invocation_ref),
        )?))
    }

    fn enqueue_with_current_worker(
        admission: &ConversationAdmissionStore,
        record: &ActionInvocationRecord,
        queue: &Rc<RefCell<VecDeque<String>>>,
    ) {
        if record.is_runnable() && !independently_scheduled(admission, record) {
            queue.borrow_mut().push_back(record.invocation_ref.clone());
        }
    }

    fn retained_host_result(&self, call: &AdapterCall) -> CwrResult<Option<Result<Value, String>>> {
        let state = self.lock()?;
        let Some(execution_ref) = state.admission.execution_conversation_ref(&call.invocation_ref) else {
            return Ok(None);
        };
        let Some(execution) = state.admission.conversation_history(execution_ref)? else {
            return Ok(None);
        };
        let mut retained_call = false;
        let mut retained = None;
        for activity in &execution.activity {
            if let ConversationActivityEvent::HostCall {
                call_id,
                operation,
                input,
            } = &activity.event
            {
                if call_id == &call.call_id {
                    if retained_call {
                        return invalid(format!(
                            "host call {} was retained more than once",
                            call.call_id
                        ));
                    }
                    if operation != &call.operation || input != &call.input {
                        return invalid(format!(
                            "host call id {} was reused with different input",
                            call.call_id
                        ));
                    }
                    retained_call = true;
                }
                continue;
            }
            let ConversationActivityEvent::HostResult {
                call_id,
                result,
                error,
            } = &activity.event
            else {
                continue;
            };
            if call_id != &call.call_id {
                continue;
            }
            let candidate = match (result, error) {
                (Some(result), None) => Ok(result.clone()),
                (None, Some(error)) => Err(error.clone()),
                _ => return invalid("retained host result is malformed"),
            };
            if retained.replace(candidate).is_some() {
                return invalid(format!(
                    "host call {} has more than one result",
                    call.call_id
                ));
            }
        }
        if retained.is_some() && !retained_call {
            return invalid(format!(
                "host call {} has a result without a retained call",
                call.call_id
            ));
        }
        Ok(retained)
    }

}
