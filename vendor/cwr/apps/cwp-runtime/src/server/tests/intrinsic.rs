use super::*;

#[test]
fn submitted_cwr_commands_are_admitted_and_retained_as_conversation_work() {
    let temporary = tempfile::tempdir().expect("temporary runtime");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();

    let admission = submitted_cwr(&state, None, "submitted-cwr", "@cwr list coworkers");
    assert_eq!(
        admission.record.target_actor_ref.as_deref(),
        Some(cwr::action_contract::INTRINSIC_CWR_ACTOR_REF)
    );
    assert!(admission.record.invocation_ref.is_some());
    let conversation_ref = admission.record.conversation_ref;
    let conversation = wait_for_cwr_reply(&runtime, &conversation_ref);
    assert_eq!(conversation.messages.len(), 2);
    assert_eq!(
        conversation.messages[1].author_actor_ref,
        cwr::action_contract::INTRINSIC_CWR_ACTOR_REF
    );
    assert!(conversation.messages[1].text.contains("@helper"));
}

#[test]
fn submitted_cwr_commands_participate_in_normal_continuation_routing() {
    let temporary = tempfile::tempdir().expect("temporary runtime");
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let first = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "before-cwr",
            crate::test_refs::actor_ref("person").as_str(),
            None,
            "@helper hello",
        )
        .unwrap();

    let cwr = submitted_cwr(
        &state,
        Some(first.conversation_ref()),
        "retained-cwr",
        "@cwr list coworkers",
    );
    wait_for_cwr_reply(&runtime, &cwr.record.conversation_ref);

    let followup = runtime
        .submit_text_as(
            "cwp://example.test/integrations/desktop",
            "after-cwr",
            crate::test_refs::actor_ref("person").as_str(),
            Some(first.conversation_ref()),
            "keep going",
        )
        .unwrap();
    assert_eq!(
        followup.record.target_actor_ref.as_deref(),
        Some(cwr::action_contract::INTRINSIC_CWR_ACTOR_REF)
    );
}

fn submitted_cwr(
    state: &SharedState,
    conversation_ref: Option<&str>,
    request_id: &str,
    text: &str,
) -> RuntimeAdmissionReceipt {
    let (client, server) = UnixStream::pair().expect("CWR wire pair");
    client
        .set_read_timeout(Some(std::time::Duration::from_secs(2)))
        .unwrap();
    execute_command(
        state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::SubmitText {
            version: RUNTIME_SERVICE_VERSION,
            request_id: request_id.into(),
            integration_ref: "cwp://example.test/integrations/desktop".into(),
            author_actor_ref: crate::test_refs::actor_ref("person").as_str().into(),
            conversation_ref: conversation_ref.map(str::to_owned),
            text: text.into(),
        },
    )
    .expect("submit CWR text through normal runtime ingress");
    let response = BufReader::new(client).lines().next().unwrap().unwrap();
    let RuntimeServiceEvent::CommandSucceeded {
        request_id: returned_id,
        admission: Some(admission),
        intrinsic_response: None,
        ..
    } = serde_json::from_str(&response).expect("decode admission result")
    else {
        panic!("CWR text must return a normal admission")
    };
    assert_eq!(returned_id, request_id);
    *admission
}

fn wait_for_cwr_reply(
    runtime: &LocalRuntime,
    conversation_ref: &str,
) -> cwr::v2026_09::conversation::RetainedConversation {
    for _ in 0..200 {
        let conversation = runtime.conversation(conversation_ref).unwrap().unwrap();
        if conversation.messages.len() >= 2 {
            return conversation;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("runtime-owned CWR coworker did not publish a retained reply")
}

#[test]
fn text_and_action_commands_record_distinct_sidebar_origins() {
    let temporary = tempfile::tempdir().unwrap();
    let state = runtime_state(&temporary);
    let runtime = lock_state(&state).unwrap().runtime.clone();
    let text = submitted_cwr(&state, None, "action-looking-text", "@cwr--list--coworkers");
    let conversation = wait_for_cwr_reply(&runtime, &text.record.conversation_ref);
    assert_eq!(conversation.sidebar_eligible, Some(true));
    assert!(conversation.is_sidebar_eligible());

    let (client, server) = UnixStream::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    execute_command(
        &state,
        &client_writer(server).unwrap(),
        RuntimeServiceCommand::SubmitAction {
            version: RUNTIME_SERVICE_VERSION,
            request_id: "standalone-action".into(),
            integration_ref: "cwp://example.test/integrations/desktop".into(),
            author_actor_ref: crate::test_refs::actor_ref("person"),
            conversation_ref: None,
            activation: "@cwr--list--coworkers".into(),
            input: serde_json::json!({}),
        },
    )
    .unwrap();
    let response = BufReader::new(client).lines().next().unwrap().unwrap();
    let RuntimeServiceEvent::CommandSucceeded {
        admission: Some(action),
        ..
    } = serde_json::from_str(&response).unwrap()
    else {
        panic!("expected action admission")
    };
    let conversation = wait_for_cwr_reply(&runtime, &action.record.conversation_ref);
    assert_eq!(conversation.sidebar_eligible, Some(false));
    assert!(!conversation.is_sidebar_eligible());
    assert_eq!(conversation.messages.len(), 2);
}
