use super::*;

#[test]
fn selective_skipped_predecessors_keep_published_progress_after_reopen() {
    for settled_history in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("skipped-progress.sqlite");
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let first = selective_working_turn(&mut store, None, "first request");
        let first_claim = claim_working(&mut store, &first);
        let conversation_ref = first.record.conversation_ref.clone();
        let mut stopped = first_claim;
        let settled_ref = if settled_history {
            settle_working(&mut store, &stopped);
            let prior_ref = stopped.adapter_invocation.invocation_ref.clone();
            let turn =
                selective_working_turn(&mut store, Some(&conversation_ref), "stopped source");
            stopped = claim_working(&mut store, &turn);
            Some(prior_ref)
        } else {
            None
        };
        let mut expected = Vec::new();
        // Two consecutive skipped invocations exercise the complete skip chain.
        for turn_index in 0..2 {
            for update in 0..6 {
                store
                    .record_invocation_progress_with_publication(
                        &stopped.adapter_invocation.invocation_ref,
                        101 + update,
                        format!("preparation-{turn_index}-{update}"),
                        true,
                    )
                    .unwrap();
            }
            expected.extend(
                store
                    .conversation(&conversation_ref)
                    .unwrap()
                    .messages
                    .iter()
                    .filter(|message| {
                        message.progress.as_ref().is_some_and(|progress| {
                            progress.invocation_ref == stopped.adapter_invocation.invocation_ref
                        })
                    })
                    .map(|message| {
                        serde_json::to_value(message.clone_for_external_delivery()).unwrap()
                    }),
            );
            super::super::retain_adapter_checkpoint(&mut store, &stopped, false);
            store
                .stop_invocation(
                    &stopped.adapter_invocation.invocation_ref,
                    crate::v2026_09::invocation::InvocationStopRequest::new(110),
                )
                .unwrap();
            store
                .settle_stopped_invocation(&stopped.adapter_invocation.invocation_ref, 110)
                .unwrap();
            if turn_index == 0 {
                let turn = selective_working_turn(
                    &mut store,
                    Some(&conversation_ref),
                    "second stopped source",
                );
                stopped = claim_working(&mut store, &turn);
            }
        }
        // Older unrelated history must still be omitted by selective delivery.
        for index in 0..5 {
            let event = format!("unrelated-{index}");
            let prepared = store
                .prepare(
                    &fixture_graph_with_reviewer(),
                    &ingress(
                        &event,
                        crate::v2026_09::test_fixture::actor_ref("person"),
                        Some(&conversation_ref),
                        ConversationIngressAddressing::Unaddressed,
                        &event,
                    ),
                    None,
                )
                .unwrap();
            store.commit(prepared).unwrap();
        }
        drop(store);
        let mut store =
            ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
        let followup =
            selective_working_turn(&mut store, Some(&conversation_ref), "next model handoff");
        let claimed = claim_working(&mut store, &followup);
        let thread = &claimed.adapter_invocation.thread;
        assert_eq!(thread["history_delivery"]["mode"], "selective");
        if let Some(prior_ref) = settled_ref {
            assert_eq!(
                thread["working_conversation"]["previous_invocation_ref"],
                prior_ref
            );
            assert!(thread["working_conversation"].get("state").is_some());
        } else {
            assert!(thread["working_conversation"].get("state").is_none());
            assert!(thread["working_conversation"].get("history_gap").is_none());
        }
        let messages = thread["messages"].as_array().unwrap();
        for progress in expected {
            assert!(
                messages.contains(&progress),
                "missing or altered progress: {progress}"
            );
            assert!(messages
                .iter()
                .any(|message| message["message_ref"] == progress["reply_to_message_ref"]));
        }
        assert!(!messages
            .iter()
            .any(|message| message["text"] == "unrelated-0"));
        assert!(messages
            .iter()
            .any(|message| message["text"] == "next model handoff"));
        // The selection remains smaller than the canonical shared archive.
        let retained = store.thread_observation(&conversation_ref).unwrap();
        assert!(retained["messages"].as_array().unwrap().len() > messages.len());
    }
}

#[test]
fn selective_stopped_progress_keeps_the_source_reply_target() {
    let parent_ref = "cwp://com.example.runtime/messages/parent";
    let source_ref = "cwp://com.example.runtime/messages/source";
    let progress_ref = "cwp://com.example.runtime/messages/progress";
    let handoff_ref = "cwp://com.example.runtime/messages/handoff";
    let message =
        |message_ref: &str, text: &str, reply_to_message_ref: Option<&str>, progress: bool| {
            serde_json::to_value(ConversationMessage {
                file_refs: Vec::new(),
                progress: progress.then(|| {
                    Box::new(ConversationProgress {
                        invocation_ref: "cwp://com.example.runtime/invocations/stopped".into(),
                        source_activity_ref: "cwp://com.example.runtime/activities/progress".into(),
                        status: Some(ActionResponseStatus::PROGRESS),
                    })
                }),
                message_ref: message_ref.into(),
                author_actor_ref: crate::v2026_09::test_fixture::actor_ref("person").into(),
                addressed_actor_refs: Vec::new(),
                addressing_mode: Some(ConversationAddressingMode::Implicit),
                text: text.into(),
                visualizations: Vec::new(),
                reply_to_message_ref: reply_to_message_ref.map(str::to_owned),
                action: None,
            })
            .unwrap()
        };
    let mut messages = vec![
        message(parent_ref, "original request", None, false),
        message(source_ref, "yes, do that", Some(parent_ref), false),
        message(progress_ref, "working", Some(source_ref), true),
    ];
    messages.extend(
        (0..5).map(|index| message(&format!("message/filler-{index}"), "filler", None, false)),
    );
    messages.push(message(handoff_ref, "continue", None, false));
    let mut thread = json!({
        "context_start": 0,
        "messages": messages,
        "working_conversation": {}
    });

    context::select_recent(&mut thread, Some(handoff_ref), &[source_ref.into()]).unwrap();

    let selected = thread["messages"].as_array().unwrap();
    for message_ref in [parent_ref, source_ref, progress_ref, handoff_ref] {
        assert!(
            selected
                .iter()
                .any(|message| message["message_ref"] == message_ref),
            "missing {message_ref}"
        );
    }
    assert!(!selected
        .iter()
        .any(|message| message["message_ref"] == "message/filler-0"));
}
