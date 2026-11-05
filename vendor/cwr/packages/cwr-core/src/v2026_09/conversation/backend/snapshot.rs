//! Decode canonical records; no physical layout knowledge crosses this seam.
use super::*;
use serde::de::DeserializeOwned;

pub(super) fn decode_state(
    page: &SnapshotPage,
    runtime_root_ref: &str,
) -> CwrResult<ConversationRuntimeState> {
    if !page.complete || page.metadata["runtime_root_ref"] != runtime_root_ref {
        return invalid("incomplete or foreign conversation bootstrap");
    }
    let version = page.metadata["runtime_state_version"]
        .as_str()
        .ok_or("missing runtime state version")?;
    if version != STATE_VERSION && version != EXPLICIT_TIMELINE_STATE_VERSION {
        return invalid("unsupported CWR runtime state version");
    }
    for record in &page.records {
        if record.schema_ref != crate::cwpdb::schema_ref(record.kind.as_str()) {
            return invalid(format!(
                "unsupported record schema for {}",
                record.reference
            ));
        }
    }
    let records = page
        .records
        .iter()
        .map(|record| (record.reference.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    if records.len() != page.records.len() {
        return invalid("duplicate conversation bootstrap record");
    }
    let root = format!("{runtime_root_ref}/cwpdb");
    let mut conversations = Vec::new();
    for edge in edges(page, &root, "conversation") {
        let record = required(&records, &edge.target_ref, RecordKind::Conversation)?;
        let props = &record.properties;
        let mut conversation = RetainedConversation {
            conversation_ref: record.reference.clone(),
            owner_context_ref: optional_string(props, "owner_context_ref")?,
            title: optional_string(props, "title")?,
            sidebar_eligible: props
                .get("sidebar_eligible")
                .filter(|value| !value.is_null())
                .map(|value| {
                    value
                        .as_bool()
                        .ok_or_else(|| invalid_error("invalid sidebar eligibility"))
                })
                .transpose()?,
            parent_conversation_ref: one_target(page, &record.reference, "parent")?,
            participant_actor_refs: edges(page, &record.reference, "participant")
                .into_iter()
                .map(|edge| edge.target_ref.clone())
                .collect(),
            archived: !edges(page, &record.reference, "archived").is_empty(),
            timeline_connection_ref: one_target(
                page,
                &record.reference,
                "conversation_timeline_root",
            )?,
            messages: Vec::new(),
            activity: Vec::new(),
        };
        hydrate_conversation(page, &records, &mut conversation)?;
        conversations.push(conversation);
    }
    let admissions = edges(page, &root, "admission")
        .into_iter()
        .map(|edge| {
            let record = required(&records, &edge.target_ref, RecordKind::Admission)?;
            let mut admission: ConversationAdmissionRecord = payload(record)?;
            admission.source_input_fingerprint =
                optional_string(&record.properties, "source_input_fingerprint")?;
            Ok(admission)
        })
        .collect::<CwrResult<Vec<_>>>()?;
    let invocations = page
        .records
        .iter()
        .filter(|record| record.kind == RecordKind::Invocation)
        .map(|record| {
            Ok((
                record.reference.clone(),
                payload::<crate::v2026_09::invocation::ActionInvocationRecord>(record)?,
            ))
        })
        .collect::<CwrResult<BTreeMap<_, _>>>()?;
    let waits = edges(page, &root, "wait")
        .into_iter()
        .map(|edge| {
            payload::<crate::v2026_09::invocation::InvocationWait>(required(
                &records,
                &edge.target_ref,
                RecordKind::Wait,
            )?)
        })
        .collect::<CwrResult<Vec<_>>>()?;
    let invocation_ledger: ActionInvocationLedger =
        serde_json::from_value(json!({"invocations":invocations,"waits":waits}))?;
    let tool_operations = decode_tool_operations(page)?;
    let mut streaming_messages = BTreeMap::new();
    for record in page
        .records
        .iter()
        .filter(|record| record.kind == RecordKind::Message)
    {
        if let Some(streaming) = record.properties.get("streaming") {
            let message: ConversationStreamingMessage = serde_json::from_value(streaming.clone())?;
            if message.message.message_ref != record.reference {
                return invalid("streaming message identity mismatch");
            }
            streaming_messages.insert(record.reference.clone(), message);
        }
    }
    let mut participant_versions = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut execution = BTreeMap::new();
    for edge in &page.relationships {
        if edge.kind == "execution" {
            if execution
                .insert(edge.source_ref.clone(), edge.target_ref.clone())
                .is_some()
            {
                return invalid("multiple execution conversations");
            }
        } else if edge.kind == "participant_version" {
            let version = edge.properties["coworker_ref"]
                .as_str()
                .ok_or("invalid participant version")?;
            participant_versions
                .entry(edge.source_ref.clone())
                .or_default()
                .insert(edge.target_ref.clone(), version.into());
        }
    }
    let cold = conversations
        .iter()
        .filter(|conversation| {
            edges(page, &conversation.conversation_ref, "message").is_empty()
                && edges(page, &conversation.conversation_ref, "activity").is_empty()
        })
        .map(|conversation| conversation.conversation_ref.clone())
        .collect();
    Ok(ConversationRuntimeState {
        version: version.into(),
        runtime_root_ref: runtime_root_ref.into(),
        conversations: conversations.into(),
        cold_conversation_refs: cold,
        files: Default::default(),
        visualization_artifacts: Default::default(),
        admissions: admissions.into(),
        invocation_ledger,
        timeline_store: decode_timelines(page, &records)?.into(),
        streaming_messages: streaming_messages.into(),
        streaming_runs: serde_json::from_value(
            page.metadata
                .get("streaming_runs")
                .cloned()
                .unwrap_or_else(|| json!({})),
        )?,
        tool_attempts: payload_map::<tools::ToolAttempt>(page, RecordKind::ToolAttempt)?.into(),
        tool_operations: tool_operations.into(),
        invocation_execution_conversation_refs: execution.into(),
        upstream_participant_versions: participant_versions.into(),
        protected_result_children: page
            .records
            .iter()
            .filter(|record| record.kind == RecordKind::Invocation)
            .filter_map(|record| {
                record
                    .properties
                    .get("protected_result_children")
                    .map(|value| (record.reference.clone(), value.clone()))
            })
            .map(|(reference, value)| Ok((reference, serde_json::from_value(value)?)))
            .collect::<CwrResult<BTreeMap<_, _>>>()?
            .into(),
    })
}
fn decode_timelines(
    page: &SnapshotPage,
    records: &BTreeMap<&str, &Record>,
) -> CwrResult<BTreeMap<String, ConversationTimeline>> {
    let mut result = BTreeMap::new();
    for root in page
        .relationships
        .iter()
        .filter(|edge| edge.kind == "conversation_timeline_root")
    {
        let mut pending = vec![root.target_ref.clone()];
        let mut connections = Vec::new();
        let mut visited = std::collections::BTreeSet::new();
        while let Some(reference) = pending.pop() {
            if !visited.insert(reference.clone()) {
                return invalid("cycle or duplicate timeline connection");
            }
            let record = required(
                records,
                &reference,
                RecordKind::ConversationTimelineConnection,
            )?;
            let owner = if reference == root.target_ref {
                ConversationTimelineConnectionOwner::Conversation {
                    conversation_ref: root.source_ref.clone(),
                }
            } else {
                let owners = page
                    .relationships
                    .iter()
                    .filter(|edge| {
                        edge.kind == "conversation_timeline_child" && edge.target_ref == reference
                    })
                    .collect::<Vec<_>>();
                let [owner] = owners.as_slice() else {
                    return invalid("timeline child requires one owner");
                };
                ConversationTimelineConnectionOwner::TimelineItem {
                    item_ref: owner.source_ref.clone(),
                }
            };
            let mut items = Vec::new();
            for edge in edges(page, &reference, "conversation_timeline_item") {
                let item = required(
                    records,
                    &edge.target_ref,
                    RecordKind::ConversationTimelineItem,
                )?;
                let message = one_target(page, &item.reference, "conversation_timeline_message")?;
                let invocation =
                    one_target(page, &item.reference, "conversation_timeline_invocation")?;
                let content = match (message, invocation) {
                    (Some(message_ref), None) => {
                        ConversationTimelineContent::Message { message_ref }
                    }
                    (None, Some(invocation_ref)) => {
                        ConversationTimelineContent::Invocation { invocation_ref }
                    }
                    _ => return invalid("timeline item requires exactly one content target"),
                };
                let child = one_target(page, &item.reference, "conversation_timeline_child")?;
                if let Some(child) = &child {
                    pending.push(child.clone());
                }
                items.push(ConversationTimelineItem {
                    item_ref: item.reference.clone(),
                    publication_ref: item.properties["publication_ref"]
                        .as_str()
                        .ok_or("missing publication ref")?
                        .into(),
                    connection_ref: reference.clone(),
                    sibling_sequence: edge.position.try_into()?,
                    content,
                    child_connection_ref: child,
                });
            }
            connections.push((
                record.properties["store_sequence"]
                    .as_u64()
                    .ok_or("invalid timeline store sequence")?,
                ConversationTimelineConnection {
                    connection_ref: reference,
                    owner,
                    revision: record.properties["revision"]
                        .as_u64()
                        .ok_or("invalid timeline revision")?,
                    next_sequence: record.properties["next_sequence"]
                        .as_u64()
                        .ok_or("invalid timeline sequence")?,
                    items,
                },
            ));
        }
        connections.sort_by_key(|(position, _)| *position);
        let timeline = ConversationTimeline {
            root_connection_ref: root.target_ref.clone(),
            connections: connections
                .into_iter()
                .map(|(_, connection)| connection)
                .collect(),
        };
        if result.insert(root.source_ref.clone(), timeline).is_some() {
            return invalid("multiple conversation timeline roots");
        }
    }
    Ok(result)
}
pub(super) fn decode_conversation(
    page: &SnapshotPage,
    reference: &str,
) -> CwrResult<RetainedConversation> {
    let records = page
        .records
        .iter()
        .map(|record| (record.reference.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let record = required(&records, reference, RecordKind::Conversation)?;
    let mut conversation = RetainedConversation {
        conversation_ref: reference.into(),
        title: optional_string(&record.properties, "title")?,
        owner_context_ref: optional_string(&record.properties, "owner_context_ref")?,
        sidebar_eligible: record.properties["sidebar_eligible"].as_bool(),
        archived: !edges(page, reference, "archived").is_empty(),
        parent_conversation_ref: one_target(page, reference, "parent")?,
        participant_actor_refs: edges(page, reference, "participant")
            .into_iter()
            .map(|edge| edge.target_ref.clone())
            .collect(),
        timeline_connection_ref: one_target(page, reference, "conversation_timeline_root")?,
        messages: Vec::new(),
        activity: Vec::new(),
    };
    hydrate_conversation(page, &records, &mut conversation)?;
    Ok(conversation)
}
fn hydrate_conversation(
    page: &SnapshotPage,
    records: &BTreeMap<&str, &Record>,
    conversation: &mut RetainedConversation,
) -> CwrResult<()> {
    for edge in edges(page, &conversation.conversation_ref, "message") {
        let mut message: ConversationMessage =
            payload(required(records, &edge.target_ref, RecordKind::Message)?)?;
        if message.message_ref != edge.target_ref {
            return invalid("message identity mismatch");
        }
        if let Some(visualizations) = edge.properties.get("visualizations") {
            message.visualizations = serde_json::from_value(visualizations.clone())?;
        }
        conversation.messages.push(message);
    }
    for kind in ["activity", "transfer_metadata"] {
        for edge in edges(page, &conversation.conversation_ref, kind) {
            let expected = if kind == "activity" {
                RecordKind::Activity
            } else {
                RecordKind::ConversationTransferMetadata
            };
            let activity: ConversationActivity =
                payload(required(records, &edge.target_ref, expected)?)?;
            if activity.activity_ref != edge.target_ref {
                return invalid("activity identity mismatch");
            }
            if (activity_kind(&activity).0 == RecordKind::ConversationTransferMetadata)
                != (kind == "transfer_metadata")
            {
                return invalid("invalid transfer metadata activity");
            }
            conversation.activity.push(activity);
        }
    }
    conversation
        .activity
        .sort_by_key(|activity| activity.sequence);
    Ok(())
}
fn optional_string(value: &Value, field: &str) -> CwrResult<Option<String>> {
    value
        .get(field)
        .filter(|value| !value.is_null())
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| invalid_error(format!("invalid {field}")))
        })
        .transpose()
}
fn required<'a>(
    records: &BTreeMap<&str, &'a Record>,
    reference: &str,
    kind: RecordKind,
) -> CwrResult<&'a Record> {
    let record = *records
        .get(reference)
        .ok_or_else(|| invalid_error(format!("missing retained {}", kind.as_str())))?;
    if record.kind != kind || record.schema_ref != crate::cwpdb::schema_ref(kind.as_str()) {
        return invalid("unexpected retained conversation record schema");
    }
    Ok(record)
}
fn payload<T: DeserializeOwned>(record: &Record) -> CwrResult<T> {
    let mut value = record.properties["record"].clone();
    if record.kind == RecordKind::Invocation {
        if let Some(fields) = value.as_object_mut() {
            fields.remove("attachment");
        }
    } else if record.kind == RecordKind::Activity {
        if let Some(event) = value.get_mut("event").and_then(Value::as_object_mut) {
            if event.get("kind").and_then(Value::as_str) == Some("work_update") {
                event.remove("attachment");
            }
        }
    }
    Ok(serde_json::from_value(value)?)
}
fn payload_map<T: DeserializeOwned>(
    page: &SnapshotPage,
    kind: RecordKind,
) -> CwrResult<BTreeMap<String, T>> {
    page.records
        .iter()
        .filter(|record| record.kind == kind)
        .map(|record| Ok((record.reference.clone(), payload(record)?)))
        .collect()
}
fn edges<'a>(page: &'a SnapshotPage, source: &str, kind: &str) -> Vec<&'a Relationship> {
    let mut edges = page
        .relationships
        .iter()
        .filter(|edge| edge.source_ref == source && edge.kind == kind)
        .collect::<Vec<_>>();
    edges.sort_by_key(|edge| (edge.position, &edge.target_ref));
    edges
}
fn one_target(page: &SnapshotPage, source: &str, kind: &str) -> CwrResult<Option<String>> {
    let edges = edges(page, source, kind);
    match edges.as_slice() {
        [] => Ok(None),
        [edge] => Ok(Some(edge.target_ref.clone())),
        _ => invalid(format!("multiple {kind} relationships")),
    }
}

pub(super) fn decode_tool_operations(
    page: &SnapshotPage,
) -> CwrResult<BTreeMap<String, tools::ToolOperation>> {
    let records = page
        .records
        .iter()
        .map(|record| (record.reference.as_str(), record))
        .collect::<BTreeMap<_, _>>();
    let mut tool_operations = payload_map::<tools::ToolOperation>(page, RecordKind::ToolOperation)?;
    for (reference, operation) in &mut tool_operations {
        operation.activity.files = edges(page, reference, "tool_file")
            .into_iter()
            .map(|edge| payload(required(&records, &edge.target_ref, RecordKind::ToolFile)?))
            .collect::<CwrResult<Vec<_>>>()?;
    }
    Ok(tool_operations)
}
