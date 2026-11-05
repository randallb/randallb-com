use super::*;
mod publications;
mod timelines;
mod tools;
use publications::{projected_publication, publication_changes};
use timelines::timeline_changes;
use tools::tool_changes;
fn record(
    changes: &mut Changes,
    reference: &str,
    kind: RecordKind,
    value: &impl Serialize,
    owner: Option<&str>,
) -> CwrResult<()> {
    properties(
        changes,
        reference,
        kind,
        json!({"record": serde_json::to_value(value)?}),
        owner,
    );
    Ok(())
}
fn properties(
    changes: &mut Changes,
    reference: &str,
    kind: RecordKind,
    properties: Value,
    owner: Option<&str>,
) {
    changes.records.push(WriteRecord {
        reference: reference.into(),
        kind,
        properties,
        owner_ref: owner.map(str::to_owned),
    });
}
fn remove(changes: &mut Changes, reference: &str, kind: RecordKind) {
    changes.remove_records.push(RemoveRecord {
        reference: reference.into(),
        kind,
    });
}
fn member(
    changes: &mut Changes,
    source: &str,
    kind: MembershipKind,
    target: &str,
    position: usize,
    properties: Value,
) -> CwrResult<()> {
    changes.memberships.push(MembershipChange::Upsert {
        source_ref: source.into(),
        kind,
        member: Member {
            target_ref: target.into(),
            position: i64::try_from(position)?,
            properties,
        },
    });
    Ok(())
}
fn replace(
    changes: &mut Changes,
    source: &str,
    kind: MembershipKind,
    targets: impl IntoIterator<Item = String>,
) {
    changes.memberships.push(MembershipChange::Replace {
        membership: Membership {
            source_ref: source.into(),
            kind,
            members: targets
                .into_iter()
                .enumerate()
                .map(|(position, target_ref)| Member {
                    target_ref,
                    position: position as i64,
                    properties: json!({}),
                })
                .collect(),
        },
    });
}
pub(super) fn changes(state: &ConversationRuntimeState) -> CwrResult<Changes> {
    let mut result = Changes::default();
    let root = format!("{}/cwpdb", state.runtime_root_ref);
    for (reference, old, new) in state.files.changes() {
        if old != new {
            if let Some(value) = new {
                record(&mut result, reference, RecordKind::File, value, None)?;
            }
        }
    }
    for (reference, old, new) in state.visualization_artifacts.changes() {
        if old == new {
            continue;
        }
        if let Some(value) = new {
            record(
                &mut result,
                reference,
                RecordKind::VisualizationArtifact,
                value,
                None,
            )?;
        } else {
            remove(&mut result, reference, RecordKind::VisualizationArtifact);
        }
    }
    for (position, old, new) in state.conversations.changes() {
        if old != Some(new) {
            conversation_changes(&mut result, &root, position, old, new)?;
        }
    }
    for (position, old, value) in state.admissions.changes() {
        if old == Some(value) {
            continue;
        }
        let reference = crate::cwpdb::derived_ref(
            "admission",
            &[&value.integration_ref, &value.provider_event_ref],
        );
        let mut payload = serde_json::to_value(value)?;
        payload
            .as_object_mut()
            .ok_or("admission is not an object")?
            .remove("source_input_fingerprint");
        properties(
            &mut result,
            &reference,
            RecordKind::Admission,
            json!({"record":payload,"source_input_fingerprint":value.source_input_fingerprint}),
            None,
        );
        member(
            &mut result,
            &root,
            MembershipKind::Admission,
            &reference,
            position,
            json!({}),
        )?;
    }
    for (reference, old, new) in state.invocation_ledger.changes() {
        if old == new {
            continue;
        }
        if let Some(value) = new {
            record(&mut result, reference, RecordKind::Invocation, value, None)?;
        } else {
            remove(&mut result, reference, RecordKind::Invocation);
        }
    }
    for (position, old, value) in state.invocation_ledger.changed_waits() {
        if old == Some(value) {
            continue;
        }
        let reference =
            crate::cwpdb::derived_ref("wait", &[&value.waiter_invocation_ref, &value.call_id]);
        record(&mut result, &reference, RecordKind::Wait, value, None)?;
        member(
            &mut result,
            &root,
            MembershipKind::Wait,
            &reference,
            position,
            json!({}),
        )?;
    }
    for (reference, old, new) in state.invocation_execution_conversation_refs.changes() {
        if old != new {
            replace(
                &mut result,
                reference,
                MembershipKind::Execution,
                new.into_iter().cloned(),
            );
        }
    }
    for (conversation, old, new) in state.upstream_participant_versions.changes() {
        if old == new {
            continue;
        }
        result.memberships.push(MembershipChange::Replace {
            membership: Membership {
                source_ref: conversation.clone(),
                kind: MembershipKind::ParticipantVersion,
                members: new
                    .into_iter()
                    .flat_map(|actors| actors.iter())
                    .map(|(actor, version)| Member {
                        target_ref: actor.clone(),
                        position: 0,
                        properties: json!({"coworker_ref":version}),
                    })
                    .collect(),
            },
        });
    }
    for (reference, old, new) in state.streaming_messages.changes() {
        if old == new {
            continue;
        }
        if let Some(value) = new {
            properties(
                &mut result,
                reference,
                RecordKind::Message,
                json!({"streaming":value}),
                None,
            );
        } else if !state.conversations.iter().any(|conversation| {
            conversation
                .messages
                .iter()
                .any(|message| &message.message_ref == reference)
        }) {
            remove(&mut result, reference, RecordKind::Message);
        }
    }
    if state
        .streaming_runs
        .changes()
        .any(|(_, old, new)| old != new)
    {
        result.streaming_runs = Some(serde_json::to_value(&state.streaming_runs)?);
    }
    tool_changes(&mut result, state)?;
    timeline_changes(&mut result, state)?;
    if state.timeline_store.iter().next().is_some() {
        result.state_version = Some(EXPLICIT_TIMELINE_STATE_VERSION.into());
    }
    for (reference, old, new) in state.protected_result_children.changes() {
        if old == new {
            continue;
        }
        let value = serde_json::to_value(new.cloned().unwrap_or_default())?;
        if let Some(record) = result
            .records
            .iter_mut()
            .find(|record| &record.reference == reference)
        {
            record.properties["protected_result_children"] = value;
        } else {
            let record = state
                .invocation_ledger
                .invocation(reference)
                .ok_or("protected result root unavailable")?;
            properties(
                &mut result,
                reference,
                RecordKind::Invocation,
                json!({"record":record,"protected_result_children":value}),
                None,
            );
        }
    }
    publication_changes(&mut result, state)?;
    Ok(result)
}

fn conversation_changes(
    result: &mut Changes,
    root: &str,
    position: usize,
    old: Option<&RetainedConversation>,
    new: &RetainedConversation,
) -> CwrResult<()> {
    let reference = &new.conversation_ref;
    properties(
        result,
        reference,
        RecordKind::Conversation,
        json!({"owner_context_ref":new.owner_context_ref,"sidebar_eligible":new.sidebar_eligible,"title":new.title,
        "last_message_ref":new.messages.last().map(|message| &message.message_ref),
        "last_message_preview":new.messages.last().map(|message|message.nonvisual_text().chars().take(240).collect::<String>())}),
        Some(reference),
    );
    member(
        result,
        root,
        MembershipKind::Conversation,
        reference,
        position,
        json!({}),
    )?;
    replace(
        result,
        reference,
        MembershipKind::Parent,
        new.parent_conversation_ref.iter().cloned(),
    );
    replace(
        result,
        reference,
        MembershipKind::Participant,
        new.participant_actor_refs.clone(),
    );
    replace(
        result,
        reference,
        MembershipKind::Archived,
        new.archived.then(|| root.to_owned()),
    );
    let old_messages = old
        .into_iter()
        .flat_map(|old| old.messages.iter().enumerate())
        .map(|(index, message)| (message.message_ref.as_str(), (index, message)))
        .collect::<BTreeMap<_, _>>();
    for reference in old_messages.keys().filter(|reference| {
        !new.messages
            .iter()
            .any(|message| &message.message_ref == *reference)
    }) {
        remove(result, reference, RecordKind::Message);
    }
    for (position, message) in new.messages.iter().enumerate() {
        if old_messages
            .get(message.message_ref.as_str())
            .is_some_and(|(index, old)| *index == position && *old == message)
        {
            continue;
        }
        record(
            result,
            &message.message_ref,
            RecordKind::Message,
            &message.clone_without_visualizations(),
            Some(reference),
        )?;
        member(
            result,
            reference,
            MembershipKind::Message,
            &message.message_ref,
            position,
            json!({"visualizations":message.visualizations}),
        )?;
    }
    let old_activity = old
        .into_iter()
        .flat_map(|old| old.activity.iter().enumerate())
        .map(|(index, item)| (item.activity_ref.as_str(), (index, item)))
        .collect::<BTreeMap<_, _>>();
    for (reference, (_, activity)) in &old_activity {
        if !new
            .activity
            .iter()
            .any(|activity| activity.activity_ref == *reference)
        {
            remove(result, reference, activity_kind(activity).0);
        }
    }
    for (position, activity) in new.activity.iter().enumerate() {
        if old_activity
            .get(activity.activity_ref.as_str())
            .is_some_and(|(index, old)| *index == position && *old == activity)
        {
            continue;
        }
        let (kind, membership) = activity_kind(activity);
        record(
            result,
            &activity.activity_ref,
            kind,
            activity,
            Some(reference),
        )?;
        member(
            result,
            reference,
            membership,
            &activity.activity_ref,
            position,
            json!({}),
        )?;
    }
    Ok(())
}
pub(super) fn activity_kind(activity: &ConversationActivity) -> (RecordKind, MembershipKind) {
    if matches!(
        activity.event,
        ConversationActivityEvent::ConversationForkedFrom { .. }
            | ConversationActivityEvent::MessageWorkAssociation { .. }
    ) {
        (
            RecordKind::ConversationTransferMetadata,
            MembershipKind::TransferMetadata,
        )
    } else {
        (RecordKind::Activity, MembershipKind::Activity)
    }
}

pub(super) fn projected_publications(
    state: &ConversationRuntimeState,
) -> CwrResult<Vec<PublicationSuffix>> {
    state
        .conversations
        .iter()
        .filter(|root| root.parent_conversation_ref.is_none())
        .map(|root| projected_publication(state, root))
        .collect()
}

pub(super) fn repair_conversation_headers(changes: &mut Changes, state: &ConversationRuntimeState) {
    for conversation in state.conversations.iter() {
        if changes
            .records
            .iter()
            .any(|record| record.reference == conversation.conversation_ref)
        {
            continue;
        }
        properties(
            changes,
            &conversation.conversation_ref,
            RecordKind::Conversation,
            json!({
                "owner_context_ref":conversation.owner_context_ref,"sidebar_eligible":conversation.sidebar_eligible,"title":conversation.title,
                "last_message_ref":conversation.messages.last().map(|message|&message.message_ref),
                "last_message_preview":conversation.messages.last().map(|message|message.nonvisual_text().chars().take(240).collect::<String>())
            }),
            Some(&conversation.conversation_ref),
        );
    }
}
