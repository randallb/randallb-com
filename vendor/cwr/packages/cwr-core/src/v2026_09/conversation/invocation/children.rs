impl ConversationAdmissionStore {
    /// Starts a child invocation and retains its first adapter invocation in a
    /// dedicated execution conversation in the same atomic state update.
    ///
    /// Replaying the same `(caller_invocation_ref, call_id)` returns the
    /// original invocation without creating another execution conversation.
    pub fn start_child_invocation(
        &mut self,
        request: StartActionInvocation,
        source_message_ref: &str,
        mut invocation: AdapterInvocation,
    ) -> CwrResult<StartInvocationOutcome> {
        validate_child_start(&self.state, &request, source_message_ref, &invocation, false)?;
        let execution_conversation_ref = self.new_ref("conversations");
        let activity_ref = self.new_ref("activities");
        if invocation
            .thread
            .as_object()
            .is_some_and(|thread| thread.is_empty())
        {
            let conversation = request
                .conversation_ref
                .as_deref()
                .and_then(|reference| self.conversation(reference))
                .ok_or_else(|| invalid_error("child context conversation is unavailable"))?;
            invocation.thread = conversation_thread(conversation);
        }
        super::working::check_child(&self.state, &invocation, &request.target_actor_ref,
            request.actor_integration_ref.as_deref().unwrap_or(""))?;
        let invocation_snapshot = context::invocation_snapshot(&invocation)?;
        let request_for_stage = request.clone();
        let source_message_ref = source_message_ref.to_owned();
        self.transact_invocations(move |state| {
            stage_child_invocation_in_state(
                state,
                request_for_stage,
                source_message_ref,
                invocation_snapshot,
                execution_conversation_ref,
                activity_ref,
            )
        })
    }

    /// Atomically retains a lifecycle host call, its visible action-call
    /// message and receipt, and the child invocation's first adapter snapshot.
    /// A failed state replacement exposes none of those records in memory.
    pub fn start_child_from_host_action_call(
        &mut self,
        host_call: &AdapterCall,
        action_call: ConversationHostActionCall<'_>,
        request: StartActionInvocation,
        invocation: AdapterInvocation,
    ) -> CwrResult<(ConversationMessage, StartInvocationOutcome)> {
        self.start_child_from_host_call(host_call, action_call, request, invocation, false)
    }

    /// Gateway execution is retained as a child call without making the
    /// transport handler a recipient or participant in the source conversation.
    pub fn start_gateway_child_from_host_call(
        &mut self,
        host_call: &AdapterCall,
        action_call: ConversationHostActionCall<'_>,
        request: StartActionInvocation,
        invocation: AdapterInvocation,
    ) -> CwrResult<(ConversationMessage, StartInvocationOutcome)> {
        self.start_child_from_host_call(host_call, action_call, request, invocation, true)
    }

    fn start_child_from_host_call(
        &mut self,
        host_call: &AdapterCall,
        action_call: ConversationHostActionCall<'_>,
        mut request: StartActionInvocation,
        mut invocation: AdapterInvocation,
        gateway: bool,
    ) -> CwrResult<(ConversationMessage, StartInvocationOutcome)> {
        if host_call.invocation_ref != action_call.caller_invocation_ref
            || host_call.call_id != action_call.host_call_id
        {
            return invalid("host action call does not match its adapter call");
        }
        if request.invocation_ref != action_call.nested_invocation_ref {
            return invalid("host action call does not match its child invocation ref");
        }

        let host_call = host_call.clone();
        let action_conversation_ref = action_call.conversation_ref.to_owned();
        let action_caller_invocation_ref = action_call.caller_invocation_ref.to_owned();
        let action_host_call_id = action_call.host_call_id.to_owned();
        let action_nested_invocation_ref = action_call.nested_invocation_ref.to_owned();
        let action_caller_actor_ref = action_call.caller_actor_ref.to_owned();
        let action_target_actor_ref = action_call.target_actor_ref.to_owned();
        let action_activation = action_call.activation.to_owned();
        let action_input = action_call.input;
        let action_text = action_call.text;
        let message_ref = self.new_ref("messages");
        if request.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
            request.input = json!({"source_message_ref": message_ref});
            invocation.input = request.input.clone();
        }
        let host_local = invocation.adapter_params.get("upstream_route").is_some();
        let execution_conversation_ref = self.new_ref("conversations");
        let turn_activity_ref = self.new_ref("activities");
        let request_for_stage = request.clone();
        self.transact_invocations(move |state| {
            retain_host_call_in_state(state, &host_call, request_for_stage.admitted_at_unix_ms)?;

            let retained_receipt = retained_host_action_receipt_in_state(
                state,
                &action_caller_invocation_ref,
                &action_host_call_id,
            )?;
            let action_message = if let Some((nested_invocation_ref, call_message_ref)) =
                retained_receipt
            {
                if nested_invocation_ref != action_nested_invocation_ref {
                    return invalid("retained host-action receipt points at another child");
                }
                state
                    .conversations
                    .iter()
                    .find(|conversation| conversation.conversation_ref == action_conversation_ref)
                    .and_then(|conversation| {
                        conversation
                            .messages
                            .iter()
                            .find(|message| message.message_ref == call_message_ref)
                    })
                    .cloned()
                    .ok_or_else(|| {
                        invalid_error("retained host-action receipt has no action-call message")
                    })?
            } else {
                let mut message = push_action_message_in_state(
                    state,
                    message_ref,
                    &action_conversation_ref,
                    &action_caller_actor_ref,
                    &action_target_actor_ref,
                    action_text,
                    None,
                    ConversationActionEvent::Call {
                        activation: action_activation,
                        input: action_input,
                    },
                )?;
                if gateway {
                    message.addressed_actor_refs.clear();
                    let conversation = state.conversations.find_mut(|c| c.conversation_ref == action_conversation_ref)
                        .ok_or_else(|| invalid_error("gateway call conversation is unavailable"))?;
                    let retained = conversation.messages.iter_mut().find(|m| m.message_ref == message.message_ref)
                        .ok_or_else(|| invalid_error("gateway call message was not retained"))?;
                    retained.addressed_actor_refs.clear();
                    conversation.participant_actor_refs.retain(|actor| actor != &action_target_actor_ref);
                }
                if request_for_stage.action_name == RESPOND_TO_MESSAGE_ACTION_NAME {
                    message.action = None;
                    let retained = state
                        .conversations
                        .find_mut(|c| c.conversation_ref == action_conversation_ref)
                        .and_then(|c| {
                            c.messages
                                .iter_mut()
                                .find(|m| m.message_ref == message.message_ref)
                        })
                        .ok_or_else(|| invalid_error("message call was not retained"))?;
                    retained.action = None;
                }
                push_execution_activity_in_state(
                    state,
                    &action_caller_invocation_ref,
                    ConversationActivityEvent::HostActionStarted {
                        call_id: action_host_call_id,
                        nested_invocation_ref: action_nested_invocation_ref,
                        call_message_ref: message.message_ref.clone(),
                    },
                    request_for_stage.admitted_at_unix_ms,
                )?;
                message
            };

            if host_local {
                if let Some(conversation) = state.conversations.find_mut(|conversation| conversation.conversation_ref == action_conversation_ref) {
                    conversation.participant_actor_refs.retain(|actor| actor != &action_target_actor_ref);
                }
            }
            let conversation = state
                .conversations
                .iter()
                .find(|conversation| conversation.conversation_ref == action_conversation_ref)
                .ok_or_else(|| invalid_error("child invocation conversation is unavailable"))?;
            invocation.thread = conversation_thread(conversation);
            validate_child_start(
                state,
                &request_for_stage,
                &action_message.message_ref,
                &invocation,
                gateway,
            )?;
            super::working::check_child(state, &invocation, &request_for_stage.target_actor_ref,
                request_for_stage.actor_integration_ref.as_deref().unwrap_or(""))?;
            let invocation_snapshot = context::invocation_snapshot(&invocation)?;
            let outcome = stage_child_invocation_in_state(
                state,
                request_for_stage,
                action_message.message_ref.clone(),
                invocation_snapshot,
                execution_conversation_ref,
                turn_activity_ref,
            )?;
            Ok((action_message, outcome))
        })
    }

    /// Retains the adapter call and its explicit host result together. This is
    /// used for both successful completions and durable validation failures so
    /// replay cannot observe a call without its settled outcome.
    pub fn retain_host_call_result(
        &mut self,
        host_call: &AdapterCall,
        result: Option<Value>,
        error: Option<String>,
        occurred_at_unix_ms: u64,
    ) -> CwrResult<ConversationActivity> {
        if result.is_some() == error.is_some() {
            return invalid("host result must contain exactly one of result or error");
        }
        let host_call = host_call.clone();
        self.transact_invocations(move |state| {
            retain_host_call_in_state(state, &host_call, occurred_at_unix_ms)?;
            let execution = execution_conversation_in_state(state, &host_call.invocation_ref)?;
            let mut retained = execution.activity.iter().filter(|activity| {
                matches!(
                    &activity.event,
                    ConversationActivityEvent::HostResult { call_id, .. }
                        if call_id == &host_call.call_id
                )
            });
            if let Some(activity) = retained.next() {
                if retained.next().is_some() {
                    return invalid(format!(
                        "host call {} retained more than one result",
                        host_call.call_id
                    ));
                }
                let ConversationActivityEvent::HostResult {
                    result: retained_result,
                    error: retained_error,
                    ..
                } = &activity.event
                else {
                    unreachable!("host-result filter only returns host results")
                };
                if retained_result != &result || retained_error != &error {
                    return invalid(format!(
                        "host call {} already has a different result",
                        host_call.call_id
                    ));
                }
                return Ok(activity.clone());
            }
            push_execution_activity_in_state(
                state,
                &host_call.invocation_ref,
                ConversationActivityEvent::HostResult {
                    call_id: host_call.call_id.clone(),
                    result,
                    error,
                },
                occurred_at_unix_ms,
            )
        })
    }
}
fn validate_child_start(
    state: &ConversationRuntimeState,
    request: &StartActionInvocation,
    source_message_ref: &str,
    invocation: &AdapterInvocation,
    gateway: bool,
) -> CwrResult<()> {
    if !matches!(&request.origin, InvocationOrigin::HostCall { .. }) {
        return invalid("child invocation must originate from a host call");
    }
    if source_message_ref.trim().is_empty() {
        return invalid("child invocation source message ref must not be blank");
    }
    if invocation.invocation_ref != request.invocation_ref
        || invocation.action_name != request.action_name
        || invocation.input != request.input
    {
        return invalid("child adapter invocation does not match its start request");
    }
    let conversation_ref = request
        .conversation_ref
        .as_deref()
        .ok_or_else(|| invalid_error("child invocation has no conversation ref"))?;
    let message = state
        .conversations
        .iter()
        .find(|conversation| conversation.conversation_ref == conversation_ref)
        .and_then(|conversation| {
            conversation
                .messages
                .iter()
                .find(|message| message.message_ref == source_message_ref)
        })
        .ok_or_else(|| invalid_error("child invocation source message is unavailable"))?;
    if message.author_actor_ref != request.requester_actor_ref
        || (!gateway && !message
            .addressed_actor_refs
            .contains(&request.target_actor_ref))
        || (gateway && !message.addressed_actor_refs.is_empty())
    {
        return invalid("child invocation source message actors do not match its request");
    }
    Ok(())
}

fn retain_host_call_in_state(
    state: &mut ConversationRuntimeState,
    host_call: &AdapterCall,
    occurred_at_unix_ms: u64,
) -> CwrResult<()> {
    let execution = execution_conversation_in_state(state, &host_call.invocation_ref)?;
    let mut matching_calls = execution.activity.iter().filter_map(|activity| {
        let ConversationActivityEvent::HostCall {
            call_id,
            operation,
            input,
        } = &activity.event
        else {
            return None;
        };
        (call_id == &host_call.call_id).then_some((operation, input))
    });
    if let Some((operation, input)) = matching_calls.next() {
        if operation != &host_call.operation || input != &host_call.input {
            return invalid(format!(
                "host call id {} was reused with different input",
                host_call.call_id
            ));
        }
        if matching_calls.next().is_some() {
            return invalid(format!(
                "host call {} was retained more than once",
                host_call.call_id
            ));
        }
        return Ok(());
    }
    push_execution_activity_in_state(
        state,
        &host_call.invocation_ref,
        ConversationActivityEvent::HostCall {
            call_id: host_call.call_id.clone(),
            operation: host_call.operation.clone(),
            input: host_call.input.clone(),
        },
        occurred_at_unix_ms,
    )?;
    Ok(())
}

fn retained_host_action_receipt_in_state(
    state: &ConversationRuntimeState,
    caller_invocation_ref: &str,
    call_id: &str,
) -> CwrResult<Option<(String, String)>> {
    let execution = execution_conversation_in_state(state, caller_invocation_ref)?;
    let mut receipt = None;
    for activity in &execution.activity {
        let ConversationActivityEvent::HostActionStarted {
            call_id: retained_call_id,
            nested_invocation_ref,
            call_message_ref,
        } = &activity.event
        else {
            continue;
        };
        if retained_call_id != call_id {
            continue;
        }
        if receipt
            .replace((nested_invocation_ref.clone(), call_message_ref.clone()))
            .is_some()
        {
            return invalid(format!(
                "host call {call_id} retained more than one started action"
            ));
        }
    }
    Ok(receipt)
}

fn stage_child_invocation_in_state(
    state: &mut ConversationRuntimeState,
    request: StartActionInvocation,
    source_message_ref: String,
    invocation_snapshot: Value,
    execution_conversation_ref: String,
    activity_ref: String,
) -> CwrResult<StartInvocationOutcome> {
    let outcome = state.invocation_ledger.start(request.clone())?;
    match &outcome {
        StartInvocationOutcome::Existing(existing) => {
            require_execution_snapshot(state, &existing.invocation_ref)?;
        }
        StartInvocationOutcome::Started(started) => {
            let conversation_ref = request
                .conversation_ref
                .clone()
                .ok_or_else(|| invalid_error("child invocation has no conversation ref"))?;
            state.conversations.push(RetainedConversation {
                owner_context_ref: state.conversations.iter().find(|parent| parent.conversation_ref == conversation_ref).and_then(|parent| parent.owner_context_ref.clone()),
                conversation_ref: execution_conversation_ref.clone(),
                title: None,
                archived: false,
                sidebar_eligible: None,
                parent_conversation_ref: Some(conversation_ref),
                participant_actor_refs: vec![request.target_actor_ref.clone()],
                messages: Vec::new(),
                timeline_connection_ref: None,
                activity: vec![ConversationActivity {
                    activity_ref,
                    sequence: 1,
                    occurred_at_unix_ms: request.admitted_at_unix_ms,
                    event: ConversationActivityEvent::TurnStarted {
                        invocation_ref: started.invocation_ref.clone(),
                        source_message_ref,
                        invocation: Some(invocation_snapshot),
                    },
                }],
            });
            if state
                .invocation_execution_conversation_refs
                .insert(started.invocation_ref.clone(), execution_conversation_ref)
                .is_some()
            {
                return invalid(format!(
                    "invocation {} already has an execution conversation",
                    started.invocation_ref
                ));
            }
        }
    }
    Ok(outcome)
}
