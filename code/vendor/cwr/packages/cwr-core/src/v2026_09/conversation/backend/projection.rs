//! Authoritative protocol publication order, independent of physical storage.
use super::*;
use std::collections::BTreeSet;
pub(in crate::v2026_09::conversation) fn authoritative_projection(
    state: &ConversationRuntimeState,
    conversation: &RetainedConversation,
) -> RetainedConversation {
    let mut projected = crate::v2026_09::host::project_conversation(conversation);
    projected.activity.extend(
        crate::v2026_09::host::project_authoritative_work_activities(
            conversation,
            state.conversations.iter(),
            state.invocation_ledger.invocations(),
            state.invocation_ledger.waits(),
            |reference| invocation_execution_conversation(state, reference),
            |reference| state.invocation_ledger.invocation(reference).is_some(),
        ),
    );
    crate::v2026_09::host::sort_work_activities(&mut projected.activity);
    projected
}

pub(in crate::v2026_09::conversation) fn invocation_execution_conversation<'a>(
    state: &'a ConversationRuntimeState,
    invocation_ref: &str,
) -> Option<&'a RetainedConversation> {
    let execution_ref = state
        .invocation_execution_conversation_refs
        .get(invocation_ref)?;
    state
        .conversations
        .iter()
        .find(|conversation| &conversation.conversation_ref == execution_ref)
}

#[derive(Default, PartialEq, Eq)]
pub(in crate::v2026_09::conversation) struct TimelineTurnShape {
    source_message_ref: String,
    parent_invocation_ref: Option<String>,
}

pub(in crate::v2026_09::conversation) fn timeline_turn_shapes(
    activities: &[ConversationActivity],
) -> (BTreeMap<String, TimelineTurnShape>, Vec<String>) {
    let mut turns = BTreeMap::<String, TimelineTurnShape>::new();
    let mut turn_order = Vec::new();
    for activity in activities {
        if let ConversationActivityEvent::WorkLifecycle {
            invocation_ref,
            source_message_ref,
            parent_invocation_ref,
            ..
        } = &activity.event
        {
            if !turns.contains_key(invocation_ref) {
                turn_order.push(invocation_ref.clone());
            }
            let turn = turns.entry(invocation_ref.clone()).or_default();
            turn.source_message_ref = source_message_ref.clone();
            if parent_invocation_ref.is_some() {
                turn.parent_invocation_ref = parent_invocation_ref.clone();
            }
            continue;
        }
        let ConversationActivityEvent::WorkUpdate {
            invocation_ref,
            source_message_ref,
            parent_invocation_ref,
            ..
        } = &activity.event
        else {
            continue;
        };
        if !turns.contains_key(invocation_ref) {
            turn_order.push(invocation_ref.clone());
        }
        let turn = turns.entry(invocation_ref.clone()).or_default();
        turn.source_message_ref = source_message_ref.clone();
        if parent_invocation_ref.is_some() {
            turn.parent_invocation_ref = parent_invocation_ref.clone();
        }
    }

    (turns, turn_order)
}

pub(in crate::v2026_09::conversation) fn semantic_timeline_items(
    conversation: &RetainedConversation,
) -> Vec<(&'static str, String)> {
    let (turns, turn_order) = timeline_turn_shapes(&conversation.activity);

    fn depth(
        invocation_ref: &str,
        turns: &BTreeMap<String, TimelineTurnShape>,
        visiting: &mut BTreeSet<String>,
    ) -> usize {
        if !visiting.insert(invocation_ref.to_owned()) {
            return 0;
        }
        let result = turns
            .get(invocation_ref)
            .and_then(|turn| turn.parent_invocation_ref.as_deref())
            .filter(|parent| turns.contains_key(*parent))
            .map_or(0, |parent| depth(parent, turns, visiting) + 1);
        visiting.remove(invocation_ref);
        result
    }
    let root_turns = turn_order
        .iter()
        .filter(|invocation_ref| {
            depth(invocation_ref, &turns, &mut BTreeSet::new()).is_multiple_of(2)
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut turns_by_source = BTreeMap::<&str, Vec<&str>>::new();
    for invocation_ref in &turn_order {
        let turn = &turns[invocation_ref];
        turns_by_source
            .entry(&turn.source_message_ref)
            .or_default()
            .push(invocation_ref);
    }
    let messages_by_ref = conversation
        .messages
        .iter()
        .filter(|message| message.progress.is_none())
        .map(|message| (message.message_ref.as_str(), message))
        .collect::<BTreeMap<_, _>>();
    fn message_depth(
        message_ref: &str,
        messages: &BTreeMap<&str, &ConversationMessage>,
        turns_by_source: &BTreeMap<&str, Vec<&str>>,
        turns: &BTreeMap<String, TimelineTurnShape>,
        visiting: &mut BTreeSet<String>,
    ) -> usize {
        if !visiting.insert(message_ref.to_owned()) {
            return 0;
        }
        let result = turns_by_source
            .get(message_ref)
            .and_then(|sources| sources.first())
            .map_or_else(
                || {
                    messages
                        .get(message_ref)
                        .and_then(|message| message.reply_to_message_ref.as_deref())
                        .map_or(0, |reply| {
                            message_depth(reply, messages, turns_by_source, turns, visiting)
                        })
                },
                |invocation_ref| depth(invocation_ref, turns, &mut BTreeSet::new()),
            );
        visiting.remove(message_ref);
        result
    }
    let mut included_turns = BTreeSet::new();
    let mut items = Vec::new();
    let mut pending_turns = Vec::new();
    for message in conversation
        .messages
        .iter()
        .filter(|message| message.progress.is_none())
    {
        if message_depth(
            &message.message_ref,
            &messages_by_ref,
            &turns_by_source,
            &turns,
            &mut BTreeSet::new(),
        ) == 0
        {
            items.append(&mut pending_turns);
        }
        items.push(("message", message.message_ref.clone()));
        for invocation_ref in turns_by_source
            .get(message.message_ref.as_str())
            .into_iter()
            .flatten()
            .filter(|invocation_ref| root_turns.contains(**invocation_ref))
        {
            included_turns.insert((*invocation_ref).to_owned());
            pending_turns.push(("turn", (*invocation_ref).to_owned()));
        }
    }
    items.append(&mut pending_turns);
    for invocation_ref in turn_order {
        if root_turns.contains(&invocation_ref) && !included_turns.contains(&invocation_ref) {
            items.push(("turn", invocation_ref));
        }
    }
    items
}

#[test]
fn semantic_timeline_preserves_embedded_messages_for_desktop_folding() {
    let conversation: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": [],
        "messages": [{
            "message_ref": "cwp://example.test/messages/request",
            "author_actor_ref": "~@/actors/person", "addressed_actor_refs": [], "text": "Request"
        }, {
            "message_ref": "cwp://example.test/messages/call",
            "author_actor_ref": "~@/actors/parent", "addressed_actor_refs": [], "text": "Call",
            "action": {"kind": "call", "activation": "@child--review", "input": {}}
        }, {
            "message_ref": "cwp://example.test/messages/result",
            "author_actor_ref": "~@/actors/child", "addressed_actor_refs": [], "text": "Done",
            "reply_to_message_ref": "cwp://example.test/messages/call",
            "action": {"kind": "result", "activation": "@child--review", "result": {}, "outcome": "succeeded"}
        }, {
            "message_ref": "cwp://example.test/messages/after",
            "author_actor_ref": "~@/actors/person", "addressed_actor_refs": [], "text": "After"
        }],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/parent-link", "sequence": 1,
            "event": {"kind": "work_update", "invocation_ref": "cwp://example.test/invocations/parent",
                "source_message_ref": "cwp://example.test/messages/request", "actor_ref": "~@/actors/parent", "status": 102,
                "activity": {"activity_ref": "cwp://example.test/work/call", "kind": "coworker", "label": "Review",
                    "state": "running", "started_at_unix_ms": 1,
                    "child_invocation_ref": "cwp://example.test/invocations/child"}}
        }, {
            "activity_ref": "cwp://example.test/activities/child", "sequence": 2,
            "event": {"kind": "work_update", "invocation_ref": "cwp://example.test/invocations/child",
                "source_message_ref": "cwp://example.test/messages/call",
                "parent_invocation_ref": "cwp://example.test/invocations/parent",
                "actor_ref": "~@/actors/child", "status": 200, "text": "Done"}
        }, {
            "activity_ref": "cwp://example.test/activities/orphan", "sequence": 3,
            "event": {"kind": "work_update", "invocation_ref": "cwp://example.test/invocations/orphan",
                "source_message_ref": "cwp://example.test/messages/missing",
                "actor_ref": "~@/actors/child", "status": 200, "text": "Orphan done"}
        }]
    })).unwrap();

    let mut running_value = serde_json::to_value(&conversation).unwrap();
    running_value["messages"]
        .as_array_mut()
        .unwrap()
        .retain(|message| message["message_ref"] != "cwp://example.test/messages/result");
    let child_event = running_value["activity"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|activity| activity["activity_ref"] == "cwp://example.test/activities/child")
        .unwrap()["event"]
        .as_object_mut()
        .unwrap();
    child_event.insert("status".into(), serde_json::json!(102));
    child_event.remove("text");
    let running: RetainedConversation = serde_json::from_value(running_value).unwrap();

    assert_eq!(
        semantic_timeline_items(&running),
        vec![
            ("message", "cwp://example.test/messages/request".into()),
            ("message", "cwp://example.test/messages/call".into()),
            ("turn", "cwp://example.test/invocations/parent".into()),
            ("message", "cwp://example.test/messages/after".into()),
            ("turn", "cwp://example.test/invocations/orphan".into()),
        ]
    );

    assert_eq!(
        semantic_timeline_items(&conversation),
        vec![
            ("message", "cwp://example.test/messages/request".into()),
            ("message", "cwp://example.test/messages/call".into()),
            ("message", "cwp://example.test/messages/result".into()),
            ("turn", "cwp://example.test/invocations/parent".into()),
            ("message", "cwp://example.test/messages/after".into()),
            ("turn", "cwp://example.test/invocations/orphan".into()),
        ]
    );
}

#[test]
fn semantic_timeline_keeps_parent_work_in_newest_nested_message_page() {
    let mut messages = vec![serde_json::json!({
        "message_ref": "cwp://example.test/messages/request",
        "author_actor_ref": "~@/actors/person", "addressed_actor_refs": [], "text": "Request"
    })];
    let mut activity = vec![serde_json::json!({
        "activity_ref": "cwp://example.test/activities/parent", "sequence": 1,
        "event": {"kind": "work_update", "invocation_ref": "cwp://example.test/invocations/parent",
            "source_message_ref": "cwp://example.test/messages/request", "actor_ref": "~@/actors/parent",
            "status": 102, "text": "Working"}
    })];
    for index in 0..60 {
        let call = format!("cwp://example.test/messages/call-{index}");
        messages.push(serde_json::json!({
            "message_ref": call, "author_actor_ref": "~@/actors/parent",
            "addressed_actor_refs": ["~@/actors/child"], "text": "Call"
        }));
        messages.push(serde_json::json!({
            "message_ref": format!("cwp://example.test/messages/result-{index}"),
            "author_actor_ref": "~@/actors/child", "addressed_actor_refs": ["~@/actors/parent"],
            "text": "Done", "reply_to_message_ref": call
        }));
        activity.push(serde_json::json!({
            "activity_ref": format!("cwp://example.test/activities/child-{index}"),
            "sequence": index + 2,
            "event": {"kind": "work_update",
                "invocation_ref": format!("cwp://example.test/invocations/child-{index}"),
                "source_message_ref": format!("cwp://example.test/messages/call-{index}"),
                "parent_invocation_ref": "cwp://example.test/invocations/parent",
                "actor_ref": "~@/actors/child", "status": 200, "text": "Done"}
        }));
    }
    messages.push(serde_json::json!({
        "message_ref": "cwp://example.test/messages/after",
        "author_actor_ref": "~@/actors/parent", "addressed_actor_refs": ["~@/actors/person"],
        "text": "Complete"
    }));
    let conversation: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": [], "messages": messages, "activity": activity
    }))
    .unwrap();

    let items = semantic_timeline_items(&conversation);
    let newest = &items[items.len() - 100..];
    assert!(newest.contains(&("turn", "cwp://example.test/invocations/parent".into())));
    assert_eq!(
        newest.last(),
        Some(&("message", "cwp://example.test/messages/after".into()))
    );
}

#[test]
fn authoritative_timeline_projects_turns_from_execution_conversations() {
    let root: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/root",
        "participant_actor_refs": ["~@/actors/person", "~@/actors/helper"],
        "messages": [{
            "message_ref": "cwp://example.test/messages/request",
            "author_actor_ref": "~@/actors/person", "addressed_actor_refs": ["~@/actors/helper"],
            "text": "Help"
        }]
    }))
    .unwrap();
    let execution: RetainedConversation = serde_json::from_value(serde_json::json!({
        "conversation_ref": "cwp://example.test/conversations/execution",
        "parent_conversation_ref": root.conversation_ref,
        "participant_actor_refs": ["~@/actors/helper"],
        "messages": [],
        "activity": [{
            "activity_ref": "cwp://example.test/activities/start", "sequence": 1,
            "event": {"kind": "turn_started", "invocation_ref": "cwp://example.test/invocations/one",
                "source_message_ref": "cwp://example.test/messages/request"}
        }, {
            "activity_ref": "cwp://example.test/activities/done", "sequence": 2,
            "event": {"kind": "turn_completed", "status": 200, "summary": "Done"}
        }]
    })).unwrap();
    let state = ConversationRuntimeState {
        protected_result_children: Default::default(),
        streaming_messages: Default::default(),
        streaming_runs: Default::default(),
        tool_attempts: Default::default(),
        tool_operations: Default::default(),
        version: STATE_VERSION.into(),
        runtime_root_ref: "cwp://example.test".into(),
        conversations: vec![root.clone(), execution].into(),
        cold_conversation_refs: Default::default(),
        files: Default::default(),
        visualization_artifacts: BTreeMap::new().into(),
        admissions: Vec::new().into(),
        invocation_ledger: ActionInvocationLedger::default(),
        timeline_store: ConversationTimelineStore::default(),
        invocation_execution_conversation_refs: BTreeMap::new().into(),
        upstream_participant_versions: BTreeMap::new().into(),
    };

    let projected = authoritative_projection(&state, &root);
    assert_eq!(
        semantic_timeline_items(&projected),
        vec![
            ("message", "cwp://example.test/messages/request".into()),
            ("turn", "cwp://example.test/invocations/one".into()),
        ]
    );
}
