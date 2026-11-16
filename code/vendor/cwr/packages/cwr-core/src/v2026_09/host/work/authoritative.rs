pub(crate) fn project_conversation_with_authoritative_work(
    conversation: &RetainedConversation,
    store: &ConversationAdmissionStore,
) -> RetainedConversation {
    let mut projected = project_conversation(conversation);
    projected
        .activity
        .extend(project_authoritative_work_activities(
            conversation,
            store.hydrated_conversations(),
            store.invocation_records(),
            store.invocation_waits(),
            |reference| store.hydrated_execution_conversation(reference),
            |reference| store.invocation_record(reference).is_some(),
        ));
    sort_work_activities(&mut projected.activity);
    projected
}

pub(crate) fn project_latest_terminal_root_work(
    store: &ConversationAdmissionStore,
) -> BTreeMap<String, Vec<ConversationActivity>> {
    let mut activity_by_conversation = BTreeMap::<String, Vec<ConversationActivity>>::new();
    // Admissions are the authoritative turn order. Do not infer which result is
    // newest from wall-clock timestamps or randomly generated references.
    for admission in store.admissions() {
        let Some(invocation_ref) = &admission.invocation_ref else {
            continue;
        };
        let Some(record) = store.invocation_record(invocation_ref) else {
            continue;
        };
        if record.conversation_ref.as_deref() != Some(&admission.conversation_ref)
            || record.parent_invocation_ref.is_some()
            || !record.state.is_terminal()
            || !matches!(record.origin, InvocationOrigin::Ingress { .. })
        {
            continue;
        }
        activity_by_conversation.insert(
            admission.conversation_ref.clone(),
            vec![completed_root_activity(
                record,
                admission.source_message_ref.clone(),
            )],
        );
    }
    activity_by_conversation
}

pub(crate) fn project_latest_terminal_root_work_for_conversation(
    store: &ConversationAdmissionStore,
    conversation_ref: &str,
) -> Vec<ConversationActivity> {
    store
        .admissions_for_conversation(conversation_ref)
        .rev()
        .find_map(|admission| {
            let invocation_ref = admission.invocation_ref.as_ref()?;
            let record = store.invocation_record(invocation_ref)?;
            (record.conversation_ref.as_deref() == Some(conversation_ref)
                && record.parent_invocation_ref.is_none()
                && record.state.is_terminal()
                && matches!(record.origin, InvocationOrigin::Ingress { .. }))
            .then(|| completed_root_activity(record, admission.source_message_ref.clone()))
        })
        .into_iter()
        .collect()
}

fn completed_root_activity(
    record: &crate::v2026_09::invocation::ActionInvocationRecord,
    source_message_ref: String,
) -> ConversationActivity {
    ConversationActivity {
        activity_ref: format!("{}/lifecycle", record.invocation_ref),
        sequence: 0,
        occurred_at_unix_ms: record.admitted_at_unix_ms,
        event: ConversationActivityEvent::WorkLifecycle {
            invocation_ref: record.invocation_ref.clone(),
            actor_ref: record.target_actor_ref.clone(),
            source_message_ref,
            parent_invocation_ref: None,
            awaited_by_invocation_ref: None,
            state: record.state,
            admitted_at_unix_ms: record.admitted_at_unix_ms,
            started_at_unix_ms: record.started_at_unix_ms,
            stop_requested_at_unix_ms: record.stop_requested_at_unix_ms,
            finished_at_unix_ms: record.finished_at_unix_ms,
        },
    }
}

pub(crate) fn project_conversation_metadata(
    conversation: &RetainedConversation,
    mut activity: Vec<ConversationActivity>,
    include_lineage: bool,
) -> RetainedConversation {
    if include_lineage {
        activity.extend(
            conversation
                .activity
                .iter()
                .filter(|item| {
                    matches!(
                        item.event,
                        ConversationActivityEvent::ConversationForkedFrom { .. }
                    )
                })
                .cloned(),
        );
    }
    RetainedConversation {
        owner_context_ref: conversation.owner_context_ref.clone(),
        conversation_ref: conversation.conversation_ref.clone(),
        archived: conversation.archived,
        title: conversation.title.clone().or_else(|| {
            conversation
                .messages
                .iter()
                .find(|message| message.progress.is_none())
                .and_then(|message| {
                    let first_line = message.text.trim().lines().next()?.trim();
                    (!first_line.is_empty()).then(|| {
                        if first_line.chars().count() > 72 {
                            format!("{}…", first_line.chars().take(69).collect::<String>())
                        } else {
                            first_line.to_owned()
                        }
                    })
                })
        }),
        sidebar_eligible: Some(conversation.is_sidebar_eligible()),
        parent_conversation_ref: conversation.parent_conversation_ref.clone(),
        participant_actor_refs: conversation.participant_actor_refs.clone(),
        messages: Vec::new(),
        timeline_connection_ref: None,
        activity,
    }
}

pub(crate) fn project_full_conversation_metadata(
    conversation: &RetainedConversation,
) -> RetainedConversation {
    if conversation.messages.is_empty() {
        let activity = conversation
            .activity
            .iter()
            .rev()
            .find(|activity| {
                matches!(
                    activity.event,
                    ConversationActivityEvent::WorkLifecycle {
                        parent_invocation_ref: None,
                        state: ActionInvocationState::Completed,
                        ..
                    }
                )
            })
            .cloned()
            .into_iter()
            .collect();
        return project_conversation_metadata(conversation, activity, true);
    }
    let activity_by_source = conversation
        .activity
        .iter()
        .filter_map(|activity| match &activity.event {
            ConversationActivityEvent::WorkLifecycle {
                source_message_ref,
                parent_invocation_ref: None,
                state: ActionInvocationState::Completed,
                ..
            } => Some((source_message_ref.as_str(), activity)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let activity = conversation
        .messages
        .iter()
        .rev()
        .find_map(|message| activity_by_source.get(message.message_ref.as_str()))
        .map(|activity| (*activity).clone())
        .into_iter()
        .collect();
    project_conversation_metadata(conversation, activity, true)
}

pub(crate) fn project_host_conversation_metadata(
    store: &ConversationAdmissionStore,
    conversation_ref: &str,
) -> CwrResult<Option<RetainedConversation>> {
    let activity = project_latest_terminal_root_work_for_conversation(store, conversation_ref);
    Ok(store
        .conversation_metadata_source(conversation_ref)?
        .as_ref()
        .map(|conversation| project_conversation_metadata(conversation, activity, true)))
}

pub(crate) fn project_host_conversation_bootstrap(
    store: &ConversationAdmissionStore,
    include_lineage: bool,
) -> CwrResult<Vec<RetainedConversation>> {
    let mut terminal = project_latest_terminal_root_work(store);
    Ok(store
        .bootstrap_conversation_metadata_sources()?
        .iter()
        .map(|conversation| {
            let activity = terminal
                .remove(&conversation.conversation_ref)
                .unwrap_or_default();
            project_conversation_metadata(conversation, activity, include_lineage)
        })
        .collect())
}

/// The snapshot and SQLite projection use the same public lifecycle and wait
/// rules. Callers may select just the changed executions and invocation records;
/// lookups still see the whole ledger when resolving synchronous parents.
pub(crate) fn project_authoritative_work_activities<'a>(
    conversation: &RetainedConversation,
    executions: impl Iterator<Item = &'a RetainedConversation>,
    records: impl Iterator<Item = &'a crate::v2026_09::invocation::ActionInvocationRecord>,
    waits: &[crate::v2026_09::invocation::InvocationWait],
    execution_conversation: impl Fn(&str) -> Option<&'a RetainedConversation>,
    invocation_exists: impl Fn(&str) -> bool,
) -> Vec<ConversationActivity> {
    let records = records
        .filter(|record| record.conversation_ref.as_deref() == Some(&conversation.conversation_ref))
        .collect::<Vec<_>>();
    let mut relations = records
        .iter()
        .map(|record| {
            (
                record.invocation_ref.clone(),
                WorkInvocationRelation {
                    parent_invocation_ref: record.parent_invocation_ref.clone(),
                    wait: None,
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
    for wait in waits {
        if let Some(relation) = relations.get_mut(&wait.awaited_invocation_ref) {
            if relation.parent_invocation_ref.as_deref() == Some(&wait.waiter_invocation_ref) {
                relation.wait = Some(crate::v2026_09::conversation::WorkInvocationWait {
                    waiter_invocation_ref: wait.waiter_invocation_ref.clone(),
                    awaited_invocation_ref: wait.awaited_invocation_ref.clone(),
                    call_id: wait.call_id.clone(),
                    state: wait.state,
                    resume_after_activity_sequence: wait.resume_after_activity_sequence,
                });
            }
        }
    }
    let mut activity = project_execution_work(conversation, executions, &relations);
    for record in records {
        let source = execution_conversation(&record.invocation_ref).and_then(|execution| {
            execution
                .activity
                .iter()
                .find_map(|item| match &item.event {
                    ConversationActivityEvent::TurnStarted {
                        source_message_ref, ..
                    } => Some(source_message_ref.clone()),
                    _ => None,
                })
        });
        let Some(source_message_ref) = source else {
            continue;
        };
        activity.push(ConversationActivity {
            activity_ref: format!("{}/lifecycle", record.invocation_ref),
            sequence: 0,
            occurred_at_unix_ms: record.admitted_at_unix_ms,
            event: ConversationActivityEvent::WorkLifecycle {
                invocation_ref: record.invocation_ref.clone(),
                actor_ref: record.target_actor_ref.clone(),
                source_message_ref,
                parent_invocation_ref: record.parent_invocation_ref.clone(),
                awaited_by_invocation_ref: synchronous_parent_from(
                    record,
                    &execution_conversation,
                    &invocation_exists,
                )
                .map(|(parent, _)| parent.to_owned()),
                state: record.state,
                admitted_at_unix_ms: record.admitted_at_unix_ms,
                started_at_unix_ms: record.started_at_unix_ms,
                stop_requested_at_unix_ms: record.stop_requested_at_unix_ms,
                finished_at_unix_ms: record.finished_at_unix_ms,
            },
        });
    }
    sort_work_activities(&mut activity);
    activity
}

pub(crate) fn sort_work_activities(activity: &mut [ConversationActivity]) {
    activity.sort_by(|left, right| {
        (left.occurred_at_unix_ms, left.sequence, &left.activity_ref).cmp(&(
            right.occurred_at_unix_ms,
            right.sequence,
            &right.activity_ref,
        ))
    });
}
