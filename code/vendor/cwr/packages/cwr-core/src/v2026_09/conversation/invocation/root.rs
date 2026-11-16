pub(super) fn stage_root_invocation(
    state: &mut ConversationRuntimeState,
    prepared: &PreparedConversationAdmission,
) -> CwrResult<()> {
    let Some(invocation) = prepared.invocation.as_ref() else {
        if prepared.record.invocation_ref.is_some()
            || prepared.record.execution_conversation_ref.is_some()
        {
            return invalid("non-executable admission has invocation state");
        }
        return Ok(());
    };
    let invocation_ref = prepared
        .record
        .invocation_ref
        .as_deref()
        .ok_or_else(|| invalid_error("executable admission has no invocation ref"))?;
    if invocation.invocation_ref != invocation_ref {
        return invalid("prepared adapter invocation ref does not match its admission");
    }
    let target_actor_ref = prepared
        .record
        .target_actor_ref
        .clone()
        .ok_or_else(|| invalid_error("executable admission has no target actor"))?;
    let actor_integration_ref = prepared
        .record
        .actor_integration_ref
        .clone()
        .ok_or_else(|| invalid_error("executable admission has no actor integration"))?;
    let source = prepared
        .conversation
        .messages
        .iter()
        .find(|message| message.message_ref == prepared.record.source_message_ref)
        .ok_or_else(|| invalid_error("prepared admission source message is unavailable"))?;
    let execution = prepared
        .execution_conversation
        .as_ref()
        .ok_or_else(|| invalid_error("executable admission has no execution conversation"))?;
    let execution_ref = prepared
        .record
        .execution_conversation_ref
        .as_deref()
        .ok_or_else(|| invalid_error("executable admission has no execution conversation ref"))?;
    if execution.conversation_ref != execution_ref {
        return invalid("prepared execution conversation ref does not match its admission");
    }
    let invocation_snapshot = context::invocation_snapshot(invocation)?;
    let admitted_at_unix_ms = execution
        .activity
        .iter()
        .find_map(|activity| match &activity.event {
            ConversationActivityEvent::TurnStarted {
                invocation_ref: started_ref,
                source_message_ref,
                invocation: snapshot,
            } if started_ref == invocation_ref
                && source_message_ref == &prepared.record.source_message_ref
                && snapshot
                    .as_ref()
                    .and_then(|value| {
                        serde_json::from_value::<AdapterInvocation>(value.clone()).ok()
                    })
                    .and_then(|value| context::invocation_snapshot(&value).ok())
                    .as_ref()
                    == Some(&invocation_snapshot) =>
            {
                Some(activity.occurred_at_unix_ms)
            }
            _ => None,
        })
        .ok_or_else(|| {
            invalid_error("prepared execution conversation has no matching turn start")
        })?;
    let outcome = state.invocation_ledger.start(StartActionInvocation {
        invocation_ref: invocation_ref.into(),
        origin: InvocationOrigin::Ingress {
            integration_ref: prepared.record.integration_ref.clone(),
            provider_event_ref: prepared.record.provider_event_ref.clone(),
        },
        requester_actor_ref: source.author_actor_ref.clone(),
        target_actor_ref,
        conversation_ref: Some(prepared.record.conversation_ref.clone()),
        actor_integration_ref: Some(actor_integration_ref),
        action_name: invocation.action_name.clone(),
        input: invocation.input.clone(),
        parent_invocation_ref: None,
        admitted_at_unix_ms,
    })?;
    if prepared.record.adapter_ref.as_deref() == Some(crate::adapter::PERSON_ADAPTER_REF) {
        state.invocation_ledger.await_person(invocation_ref)?;
    }
    if !matches!(outcome, StartInvocationOutcome::Started(_)) {
        return invalid("root invocation exists without its ingress admission");
    }
    if state
        .invocation_execution_conversation_refs
        .insert(invocation_ref.into(), execution_ref.into())
        .is_some()
    {
        return invalid(format!(
            "invocation {invocation_ref} already has an execution conversation"
        ));
    }
    Ok(())
}

pub(super) fn validate_invocation_execution_links(
    state: &ConversationRuntimeState,
) -> CwrResult<()> {
    validate_execution_links_selected(state, None)
}

pub(super) fn validate_recovery_execution_links(state: &ConversationRuntimeState) -> CwrResult<()> {
    let mut selected = state
        .invocation_ledger
        .invocations()
        .filter(|record| !record.state.is_terminal() || record.attempt_active)
        .map(|record| record.invocation_ref.clone())
        .collect::<BTreeSet<_>>();
    let mut pending = selected.iter().cloned().collect::<Vec<_>>();
    while let Some(reference) = pending.pop() {
        if let Some(parent) = state
            .invocation_ledger
            .invocation(&reference)
            .and_then(|record| record.parent_invocation_ref.as_ref())
        {
            if selected.insert(parent.clone()) {
                pending.push(parent.clone());
            }
        }
    }
    // Live-root work projection also reads its completed direct executions.
    // They were hydrated at open, so validate their ledger links and activity
    // before they can contribute public status signals.
    selected.extend(
        state
            .invocation_execution_conversation_refs
            .iter()
            .filter(|(_, execution)| !state.cold_conversation_refs.contains(*execution))
            .map(|(invocation, _)| invocation.clone()),
    );
    validate_execution_links_selected(state, Some(&selected))
}

pub(super) fn validate_changed_execution_links(state: &ConversationRuntimeState) -> CwrResult<()> {
    let mut refs = state
        .invocation_ledger
        .changes()
        .map(|(key, _, _)| key.clone())
        .collect::<BTreeSet<_>>();
    for (_, old, new) in state.admissions.changes() {
        refs.extend(old.and_then(|record| record.invocation_ref.clone()));
        refs.extend(new.invocation_ref.clone());
    }
    for (_, old, new) in state.conversations.changes() {
        for conversation in old.into_iter().chain(std::iter::once(new)) {
            refs.extend(conversation.activity.iter().filter_map(
                |activity| match &activity.event {
                    ConversationActivityEvent::TurnStarted { invocation_ref, .. } => {
                        Some(invocation_ref.clone())
                    }
                    _ => None,
                },
            ));
        }
    }
    for (key, _, new) in state.invocation_execution_conversation_refs.changes() {
        refs.insert(key.clone());
        let Some(execution) = new else {
            return invalid("execution conversation mappings cannot be removed");
        };
        if state
            .invocation_execution_conversation_refs
            .iter()
            .any(|(other, value)| other != key && value == execution)
        {
            return invalid("execution conversation belongs to more than one invocation");
        }
    }
    validate_execution_links_selected(state, Some(&refs))
}

fn validate_execution_links_selected(
    state: &ConversationRuntimeState,
    selected: Option<&BTreeSet<String>>,
) -> CwrResult<()> {
    let mut execution_refs = BTreeSet::new();
    let invocations: Box<dyn Iterator<Item = &ActionInvocationRecord> + '_> = match selected {
        Some(refs) => Box::new(
            refs.iter()
                .filter_map(|key| state.invocation_ledger.invocation(key)),
        ),
        None => Box::new(state.invocation_ledger.invocations()),
    };
    for invocation in invocations {
        if invocation.forked_from_invocation_ref.is_some() { continue; }
        let execution_ref = state
            .invocation_execution_conversation_refs
            .get(&invocation.invocation_ref)
            .or_else(|| {
                state
                    .admissions
                    .iter()
                    .find(|record| {
                        record.invocation_ref.as_deref() == Some(&invocation.invocation_ref)
                    })
                    .and_then(|record| record.execution_conversation_ref.as_ref())
            })
            .ok_or_else(|| {
                invalid_error(format!(
                    "invocation {} has no execution conversation mapping",
                    invocation.invocation_ref
                ))
            })?;
        if !execution_refs.insert(execution_ref.clone()) {
            return invalid(format!(
                "execution conversation {execution_ref} belongs to more than one invocation"
            ));
        }
        let execution = require_execution_snapshot(state, &invocation.invocation_ref)?;
        validate_execution_activity_statuses(invocation, execution)?;
        if let InvocationOrigin::Ingress {
            integration_ref,
            provider_event_ref,
        } = &invocation.origin
        {
            let linked = state.admissions.iter().any(|record| {
                record.integration_ref == *integration_ref
                    && record.provider_event_ref == *provider_event_ref
                    && record.invocation_ref.as_deref() == Some(&invocation.invocation_ref)
            });
            if !linked {
                return invalid(format!(
                    "ingress invocation {} has no matching admission",
                    invocation.invocation_ref
                ));
            }
        }
    }
    for (invocation_ref, execution_ref) in state
        .invocation_execution_conversation_refs
        .iter()
        .filter(|(key, _)| selected.is_none_or(|refs| refs.contains(*key)))
    {
        if state.invocation_ledger.invocation(invocation_ref).is_none() {
            return invalid(format!(
                "execution conversation {execution_ref} maps unknown invocation {invocation_ref}"
            ));
        }
    }
    Ok(())
}

fn validate_execution_activity_statuses(
    invocation: &ActionInvocationRecord,
    execution: &RetainedConversation,
) -> CwrResult<()> {
    let terminal_response = invocation.terminal_response();
    let progress_responses = invocation
        .responses
        .iter()
        .filter(|response| response.status.is_progress())
        .collect::<Vec<_>>();
    let mut next_progress_response = 0;
    let mut terminal_activity_count = 0;
    for activity in &execution.activity {
        let (status, expected_category) = match &activity.event {
            ConversationActivityEvent::Progress {
                status,
                summary,
                progress,
            } => {
                if !status.is_progress() {
                    return invalid(format!(
                        "invocation {} progress activity has non-progress status {}",
                        invocation.invocation_ref,
                        status.as_u16()
                    ));
                }
                let relative_index = progress_responses[next_progress_response..]
                    .iter()
                    .position(|response| {
                        response.status == *status
                            && response.summary == *summary
                            && response.progress == *progress
                    })
                    .ok_or_else(|| {
                        invalid_error(format!(
                            "invocation {} has progress activity without a retained response",
                            invocation.invocation_ref
                        ))
                    })?;
                let response_index = next_progress_response + relative_index;
                next_progress_response = response_index + 1;
                continue;
            }
            ConversationActivityEvent::TurnCompleted { status, .. } => {
                (*status, status.is_completed())
            }
            ConversationActivityEvent::TurnFailed { status, .. } => {
                (*status, status.is_blocked() || status.is_failed())
            }
            ConversationActivityEvent::TurnStopped { status, .. } => (*status, status.is_stopped()),
            _ => continue,
        };
        terminal_activity_count += 1;
        if !expected_category {
            return invalid(format!(
                "invocation {} terminal activity has status {} in the wrong category",
                invocation.invocation_ref,
                status.as_u16()
            ));
        }
        let response = terminal_response.ok_or_else(|| {
            invalid_error(format!(
                "invocation {} has terminal activity without a terminal response",
                invocation.invocation_ref
            ))
        })?;
        if status != response.status {
            return invalid(format!(
                "invocation {} terminal activity status {} does not match response status {}",
                invocation.invocation_ref,
                status.as_u16(),
                response.status.as_u16()
            ));
        }
    }
    if terminal_activity_count > 1 {
        return invalid(format!(
            "invocation {} has more than one terminal activity",
            invocation.invocation_ref
        ));
    }
    Ok(())
}
