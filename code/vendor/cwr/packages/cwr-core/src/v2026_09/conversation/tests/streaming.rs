fn stream_output(claimed: &ClaimedConversationInvocation, item: &str, phase: crate::open_responses::v2026_04_24::AssistantPhase, sequence: i64, text: &str) -> crate::adapter::AdapterLiveOutput {
    crate::adapter::AdapterLiveOutput { invocation_ref: claimed.claim.invocation.invocation_ref.clone(),
        model_run_ref: claimed.model_run_ref.clone(), provider: "codex".into(), thread_id: Some("thread".into()),
        response_or_turn_id: "turn".into(), item_id: item.into(), phase, sequence_number: sequence, text: text.into() }
}
#[test]
fn streaming_commentary_finalization_preserves_message_and_placement_identity() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(directory.path().join("streaming.sqlite"), "cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "streaming-commentary");
    let claimed = claim_working(&mut store, &turn);
    let output = stream_output(&claimed, "commentary", AssistantPhase::Commentary, 1, "Thinking");
    assert!(store.record_streaming_output(Some(&output), &[]).unwrap());
    let stream = store.state.streaming_messages.values().next().unwrap().clone();
    let item = store.retained_timeline(&stream.conversation_ref).unwrap().connections.iter().flat_map(|c| &c.items).find(|i| i.content.reference() == stream.message.message_ref).unwrap().clone();
    let mut updated = output.clone(); updated.sequence_number = 2; updated.text = "Thinking more".into();
    assert!(store.record_streaming_output(Some(&updated), &[]).unwrap());
    assert!(!store.record_streaming_output(Some(&output), &[]).unwrap());
    let mut reader = store.read_store().unwrap();
    let operation = reader.begin_operation(std::time::Duration::from_secs(2)).unwrap();
    assert_eq!(operation.conversation_timeline_placed_message(&stream.conversation_ref, &item.item_ref).unwrap().unwrap().text, "Thinking more");
    let resource = operation.conversation_resource(&stream.message.message_ref).unwrap().unwrap();
    assert!(matches!(resource.resource, ConversationResource::Message { streaming: true, .. }));
    let compatibility = operation.conversation_timeline(&stream.conversation_ref, ConversationRecordPageRequest { first: Some(100), ..Default::default() }.window()).unwrap();
    assert!(compatibility.messages.iter().all(|m| m.message_ref != stream.message.message_ref));
    drop(operation);
    let commentary = AdapterCompletedCommentary { model_run_ref: output.model_run_ref.clone(), provider: output.provider.clone(), thread_id: output.thread_id.clone(), response_or_turn_id: output.response_or_turn_id.clone(), item_id: output.item_id.clone(), text: "Final commentary".into() };
    store.record_commentary_progress(&output.invocation_ref, claimed.claim.invocation.attempt_count, commentary).unwrap();
    assert!(store.state.streaming_messages.is_empty());
    let published = store.conversation(&stream.conversation_ref).unwrap().messages.iter().find(|m| m.message_ref == stream.message.message_ref).unwrap();
    assert_eq!(published.text, "Final commentary");
    let current = store.retained_timeline(&stream.conversation_ref).unwrap().item(&item.item_ref).unwrap();
    assert_eq!(current, &item);
    assert!(!store.record_streaming_output(Some(&updated), &[]).unwrap());
    let mut reader = store.read_store().unwrap();
    let resource = reader.begin_operation(std::time::Duration::from_secs(2)).unwrap().conversation_resource(&stream.message.message_ref).unwrap().unwrap();
    assert!(matches!(resource.resource, ConversationResource::Message { streaming: false, .. }));
}
#[test]
fn streaming_final_answer_and_closed_run_keep_the_id_until_atomic_completion() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(directory.path().join("final.sqlite"), "cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "streaming-answer");
    let claimed = claim_working(&mut store, &turn);
    let output = stream_output(&claimed, "answer", AssistantPhase::FinalAnswer, 1, "Part");
    store.record_streaming_output(Some(&output), &[]).unwrap();
    let reference = store.state.streaming_messages.keys().next().unwrap().clone();
    store.record_streaming_output(None, &[crate::adapter::AdapterClosedModelRun { invocation_ref: output.invocation_ref.clone(), model_run_ref: output.model_run_ref.clone() }]).unwrap();
    let mut late = output.clone(); late.sequence_number = 9; late.text = "late".into();
    assert!(!store.record_streaming_output(Some(&late), &[]).unwrap());
    assert!(matches!(store.record_commentary_progress(&output.invocation_ref, claimed.claim.invocation.attempt_count,
        AdapterCompletedCommentary { model_run_ref: output.model_run_ref.clone(), provider: output.provider.clone(), thread_id: output.thread_id.clone(), response_or_turn_id: output.response_or_turn_id.clone(), item_id: "late-commentary".into(), text: "Must not publish".into() }).unwrap(), RecordCommentaryProgressOutcome::IgnoredStaleModelRun));
    settle_working(&mut store, &claimed);
    assert!(store.state.streaming_messages.is_empty());
    let reply = store.admission_record_for_invocation(&output.invocation_ref).unwrap().reply_message_ref.as_ref().unwrap();
    assert_eq!(reply, &reference);
}
#[test]
fn streaming_withdrawal_on_restart_leaves_retained_messages_and_fences_old_runs() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("restart.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "streaming-restart"); let claimed = claim_working(&mut store, &turn);
    let output = stream_output(&claimed, "answer", AssistantPhase::FinalAnswer, 1, "Unpublished");
    store.record_streaming_output(Some(&output), &[]).unwrap();
    let reference = store.state.streaming_messages.keys().next().unwrap().clone();
    let source = turn.record.source_message_ref.clone();
    drop(store);
    let mut store = ConversationAdmissionStore::open_with_timeline_rollout(&path, "cwp://com.example.runtime", ConversationTimelineRollout::NewConversations).unwrap();
    assert!(store.state.streaming_messages.is_empty());
    assert!(!store.record_streaming_output(Some(&output), &[]).unwrap());
    let timeline = store.retained_timeline(&turn.record.conversation_ref).unwrap();
    assert!(timeline.connections.iter().flat_map(|c| &c.items).all(|i| i.content.reference() != reference));
    assert!(timeline.connections.iter().flat_map(|c| &c.items).any(|i| i.content.reference() == source));
    let mut reader = store.read_store().unwrap();
    assert!(reader.begin_operation(std::time::Duration::from_secs(2)).unwrap().conversation_resource(&reference).unwrap().is_none());
}

#[test]
fn streaming_retry_allocates_new_ids_and_rejects_the_previous_model_run() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(directory.path().join("retry.sqlite"), "cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "streaming-retry");
    let claimed = claim_working(&mut store, &turn);
    let old = stream_output(&claimed, "same-provider-item", AssistantPhase::FinalAnswer, 1, "First attempt");
    store.record_streaming_output(Some(&old), &[]).unwrap();
    let old_ref = store.state.streaming_messages.keys().next().unwrap().clone();
    assert!(store.recover_interrupted_invocation(&old.invocation_ref).unwrap());
    assert!(store.state.streaming_messages.is_empty());
    let retried = claim_working(&mut store, &turn);
    let next = stream_output(&retried, "same-provider-item", AssistantPhase::FinalAnswer, 1, "Second attempt");
    assert!(store.record_streaming_output(Some(&next), &[]).unwrap());
    let next_ref = store.state.streaming_messages.keys().next().unwrap().clone();
    assert_ne!(next_ref, old_ref);
    let mut late = old; late.sequence_number = 100;
    assert!(!store.record_streaming_output(Some(&late), &[]).unwrap());
    assert_eq!(store.state.streaming_messages.keys().next().unwrap(), &next_ref);
}

#[test]
fn streaming_failed_write_rolls_back_text_sequence_and_snapshot_visibility() {
    use crate::open_responses::v2026_04_24::AssistantPhase;
    let directory = tempfile::tempdir().unwrap(); let path = directory.path().join("rollback.sqlite");
    let mut store = ConversationAdmissionStore::open(&path, "cwp://com.example.runtime").unwrap();
    store.configure_timeline_rollout(ConversationTimelineRollout::NewConversations).unwrap();
    let turn = working_turn(&mut store, None, "streaming-rollback"); let claimed = claim_working(&mut store, &turn);
    let mut output = stream_output(&claimed, "answer", AssistantPhase::FinalAnswer, 1, "Old text");
    store.record_streaming_output(Some(&output), &[]).unwrap();
    let reference = store.state.streaming_messages.keys().next().unwrap().clone();
    let snapshot = ConversationPublicationSnapshot::open(&path, "cwp://com.example.runtime", crate::v2026_09::test_fixture::actor_ref("person"), std::time::Instant::now()+std::time::Duration::from_secs(10), ConversationReadCancellation::pending()).unwrap();
    cwpdb_store::test_support::publication_faults::reject_streaming_updates(&path,true).unwrap();
    output.sequence_number = 2; output.text = "New text".into();
    assert!(store.record_streaming_output(Some(&output), &[]).is_err());
    assert_eq!(store.state.streaming_messages[&reference].message.text, "Old text");
    cwpdb_store::test_support::publication_faults::reject_streaming_updates(&path,false).unwrap();
    assert!(store.record_streaming_output(Some(&output), &[]).unwrap());
    let frozen = snapshot.resource(&reference).unwrap().unwrap();
    assert!(matches!(frozen.resource, ConversationResource::Message { message, streaming: true, .. } if message.text == "Old text"));
    store.fail(&output.invocation_ref, "Interrupted".into()).unwrap();
    assert!(store.state.streaming_messages.is_empty());
    assert!(snapshot.resource(&reference).unwrap().is_some());
    let fresh = ConversationPublicationSnapshot::open(&path, "cwp://com.example.runtime", crate::v2026_09::test_fixture::actor_ref("person"), std::time::Instant::now()+std::time::Duration::from_secs(10), ConversationReadCancellation::pending()).unwrap();
    assert!(fresh.resource(&reference).unwrap().is_none());
}
