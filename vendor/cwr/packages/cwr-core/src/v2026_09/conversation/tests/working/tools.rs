use super::*;

fn working_turn_with_tools(
    store: &mut ConversationAdmissionStore,
    conversation: Option<&str>,
    event: &str,
    tools: Vec<crate::adapter::AdapterHostOperation>,
    route_fingerprint: Option<String>,
) -> AdmittedConversationTurn {
    let mut prepared = store
        .prepare(
            &fixture_graph_with_reviewer(),
            &ingress(
                event,
                crate::v2026_09::test_fixture::actor_ref("person"),
                conversation,
                ConversationIngressAddressing::Addressed {
                    actor_refs: vec![crate::v2026_09::test_fixture::actor_ref("helper").into()],
                },
                event,
            ),
            Some(crate::v2026_09::test_fixture::binding_ref("helper")),
        )
        .unwrap();
    if let ConversationPreparationOutcome::New(turn) = &mut prepared {
        let invocation = turn.invocation.as_mut().unwrap();
        invocation.conversation_continuity = true;
        invocation.host_operations = tools;
        if let Some(fingerprint) = route_fingerprint {
            invocation.route_fingerprint = Some(fingerprint);
        }
    }
    let ConversationAdmissionOutcome::Started(turn) = store.commit(prepared).unwrap() else {
        panic!("start")
    };
    *turn
}

#[test]
fn private_history_retains_tool_names_schemas_and_targets_across_preference_and_grant_changes() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("retained.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let native = crate::adapter::AdapterHostOperation {
        name: "cwp.alice--resize--photo".into(),
        description: "Resize a photo".into(),
        input_schema: json!({"type":"object", "required":["width"],
            "properties":{"width":{"type":"integer"}},
            "x-cwr-target":{"coworker_ref":"original-alice", "binding_ref":"original-claude", "contract_id":"width-v1"}}),
        output_schema: json!({"type":"object"}),
    };
    let first = working_turn_with_tools(&mut store, None, "first", vec![native], None);
    let claimed = claim_working(&mut store, &first);
    let original = claimed.adapter_invocation.host_operations.clone();
    assert_eq!(
        original[0].input_schema["x-cwr-model-name"],
        "cwp.alice--resize--photo"
    );
    settle_working(&mut store, &claimed);
    let original_snapshot =
        retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref);
    let changed = crate::adapter::AdapterHostOperation {
        name: "cwp.bob--resize--photo".into(),
        description: "A new preference".into(),
        input_schema: json!({"type":"object", "required":["max_width"]}),
        output_schema: json!({}),
    };
    let next = working_turn_with_tools(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup",
        vec![changed],
        Some("current-live-authorization-changed".into()),
    );
    let followup = claim_working(&mut store, &next);
    assert_eq!(followup.adapter_invocation.host_operations, original);
    assert_ne!(
        followup.adapter_invocation.route_fingerprint,
        claimed.adapter_invocation.route_fingerprint
    );
    assert_eq!(
        followup.adapter_invocation.provider_fingerprint,
        claimed.adapter_invocation.provider_fingerprint
    );
    assert_eq!(
        retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref),
        original_snapshot
    );
}

#[test]
fn private_followup_keeps_tools_and_freezes_its_new_directory_at_execution_start() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = ConversationAdmissionStore::open(
        directory.path().join("directory.sqlite"),
        "cwp://com.example.runtime",
    )
    .unwrap();
    let tool = crate::adapter::AdapterHostOperation {
        name: "cwr.list_actions".into(),
        description: "Discover actions".into(),
        input_schema: json!({"type":"object"}),
        output_schema: json!({"type":"object"}),
    };
    let first = working_turn_with_tools(&mut store, None, "first", vec![tool], None);
    let mut claimed = claim_working(&mut store, &first);
    claimed.adapter_invocation.thread["coworkers"] = json!({"coworkers":["first-directory"]});
    store
        .retain_execution_invocation(
            &claimed.adapter_invocation.invocation_ref,
            &claimed.adapter_invocation,
        )
        .unwrap();
    settle_working(&mut store, &claimed);
    let next = working_turn_with_tools(
        &mut store,
        Some(&first.record.conversation_ref),
        "followup",
        vec![],
        None,
    );
    let mut followup = claim_working(&mut store, &next);
    assert_eq!(
        followup.adapter_invocation.host_operations,
        claimed.adapter_invocation.host_operations
    );
    assert!(!followup.adapter_invocation.host_operations.is_empty());
    followup.adapter_invocation.thread["coworkers"] = json!({"coworkers":["updated-directory"]});
    store
        .retain_execution_invocation(
            &followup.adapter_invocation.invocation_ref,
            &followup.adapter_invocation,
        )
        .unwrap();
    let mut changed = followup.adapter_invocation.clone();
    changed.thread["coworkers"] = json!({"coworkers":["later-directory"]});
    assert!(store
        .retain_execution_invocation(&changed.invocation_ref, &changed)
        .unwrap_err()
        .to_string()
        .contains("recorded conversation context"));
    let retained = retained_context_snapshot(&store, &followup.adapter_invocation.invocation_ref);
    assert_eq!(
        retained["thread"]["coworkers"],
        followup.adapter_invocation.thread["coworkers"]
    );
    assert_eq!(
        retained_context_snapshot(&store, &claimed.adapter_invocation.invocation_ref)["thread"]
            ["coworkers"],
        claimed.adapter_invocation.thread["coworkers"]
    );
}

