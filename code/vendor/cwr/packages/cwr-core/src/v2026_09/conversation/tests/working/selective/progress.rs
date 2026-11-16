use super::*;

#[test]
fn selective_history_delivers_all_new_progress_as_messages() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("progress-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    for index in 0..6 {
        store
            .record_invocation_progress_with_publication(
                &claimed.adapter_invocation.invocation_ref,
                102,
                format!("progress-{index}"),
                true,
            )
            .unwrap();
    }
    settle_working(&mut store, &claimed);

    let second =
        selective_working_turn(&mut store, Some(&first.record.conversation_ref), "followup");
    let followup = claim_working(&mut store, &second);
    let progress = followup.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["progress"].is_object())
        .map(|message| message["text"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        progress,
        (0..6)
            .map(|index| format!("progress-{index}"))
            .collect::<Vec<_>>()
    );
    settle_working(&mut store, &followup);
    let third =
        selective_working_turn(&mut store, Some(&first.record.conversation_ref), "third");
    let third = claim_working(&mut store, &third);
    assert!(third.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .all(|message| !message["progress"].is_object()));
}

#[test]
fn selective_history_delivers_stopped_progress_as_messages() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("stopped-progress-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    for index in 0..6 {
        store
            .record_invocation_progress_with_publication(
                &claimed.adapter_invocation.invocation_ref,
                102,
                format!("stopped-progress-{index}"),
                true,
            )
            .unwrap();
    }
    store
        .stop_invocation(
            &claimed.adapter_invocation.invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(103),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&claimed.adapter_invocation.invocation_ref, 103)
        .unwrap();

    let second = selective_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "continue after stop",
    );
    let followup = claim_working(&mut store, &second);
    assert!(followup.adapter_invocation.thread["working_conversation"]["history_gap"]
        .as_str()
        .unwrap()
        .contains("previous request was stopped"));
    let progress = followup.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["progress"].is_object())
        .map(|message| message["text"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        progress,
        (0..6)
            .map(|index| format!("stopped-progress-{index}"))
            .collect::<Vec<_>>()
    );
}

#[test]
fn selective_history_delivers_progress_from_a_safely_stopped_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("safely-stopped-progress-selective.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let first = selective_working_turn(&mut store, None, "first");
    let claimed = claim_working(&mut store, &first);
    for index in 0..6 {
        store
            .record_invocation_progress_with_publication(
                &claimed.adapter_invocation.invocation_ref,
                102,
                format!("pre-live-progress-{index}"),
                true,
            )
            .unwrap();
    }
    store
        .append_execution_activity(
            &claimed.adapter_invocation.invocation_ref,
            ConversationActivityEvent::ProviderEvent {
                provider: "codex".into(),
                method: "cwr.adapter_checkpoint".into(),
                data: json!({
                    "version": 4,
                    "invocation_ref": claimed.adapter_invocation.invocation_ref,
                    "integration_ref": claimed.adapter_invocation.integration_ref,
                    "live_started": false
                }),
            },
        )
        .unwrap();
    store
        .stop_invocation(
            &claimed.adapter_invocation.invocation_ref,
            crate::v2026_09::invocation::InvocationStopRequest::new(103),
        )
        .unwrap();
    store
        .settle_stopped_invocation(&claimed.adapter_invocation.invocation_ref, 103)
        .unwrap();

    let second = selective_working_turn(
        &mut store,
        Some(&first.record.conversation_ref),
        "continue after pre-live stop",
    );
    let followup = claim_working(&mut store, &second);
    assert!(followup.adapter_invocation.thread["working_conversation"]
        .get("history_gap")
        .is_none());
    let progress = followup.adapter_invocation.thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["progress"].is_object())
        .map(|message| message["text"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(
        progress,
        (0..6)
            .map(|index| format!("pre-live-progress-{index}"))
            .collect::<Vec<_>>()
    );
}
