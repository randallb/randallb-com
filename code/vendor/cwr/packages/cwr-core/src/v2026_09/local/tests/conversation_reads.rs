#[test]
fn conversation_reads_are_scoped_to_the_requesting_participant() {
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let helper = participation_ref(&runtime, "helper");
    let admission = runtime.admit_text(None, "private note").unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);

    let visible = runtime
        .participant_root_conversation_summaries(
            &person,
            &default_conversation_summary_page(),
            None,
            deadline,
        )
        .unwrap();
    assert_eq!(visible.total_count, 1);
    assert_eq!(
        visible.summaries[0].key.conversation_ref,
        admission.conversation_ref()
    );
    let hidden = runtime
        .participant_root_conversation_summaries(
            &helper,
            &default_conversation_summary_page(),
            None,
            deadline,
        )
        .unwrap();
    assert_eq!(hidden.total_count, 0);

    let records = runtime
        .participant_conversation_timeline(
            admission.conversation_ref(),
            &person,
            &default_conversation_record_page(),
            deadline,
        )
        .unwrap();
    assert_eq!(records.total_count, 1);
    let error = runtime
        .participant_conversation_timeline(
            admission.conversation_ref(),
            &helper,
            &default_conversation_record_page(),
            deadline,
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("unavailable to the requesting participant"));

    let browse_request = crate::intrinsic_cwr::default_conversation_browse_request();
    let browsed = runtime
        .participant_conversation_browse(
            admission.conversation_ref(),
            &person,
            &browse_request,
            deadline,
        )
        .unwrap();
    let crate::intrinsic_cwr::ConversationBrowseRecords::Page { page } = browsed.records else {
        panic!("recent browsing returns a message page")
    };
    assert_eq!(page.content.len(), 1);
    let error = runtime
        .participant_conversation_browse(
            admission.conversation_ref(),
            &helper,
            &browse_request,
            deadline,
        )
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("unavailable to the requesting participant"));
}

#[test]
fn conversation_browse_deadline_includes_runtime_lock_wait() {
    let (_temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let admission = runtime.admit_text(None, "private note").unwrap();
    let held_state = runtime.inner.lock().unwrap();
    let error = runtime
        .participant_conversation_browse(
            admission.conversation_ref(),
            &person,
            &crate::intrinsic_cwr::default_conversation_browse_request(),
            Instant::now() + Duration::from_millis(20),
        )
        .unwrap_err();
    assert!(error.to_string().contains("exceeded its deadline"));
    drop(held_state);
}

#[test]
fn oversized_browse_records_one_private_diagnostic_for_the_offending_message() {
    let (temporary, runtime) = participation_runtime();
    let person = participation_ref(&runtime, "person");
    let admission = runtime
        .admit_text(
            None,
            format!("private-message-marker{}", "x".repeat(8 * 1024 * 1024)),
        )
        .unwrap();
    let message_ref = admission.conversation.messages[0].message_ref.clone();
    let payload_bytes = crate::cwpdb::test_support::conversation_observations::message_payload_bytes(
        &temporary.path().join("runtime.sqlite"), &message_ref,
    ).unwrap();
    let error = runtime
        .participant_conversation_browse_with_context(
            admission.conversation_ref(),
            &person,
            &crate::intrinsic_cwr::default_conversation_browse_request(),
            Instant::now() + Duration::from_secs(10),
            BrowseDiagnosticContext {
                invocation_ref: Some("cwp://example.test/invocations/browse"),
                request_id: Some("Authorization: Bearer request-private-value"),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("8 MiB browse limit"));
    assert!(!error.to_string().contains("private-message-marker"));
    drop(runtime);

    let mut db = crate::cwpdb::NativeStore::open_read_only(temporary.path().join("diagnostics/wlpdb.sqlite3")).unwrap();
    let page = db.diagnostic_records(None, 10).unwrap();
    assert!(!page.truncated);
    assert_eq!(page.records.len(), 1);
    let record = &page.records[0].value;
    assert_eq!(record["source"], "conversation_browse_overage");
    assert_eq!(record["conversation_ref"], admission.conversation_ref());
    assert_eq!(record["message_ref"], message_ref);
    assert_eq!(record["operation"], "conversation_browse_page");
    assert_eq!(record["payload_bytes"], payload_bytes);
    assert_eq!(record["limit_bytes"], 8 * 1024 * 1024);
    assert_eq!(record["invocation_ref"], "cwp://example.test/invocations/browse");
    assert!(!serde_json::to_string(record).unwrap().contains("private-message-marker"));
    assert!(!serde_json::to_string(record).unwrap().contains("request-private-value"));
}

#[test]
fn sidebar_eligibility_survives_actions_and_restart_without_losing_direct_history() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("sidebar.sqlite");
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    let person = runtime.submit_text_as(
        "cwp://example.test/ingress", "explicit-create", crate::v2026_09::test_fixture::actor_ref("person"),
        None, "@cwr--list--coworkers",
    ).unwrap();
    let action = runtime.admit_action(None, "@cwr--list--coworkers", json!({})).unwrap();
    let helper = crate::v2026_09::test_fixture::actor_ref("helper");
    let explicit = runtime.submit_text_as(
        "cwp://example.test/ingress", "explicit-bound-author", helper, None, "Explicit host note",
    ).unwrap();
    runtime.admit_action(Some(person.conversation_ref()), "@cwr--list--coworkers", json!({})).unwrap();
    assert!(person.conversation.is_sidebar_eligible());
    assert!(!action.conversation.is_sidebar_eligible());
    assert!(explicit.conversation.is_sidebar_eligible());
    let exchange = runtime.record_host_exchange(None, helper, "cwp://example.test/host",
        "Explicit host request".into(), "Done".into()).unwrap();
    assert!(exchange.conversation.is_sidebar_eligible());
    let continued = runtime.record_host_exchange(Some(action.conversation_ref()), helper,
        "cwp://example.test/host", "Follow-up".into(), "Done".into()).unwrap();
    assert!(!continued.conversation.is_sidebar_eligible());
    while let Some(mut task) = runtime.try_schedule_execution().unwrap() {
        task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
    }
    drop(runtime);

    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(), &path, fixture_host_identity(),
    ).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let page = runtime.root_conversation_summaries(
        &default_conversation_summary_page(), deadline, || false, |_| {},
    ).unwrap();
    assert_eq!(page.total_count, 3);
    assert!(page.summaries.iter().any(|summary| summary.key.conversation_ref == person.conversation_ref()));
    assert!(!page.summaries.iter().any(|summary| summary.key.conversation_ref == action.conversation_ref()));
    let bootstrap = runtime.host_conversation_bootstrap(false).unwrap();
    assert_eq!(bootstrap.iter().filter(|c| c.is_sidebar_eligible()).count(), 3);
    let history = runtime.participant_conversation_timeline(
        action.conversation_ref(), crate::v2026_09::test_fixture::actor_ref("person"),
        &default_conversation_record_page(), deadline,
    ).unwrap();
    assert!(history.messages.iter().any(|message| matches!(
        &message.action, Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. })
    )));
    assert!(runtime.conversation_ref_for_invocation(action.invocation_ref().unwrap()).unwrap().is_some());
}

#[test]
fn all_retained_data_reads_cross_participation_without_changing_membership() {
    let (_temporary, runtime) = participation_runtime();
    let helper = participation_ref(&runtime, "helper");
    let root = runtime.admit_text(None, "Private grading fixture").unwrap();
    let admission = runtime.admit_action(Some(root.conversation_ref()), "@cwr--list--coworkers", json!({})).unwrap();
    let invocation_ref = admission.invocation_ref().unwrap().to_owned();
    let before = runtime.lock().unwrap().admission.conversation(admission.conversation_ref()).unwrap().participant_actor_refs.clone();
    assert!(!before.contains(&helper));
    let deadline = || Instant::now() + Duration::from_secs(5);
    assert!(runtime.inspect_invocation(&invocation_ref).unwrap().original_invocation.is_none());
    {
        let mut state = runtime.lock().unwrap();
        state.graph.permissions.read_retained_data = crate::v2026_09::graph::RetainedDataReadAccess::All;
    }
    while let Some(mut task) = runtime.try_schedule_execution().unwrap() {
        task.execute_with_model_output(&AdapterModelOutputMailbox::discard(), |_| {}).unwrap();
    }
    let listed = runtime.participant_root_conversation_summaries(&helper, &default_conversation_summary_page(), None, deadline()).unwrap();
    assert_eq!(listed.total_count, 1);
    let timeline = runtime.participant_conversation_timeline(admission.conversation_ref(), &helper, &default_conversation_record_page(), deadline()).unwrap();
    assert!(timeline.messages.iter().any(|message| matches!(&message.action, Some(crate::v2026_09::conversation::ConversationActionEvent::Result { .. }))));
    runtime.participant_conversation_browse(admission.conversation_ref(), &helper, &crate::intrinsic_cwr::default_conversation_browse_request(), deadline()).unwrap();
    let report = runtime.inspect_invocation(&invocation_ref).unwrap();
    let original = report.original_invocation.unwrap();
    assert_eq!(original.action_name, "list--coworkers");
    assert!(!original.truncated);
    let full: Value = serde_json::from_str(&original.json_excerpt).unwrap();
    assert!(!full["responses"].as_array().unwrap().is_empty());
    let mut large = runtime.lock().unwrap().admission.invocation_record(&invocation_ref).unwrap().clone();
    large.input = json!({"huge": "💡".repeat(10_000)});
    let excerpt = crate::v2026_09::conversation::InvocationRecordExcerpt::new(&large).unwrap();
    assert!(excerpt.truncated);
    assert!(excerpt.json_excerpt.len() <= 8192);
    assert_eq!(excerpt.source_message_ref, large.source_message_ref);
    assert_eq!(excerpt.parent_invocation_ref, large.parent_invocation_ref);
    large.input = json!({});
    large.responses[0].result = Some(json!({"huge": "r".repeat(30_000)}));
    large.responses = vec![large.responses[0].clone(); 30];
    let excerpt = crate::v2026_09::conversation::InvocationRecordExcerpt::new(&large).unwrap();
    assert!(excerpt.truncated);
    assert!(excerpt.json_excerpt.len() <= 8192);
    runtime.participant_conversation_timeline(&report.execution_conversation_ref, &helper, &default_conversation_record_page(), deadline()).unwrap();
    runtime.participant_conversation_browse(&report.execution_conversation_ref, &helper, &crate::intrinsic_cwr::default_conversation_browse_request(), deadline()).unwrap();
    let state = runtime.lock().unwrap();
    assert_eq!(state.admission.conversation(admission.conversation_ref()).unwrap().participant_actor_refs, before);
    drop(state);
    runtime.lock().unwrap().graph.permissions.read_retained_data = crate::v2026_09::graph::RetainedDataReadAccess::Participants;
    assert!(runtime.participant_conversation_timeline(admission.conversation_ref(), &helper, &default_conversation_record_page(), deadline()).is_err());
    assert!(runtime.participant_conversation_timeline(&report.execution_conversation_ref, &helper, &default_conversation_record_page(), deadline()).is_err());
    assert!(runtime.inspect_invocation(&invocation_ref).unwrap().original_invocation.is_none());
}

#[test]
fn finite_timeline_releases_runtime_lock_registers_cancellation_and_measures_storage_work(){
    let (_temporary,runtime)=participation_runtime();let person=participation_ref(&runtime,"person");let admission=runtime.admit_text(None,"finite timeline source").unwrap();
    let deadline=||Instant::now()+Duration::from_secs(5);
    let error=runtime.participant_conversation_timeline_cancellable(admission.conversation_ref(),&person,&default_conversation_record_page(),deadline(),||false,|cancellation|{
        assert!(runtime.inner.try_lock().is_ok(),"cancellation registration must run after releasing the runtime lock");cancellation.cancel();
    }).unwrap_err();assert!(error.to_string().contains("cancelled"));
    let result=runtime.participant_conversation_timeline(admission.conversation_ref(),&person,&default_conversation_record_page(),deadline());
    assert_eq!(result.unwrap().total_count,1);
}
