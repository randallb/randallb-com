#[test]
fn execution_status_publication_checks_unpublished_ancestor_calls_once() {
    use crate::adapter::{ExecutionActivity, ExecutionPhase};
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent, child) = detached_parent_and_child(&runtime);
    let owner = runtime
        .conversation_ref_for_invocation(&parent)
        .unwrap()
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .claim_invocation(&child, 20)
        .unwrap();
    let changed = BTreeSet::from([child.clone()]);
    let previous = runtime
        .conversation_host()
        .turn_status_publication_for_execution(&changed, &BTreeSet::new(), &BTreeMap::new())
        .unwrap();
    // A retained host call can create an independent root without emitting a
    // callback before the caller invokes a nested coworker. Capture its call
    // before completing the effect to model independently scheduled child work.
    let create = runtime
        .lock()
        .unwrap()
        .admission
        .append_execution_activity(
            &parent,
            ConversationActivityEvent::HostCall {
                call_id: "unpublished-independent-create".into(),
                operation: "cwr.create_conversation".into(),
                input: json!({}),
            },
        )
        .unwrap();
    let at_call = runtime
        .conversation_host()
        .turn_status_publication_for_execution(
            &changed,
            &BTreeSet::new(),
            &previous.activity_cursors,
        )
        .unwrap();
    assert!(at_call.replace_all);
    assert_eq!(
        at_call.activity_cursors[&parent].as_deref(),
        Some(create.activity_ref.as_str())
    );
    let independent = runtime
        .admit_text(None, "@helper independent pending root")
        .unwrap();
    let result = runtime
        .lock()
        .unwrap()
        .admission
        .append_execution_activity(
            &parent,
            ConversationActivityEvent::HostResult {
                call_id: "unpublished-independent-create".into(),
                result: Some(json!({})),
                error: None,
            },
        )
        .unwrap();
    let publication = runtime
        .conversation_host()
        .turn_status_publication_for_execution(
            &changed,
            &BTreeSet::new(),
            &at_call.activity_cursors,
        )
        .unwrap();
    assert!(
        publication.replace_all,
        "completion after the consumed call must refresh its independent effect"
    );
    assert_eq!(
        publication.statuses,
        runtime.conversation_turn_statuses().unwrap()
    );
    assert!(publication
        .statuses
        .contains_key(independent.conversation_ref()));
    assert_eq!(
        publication.activity_cursors[&parent].as_deref(),
        Some(result.activity_ref.as_str())
    );
    assert!(publication.activity_cursors.contains_key(&child));
    assert_eq!(publication.activity_cursors.len(), 2);
    let child_activity = runtime
        .lock()
        .unwrap()
        .admission
        .append_execution_activity(
            &child,
            ConversationActivityEvent::ExecutionActivity {
                activity: ExecutionActivity::started(ExecutionPhase::Thinking),
            },
        )
        .unwrap();
    let (next, work) = measure_turn_status_projection(|| {
        runtime
            .conversation_host()
            .turn_status_publication_for_execution(
                &changed,
                &BTreeSet::new(),
                &publication.activity_cursors,
            )
            .unwrap()
    });
    assert!(
        !next.replace_all,
        "published ancestor effects must not repeat a full refresh"
    );
    assert!(!next.statuses.contains_key(independent.conversation_ref()));
    assert_eq!(
        next.statuses[&owner],
        runtime.conversation_turn_statuses().unwrap()[&owner]
    );
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].0, owner);
    assert_eq!(
        next.activity_cursors[&parent],
        publication.activity_cursors[&parent]
    );
    assert_eq!(
        next.activity_cursors[&child].as_deref(),
        Some(child_activity.activity_ref.as_str())
    );

    // A cursor from an unavailable snapshot must replay the related ancestor
    // inspection rather than silently skipping its independent effects.
    let mut unknown = next.activity_cursors;
    unknown.insert(
        parent,
        Some("cwp://example.test/activities/unknown-cursor".into()),
    );
    let replay = runtime
        .conversation_host()
        .turn_status_publication_for_execution(&changed, &BTreeSet::new(), &unknown)
        .unwrap();
    assert!(replay.replace_all);
    assert!(replay.statuses.contains_key(independent.conversation_ref()));
}

#[test]
fn execution_status_publication_guard_does_not_check_other_roots_in_same_conversation() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let current = runtime.admit_text(None, "@helper current worker").unwrap();
    let other = runtime
        .admit_text(Some(current.conversation_ref()), "@cwr other root")
        .unwrap();
    let unrelated = runtime.admit_text(None, "@helper unrelated").unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .append_execution_activity(
            other.invocation_ref().unwrap(),
            ConversationActivityEvent::HostCall {
                call_id: "different-root-independent-create".into(),
                operation: "cwr.create_conversation".into(),
                input: json!({}),
            },
        )
        .unwrap();
    let current_ref = current.invocation_ref().unwrap().to_owned();
    let (publication, work) = measure_turn_status_projection(|| {
        runtime
            .conversation_host()
            .turn_status_publication_for_execution(
                &BTreeSet::from([current_ref.clone()]),
                &BTreeSet::from([current.conversation_ref().to_owned()]),
                &BTreeMap::new(),
            )
            .unwrap()
    });
    assert!(!publication.replace_all);
    assert_eq!(publication.activity_cursors.len(), 1);
    assert!(publication.activity_cursors.contains_key(&current_ref));
    assert!(!publication
        .activity_cursors
        .contains_key(other.invocation_ref().unwrap()));
    assert!(!publication
        .statuses
        .contains_key(unrelated.conversation_ref()));
    assert_eq!(publication.statuses[current.conversation_ref()].len(), 2);
    assert_eq!(work.len(), 1);
    assert_eq!(work[0].0, current.conversation_ref());
}

#[test]
fn execution_status_publication_reads_and_advances_query_only_ancestry_without_full_refresh() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent, child) = detached_parent_and_child(&runtime);
    let changed = BTreeSet::from([child]);
    let previous = runtime
        .conversation_host()
        .turn_status_publication_for_execution(&changed, &BTreeSet::new(), &BTreeMap::new())
        .unwrap();
    for (index, operation) in [
        "cwr.list_coworkers",
        "cwr.list_actions",
        "cwr.list_conversation_contexts",
        "cwr.read_file",
    ]
    .into_iter()
    .enumerate()
    {
        let call = runtime
            .lock()
            .unwrap()
            .admission
            .append_execution_activity(
                &parent,
                ConversationActivityEvent::HostCall {
                    call_id: format!("query-only-{index}"),
                    operation: operation.into(),
                    input: json!({}),
                },
            )
            .unwrap();
        let at_call = runtime
            .conversation_host()
            .turn_status_publication_for_execution(
                &changed,
                &BTreeSet::new(),
                &previous.activity_cursors,
            )
            .unwrap();
        assert!(!at_call.replace_all);
        assert_eq!(
            at_call.activity_cursors[&parent].as_deref(),
            Some(call.activity_ref.as_str())
        );
        let result = runtime
            .lock()
            .unwrap()
            .admission
            .append_execution_activity(
                &parent,
                ConversationActivityEvent::HostResult {
                    call_id: format!("query-only-{index}"),
                    result: Some(json!({})),
                    error: None,
                },
            )
            .unwrap();
        let publication = runtime
            .conversation_host()
            .turn_status_publication_for_execution(
                &changed,
                &BTreeSet::new(),
                &at_call.activity_cursors,
            )
            .unwrap();
        assert!(!publication.replace_all);
        assert_eq!(
            publication.activity_cursors[&parent].as_deref(),
            Some(result.activity_ref.as_str())
        );
    }
}

#[test]
fn execution_status_publication_conservatively_refreshes_unmatched_host_results() {
    let temporary = tempfile::tempdir().unwrap();
    let runtime = LocalRuntime::open_with_state_path(
        fixture_workspace(),
        temporary.path().join("state.sqlite"),
        fixture_host_identity(),
    )
    .unwrap();
    let (parent, child) = detached_parent_and_child(&runtime);
    let changed = BTreeSet::from([child]);
    let previous = runtime
        .conversation_host()
        .turn_status_publication_for_execution(&changed, &BTreeSet::new(), &BTreeMap::new())
        .unwrap();
    runtime
        .lock()
        .unwrap()
        .admission
        .append_execution_activity(
            &parent,
            ConversationActivityEvent::HostResult {
                call_id: "unavailable-call".into(),
                result: Some(json!({})),
                error: None,
            },
        )
        .unwrap();
    let publication = runtime
        .conversation_host()
        .turn_status_publication_for_execution(
            &changed,
            &BTreeSet::new(),
            &previous.activity_cursors,
        )
        .unwrap();
    assert!(publication.replace_all);
}
