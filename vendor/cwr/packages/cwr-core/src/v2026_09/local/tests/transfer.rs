#[test]
fn conversation_transfer_is_available_only_through_host_controls() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), temporary.path().join("runtime.sqlite"), fixture_host_identity(),
    ).unwrap();
    let source = runtime.admit_text(None, "Remember the original request").unwrap();
    let reference = source.conversation_ref().to_owned();
    for activation in ["@cwr--export--conversation", "@cwr--import--conversation"] {
        assert!(runtime.admit_action(Some(&reference), activation, json!({})).is_err());
    }
    assert!(runtime.try_schedule_execution().unwrap().is_none());
}

#[test]
fn import_retry_succeeds_after_the_source_file_is_removed() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(
        fixture_workspace(), temporary.path().join("runtime.sqlite"), fixture_host_identity(), LocalRuntimeOptions {
            conversation_timeline_rollout: ConversationTimelineRollout::Disabled,
            ..Default::default()
        },
    ).unwrap();
    let source = runtime.admit_text(None, "Retry this import").unwrap();
    let caller = crate::v2026_09::test_fixture::actor_ref("person");
    let crate::intrinsic_cwr::IntrinsicCwrResponse::ConversationExported { export } =
        runtime.export_conversation_as(Some(source.conversation_ref()), caller).unwrap()
    else { panic!("export") };
    let file = temporary.path().join("retry-export.json");
    std::fs::write(&file, serde_json::to_vec(&export).unwrap()).unwrap();

    let first = runtime.import_conversation_as(caller, file.to_str().unwrap(), Some("retry-id")).unwrap();
    std::fs::remove_file(&file).unwrap();
    let retried = runtime.import_conversation_as(caller, file.to_str().unwrap(), Some("retry-id")).unwrap();

    assert_eq!(retried, first);
}

#[test]
fn fork_action_owns_admission_and_recovers_the_same_destination() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(fixture_workspace(), temporary.path().join("fork-action.sqlite"), fixture_host_identity(), LocalRuntimeOptions { conversation_timeline_rollout: ConversationTimelineRollout::NewConversations, ..Default::default() }).unwrap();
    let source = runtime.admit_text(None, "Fork this message").unwrap();
    let item = runtime.lock().unwrap().admission.retained_timeline(source.conversation_ref()).unwrap().connections.iter().flat_map(|c| &c.items).find(|item| item.content.reference() == source.record.source_message_ref).unwrap().item_ref.clone();
    let person = crate::v2026_09::test_fixture::actor_ref("person");
    let input = json!({"message_ref":source.record.source_message_ref,"timeline_item_ref":item});
    let admit = || runtime.admit_action_as("cwp://com.example.fixture/integrations/fork", "fork-action", person, Some(source.conversation_ref()), "@cwr--fork--conversation", input.clone()).unwrap();
    let first = admit();
    let mut task = runtime.try_schedule_execution().unwrap().unwrap();
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
    let state = runtime.lock().unwrap();
    let record = state.admission.invocation_record(first.invocation_ref().unwrap()).unwrap();
    assert_eq!(record.state, ActionInvocationState::Completed);
    let destination = record.terminal_response().unwrap().result.as_ref().unwrap()["conversation_ref"].as_str().unwrap().to_owned();
    assert_eq!(state.admission.conversation(&destination).unwrap().messages.len(),1);
    drop(state);
    let replay = admit();
    assert_eq!(replay.invocation_ref(),first.invocation_ref());
    assert!(runtime.try_schedule_execution().unwrap().is_none());
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.invocation_record(replay.invocation_ref().unwrap()).unwrap().terminal_response().unwrap().result.as_ref().unwrap()["conversation_ref"],destination);
}

#[test]
fn fork_action_accepts_legacy_message_boundary_without_a_timeline_item() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path_and_options(fixture_workspace(),temporary.path().join("legacy-action.sqlite"),fixture_host_identity(),LocalRuntimeOptions{conversation_timeline_rollout:ConversationTimelineRollout::Disabled,..Default::default()}).unwrap();
    let source=runtime.admit_text(None,"Legacy message").unwrap();
    let fork=runtime.admit_action(Some(source.conversation_ref()),"@cwr--fork--conversation",json!({"message_ref":source.record.source_message_ref})).unwrap();
    let mut task=runtime.try_schedule_execution().unwrap().unwrap();
    task.execute_with_model_output(&AdapterModelOutputMailbox::discard(),|_|{}).unwrap();
    let state=runtime.lock().unwrap();
    let record=state.admission.invocation_record(fork.invocation_ref().unwrap()).unwrap();
    assert_eq!(record.state,ActionInvocationState::Completed);
    let destination=record.terminal_response().unwrap().result.as_ref().unwrap()["conversation_ref"].as_str().unwrap();
    let copied=state.admission.conversation(destination).unwrap();
    assert!(copied.timeline_connection_ref.is_none());
    assert_eq!(copied.messages.len(),1);
}
