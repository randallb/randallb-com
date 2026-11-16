#[test]
fn legacy_source_retry_rejects_changed_files_before_upgrading_identity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        directory.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let integration = "cwp://example.test/integrations/test";
    let request = "legacy-source-files";
    let admitted = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                integration_ref: integration.into(),
                provider_event_ref: request.into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Unaddressed,
                text: "Original".into(),
                file_refs: Vec::new(),
            },
            None,
        )
        .unwrap();
    assert!(admitted.record.source_input_fingerprint.is_none());
    let file = runtime
        .ingest_file_as(author, "image.png", "image/png", b"image bytes")
        .unwrap();
    assert!(runtime
        .submit_message_in_upstream_scope(
            integration,
            request,
            author,
            None,
            "Original",
            vec![file.file_ref],
            None
        )
        .is_err());
    assert!(runtime
        .admission_record(integration, request)
        .unwrap()
        .unwrap()
        .source_input_fingerprint
        .is_none());
    let replay = runtime
        .submit_text_as(integration, request, author, None, "Original")
        .unwrap();
    assert_eq!(
        replay.record.source_message_ref,
        admitted.record.source_message_ref
    );
    assert!(replay.record.source_input_fingerprint.is_some());
    assert!(replay.conversation.messages[0].file_refs.is_empty());
}

#[test]
fn attachment_host_read_checks_current_actor_without_retaining_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        directory.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let file = runtime
        .ingest_file_as(author, "image.png", "image/png", b"image bytes")
        .unwrap();
    let private = runtime
        .ingest_file_as(author, "private.png", "image/png", b"private bytes")
        .unwrap();
    let admission = runtime
        .submit_message_in_upstream_scope(
            "cwp://example.test/integrations/upload",
            "image-question",
            author,
            None,
            "@helper inspect this image",
            vec![file.file_ref.clone()],
            None,
        )
        .unwrap();
    let invocation_ref = admission.invocation_ref().unwrap();
    let caller = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) = state
            .admission
            .claim_invocation(invocation_ref, 10)
            .unwrap()
        else {
            panic!("claim")
        };
        claim.claim.invocation
    };
    let mut call = AdapterCall {
        invocation_ref: invocation_ref.into(),
        call_id: "image-read".into(),
        operation: READ_FILE_HOST_OPERATION.into(),
        input: json!({"file_ref":file.file_ref}),
    };
    let read = |call: AdapterCall| {
        runtime.handle_host_call(
            call,
            &caller,
            &Rc::new(RefCell::new(VecDeque::new())),
            &AdapterModelOutputMailbox::default(),
            &mut |_| {},
        )
    };
    let AdapterHostCallOutcome::Returned(result) = read(call.clone()).unwrap();
    assert_eq!(result["content_hex"], hex::encode(b"image bytes"));
    call.input = json!({"file_ref":private.file_ref});
    assert!(read(call.clone()).is_err());
    call.input = json!({"file_ref":file.file_ref,"actor_ref":author});
    assert!(read(call).is_err());
    let state = runtime.lock().unwrap();
    assert!(!state.admission.execution_conversation(invocation_ref).unwrap().activity.iter().any(|activity| matches!(&activity.event, ConversationActivityEvent::HostCall { operation, .. } if operation == READ_FILE_HOST_OPERATION)));
}

#[test]
fn older_codex_conversations_pick_up_image_reading_once_and_then_resume() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        directory.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let (first, mut old) = claim_gateway_turn(&runtime, None);
    assert!(old
        .host_operations
        .iter()
        .any(|tool| tool.name == READ_FILE_HOST_OPERATION));
    old.host_operations
        .retain(|tool| tool.name != READ_FILE_HOST_OPERATION);
    runtime
        .lock()
        .unwrap()
        .admission
        .retain_execution_invocation(&first.invocation_ref, &old)
        .unwrap();
    settle_gateway_turn(&runtime, &first, &old);
    let (second, upgraded) = claim_gateway_turn(&runtime, first.conversation_ref.as_deref());
    let context = crate::adapter::working_context(&upgraded.thread).unwrap();
    assert!(context["working_conversation"]["state"].is_null());
    assert!(context["working_conversation"]["history_gap"]
        .as_str()
        .unwrap()
        .contains("host tools changed"));
    assert!(upgraded
        .host_operations
        .iter()
        .any(|tool| tool.name == READ_FILE_HOST_OPERATION));
    settle_gateway_turn(&runtime, &second, &upgraded);
    let (_, resumed) = claim_gateway_turn(&runtime, second.conversation_ref.as_deref());
    let resumed_context = crate::adapter::working_context(&resumed.thread).unwrap();
    assert_eq!(
        serde_json::to_value(&resumed.host_operations).unwrap(),
        serde_json::to_value(&upgraded.host_operations).unwrap(),
        "resuming must preserve the exact provider tool configuration"
    );
    assert_eq!(
        resumed_context["working_conversation"]["state"]["thread_id"],
        "gateway-thread"
    );
    assert_eq!(
        resumed_context["working_conversation"]["ref"],
        context["working_conversation"]["ref"]
    );
}

#[test]
fn legacy_source_retry_preserves_original_files_and_upgrades_identity() {
    let directory = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        directory.path().join("runtime.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let author = crate::v2026_09::test_fixture::actor_ref("person");
    let file = runtime
        .ingest_file_as(author, "image.png", "image/png", b"image bytes")
        .unwrap();
    let admitted = runtime
        .admit_ingress(
            ConversationIngress {
                owner_context_ref: None,
                integration_ref: "cwp://example.test/integrations/retry".into(),
                provider_event_ref: "legacy-image".into(),
                author_actor_ref: author.into(),
                conversation: ConversationIngressConversation::Create,
                addressing: ConversationIngressAddressing::Unaddressed,
                text: "Look".into(),
                file_refs: vec![file.file_ref.clone()],
            },
            None,
        )
        .unwrap();
    assert!(admitted.record.source_input_fingerprint.is_none());
    let retried = runtime
        .submit_message_in_upstream_scope(
            "cwp://example.test/integrations/retry",
            "legacy-image",
            author,
            None,
            "Look",
            vec![file.file_ref.clone()],
            None,
        )
        .unwrap();
    assert_eq!(
        retried.record.source_message_ref,
        admitted.record.source_message_ref
    );
    assert!(retried.record.source_input_fingerprint.is_some());
    assert_eq!(
        retried.conversation.messages[0].file_refs,
        vec![file.file_ref]
    );
}

#[test]
fn codex_resume_after_reopen_preserves_the_provider_tool_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let state_path = directory.path().join("runtime.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &state_path, fixture_host_identity(),
    ).unwrap();
    let (first, original) = claim_gateway_turn(&runtime, None);
    settle_gateway_turn(&runtime, &first, &original);
    drop(runtime);

    let reopened = LocalRuntime::open_with_state_path(
        fixture_workspace(), &state_path, fixture_host_identity(),
    ).unwrap();
    let (_, resumed) = claim_gateway_turn(&reopened, first.conversation_ref.as_deref());
    let context = crate::adapter::working_context(&resumed.thread).unwrap();
    assert_eq!(context["working_conversation"]["state"]["thread_id"], "gateway-thread");
    assert_eq!(
        serde_json::to_value(&resumed.host_operations).unwrap(),
        serde_json::to_value(&original.host_operations).unwrap(),
        "reopening must preserve tool names, schemas, and exact targets"
    );
}

#[test]
fn invocation_image_readers_follow_request_chain_and_survive_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("runtime.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    let (parent, child) = detached_parent_and_child(&runtime);
    let queued_requester = runtime.lock().unwrap().admission.invocation_record(&child).unwrap().requester_actor_ref.clone();
    assert!(runtime.ingest_invocation_file_as(&queued_requester, &child, "image.png", "image/png", b"image").is_err());
    let (requester, ancestor) = {
        let mut state = runtime.lock().unwrap();
        let ConversationInvocationClaimOutcome::Claimed(claim) =
            state.admission.claim_invocation(&child, 30).unwrap()
        else { panic!("claim child") };
        (claim.claim.invocation.requester_actor_ref,
         state.admission.invocation_record(&parent).unwrap().requester_actor_ref.clone())
    };
    let unrelated = "cwp://example.test/actors/unrelated";
    assert!(runtime.ingest_invocation_file_as(unrelated, &child, "image.png", "image/png", b"image").is_err());
    assert!(runtime.ingest_invocation_file_as(&requester, "cwp://example.test/invocations/missing", "image.png", "image/png", b"image").is_err());
    let file = runtime.ingest_invocation_file_as(&requester, &child, "image.png", "image/png", b"image").unwrap();
    {
        let state = runtime.lock().unwrap();
        assert!(state.admission.read_file(&requester, &file.file_ref).is_ok());
        assert!(state.admission.read_file(&ancestor, &file.file_ref).is_ok());
        assert!(state.admission.read_file(unrelated, &file.file_ref).is_err());
    }
    runtime.stop_invocation(&parent, Some("stop image request".into())).unwrap();
    assert!(runtime.ingest_invocation_file_as(&requester, &child, "image.png", "image/png", b"image").is_err());
    drop(runtime);
    let reopened = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    let state = reopened.lock().unwrap();
    assert!(state.admission.read_file(&ancestor, &file.file_ref).is_ok());
    assert!(state.admission.read_file(unrelated, &file.file_ref).is_err());
}
